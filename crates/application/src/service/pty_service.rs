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
	pty_host::terminal_size::{TerminalGridSize, TerminalPtySize},
	workspace::pane_id::PaneId,
};
use germinal_infra::{
	pty::{
		PlatformPtyBackend,
		portable_pty_bridge::{PtyBridgeInput, PtyInputSender},
	},
	pty_host::worker::TerminalWorkerInput,
};
use germinal_ports::{
	event::{
		gshell_input::{GShellInput, GShellInputEvent},
		runtime_event_dispatcher::RuntimeEventDispatcher,
		window_input_event::{
			WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
			WindowInputNamedKey,
		},
	},
	rendering::surface_snapshot::RenderSurfaceSnapshot,
	service::{pty_service::IPtyService, worker_service::IWorkerService},
};

#[derive(Debug, Clone)]
struct PtyPaneRuntime {
	pty_input_sender:       PtyInputSender,
	terminal_worker_sender: SyncSender<TerminalWorkerInput>,
}

#[derive(kudi::DepInj)]
#[target(PtyService)]
pub struct PtyServiceState {
	backend:       PlatformPtyBackend,
	pane_runtimes: RefCell<HashMap<PaneId, PtyPaneRuntime>>,
	modifiers:     RefCell<WindowInputModifiers>,
}

impl PtyServiceState {
	pub fn new() -> Self {
		Self {
			backend:       PlatformPtyBackend::new(),
			pane_runtimes: RefCell::new(HashMap::new()),
			modifiers:     RefCell::new(WindowInputModifiers::new(false, false)),
		}
	}
}

impl Default for PtyServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IPtyService for PtyService<Deps>
where Deps:
		AsRef<PtyServiceState> + IWorkerService<TerminalWorkerSender = SyncSender<TerminalWorkerInput>>
{
	fn ensure_pane_pty(
		&self,
		pane_id: PaneId,
		proxy: RuntimeEventDispatcher,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		if state.pane_runtimes.borrow().contains_key(&pane_id) {
			return;
		}

		let Some(terminal_worker_sender) = self.prj_ref().spawn_terminal_worker(
			pane_id,
			term_size,
			proxy.clone(),
			surface_snapshot_tx,
			snapshot_wake_pending,
		) else {
			return;
		};

		let pty_input_sender =
			state.backend.spawn(proxy, pane_id, pty_size, terminal_worker_sender.clone());

		let _ = terminal_worker_sender.send(TerminalWorkerInput::SetPtyInput(pty_input_sender.clone()));

		state
			.pane_runtimes
			.borrow_mut()
			.insert(pane_id, PtyPaneRuntime { pty_input_sender, terminal_worker_sender });
	}

	fn send_pane_pty_input(&self, input: GShellInput) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		match input.event {
			GShellInputEvent::Bytes(bytes) => send_pane_bytes(state, input.pane_id, bytes),
			GShellInputEvent::Paste(text) => {
				send_pane_bytes(state, input.pane_id, text.into_bytes());
			}
			GShellInputEvent::Window(window_input) => match window_input {
				WindowInputEvent::ModifiersChanged(modifiers) => {
					*state.modifiers.borrow_mut() = modifiers;
				}
				WindowInputEvent::Key { state: key_state, logical_key, text } => {
					let modifiers = *state.modifiers.borrow();
					if let Some(bytes) =
						translate_key_event(modifiers, key_state, &logical_key, text.as_deref())
					{
						send_pane_bytes(state, input.pane_id, bytes);
					}
				}
				WindowInputEvent::Ime(text) => {
					if let Some(bytes) = translate_ime_commit(&text) {
						send_pane_bytes(state, input.pane_id, bytes);
					}
				}
				WindowInputEvent::Paste(text) => {
					send_pane_bytes(state, input.pane_id, text.into_bytes());
				}
			},
		}
	}

	fn resize_pane_pty(
		&self,
		pane_id: PaneId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		let Some(runtime) = state.pane_runtimes.borrow().get(&pane_id).cloned() else {
			return;
		};

		let _ = runtime.pty_input_sender.send(PtyBridgeInput::Resize(pty_size));
		let _ = runtime
			.terminal_worker_sender
			.send(TerminalWorkerInput::Resize(to_alacritty_term_size(term_size)));
	}
}

fn to_alacritty_term_size(
	size: TerminalGridSize,
) -> germinal_infra::pty_host::alacritty_state_store::AlacrittyTermSize {
	germinal_infra::pty_host::alacritty_state_store::AlacrittyTermSize::new(
		size.columns(),
		size.rows(),
	)
}

fn send_pane_bytes(state: &PtyServiceState, pane_id: PaneId, bytes: Vec<u8>) {
	let Some(runtime) = state.pane_runtimes.borrow().get(&pane_id).cloned() else {
		return;
	};

	let _ = runtime.pty_input_sender.send(PtyBridgeInput::Bytes(bytes));
}

fn translate_ime_commit(text: &str) -> Option<Vec<u8>> {
	if text.is_empty() {
		return None;
	}

	Some(text.as_bytes().to_vec())
}

fn translate_key_event(
	modifiers: WindowInputModifiers,
	state: WindowInputElementState,
	logical_key: &WindowInputKey,
	text: Option<&str>,
) -> Option<Vec<u8>> {
	if state != WindowInputElementState::Pressed {
		return None;
	}

	if let Some(bytes) = named_key_bytes(logical_key) {
		return Some(bytes);
	}

	if modifiers.control_key() {
		return ctrl_bytes_from_key(logical_key);
	}

	if modifiers.alt_key() {
		if let Some(bytes) = text_bytes(logical_key, text) {
			let mut escaped = Vec::with_capacity(bytes.len() + 1);
			escaped.push(0x1B);
			escaped.extend(bytes);
			return Some(escaped);
		}

		return None;
	}

	text_bytes(logical_key, text)
}

fn named_key_bytes(key: &WindowInputKey) -> Option<Vec<u8>> {
	match key {
		WindowInputKey::Named(WindowInputNamedKey::Enter) => Some(b"\r".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::Tab) => Some(b"\t".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::Backspace) => Some(vec![0x7F]),
		WindowInputKey::Named(WindowInputNamedKey::Escape) => Some(vec![0x1B]),
		WindowInputKey::Named(WindowInputNamedKey::ArrowUp) => Some(b"\x1b[A".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::ArrowDown) => Some(b"\x1b[B".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::ArrowRight) => Some(b"\x1b[C".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::ArrowLeft) => Some(b"\x1b[D".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::Home) => Some(b"\x1b[H".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::End) => Some(b"\x1b[F".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::Delete) => Some(b"\x1b[3~".to_vec()),
		_ => None,
	}
}

fn text_bytes(key: &WindowInputKey, text: Option<&str>) -> Option<Vec<u8>> {
	if let Some(text) = text {
		if !text.is_empty() {
			return Some(text.as_bytes().to_vec());
		}
	}

	match key {
		WindowInputKey::Character(text) if !text.is_empty() => Some(text.as_bytes().to_vec()),
		_ => None,
	}
}

fn ctrl_bytes_from_key(key: &WindowInputKey) -> Option<Vec<u8>> {
	let WindowInputKey::Character(text) = key else {
		return None;
	};

	let mut chars = text.chars();
	let c = chars.next()?.to_ascii_lowercase();

	if chars.next().is_some() {
		return None;
	}

	let byte = match c {
		'a' => 0x01,
		'b' => 0x02,
		'c' => 0x03,
		'd' => 0x04,
		'e' => 0x05,
		'f' => 0x06,
		'h' => 0x08,
		'i' => 0x09,
		'j' => 0x0A,
		'k' => 0x0B,
		'l' => 0x0C,
		'm' => 0x0D,
		'n' => 0x0E,
		'o' => 0x0F,
		'p' => 0x10,
		'q' => 0x11,
		'r' => 0x12,
		's' => 0x13,
		't' => 0x14,
		'u' => 0x15,
		'v' => 0x16,
		'w' => 0x17,
		'x' => 0x18,
		'y' => 0x19,
		'z' => 0x1A,
		'[' => 0x1B,
		'\\' => 0x1C,
		']' => 0x1D,
		'^' => 0x1E,
		'_' => 0x1F,
		_ => return None,
	};

	Some(vec![byte])
}
