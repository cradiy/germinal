#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowInputElementState {
	Pressed,
	Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowInputModifiers {
	control: bool,
	alt:     bool,
}

impl WindowInputModifiers {
	pub fn new(control: bool, alt: bool) -> Self { Self { control, alt } }

	pub fn control_key(&self) -> bool { self.control }

	pub fn alt_key(&self) -> bool { self.alt }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowInputNamedKey {
	Enter,
	Tab,
	Backspace,
	Escape,
	ArrowUp,
	ArrowDown,
	ArrowRight,
	ArrowLeft,
	Home,
	End,
	Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowInputKey {
	Named(WindowInputNamedKey),
	Character(String),
	Unidentified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowInputEvent {
	ModifiersChanged(WindowInputModifiers),
	Key {
		state:       WindowInputElementState,
		logical_key: WindowInputKey,
		text:        Option<String>,
	},
	Ime(String),
	Paste(String),
}
