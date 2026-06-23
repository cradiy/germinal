use std::{
	io::{BufRead, BufReader, Write},
	net::{Shutdown, TcpListener, TcpStream},
	time::{SystemTime, UNIX_EPOCH},
};

use germinal_ports::gnative::{
	frame::GNativeFrame,
	input::GNativeInputEvent,
	rpc::{GNativeAppToHost, GNativeHostToApp},
	session::{GNativeAppHello, GNativeSessionAccepted},
};

use crate::control_sequence::{GNativeLaunchDescriptor, write_enter_control_sequence};

const TCP_ENDPOINT_PREFIX: &str = "tcp://";

pub struct LocalGNativeBootstrap {
	listener:   TcpListener,
	descriptor: GNativeLaunchDescriptor,
}

impl LocalGNativeBootstrap {
	pub fn bind_temporary(protocol_version: u32) -> Result<Self, String> {
		let token = unique_token();
		Self::bind("127.0.0.1:0".to_string(), token, protocol_version)
	}

	pub fn bind(endpoint: String, token: String, protocol_version: u32) -> Result<Self, String> {
		let bind_addr = strip_tcp_endpoint_prefix(&endpoint);
		let listener = TcpListener::bind(bind_addr).map_err(|error| error.to_string())?;
		let endpoint = encode_tcp_endpoint(listener.local_addr().map_err(|error| error.to_string())?);
		Ok(Self { listener, descriptor: GNativeLaunchDescriptor { endpoint, token, protocol_version } })
	}

	pub fn descriptor(&self) -> &GNativeLaunchDescriptor { &self.descriptor }

	pub fn write_enter_control_sequence<W: std::io::Write>(
		&self,
		writer: &mut W,
	) -> std::io::Result<()> {
		write_enter_control_sequence(writer, &self.descriptor)
	}

	pub fn accept(self) -> Result<LocalGNativeSession, String> {
		let (mut stream, _) = self.listener.accept().map_err(|error| error.to_string())?;
		stream.set_nodelay(true).ok();
		write_app_message(
			&mut stream,
			&GNativeAppToHost::Hello(GNativeAppHello {
				token:            self.descriptor.token,
				protocol_version: self.descriptor.protocol_version,
			}),
		)?;

		let welcome = read_host_message(&mut BufReader::new(
			stream.try_clone().map_err(|error| error.to_string())?,
		))?
		.ok_or_else(|| "host closed before welcome".to_string())?;
		let GNativeHostToApp::Welcome(accepted) = welcome else {
			return Err("expected welcome after gnative hello".to_string());
		};

		Ok(LocalGNativeSession {
			accepted,
			reader: BufReader::new(stream.try_clone().map_err(|error| error.to_string())?),
			writer: stream,
		})
	}
}

pub struct LocalGNativeSession {
	accepted: GNativeSessionAccepted,
	reader:   BufReader<TcpStream>,
	writer:   TcpStream,
}

impl LocalGNativeSession {
	pub fn accepted(&self) -> &GNativeSessionAccepted { &self.accepted }

	pub fn frame_writer(&self) -> Result<LocalGNativeFrameWriter, String> {
		Ok(LocalGNativeFrameWriter {
			accepted: self.accepted.clone(),
			stream:   self.writer.try_clone().map_err(|error| error.to_string())?,
		})
	}

	pub fn read_input(&mut self) -> Result<Option<GNativeInputEvent>, String> {
		match read_host_message(&mut self.reader)? {
			Some(GNativeHostToApp::Input(input)) => Ok(Some(input)),
			Some(GNativeHostToApp::Welcome(_)) => Err("unexpected duplicate welcome message".to_string()),
			None => Ok(None),
		}
	}

	pub fn send_exit(&mut self) -> Result<(), String> {
		write_app_message(&mut self.writer, &GNativeAppToHost::Exit)
	}
}

pub struct LocalGNativeFrameWriter {
	accepted: GNativeSessionAccepted,
	stream:   TcpStream,
}

impl LocalGNativeFrameWriter {
	pub fn send_frame(&mut self, frame: GNativeFrame) -> Result<(), String> {
		if frame.gshell_id != self.accepted.gshell_id {
			return Err("frame gshell_id does not match accepted session".to_string());
		}
		write_app_message(&mut self.stream, &GNativeAppToHost::Frame(frame))
	}

	pub fn send_exit(&mut self) -> Result<(), String> {
		write_app_message(&mut self.stream, &GNativeAppToHost::Exit)
	}
}

fn write_app_message(stream: &mut TcpStream, message: &GNativeAppToHost) -> Result<(), String> {
	let payload = serde_json::to_string(message).map_err(|error| error.to_string())?;
	stream.write_all(payload.as_bytes()).map_err(|error| error.to_string())?;
	stream.write_all(b"\n").map_err(|error| error.to_string())?;
	stream.flush().map_err(|error| error.to_string())
}

fn read_host_message(
	reader: &mut BufReader<TcpStream>,
) -> Result<Option<GNativeHostToApp>, String> {
	let mut line = String::new();
	let bytes_read = reader.read_line(&mut line).map_err(|error| error.to_string())?;
	if bytes_read == 0 {
		return Ok(None);
	}
	serde_json::from_str(line.trim_end()).map(Some).map_err(|error| error.to_string())
}

fn encode_tcp_endpoint(addr: std::net::SocketAddr) -> String {
	format!("{TCP_ENDPOINT_PREFIX}{addr}")
}

fn strip_tcp_endpoint_prefix(endpoint: &str) -> &str {
	endpoint.strip_prefix(TCP_ENDPOINT_PREFIX).unwrap_or(endpoint)
}

fn unique_token() -> String {
	let timestamp =
		SystemTime::now().duration_since(UNIX_EPOCH).expect("clock should be after epoch").as_nanos();
	format!("gnative-{timestamp:x}")
}

impl Drop for LocalGNativeSession {
	fn drop(&mut self) { self.writer.shutdown(Shutdown::Both).ok(); }
}

#[cfg(test)]
mod tests {
	use std::{sync::mpsc, thread, time::Duration};

	use germinal_domain::gshell::vo::gshell_id::GShellId;
	use germinal_infra::gnative::local_rpc::LocalGNativeRpcClient;
	use germinal_ports::{
		event::runtime_event_dispatcher::IRuntimeEventDispatcher,
		gnative::{
			frame::GNativeFrame, input::GNativeInputEvent, rpc::IGNativeRpcClient,
			session::GNativeSessionDescriptor,
		},
		rendering::frame_plan_builder::{RenderCommandDto, TextStyleDto},
		seq::Seq,
	};

	use super::LocalGNativeBootstrap;

	#[derive(Clone)]
	struct TestDispatcher;

	impl IRuntimeEventDispatcher for TestDispatcher {
		fn dispatch(
			&self,
			_event: germinal_ports::event::runtime_event::RuntimeEvent,
		) -> Result<(), String> {
			Ok(())
		}
	}

	#[test]
	fn bootstrap_accepts_host_handshake_and_reads_input() {
		let bootstrap = LocalGNativeBootstrap::bind_temporary(1).expect("bootstrap should bind");
		let descriptor = GNativeSessionDescriptor {
			gshell_id:        GShellId::new(31),
			endpoint:         bootstrap.descriptor().endpoint.clone(),
			token:            bootstrap.descriptor().token.clone(),
			protocol_version: bootstrap.descriptor().protocol_version,
		};
		let (snapshot_tx, _snapshot_rx) = mpsc::channel();
		let client = LocalGNativeRpcClient::new();
		client.configure(TestDispatcher, snapshot_tx);

		let app = thread::spawn(move || {
			let mut session = bootstrap.accept().expect("session should accept host");
			session.read_input().expect("input should read")
		});

		client.connect_and_handshake(&descriptor).expect("handshake should complete");
		client
			.send_input(GShellId::new(31), GNativeInputEvent::Paste("hello".to_string()))
			.expect("host should send input");

		assert_eq!(
			app.join().expect("app thread should join"),
			Some(GNativeInputEvent::Paste("hello".to_string()))
		);
	}

	#[test]
	fn frame_writer_sends_frame_back_to_host() {
		let bootstrap = LocalGNativeBootstrap::bind_temporary(1).expect("bootstrap should bind");
		let descriptor = GNativeSessionDescriptor {
			gshell_id:        GShellId::new(32),
			endpoint:         bootstrap.descriptor().endpoint.clone(),
			token:            bootstrap.descriptor().token.clone(),
			protocol_version: bootstrap.descriptor().protocol_version,
		};
		let (snapshot_tx, snapshot_rx) = mpsc::channel();
		let client = LocalGNativeRpcClient::new();
		client.configure(TestDispatcher, snapshot_tx);

		let app = thread::spawn(move || {
			let session = bootstrap.accept().expect("session should accept host");
			let mut writer = session.frame_writer().expect("frame writer should clone stream");
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

		client.connect_and_handshake(&descriptor).expect("handshake should complete");
		let snapshot =
			snapshot_rx.recv_timeout(Duration::from_secs(1)).expect("frame snapshot should arrive");
		assert_eq!(snapshot.target_id.value(), 32);
		assert_eq!(snapshot.rows[0].runs[0].text, "sdk");
		app.join().expect("app thread should join");
	}
}
