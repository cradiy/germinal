use std::sync::{
    Arc,
    atomic::AtomicBool,
    mpsc::{Sender, SyncSender},
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};

use crate::{
    pty_host::worker_input::TerminalWorkerInput, rendering::surface_snapshot::RenderSurfaceSnapshot,
};

pub trait ITerminalWorkerBackend {
    fn start_worker_pool(&self);
    fn spawn_terminal_worker(
        &self,
        gshell_id: GShellId,
        initial_size: TerminalGridSize,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) -> SyncSender<TerminalWorkerInput>;
}

pub trait ITerminalWorkerBackendProvider {
    type TerminalWorkerBackend: ITerminalWorkerBackend;

    fn terminal_worker_backend(&self) -> &Self::TerminalWorkerBackend;
}
