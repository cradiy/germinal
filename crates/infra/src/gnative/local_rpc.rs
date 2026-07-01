use std::{
	cell::RefCell,
	collections::HashMap,
	io::{BufRead, BufReader, Write},
	net::{Shutdown, TcpStream},
	sync::mpsc::Sender,
	thread,
};

use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::{
	frame::{GNativeFrame, GNativeFrameCursor},
	input::GNativeInputEvent,
	rpc::{GNativeAppToHost, GNativeHostToApp},
	session::{GNativeSessionAccepted, GNativeSessionDescriptor},
};
use germinal_ports::{
	event::{
		runtime_event::{GShellRuntimeEvent, RuntimeEvent},
		runtime_event_dispatcher::IRuntimeEventDispatcher,
	},
	rendering::{
		frame_plan_builder::BuiltFramePlan,
		frame_plan_presenter::FramePlanPresenter,
		render_target_id::RenderTargetId,
		surface_snapshot::{
			RenderSurfaceCursorSnapshot, RenderSurfaceSnapshot, RenderSurfaceSnapshotProvider,
		},
	},
	service::gnative_rpc_client::IGNativeRpcClient,
};

use crate::rendering::text_surface_frame_plan_presenter::TextSurfaceFramePlanPresenter;

const TCP_ENDPOINT_PREFIX: &str = "tcp://";

pub struct LocalGNativeRpcClient<Dispatch> {
	sessions:            RefCell<HashMap<GShellId, TcpStream>>,
	dispatcher:          RefCell<Option<Dispatch>>,
	surface_snapshot_tx: RefCell<Option<Sender<RenderSurfaceSnapshot>>>,
}

impl<Dispatch> LocalGNativeRpcClient<Dispatch> {
	pub fn new() -> Self {
		Self {
			sessions:            RefCell::new(HashMap::new()),
			dispatcher:          RefCell::new(None),
			surface_snapshot_tx: RefCell::new(None),
		}
	}

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

fn connect_and_handshake<Dispatch>(
	client: &LocalGNativeRpcClient<Dispatch>,
	descriptor: &GNativeSessionDescriptor,
) -> Result<GNativeSessionAccepted, String>
where
	Dispatch: IRuntimeEventDispatcher,
{
	let stream = TcpStream::connect(strip_tcp_endpoint_prefix(&descriptor.endpoint))
		.map_err(|error| error.to_string())?;
	stream.set_nodelay(true).ok();
	let mut reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
	let message =
		read_app_message(&mut reader)?.ok_or_else(|| "gnative app closed before hello".to_string())?;
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
	write_host_message(&stream, &GNativeHostToApp::Welcome(accepted.clone()))?;

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

fn send_input<Dispatch>(
	client: &LocalGNativeRpcClient<Dispatch>,
	gshell_id: GShellId,
	input: GNativeInputEvent,
) -> Result<(), String> {
	let sessions = client.sessions.borrow_mut();
	let Some(stream) = sessions.get(&gshell_id) else {
		return Err(format!("no gnative session for {}", gshell_id.value()));
	};
	write_host_message(stream, &GNativeHostToApp::Input(input))
}

fn close_session<Dispatch>(
	client: &LocalGNativeRpcClient<Dispatch>,
	gshell_id: GShellId,
) -> Result<(), String> {
	let Some(stream) = client.sessions.borrow_mut().remove(&gshell_id) else {
		return Ok(());
	};
	stream.shutdown(Shutdown::Both).map_err(|error| error.to_string())
}

fn write_host_message(stream: &TcpStream, message: &GNativeHostToApp) -> Result<(), String> {
	let payload = serde_json::to_string(message).map_err(|error| error.to_string())?;
	let mut writer = stream;
	writer.write_all(payload.as_bytes()).map_err(|error| error.to_string())?;
	writer.write_all(b"\n").map_err(|error| error.to_string())?;
	writer.flush().map_err(|error| error.to_string())
}

fn read_app_message(reader: &mut BufReader<TcpStream>) -> Result<Option<GNativeAppToHost>, String> {
	let mut line = String::new();
	let bytes_read = reader.read_line(&mut line).map_err(|error| error.to_string())?;
	if bytes_read == 0 {
		return Ok(None);
	}
	serde_json::from_str(line.trim_end()).map(Some).map_err(|error| error.to_string())
}

fn read_frames_loop<Dispatch>(
	gshell_id: GShellId,
	stream: TcpStream,
	dispatcher: Dispatch,
	surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
) where
	Dispatch: IRuntimeEventDispatcher,
{
	let presenter = TextSurfaceFramePlanPresenter::new();
	let mut reader = BufReader::new(stream);
	let mut exit_dispatched = false;

	loop {
		let Ok(message) = read_app_message(&mut reader) else {
			break;
		};
		let Some(message) = message else {
			break;
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

fn cursor_snapshot(cursor: GNativeFrameCursor) -> RenderSurfaceCursorSnapshot {
	RenderSurfaceCursorSnapshot { x: cursor.x, y: cursor.y, focused: true }
}

fn dispatch_exit_gnative<Dispatch>(gshell_id: GShellId, dispatcher: &Dispatch)
where Dispatch: IRuntimeEventDispatcher {
	let _ = dispatcher.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::ExitGNative { gshell_id }));
}

fn strip_tcp_endpoint_prefix(endpoint: &str) -> &str {
	endpoint.strip_prefix(TCP_ENDPOINT_PREFIX).unwrap_or(endpoint)
}
