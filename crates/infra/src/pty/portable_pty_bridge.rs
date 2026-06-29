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
		terminal_size::TerminalPtySize,
		worker_input::TerminalWorkerInput,
	},
};
use portable_pty::{CommandBuilder, PtySize};

use crate::pty::shell_command::{ShellCommand, default_shell_command};
#[cfg(unix)]
use crate::pty::unix_pty_bridge::spawn_compio_bridge_thread;
#[cfg(windows)]
use crate::pty::windows_pty_bridge::spawn_blocking_bridge_thread;

const ALACRITTY_TERMINFO: &str = "alacritty";
const FALLBACK_TERMINFO: &str = "xterm-256color";
const TRUECOLOR_COLORTERM: &str = "truecolor";

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
	pub fn spawn<Dispatch>(
		proxy: Dispatch,
		gshell_id: GShellId,
		pty_host_id: PtyHostId,
		initial_size: TerminalPtySize,
		terminal_worker_tx: SyncSender<TerminalWorkerInput>,
	) -> PtyInputSender
	where
		Dispatch: IRuntimeEventDispatcher,
	{
		Self::spawn_with_config(
			proxy,
			gshell_id,
			pty_host_id,
			PtyBridgeConfig::new(initial_size),
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

pub(crate) fn apply_default_terminal_env(command: &mut CommandBuilder) {
	command.env("TERM", preferred_terminal_term_name());
	command.env("COLORTERM", TRUECOLOR_COLORTERM);
}

pub(crate) fn to_portable_pty_size(size: TerminalPtySize) -> PtySize {
	PtySize {
		rows:         size.rows(),
		cols:         size.columns(),
		pixel_width:  size.pixel_width(),
		pixel_height: size.pixel_height(),
	}
}

fn preferred_terminal_term_name() -> &'static str {
	preferred_terminal_term_name_with_paths(default_terminfo_search_paths())
}

fn default_terminfo_search_paths() -> Vec<PathBuf> {
	let mut paths = Vec::new();

	if let Some(dir) = env::var_os("TERMINFO") {
		paths.push(PathBuf::from(dir));
	} else if let Some(home) = env::var_os("HOME") {
		paths.push(PathBuf::from(home).join(".terminfo"));
	}

	if let Ok(dirs) = env::var("TERMINFO_DIRS") {
		paths.extend(dirs.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from));
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

	paths.into_iter().any(|path| terminfo_exists_under_path(&path, terminfo, &first, &first_hex))
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
		time::{SystemTime, UNIX_EPOCH},
	};

	use super::{
		ALACRITTY_TERMINFO, FALLBACK_TERMINFO, preferred_terminal_term_name_with_paths,
		terminfo_exists_in_paths,
	};

	#[test]
	fn finds_terminfo_in_letter_directory() {
		let root = temp_path("letter");
		let entry_dir = root.join("a");
		fs::create_dir_all(&entry_dir).expect("create letter directory");
		fs::write(entry_dir.join(ALACRITTY_TERMINFO), []).expect("write terminfo file");

		assert!(terminfo_exists_in_paths(ALACRITTY_TERMINFO, vec![root]));
	}

	#[test]
	fn finds_terminfo_in_hex_directory() {
		let root = temp_path("hex");
		let entry_dir = root.join("61");
		fs::create_dir_all(&entry_dir).expect("create hex directory");
		fs::write(entry_dir.join(ALACRITTY_TERMINFO), []).expect("write terminfo file");

		assert!(terminfo_exists_in_paths(ALACRITTY_TERMINFO, vec![root]));
	}

	#[test]
	fn prefers_alacritty_only_when_its_terminfo_exists() {
		let missing_root = temp_path("missing");
		fs::create_dir_all(&missing_root).expect("create missing root");
		assert_eq!(
			preferred_terminal_term_name_with_paths(vec![missing_root.clone()]),
			FALLBACK_TERMINFO
		);

		let entry_dir = missing_root.join("a");
		fs::create_dir_all(&entry_dir).expect("create alacritty dir");
		fs::write(entry_dir.join(ALACRITTY_TERMINFO), []).expect("write alacritty terminfo");
		assert_eq!(preferred_terminal_term_name_with_paths(vec![missing_root]), ALACRITTY_TERMINFO);
	}

	fn temp_path(label: &str) -> std::path::PathBuf {
		let unique = SystemTime::now().duration_since(UNIX_EPOCH).expect("monotonic time").as_nanos();
		std::env::temp_dir().join(format!("germinal-pty-env-{label}-{unique}"))
	}
}
