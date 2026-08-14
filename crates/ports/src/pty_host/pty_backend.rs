use std::sync::mpsc::SyncSender;

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::pty_host_id::PtyHostId};

use crate::{
    event::runtime_event_dispatcher::IRuntimeEventDispatcher,
    pty_host::{
        pty_input::PtyInputSender, terminal_size::TerminalPtySize,
        worker_input::TerminalWorkerInput,
    },
};

pub trait IPtyBackend {
    fn spawn_pty<Dispatch>(
        &self,
        proxy: Dispatch,
        gshell_id: GShellId,
        pty_host_id: PtyHostId,
        initial_size: TerminalPtySize,
        shell_env: Vec<(String, String)>,
        terminal_worker_sender: SyncSender<TerminalWorkerInput>,
    ) -> PtyInputSender
    where
        Dispatch: IRuntimeEventDispatcher;
}

pub trait IPtyBackendProvider {
    type PtyBackend: IPtyBackend;

    fn pty_backend(&self) -> &Self::PtyBackend;
}
