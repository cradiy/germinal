use std::sync::{
    Arc,
    atomic::AtomicBool,
    mpsc::{Sender, SyncSender},
};

use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_ports::{
    pty_host::{
        terminal_size::TerminalPtySize,
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
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkerServiceState {
    fn default() -> Self {
        Self::new()
    }
}

impl<Deps> IWorkerService for WorkerService<Deps>
where
    Deps: AsRef<WorkerServiceState> + ITerminalWorkerBackendProvider,
{
    type TerminalWorkerSender = SyncSender<TerminalWorkerInput>;

    fn start_worker_pool(&self) {
        self.prj_ref().terminal_worker_backend().start_worker_pool();
    }

    fn spawn_terminal_worker(
        &self,
        gshell_id: GShellId,
        initial_size: TerminalPtySize,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) -> Option<SyncSender<TerminalWorkerInput>> {
        self.prj_ref().terminal_worker_backend().start_worker_pool();
        Some(
            self.prj_ref()
                .terminal_worker_backend()
                .spawn_terminal_worker(
                    gshell_id,
                    initial_size,
                    surface_snapshot_tx,
                    snapshot_wake_pending,
                ),
        )
    }
}
