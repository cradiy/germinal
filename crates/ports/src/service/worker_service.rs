use std::sync::{Arc, atomic::AtomicBool, mpsc::Sender};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};

use crate::rendering::surface_snapshot::RenderSurfaceSnapshot;

pub trait IWorkerService {
    type TerminalWorkerSender;

    fn start_worker_pool(&self);
    fn spawn_terminal_worker(
        &self,
        gshell_id: GShellId,
        initial_size: TerminalGridSize,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) -> Option<Self::TerminalWorkerSender>;
}
