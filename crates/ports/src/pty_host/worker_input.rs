use germinal_domain::pty_host::terminal_size::TerminalGridSize;

use crate::pty_host::{
	pty_input::PtyInputSender,
	terminal_input_mode::TerminalInputModeState,
};

pub enum TerminalWorkerInput {
	Bytes(Vec<u8>),
	Resize(TerminalGridSize),
	SetPtyInput {
		sender:      PtyInputSender,
		input_modes: TerminalInputModeState,
	},
}
