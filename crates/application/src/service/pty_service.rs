use std::{
	cell::RefCell,
	collections::HashMap,
	sync::{
		Arc,
		atomic::AtomicBool,
		mpsc::{Sender, SyncSender},
	},
};

use germinal_domain::{
	gshell::vo::gshell_id::GShellId,
	pty_host::{pty_host_id::PtyHostId, terminal_size::TerminalGridSize},
};
use germinal_gnative_protocol::gnative::session::GNATIVE_PROTOCOL_VERSION;
use germinal_ports::{
	event::{
		gshell_input::GShellInputEvent,
		runtime_event_dispatcher::IRuntimeEventDispatcherProvider,
		window_input_event::{
			WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
			WindowPointerButton, WindowPointerPosition, WindowScrollDelta,
		},
	},
	pty_host::{
		pty_backend::{IPtyBackend, IPtyBackendProvider},
		pty_input::{PtyInput, PtyInputSender},
		terminal_input_mode::TerminalInputModeState,
		terminal_size::TerminalPtySize,
		worker_input::TerminalWorkerInput,
	},
	rendering::surface_snapshot::RenderSurfaceSnapshot,
	service::{
		gnative_tunnel::{IGNativeTunnel, IGNativeTunnelProvider},
		pty_service::IPtyService,
		worker_service::IWorkerService,
	},
};
use tracing::warn;

use super::pty_input_encoder::{
	PtyMouseEncoder, encode_focus_changed, encode_ime_commit, encode_key_event, encode_paste,
};

struct PtyPaneRuntime {
	pty_input_sender:       PtyInputSender,
	terminal_worker_sender: SyncSender<TerminalWorkerInput>,
	input_modes:            TerminalInputModeState,
	mouse:                  PtyMouseEncoder,
}

#[derive(kudi::DepInj)]
#[target(PtyService)]
pub struct PtyServiceState {
	pty_host_runtimes: RefCell<HashMap<PtyHostId, PtyPaneRuntime>>,
	modifiers:         RefCell<WindowInputModifiers>,
}

impl PtyServiceState {
	pub fn new() -> Self {
		Self {
			pty_host_runtimes: RefCell::new(HashMap::new()),
			modifiers:         RefCell::new(WindowInputModifiers::new(false, false, false, false)),
		}
	}
}

impl Default for PtyServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IPtyService for PtyService<Deps>
where Deps: AsRef<PtyServiceState>
		+ IRuntimeEventDispatcherProvider
		+ IGNativeTunnelProvider
		+ IPtyBackendProvider
		+ IWorkerService<TerminalWorkerSender = SyncSender<TerminalWorkerInput>>
{
	fn ensure_gshell_pty(
		&self,
		gshell_id: GShellId,
		pty_host_id: PtyHostId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		if state.pty_host_runtimes.borrow().contains_key(&pty_host_id) {
			return;
		}

		let proxy = self.prj_ref().runtime_event_dispatcher().clone();
		let Some(terminal_worker_sender) = self.prj_ref().spawn_terminal_worker(
			gshell_id,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		) else {
			return;
		};
		let shell_env = match self
			.prj_ref()
			.gnative_tunnel()
			.ensure_session_descriptor(gshell_id, GNATIVE_PROTOCOL_VERSION)
		{
			Ok(descriptor) => descriptor.tunnel_env(),
			Err(error) => {
				warn!(gshell_id = gshell_id.value(), error = %error, "failed to prepare gnative tunnel");
				return;
			}
		};

		let pty_input_sender = self.prj_ref().pty_backend().spawn_pty(
			proxy,
			gshell_id,
			pty_host_id,
			pty_size,
			shell_env,
			terminal_worker_sender.clone(),
		);

		let input_modes = TerminalInputModeState::default();
		let _ = terminal_worker_sender.send(TerminalWorkerInput::SetPtyInput {
			sender: pty_input_sender.clone(),
			input_modes: input_modes.clone(),
		});

		state.pty_host_runtimes.borrow_mut().insert(
			pty_host_id,
			PtyPaneRuntime {
				pty_input_sender,
				terminal_worker_sender,
				input_modes,
				mouse: PtyMouseEncoder::new(pty_size),
			},
		);
	}

	fn send_pty_host_input(&self, pty_host_id: PtyHostId, event: GShellInputEvent) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		match event {
			GShellInputEvent::Bytes(bytes) => send_pty_host_bytes(state, pty_host_id, bytes),
			GShellInputEvent::Paste(text) => send_pty_host_paste(state, pty_host_id, &text),
			GShellInputEvent::Window(window_input) => match window_input {
				WindowInputEvent::ModifiersChanged(modifiers) => {
					*state.modifiers.borrow_mut() = modifiers;
				}
				WindowInputEvent::FocusChanged(focused) => {
					send_pty_host_focus(state, pty_host_id, focused);
				}
				WindowInputEvent::Key { state: key_state, logical_key, text } => {
					let modifiers = *state.modifiers.borrow();
					send_pty_host_key(
						state,
						pty_host_id,
						modifiers,
						key_state,
						&logical_key,
						text.as_deref(),
					);
				}
				WindowInputEvent::Ime(text) => {
					if let Some(bytes) = encode_ime_commit(&text) {
						send_pty_host_bytes(state, pty_host_id, bytes);
					}
				}
				WindowInputEvent::Paste(text) => send_pty_host_paste(state, pty_host_id, &text),
				WindowInputEvent::PointerMoved { position, modifiers } => {
					send_pty_host_pointer_moved(state, pty_host_id, position, modifiers);
				}
				WindowInputEvent::PointerLeft => {
					if let Some(runtime) = state.pty_host_runtimes.borrow_mut().get_mut(&pty_host_id) {
						runtime.mouse.pointer_left();
					}
				}
				WindowInputEvent::PointerButton {
					state: button_state,
					button,
					position,
					modifiers,
				} => {
					send_pty_host_pointer_button(
						state,
						pty_host_id,
						button_state,
						button,
						position,
						modifiers,
					);
				}
				WindowInputEvent::Scroll { delta, position, modifiers } => {
					send_pty_host_scroll(state, pty_host_id, delta, position, modifiers);
				}
			},
		}
	}

	fn remove_pty_host(&self, pty_host_id: PtyHostId) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		state.pty_host_runtimes.borrow_mut().remove(&pty_host_id);
	}

	fn resize_pty_host(
		&self,
		pty_host_id: PtyHostId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		let mut runtimes = state.pty_host_runtimes.borrow_mut();
		let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
			return;
		};

		runtime.mouse.resize(pty_size);
		let _ = runtime.pty_input_sender.send(PtyInput::Resize(pty_size));
		let _ = runtime.terminal_worker_sender.send(TerminalWorkerInput::Resize(term_size));
	}
}

fn send_pty_host_bytes(state: &PtyServiceState, pty_host_id: PtyHostId, bytes: Vec<u8>) {
	let runtimes = state.pty_host_runtimes.borrow();
	let Some(runtime) = runtimes.get(&pty_host_id) else {
		return;
	};

	let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_paste(state: &PtyServiceState, pty_host_id: PtyHostId, text: &str) {
	let runtimes = state.pty_host_runtimes.borrow();
	let Some(runtime) = runtimes.get(&pty_host_id) else {
		return;
	};
	let Some(bytes) = encode_paste(runtime.input_modes.load(), text) else {
		return;
	};

	let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_focus(state: &PtyServiceState, pty_host_id: PtyHostId, focused: bool) {
	let runtimes = state.pty_host_runtimes.borrow();
	let Some(runtime) = runtimes.get(&pty_host_id) else {
		return;
	};
	let Some(bytes) = encode_focus_changed(runtime.input_modes.load(), focused) else {
		return;
	};

	let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_key(
	state: &PtyServiceState,
	pty_host_id: PtyHostId,
	modifiers: WindowInputModifiers,
	key_state: WindowInputElementState,
	logical_key: &WindowInputKey,
	text: Option<&str>,
) {
	let runtimes = state.pty_host_runtimes.borrow();
	let Some(runtime) = runtimes.get(&pty_host_id) else {
		return;
	};
	let Some(bytes) = encode_key_event(
		runtime.input_modes.load(),
		modifiers,
		key_state,
		logical_key,
		text,
	) else {
		return;
	};

	let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_pointer_moved(
	state: &PtyServiceState,
	pty_host_id: PtyHostId,
	position: WindowPointerPosition,
	modifiers: WindowInputModifiers,
) {
	let mut runtimes = state.pty_host_runtimes.borrow_mut();
	let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
		return;
	};
	let Some(bytes) = runtime.mouse.moved(runtime.input_modes.load(), position, modifiers) else {
		return;
	};

	let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_pointer_button(
	state: &PtyServiceState,
	pty_host_id: PtyHostId,
	button_state: WindowInputElementState,
	button: WindowPointerButton,
	position: WindowPointerPosition,
	modifiers: WindowInputModifiers,
) {
	let mut runtimes = state.pty_host_runtimes.borrow_mut();
	let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
		return;
	};
	let Some(bytes) = runtime.mouse.button(
		runtime.input_modes.load(),
		button_state,
		button,
		position,
		modifiers,
	) else {
		return;
	};

	let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_scroll(
	state: &PtyServiceState,
	pty_host_id: PtyHostId,
	delta: WindowScrollDelta,
	position: WindowPointerPosition,
	modifiers: WindowInputModifiers,
) {
	let mut runtimes = state.pty_host_runtimes.borrow_mut();
	let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
		return;
	};
	let reports = runtime.mouse.scroll(runtime.input_modes.load(), delta, position, modifiers);
	for bytes in reports {
		let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
	}
}
