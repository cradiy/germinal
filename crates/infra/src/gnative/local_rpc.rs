use std::sync::mpsc::Sender;
#[cfg(unix)]
use std::{cell::RefCell, collections::HashMap, os::unix::net::UnixStream, thread};

use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_ports::{
	event::{
		runtime_event::{GShellRuntimeEvent, RuntimeEvent},
		runtime_event_dispatcher::IRuntimeEventDispatcher,
	},
	gnative::{
		frame::{GNativeFrame, GNativeFrameCursor},
		input::GNativeInputEvent,
		rpc::{GNativeAppToHost, GNativeHostToApp, IGNativeRpcClient},
		session::{GNativeSessionAccepted, GNativeSessionDescriptor},
	},
	rendering::{
		frame_plan_builder::BuiltFramePlan,
		frame_plan_presenter::FramePlanPresenter,
		render_target_id::RenderTargetId,
		surface_snapshot::{
			RenderSurfaceCursorSnapshot, RenderSurfaceSnapshot, RenderSurfaceSnapshotProvider,
		},
	},
};

use crate::rendering::text_surface_frame_plan_presenter::TextSurfaceFramePlanPresenter;

pub struct LocalGNativeRpcClient<Dispatch> {
	#[cfg(unix)]
	sessions:            RefCell<HashMap<GShellId, UnixStream>>,
	#[cfg(unix)]
	dispatcher:          RefCell<Option<Dispatch>>,
	#[cfg(unix)]
	surface_snapshot_tx: RefCell<Option<Sender<RenderSurfaceSnapshot>>>,
}

impl<Dispatch> LocalGNativeRpcClient<Dispatch> {
	pub fn new() -> Self {
		Self {
			#[cfg(unix)]
			sessions:                         RefCell::new(HashMap::new()),
			#[cfg(unix)]
			dispatcher:                       RefCell::new(None),
			#[cfg(unix)]
			surface_snapshot_tx:              RefCell::new(None),
		}
	}

	#[cfg(unix)]
	pub fn configure(
		&self,
		dispatcher: Dispatch,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
	) {
		*self.dispatcher.borrow_mut() = Some(dispatcher);
		*self.surface_snapshot_tx.borrow_mut() = Some(surface_snapshot_tx);
	}
}

impl<Dispatch> Default for LocalGNativeRpcClient<Dispatch> {
	fn default() -> Self { Self::new() }
}

impl<Dispatch> IGNativeRpcClient for LocalGNativeRpcClient<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	fn connect_and_handshake(
		&self,
		descriptor: &GNativeSessionDescriptor,
	) -> Result<GNativeSessionAccepted, String> {
		connect_and_handshake(self, descriptor)
	}

	fn send_input(&self, gshell_id: GShellId, input: GNativeInputEvent) -> Result<(), String> {
		send_input(self, gshell_id, input)
	}

	fn close_session(&self, gshell_id: GShellId) -> Result<(), String> {
		close_session(self, gshell_id)
	}
}

#[cfg(unix)]
fn connect_and_handshake<Dispatch>(
	client: &LocalGNativeRpcClient<Dispatch>,
	descriptor: &GNativeSessionDescriptor,
) -> Result<GNativeSessionAccepted, String>
where
	Dispatch: IRuntimeEventDispatcher,
{
	use std::io::{BufRead, BufReader};

	let stream = UnixStream::connect(&descriptor.endpoint).map_err(|error| error.to_string())?;
	let mut reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
	let mut line = String::new();
	reader.read_line(&mut line).map_err(|error| error.to_string())?;

	let message: GNativeAppToHost =
		serde_json::from_str(line.trim_end()).map_err(|error| error.to_string())?;
	let GNativeAppToHost::Hello(hello) = message else {
		return Err("expected gnative hello during handshake".to_string());
	};

	if hello.token != descriptor.token {
		return Err("gnative hello token mismatch".to_string());
	}
	if hello.protocol_version != descriptor.protocol_version {
		return Err("gnative protocol version mismatch".to_string());
	}

	let accepted = GNativeSessionAccepted {
		gshell_id:        descriptor.gshell_id,
		protocol_version: descriptor.protocol_version,
	};
	write_message(&stream, &GNativeHostToApp::Welcome(accepted.clone()))?;

	let dispatcher = client
		.dispatcher
		.borrow()
		.clone()
		.ok_or_else(|| "gnative rpc client is not configured with a dispatcher".to_string())?;
	let surface_snapshot_tx = client
		.surface_snapshot_tx
		.borrow()
		.clone()
		.ok_or_else(|| "gnative rpc client is not configured with a snapshot sender".to_string())?;
	let reader_stream = stream.try_clone().map_err(|error| error.to_string())?;
	let gshell_id = descriptor.gshell_id;
	thread::spawn(move || {
		read_frames_loop(gshell_id, reader_stream, dispatcher, surface_snapshot_tx)
	});

	client.sessions.borrow_mut().insert(descriptor.gshell_id, stream);
	Ok(accepted)
}

#[cfg(unix)]
fn send_input<Dispatch>(
	client: &LocalGNativeRpcClient<Dispatch>,
	gshell_id: GShellId,
	input: GNativeInputEvent,
) -> Result<(), String> {
	let sessions = client.sessions.borrow_mut();
	let Some(stream) = sessions.get(&gshell_id) else {
		return Err(format!("no gnative session for {}", gshell_id.value()));
	};
	write_message(stream, &GNativeHostToApp::Input(input))
}

#[cfg(not(unix))]
fn connect_and_handshake<Dispatch>(
	_client: &LocalGNativeRpcClient<Dispatch>,
	_descriptor: &GNativeSessionDescriptor,
) -> Result<GNativeSessionAccepted, String> {
	Err("local gnative rpc is only implemented for unix targets".to_string())
}

#[cfg(not(unix))]
fn send_input<Dispatch>(
	_client: &LocalGNativeRpcClient<Dispatch>,
	_gshell_id: GShellId,
	_input: GNativeInputEvent,
) -> Result<(), String> {
	Err("local gnative rpc is only implemented for unix targets".to_string())
}

#[cfg(unix)]
fn close_session<Dispatch>(
	client: &LocalGNativeRpcClient<Dispatch>,
	gshell_id: GShellId,
) -> Result<(), String> {
	use std::net::Shutdown;

	let Some(stream) = client.sessions.borrow_mut().remove(&gshell_id) else {
		return Ok(());
	};
	stream.shutdown(Shutdown::Both).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn close_session<Dispatch>(
	_client: &LocalGNativeRpcClient<Dispatch>,
	_gshell_id: GShellId,
) -> Result<(), String> {
	Err("local gnative rpc is only implemented for unix targets".to_string())
}

#[cfg(unix)]
fn write_message(stream: &UnixStream, message: &GNativeHostToApp) -> Result<(), String> {
	use std::io::Write;

	let payload = serde_json::to_string(message).map_err(|error| error.to_string())?;
	let mut writer = stream;
	writer.write_all(payload.as_bytes()).map_err(|error| error.to_string())?;
	writer.write_all(b"\n").map_err(|error| error.to_string())?;
	writer.flush().map_err(|error| error.to_string())
}

#[cfg(unix)]
fn read_frames_loop<Dispatch>(
	gshell_id: GShellId,
	stream: UnixStream,
	dispatcher: Dispatch,
	surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
) where
	Dispatch: IRuntimeEventDispatcher,
{
	use std::io::{BufRead, BufReader};

	let presenter = TextSurfaceFramePlanPresenter::new();
	let mut reader = BufReader::new(stream);
	let mut line = String::new();
	let mut exit_dispatched = false;

	loop {
		line.clear();
		match reader.read_line(&mut line) {
			Ok(0) => break,
			Ok(_) => {}
			Err(_) => break,
		}

		let Ok(message) = serde_json::from_str::<GNativeAppToHost>(line.trim_end()) else {
			continue;
		};

		match message {
			GNativeAppToHost::Frame(frame) => {
				present_frame(frame, &dispatcher, &surface_snapshot_tx, &presenter)
			}
			GNativeAppToHost::Exit => {
				dispatch_exit_gnative(gshell_id, &dispatcher);
				exit_dispatched = true;
				break;
			}
			GNativeAppToHost::Hello(_) => continue,
		}
	}

	if !exit_dispatched {
		dispatch_exit_gnative(gshell_id, &dispatcher);
	}
}

#[cfg(unix)]
fn present_frame<Dispatch>(
	frame: GNativeFrame,
	dispatcher: &Dispatch,
	surface_snapshot_tx: &Sender<RenderSurfaceSnapshot>,
	presenter: &TextSurfaceFramePlanPresenter,
) where
	Dispatch: IRuntimeEventDispatcher,
{
	let target_id = RenderTargetId::new(frame.gshell_id.value());
	let plan = BuiltFramePlan { target_id, seq: frame.seq, commands: frame.commands };
	presenter.present(&plan);
	let Some(mut snapshot) = presenter.surface_snapshot_of(target_id) else {
		return;
	};
	snapshot.cursor = frame.cursor.map(cursor_snapshot);

	if surface_snapshot_tx.send(snapshot).is_err() {
		return;
	}

	let _ = dispatcher.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady {
		gshell_id: frame.gshell_id,
		seq:       frame.seq,
	}));
}

#[cfg(unix)]
fn cursor_snapshot(cursor: GNativeFrameCursor) -> RenderSurfaceCursorSnapshot {
	RenderSurfaceCursorSnapshot { x: cursor.x, y: cursor.y, focused: true }
}

#[cfg(unix)]
fn dispatch_exit_gnative<Dispatch>(gshell_id: GShellId, dispatcher: &Dispatch)
where Dispatch: IRuntimeEventDispatcher {
	let _ = dispatcher.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::ExitGNative { gshell_id }));
}

#[cfg(test)]
mod tests {
	use std::{
		fs,
		io::{BufRead, BufReader, Write},
		sync::mpsc,
		time::{SystemTime, UNIX_EPOCH},
	};

	use germinal_domain::gshell::vo::gshell_id::GShellId;
	use germinal_ports::{
		event::{
			runtime_event::{GShellRuntimeEvent, RuntimeEvent},
			runtime_event_dispatcher::IRuntimeEventDispatcher,
		},
		gnative::{
			frame::{GNativeFrame, GNativeFrameCursor},
			input::GNativeInputEvent,
			rpc::{GNativeAppToHost, GNativeHostToApp, IGNativeRpcClient},
			session::{GNativeAppHello, GNativeSessionDescriptor},
		},
		rendering::frame_plan_builder::{RenderCommandDto, TextStyleDto},
		seq::Seq,
	};

	use super::LocalGNativeRpcClient;

	#[derive(Clone)]
	struct TestDispatcher {
		tx: mpsc::Sender<RuntimeEvent>,
	}

	impl IRuntimeEventDispatcher for TestDispatcher {
		fn dispatch(&self, event: RuntimeEvent) -> Result<(), String> {
			self.tx.send(event).map_err(|error| error.to_string())
		}
	}

	#[cfg(unix)]
	#[test]
	fn unix_client_completes_hello_welcome_handshake() {
		use std::{os::unix::net::UnixListener, thread};

		let socket_path = unique_socket_path();
		let listener = UnixListener::bind(&socket_path).expect("listener should bind");
		let (event_tx, _event_rx) = mpsc::channel();
		let (snapshot_tx, _snapshot_rx) = mpsc::channel();

		let server = thread::spawn({
			let socket_path = socket_path.clone();
			move || {
				let (mut stream, _) = listener.accept().expect("server should accept");
				let mut reader = BufReader::new(stream.try_clone().expect("server should clone stream"));
				send_hello(&mut stream);
				let message = read_host_message(&mut reader);
				fs::remove_file(&socket_path).ok();
				message
			}
		});

		let client = LocalGNativeRpcClient::new();
		client.configure(TestDispatcher { tx: event_tx }, snapshot_tx);
		let descriptor = GNativeSessionDescriptor {
			gshell_id:        GShellId::new(11),
			endpoint:         socket_path.to_string_lossy().into_owned(),
			token:            "secret".to_string(),
			protocol_version: 1,
		};

		let accepted =
			client.connect_and_handshake(&descriptor).expect("client should complete handshake");

		let welcome = server.join().expect("server should join");
		assert_eq!(accepted.gshell_id, GShellId::new(11));
		assert_eq!(welcome, GNativeHostToApp::Welcome(accepted));
	}

	#[cfg(unix)]
	#[test]
	fn unix_client_sends_input_after_handshake() {
		use std::{os::unix::net::UnixListener, thread};

		let socket_path = unique_socket_path();
		let listener = UnixListener::bind(&socket_path).expect("listener should bind");
		let (event_tx, _event_rx) = mpsc::channel();
		let (snapshot_tx, _snapshot_rx) = mpsc::channel();

		let server = thread::spawn({
			let socket_path = socket_path.clone();
			move || {
				let (mut stream, _) = listener.accept().expect("server should accept");
				let mut reader = BufReader::new(stream.try_clone().expect("server should clone stream"));
				send_hello(&mut stream);
				let welcome = read_host_message(&mut reader);
				let input = read_host_message(&mut reader);
				fs::remove_file(&socket_path).ok();
				(welcome, input)
			}
		});

		let client = LocalGNativeRpcClient::new();
		client.configure(TestDispatcher { tx: event_tx }, snapshot_tx);
		let descriptor = GNativeSessionDescriptor {
			gshell_id:        GShellId::new(12),
			endpoint:         socket_path.to_string_lossy().into_owned(),
			token:            "secret".to_string(),
			protocol_version: 1,
		};
		let accepted =
			client.connect_and_handshake(&descriptor).expect("client should complete handshake");
		client
			.send_input(GShellId::new(12), GNativeInputEvent::Paste("hello".to_string()))
			.expect("client should send input");

		let (welcome, input) = server.join().expect("server should join");
		assert_eq!(welcome, GNativeHostToApp::Welcome(accepted));
		assert_eq!(input, GNativeHostToApp::Input(GNativeInputEvent::Paste("hello".to_string())));
	}

	#[cfg(unix)]
	#[test]
	fn unix_client_presents_remote_frame_after_handshake() {
		use std::{os::unix::net::UnixListener, thread};

		let socket_path = unique_socket_path();
		let listener = UnixListener::bind(&socket_path).expect("listener should bind");
		let (event_tx, event_rx) = mpsc::channel();
		let (snapshot_tx, snapshot_rx) = mpsc::channel();

		let server = thread::spawn({
			let socket_path = socket_path.clone();
			move || {
				let (mut stream, _) = listener.accept().expect("server should accept");
				let mut reader = BufReader::new(stream.try_clone().expect("server should clone stream"));
				send_hello(&mut stream);
				let _welcome = read_host_message(&mut reader);
				let frame = serde_json::to_string(&GNativeAppToHost::Frame(GNativeFrame {
					gshell_id: GShellId::new(13),
					seq:       Seq::new(7),
					commands:  vec![RenderCommandDto::ClearLine { y: 0 }, RenderCommandDto::StyledTextRun {
						x:     0,
						y:     0,
						text:  "hi".to_string(),
						style: TextStyleDto::plain(),
					}],
					cursor:    Some(GNativeFrameCursor { x: 2, y: 0 }),
				}))
				.expect("frame should serialize");
				stream.write_all(frame.as_bytes()).expect("server should write frame");
				stream.write_all(b"\n").expect("server should write newline");
				stream.flush().expect("server should flush");
				fs::remove_file(&socket_path).ok();
			}
		});

		let client = LocalGNativeRpcClient::new();
		client.configure(TestDispatcher { tx: event_tx }, snapshot_tx);
		let descriptor = GNativeSessionDescriptor {
			gshell_id:        GShellId::new(13),
			endpoint:         socket_path.to_string_lossy().into_owned(),
			token:            "secret".to_string(),
			protocol_version: 1,
		};
		client.connect_and_handshake(&descriptor).expect("client should complete handshake");

		let snapshot =
			snapshot_rx.recv_timeout(std::time::Duration::from_secs(1)).expect("snapshot should arrive");
		assert_eq!(snapshot.target_id.value(), 13);
		assert_eq!(snapshot.latest_seq, Seq::new(7));
		assert_eq!(snapshot.rows[0].runs[0].text, "hi");
		assert_eq!(snapshot.cursor.expect("cursor should be present").x, 2);

		let event = event_rx
			.recv_timeout(std::time::Duration::from_secs(1))
			.expect("frame-ready event should arrive");
		assert_eq!(
			event,
			RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady {
				gshell_id: GShellId::new(13),
				seq:       Seq::new(7),
			})
		);

		server.join().expect("server should join");
	}

	#[cfg(unix)]
	#[test]
	fn unix_client_dispatches_exit_when_app_requests_it() {
		use std::{os::unix::net::UnixListener, thread};

		let socket_path = unique_socket_path();
		let listener = UnixListener::bind(&socket_path).expect("listener should bind");
		let (event_tx, event_rx) = mpsc::channel();
		let (snapshot_tx, _snapshot_rx) = mpsc::channel();

		let server = thread::spawn({
			let socket_path = socket_path.clone();
			move || {
				let (mut stream, _) = listener.accept().expect("server should accept");
				let mut reader = BufReader::new(stream.try_clone().expect("server should clone stream"));
				send_hello(&mut stream);
				let _welcome = read_host_message(&mut reader);
				let exit = serde_json::to_string(&GNativeAppToHost::Exit).expect("exit should serialize");
				stream.write_all(exit.as_bytes()).expect("server should write exit");
				stream.write_all(b"\n").expect("server should write newline");
				stream.flush().expect("server should flush");
				fs::remove_file(&socket_path).ok();
			}
		});

		let client = LocalGNativeRpcClient::new();
		client.configure(TestDispatcher { tx: event_tx }, snapshot_tx);
		let descriptor = GNativeSessionDescriptor {
			gshell_id:        GShellId::new(14),
			endpoint:         socket_path.to_string_lossy().into_owned(),
			token:            "secret".to_string(),
			protocol_version: 1,
		};
		client.connect_and_handshake(&descriptor).expect("client should complete handshake");

		let event =
			event_rx.recv_timeout(std::time::Duration::from_secs(1)).expect("exit event should arrive");
		assert_eq!(
			event,
			RuntimeEvent::GShell(GShellRuntimeEvent::ExitGNative { gshell_id: GShellId::new(14) })
		);

		server.join().expect("server should join");
	}

	#[cfg(unix)]
	fn send_hello(stream: &mut std::os::unix::net::UnixStream) {
		let hello = serde_json::to_string(&GNativeAppToHost::Hello(GNativeAppHello {
			token:            "secret".to_string(),
			protocol_version: 1,
		}))
		.expect("hello should serialize");
		stream.write_all(hello.as_bytes()).expect("server should write hello");
		stream.write_all(b"\n").expect("server should write newline");
		stream.flush().expect("server should flush");
	}

	#[cfg(unix)]
	fn read_host_message(reader: &mut BufReader<std::os::unix::net::UnixStream>) -> GNativeHostToApp {
		let mut line = String::new();
		reader.read_line(&mut line).expect("server should read host message");
		serde_json::from_str(line.trim_end()).expect("host message should deserialize")
	}

	fn unique_socket_path() -> std::path::PathBuf {
		let timestamp =
			SystemTime::now().duration_since(UNIX_EPOCH).expect("clock should be after epoch").as_nanos();
		std::env::temp_dir().join(format!("germinal-gnative-{timestamp}.sock"))
	}
}
