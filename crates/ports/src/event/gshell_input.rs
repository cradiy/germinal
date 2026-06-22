use germinal_domain::gshell::vo::gshell_id::GShellId;

use crate::event::window_input_event::WindowInputEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GShellInput {
	pub gshell_id: GShellId,
	pub event:     GShellInputEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GShellInputEvent {
	Bytes(Vec<u8>),
	Paste(String),
	Window(WindowInputEvent),
}
