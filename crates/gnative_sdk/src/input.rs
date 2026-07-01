use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use germinal_gnative_protocol::gnative::input::{
	GNativeInputElementState, GNativeInputEvent, GNativeInputKey, GNativeInputNamedKey,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnsupportedCrosstermEvent {
	Bytes(Vec<u8>),
	Ime(String),
	Character(String),
}

pub fn try_to_crossterm_event(
	input: GNativeInputEvent,
) -> Result<Event, UnsupportedCrosstermEvent> {
	match input {
		GNativeInputEvent::Bytes(bytes) => Err(UnsupportedCrosstermEvent::Bytes(bytes)),
		GNativeInputEvent::Paste(text) => Ok(Event::Paste(text)),
		GNativeInputEvent::Ime(text) => Err(UnsupportedCrosstermEvent::Ime(text)),
		GNativeInputEvent::Resize { columns, rows } => Ok(Event::Resize(
			u16::try_from(columns).unwrap_or(u16::MAX),
			u16::try_from(rows).unwrap_or(u16::MAX),
		)),
		GNativeInputEvent::Key { state, logical_key, text, modifiers } => Ok(Event::Key(KeyEvent {
			code:      key_code_of(logical_key, text.as_deref())?,
			modifiers: modifiers_of(modifiers.control, modifiers.alt),
			kind:      key_event_kind_of(state),
			state:     KeyEventState::empty(),
		})),
	}
}

fn key_code_of(
	key: GNativeInputKey,
	text: Option<&str>,
) -> Result<KeyCode, UnsupportedCrosstermEvent> {
	match key {
		GNativeInputKey::Named(named) => Ok(match named {
			GNativeInputNamedKey::Enter => KeyCode::Enter,
			GNativeInputNamedKey::Tab => KeyCode::Tab,
			GNativeInputNamedKey::Backspace => KeyCode::Backspace,
			GNativeInputNamedKey::Escape => KeyCode::Esc,
			GNativeInputNamedKey::ArrowUp => KeyCode::Up,
			GNativeInputNamedKey::ArrowDown => KeyCode::Down,
			GNativeInputNamedKey::ArrowRight => KeyCode::Right,
			GNativeInputNamedKey::ArrowLeft => KeyCode::Left,
			GNativeInputNamedKey::Home => KeyCode::Home,
			GNativeInputNamedKey::End => KeyCode::End,
			GNativeInputNamedKey::Delete => KeyCode::Delete,
		}),
		GNativeInputKey::Character(text) => single_char_key_code(&text),
		GNativeInputKey::Unidentified => {
			if let Some(text) = text {
				single_char_key_code(text)
			} else {
				Ok(KeyCode::Null)
			}
		}
	}
}

fn single_char_key_code(text: &str) -> Result<KeyCode, UnsupportedCrosstermEvent> {
	let mut chars = text.chars();
	let Some(first) = chars.next() else {
		return Err(UnsupportedCrosstermEvent::Character(text.to_string()));
	};
	if chars.next().is_some() {
		return Err(UnsupportedCrosstermEvent::Character(text.to_string()));
	}
	Ok(KeyCode::Char(first))
}

fn modifiers_of(control: bool, alt: bool) -> KeyModifiers {
	let mut modifiers = KeyModifiers::empty();
	if control {
		modifiers |= KeyModifiers::CONTROL;
	}
	if alt {
		modifiers |= KeyModifiers::ALT;
	}
	modifiers
}

fn key_event_kind_of(state: GNativeInputElementState) -> KeyEventKind {
	match state {
		GNativeInputElementState::Pressed => KeyEventKind::Press,
		GNativeInputElementState::Released => KeyEventKind::Release,
	}
}

#[cfg(test)]
mod tests {
	use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
	use germinal_gnative_protocol::gnative::input::{
		GNativeInputElementState, GNativeInputEvent, GNativeInputKey, GNativeInputModifiers,
		GNativeInputNamedKey,
	};

	use super::{UnsupportedCrosstermEvent, try_to_crossterm_event};

	#[test]
	fn maps_key_input_to_crossterm_key_event() {
		let event = try_to_crossterm_event(GNativeInputEvent::Key {
			state:       GNativeInputElementState::Pressed,
			logical_key: GNativeInputKey::Named(GNativeInputNamedKey::Enter),
			text:        Some("\n".to_string()),
			modifiers:   GNativeInputModifiers { control: true, alt: false },
		})
		.expect("named key should convert");

		assert!(matches!(
			event,
			Event::Key(key)
				if key.code == KeyCode::Enter
					&& key.kind == KeyEventKind::Press
					&& key.modifiers == KeyModifiers::CONTROL
		));
	}

	#[test]
	fn maps_resize_and_paste_events() {
		assert_eq!(
			try_to_crossterm_event(GNativeInputEvent::Resize { columns: 132, rows: 40 }),
			Ok(Event::Resize(132, 40))
		);
		assert_eq!(
			try_to_crossterm_event(GNativeInputEvent::Paste("hello".to_string())),
			Ok(Event::Paste("hello".to_string()))
		);
	}

	#[test]
	fn rejects_non_crossterm_event_kinds() {
		assert_eq!(
			try_to_crossterm_event(GNativeInputEvent::Ime("nihao".to_string())),
			Err(UnsupportedCrosstermEvent::Ime("nihao".to_string()))
		);
		assert_eq!(
			try_to_crossterm_event(GNativeInputEvent::Bytes(vec![0x1B])),
			Err(UnsupportedCrosstermEvent::Bytes(vec![0x1B]))
		);
		assert_eq!(
			try_to_crossterm_event(GNativeInputEvent::Key {
				state:       GNativeInputElementState::Pressed,
				logical_key: GNativeInputKey::Character("ab".to_string()),
				text:        Some("ab".to_string()),
				modifiers:   GNativeInputModifiers { control: false, alt: false },
			}),
			Err(UnsupportedCrosstermEvent::Character("ab".to_string()))
		);
	}

	#[test]
	fn falls_back_to_text_for_unidentified_single_char_keys() {
		let event = try_to_crossterm_event(GNativeInputEvent::Key {
			state:       GNativeInputElementState::Pressed,
			logical_key: GNativeInputKey::Unidentified,
			text:        Some(" ".to_string()),
			modifiers:   GNativeInputModifiers { control: false, alt: false },
		})
		.expect("text fallback should convert");

		assert!(matches!(
			event,
			Event::Key(key)
				if key.code == KeyCode::Char(' ')
					&& key.kind == KeyEventKind::Press
					&& key.modifiers == KeyModifiers::empty()
		));
	}
}
