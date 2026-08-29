use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    thread,
};

use germinal_gnative_protocol::gnative::{
    frame::GNativeFrame,
    input::GNativeInputEvent,
    session::{GNativeAppHello, GNativeSessionAccepted},
    tunnel::{GNATIVE_MAX_MESSAGE_BYTES, GNativeAppToHost, GNativeHostToApp},
};

use crate::{
    control_sequence::{GNativeTunnelEnv, write_enter_control_sequence},
    error::{GNativeSdkError, GNativeSdkResult},
};

const TCP_ENDPOINT_PREFIX: &str = "tcp://";
const OUTBOUND_QUEUE_CAPACITY: usize = 256;

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
            outbound: LocalGNativeOutbound { queue_tx },
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
            Some(GNativeHostToApp::Input(input)) => Ok(Some(input)),
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
        self.outbound.send(GNativeAppToHost::Frame(frame))
    }

    pub fn send_exit(&mut self) -> GNativeSdkResult<()> {
        self.outbound.send_exit()
    }
}

#[derive(Clone)]
struct LocalGNativeOutbound {
    queue_tx: flume::Sender<GNativeAppToHost>,
}

impl LocalGNativeOutbound {
    fn send(&self, message: GNativeAppToHost) -> GNativeSdkResult<()> {
        self.queue_tx
            .send(message)
            .map_err(|_| GNativeSdkError::OutboundQueueClosed)
    }

    fn send_exit(&self) -> GNativeSdkResult<()> {
        self.send(GNativeAppToHost::Exit)
    }
}

fn spawn_outbound_writer(
    stream: TcpStream,
    queue_rx: flume::Receiver<GNativeAppToHost>,
) -> GNativeSdkResult<()> {
    thread::Builder::new()
        .name("gnative-sdk-outbound".to_string())
        .spawn(move || run_outbound_writer(stream, queue_rx))
        .map(|_| ())
        .map_err(GNativeSdkError::from)
}

fn run_outbound_writer(mut stream: TcpStream, queue_rx: flume::Receiver<GNativeAppToHost>) {
    while let Ok(message) = queue_rx.recv() {
        if write_app_message(&mut stream, &message).is_err() {
            return;
        }
    }
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
            session::GNATIVE_PROTOCOL_VERSION,
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

    use super::{LocalGNativeTunnelBootstrap, read_host_message_with_limit};
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
