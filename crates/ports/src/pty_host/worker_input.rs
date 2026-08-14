use germinal_domain::pty_host::terminal_size::TerminalGridSize;

use crate::pty_host::{pty_input::PtyInputSender, terminal_input_mode::TerminalInputModeState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalDisplayScroll {
    Delta(i32),
    PageUp,
    PageDown,
    Top,
    Bottom,
}

pub enum TerminalWorkerInput {
    Bytes(Vec<u8>),
    Resize(TerminalGridSize),
    ScrollDisplay(TerminalDisplayScroll),
    SetPtyInput {
        sender: PtyInputSender,
        input_modes: TerminalInputModeState,
    },
}
