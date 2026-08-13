use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeInputModifiers {
	pub control:   bool,
	pub alt:       bool,
	pub shift:     bool,
	pub super_key: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativePointerButton {
	Primary,
	Secondary,
	Middle,
	Back,
	Forward,
	Other(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GNativePointerPosition {
	pub x_px: f64,
	pub y_px: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GNativeScrollDelta {
	Lines { x: f32, y: f32 },
	Pixels { x: f64, y: f64 },
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
	ModifiersChanged(GNativeInputModifiers),
	FocusChanged(bool),
	PointerMoved {
		position:  GNativePointerPosition,
		modifiers: GNativeInputModifiers,
	},
	PointerLeft,
	PointerButton {
		state:     GNativeInputElementState,
		button:    GNativePointerButton,
		position:  GNativePointerPosition,
		modifiers: GNativeInputModifiers,
	},
	Scroll {
		delta:     GNativeScrollDelta,
		position:  GNativePointerPosition,
		modifiers: GNativeInputModifiers,
	},
	Resize {
		columns:           u32,
		rows:              u32,
		content_width_px:  u32,
		content_height_px: u32,
		cell_width_px:     u32,
		cell_height_px:    u32,
	},
}
