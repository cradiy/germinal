use std::sync::{
	Arc,
	atomic::AtomicBool,
	mpsc::{Sender, SyncSender},
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};

use crate::{
	event::runtime_event_dispatcher::RuntimeEventDispatcher,
	pty_host::worker_input::TerminalWorkerInput, rendering::surface_snapshot::RenderSurfaceSnapshot,
};

pub trait ITerminalWorkerBackend {
	fn spawn_terminal_worker(
		&self,
		gshell_id: GShellId,
		initial_size: TerminalGridSize,
		proxy: RuntimeEventDispatcher,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> SyncSender<TerminalWorkerInput>;
}

pub trait ITerminalWorkerBackendProvider {
	type TerminalWorkerBackend: ITerminalWorkerBackend;

	fn terminal_worker_backend(&self) -> &Self::TerminalWorkerBackend;
}
