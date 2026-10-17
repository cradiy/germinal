use std::{
    env,
    path::{Path, PathBuf},
    sync::mpsc::SyncSender,
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::pty_host_id::PtyHostId};
use germinal_ports::{
    event::runtime_event_dispatcher::IRuntimeEventDispatcher,
    pty_host::{
        pty_input::{PtyInputSender, pty_input_channel},
        spawn_config::PtySpawnConfig,
        terminal_size::TerminalPtySize,
        worker_input::TerminalWorkerInput,
    },
};
use portable_pty::{CommandBuilder, PtySize};

use crate::pty::shell_command::{ShellCommand, default_shell_command};
#[cfg(unix)]
use crate::pty::unix_pty_bridge::spawn_compio_bridge_thread;
#[cfg(windows)]
use crate::pty::windows_pty_bridge::spawn_compio_bridge_thread;

const ALACRITTY_TERMINFO: &str = "alacritty";
const FALLBACK_TERMINFO: &str = "xterm-256color";
const TRUECOLOR_COLORTERM: &str = "truecolor";

#[derive(Debug, Clone)]
pub(crate) struct PtyBridgeConfig {
    pub shell: ShellCommand,
    pub shell_env: Vec<(String, String)>,
    pub initial_size: TerminalPtySize,
    pub working_directory: Option<PathBuf>,
}

impl PtyBridgeConfig {
    pub fn new(shell_env: Vec<(String, String)>, spawn_config: PtySpawnConfig) -> Self {
        let initial_size = spawn_config.initial_size;
        Self {
            shell: spawn_config
                .shell
                .map(|shell| ShellCommand::new(shell.program, shell.args))
                .unwrap_or_else(default_shell_command),
            shell_env,
            initial_size,
            working_directory: spawn_config.working_directory,
        }
    }
}

pub struct PtyBridge;

impl PtyBridge {
    pub fn spawn<Dispatch>(
        proxy: Dispatch,
        gshell_id: GShellId,
        pty_host_id: PtyHostId,
        spawn_config: PtySpawnConfig,
        shell_env: Vec<(String, String)>,
        terminal_worker_tx: SyncSender<TerminalWorkerInput>,
    ) -> PtyInputSender
    where
        Dispatch: IRuntimeEventDispatcher,
    {
        Self::spawn_with_config(
            proxy,
            gshell_id,
            pty_host_id,
            PtyBridgeConfig::new(shell_env, spawn_config),
            terminal_worker_tx,
        )
    }

    pub(crate) fn spawn_with_config<Dispatch>(
        proxy: Dispatch,
        gshell_id: GShellId,
        pty_host_id: PtyHostId,
        config: PtyBridgeConfig,
        terminal_worker_tx: SyncSender<TerminalWorkerInput>,
    ) -> PtyInputSender
    where
        Dispatch: IRuntimeEventDispatcher,
    {
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
        spawn_compio_bridge_thread(
            proxy,
            gshell_id,
            pty_host_id,
            config,
            terminal_worker_tx,
            input_tx.clone(),
            input_rx,
        );

        input_tx
    }
}

pub(crate) fn apply_default_terminal_env(command: &mut CommandBuilder) {
    command.env("TERM", preferred_terminal_term_name());
    command.env("COLORTERM", TRUECOLOR_COLORTERM);
}

pub(crate) fn apply_shell_env(command: &mut CommandBuilder, shell_env: &[(String, String)]) {
    for (key, value) in shell_env {
        command.env(key, value);
    }
}

pub(crate) fn to_portable_pty_size(size: TerminalPtySize) -> PtySize {
    PtySize {
        rows: size.rows(),
        cols: size.columns(),
        pixel_width: size.pixel_width(),
        pixel_height: size.pixel_height(),
    }
}

pub(crate) fn preferred_terminal_term_name() -> &'static str {
    preferred_terminal_term_name_with_paths(default_terminfo_search_paths())
}

fn default_terminfo_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(dir) = env::var_os("TERMINFO") {
        paths.push(PathBuf::from(dir));
    } else if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".terminfo"));
    }

    if let Ok(dirs) = env::var("TERMINFO_DIRS") {
        paths.extend(
            dirs.split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        );
    }

    if let Ok(prefix) = env::var("PREFIX") {
        let prefix = PathBuf::from(prefix);
        paths.push(prefix.join("etc/terminfo"));
        paths.push(prefix.join("lib/terminfo"));
        paths.push(prefix.join("share/terminfo"));
    }

    paths.extend([
        PathBuf::from("/etc/terminfo"),
        PathBuf::from("/lib/terminfo"),
        PathBuf::from("/usr/share/terminfo"),
        PathBuf::from("/boot/system/data/terminfo"),
    ]);

    paths
}

fn terminfo_exists_in_paths(terminfo: &str, paths: impl IntoIterator<Item = PathBuf>) -> bool {
    let Some(first_char) = terminfo.chars().next() else {
        return false;
    };
    let first = first_char.to_string();
    let first_hex = format!("{:x}", first_char as usize);

    paths
        .into_iter()
        .any(|path| terminfo_exists_under_path(&path, terminfo, &first, &first_hex))
}

fn terminfo_exists_under_path(path: &Path, terminfo: &str, first: &str, first_hex: &str) -> bool {
    path.join(first).join(terminfo).exists() || path.join(first_hex).join(terminfo).exists()
}

fn preferred_terminal_term_name_with_paths(
    paths: impl IntoIterator<Item = PathBuf>,
) -> &'static str {
    if terminfo_exists_in_paths(ALACRITTY_TERMINFO, paths) {
        ALACRITTY_TERMINFO
    } else {
        FALLBACK_TERMINFO
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    #[cfg(any(target_os = "linux", windows))]
    use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::vo::pty_host_id::PtyHostId};
    use germinal_ports::pty_host::{
        spawn_config::{PtyShellCommand, PtySpawnConfig},
        terminal_size::TerminalPtySize,
    };
    #[cfg(any(target_os = "linux", windows))]
    use germinal_ports::{
        event::{
            runtime_event::{GShellRuntimeEvent, RuntimeEvent},
            runtime_event_dispatcher::{IRuntimeEventDispatcher, RuntimeEventDispatchError},
        },
        pty_host::worker_input::TerminalWorkerInput,
    };
    #[cfg(any(target_os = "linux", windows))]
    use std::{sync::mpsc, thread, time::Duration};

    #[cfg(any(target_os = "linux", windows))]
    use super::PtyBridge;
    use super::{
        ALACRITTY_TERMINFO, FALLBACK_TERMINFO, PtyBridgeConfig,
        preferred_terminal_term_name_with_paths, terminfo_exists_in_paths,
    };

    #[cfg(any(target_os = "linux", windows))]
    #[derive(Clone)]
    struct TestDispatcher(mpsc::Sender<RuntimeEvent>);

    #[cfg(any(target_os = "linux", windows))]
    impl IRuntimeEventDispatcher for TestDispatcher {
        fn dispatch(&self, event: RuntimeEvent) -> Result<(), RuntimeEventDispatchError> {
            self.0
                .send(event)
                .map_err(|_| RuntimeEventDispatchError::Closed)
        }
    }

    static NEXT_TEMP_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let unique = NEXT_TEMP_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "germinal-pty-env-{label}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create isolated test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove isolated test directory");
        }
    }

    #[test]
    fn bridge_config_preserves_shell_and_working_directory() {
        let config = PtyBridgeConfig::new(
            Vec::new(),
            PtySpawnConfig {
                shell: Some(PtyShellCommand::new(
                    "/bin/test-shell".to_string(),
                    vec!["--login".to_string()],
                )),
                working_directory: Some("/tmp/project".into()),
                initial_size: TerminalPtySize::new(24, 80, 960, 576),
            },
        );

        assert_eq!(config.shell.program, "/bin/test-shell");
        assert_eq!(config.shell.args, ["--login"]);
        assert_eq!(config.working_directory, Some("/tmp/project".into()));
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn closing_pty_input_terminates_the_child_process() {
        const CHILD_TEST: &str = "pty::portable_pty_bridge::tests::pty_child_fixture";

        let (event_tx, event_rx) = mpsc::channel();
        let (worker_tx, worker_rx) = mpsc::sync_channel::<TerminalWorkerInput>(16);
        let test_executable = std::env::current_exe().expect("locate the current test executable");
        let input = PtyBridge::spawn(
            TestDispatcher(event_tx),
            GShellId::new(1),
            PtyHostId::new(1),
            PtySpawnConfig {
                shell: Some(PtyShellCommand::new(
                    test_executable.to_string_lossy().into_owned(),
                    vec![
                        "--exact".to_string(),
                        CHILD_TEST.to_string(),
                        "--ignored".to_string(),
                        "--nocapture".to_string(),
                    ],
                )),
                working_directory: None,
                initial_size: TerminalPtySize::new(24, 80, 640, 384),
            },
            Vec::new(),
            worker_tx,
        );

        let drain = drain_pty_output(worker_rx, input.clone());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if input.child_process_id().is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "PTY child did not start"
            );
            thread::sleep(Duration::from_millis(10));
        }

        input.close();

        let event = event_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("PTY bridge should report that the shell closed");
        assert_eq!(
            event,
            RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
                gshell_id: GShellId::new(1)
            })
        );
        drain.join().expect("PTY output drain");
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    #[ignore = "PTY child fixture; spawned by closing_pty_input_terminates_the_child_process"]
    fn pty_child_fixture() {
        thread::sleep(Duration::from_secs(30));
    }

    #[cfg(windows)]
    #[test]
    fn shell_exit_closes_bridge_while_input_sender_is_alive() {
        let (event_tx, event_rx) = mpsc::channel();
        let (worker_tx, worker_rx) = mpsc::sync_channel::<TerminalWorkerInput>(16);
        let input = PtyBridge::spawn(
            TestDispatcher(event_tx),
            GShellId::new(1),
            PtyHostId::new(1),
            PtySpawnConfig {
                shell: Some(PtyShellCommand::new(
                    "cmd".to_owned(),
                    vec!["/d".into(), "/c".into(), "exit 0".into()],
                )),
                working_directory: None,
                initial_size: TerminalPtySize::new(24, 80, 640, 384),
            },
            Vec::new(),
            worker_tx,
        );
        let drain = drain_pty_output(worker_rx, input.clone());
        let result = event_rx.recv_timeout(Duration::from_secs(5));
        // Clean up even when exercising the broken implementation.
        input.close();
        assert_eq!(
            result.expect("shell exit must not wait for the application to drop its sender"),
            RuntimeEvent::GShell(GShellRuntimeEvent::Closed {
                gshell_id: GShellId::new(1),
            })
        );
        drain.join().expect("PTY output drain");
    }

    #[cfg(any(target_os = "linux", windows))]
    fn drain_pty_output(
        worker_rx: mpsc::Receiver<TerminalWorkerInput>,
        reply_input: germinal_ports::pty_host::pty_input::PtyInputSender,
    ) -> thread::JoinHandle<()> {
        drain_pty_output_with_observer(worker_rx, reply_input, |_| {})
    }

    #[cfg(any(target_os = "linux", windows))]
    fn drain_pty_output_with_observer(
        worker_rx: mpsc::Receiver<TerminalWorkerInput>,
        reply_input: germinal_ports::pty_host::pty_input::PtyInputSender,
        mut observe: impl FnMut(&[u8]) + Send + 'static,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut output = Vec::new();
            let mut replied = false;
            while let Ok(message) = worker_rx.recv() {
                let TerminalWorkerInput::Bytes(bytes) = message else {
                    continue;
                };
                observe(&bytes);
                output.extend(bytes);
                // portable-pty enables PSEUDOCONSOLE_INHERIT_CURSOR. A real
                // terminal worker answers this query before ConPTY can proceed.
                if !replied && output.windows(4).any(|bytes| bytes == b"\x1b[6n") {
                    let _ = reply_input.send(germinal_ports::pty_host::pty_input::PtyInput::Bytes(
                        b"\x1b[1;1R".to_vec(),
                    ));
                    replied = true;
                }
            }
        })
    }

    #[cfg(windows)]
    #[test]
    fn interactive_windows_shells_report_directory_changes() {
        use crate::pty_host::{NotificationProtocolEvent, TerminalNotificationProtocolDecoder};
        use germinal_ports::pty_host::pty_input::PtyInput;
        let root = TestDirectory::new("directory reporting");
        let destination = root.path().join("changed directory");
        fs::create_dir(&destination).unwrap();
        for program in ["powershell.exe", "cmd.exe"] {
            let (event_tx, event_rx) = mpsc::channel();
            let (worker_tx, worker_rx) = mpsc::sync_channel(16);
            let (directory_tx, directory_rx) = mpsc::channel();
            let input = PtyBridge::spawn(
                TestDispatcher(event_tx),
                GShellId::new(1),
                PtyHostId::new(1),
                PtySpawnConfig {
                    shell: Some(PtyShellCommand::new(
                        program.into(),
                        if program == "powershell.exe" {
                            vec!["-NoLogo".into(), "-NoProfile".into()]
                        } else {
                            Vec::new()
                        },
                    )),
                    working_directory: Some(root.path().to_path_buf()),
                    initial_size: TerminalPtySize::new(24, 120, 960, 384),
                },
                Vec::new(),
                worker_tx,
            );
            let mut decoder = TerminalNotificationProtocolDecoder::default();
            let drain = drain_pty_output_with_observer(worker_rx, input.clone(), move |bytes| {
                for event in decoder.feed(bytes) {
                    if let NotificationProtocolEvent::WorkingDirectory(path) = event {
                        let _ = directory_tx.send(path);
                    }
                }
            });
            let wait_directory = || {
                directory_rx
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap_or_else(|error| {
                        input.close();
                        panic!("{program} did not report its directory: {error}");
                    })
            };
            let initial = wait_directory();
            assert_eq!(initial, root.path());
            let change = if program == "powershell.exe" {
                format!("Set-Location -LiteralPath '{}'\r", destination.display())
            } else {
                format!("cd /d \"{}\"\r", destination.display())
            };
            input.send(PtyInput::Bytes(change.into_bytes())).unwrap();
            let changed = wait_directory();
            input.close();
            assert_eq!(changed, destination);
            event_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("shell closed");
            drain.join().unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_resolution_respects_extensions_and_search_order() {
        use super::super::windows_shell::resolve;
        let root = TestDirectory::new("shell-path");
        let first = root.path().join("first");
        let second = root.path().join("second");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(first.join("shell.cmd"), []).unwrap();
        fs::write(first.join("shell.exe"), []).unwrap();
        fs::write(second.join("shell.exe"), []).unwrap();
        fs::create_dir(first.join("directory.exe")).unwrap();
        let path = std::env::join_paths([&first, &second]).unwrap();
        let find = |program, extensions| resolve(program, root.path(), &path, extensions);
        assert_eq!(find("shell", ".CMD;.EXE"), Some(first.join("shell.CMD")));
        assert_eq!(find("shell", ".EXE;.CMD"), Some(first.join("shell.EXE")));
        assert_eq!(find("shell.exe", ".CMD"), Some(first.join("shell.exe")));
        assert_eq!(find("shell.missing", ".EXE"), None);
        assert_eq!(find("directory", ".EXE"), None);
        assert_eq!(find("first/shell", ".EXE"), Some(first.join("shell.EXE")));
    }

    #[cfg(windows)]
    #[test]
    fn batch_shells_launch_in_conpty_with_literal_arguments() {
        let root = TestDirectory::new("batch shell & paths");
        for extension in ["cmd", "bat"] {
            let name = format!("shell-{extension}");
            fs::write(root.path().join(format!("{name}.{extension}")),
                "@echo off\r\n> \"%~dp0result.txt\" echo %1\r\n>> \"%~dp0result.txt\" echo %2\r\n>> \"%~dp0result.txt\" echo %3\r\n>> \"%~dp0result.txt\" echo %4\r\n").unwrap();
            let args = vec![
                "hello world".to_owned(),
                "literal & | < > ^ %PATH% !bang!".into(),
                String::new(),
                "C:\\trailing\\".into(),
            ];
            let program = root.path().join(name).to_string_lossy().into_owned();
            let (event_tx, event_rx) = mpsc::channel();
            let (worker_tx, worker_rx) = mpsc::sync_channel(16);
            let input = PtyBridge::spawn(
                TestDispatcher(event_tx),
                GShellId::new(1),
                PtyHostId::new(1),
                PtySpawnConfig {
                    shell: Some(PtyShellCommand::new(program.clone(), args)),
                    working_directory: Some(root.path().to_path_buf()),
                    initial_size: TerminalPtySize::new(24, 80, 640, 384),
                },
                Vec::new(),
                worker_tx,
            );
            let drain = drain_pty_output(worker_rx, input.clone());
            let result = event_rx.recv_timeout(Duration::from_secs(5));
            input.close();
            result.expect("batch shell should exit");
            drain.join().unwrap();
            assert_eq!(
                fs::read_to_string(root.path().join("result.txt")).unwrap(),
                "\"hello world\"\n\"literal & | < > ^ %PATH% !bang!\"\n\"\"\n\"C:\\trailing\\\"\n"
                    .replace('\n', "\r\n")
            );
            fs::remove_file(root.path().join("result.txt")).unwrap();
            assert!(
                super::super::windows_shell::command(&program, &["embedded\"quote".into()], None)
                    .is_err()
            );
        }
    }

    #[test]
    fn finds_terminfo_in_letter_directory() {
        let root = TestDirectory::new("letter");
        let entry_dir = root.path().join("a");
        fs::create_dir_all(&entry_dir).expect("create letter directory");
        fs::write(entry_dir.join(ALACRITTY_TERMINFO), []).expect("write terminfo file");

        assert!(terminfo_exists_in_paths(
            ALACRITTY_TERMINFO,
            vec![root.path().to_path_buf()]
        ));
    }

    #[test]
    fn finds_terminfo_in_hex_directory() {
        let root = TestDirectory::new("hex");
        let entry_dir = root.path().join("61");
        fs::create_dir_all(&entry_dir).expect("create hex directory");
        fs::write(entry_dir.join(ALACRITTY_TERMINFO), []).expect("write terminfo file");

        assert!(terminfo_exists_in_paths(
            ALACRITTY_TERMINFO,
            vec![root.path().to_path_buf()]
        ));
    }

    #[test]
    fn prefers_alacritty_only_when_its_terminfo_exists() {
        let missing_root = TestDirectory::new("missing");
        assert_eq!(
            preferred_terminal_term_name_with_paths(vec![missing_root.path().to_path_buf()]),
            FALLBACK_TERMINFO
        );

        let entry_dir = missing_root.path().join("a");
        fs::create_dir_all(&entry_dir).expect("create alacritty dir");
        fs::write(entry_dir.join(ALACRITTY_TERMINFO), []).expect("write alacritty terminfo");
        assert_eq!(
            preferred_terminal_term_name_with_paths(vec![missing_root.path().to_path_buf()]),
            ALACRITTY_TERMINFO
        );
    }
}
