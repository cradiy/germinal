use germinal_ports::{
	event::window_input_event::{
		WindowInputElementState, WindowInputKey, WindowInputModifiers, WindowInputNamedKey,
		WindowPointerButton, WindowPointerPosition, WindowScrollDelta,
	},
	pty_host::{
		terminal_input_mode::TerminalInputModes,
		terminal_size::TerminalPtySize,
	},
};

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
const MAX_SCROLL_REPORTS_PER_EVENT: i32 = 32;

pub(super) fn encode_key_event(
	modes: TerminalInputModes,
	modifiers: WindowInputModifiers,
	state: WindowInputElementState,
	logical_key: &WindowInputKey,
	text: Option<&str>,
) -> Option<Vec<u8>> {
	if state != WindowInputElementState::Pressed {
		return None;
	}

	if let Some(bytes) = named_key_bytes(modes, logical_key) {
		return Some(bytes);
	}

	if modifiers.control_key() {
		return ctrl_bytes_from_key(logical_key);
	}

	if modifiers.alt_key() {
		if let Some(bytes) = text_bytes(logical_key, text) {
			let mut escaped = Vec::with_capacity(bytes.len() + 1);
			escaped.push(0x1B);
			escaped.extend(bytes);
			return Some(escaped);
		}

		return None;
	}

	text_bytes(logical_key, text)
}

pub(super) fn encode_ime_commit(text: &str) -> Option<Vec<u8>> {
	(!text.is_empty()).then(|| text.as_bytes().to_vec())
}

pub(super) fn encode_paste(modes: TerminalInputModes, text: &str) -> Option<Vec<u8>> {
	if text.is_empty() {
		return None;
	}
	if !modes.bracketed_paste() {
		return Some(text.as_bytes().to_vec());
	}

	let mut bytes = Vec::with_capacity(
		BRACKETED_PASTE_START.len() + text.len() + BRACKETED_PASTE_END.len(),
	);
	bytes.extend_from_slice(BRACKETED_PASTE_START);
	bytes.extend(text.bytes().filter(|byte| *byte != 0x1B));
	bytes.extend_from_slice(BRACKETED_PASTE_END);
	Some(bytes)
}

pub(super) fn encode_focus_changed(
	modes: TerminalInputModes,
	focused: bool,
) -> Option<Vec<u8>> {
	modes.focus_in_out().then(|| {
		if focused { b"\x1b[I".to_vec() } else { b"\x1b[O".to_vec() }
	})
}

pub(super) struct PtyMouseEncoder {
	size:              TerminalPtySize,
	pressed_buttons:   Vec<WindowPointerButton>,
	last_pointer_cell: Option<(u16, u16)>,
	wheel_x:           f64,
	wheel_y:           f64,
}

impl PtyMouseEncoder {
	pub(super) fn new(size: TerminalPtySize) -> Self {
		Self {
			size,
			pressed_buttons: Vec::new(),
			last_pointer_cell: None,
			wheel_x: 0.0,
			wheel_y: 0.0,
		}
	}

	pub(super) fn resize(&mut self, size: TerminalPtySize) {
		self.size = size;
		self.last_pointer_cell = None;
		self.wheel_x = 0.0;
		self.wheel_y = 0.0;
	}

	pub(super) fn pointer_left(&mut self) {
		self.last_pointer_cell = None;
		self.pressed_buttons.clear();
	}

	pub(super) fn moved(
		&mut self,
		modes: TerminalInputModes,
		position: WindowPointerPosition,
		modifiers: WindowInputModifiers,
	) -> Option<Vec<u8>> {
		let cell = terminal_cell_at(position, self.size)?;
		if self.last_pointer_cell == Some(cell) {
			return None;
		}
		self.last_pointer_cell = Some(cell);
		if !mouse_reporting_enabled(modes) {
			return None;
		}

		let button = self.pressed_buttons.last().and_then(|button| mouse_button_code(*button));
		let report = if modes.mouse_motion() {
			Some(button.unwrap_or(3))
		} else if modes.mouse_drag() {
			button
		} else {
			None
		}?;
		Some(sgr_mouse_report(report + 32 + modifier_code(modifiers), cell, false))
	}

	pub(super) fn button(
		&mut self,
		modes: TerminalInputModes,
		state: WindowInputElementState,
		button: WindowPointerButton,
		position: WindowPointerPosition,
		modifiers: WindowInputModifiers,
	) -> Option<Vec<u8>> {
		let button_code = mouse_button_code(button)?;
		let cell = terminal_cell_at(position, self.size)?;
		self.last_pointer_cell = Some(cell);

		match state {
			WindowInputElementState::Pressed => {
				self.pressed_buttons.retain(|pressed| *pressed != button);
				self.pressed_buttons.push(button);
			},
			WindowInputElementState::Released => {
				self.pressed_buttons.retain(|pressed| *pressed != button);
			},
		}
		if !mouse_reporting_enabled(modes) {
			return None;
		}

		Some(sgr_mouse_report(
			button_code + modifier_code(modifiers),
			cell,
			state == WindowInputElementState::Released,
		))
	}

	pub(super) fn scroll(
		&mut self,
		modes: TerminalInputModes,
		delta: WindowScrollDelta,
		position: WindowPointerPosition,
		modifiers: WindowInputModifiers,
	) -> Vec<Vec<u8>> {
		let Some(cell) = terminal_cell_at(position, self.size) else {
			return Vec::new();
		};
		self.last_pointer_cell = Some(cell);
		if !mouse_reporting_enabled(modes) {
			return Vec::new();
		}

		let (x, y) = match delta {
			WindowScrollDelta::Lines { x, y } => (f64::from(x), f64::from(y)),
			WindowScrollDelta::Pixels { x, y } => {
				let cell_width = f64::from(self.size.pixel_width())
					/ f64::from(self.size.columns().max(1));
				let cell_height = f64::from(self.size.pixel_height())
					/ f64::from(self.size.rows().max(1));
				(x / cell_width.max(1.0), y / cell_height.max(1.0))
			},
		};
		let x_steps = take_scroll_steps(&mut self.wheel_x, x);
		let y_steps = take_scroll_steps(&mut self.wheel_y, y);
		let modifiers = modifier_code(modifiers);
		let mut reports = Vec::with_capacity((x_steps.abs() + y_steps.abs()) as usize);
		append_scroll_reports(&mut reports, y_steps, 64, 65, modifiers, cell);
		append_scroll_reports(&mut reports, x_steps, 67, 66, modifiers, cell);
		reports
	}
}

fn mouse_reporting_enabled(modes: TerminalInputModes) -> bool {
	modes.sgr_mouse() && modes.mouse_tracking()
}

fn terminal_cell_at(
	position: WindowPointerPosition,
	size: TerminalPtySize,
) -> Option<(u16, u16)> {
	if !position.x_px.is_finite()
		|| !position.y_px.is_finite()
		|| position.x_px < 0.0
		|| position.y_px < 0.0
		|| size.pixel_width() == 0
		|| size.pixel_height() == 0
	{
		return None;
	}

	let column = ((position.x_px * f64::from(size.columns())
		/ f64::from(size.pixel_width()))
		.floor() as u32
		+ 1)
		.clamp(1, u32::from(size.columns().max(1)));
	let row = ((position.y_px * f64::from(size.rows()) / f64::from(size.pixel_height())).floor()
		as u32
		+ 1)
		.clamp(1, u32::from(size.rows().max(1)));
	Some((column as u16, row as u16))
}

fn sgr_mouse_report(code: u8, cell: (u16, u16), released: bool) -> Vec<u8> {
	format!("\x1b[<{};{};{}{}", code, cell.0, cell.1, if released { 'm' } else { 'M' })
		.into_bytes()
}

fn mouse_button_code(button: WindowPointerButton) -> Option<u8> {
	match button {
		WindowPointerButton::Primary => Some(0),
		WindowPointerButton::Middle => Some(1),
		WindowPointerButton::Secondary => Some(2),
		WindowPointerButton::Back
		| WindowPointerButton::Forward
		| WindowPointerButton::Other(_) => None,
	}
}

fn modifier_code(modifiers: WindowInputModifiers) -> u8 {
	u8::from(modifiers.shift_key()) * 4
		+ u8::from(modifiers.alt_key()) * 8
		+ u8::from(modifiers.control_key()) * 16
}

fn take_scroll_steps(remainder: &mut f64, delta: f64) -> i32 {
	if !delta.is_finite() {
		return 0;
	}
	*remainder = (*remainder + delta).clamp(
		-f64::from(MAX_SCROLL_REPORTS_PER_EVENT),
		f64::from(MAX_SCROLL_REPORTS_PER_EVENT),
	);
	let steps = remainder.trunc() as i32;
	*remainder -= f64::from(steps);
	steps
}

fn append_scroll_reports(
	reports: &mut Vec<Vec<u8>>,
	steps: i32,
	positive_code: u8,
	negative_code: u8,
	modifiers: u8,
	cell: (u16, u16),
) {
	let code = (if steps >= 0 { positive_code } else { negative_code }) + modifiers;
	for _ in 0..steps.unsigned_abs() {
		reports.push(sgr_mouse_report(code, cell, false));
	}
}

fn named_key_bytes(modes: TerminalInputModes, key: &WindowInputKey) -> Option<Vec<u8>> {
	let app_cursor = modes.app_cursor();
	match key {
		WindowInputKey::Named(WindowInputNamedKey::Enter) => Some(b"\r".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::Tab) => Some(b"\t".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::Backspace) => Some(vec![0x7F]),
		WindowInputKey::Named(WindowInputNamedKey::Escape) => Some(vec![0x1B]),
		WindowInputKey::Named(WindowInputNamedKey::ArrowUp) => {
			Some(if app_cursor { b"\x1bOA" } else { b"\x1b[A" }.to_vec())
		},
		WindowInputKey::Named(WindowInputNamedKey::ArrowDown) => {
			Some(if app_cursor { b"\x1bOB" } else { b"\x1b[B" }.to_vec())
		},
		WindowInputKey::Named(WindowInputNamedKey::ArrowRight) => {
			Some(if app_cursor { b"\x1bOC" } else { b"\x1b[C" }.to_vec())
		},
		WindowInputKey::Named(WindowInputNamedKey::ArrowLeft) => {
			Some(if app_cursor { b"\x1bOD" } else { b"\x1b[D" }.to_vec())
		},
		WindowInputKey::Named(WindowInputNamedKey::Home) => Some(b"\x1b[H".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::End) => Some(b"\x1b[F".to_vec()),
		WindowInputKey::Named(WindowInputNamedKey::Delete) => Some(b"\x1b[3~".to_vec()),
		_ => None,
	}
}

fn text_bytes(key: &WindowInputKey, text: Option<&str>) -> Option<Vec<u8>> {
	if let Some(text) = text
		&& !text.is_empty()
	{
		return Some(text.as_bytes().to_vec());
	}

	match key {
		WindowInputKey::Character(text) if !text.is_empty() => Some(text.as_bytes().to_vec()),
		_ => None,
	}
}

fn ctrl_bytes_from_key(key: &WindowInputKey) -> Option<Vec<u8>> {
	let WindowInputKey::Character(text) = key else {
		return None;
	};

	let mut chars = text.chars();
	let c = chars.next()?.to_ascii_lowercase();
	if chars.next().is_some() {
		return None;
	}

	let byte = match c {
		'a' => 0x01,
		'b' => 0x02,
		'c' => 0x03,
		'd' => 0x04,
		'e' => 0x05,
		'f' => 0x06,
		'h' => 0x08,
		'i' => 0x09,
		'j' => 0x0A,
		'k' => 0x0B,
		'l' => 0x0C,
		'm' => 0x0D,
		'n' => 0x0E,
		'o' => 0x0F,
		'p' => 0x10,
		'q' => 0x11,
		'r' => 0x12,
		's' => 0x13,
		't' => 0x14,
		'u' => 0x15,
		'v' => 0x16,
		'w' => 0x17,
		'x' => 0x18,
		'y' => 0x19,
		'z' => 0x1A,
		'[' => 0x1B,
		'\\' => 0x1C,
		']' => 0x1D,
		'^' => 0x1E,
		'_' => 0x1F,
		_ => return None,
	};

	Some(vec![byte])
}

#[cfg(test)]
mod tests {
	use germinal_ports::event::window_input_event::{
		WindowInputElementState, WindowInputKey, WindowInputModifiers, WindowInputNamedKey,
		WindowPointerButton, WindowPointerPosition, WindowScrollDelta,
	};

	use super::*;

	fn modes(
		app_cursor: bool,
		bracketed_paste: bool,
		focus: bool,
		mouse_click: bool,
		mouse_drag: bool,
		mouse_motion: bool,
	) -> TerminalInputModes {
		TerminalInputModes::new(
			app_cursor,
			bracketed_paste,
			focus,
			mouse_click || mouse_drag || mouse_motion,
			mouse_click,
			mouse_drag,
			mouse_motion,
		)
	}

	#[test]
	fn application_cursor_mode_changes_arrow_sequences() {
		let modifiers = WindowInputModifiers::new(false, false, false, false);
		let up = WindowInputKey::Named(WindowInputNamedKey::ArrowUp);
		assert_eq!(
			encode_key_event(
				modes(false, false, false, false, false, false),
				modifiers,
				WindowInputElementState::Pressed,
				&up,
				None,
			),
			Some(b"\x1b[A".to_vec())
		);
		assert_eq!(
			encode_key_event(
				modes(true, false, false, false, false, false),
				modifiers,
				WindowInputElementState::Pressed,
				&up,
				None,
			),
			Some(b"\x1bOA".to_vec())
		);
	}

	#[test]
	fn bracketed_paste_wraps_text_and_removes_escape_bytes() {
		assert_eq!(
			encode_paste(modes(false, true, false, false, false, false), "a\x1bb"),
			Some(b"\x1b[200~ab\x1b[201~".to_vec())
		);
	}

	#[test]
	fn focus_reporting_only_emits_when_enabled() {
		assert_eq!(
			encode_focus_changed(modes(false, false, true, false, false, false), true),
			Some(b"\x1b[I".to_vec())
		);
		assert_eq!(
			encode_focus_changed(modes(false, false, false, false, false, false), true),
			None
		);
	}

	#[test]
	fn sgr_mouse_encodes_click_drag_and_release_in_terminal_cells() {
		let size = TerminalPtySize::new(10, 20, 200, 100);
		let mut encoder = PtyMouseEncoder::new(size);
		let mouse_modes = modes(false, false, false, false, true, false);
		let modifiers = WindowInputModifiers::new(true, false, true, false);
		let position = WindowPointerPosition::new(25.0, 35.0);

		assert_eq!(
			encoder.button(
				mouse_modes,
				WindowInputElementState::Pressed,
				WindowPointerButton::Primary,
				position,
				modifiers,
			),
			Some(b"\x1b[<20;3;4M".to_vec())
		);
		assert_eq!(
			encoder.moved(
				mouse_modes,
				WindowPointerPosition::new(35.0, 45.0),
				modifiers,
			),
			Some(b"\x1b[<52;4;5M".to_vec())
		);
		assert_eq!(
			encoder.button(
				mouse_modes,
				WindowInputElementState::Released,
				WindowPointerButton::Primary,
				WindowPointerPosition::new(35.0, 45.0),
				modifiers,
			),
			Some(b"\x1b[<20;4;5m".to_vec())
		);
	}

	#[test]
	fn sgr_mouse_accumulates_pixel_scroll_into_wheel_reports() {
		let mut encoder = PtyMouseEncoder::new(TerminalPtySize::new(10, 20, 200, 100));
		let mouse_modes = modes(false, false, false, true, false, false);
		let modifiers = WindowInputModifiers::new(false, false, false, false);
		let position = WindowPointerPosition::new(5.0, 5.0);

		assert!(
			encoder
				.scroll(
					mouse_modes,
					WindowScrollDelta::Pixels { x: 0.0, y: 5.0 },
					position,
					modifiers,
				)
				.is_empty()
		);
		assert_eq!(
			encoder.scroll(
				mouse_modes,
				WindowScrollDelta::Pixels { x: 0.0, y: 5.0 },
				position,
				modifiers,
			),
			vec![b"\x1b[<64;1;1M".to_vec()]
		);
	}
}
