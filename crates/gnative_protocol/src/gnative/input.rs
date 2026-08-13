use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeInputModifiers {
	pub control: bool,
	pub alt:     bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeInputNamedKey {
	F1,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeInputKey {
	Named(GNativeInputNamedKey),
	Character(String),
	Unidentified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeInputElementState {
	Pressed,
	Released,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeInputEvent {
	Bytes(Vec<u8>),
	Paste(String),
	Key {
		state:       GNativeInputElementState,
		logical_key: GNativeInputKey,
		text:        Option<String>,
		modifiers:   GNativeInputModifiers,
	},
	Ime(String),
	Resize {
		columns:           u32,
		rows:              u32,
		content_width_px:  u32,
		content_height_px: u32,
		cell_width_px:     u32,
		cell_height_px:    u32,
	},
}
