use std::sync::{
	Arc,
	atomic::AtomicBool,
	mpsc::{Sender, SyncSender},
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};
use germinal_ports::{
	event::runtime_event_dispatcher::IRuntimeEventDispatcherProvider,
	pty_host::{
		worker_backend::{ITerminalWorkerBackend, ITerminalWorkerBackendProvider},
		worker_input::TerminalWorkerInput,
	},
	rendering::surface_snapshot::RenderSurfaceSnapshot,
	service::worker_service::IWorkerService,
};

#[derive(kudi::DepInj)]
#[target(WorkerService)]
pub struct WorkerServiceState;

impl WorkerServiceState {
	pub fn new() -> Self { Self }
}

impl Default for WorkerServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IWorkerService for WorkerService<Deps>
where Deps:
		AsRef<WorkerServiceState> + IRuntimeEventDispatcherProvider + ITerminalWorkerBackendProvider
{
	type TerminalWorkerSender = SyncSender<TerminalWorkerInput>;

	fn start_worker_pool(&self) {
		// Current PTY path spawns one terminal worker per pane.
	}

	fn spawn_terminal_worker(
		&self,
		gshell_id: GShellId,
		initial_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> Option<SyncSender<TerminalWorkerInput>> {
		Some(self.prj_ref().terminal_worker_backend().spawn_terminal_worker(
			gshell_id,
			initial_size,
			self.prj_ref().runtime_event_dispatcher().clone(),
			surface_snapshot_tx,
			snapshot_wake_pending,
		))
	}
}
