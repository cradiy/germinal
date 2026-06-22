use std::{
	io::{Read, Write},
	sync::mpsc::SyncSender,
	thread,
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::pty_host_id::PtyHostId};
use germinal_ports::{
	event::{
		runtime_event::{GShellRuntimeEvent, RuntimeEvent},
		runtime_event_dispatcher::RuntimeEventDispatcher,
	},
	pty_host::{
		pty_input::{PtyInput, PtyInputReceiver},
		terminal_size::TerminalPtySize,
		worker_input::TerminalWorkerInput,
	},
};
use portable_pty::{CommandBuilder, MasterPty, SlavePty, native_pty_system};

use crate::pty::portable_pty_bridge::{PtyBridgeConfig, to_portable_pty_size};

pub(crate) fn spawn_blocking_bridge_thread(
	proxy: RuntimeEventDispatcher,
	gshell_id: GShellId,
	_pty_host_id: PtyHostId,
	config: PtyBridgeConfig,
	terminal_worker_tx: SyncSender<TerminalWorkerInput>,
	input_rx: PtyInputReceiver,
) {
	thread::spawn(move || {
		let pty_system = native_pty_system();

		let pair =
			pty_system.openpty(to_portable_pty_size(config.initial_size)).expect("failed to open pty");

		let mut command = CommandBuilder::new(&config.shell.program);
		for arg in &config.shell.args {
			command.arg(arg);
		}

		let mut child =
			pair.slave.spawn_command(command).expect("failed to spawn interactive shell in pty");

		drop(pair.slave);

		let master = pair.master;
		let mut reader = master.try_clone_reader().expect("failed to clone pty reader");
		let mut writer = master.take_writer().expect("failed to take pty writer");

		let _input_thread = thread::spawn(move || {
			while let Some(input) = input_rx.recv_blocking() {
				match input {
					PtyInput::Bytes(bytes) => {
						if writer.write_all(&bytes).is_err() {
							break;
						}

						let _ = writer.flush();
					}
					PtyInput::Resize(size) => {
						let _ = master.resize(to_portable_pty_size(size));
					}
				}
			}
		});

		read_pty_to_terminal_worker(&mut reader, &terminal_worker_tx);

		let _ = child.wait();
		let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed { gshell_id }));
	});
}

fn read_pty_to_terminal_worker<R>(
	reader: &mut R,
	terminal_worker_tx: &SyncSender<TerminalWorkerInput>,
) where
	R: Read + ?Sized,
{
	let mut buffer = [0u8; 4096];

	loop {
		match reader.read(&mut buffer) {
			Ok(0) => break,
			Ok(n) => {
				let bytes = buffer[..n].to_vec();

				if terminal_worker_tx.send(TerminalWorkerInput::Bytes(bytes)).is_err() {
					break;
				}
			}
			Err(error) => {
				eprintln!("pty read error: {error}");
				break;
			}
		}
	}
}
