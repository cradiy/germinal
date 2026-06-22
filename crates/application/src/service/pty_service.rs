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
use germinal_ports::{
	event::{
		gshell_input::GShellInputEvent,
		runtime_event_dispatcher::RuntimeEventDispatcher,
		window_input_event::{
			WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
			WindowInputNamedKey,
		},
	},
	pty_host::{
		pty_backend::{IPtyBackend, IPtyBackendProvider},
		pty_input::{PtyInput, PtyInputSender},
		terminal_size::TerminalPtySize,
		worker_input::TerminalWorkerInput,
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
	pty_host_runtimes: RefCell<HashMap<PtyHostId, PtyPaneRuntime>>,
	modifiers:         RefCell<WindowInputModifiers>,
}

impl PtyServiceState {
	pub fn new() -> Self {
		Self {
			pty_host_runtimes: RefCell::new(HashMap::new()),
			modifiers:         RefCell::new(WindowInputModifiers::new(false, false)),
		}
	}
}

impl Default for PtyServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IPtyService for PtyService<Deps>
where Deps: AsRef<PtyServiceState>
		+ IPtyBackendProvider
		+ IWorkerService<TerminalWorkerSender = SyncSender<TerminalWorkerInput>>
{
	fn ensure_gshell_pty(
		&self,
		gshell_id: GShellId,
		pty_host_id: PtyHostId,
		proxy: RuntimeEventDispatcher,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		if state.pty_host_runtimes.borrow().contains_key(&pty_host_id) {
			return;
		}

		let Some(terminal_worker_sender) = self.prj_ref().spawn_terminal_worker(
			gshell_id,
			term_size,
			proxy.clone(),
			surface_snapshot_tx,
			snapshot_wake_pending,
		) else {
			return;
		};

		let pty_input_sender = self.prj_ref().pty_backend().spawn_pty(
			proxy,
			gshell_id,
			pty_host_id,
			pty_size,
			terminal_worker_sender.clone(),
		);

		let _ = terminal_worker_sender.send(TerminalWorkerInput::SetPtyInput(pty_input_sender.clone()));

		state
			.pty_host_runtimes
			.borrow_mut()
			.insert(pty_host_id, PtyPaneRuntime { pty_input_sender, terminal_worker_sender });
	}

	fn send_pty_host_input(&self, pty_host_id: PtyHostId, event: GShellInputEvent) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		match event {
			GShellInputEvent::Bytes(bytes) => send_pty_host_bytes(state, pty_host_id, bytes),
			GShellInputEvent::Paste(text) => {
				send_pty_host_bytes(state, pty_host_id, text.into_bytes());
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
						send_pty_host_bytes(state, pty_host_id, bytes);
					}
				}
				WindowInputEvent::Ime(text) => {
					if let Some(bytes) = translate_ime_commit(&text) {
						send_pty_host_bytes(state, pty_host_id, bytes);
					}
				}
				WindowInputEvent::Paste(text) => {
					send_pty_host_bytes(state, pty_host_id, text.into_bytes());
				}
			},
		}
	}

	fn resize_pty_host(
		&self,
		pty_host_id: PtyHostId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		let state: &PtyServiceState = self.prj_ref().as_ref();
		let Some(runtime) = state.pty_host_runtimes.borrow().get(&pty_host_id).cloned() else {
			return;
		};

		let _ = runtime.pty_input_sender.send(PtyInput::Resize(pty_size));
		let _ = runtime.terminal_worker_sender.send(TerminalWorkerInput::Resize(term_size));
	}
}

fn send_pty_host_bytes(state: &PtyServiceState, pty_host_id: PtyHostId, bytes: Vec<u8>) {
	let Some(runtime) = state.pty_host_runtimes.borrow().get(&pty_host_id).cloned() else {
		return;
	};

	let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
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
	if let Some(text) = text
		&& !text.is_empty()
	{
		return Some(text.as_bytes().to_vec());
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
