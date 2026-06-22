use std::sync::mpsc::SyncSender;

use germinal_domain::{pty_host::terminal_size::TerminalPtySize, workspace::pane_id::PaneId};
use germinal_ports::{
	event::runtime_event_dispatcher::RuntimeEventDispatcher,
	pty_host::{
		pty_backend::IPtyBackend, pty_input::PtyInputSender, worker_input::TerminalWorkerInput,
	},
};

use crate::pty::portable_pty_bridge::PtyBridge;

#[derive(Debug, Clone, Copy, Default)]
pub struct PlatformPtyBackend;

impl PlatformPtyBackend {
	pub fn new() -> Self { Self }
}

impl IPtyBackend for PlatformPtyBackend {
	fn spawn_pty(
		&self,
		proxy: RuntimeEventDispatcher,
		pane_id: PaneId,
		initial_size: TerminalPtySize,
		terminal_worker_sender: SyncSender<TerminalWorkerInput>,
	) -> PtyInputSender {
		PtyBridge::spawn(proxy, pane_id, initial_size, terminal_worker_sender)
	}
}
