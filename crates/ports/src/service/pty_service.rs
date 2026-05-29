use std::sync::{Arc, atomic::AtomicBool, mpsc::Sender};

use germinal_domain::{
	pty_host::terminal_size::{TerminalGridSize, TerminalPtySize},
	workspace::pane_id::PaneId,
};

use crate::{
	event::{gshell_input::GShellInput, runtime_event_dispatcher::RuntimeEventDispatcher},
	rendering::surface_snapshot::RenderSurfaceSnapshot,
};

pub trait IPtyService {
	fn ensure_pane_pty(
		&self,
		pane_id: PaneId,
		proxy: RuntimeEventDispatcher,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	);
	fn send_pane_pty_input(&self, input: GShellInput);
	fn resize_pane_pty(
		&self,
		pane_id: PaneId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	);
}
