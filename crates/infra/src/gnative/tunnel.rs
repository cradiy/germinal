use std::{
	cell::RefCell,
	collections::HashMap,
	sync::{
		Arc,
		mpsc::{self, Sender},
	},
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
	tunnel::{GNativeAppPayload, GNativeAppToHost, GNativeHostMuxFrame, GNativeHostToApp},
};
use germinal_ports::{
	error::BoxResult,
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
use thiserror::Error;

use crate::{
	gnative::media_bridge::{GNativeMediaBridgeHandle, IGNativeMediaBridge, NoopGNativeMediaBridge},
	rendering::text_surface_frame_plan_presenter::TextSurfaceFramePlanPresenter,
};

const TCP_ENDPOINT_PREFIX: &str = "tcp://";

#[derive(Debug, Error)]
enum GNativeTunnelError {
	#[error("gnative tunnel command channel is closed")]
	CommandChannelClosed,
	#[error("gnative tunnel response channel is closed: {0}")]
	ResponseChannelClosed(#[source] mpsc::RecvError),
	#[error("gnative tunnel is not configured with a dispatcher")]
	DispatcherNotConfigured,
	#[error("gnative tunnel is not configured with a snapshot sender")]
	SnapshotSenderNotConfigured,
	#[error("no gnative tunnel slot for gshell {gshell_id}")]
	MissingTunnelSlot { gshell_id: u64 },
	#[error("no active gnative tunnel session for gshell {gshell_id}")]
	InactiveSession { gshell_id: u64 },
	#[error("gnative app closed before hello")]
	AppClosedBeforeHello,
	#[error("expected gnative hello during handshake")]
	UnexpectedHandshakeMessage,
	#[error("gnative hello token mismatch")]
	TokenMismatch,
	#[error("gnative protocol version mismatch: expected {expected}, got {actual}")]
	ProtocolVersionMismatch { expected: u32, actual: u32 },
	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),
	#[error("failed to encode gnative tunnel message: {0}")]
	EncodeMessage(#[source] serde_json::Error),
	#[error("failed to decode gnative tunnel message: {0}")]
	DecodeMessage(#[source] serde_json::Error),
	#[error("gnative app closed mid-message")]
	AppClosedMidMessage,
}

pub struct GNativeTunnel<Dispatch> {
	command_tx:          flume::Sender<TunnelCommand<Dispatch>>,
	dispatcher:          RefCell<Option<Dispatch>>,
	media_bridge:        RefCell<GNativeMediaBridgeHandle>,
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

	pub fn configure_media_bridge(&self, media_bridge: GNativeMediaBridgeHandle) {
		*self.media_bridge.borrow_mut() = media_bridge;
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

		Self {
			command_tx,
			dispatcher: RefCell::new(None),
			media_bridge: RefCell::new(Arc::new(NoopGNativeMediaBridge)),
			surface_snapshot_tx: RefCell::new(None),
		}
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
	) -> BoxResult<GNativeSessionDescriptor> {
		let (response_tx, response_rx) = mpsc::channel();
		self
			.command_tx
			.send(TunnelCommand::EnsureSessionDescriptor { gshell_id, protocol_version, response_tx })
			.map_err(|_| GNativeTunnelError::CommandChannelClosed)?;

		response_rx.recv().map_err(GNativeTunnelError::ResponseChannelClosed)?
	}

	fn accept_session(&self, gshell_id: GShellId) -> BoxResult<GNativeSessionAccepted> {
		let dispatcher =
			self.dispatcher.borrow().clone().ok_or(GNativeTunnelError::DispatcherNotConfigured)?;
		let surface_snapshot_tx = self
			.surface_snapshot_tx
			.borrow()
			.clone()
			.ok_or(GNativeTunnelError::SnapshotSenderNotConfigured)?;
		let media_bridge = self.media_bridge.borrow().clone();
		let (response_tx, response_rx) = mpsc::channel();

		self
			.command_tx
			.send(TunnelCommand::AcceptSession {
				gshell_id,
				dispatcher,
				media_bridge,
				surface_snapshot_tx,
				response_tx,
			})
			.map_err(|_| GNativeTunnelError::CommandChannelClosed)?;

		response_rx.recv().map_err(GNativeTunnelError::ResponseChannelClosed)?
	}

	fn send_input(&self, gshell_id: GShellId, input: GNativeInputEvent) -> BoxResult<()> {
		let (response_tx, response_rx) = mpsc::channel();
		self
			.command_tx
			.send(TunnelCommand::SendInput { gshell_id, input, response_tx })
			.map_err(|_| GNativeTunnelError::CommandChannelClosed)?;

		response_rx.recv().map_err(GNativeTunnelError::ResponseChannelClosed)?
	}

	fn close_session(&self, gshell_id: GShellId) -> BoxResult<()> {
		let (response_tx, response_rx) = mpsc::channel();
		self
			.command_tx
			.send(TunnelCommand::CloseSession { gshell_id, response_tx })
			.map_err(|_| GNativeTunnelError::CommandChannelClosed)?;

		response_rx.recv().map_err(GNativeTunnelError::ResponseChannelClosed)?
	}
}

enum TunnelCommand<Dispatch> {
	EnsureSessionDescriptor {
		gshell_id:        GShellId,
		protocol_version: u32,
		response_tx:      mpsc::Sender<BoxResult<GNativeSessionDescriptor>>,
	},
	AcceptSession {
		gshell_id:           GShellId,
		dispatcher:          Dispatch,
		media_bridge:        GNativeMediaBridgeHandle,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		response_tx:         mpsc::Sender<BoxResult<GNativeSessionAccepted>>,
	},
	SendInput {
		gshell_id:   GShellId,
		input:       GNativeInputEvent,
		response_tx: mpsc::Sender<BoxResult<()>>,
	},
	CloseSession {
		gshell_id:   GShellId,
		response_tx: mpsc::Sender<BoxResult<()>>,
	},
}

struct TunnelSlot {
	descriptor:        GNativeSessionDescriptor,
	listener:          TcpListener,
	writer:            Option<TcpStream>,
	next_host_mux_seq: u64,
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
			TunnelCommand::AcceptSession {
				gshell_id,
				dispatcher,
				media_bridge,
				surface_snapshot_tx,
				response_tx,
			} => {
				let result = accept_session_async(
					&mut slots,
					gshell_id,
					dispatcher,
					media_bridge,
					surface_snapshot_tx,
				)
				.await;
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
) -> BoxResult<GNativeSessionDescriptor> {
	if let Some(slot) = slots.get(&gshell_id) {
		return Ok(slot.descriptor.clone());
	}

	let listener = TcpListener::bind("127.0.0.1:0").await?;
	let endpoint = encode_tcp_endpoint(listener.local_addr()?);
	let descriptor =
		GNativeSessionDescriptor { gshell_id, endpoint, token: unique_token(), protocol_version };

	slots.insert(gshell_id, TunnelSlot {
		descriptor: descriptor.clone(),
		listener,
		writer: None,
		next_host_mux_seq: 1,
	});
	Ok(descriptor)
}

async fn accept_session_async<Dispatch>(
	slots: &mut HashMap<GShellId, TunnelSlot>,
	gshell_id: GShellId,
	dispatcher: Dispatch,
	media_bridge: GNativeMediaBridgeHandle,
	surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
) -> BoxResult<GNativeSessionAccepted>
where
	Dispatch: IRuntimeEventDispatcher,
{
	let mut slot = slots
		.remove(&gshell_id)
		.ok_or(GNativeTunnelError::MissingTunnelSlot { gshell_id: gshell_id.value() })?;
	let descriptor = slot.descriptor.clone();

	if slot.writer.is_some() {
		let accepted = GNativeSessionAccepted {
			gshell_id:        descriptor.gshell_id,
			protocol_version: descriptor.protocol_version,
		};
		slots.insert(gshell_id, slot);
		return Ok(accepted);
	}

	let (mut stream, _) = slot.listener.accept().await?;
	stream.set_nodelay(true).ok();

	let message = read_app_message(&mut stream, &mut Vec::new())
		.await?
		.ok_or(GNativeTunnelError::AppClosedBeforeHello)?;
	let GNativeAppToHost::Hello(hello) = message else {
		slots.insert(gshell_id, slot);
		return Err(GNativeTunnelError::UnexpectedHandshakeMessage.into());
	};

	if hello.token != descriptor.token {
		slots.insert(gshell_id, slot);
		return Err(GNativeTunnelError::TokenMismatch.into());
	}
	if hello.protocol_version != descriptor.protocol_version {
		slots.insert(gshell_id, slot);
		return Err(
			GNativeTunnelError::ProtocolVersionMismatch {
				expected: descriptor.protocol_version,
				actual:   hello.protocol_version,
			}
			.into(),
		);
	}

	let accepted = GNativeSessionAccepted {
		gshell_id:        descriptor.gshell_id,
		protocol_version: descriptor.protocol_version,
	};
	write_host_message(&mut stream, &GNativeHostToApp::Welcome(accepted.clone())).await?;

	let gshell_id = descriptor.gshell_id;
	let (reader, writer) = stream.into_split();
	spawn(async move {
		read_frames_loop(gshell_id, reader, dispatcher, media_bridge, surface_snapshot_tx).await;
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
) -> BoxResult<()> {
	let mut slot = slots
		.remove(&gshell_id)
		.ok_or(GNativeTunnelError::MissingTunnelSlot { gshell_id: gshell_id.value() })?;
	let mux_seq = next_host_mux_seq(&mut slot);

	let result = match slot.writer.as_mut() {
		Some(writer) => {
			write_host_message(writer, &GNativeHostToApp::Mux(GNativeHostMuxFrame::input(mux_seq, input)))
				.await
		}
		None => Err(GNativeTunnelError::InactiveSession { gshell_id: gshell_id.value() }.into()),
	};

	slots.insert(gshell_id, slot);
	result
}

async fn close_session_async(
	slots: &mut HashMap<GShellId, TunnelSlot>,
	gshell_id: GShellId,
) -> BoxResult<()> {
	let Some(mut slot) = slots.remove(&gshell_id) else {
		return Ok(());
	};

	if let Some(mut writer) = slot.writer.take() {
		writer.shutdown().await?;
	}

	slots.insert(gshell_id, slot);
	Ok(())
}

async fn write_host_message<W>(writer: &mut W, message: &GNativeHostToApp) -> BoxResult<()>
where W: AsyncWrite + Unpin + ?Sized {
	let mut payload = serde_json::to_vec(message).map_err(GNativeTunnelError::EncodeMessage)?;
	payload.push(b'\n');

	let BufResult(result, _) = writer.write_all(payload).await;
	result?;
	writer.flush().await?;
	Ok(())
}

async fn read_app_message<R>(
	reader: &mut R,
	read_buffer: &mut Vec<u8>,
) -> BoxResult<Option<GNativeAppToHost>>
where
	R: AsyncRead + Unpin + ?Sized,
{
	loop {
		if let Some(newline_index) = read_buffer.iter().position(|byte| *byte == b'\n') {
			let mut line = read_buffer.drain(..=newline_index).collect::<Vec<_>>();
			while matches!(line.last(), Some(b'\n' | b'\r')) {
				line.pop();
			}
			return serde_json::from_slice(&line)
				.map(Some)
				.map_err(|source| GNativeTunnelError::DecodeMessage(source).into());
		}

		let BufResult(result, mut chunk) = reader.read(Vec::with_capacity(4096)).await;
		match result? {
			0 => {
				if read_buffer.is_empty() {
					return Ok(None);
				}
				return Err(GNativeTunnelError::AppClosedMidMessage.into());
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
	media_bridge: GNativeMediaBridgeHandle,
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
			GNativeAppToHost::Mux(frame) => handle_app_mux_payload(
				frame.payload,
				&dispatcher,
				media_bridge.as_ref(),
				&surface_snapshot_tx,
				&presenter,
			),
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

fn handle_app_mux_payload<Dispatch>(
	payload: GNativeAppPayload,
	dispatcher: &Dispatch,
	media_bridge: &dyn IGNativeMediaBridge,
	surface_snapshot_tx: &Sender<RenderSurfaceSnapshot>,
	presenter: &TextSurfaceFramePlanPresenter,
) where
	Dispatch: IRuntimeEventDispatcher,
{
	match payload {
		GNativeAppPayload::Render(frame) => {
			present_frame(frame, dispatcher, surface_snapshot_tx, presenter)
		}
		GNativeAppPayload::Control(command) => {
			media_bridge.handle_media_control_command(command);
		}
		GNativeAppPayload::Audio(packet) => {
			media_bridge.handle_audio_packet(packet);
		}
		GNativeAppPayload::Video(packet) => {
			media_bridge.handle_video_packet(packet);
		}
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

fn next_host_mux_seq(slot: &mut TunnelSlot) -> u64 {
	let mux_seq = slot.next_host_mux_seq;
	slot.next_host_mux_seq += 1;
	mux_seq
}

fn unique_token() -> String {
	let timestamp =
		SystemTime::now().duration_since(UNIX_EPOCH).expect("clock should be after epoch").as_nanos();
	format!("gnative-{timestamp:x}")
}
