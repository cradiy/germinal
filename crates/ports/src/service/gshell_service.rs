use std::sync::{Arc, atomic::AtomicBool, mpsc::Sender};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};

use crate::{
    event::gshell_input::GShellInput,
    pty_host::{size_info::TerminalSizeInfo, terminal_size::TerminalPtySize},
    rendering::surface_snapshot::RenderSurfaceSnapshot,
};

pub trait IGShellService {
    fn ensure_gshell(
        &self,
        gshell_id: GShellId,
        pty_size: TerminalPtySize,
        term_size: TerminalGridSize,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    );
    fn begin_gnative_mode(&self, gshell_id: GShellId);
    fn enter_gnative_mode(&self, gshell_id: GShellId);
    fn exit_gnative_mode(&self, gshell_id: GShellId);
    fn remove_gshell(&self, gshell_id: GShellId);
    fn route_input_to_gshell(&self, input: GShellInput);
    fn resize_gshell(&self, gshell_id: GShellId, size_info: TerminalSizeInfo);
}
