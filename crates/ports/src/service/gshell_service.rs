use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};

use crate::{
    event::gshell_input::GShellInput,
    pty_host::{size_info::TerminalSizeInfo, spawn_config::PtySpawnConfig},
    rendering::surface_snapshot_mailbox::SurfaceSnapshotSender,
};

pub trait IGShellService {
    fn ensure_gshell(
        &self,
        gshell_id: GShellId,
        spawn_config: PtySpawnConfig,
        term_size: TerminalGridSize,
        surface_snapshot_tx: SurfaceSnapshotSender,
        snapshot_wake_pending: Arc<AtomicBool>,
    );
    fn begin_gnative_mode(&self, gshell_id: GShellId);
    fn enter_gnative_mode(&self, gshell_id: GShellId);
    fn exit_gnative_mode(&self, gshell_id: GShellId);
    fn remove_gshell(&self, gshell_id: GShellId);
    fn gshell_working_directory(&self, gshell_id: GShellId) -> Option<PathBuf>;
    fn report_gshell_working_directory(&self, gshell_id: GShellId, working_directory: PathBuf);
    fn route_input_to_gshell(&self, input: GShellInput);
    fn resize_gshell(&self, gshell_id: GShellId, size_info: TerminalSizeInfo);
}
