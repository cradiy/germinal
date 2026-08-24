use std::{
    os::fd::{BorrowedFd, OwnedFd},
    sync::mpsc::SyncSender,
    thread,
};

use compio::{
    BufResult,
    io::{AsyncWrite, AsyncWriteExt},
    runtime::{ResumeUnwind, fd::AsyncFd, spawn},
};
use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::pty_host_id::PtyHostId};
use germinal_ports::{
    event::{
        runtime_event::{GShellRuntimeEvent, RuntimeEvent},
        runtime_event_dispatcher::IRuntimeEventDispatcher,
    },
    pty_host::{
        pty_input::{PtyInput, PtyInputReceiver, PtyInputSender},
        worker_input::TerminalWorkerInput,
    },
};
use nix::{
    errno::Errno,
    sys::termios::{SpecialCharacterIndices, tcgetattr},
    unistd::dup,
};
use portable_pty::{CommandBuilder, native_pty_system};
use tracing::{error, warn};

use crate::pty::portable_pty_bridge::{
    PtyBridgeConfig, apply_default_terminal_env, apply_shell_env, to_portable_pty_size,
};

pub(crate) fn spawn_compio_bridge_thread<Dispatch>(
    proxy: Dispatch,
    gshell_id: GShellId,
    _pty_host_id: PtyHostId,
    config: PtyBridgeConfig,
    terminal_worker_tx: SyncSender<TerminalWorkerInput>,
    shutdown_tx: PtyInputSender,
    input_rx: PtyInputReceiver,
) where
    Dispatch: IRuntimeEventDispatcher,
{
    thread::spawn(move || {
        let Ok(runtime) = compio::runtime::Runtime::new() else {
            error!(
                gshell_id = gshell_id.value(),
                "failed to create compio runtime for pty bridge"
            );
            let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
                gshell_id,
            }));
            return;
        };
        runtime.block_on(run_compio_bridge(
            proxy,
            gshell_id,
            config,
            terminal_worker_tx,
            shutdown_tx,
            input_rx,
        ));
    });
}

async fn run_compio_bridge<Dispatch>(
    proxy: Dispatch,
    gshell_id: GShellId,
    config: PtyBridgeConfig,
    terminal_worker_tx: SyncSender<TerminalWorkerInput>,
    shutdown_tx: PtyInputSender,
    input_rx: PtyInputReceiver,
) where
    Dispatch: IRuntimeEventDispatcher,
{
    let pty_system = native_pty_system();
    let Ok(pair) = pty_system.openpty(to_portable_pty_size(config.initial_size)) else {
        error!(gshell_id = gshell_id.value(), "failed to open pty");
        let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
            gshell_id,
        }));
        return;
    };

    let mut command = CommandBuilder::new(&config.shell.program);
    for arg in &config.shell.args {
        command.arg(arg);
    }
    if let Some(working_directory) = &config.working_directory {
        command.cwd(working_directory);
    }
    apply_default_terminal_env(&mut command);
    apply_shell_env(&mut command, &config.shell_env);

    let Ok(mut child) = pair.slave.spawn_command(command) else {
        error!(
            gshell_id = gshell_id.value(),
            "failed to spawn interactive shell in pty"
        );
        let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
            gshell_id,
        }));
        return;
    };
    if let Some(process_id) = child.process_id() {
        input_rx.set_child_process_id(process_id);
    }
    let mut child_killer = child.clone_killer();
    drop(pair.slave);

    let master = pair.master;
    let Some(master_fd) = master.as_raw_fd() else {
        error!(
            gshell_id = gshell_id.value(),
            "unix pty master does not expose a raw fd"
        );
        let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
            gshell_id,
        }));
        return;
    };
    let Ok(reader_fd) = dup(unsafe { BorrowedFd::borrow_raw(master_fd) }) else {
        error!(gshell_id = gshell_id.value(), "failed to dup pty reader fd");
        let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
            gshell_id,
        }));
        return;
    };
    let Ok(writer_fd) = dup(unsafe { BorrowedFd::borrow_raw(master_fd) }) else {
        error!(gshell_id = gshell_id.value(), "failed to dup pty writer fd");
        let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
            gshell_id,
        }));
        return;
    };

    let Ok(mut reader) = AsyncFd::<OwnedFd>::new(reader_fd) else {
        error!(
            gshell_id = gshell_id.value(),
            "failed to attach pty reader to compio"
        );
        let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
            gshell_id,
        }));
        return;
    };
    let Ok(mut writer) = AsyncFd::<OwnedFd>::new(writer_fd) else {
        error!(
            gshell_id = gshell_id.value(),
            "failed to attach pty writer to compio"
        );
        let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
            gshell_id,
        }));
        return;
    };

    let read_task = spawn(async move {
        read_pty_to_terminal_worker_async(&mut reader, &terminal_worker_tx).await;
        shutdown_tx.close();
    });

    let input_task = spawn(async move {
        while let Some(input) = input_rx.recv().await {
            match input {
                PtyInput::Bytes(bytes) => {
                    let BufResult(result, _) = writer.write_all(bytes).await;
                    if result.is_err() {
                        break;
                    }

                    if writer.flush().await.is_err() {
                        break;
                    }
                }
                PtyInput::Resize(size) => {
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
        let _ = child_killer.kill();
    });

    let _ = read_task.await.resume_unwind();
    let _ = input_task.await.resume_unwind();

    let _ = child.wait();
    let _ = proxy.dispatch(RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
        gshell_id,
    }));
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
                if terminal_worker_tx
                    .send(TerminalWorkerInput::Bytes(buffer))
                    .is_err()
                {
                    break;
                }
            }
            Err(error) => {
                if is_pty_hangup_read_error(&error) {
                    break;
                }

                warn!(error = %error, "pty read error");
                break;
            }
        }
    }
}

fn is_pty_hangup_read_error(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(Errno::EIO as i32)
}

fn pty_eof_bytes(raw_fd: i32) -> Option<Vec<u8>> {
    let fd = unsafe { BorrowedFd::borrow_raw(raw_fd) };

    let termios = tcgetattr(fd).ok()?;
    let eof = termios.control_chars[SpecialCharacterIndices::VEOF as usize];

    if eof == 0 {
        return None;
    }

    Some(vec![b'\n', eof])
}
