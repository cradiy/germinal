use germinal_domain::workspace::pane_id::PaneId;

use crate::event::window_input_event::WindowInputEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GShellInput {
	pub pane_id: PaneId,
	pub event:   GShellInputEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GShellInputEvent {
	Bytes(Vec<u8>),
	Paste(String),
	Window(WindowInputEvent),
}
