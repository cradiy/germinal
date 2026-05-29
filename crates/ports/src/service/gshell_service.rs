use std::sync::{Arc, atomic::AtomicBool, mpsc::Sender};

use germinal_domain::{
	pty_host::terminal_size::{TerminalGridSize, TerminalPtySize},
	workspace::pane_id::PaneId,
};

use crate::{
	event::{gshell_input::GShellInput, runtime_event_dispatcher::RuntimeEventDispatcher},
	rendering::surface_snapshot::RenderSurfaceSnapshot,
};

pub trait IGShellService {
	fn ensure_pane_gshell(
		&self,
		pane_id: PaneId,
		proxy: RuntimeEventDispatcher,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	);
	fn route_input_to_gshell(&self, input: GShellInput);
	fn resize_pane_gshell(
		&self,
		pane_id: PaneId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	);
}
