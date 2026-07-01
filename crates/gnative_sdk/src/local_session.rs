use std::{
	io::{BufRead, BufReader, Write},
	net::{Shutdown, TcpStream},
};

use germinal_gnative_protocol::gnative::{
	frame::GNativeFrame,
	input::GNativeInputEvent,
	session::{GNativeAppHello, GNativeSessionAccepted},
	tunnel::{GNativeAppToHost, GNativeHostToApp},
};

use crate::control_sequence::{GNativeTunnelEnv, write_enter_control_sequence};

const TCP_ENDPOINT_PREFIX: &str = "tcp://";

pub struct LocalGNativeTunnelBootstrap {
	tunnel_env: GNativeTunnelEnv,
}

impl LocalGNativeTunnelBootstrap {
	pub fn from_env() -> Result<Self, String> {
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

	pub fn connect(self) -> Result<LocalGNativeSession, String> {
		let mut stream = TcpStream::connect(strip_tcp_endpoint_prefix(&self.tunnel_env.endpoint))
			.map_err(|error| error.to_string())?;
		stream.set_nodelay(true).ok();
		write_app_message(
			&mut stream,
			&GNativeAppToHost::Hello(GNativeAppHello {
				token:            self.tunnel_env.token,
				protocol_version: self.tunnel_env.protocol_version,
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

fn strip_tcp_endpoint_prefix(endpoint: &str) -> &str {
	endpoint.strip_prefix(TCP_ENDPOINT_PREFIX).unwrap_or(endpoint)
}

impl Drop for LocalGNativeSession {
	fn drop(&mut self) { self.writer.shutdown(Shutdown::Both).ok(); }
}

#[cfg(test)]
mod tests {
	use std::{sync::mpsc, thread, time::Duration};

	use germinal_domain::gshell::vo::gshell_id::GShellId;
	use germinal_gnative_protocol::{
		gnative::{frame::GNativeFrame, input::GNativeInputEvent},
		rendering::frame_plan_builder::{RenderCommandDto, TextStyleDto},
		seq::Seq,
	};
	use germinal_infra::gnative::tunnel::GNativeTunnel;
	use germinal_ports::{
		event::runtime_event_dispatcher::IRuntimeEventDispatcher,
		service::gnative_tunnel::IGNativeTunnel,
	};

	use super::LocalGNativeTunnelBootstrap;
	use crate::control_sequence::GNativeTunnelEnv;

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

		tunnel.accept_session(GShellId::new(32)).expect("handshake should complete");
		let snapshot =
			snapshot_rx.recv_timeout(Duration::from_secs(1)).expect("frame snapshot should arrive");
		assert_eq!(snapshot.target_id.value(), 32);
		assert_eq!(snapshot.rows[0].runs[0].text, "sdk");
		app.join().expect("app thread should join");
	}
}
