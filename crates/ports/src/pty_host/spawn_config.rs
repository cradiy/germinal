use std::path::PathBuf;

use crate::pty_host::terminal_size::TerminalPtySize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyShellCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl PtyShellCommand {
    pub fn new(program: String, args: Vec<String>) -> Self {
        Self { program, args }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtySpawnConfig {
    pub shell: Option<PtyShellCommand>,
    pub working_directory: Option<PathBuf>,
    pub initial_size: TerminalPtySize,
}
