use germinal_domain::gshell::vo::gshell_id::GShellId;

use crate::event::window_input_event::WindowInputEvent;
use crate::pty_host::terminal_clipboard::TerminalClipboard;

#[derive(Debug, Clone, PartialEq)]
pub struct GShellInput {
    pub gshell_id: GShellId,
    pub event: GShellInputEvent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GShellInputEvent {
    Bytes(Vec<u8>),
    Paste(String),
    CopySelection,
    Osc52ClipboardLoadResponse {
        clipboard: TerminalClipboard,
        request_id: u64,
        text: Option<String>,
    },
    ToggleViMode,
    ToggleSearch,
    Window(WindowInputEvent),
}
