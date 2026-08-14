use std::{
	cell::RefCell,
	collections::HashMap,
	rc::Rc,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
		mpsc::{self, Sender},
	},
	thread,
	time::{Duration, SystemTime, UNIX_EPOCH},
};

use compio::{
	BufResult,
	io::{AsyncRead, AsyncWrite, AsyncWriteExt},
	net::{TcpListener, TcpStream},
	runtime::{Runtime, spawn},
	time::timeout,
};
use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::{
	frame::{GNativeFrame, GNativeFrameCursor},
	input::GNativeInputEvent,
	session::{GNativeSessionAccepted, GNativeSessionDescriptor},
	tunnel::{
		GNATIVE_MAX_MESSAGE_BYTES, GNativeAppPayload, GNativeAppToHost, GNativeHostMuxFrame,
		GNativeHostToApp,
	},
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
	service::gnative_tunnel::{GNativeTunnelError, IGNativeTunnel},
};
use tracing::warn;

use crate::{
	gnative::media_bridge::{GNativeMediaBridgeHandle, IGNativeMediaBridge, NoopGNativeMediaBridge},
	rendering::text_surface_frame_plan_presenter::TextSurfaceFramePlanPresenter,
};

const TCP_ENDPOINT_PREFIX: &str = "tcp://";
const TUNNEL_COMMAND_QUEUE_CAPACITY: usize = 256;
const ACCEPT_SESSION_TIMEOUT: Duration = Duration::from_secs(10);

pub struct GNativeTunnel<Dispatch> {
	command_tx:             flume::Sender<TunnelCommand<Dispatch>>,
	dispatcher:             RefCell<Option<Dispatch>>,
	media_bridge:           RefCell<GNativeMediaBridgeHandle>,
	snapshot_wake_pending:  RefCell<Arc<AtomicBool>>,
	surface_snapshot_tx:    RefCell<Option<Sender<RenderSurfaceSnapshot>>>,
	accept_session_timeout: Duration,
}

impl<Dispatch> GNativeTunnel<Dispatch> {
	pub fn configure(
		&self,
		dispatcher: Dispatch,
		snapshot_wake_pending: Arc<AtomicBool>,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
	) {
		*self.dispatcher.borrow_mut() = Some(dispatcher);
		snapshot_wake_pending.store(false, Ordering::Release);
		*self.snapshot_wake_pending.borrow_mut() = snapshot_wake_pending;
		*self.surface_snapshot_tx.borrow_mut() = Some(surface_snapshot_tx);
	}

	pub fn configure_media_bridge(&self, media_bridge: GNativeMediaBridgeHandle) {
		*self.media_bridge.borrow_mut() = media_bridge;
	}

	fn enqueue_command(&self, command: TunnelCommand<Dispatch>) -> Result<(), GNativeTunnelError> {
		self.command_tx.try_send(command).map_err(|error| match error {
			flume::TrySendError::Full(_) => GNativeTunnelError::CommandQueueFull,
			flume::TrySendError::Disconnected(_) => GNativeTunnelError::CommandChannelClosed,
		})
	}
}

impl<Dispatch> GNativeTunnel<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	pub fn new() -> Result<Self, GNativeTunnelError> {
		Self::new_with_accept_timeout(ACCEPT_SESSION_TIMEOUT)
	}

	fn new_with_accept_timeout(
		accept_session_timeout: Duration,
	) -> Result<Self, GNativeTunnelError> {
		let (command_tx, command_rx) = flume::bounded(TUNNEL_COMMAND_QUEUE_CAPACITY);
		let (ready_tx, ready_rx) = mpsc::channel();

		thread::Builder::new()
			.name("gnative-tunnel".to_string())
			.spawn(move || {
				let runtime = match Runtime::new() {
					Ok(runtime) => {
						let _ = ready_tx.send(Ok(()));
						runtime
					}
					Err(source) => {
						let _ = ready_tx.send(Err(source));
						return;
					}
				};
				runtime.block_on(run_tunnel_runtime(command_rx));
			})
			.map_err(|source| GNativeTunnelError::SpawnRuntimeThread { source })?;
		ready_rx
			.recv()
			.map_err(GNativeTunnelError::RuntimeBootstrapChannelClosed)?
			.map_err(|source| GNativeTunnelError::CreateRuntime { source })?;

		Ok(Self {
			command_tx,
			dispatcher: RefCell::new(None),
			media_bridge: RefCell::new(Arc::new(NoopGNativeMediaBridge)),
			snapshot_wake_pending: RefCell::new(Arc::new(AtomicBool::new(false))),
			surface_snapshot_tx: RefCell::new(None),
			accept_session_timeout,
		})
	}
}

impl<Dispatch> IGNativeTunnel for GNativeTunnel<Dispatch>
where Dispatch: IRuntimeEventDispatcher
{
	fn ensure_session_descriptor(
		&self,
		gshell_id: GShellId,
		protocol_version: u32,
	) -> Result<GNativeSessionDescriptor, GNativeTunnelError> {
		let (response_tx, response_rx) = mpsc::channel();
		self.enqueue_command(TunnelCommand::EnsureSessionDescriptor {
			gshell_id,
			protocol_version,
			response_tx,
		})?;

		response_rx.recv().map_err(GNativeTunnelError::ResponseChannelClosed)?
	}

	fn begin_accept_session(&self, gshell_id: GShellId) -> Result<(), GNativeTunnelError> {
		let dispatcher =
			self.dispatcher.borrow().clone().ok_or(GNativeTunnelError::DispatcherNotConfigured)?;
		let surface_snapshot_tx = self
			.surface_snapshot_tx
			.borrow()
			.clone()
			.ok_or(GNativeTunnelError::SnapshotSenderNotConfigured)?;
		let snapshot_wake_pending = Arc::clone(&self.snapshot_wake_pending.borrow());
		let media_bridge = self.media_bridge.borrow().clone();
		self.enqueue_command(TunnelCommand::BeginAcceptSession {
			gshell_id,
			dispatcher,
			media_bridge,
			snapshot_wake_pending,
			surface_snapshot_tx,
			accept_timeout: self.accept_session_timeout,
		})
	}

	fn send_input(
		&self,
		gshell_id: GShellId,
		input: GNativeInputEvent,
	) -> Result<(), GNativeTunnelError> {
		self.enqueue_command(TunnelCommand::SendInput { gshell_id, input })
	}

	fn close_session(&self, gshell_id: GShellId) -> Result<(), GNativeTunnelError> {
		self.enqueue_command(TunnelCommand::CloseSession { gshell_id })
	}
}

enum TunnelCommand<Dispatch> {
	EnsureSessionDescriptor {
		gshell_id:        GShellId,
		protocol_version: u32,
		response_tx:      mpsc::Sender<Result<GNativeSessionDescriptor, GNativeTunnelError>>,
	},
	BeginAcceptSession {
		gshell_id:             GShellId,
		dispatcher:            Dispatch,
		media_bridge:          GNativeMediaBridgeHandle,
		snapshot_wake_pending: Arc<AtomicBool>,
		surface_snapshot_tx:   Sender<RenderSurfaceSnapshot>,
		accept_timeout:        Duration,
	},
	SendInput {
		gshell_id: GShellId,
		input:     GNativeInputEvent,
	},
	CloseSession { gshell_id: GShellId },
}

struct TunnelSlot {
	descriptor:        GNativeSessionDescriptor,
	listener:          TcpListener,
	writer:            Option<TcpStream>,
	next_host_mux_seq: u64,
	accepting:         bool,
	accept_generation: u64,
}

async fn run_tunnel_runtime<Dispatch>(command_rx: flume::Receiver<TunnelCommand<Dispatch>>)
where Dispatch: IRuntimeEventDispatcher {
	let slots = Rc::new(RefCell::new(HashMap::<GShellId, TunnelSlot>::new()));

	while let Ok(command) = command_rx.recv_async().await {
		match command {
			TunnelCommand::EnsureSessionDescriptor { gshell_id, protocol_version, response_tx } => {
				let result = ensure_session_descriptor_async(
					Rc::clone(&slots),
					gshell_id,
					protocol_version,
				)
				.await;
				let _ = response_tx.send(result);
			}
			TunnelCommand::BeginAcceptSession {
				gshell_id,
				dispatcher,
				media_bridge,
				snapshot_wake_pending,
				surface_snapshot_tx,
				accept_timeout,
			} => {
				let begin = begin_accept(&slots, gshell_id);
				match begin {
					BeginAccept::Connected(accepted) => {
						dispatch_gnative_connected(accepted, &dispatcher);
					}
					BeginAccept::Pending => {}
					BeginAccept::Failed(error) => {
						dispatch_gnative_connection_failed(gshell_id, error.to_string(), &dispatcher);
					}
					BeginAccept::Start { descriptor, listener, generation } => {
						let slots = Rc::clone(&slots);
						spawn(async move {
							let result = timeout(
								accept_timeout,
								accept_session_async(
									descriptor,
									listener,
									dispatcher.clone(),
									media_bridge,
									snapshot_wake_pending,
									surface_snapshot_tx,
								),
							)
							.await
							.unwrap_or_else(|_| {
								Err(GNativeTunnelError::AcceptConnectionTimeout {
									gshell_id: gshell_id.value(),
								})
							});

							complete_accept(&slots, gshell_id, generation, result, &dispatcher).await;
						})
						.detach();
					}
				}
			}
			TunnelCommand::SendInput { gshell_id, input } => {
				if let Err(error) = send_input_async(&slots, gshell_id, input).await {
					warn!(gshell_id = gshell_id.value(), error = %error, "failed to send queued gnative input");
				}
			}
			TunnelCommand::CloseSession { gshell_id } => {
				if let Err(error) = close_session_async(&slots, gshell_id).await {
					warn!(gshell_id = gshell_id.value(), error = %error, "failed to close queued gnative session");
				}
			}
		}
	}
}

async fn ensure_session_descriptor_async(
	slots: Rc<RefCell<HashMap<GShellId, TunnelSlot>>>,
	gshell_id: GShellId,
	protocol_version: u32,
) -> Result<GNativeSessionDescriptor, GNativeTunnelError> {
	if let Some(slot) = slots.borrow().get(&gshell_id) {
		return Ok(slot.descriptor.clone());
	}

	let listener = TcpListener::bind("127.0.0.1:0")
		.await
		.map_err(|source| GNativeTunnelError::BindListener { source })?;
	let endpoint = encode_tcp_endpoint(
		listener
			.local_addr()
			.map_err(|source| GNativeTunnelError::ResolveListenerAddress { source })?,
	);
	let descriptor =
		GNativeSessionDescriptor { gshell_id, endpoint, token: unique_token(), protocol_version };

	slots.borrow_mut().insert(gshell_id, TunnelSlot {
		descriptor: descriptor.clone(),
		listener,
		writer: None,
		next_host_mux_seq: 1,
		accepting: false,
		accept_generation: 0,
	});
	Ok(descriptor)
}

enum BeginAccept {
	Connected(GNativeSessionAccepted),
	Pending,
	Start {
		descriptor: GNativeSessionDescriptor,
		listener: TcpListener,
		generation: u64,
	},
	Failed(GNativeTunnelError),
}

fn begin_accept(
	slots: &Rc<RefCell<HashMap<GShellId, TunnelSlot>>>,
	gshell_id: GShellId,
) -> BeginAccept {
	let mut slots = slots.borrow_mut();
	let Some(slot) = slots.get_mut(&gshell_id) else {
		return BeginAccept::Failed(GNativeTunnelError::MissingTunnelSlot {
			gshell_id: gshell_id.value(),
		});
	};

	if slot.writer.is_some() {
		return BeginAccept::Connected(GNativeSessionAccepted {
			gshell_id,
			protocol_version: slot.descriptor.protocol_version,
		});
	}
	if slot.accepting {
		return BeginAccept::Pending;
	}

	slot.accepting = true;
	slot.accept_generation += 1;
	BeginAccept::Start {
		descriptor: slot.descriptor.clone(),
		listener: slot.listener.clone(),
		generation: slot.accept_generation,
	}
}

async fn accept_session_async<Dispatch>(
	descriptor: GNativeSessionDescriptor,
	listener: TcpListener,
	dispatcher: Dispatch,
	media_bridge: GNativeMediaBridgeHandle,
	snapshot_wake_pending: Arc<AtomicBool>,
	surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
) -> Result<(GNativeSessionAccepted, TcpStream), GNativeTunnelError>
where
	Dispatch: IRuntimeEventDispatcher,
{
	let (mut stream, _) = listener.accept().await.map_err(|source| {
		GNativeTunnelError::AcceptConnection { gshell_id: descriptor.gshell_id.value(), source }
	})?;
	stream.set_nodelay(true).ok();

	let message = read_app_message(&mut stream, &mut Vec::new())
		.await?
		.ok_or(GNativeTunnelError::AppClosedBeforeHello)?;
	let GNativeAppToHost::Hello(hello) = message else {
		return Err(GNativeTunnelError::UnexpectedHandshakeMessage);
	};

	if hello.token != descriptor.token {
		return Err(GNativeTunnelError::TokenMismatch);
	}
	if hello.protocol_version != descriptor.protocol_version {
		return Err(GNativeTunnelError::ProtocolVersionMismatch {
			expected: descriptor.protocol_version,
			actual:   hello.protocol_version,
		});
	}

	let accepted = GNativeSessionAccepted {
		gshell_id:        descriptor.gshell_id,
		protocol_version: descriptor.protocol_version,
	};
	write_host_message(&mut stream, &GNativeHostToApp::Welcome(accepted.clone())).await?;

	let gshell_id = descriptor.gshell_id;
	let (reader, writer) = stream.into_split();
	spawn(async move {
		read_frames_loop(
			gshell_id,
			reader,
			dispatcher,
			media_bridge,
			snapshot_wake_pending,
			surface_snapshot_tx,
		)
		.await;
	})
	.detach();

	Ok((accepted, writer))
}

async fn complete_accept<Dispatch>(
	slots: &Rc<RefCell<HashMap<GShellId, TunnelSlot>>>,
	gshell_id: GShellId,
	generation: u64,
	result: Result<(GNativeSessionAccepted, TcpStream), GNativeTunnelError>,
	dispatcher: &Dispatch,
) where
	Dispatch: IRuntimeEventDispatcher,
{
	let is_current = slots
		.borrow()
		.get(&gshell_id)
		.is_some_and(|slot| slot.accepting && slot.accept_generation == generation);
	if !is_current {
		if let Ok((_, mut writer)) = result {
			let _ = writer.shutdown().await;
		}
		return;
	}

	match result {
		Ok((accepted, writer)) => {
			if let Some(slot) = slots.borrow_mut().get_mut(&gshell_id) {
				slot.accepting = false;
				slot.writer = Some(writer);
			}
			dispatch_gnative_connected(accepted, dispatcher);
		}
		Err(error) => {
			if let Some(slot) = slots.borrow_mut().get_mut(&gshell_id) {
				slot.accepting = false;
			}
			dispatch_gnative_connection_failed(gshell_id, error.to_string(), dispatcher);
		}
	}
}

async fn send_input_async(
	slots: &Rc<RefCell<HashMap<GShellId, TunnelSlot>>>,
	gshell_id: GShellId,
	input: GNativeInputEvent,
) -> Result<(), GNativeTunnelError> {
	let mut slot = slots
		.borrow_mut()
		.remove(&gshell_id)
		.ok_or(GNativeTunnelError::MissingTunnelSlot { gshell_id: gshell_id.value() })?;
	let mux_seq = next_host_mux_seq(&mut slot);

	let result = match slot.writer.as_mut() {
		Some(writer) => {
			write_host_message(writer, &GNativeHostToApp::Mux(GNativeHostMuxFrame::input(mux_seq, input)))
				.await
		}
		None => Err(GNativeTunnelError::InactiveSession { gshell_id: gshell_id.value() }),
	};

	if result.is_err() {
		slot.writer = None;
	}
	slots.borrow_mut().insert(gshell_id, slot);
	result
}

async fn close_session_async(
	slots: &Rc<RefCell<HashMap<GShellId, TunnelSlot>>>,
	gshell_id: GShellId,
) -> Result<(), GNativeTunnelError> {
	let Some(mut slot) = slots.borrow_mut().remove(&gshell_id) else {
		return Ok(());
	};
	slot.accepting = false;
	slot.accept_generation += 1;

	if let Some(mut writer) = slot.writer.take() {
		let result = writer.shutdown().await.map_err(|source| GNativeTunnelError::ShutdownSession {
			gshell_id: gshell_id.value(),
			source,
		});
		slots.borrow_mut().insert(gshell_id, slot);
		return result;
	}

	slots.borrow_mut().insert(gshell_id, slot);
	Ok(())
}

async fn write_host_message<W>(
	writer: &mut W,
	message: &GNativeHostToApp,
) -> Result<(), GNativeTunnelError>
where
	W: AsyncWrite + Unpin + ?Sized,
{
	let mut payload = serde_json::to_vec(message).map_err(GNativeTunnelError::EncodeMessage)?;
	payload.push(b'\n');

	let BufResult(result, _) = writer.write_all(payload).await;
	result.map_err(|source| GNativeTunnelError::WriteMessage { source })?;
	writer.flush().await.map_err(|source| GNativeTunnelError::FlushMessage { source })?;
	Ok(())
}

async fn read_app_message<R>(
	reader: &mut R,
	read_buffer: &mut Vec<u8>,
) -> Result<Option<GNativeAppToHost>, GNativeTunnelError>
where
	R: AsyncRead + Unpin + ?Sized,
{
	loop {
		if let Some(message) = take_buffered_app_message(read_buffer, GNATIVE_MAX_MESSAGE_BYTES)? {
			return Ok(Some(message));
		}

		let BufResult(result, mut chunk) = reader.read(Vec::with_capacity(4096)).await;
		match result.map_err(|source| GNativeTunnelError::ReadMessage { source })? {
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

fn take_buffered_app_message(
	read_buffer: &mut Vec<u8>,
	max_bytes: usize,
) -> Result<Option<GNativeAppToHost>, GNativeTunnelError> {
	let Some(newline_index) = read_buffer.iter().position(|byte| *byte == b'\n') else {
		if read_buffer.len() > max_bytes {
			return Err(GNativeTunnelError::MessageTooLarge { max_bytes });
		}
		return Ok(None);
	};
	if newline_index > max_bytes {
		return Err(GNativeTunnelError::MessageTooLarge { max_bytes });
	}

	let mut line = read_buffer.drain(..=newline_index).collect::<Vec<_>>();
	while matches!(line.last(), Some(b'\n' | b'\r')) {
		line.pop();
	}
	serde_json::from_slice(&line).map(Some).map_err(GNativeTunnelError::DecodeMessage)
}

async fn read_frames_loop<Dispatch>(
	gshell_id: GShellId,
	mut reader: TcpStream,
	dispatcher: Dispatch,
	media_bridge: GNativeMediaBridgeHandle,
	snapshot_wake_pending: Arc<AtomicBool>,
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
				gshell_id,
				frame.payload,
				&dispatcher,
				media_bridge.as_ref(),
				snapshot_wake_pending.as_ref(),
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
	gshell_id: GShellId,
	payload: GNativeAppPayload,
	dispatcher: &Dispatch,
	media_bridge: &dyn IGNativeMediaBridge,
	snapshot_wake_pending: &AtomicBool,
	surface_snapshot_tx: &Sender<RenderSurfaceSnapshot>,
	presenter: &TextSurfaceFramePlanPresenter,
) where
	Dispatch: IRuntimeEventDispatcher,
{
	match payload {
		GNativeAppPayload::Render(frame) => {
			present_frame(
				gshell_id,
				frame,
				dispatcher,
				snapshot_wake_pending,
				surface_snapshot_tx,
				presenter,
			)
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
	session_gshell_id: GShellId,
	frame: GNativeFrame,
	dispatcher: &Dispatch,
	snapshot_wake_pending: &AtomicBool,
	surface_snapshot_tx: &Sender<RenderSurfaceSnapshot>,
	presenter: &TextSurfaceFramePlanPresenter,
) where
	Dispatch: IRuntimeEventDispatcher,
{
	if frame.gshell_id != session_gshell_id {
		warn!(
			session_gshell_id = session_gshell_id.value(),
			frame_gshell_id = frame.gshell_id.value(),
			"dropped gnative frame targeting a different session"
		);
		return;
	}

	let target_id = RenderTargetId::new(frame.gshell_id.value());
	let plan = BuiltFramePlan { target_id, seq: frame.seq, commands: frame.commands };
	if !presenter.present(&plan) {
		return;
	}
	let Some(mut snapshot) = presenter.surface_snapshot_of(target_id) else {
		return;
	};
	snapshot.cursor = frame.cursor.map(cursor_snapshot);

	if let Err(error) = surface_snapshot_tx.send(snapshot) {
		warn!(gshell_id = frame.gshell_id.value(), error = %error, "failed to publish gnative surface snapshot");
		return;
	}

	if snapshot_wake_pending.swap(true, Ordering::AcqRel) {
		return;
	}

	if let Err(error) = dispatcher.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady {
		gshell_id: frame.gshell_id,
		seq:       frame.seq,
	})) {
		warn!(
			gshell_id = frame.gshell_id.value(),
			error = %error,
			"failed to dispatch gnative frame-ready event"
		);
	}
}

fn cursor_snapshot(cursor: GNativeFrameCursor) -> RenderSurfaceCursorSnapshot {
	RenderSurfaceCursorSnapshot {
		x: cursor.x,
		y: cursor.y,
		focused: true,
		shape: Default::default(),
	}
}

fn dispatch_exit_gnative<Dispatch>(gshell_id: GShellId, dispatcher: &Dispatch)
where Dispatch: IRuntimeEventDispatcher {
	if let Err(error) =
		dispatcher.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::ExitGNative { gshell_id }))
	{
		warn!(gshell_id = gshell_id.value(), error = %error, "failed to dispatch gnative exit event");
	}
}

fn dispatch_gnative_connected<Dispatch>(accepted: GNativeSessionAccepted, dispatcher: &Dispatch)
where Dispatch: IRuntimeEventDispatcher {
	let gshell_id = accepted.gshell_id;
	if let Err(error) = dispatcher.dispatch(RuntimeEvent::GShell(
		GShellRuntimeEvent::GNativeConnected { accepted },
	)) {
		warn!(gshell_id = gshell_id.value(), error = %error, "failed to dispatch gnative connected event");
	}
}

fn dispatch_gnative_connection_failed<Dispatch>(
	gshell_id: GShellId,
	reason: String,
	dispatcher: &Dispatch,
) where
	Dispatch: IRuntimeEventDispatcher,
{
	if let Err(error) = dispatcher.dispatch(RuntimeEvent::GShell(
		GShellRuntimeEvent::GNativeConnectionFailed { gshell_id, reason },
	)) {
		warn!(gshell_id = gshell_id.value(), error = %error, "failed to dispatch gnative connection failure");
	}
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
	let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
	format!("gnative-{timestamp:x}")
}

#[cfg(test)]
mod tests {
	use std::{
		sync::{
			Arc, Mutex,
			atomic::{AtomicBool, Ordering},
			mpsc,
		},
		time::{Duration, Instant},
	};

	use germinal_domain::gshell::vo::gshell_id::GShellId;
	use germinal_gnative_protocol::{
		gnative::{
			frame::{GNativeFrame, GNativeFrameCursor},
			input::GNativeInputEvent,
		},
		rendering::frame_plan_builder::RenderCommandDto,
	};
	use germinal_ports::{
		event::{
			runtime_event::{GShellRuntimeEvent, RuntimeEvent},
			runtime_event_dispatcher::{IRuntimeEventDispatcher, RuntimeEventDispatchError},
		},
		rendering::render_target_id::RenderTargetId,
		seq::Seq,
		service::gnative_tunnel::{GNativeTunnelError, IGNativeTunnel},
	};

	use super::{
		GNativeTunnel, TextSurfaceFramePlanPresenter, present_frame, take_buffered_app_message,
	};

	#[derive(Clone, Default)]
	struct TestDispatcher {
		events: Arc<Mutex<Vec<RuntimeEvent>>>,
	}

	impl TestDispatcher {
		fn events(&self) -> Vec<RuntimeEvent> { self.events.lock().expect("events lock").clone() }
	}

	impl IRuntimeEventDispatcher for TestDispatcher {
		fn dispatch(&self, event: RuntimeEvent) -> Result<(), RuntimeEventDispatchError> {
			self.events.lock().expect("events lock").push(event);
			Ok(())
		}
	}

	#[test]
	fn begin_accept_is_non_blocking_and_timeout_dispatches_failure() {
		let dispatcher = TestDispatcher::default();
		let (snapshot_tx, _snapshot_rx) = mpsc::channel();
		let tunnel = GNativeTunnel::new_with_accept_timeout(Duration::from_millis(250))
			.expect("tunnel should initialize");
		tunnel.configure(
			dispatcher.clone(),
			Arc::new(AtomicBool::new(false)),
			snapshot_tx,
		);
		let gshell_id = GShellId::new(8);
		tunnel
			.ensure_session_descriptor(gshell_id, 1)
			.expect("descriptor should be created");

		let started = Instant::now();
		tunnel.begin_accept_session(gshell_id).expect("accept should be queued");
		assert!(started.elapsed() < Duration::from_millis(50));
		let input_started = Instant::now();
		tunnel
			.send_input(gshell_id, GNativeInputEvent::Bytes(vec![b'a']))
			.expect("input should be queued without waiting for a session response");
		assert!(input_started.elapsed() < Duration::from_millis(50));

		let deadline = Instant::now() + Duration::from_secs(1);
		loop {
			let failure = dispatcher.events().into_iter().find_map(|event| match event {
				RuntimeEvent::GShell(GShellRuntimeEvent::GNativeConnectionFailed {
					gshell_id: failed_id,
					reason,
				}) if failed_id == gshell_id => Some(reason),
				_ => None,
			});
			if let Some(reason) = failure {
				assert!(reason.contains("timed out"));
				break;
			}
			assert!(Instant::now() < deadline, "connection failure event should arrive");
			std::thread::sleep(Duration::from_millis(5));
		}
	}

	#[test]
	fn present_frame_coalesces_frame_ready_wakeups_while_snapshot_is_pending() {
		let dispatcher = TestDispatcher::default();
		let wake_pending = AtomicBool::new(false);
		let (snapshot_tx, snapshot_rx) = mpsc::channel();
		let presenter = TextSurfaceFramePlanPresenter::new();
		let gshell_id = GShellId::new(7);

		present_frame(
			gshell_id,
			GNativeFrame {
				gshell_id,
				seq: Seq::new(1),
				commands: vec![RenderCommandDto::Clear],
				cursor: None,
			},
			&dispatcher,
			&wake_pending,
			&snapshot_tx,
			&presenter,
		);
		present_frame(
			gshell_id,
			GNativeFrame {
				gshell_id,
				seq: Seq::new(2),
				commands: vec![RenderCommandDto::Clear],
				cursor: None,
			},
			&dispatcher,
			&wake_pending,
			&snapshot_tx,
			&presenter,
		);

		let events = dispatcher.events();
		assert_eq!(events.len(), 1);
		assert_eq!(
			events[0],
			RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady { gshell_id, seq: Seq::new(1) })
		);
		assert_eq!(snapshot_rx.recv().expect("first snapshot").latest_seq, Seq::new(1));
		assert_eq!(snapshot_rx.recv().expect("second snapshot").latest_seq, Seq::new(2));
	}

	#[test]
	fn present_frame_drops_stale_frame_before_snapshot_and_wakeup() {
		let dispatcher = TestDispatcher::default();
		let wake_pending = AtomicBool::new(false);
		let (snapshot_tx, snapshot_rx) = mpsc::channel();
		let presenter = TextSurfaceFramePlanPresenter::new();
		let gshell_id = GShellId::new(7);

		present_frame(
			gshell_id,
			GNativeFrame {
				gshell_id,
				seq: Seq::new(2),
				commands: vec![RenderCommandDto::Clear],
				cursor: None,
			},
			&dispatcher,
			&wake_pending,
			&snapshot_tx,
			&presenter,
		);
		wake_pending.store(false, Ordering::Release);
		let _ = snapshot_rx.recv().expect("new snapshot");
		present_frame(
			gshell_id,
			GNativeFrame {
				gshell_id,
				seq: Seq::new(1),
				commands: vec![RenderCommandDto::Clear],
				cursor: Some(GNativeFrameCursor { x: 99, y: 99 }),
			},
			&dispatcher,
			&wake_pending,
			&snapshot_tx,
			&presenter,
		);

		assert!(snapshot_rx.try_recv().is_err());
		assert!(!wake_pending.load(Ordering::Acquire));
		assert_eq!(dispatcher.events().len(), 1);
		assert_eq!(presenter.surface_of(RenderTargetId::new(7)).unwrap().latest_seq, Seq::new(2));
	}

	#[test]
	fn present_frame_rejects_a_frame_for_another_gshell() {
		let dispatcher = TestDispatcher::default();
		let wake_pending = AtomicBool::new(false);
		let (snapshot_tx, snapshot_rx) = mpsc::channel();
		let presenter = TextSurfaceFramePlanPresenter::new();
		let session_gshell_id = GShellId::new(7);
		let forged_gshell_id = GShellId::new(8);

		present_frame(
			session_gshell_id,
			GNativeFrame {
				gshell_id: forged_gshell_id,
				seq: Seq::new(1),
				commands: vec![RenderCommandDto::Clear],
				cursor: None,
			},
			&dispatcher,
			&wake_pending,
			&snapshot_tx,
			&presenter,
		);

		assert!(snapshot_rx.try_recv().is_err());
		assert!(!wake_pending.load(Ordering::Acquire));
		assert!(dispatcher.events().is_empty());
		assert!(presenter.surface_of(RenderTargetId::new(7)).is_none());
		assert!(presenter.surface_of(RenderTargetId::new(8)).is_none());
	}

	#[test]
	fn app_message_reader_rejects_a_buffer_over_the_limit() {
		let mut buffer = b"123456789\n".to_vec();

		let error =
			take_buffered_app_message(&mut buffer, 8).expect_err("line should exceed limit");

		assert!(matches!(error, GNativeTunnelError::MessageTooLarge { max_bytes: 8 }));
	}
}
