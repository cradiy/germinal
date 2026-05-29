use std::sync::{
	Arc,
	atomic::AtomicBool,
	mpsc::{Sender, SyncSender},
};

use germinal_domain::{pty_host::terminal_size::TerminalGridSize, workspace::pane_id::PaneId};
use germinal_infra::pty_host::{
	alacritty_state_store::AlacrittyTermSize,
	worker::{TerminalWorker, TerminalWorkerInput},
};
use germinal_ports::{
	event::runtime_event_dispatcher::RuntimeEventDispatcher,
	rendering::surface_snapshot::RenderSurfaceSnapshot, service::worker_service::IWorkerService,
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
where Deps: AsRef<WorkerServiceState>
{
	type TerminalWorkerSender = SyncSender<TerminalWorkerInput>;

	fn start_worker_pool(&self) {
		// Current PTY path spawns one terminal worker per pane.
	}

	fn spawn_terminal_worker(
		&self,
		pane_id: PaneId,
		initial_size: TerminalGridSize,
		proxy: RuntimeEventDispatcher,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> Option<SyncSender<TerminalWorkerInput>> {
		Some(TerminalWorker::spawn(
			proxy,
			pane_id,
			AlacrittyTermSize::new(initial_size.columns(), initial_size.rows()),
			surface_snapshot_tx,
			snapshot_wake_pending,
		))
	}
}
