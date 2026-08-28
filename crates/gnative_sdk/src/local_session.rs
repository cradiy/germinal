use std::{
    cmp::Ordering,
    collections::BinaryHeap,
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    thread,
};

use germinal_gnative_protocol::gnative::{
    frame::GNativeFrame,
    input::GNativeInputEvent,
    media::{GNativeAudioPacket, GNativeMediaControlCommand, GNativeVideoPacket},
    session::{GNativeAppHello, GNativeSessionAccepted},
    tunnel::{
        GNATIVE_MAX_MESSAGE_BYTES, GNativeAppMuxFrame, GNativeAppPayload, GNativeAppToHost,
        GNativeHostPayload, GNativeHostToApp, GNativeStreamPriority,
    },
};

use crate::{
    control_sequence::{GNativeTunnelEnv, write_enter_control_sequence},
    error::{GNativeSdkError, GNativeSdkResult},
};

const TCP_ENDPOINT_PREFIX: &str = "tcp://";
const OUTBOUND_QUEUE_CAPACITY: usize = 256;
const MAX_OUTBOUND_BATCH: usize = 256;

pub struct LocalGNativeTunnelBootstrap {
    tunnel_env: GNativeTunnelEnv,
}

impl LocalGNativeTunnelBootstrap {
    pub fn from_env() -> GNativeSdkResult<Self> {
        Ok(Self {
            tunnel_env: GNativeTunnelEnv::from_env()?,
        })
    }

    pub fn from_tunnel_env(tunnel_env: GNativeTunnelEnv) -> Self {
        Self { tunnel_env }
    }

    pub fn tunnel_env(&self) -> &GNativeTunnelEnv {
        &self.tunnel_env
    }

    pub fn write_enter_control_sequence<W: std::io::Write>(
        &self,
        writer: &mut W,
    ) -> std::io::Result<()> {
        write_enter_control_sequence(writer, &self.tunnel_env)
    }

    pub fn connect(self) -> GNativeSdkResult<LocalGNativeSession> {
        let mut stream = TcpStream::connect(strip_tcp_endpoint_prefix(&self.tunnel_env.endpoint))?;
        stream.set_nodelay(true).ok();
        write_app_message(
            &mut stream,
            &GNativeAppToHost::Hello(GNativeAppHello {
                token: self.tunnel_env.token,
                protocol_version: self.tunnel_env.protocol_version,
            }),
        )?;

        let welcome = read_host_message(&mut BufReader::new(stream.try_clone()?))?
            .ok_or(GNativeSdkError::HostClosedBeforeWelcome)?;
        let GNativeHostToApp::Welcome(accepted) = welcome else {
            return Err(GNativeSdkError::ExpectedWelcomeAfterHello);
        };

        let writer_stream = stream.try_clone()?;
        let (queue_tx, queue_rx) = flume::bounded(OUTBOUND_QUEUE_CAPACITY);
        spawn_outbound_writer(writer_stream, queue_rx)?;

        Ok(LocalGNativeSession {
            accepted,
            reader: BufReader::new(stream),
            outbound: LocalGNativeOutbound {
                queue_tx,
                next_mux_seq: Arc::new(AtomicU64::new(1)),
            },
        })
    }
}

pub struct LocalGNativeSession {
    accepted: GNativeSessionAccepted,
    reader: BufReader<TcpStream>,
    outbound: LocalGNativeOutbound,
}

impl LocalGNativeSession {
    pub fn accepted(&self) -> &GNativeSessionAccepted {
        &self.accepted
    }

    pub fn frame_writer(&self) -> LocalGNativeFrameWriter {
        LocalGNativeFrameWriter {
            accepted: self.accepted.clone(),
            outbound: self.outbound.clone(),
        }
    }

    pub fn read_input(&mut self) -> GNativeSdkResult<Option<GNativeInputEvent>> {
        match read_host_message(&mut self.reader)? {
            Some(GNativeHostToApp::Welcome(_)) => Err(GNativeSdkError::UnexpectedDuplicateWelcome),
            Some(GNativeHostToApp::Mux(frame)) => match frame.payload {
                GNativeHostPayload::Input(input) => Ok(Some(input)),
            },
            None => Ok(None),
        }
    }

    pub fn send_exit(&mut self) -> GNativeSdkResult<()> {
        self.outbound.send_exit()
    }
}

pub struct LocalGNativeFrameWriter {
    accepted: GNativeSessionAccepted,
    outbound: LocalGNativeOutbound,
}

impl LocalGNativeFrameWriter {
    pub fn send_frame(&mut self, frame: GNativeFrame) -> GNativeSdkResult<()> {
        if frame.gshell_id != self.accepted.gshell_id {
            return Err(GNativeSdkError::FrameGshellMismatch);
        }
        self.outbound.send_payload(GNativeAppPayload::Render(frame))
    }

    pub fn send_control(&mut self, command: GNativeMediaControlCommand) -> GNativeSdkResult<()> {
        self.outbound
            .send_payload(GNativeAppPayload::Control(command))
    }

    pub fn send_audio_packet(&mut self, packet: GNativeAudioPacket) -> GNativeSdkResult<()> {
        self.outbound.send_payload(GNativeAppPayload::Audio(packet))
    }

    pub fn send_video_packet(&mut self, packet: GNativeVideoPacket) -> GNativeSdkResult<()> {
        self.outbound.send_payload(GNativeAppPayload::Video(packet))
    }

    pub fn send_exit(&mut self) -> GNativeSdkResult<()> {
        self.outbound.send_exit()
    }
}

#[derive(Clone)]
struct LocalGNativeOutbound {
    queue_tx: flume::Sender<QueuedAppMessage>,
    next_mux_seq: Arc<AtomicU64>,
}

impl LocalGNativeOutbound {
    fn send_payload(&self, payload: GNativeAppPayload) -> GNativeSdkResult<()> {
        let mux_seq = self.next_mux_seq.fetch_add(1, AtomicOrdering::Relaxed);
        self.queue_tx
            .send(QueuedAppMessage {
                mux_seq,
                priority: payload.default_priority(),
                message: GNativeAppToHost::Mux(GNativeAppMuxFrame::new(mux_seq, payload)),
            })
            .map_err(|_| GNativeSdkError::OutboundQueueClosed)
    }

    fn send_exit(&self) -> GNativeSdkResult<()> {
        let mux_seq = self.next_mux_seq.fetch_add(1, AtomicOrdering::Relaxed);
        self.queue_tx
            .send(QueuedAppMessage {
                mux_seq,
                priority: GNativeStreamPriority::Critical,
                message: GNativeAppToHost::Exit,
            })
            .map_err(|_| GNativeSdkError::OutboundQueueClosed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueuedAppMessage {
    mux_seq: u64,
    priority: GNativeStreamPriority,
    message: GNativeAppToHost,
}

impl Ord for QueuedAppMessage {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.mux_seq.cmp(&self.mux_seq))
    }
}

impl PartialOrd for QueuedAppMessage {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn spawn_outbound_writer(
    stream: TcpStream,
    queue_rx: flume::Receiver<QueuedAppMessage>,
) -> GNativeSdkResult<()> {
    thread::Builder::new()
        .name("gnative-sdk-outbound".to_string())
        .spawn(move || run_outbound_writer(stream, queue_rx))
        .map(|_| ())
        .map_err(GNativeSdkError::from)
}

fn run_outbound_writer(mut stream: TcpStream, queue_rx: flume::Receiver<QueuedAppMessage>) {
    while let Ok(message) = queue_rx.recv() {
        let mut pending = collect_outbound_batch(message, &queue_rx);

        while let Some(message) = pending.pop() {
            if write_app_message(&mut stream, &message.message).is_err() {
                return;
            }
        }
    }
}

fn collect_outbound_batch(
    first: QueuedAppMessage,
    queue_rx: &flume::Receiver<QueuedAppMessage>,
) -> BinaryHeap<QueuedAppMessage> {
    let mut pending = BinaryHeap::with_capacity(MAX_OUTBOUND_BATCH);
    pending.push(first);

    for _ in 1..MAX_OUTBOUND_BATCH {
        let Ok(message) = queue_rx.try_recv() else {
            break;
        };
        pending.push(message);
    }

    pending
}

fn write_app_message(stream: &mut TcpStream, message: &GNativeAppToHost) -> GNativeSdkResult<()> {
    let payload = serde_json::to_string(message).map_err(GNativeSdkError::EncodeMessage)?;
    stream.write_all(payload.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

fn read_host_message<R: BufRead>(reader: &mut R) -> GNativeSdkResult<Option<GNativeHostToApp>> {
    read_host_message_with_limit(reader, GNATIVE_MAX_MESSAGE_BYTES)
}

fn read_host_message_with_limit<R: BufRead>(
    reader: &mut R,
    max_bytes: usize,
) -> GNativeSdkResult<Option<GNativeHostToApp>> {
    let mut line = Vec::new();
    let bytes_read = reader
        .take(max_bytes.saturating_add(1) as u64)
        .read_until(b'\n', &mut line)?;
    if bytes_read == 0 {
        return Ok(None);
    }
    if line.len() > max_bytes || !line.ends_with(b"\n") {
        return Err(GNativeSdkError::MessageTooLarge { max_bytes });
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    serde_json::from_slice(&line)
        .map(Some)
        .map_err(GNativeSdkError::DecodeMessage)
}

fn strip_tcp_endpoint_prefix(endpoint: &str) -> &str {
    endpoint
        .strip_prefix(TCP_ENDPOINT_PREFIX)
        .unwrap_or(endpoint)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BinaryHeap,
        io::BufReader,
        sync::{Arc, atomic::AtomicBool, mpsc},
        thread,
        time::Duration,
    };

    use germinal_domain::gshell::vo::gshell_id::GShellId;
    use germinal_gnative_protocol::{
        gnative::{
            frame::GNativeFrame,
            input::{
                GNativeInputEvent, GNativeInputModifiers, GNativePointerPosition,
                GNativeScrollDelta,
            },
            media::GNativeMediaControlCommand,
            session::GNATIVE_PROTOCOL_VERSION,
            tunnel::{GNativeAppPayload, GNativeAppToHost, GNativeStreamPriority},
        },
        rendering::frame_plan_builder::{RenderCommandDto, TextStyleDto},
        seq::Seq,
    };
    use germinal_infra::gnative::tunnel::GNativeTunnel;
    use germinal_ports::{
        event::{
            runtime_event::{GShellRuntimeEvent, RuntimeEvent},
            runtime_event_dispatcher::{IRuntimeEventDispatcher, RuntimeEventDispatchError},
        },
        rendering::surface_snapshot_mailbox::surface_snapshot_mailbox,
        service::gnative_tunnel::IGNativeTunnel,
    };

    use super::{
        LocalGNativeTunnelBootstrap, MAX_OUTBOUND_BATCH, QueuedAppMessage, collect_outbound_batch,
        read_host_message_with_limit,
    };
    use crate::control_sequence::GNativeTunnelEnv;

    #[derive(Clone)]
    struct TestDispatcher(mpsc::Sender<RuntimeEvent>);

    impl IRuntimeEventDispatcher for TestDispatcher {
        fn dispatch(&self, event: RuntimeEvent) -> Result<(), RuntimeEventDispatchError> {
            self.0
                .send(event)
                .map_err(|_| RuntimeEventDispatchError::Closed)?;
            Ok(())
        }
    }

    fn wait_for_connected(event_rx: &mpsc::Receiver<RuntimeEvent>, gshell_id: GShellId) {
        loop {
            let event = event_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("gnative connected event should arrive");
            match event {
                RuntimeEvent::GShell(GShellRuntimeEvent::GNativeConnected { accepted }) => {
                    assert_eq!(accepted.gshell_id, gshell_id);
                    return;
                }
                RuntimeEvent::GShell(GShellRuntimeEvent::GNativeConnectionFailed {
                    reason,
                    ..
                }) => panic!("gnative connection failed: {reason}"),
                _ => {}
            }
        }
    }

    #[test]
    fn bootstrap_reads_viewport_pointer_and_focus_input() {
        let (snapshot_tx, _snapshot_rx) = surface_snapshot_mailbox();
        let (event_tx, event_rx) = mpsc::channel();
        let tunnel = GNativeTunnel::new().expect("tunnel should initialize");
        tunnel.configure(
            TestDispatcher(event_tx),
            Arc::new(AtomicBool::new(false)),
            snapshot_tx,
        );
        let descriptor = tunnel
            .ensure_session_descriptor(GShellId::new(31), GNATIVE_PROTOCOL_VERSION)
            .expect("descriptor should exist");
        let bootstrap = LocalGNativeTunnelBootstrap::from_tunnel_env(GNativeTunnelEnv {
            endpoint: descriptor.endpoint.clone(),
            token: descriptor.token.clone(),
            protocol_version: descriptor.protocol_version,
        });

        let app = thread::spawn(move || {
            let mut session = bootstrap.connect().expect("session should connect host");
            [
                session.read_input().expect("resize should read"),
                session.read_input().expect("pointer input should read"),
                session.read_input().expect("focus input should read"),
            ]
        });

        tunnel
            .begin_accept_session(GShellId::new(31))
            .expect("handshake should begin");
        wait_for_connected(&event_rx, GShellId::new(31));
        let resize = GNativeInputEvent::Resize {
            columns: 80,
            rows: 24,
            content_width_px: 960,
            content_height_px: 576,
            cell_width_px: 12,
            cell_height_px: 24,
        };
        tunnel
            .send_input(GShellId::new(31), resize.clone())
            .expect("host should send input");
        let pointer = GNativeInputEvent::Scroll {
            delta: GNativeScrollDelta::Pixels { x: 0.25, y: -12.5 },
            position: GNativePointerPosition {
                x_px: 41.75,
                y_px: 9.125,
            },
            modifiers: GNativeInputModifiers {
                control: false,
                alt: false,
                shift: true,
                super_key: false,
            },
        };
        tunnel
            .send_input(GShellId::new(31), pointer.clone())
            .expect("host should send pointer input");
        let focus = GNativeInputEvent::FocusChanged(true);
        tunnel
            .send_input(GShellId::new(31), focus.clone())
            .expect("host should send focus input");

        assert_eq!(
            app.join().expect("app thread should join"),
            [Some(resize), Some(pointer), Some(focus)]
        );
    }

    #[test]
    fn frame_writer_sends_frame_back_to_host() {
        let (snapshot_tx, snapshot_rx) = surface_snapshot_mailbox();
        let (event_tx, event_rx) = mpsc::channel();
        let tunnel = GNativeTunnel::new().expect("tunnel should initialize");
        tunnel.configure(
            TestDispatcher(event_tx),
            Arc::new(AtomicBool::new(false)),
            snapshot_tx,
        );
        let descriptor = tunnel
            .ensure_session_descriptor(GShellId::new(32), 1)
            .expect("descriptor should exist");
        let bootstrap = LocalGNativeTunnelBootstrap::from_tunnel_env(GNativeTunnelEnv {
            endpoint: descriptor.endpoint.clone(),
            token: descriptor.token.clone(),
            protocol_version: descriptor.protocol_version,
        });

        let app = thread::spawn(move || {
            let session = bootstrap.connect().expect("session should connect host");
            let mut writer = session.frame_writer();
            writer
                .send_frame(GNativeFrame {
                    gshell_id: session.accepted().gshell_id,
                    seq: Seq::new(9),
                    commands: vec![
                        RenderCommandDto::ClearLine { y: 0 },
                        RenderCommandDto::StyledTextRun {
                            x: 0,
                            y: 0,
                            text: "sdk".to_string(),
                            style: TextStyleDto::plain(),
                        },
                    ],
                    cursor: None,
                })
                .expect("frame should send");
        });

        tunnel
            .begin_accept_session(GShellId::new(32))
            .expect("handshake should begin");
        wait_for_connected(&event_rx, GShellId::new(32));
        let snapshot = snapshot_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("frame snapshot should arrive");
        assert_eq!(snapshot.target_id.value(), 32);
        assert_eq!(snapshot.rows[0].runs[0].text, "sdk");
        app.join().expect("app thread should join");
    }

    #[test]
    fn outbound_priority_queue_prefers_control_over_render_audio_and_video() {
        let mut pending = BinaryHeap::from([
            QueuedAppMessage {
                mux_seq: 1,
                priority: GNativeStreamPriority::Low,
                message: GNativeAppToHost::Mux(
                    germinal_gnative_protocol::gnative::tunnel::GNativeAppMuxFrame::new(
                        1,
                        GNativeAppPayload::Video(
                            germinal_gnative_protocol::gnative::media::GNativeVideoPacket {
                                stream_id: 1,
                                codec: "h264".to_string(),
                                pts_us: 10,
                                dts_us: Some(9),
                                keyframe: true,
                                payload: vec![1],
                            },
                        ),
                    ),
                ),
            },
            QueuedAppMessage {
                mux_seq: 2,
                priority: GNativeStreamPriority::Normal,
                message: GNativeAppToHost::Mux(
                    germinal_gnative_protocol::gnative::tunnel::GNativeAppMuxFrame::new(
                        2,
                        GNativeAppPayload::Audio(
                            germinal_gnative_protocol::gnative::media::GNativeAudioPacket {
                                stream_id: 2,
                                codec: "aac".to_string(),
                                pts_us: 11,
                                dts_us: Some(10),
                                payload: vec![2],
                            },
                        ),
                    ),
                ),
            },
            QueuedAppMessage {
                mux_seq: 3,
                priority: GNativeStreamPriority::High,
                message: GNativeAppToHost::Mux(
                    germinal_gnative_protocol::gnative::tunnel::GNativeAppMuxFrame::new(
                        3,
                        GNativeAppPayload::Render(GNativeFrame {
                            gshell_id: GShellId::new(99),
                            seq: Seq::new(1),
                            commands: Vec::new(),
                            cursor: None,
                        }),
                    ),
                ),
            },
            QueuedAppMessage {
                mux_seq: 4,
                priority: GNativeStreamPriority::Critical,
                message: GNativeAppToHost::Mux(
                    germinal_gnative_protocol::gnative::tunnel::GNativeAppMuxFrame::new(
                        4,
                        GNativeAppPayload::Control(GNativeMediaControlCommand::Pause),
                    ),
                ),
            },
        ]);

        let first = pending.pop().expect("control should pop first");
        let second = pending.pop().expect("render should pop second");
        let third = pending.pop().expect("audio should pop third");
        let fourth = pending.pop().expect("video should pop fourth");

        assert_eq!(first.priority, GNativeStreamPriority::Critical);
        assert_eq!(second.priority, GNativeStreamPriority::High);
        assert_eq!(third.priority, GNativeStreamPriority::Normal);
        assert_eq!(fourth.priority, GNativeStreamPriority::Low);
    }

    #[test]
    fn outbound_batch_drain_is_bounded_under_continuous_production() {
        let (tx, rx) = flume::unbounded();
        for mux_seq in 1..=(MAX_OUTBOUND_BATCH as u64 + 32) {
            tx.send(QueuedAppMessage {
                mux_seq,
                priority: GNativeStreamPriority::High,
                message: GNativeAppToHost::Mux(
                    germinal_gnative_protocol::gnative::tunnel::GNativeAppMuxFrame::new(
                        mux_seq,
                        GNativeAppPayload::Render(GNativeFrame {
                            gshell_id: GShellId::new(99),
                            seq: Seq::new(mux_seq),
                            commands: Vec::new(),
                            cursor: None,
                        }),
                    ),
                ),
            })
            .expect("test queue should stay open");
        }

        let first = rx.recv().expect("first message should exist");
        let batch = collect_outbound_batch(first, &rx);

        assert_eq!(batch.len(), MAX_OUTBOUND_BATCH);
        assert_eq!(rx.len(), 32);
    }

    #[test]
    fn host_message_reader_rejects_a_line_over_the_limit() {
        let mut reader = BufReader::new(&b"123456789\n"[..]);

        let error =
            read_host_message_with_limit(&mut reader, 8).expect_err("line should be rejected");

        assert!(matches!(
            error,
            crate::GNativeSdkError::MessageTooLarge { max_bytes: 8 }
        ));
    }
}
