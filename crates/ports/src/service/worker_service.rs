use std::sync::{Arc, atomic::AtomicBool, mpsc::Sender};

use germinal_domain::{pty_host::terminal_size::TerminalGridSize, workspace::pane_id::PaneId};

use crate::{
	event::runtime_event_dispatcher::RuntimeEventDispatcher,
	rendering::surface_snapshot::RenderSurfaceSnapshot,
};

pub trait IWorkerService {
	type TerminalWorkerSender;

	fn start_worker_pool(&self);
	fn spawn_terminal_worker(
		&self,
		pane_id: PaneId,
		initial_size: TerminalGridSize,
		proxy: RuntimeEventDispatcher,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> Option<Self::TerminalWorkerSender>;
}
