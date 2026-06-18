#[derive(Debug, Clone)]
pub struct ShellCommand {
	pub program: String,
	pub args:    Vec<String>,
}

impl ShellCommand {
	pub fn new(program: String, args: Vec<String>) -> Self { Self { program, args } }
}

#[cfg(windows)]
pub fn default_shell_command() -> ShellCommand {
	let program = "powershell.exe".to_string();

	ShellCommand::new(program, vec!["-NoLogo".to_string()])
}

#[cfg(unix)]
pub fn default_shell_command() -> ShellCommand {
	let program = std::env::var("SHELL")
		.ok()
		.filter(|shell| !shell.trim().is_empty())
		.unwrap_or_else(|| "/bin/sh".to_string());

	ShellCommand::new(program, vec!["-i".to_string()])
}

#[cfg(not(any(unix, windows)))]
pub fn default_shell_command() -> ShellCommand { ShellCommand::new("sh".to_string(), Vec::new()) }
