use std::sync::{Arc, atomic::AtomicBool, mpsc::SyncSender};

use germinal_domain::gshell::vo::gshell_id::GShellId;

use crate::{
    pty_host::{terminal_size::TerminalPtySize, worker_input::TerminalWorkerInput},
    rendering::surface_snapshot_mailbox::SurfaceSnapshotSender,
};

pub trait ITerminalWorkerBackend {
    fn start_worker_pool(&self);
    fn spawn_terminal_worker(
        &self,
        gshell_id: GShellId,
        initial_size: TerminalPtySize,
        surface_snapshot_tx: SurfaceSnapshotSender,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) -> SyncSender<TerminalWorkerInput>;
}

pub trait ITerminalWorkerBackendProvider {
    type TerminalWorkerBackend: ITerminalWorkerBackend;

    fn terminal_worker_backend(&self) -> &Self::TerminalWorkerBackend;
}
