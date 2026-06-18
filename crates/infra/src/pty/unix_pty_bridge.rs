use std::{
	os::fd::{FromRawFd, OwnedFd},
	sync::mpsc::SyncSender,
	thread,
};

use compio::{
	BufResult,
	io::{AsyncWrite, AsyncWriteExt},
	runtime::{ResumeUnwind, fd::AsyncFd, spawn},
};
use germinal_domain::workspace::pane_id::PaneId;
use germinal_ports::event::{
	runtime_event::{PaneRuntimeEvent, RuntimeEvent},
	runtime_event_dispatcher::RuntimeEventDispatcher,
};
use nix::unistd::dup;
use portable_pty::{CommandBuilder, native_pty_system};

use crate::{
	pty::portable_pty_bridge::{
		PtyBridgeConfig, PtyBridgeInput, PtyInputReceiver, PtyInputSender, to_portable_pty_size,
	},
	pty_host::worker::TerminalWorkerInput,
};

pub(crate) fn spawn_compio_bridge_thread(
	proxy: RuntimeEventDispatcher,
	pane_id: PaneId,
	config: PtyBridgeConfig,
	terminal_worker_tx: SyncSender<TerminalWorkerInput>,
	shutdown_tx: PtyInputSender,
	input_rx: PtyInputReceiver,
) {
	thread::spawn(move || {
		let runtime = compio::runtime::Runtime::new().expect("failed to create compio runtime");
		runtime.block_on(run_compio_bridge(
			proxy,
			pane_id,
			config,
			terminal_worker_tx,
			shutdown_tx,
			input_rx,
		));
	});
}

async fn run_compio_bridge(
	proxy: RuntimeEventDispatcher,
	pane_id: PaneId,
	config: PtyBridgeConfig,
	terminal_worker_tx: SyncSender<TerminalWorkerInput>,
	shutdown_tx: PtyInputSender,
	input_rx: PtyInputReceiver,
) {
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
	let master_fd = master.as_raw_fd().expect("unix pty master must expose a raw fd");
	let reader_fd = dup(master_fd).expect("failed to dup pty reader fd");
	let writer_fd = dup(master_fd).expect("failed to dup pty writer fd");

	let mut reader = AsyncFd::<OwnedFd>::new(unsafe { OwnedFd::from_raw_fd(reader_fd) })
		.expect("failed to attach pty reader to compio");
	let mut writer = AsyncFd::<OwnedFd>::new(unsafe { OwnedFd::from_raw_fd(writer_fd) })
		.expect("failed to attach pty writer to compio");

	let read_task = spawn(async move {
		read_pty_to_terminal_worker_async(&mut reader, &terminal_worker_tx).await;
		shutdown_tx.close();
	});

	let input_task = spawn(async move {
		while let Some(input) = input_rx.recv().await {
			match input {
				PtyBridgeInput::Bytes(bytes) => {
					let BufResult(result, _) = writer.write_all(bytes).await;
					if result.is_err() {
						break;
					}

					if writer.flush().await.is_err() {
						break;
					}
				}
				PtyBridgeInput::Resize(size) => {
					if master.resize(to_portable_pty_size(size)).is_err() {
						break;
					}
				}
			}
		}

		if let Some(eof_bytes) = pty_eof_bytes(master_fd) {
			let BufResult(..) = writer.write_all(eof_bytes).await;
			let _ = writer.flush().await;
		}
	});

	let _ = read_task.await.resume_unwind();
	let _ = input_task.await.resume_unwind();

	let _ = child.wait();
	let _ = proxy.dispatch(RuntimeEvent::Pane(PaneRuntimeEvent::Closed { pane_id }));
}

async fn read_pty_to_terminal_worker_async(
	reader: &mut AsyncFd<OwnedFd>,
	terminal_worker_tx: &SyncSender<TerminalWorkerInput>,
) {
	use compio::{BufResult, io::AsyncRead};

	loop {
		let BufResult(result, mut buffer) = reader.read(Vec::with_capacity(4096)).await;
		match result {
			Ok(0) => break,
			Ok(n) => {
				buffer.truncate(n);
				if terminal_worker_tx.send(TerminalWorkerInput::Bytes(buffer)).is_err() {
					break;
				}
			}
			Err(error) => {
				if is_pty_hangup_read_error(&error) {
					break;
				}

				eprintln!("pty read error: {error}");
				break;
			}
		}
	}
}

fn is_pty_hangup_read_error(error: &std::io::Error) -> bool {
	error.raw_os_error() == Some(libc::EIO)
}

fn pty_eof_bytes(raw_fd: i32) -> Option<Vec<u8>> {
	let mut termios = std::mem::MaybeUninit::<libc::termios>::zeroed();
	if unsafe { libc::tcgetattr(raw_fd, termios.as_mut_ptr()) } != 0 {
		return None;
	}

	let termios = unsafe { termios.assume_init() };
	let eof = termios.c_cc[libc::VEOF];
	if eof == 0 {
		return None;
	}

	Some(vec![b'\n', eof])
}
