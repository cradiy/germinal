use std::sync::mpsc::SyncSender;

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::pty_host_id::PtyHostId};
use germinal_ports::{
	event::runtime_event_dispatcher::RuntimeEventDispatcher,
	pty_host::{
		pty_input::{PtyInputSender, pty_input_channel},
		terminal_size::TerminalPtySize,
		worker_input::TerminalWorkerInput,
	},
};
use portable_pty::PtySize;

use crate::pty::shell_command::{ShellCommand, default_shell_command};
#[cfg(unix)]
use crate::pty::unix_pty_bridge::spawn_compio_bridge_thread;
#[cfg(windows)]
use crate::pty::windows_pty_bridge::spawn_blocking_bridge_thread;

#[derive(Debug, Clone)]
pub(crate) struct PtyBridgeConfig {
	pub shell:        ShellCommand,
	pub initial_size: TerminalPtySize,
}

impl PtyBridgeConfig {
	pub fn new(initial_size: TerminalPtySize) -> Self {
		Self { shell: default_shell_command(), initial_size }
	}
}

pub struct PtyBridge;

impl PtyBridge {
	pub fn spawn(
		proxy: RuntimeEventDispatcher,
		gshell_id: GShellId,
		pty_host_id: PtyHostId,
		initial_size: TerminalPtySize,
		terminal_worker_tx: SyncSender<TerminalWorkerInput>,
	) -> PtyInputSender {
		Self::spawn_with_config(
			proxy,
			gshell_id,
			pty_host_id,
			PtyBridgeConfig::new(initial_size),
			terminal_worker_tx,
		)
	}

	pub(crate) fn spawn_with_config(
		proxy: RuntimeEventDispatcher,
		gshell_id: GShellId,
		pty_host_id: PtyHostId,
		config: PtyBridgeConfig,
		terminal_worker_tx: SyncSender<TerminalWorkerInput>,
	) -> PtyInputSender {
		let (input_tx, input_rx) = pty_input_channel();

		#[cfg(unix)]
		spawn_compio_bridge_thread(
			proxy,
			gshell_id,
			pty_host_id,
			config,
			terminal_worker_tx,
			input_tx.clone(),
			input_rx,
		);

		#[cfg(windows)]
		spawn_blocking_bridge_thread(
			proxy,
			gshell_id,
			pty_host_id,
			config,
			terminal_worker_tx,
			input_rx,
		);

		input_tx
	}
}

pub(crate) fn to_portable_pty_size(size: TerminalPtySize) -> PtySize {
	PtySize {
		rows:         size.rows(),
		cols:         size.columns(),
		pixel_width:  size.pixel_width(),
		pixel_height: size.pixel_height(),
	}
}
