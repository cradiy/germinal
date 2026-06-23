use serde::{Deserialize, Serialize};

use crate::event::window_input_event::{
	WindowInputElementState, WindowInputKey, WindowInputModifiers, WindowInputNamedKey,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeInputModifiers {
	pub control: bool,
	pub alt:     bool,
}

impl From<WindowInputModifiers> for GNativeInputModifiers {
	fn from(value: WindowInputModifiers) -> Self {
		Self { control: value.control_key(), alt: value.alt_key() }
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeInputNamedKey {
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

impl From<WindowInputNamedKey> for GNativeInputNamedKey {
	fn from(value: WindowInputNamedKey) -> Self {
		match value {
			WindowInputNamedKey::Enter => Self::Enter,
			WindowInputNamedKey::Tab => Self::Tab,
			WindowInputNamedKey::Backspace => Self::Backspace,
			WindowInputNamedKey::Escape => Self::Escape,
			WindowInputNamedKey::ArrowUp => Self::ArrowUp,
			WindowInputNamedKey::ArrowDown => Self::ArrowDown,
			WindowInputNamedKey::ArrowRight => Self::ArrowRight,
			WindowInputNamedKey::ArrowLeft => Self::ArrowLeft,
			WindowInputNamedKey::Home => Self::Home,
			WindowInputNamedKey::End => Self::End,
			WindowInputNamedKey::Delete => Self::Delete,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeInputKey {
	Named(GNativeInputNamedKey),
	Character(String),
	Unidentified,
}

impl From<&WindowInputKey> for GNativeInputKey {
	fn from(value: &WindowInputKey) -> Self {
		match value {
			WindowInputKey::Named(named) => Self::Named((*named).into()),
			WindowInputKey::Character(text) => Self::Character(text.to_string()),
			WindowInputKey::Unidentified => Self::Unidentified,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GNativeInputElementState {
	Pressed,
	Released,
}

impl From<WindowInputElementState> for GNativeInputElementState {
	fn from(value: WindowInputElementState) -> Self {
		match value {
			WindowInputElementState::Pressed => Self::Pressed,
			WindowInputElementState::Released => Self::Released,
		}
	}
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
		columns: u32,
		rows:    u32,
	},
}
