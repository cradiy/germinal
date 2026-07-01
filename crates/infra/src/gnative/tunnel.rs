use std::{
	cell::RefCell,
	collections::HashMap,
	sync::mpsc::{self, Sender},
	thread,
	time::{SystemTime, UNIX_EPOCH},
};

use compio::{
	BufResult,
	io::{AsyncRead, AsyncWrite, AsyncWriteExt},
	net::{TcpListener, TcpStream},
	runtime::{Runtime, spawn},
};
use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::{
	frame::{GNativeFrame, GNativeFrameCursor},
	input::GNativeInputEvent,
	session::{GNativeSessionAccepted, GNativeSessionDescriptor},
	tunnel::{GNativeAppToHost, GNativeHostToApp},
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
	service::gnative_tunnel::IGNativeTunnel,
};

use crate::rendering::text_surface_frame_plan_presenter::TextSurfaceFramePlanPresenter;

const TCP_ENDPOINT_PREFIX: &str = "tcp://";

pub struct GNativeTunnel<Dispatch> {
	command_tx:          flume::Sender<TunnelCommand<Dispatch>>,
	dispatcher:          RefCell<Option<Dispatch>>,
	surface_snapshot_tx: RefCell<Option<Sender<RenderSurfaceSnapshot>>>,
}

impl<Dispatch> GNativeTunnel<Dispatch> {
	pub fn configure(
		&self,
		dispatcher: Dispatch,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
	) {
		*self.dispatcher.borrow_mut() = Some(dispatcher);
		*self.surface_snapshot_tx.borrow_mut() = Some(surface_snapshot_tx);
	}
}

impl<Dispatch> GNativeTunnel<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	pub fn new() -> Self {
		let (command_tx, command_rx) = flume::unbounded();

		thread::Builder::new()
			.name("gnative-tunnel".to_string())
			.spawn(move || {
				let runtime = Runtime::new().expect("failed to create gnative tunnel runtime");
				runtime.block_on(run_tunnel_runtime(command_rx));
			})
			.expect("failed to spawn gnative tunnel runtime thread");

		Self { command_tx, dispatcher: RefCell::new(None), surface_snapshot_tx: RefCell::new(None) }
	}
}

impl<Dispatch> Default for GNativeTunnel<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	fn default() -> Self { Self::new() }
}

impl<Dispatch> IGNativeTunnel for GNativeTunnel<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	fn ensure_session_descriptor(
		&self,
		gshell_id: GShellId,
		protocol_version: u32,
	) -> Result<GNativeSessionDescriptor, String> {
		let (response_tx, response_rx) = mpsc::channel();
		self
			.command_tx
			.send(TunnelCommand::EnsureSessionDescriptor { gshell_id, protocol_version, response_tx })
			.map_err(|error| error.to_string())?;

		response_rx.recv().map_err(|error| error.to_string())?
	}

	fn accept_session(&self, gshell_id: GShellId) -> Result<GNativeSessionAccepted, String> {
		let dispatcher = self
			.dispatcher
			.borrow()
			.clone()
			.ok_or_else(|| "gnative tunnel is not configured with a dispatcher".to_string())?;
		let surface_snapshot_tx = self
			.surface_snapshot_tx
			.borrow()
			.clone()
			.ok_or_else(|| "gnative tunnel is not configured with a snapshot sender".to_string())?;
		let (response_tx, response_rx) = mpsc::channel();

		self
			.command_tx
			.send(TunnelCommand::AcceptSession {
				gshell_id,
				dispatcher,
				surface_snapshot_tx,
				response_tx,
			})
			.map_err(|error| error.to_string())?;

		response_rx.recv().map_err(|error| error.to_string())?
	}

	fn send_input(&self, gshell_id: GShellId, input: GNativeInputEvent) -> Result<(), String> {
		let (response_tx, response_rx) = mpsc::channel();
		self
			.command_tx
			.send(TunnelCommand::SendInput { gshell_id, input, response_tx })
			.map_err(|error| error.to_string())?;

		response_rx.recv().map_err(|error| error.to_string())?
	}

	fn close_session(&self, gshell_id: GShellId) -> Result<(), String> {
		let (response_tx, response_rx) = mpsc::channel();
		self
			.command_tx
			.send(TunnelCommand::CloseSession { gshell_id, response_tx })
			.map_err(|error| error.to_string())?;

		response_rx.recv().map_err(|error| error.to_string())?
	}
}

enum TunnelCommand<Dispatch> {
	EnsureSessionDescriptor {
		gshell_id:        GShellId,
		protocol_version: u32,
		response_tx:      mpsc::Sender<Result<GNativeSessionDescriptor, String>>,
	},
	AcceptSession {
		gshell_id:           GShellId,
		dispatcher:          Dispatch,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		response_tx:         mpsc::Sender<Result<GNativeSessionAccepted, String>>,
	},
	SendInput {
		gshell_id:   GShellId,
		input:       GNativeInputEvent,
		response_tx: mpsc::Sender<Result<(), String>>,
	},
	CloseSession {
		gshell_id:   GShellId,
		response_tx: mpsc::Sender<Result<(), String>>,
	},
}

struct TunnelSlot {
	descriptor: GNativeSessionDescriptor,
	listener:   TcpListener,
	writer:     Option<TcpStream>,
}

async fn run_tunnel_runtime<Dispatch>(command_rx: flume::Receiver<TunnelCommand<Dispatch>>)
where Dispatch: IRuntimeEventDispatcher {
	let mut slots = HashMap::<GShellId, TunnelSlot>::new();

	while let Ok(command) = command_rx.recv_async().await {
		match command {
			TunnelCommand::EnsureSessionDescriptor { gshell_id, protocol_version, response_tx } => {
				let result = ensure_session_descriptor_async(&mut slots, gshell_id, protocol_version).await;
				let _ = response_tx.send(result);
			}
			TunnelCommand::AcceptSession { gshell_id, dispatcher, surface_snapshot_tx, response_tx } => {
				let result =
					accept_session_async(&mut slots, gshell_id, dispatcher, surface_snapshot_tx).await;
				let _ = response_tx.send(result);
			}
			TunnelCommand::SendInput { gshell_id, input, response_tx } => {
				let result = send_input_async(&mut slots, gshell_id, input).await;
				let _ = response_tx.send(result);
			}
			TunnelCommand::CloseSession { gshell_id, response_tx } => {
				let result = close_session_async(&mut slots, gshell_id).await;
				let _ = response_tx.send(result);
			}
		}
	}
}

async fn ensure_session_descriptor_async(
	slots: &mut HashMap<GShellId, TunnelSlot>,
	gshell_id: GShellId,
	protocol_version: u32,
) -> Result<GNativeSessionDescriptor, String> {
	if let Some(slot) = slots.get(&gshell_id) {
		return Ok(slot.descriptor.clone());
	}

	let listener = TcpListener::bind("127.0.0.1:0").await.map_err(io_error)?;
	let endpoint = encode_tcp_endpoint(listener.local_addr().map_err(io_error)?);
	let descriptor =
		GNativeSessionDescriptor { gshell_id, endpoint, token: unique_token(), protocol_version };

	slots.insert(gshell_id, TunnelSlot { descriptor: descriptor.clone(), listener, writer: None });
	Ok(descriptor)
}

async fn accept_session_async<Dispatch>(
	slots: &mut HashMap<GShellId, TunnelSlot>,
	gshell_id: GShellId,
	dispatcher: Dispatch,
	surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
) -> Result<GNativeSessionAccepted, String>
where
	Dispatch: IRuntimeEventDispatcher,
{
	let mut slot = slots
		.remove(&gshell_id)
		.ok_or_else(|| format!("no gnative tunnel slot for {}", gshell_id.value()))?;
	let descriptor = slot.descriptor.clone();

	if slot.writer.is_some() {
		let accepted = GNativeSessionAccepted {
			gshell_id:        descriptor.gshell_id,
			protocol_version: descriptor.protocol_version,
		};
		slots.insert(gshell_id, slot);
		return Ok(accepted);
	}

	let (mut stream, _) = slot.listener.accept().await.map_err(io_error)?;
	stream.set_nodelay(true).ok();

	let message = read_app_message(&mut stream, &mut Vec::new())
		.await?
		.ok_or_else(|| "gnative app closed before hello".to_string())?;
	let GNativeAppToHost::Hello(hello) = message else {
		slots.insert(gshell_id, slot);
		return Err("expected gnative hello during handshake".to_string());
	};

	if hello.token != descriptor.token {
		slots.insert(gshell_id, slot);
		return Err("gnative hello token mismatch".to_string());
	}
	if hello.protocol_version != descriptor.protocol_version {
		slots.insert(gshell_id, slot);
		return Err("gnative protocol version mismatch".to_string());
	}

	let accepted = GNativeSessionAccepted {
		gshell_id:        descriptor.gshell_id,
		protocol_version: descriptor.protocol_version,
	};
	write_host_message(&mut stream, &GNativeHostToApp::Welcome(accepted.clone())).await?;

	let gshell_id = descriptor.gshell_id;
	let (reader, writer) = stream.into_split();
	spawn(async move {
		read_frames_loop(gshell_id, reader, dispatcher, surface_snapshot_tx).await;
	})
	.detach();

	slot.writer = Some(writer);
	slots.insert(gshell_id, slot);
	Ok(accepted)
}

async fn send_input_async(
	slots: &mut HashMap<GShellId, TunnelSlot>,
	gshell_id: GShellId,
	input: GNativeInputEvent,
) -> Result<(), String> {
	let mut slot = slots
		.remove(&gshell_id)
		.ok_or_else(|| format!("no gnative tunnel slot for {}", gshell_id.value()))?;

	let result = match slot.writer.as_mut() {
		Some(writer) => write_host_message(writer, &GNativeHostToApp::Input(input)).await,
		None => Err(format!("no active gnative tunnel session for {}", gshell_id.value())),
	};

	slots.insert(gshell_id, slot);
	result
}

async fn close_session_async(
	slots: &mut HashMap<GShellId, TunnelSlot>,
	gshell_id: GShellId,
) -> Result<(), String> {
	let Some(mut slot) = slots.remove(&gshell_id) else {
		return Ok(());
	};

	if let Some(mut writer) = slot.writer.take() {
		writer.shutdown().await.map_err(io_error)?;
	}

	slots.insert(gshell_id, slot);
	Ok(())
}

async fn write_host_message<W>(writer: &mut W, message: &GNativeHostToApp) -> Result<(), String>
where W: AsyncWrite + Unpin + ?Sized {
	let mut payload = serde_json::to_vec(message).map_err(|error| error.to_string())?;
	payload.push(b'\n');

	let BufResult(result, _) = writer.write_all(payload).await;
	result.map_err(io_error)?;
	writer.flush().await.map_err(io_error)
}

async fn read_app_message<R>(
	reader: &mut R,
	read_buffer: &mut Vec<u8>,
) -> Result<Option<GNativeAppToHost>, String>
where
	R: AsyncRead + Unpin + ?Sized,
{
	loop {
		if let Some(newline_index) = read_buffer.iter().position(|byte| *byte == b'\n') {
			let mut line = read_buffer.drain(..=newline_index).collect::<Vec<_>>();
			while matches!(line.last(), Some(b'\n' | b'\r')) {
				line.pop();
			}
			return serde_json::from_slice(&line).map(Some).map_err(|error| error.to_string());
		}

		let BufResult(result, mut chunk) = reader.read(Vec::with_capacity(4096)).await;
		match result.map_err(io_error)? {
			0 => {
				if read_buffer.is_empty() {
					return Ok(None);
				}
				return Err("gnative app closed mid-message".to_string());
			}
			n => {
				chunk.truncate(n);
				read_buffer.extend_from_slice(&chunk);
			}
		}
	}
}

async fn read_frames_loop<Dispatch>(
	gshell_id: GShellId,
	mut reader: TcpStream,
	dispatcher: Dispatch,
	surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
) where
	Dispatch: IRuntimeEventDispatcher,
{
	let presenter = TextSurfaceFramePlanPresenter::new();
	let mut read_buffer = Vec::new();
	let mut exit_dispatched = false;

	loop {
		let Ok(message) = read_app_message(&mut reader, &mut read_buffer).await else {
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

fn encode_tcp_endpoint(addr: std::net::SocketAddr) -> String {
	format!("{TCP_ENDPOINT_PREFIX}{addr}")
}

fn unique_token() -> String {
	let timestamp =
		SystemTime::now().duration_since(UNIX_EPOCH).expect("clock should be after epoch").as_nanos();
	format!("gnative-{timestamp:x}")
}

fn io_error(error: std::io::Error) -> String { error.to_string() }
