use std::sync::{
	Arc,
	atomic::AtomicBool,
	mpsc::{Sender, SyncSender},
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};

use crate::{
	event::runtime_event_dispatcher::IRuntimeEventDispatcher,
	pty_host::worker_input::TerminalWorkerInput, rendering::surface_snapshot::RenderSurfaceSnapshot,
};

pub trait ITerminalWorkerBackend {
	fn spawn_terminal_worker<Dispatch>(
		&self,
		gshell_id: GShellId,
		initial_size: TerminalGridSize,
		proxy: Dispatch,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> SyncSender<TerminalWorkerInput>
	where
		Dispatch: IRuntimeEventDispatcher;
}

pub trait ITerminalWorkerBackendProvider {
	type TerminalWorkerBackend: ITerminalWorkerBackend;

	fn terminal_worker_backend(&self) -> &Self::TerminalWorkerBackend;
}
