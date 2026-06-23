use std::time::{SystemTime, UNIX_EPOCH};
#[cfg(unix)]
use std::{
	io::{BufRead, BufReader, Write},
	os::unix::net::{UnixListener, UnixStream},
	path::PathBuf,
};

use germinal_ports::gnative::{
	frame::GNativeFrame,
	input::GNativeInputEvent,
	rpc::{GNativeAppToHost, GNativeHostToApp},
	session::{GNativeAppHello, GNativeSessionAccepted},
};

use crate::control_sequence::{GNativeLaunchDescriptor, write_enter_control_sequence};

pub struct LocalGNativeBootstrap {
	#[cfg(unix)]
	listener:   LocalListener,
	descriptor: GNativeLaunchDescriptor,
}

#[cfg(unix)]
struct LocalListener {
	listener:    UnixListener,
	socket_path: PathBuf,
}

impl LocalGNativeBootstrap {
	pub fn bind_temporary(protocol_version: u32) -> Result<Self, String> {
		let endpoint = unique_socket_path();
		let token = unique_token();
		Self::bind(endpoint.to_string_lossy().into_owned(), token, protocol_version)
	}

	pub fn bind(endpoint: String, token: String, protocol_version: u32) -> Result<Self, String> {
		bind_local(endpoint, token, protocol_version)
	}

	pub fn descriptor(&self) -> &GNativeLaunchDescriptor { &self.descriptor }

	pub fn write_enter_control_sequence<W: std::io::Write>(
		&self,
		writer: &mut W,
	) -> std::io::Result<()> {
		write_enter_control_sequence(writer, &self.descriptor)
	}

	pub fn accept(self) -> Result<LocalGNativeSession, String> { accept_local(self) }
}

pub struct LocalGNativeSession {
	accepted: GNativeSessionAccepted,
	#[cfg(unix)]
	reader:   BufReader<UnixStream>,
	#[cfg(unix)]
	writer:   UnixStream,
}

impl LocalGNativeSession {
	pub fn accepted(&self) -> &GNativeSessionAccepted { &self.accepted }

	pub fn frame_writer(&self) -> Result<LocalGNativeFrameWriter, String> { frame_writer_of(self) }

	pub fn read_input(&mut self) -> Result<Option<GNativeInputEvent>, String> { read_input_of(self) }

	pub fn send_exit(&mut self) -> Result<(), String> { send_exit_of(self) }
}

pub struct LocalGNativeFrameWriter {
	accepted: GNativeSessionAccepted,
	#[cfg(unix)]
	stream:   UnixStream,
}

impl LocalGNativeFrameWriter {
	pub fn send_frame(&mut self, frame: GNativeFrame) -> Result<(), String> {
		if frame.gshell_id != self.accepted.gshell_id {
			return Err("frame gshell_id does not match accepted session".to_string());
		}
		write_app_message(&mut self.stream, &GNativeAppToHost::Frame(frame))
	}
}

#[cfg(unix)]
fn bind_local(
	endpoint: String,
	token: String,
	protocol_version: u32,
) -> Result<LocalGNativeBootstrap, String> {
	let socket_path = PathBuf::from(&endpoint);
	std::fs::remove_file(&socket_path).ok();
	let listener = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
	Ok(LocalGNativeBootstrap {
		listener:   LocalListener { listener, socket_path },
		descriptor: GNativeLaunchDescriptor { endpoint, token, protocol_version },
	})
}

#[cfg(not(unix))]
fn bind_local(
	_endpoint: String,
	_token: String,
	_protocol_version: u32,
) -> Result<LocalGNativeBootstrap, String> {
	Err("local gnative sessions are only implemented for unix targets".to_string())
}

#[cfg(unix)]
fn accept_local(bootstrap: LocalGNativeBootstrap) -> Result<LocalGNativeSession, String> {
	let LocalGNativeBootstrap { listener, descriptor } = bootstrap;
	let (mut stream, _) = listener.listener.accept().map_err(|error| error.to_string())?;
	write_app_message(
		&mut stream,
		&GNativeAppToHost::Hello(GNativeAppHello {
			token:            descriptor.token,
			protocol_version: descriptor.protocol_version,
		}),
	)?;

	let welcome =
		read_host_message(&mut BufReader::new(stream.try_clone().map_err(|error| error.to_string())?))?
			.ok_or_else(|| "host closed before welcome".to_string())?;
	let GNativeHostToApp::Welcome(accepted) = welcome else {
		return Err("expected welcome after gnative hello".to_string());
	};

	std::fs::remove_file(listener.socket_path).ok();
	Ok(LocalGNativeSession {
		accepted,
		reader: BufReader::new(stream.try_clone().map_err(|error| error.to_string())?),
		writer: stream,
	})
}

#[cfg(not(unix))]
fn accept_local(_bootstrap: LocalGNativeBootstrap) -> Result<LocalGNativeSession, String> {
	Err("local gnative sessions are only implemented for unix targets".to_string())
}

#[cfg(unix)]
fn frame_writer_of(session: &LocalGNativeSession) -> Result<LocalGNativeFrameWriter, String> {
	Ok(LocalGNativeFrameWriter {
		accepted: session.accepted.clone(),
		stream:   session.writer.try_clone().map_err(|error| error.to_string())?,
	})
}

#[cfg(not(unix))]
fn frame_writer_of(_session: &LocalGNativeSession) -> Result<LocalGNativeFrameWriter, String> {
	Err("local gnative sessions are only implemented for unix targets".to_string())
}

#[cfg(unix)]
fn read_input_of(session: &mut LocalGNativeSession) -> Result<Option<GNativeInputEvent>, String> {
	match read_host_message(&mut session.reader)? {
		Some(GNativeHostToApp::Input(input)) => Ok(Some(input)),
		Some(GNativeHostToApp::Welcome(_)) => Err("unexpected duplicate welcome message".to_string()),
		None => Ok(None),
	}
}

#[cfg(not(unix))]
fn read_input_of(_session: &mut LocalGNativeSession) -> Result<Option<GNativeInputEvent>, String> {
	Err("local gnative sessions are only implemented for unix targets".to_string())
}

#[cfg(unix)]
fn send_exit_of(session: &mut LocalGNativeSession) -> Result<(), String> {
	write_app_message(&mut session.writer, &GNativeAppToHost::Exit)
}

#[cfg(not(unix))]
fn send_exit_of(_session: &mut LocalGNativeSession) -> Result<(), String> {
	Err("local gnative sessions are only implemented for unix targets".to_string())
}

#[cfg(unix)]
fn write_app_message(stream: &mut UnixStream, message: &GNativeAppToHost) -> Result<(), String> {
	let payload = serde_json::to_string(message).map_err(|error| error.to_string())?;
	stream.write_all(payload.as_bytes()).map_err(|error| error.to_string())?;
	stream.write_all(b"\n").map_err(|error| error.to_string())?;
	stream.flush().map_err(|error| error.to_string())
}

#[cfg(unix)]
fn read_host_message(
	reader: &mut BufReader<UnixStream>,
) -> Result<Option<GNativeHostToApp>, String> {
	let mut line = String::new();
	let bytes_read = reader.read_line(&mut line).map_err(|error| error.to_string())?;
	if bytes_read == 0 {
		return Ok(None);
	}
	serde_json::from_str(line.trim_end()).map(Some).map_err(|error| error.to_string())
}

fn unique_socket_path() -> std::path::PathBuf {
	let timestamp =
		SystemTime::now().duration_since(UNIX_EPOCH).expect("clock should be after epoch").as_nanos();
	std::env::temp_dir().join(format!("germinal-gnative-sdk-{timestamp}.sock"))
}

fn unique_token() -> String {
	let timestamp =
		SystemTime::now().duration_since(UNIX_EPOCH).expect("clock should be after epoch").as_nanos();
	format!("gnative-{timestamp:x}")
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

	#[cfg(unix)]
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

	#[cfg(unix)]
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
