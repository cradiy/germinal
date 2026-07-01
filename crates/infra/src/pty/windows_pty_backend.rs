use std::sync::mpsc::SyncSender;

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::pty_host_id::PtyHostId};
use germinal_ports::{
	event::runtime_event_dispatcher::IRuntimeEventDispatcher,
	pty_host::{
		pty_backend::IPtyBackend, pty_input::PtyInputSender, terminal_size::TerminalPtySize,
		worker_input::TerminalWorkerInput,
	},
};

use crate::pty::portable_pty_bridge::PtyBridge;

#[derive(Debug, Clone, Copy, Default)]
pub struct PlatformPtyBackend;

impl PlatformPtyBackend {
	pub fn new() -> Self { Self }
}

impl IPtyBackend for PlatformPtyBackend {
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
		Dispatch: IRuntimeEventDispatcher,
	{
		PtyBridge::spawn(proxy, gshell_id, pty_host_id, initial_size, shell_env, terminal_worker_sender)
	}
}
