use std::{
	cmp::Ordering,
	collections::BinaryHeap,
	io::{BufRead, BufReader, Write},
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
		GNativeAppMuxFrame, GNativeAppPayload, GNativeAppToHost, GNativeHostPayload, GNativeHostToApp,
		GNativeStreamPriority,
	},
};

use crate::{
	control_sequence::{GNativeTunnelEnv, write_enter_control_sequence},
	error::{GNativeSdkError, GNativeSdkResult},
};

const TCP_ENDPOINT_PREFIX: &str = "tcp://";

pub struct LocalGNativeTunnelBootstrap {
	tunnel_env: GNativeTunnelEnv,
}

impl LocalGNativeTunnelBootstrap {
	pub fn from_env() -> GNativeSdkResult<Self> {
		Ok(Self { tunnel_env: GNativeTunnelEnv::from_env()? })
	}

	pub fn from_tunnel_env(tunnel_env: GNativeTunnelEnv) -> Self { Self { tunnel_env } }

	pub fn tunnel_env(&self) -> &GNativeTunnelEnv { &self.tunnel_env }

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
				token:            self.tunnel_env.token,
				protocol_version: self.tunnel_env.protocol_version,
			}),
		)?;

		let welcome = read_host_message(&mut BufReader::new(stream.try_clone()?))?
			.ok_or(GNativeSdkError::HostClosedBeforeWelcome)?;
		let GNativeHostToApp::Welcome(accepted) = welcome else {
			return Err(GNativeSdkError::ExpectedWelcomeAfterHello);
		};

		let writer_stream = stream.try_clone()?;
		let (queue_tx, queue_rx) = flume::unbounded();
		spawn_outbound_writer(writer_stream, queue_rx)?;

		Ok(LocalGNativeSession {
			accepted,
			reader: BufReader::new(stream),
			outbound: LocalGNativeOutbound { queue_tx, next_mux_seq: Arc::new(AtomicU64::new(1)) },
		})
	}
}

pub struct LocalGNativeSession {
	accepted: GNativeSessionAccepted,
	reader:   BufReader<TcpStream>,
	outbound: LocalGNativeOutbound,
}

impl LocalGNativeSession {
	pub fn accepted(&self) -> &GNativeSessionAccepted { &self.accepted }

	pub fn frame_writer(&self) -> LocalGNativeFrameWriter {
		LocalGNativeFrameWriter { accepted: self.accepted.clone(), outbound: self.outbound.clone() }
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

	pub fn send_exit(&mut self) -> GNativeSdkResult<()> { self.outbound.send_exit() }
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
		self.outbound.send_payload(GNativeAppPayload::Control(command))
	}

	pub fn send_audio_packet(&mut self, packet: GNativeAudioPacket) -> GNativeSdkResult<()> {
		self.outbound.send_payload(GNativeAppPayload::Audio(packet))
	}

	pub fn send_video_packet(&mut self, packet: GNativeVideoPacket) -> GNativeSdkResult<()> {
		self.outbound.send_payload(GNativeAppPayload::Video(packet))
	}

	pub fn send_exit(&mut self) -> GNativeSdkResult<()> { self.outbound.send_exit() }
}

#[derive(Clone)]
struct LocalGNativeOutbound {
	queue_tx:     flume::Sender<QueuedAppMessage>,
	next_mux_seq: Arc<AtomicU64>,
}

impl LocalGNativeOutbound {
	fn send_payload(&self, payload: GNativeAppPayload) -> GNativeSdkResult<()> {
		let mux_seq = self.next_mux_seq.fetch_add(1, AtomicOrdering::Relaxed);
		self
			.queue_tx
			.send(QueuedAppMessage {
				mux_seq,
				priority: payload.default_priority(),
				message: GNativeAppToHost::Mux(GNativeAppMuxFrame::new(mux_seq, payload)),
			})
			.map_err(|_| GNativeSdkError::OutboundQueueClosed)
	}

	fn send_exit(&self) -> GNativeSdkResult<()> {
		let mux_seq = self.next_mux_seq.fetch_add(1, AtomicOrdering::Relaxed);
		self
			.queue_tx
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
	mux_seq:  u64,
	priority: GNativeStreamPriority,
	message:  GNativeAppToHost,
}

impl Ord for QueuedAppMessage {
	fn cmp(&self, other: &Self) -> Ordering {
		self.priority.cmp(&other.priority).then_with(|| other.mux_seq.cmp(&self.mux_seq))
	}
}

impl PartialOrd for QueuedAppMessage {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
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
	let mut pending = BinaryHeap::new();

	while let Ok(message) = queue_rx.recv() {
		pending.push(message);
		while let Ok(message) = queue_rx.try_recv() {
			pending.push(message);
		}

		while let Some(message) = pending.pop() {
			if write_app_message(&mut stream, &message.message).is_err() {
				return;
			}
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

fn read_host_message(
	reader: &mut BufReader<TcpStream>,
) -> GNativeSdkResult<Option<GNativeHostToApp>> {
	let mut line = String::new();
	let bytes_read = reader.read_line(&mut line)?;
	if bytes_read == 0 {
		return Ok(None);
	}
	serde_json::from_str(line.trim_end()).map(Some).map_err(GNativeSdkError::DecodeMessage)
}

fn strip_tcp_endpoint_prefix(endpoint: &str) -> &str {
	endpoint.strip_prefix(TCP_ENDPOINT_PREFIX).unwrap_or(endpoint)
}

#[cfg(test)]
mod tests {
	use std::{collections::BinaryHeap, sync::mpsc, thread, time::Duration};

	use germinal_domain::gshell::vo::gshell_id::GShellId;
	use germinal_gnative_protocol::{
		gnative::{
			frame::GNativeFrame,
			input::GNativeInputEvent,
			media::GNativeMediaControlCommand,
			tunnel::{GNativeAppPayload, GNativeAppToHost, GNativeStreamPriority},
		},
		rendering::frame_plan_builder::{RenderCommandDto, TextStyleDto},
		seq::Seq,
	};
	use germinal_infra::gnative::tunnel::GNativeTunnel;
	use germinal_ports::{
		event::runtime_event_dispatcher::IRuntimeEventDispatcher,
		service::gnative_tunnel::IGNativeTunnel,
	};

	use super::{LocalGNativeTunnelBootstrap, QueuedAppMessage};
	use crate::control_sequence::GNativeTunnelEnv;

	#[derive(Clone)]
	struct TestDispatcher;

	impl IRuntimeEventDispatcher for TestDispatcher {
		fn dispatch(
			&self,
			_event: germinal_ports::event::runtime_event::RuntimeEvent,
		) -> germinal_ports::error::BoxResult<()> {
			Ok(())
		}
	}

	#[test]
	fn bootstrap_connects_to_host_tunnel_and_reads_input() {
		let (snapshot_tx, _snapshot_rx) = mpsc::channel();
		let tunnel = GNativeTunnel::new();
		tunnel.configure(TestDispatcher, snapshot_tx);
		let descriptor =
			tunnel.ensure_session_descriptor(GShellId::new(31), 1).expect("descriptor should exist");
		let bootstrap = LocalGNativeTunnelBootstrap::from_tunnel_env(GNativeTunnelEnv {
			endpoint:         descriptor.endpoint.clone(),
			token:            descriptor.token.clone(),
			protocol_version: descriptor.protocol_version,
		});

		let app = thread::spawn(move || {
			let mut session = bootstrap.connect().expect("session should connect host");
			session.read_input().expect("input should read")
		});

		tunnel.accept_session(GShellId::new(31)).expect("handshake should complete");
		tunnel
			.send_input(GShellId::new(31), GNativeInputEvent::Paste("hello".to_string()))
			.expect("host should send input");

		assert_eq!(
			app.join().expect("app thread should join"),
			Some(GNativeInputEvent::Paste("hello".to_string()))
		);
	}

	#[test]
	fn frame_writer_sends_frame_back_to_host() {
		let (snapshot_tx, snapshot_rx) = mpsc::channel();
		let tunnel = GNativeTunnel::new();
		tunnel.configure(TestDispatcher, snapshot_tx);
		let descriptor =
			tunnel.ensure_session_descriptor(GShellId::new(32), 1).expect("descriptor should exist");
		let bootstrap = LocalGNativeTunnelBootstrap::from_tunnel_env(GNativeTunnelEnv {
			endpoint:         descriptor.endpoint.clone(),
			token:            descriptor.token.clone(),
			protocol_version: descriptor.protocol_version,
		});

		let app = thread::spawn(move || {
			let session = bootstrap.connect().expect("session should connect host");
			let mut writer = session.frame_writer();
			writer
				.send_frame(GNativeFrame {
					gshell_id: session.accepted().gshell_id,
					seq:       Seq::new(9),
					commands:  vec![RenderCommandDto::ClearLine { y: 0 }, RenderCommandDto::StyledTextRun {
						x:     0,
						y:     0,
						text:  "sdk".to_string(),
						style: TextStyleDto::plain(),
					}],
					cursor:    None,
				})
				.expect("frame should send");
		});

		tunnel.accept_session(GShellId::new(32)).expect("handshake should complete");
		let snapshot =
			snapshot_rx.recv_timeout(Duration::from_secs(1)).expect("frame snapshot should arrive");
		assert_eq!(snapshot.target_id.value(), 32);
		assert_eq!(snapshot.rows[0].runs[0].text, "sdk");
		app.join().expect("app thread should join");
	}

	#[test]
	fn outbound_priority_queue_prefers_control_over_render_audio_and_video() {
		let mut pending = BinaryHeap::from([
			QueuedAppMessage {
				mux_seq:  1,
				priority: GNativeStreamPriority::Low,
				message:  GNativeAppToHost::Mux(
					germinal_gnative_protocol::gnative::tunnel::GNativeAppMuxFrame::new(
						1,
						GNativeAppPayload::Video(
							germinal_gnative_protocol::gnative::media::GNativeVideoPacket {
								stream_id: 1,
								codec:     "h264".to_string(),
								pts_us:    10,
								dts_us:    Some(9),
								keyframe:  true,
								payload:   vec![1],
							},
						),
					),
				),
			},
			QueuedAppMessage {
				mux_seq:  2,
				priority: GNativeStreamPriority::Normal,
				message:  GNativeAppToHost::Mux(
					germinal_gnative_protocol::gnative::tunnel::GNativeAppMuxFrame::new(
						2,
						GNativeAppPayload::Audio(
							germinal_gnative_protocol::gnative::media::GNativeAudioPacket {
								stream_id: 2,
								codec:     "aac".to_string(),
								pts_us:    11,
								dts_us:    Some(10),
								payload:   vec![2],
							},
						),
					),
				),
			},
			QueuedAppMessage {
				mux_seq:  3,
				priority: GNativeStreamPriority::High,
				message:  GNativeAppToHost::Mux(
					germinal_gnative_protocol::gnative::tunnel::GNativeAppMuxFrame::new(
						3,
						GNativeAppPayload::Render(GNativeFrame {
							gshell_id: GShellId::new(99),
							seq:       Seq::new(1),
							commands:  Vec::new(),
							cursor:    None,
						}),
					),
				),
			},
			QueuedAppMessage {
				mux_seq:  4,
				priority: GNativeStreamPriority::Critical,
				message:  GNativeAppToHost::Mux(
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
}
