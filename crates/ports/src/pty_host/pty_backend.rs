use std::sync::mpsc::SyncSender;

use germinal_domain::{pty_host::terminal_size::TerminalPtySize, workspace::pane_id::PaneId};

use crate::{
	event::runtime_event_dispatcher::RuntimeEventDispatcher,
	pty_host::{pty_input::PtyInputSender, worker_input::TerminalWorkerInput},
};

pub trait IPtyBackend {
	fn spawn_pty(
		&self,
		proxy: RuntimeEventDispatcher,
		pane_id: PaneId,
		initial_size: TerminalPtySize,
		terminal_worker_sender: SyncSender<TerminalWorkerInput>,
	) -> PtyInputSender;
}

pub trait IPtyBackendProvider {
	type PtyBackend: IPtyBackend;

	fn pty_backend(&self) -> &Self::PtyBackend;
}
