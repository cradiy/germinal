use std::sync::{Arc, atomic::AtomicBool, mpsc::Sender};

use germinal_domain::gshell::vo::gshell_id::GShellId;

use crate::{
    pty_host::terminal_size::TerminalPtySize, rendering::surface_snapshot::RenderSurfaceSnapshot,
};

pub trait IWorkerService {
    type TerminalWorkerSender;

    fn start_worker_pool(&self);
    fn spawn_terminal_worker(
        &self,
        gshell_id: GShellId,
        initial_size: TerminalPtySize,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) -> Option<Self::TerminalWorkerSender>;
}
