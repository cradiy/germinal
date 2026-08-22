use germinal_ports::{
    event::window_input_event::{
        WindowInputElementState, WindowInputKey, WindowInputModifiers, WindowInputNamedKey,
        WindowPointerButton, WindowPointerPosition, WindowScrollDelta,
    },
    pty_host::{
        terminal_input_mode::TerminalInputModes,
        terminal_size::TerminalPtySize,
        worker_input::{TerminalSelectionPoint, TerminalSelectionSide},
    },
};

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";
const MAX_SCROLL_REPORTS_PER_EVENT: i32 = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PtyScrollAction {
    ReportToPty(Vec<Vec<u8>>),
    ScrollDisplay(i32),
}

#[cfg(test)]
pub(super) fn encode_key_event(
    modes: TerminalInputModes,
    modifiers: WindowInputModifiers,
    state: WindowInputElementState,
    logical_key: &WindowInputKey,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    encode_key_event_with_repeat(modes, modifiers, state, false, logical_key, text)
}

pub(super) fn encode_key_event_with_repeat(
    modes: TerminalInputModes,
    modifiers: WindowInputModifiers,
    state: WindowInputElementState,
    repeat: bool,
    logical_key: &WindowInputKey,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    if modes.kitty_keyboard() {
        return encode_kitty_key_event(modes, modifiers, state, repeat, logical_key, text);
    }

    if state != WindowInputElementState::Pressed {
        return None;
    }

    if let Some(bytes) = named_key_bytes(modes, modifiers, logical_key) {
        return Some(bytes);
    }

    if modifiers.control_key() {
        return ctrl_bytes_from_key(logical_key)
            .or_else(|| control_text_bytes(text))
            .map(|bytes| prefix_escape_if(bytes, modifiers.alt_key()));
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

    let mut bytes =
        Vec::with_capacity(BRACKETED_PASTE_START.len() + text.len() + BRACKETED_PASTE_END.len());
    bytes.extend_from_slice(BRACKETED_PASTE_START);
    bytes.extend(text.bytes().filter(|byte| *byte != 0x1B));
    bytes.extend_from_slice(BRACKETED_PASTE_END);
    Some(bytes)
}

pub(super) fn encode_focus_changed(modes: TerminalInputModes, focused: bool) -> Option<Vec<u8>> {
    modes.focus_in_out().then(|| {
        if focused {
            b"\x1b[I".to_vec()
        } else {
            b"\x1b[O".to_vec()
        }
    })
}

pub(super) struct PtyMouseEncoder {
    size: TerminalPtySize,
    pressed_buttons: Vec<WindowPointerButton>,
    last_pointer_location: Option<(u16, u16)>,
    wheel_x: f64,
    wheel_y: f64,
}

impl PtyMouseEncoder {
    pub(super) fn new(size: TerminalPtySize) -> Self {
        Self {
            size,
            pressed_buttons: Vec::new(),
            last_pointer_location: None,
            wheel_x: 0.0,
            wheel_y: 0.0,
        }
    }

    pub(super) fn resize(&mut self, size: TerminalPtySize) {
        self.size = size;
        self.last_pointer_location = None;
        self.wheel_x = 0.0;
        self.wheel_y = 0.0;
    }

    pub(super) fn pointer_left(&mut self) {
        self.last_pointer_location = None;
        self.pressed_buttons.clear();
    }

    pub(super) fn selection_point(
        &self,
        position: WindowPointerPosition,
    ) -> Option<TerminalSelectionPoint> {
        terminal_selection_point_at(position, self.size)
    }

    pub(super) fn moved(
        &mut self,
        modes: TerminalInputModes,
        position: WindowPointerPosition,
        modifiers: WindowInputModifiers,
    ) -> Option<Vec<u8>> {
        let location = terminal_mouse_location(modes, position, self.size)?;
        if self.last_pointer_location == Some(location) {
            return None;
        }
        self.last_pointer_location = Some(location);
        if !mouse_reporting_enabled(modes) {
            return None;
        }

        let button = self
            .pressed_buttons
            .last()
            .and_then(|button| mouse_button_code(*button));
        let report = if modes.mouse_motion() {
            Some(button.unwrap_or(3))
        } else if modes.mouse_drag() {
            button
        } else {
            None
        }?;
        Some(mouse_report(
            modes,
            report + 32 + modifier_code(modifiers),
            location,
            false,
        ))
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
        let location = terminal_mouse_location(modes, position, self.size)?;
        self.last_pointer_location = Some(location);

        match state {
            WindowInputElementState::Pressed => {
                self.pressed_buttons.retain(|pressed| *pressed != button);
                self.pressed_buttons.push(button);
            }
            WindowInputElementState::Released => {
                self.pressed_buttons.retain(|pressed| *pressed != button);
            }
        }
        if !mouse_reporting_enabled(modes) {
            return None;
        }

        Some(mouse_report(
            modes,
            button_code + modifier_code(modifiers),
            location,
            state == WindowInputElementState::Released,
        ))
    }

    pub(super) fn scroll(
        &mut self,
        modes: TerminalInputModes,
        delta: WindowScrollDelta,
        position: WindowPointerPosition,
        modifiers: WindowInputModifiers,
    ) -> PtyScrollAction {
        let (x, y) = match delta {
            WindowScrollDelta::Lines { x, y } => (f64::from(x), f64::from(y)),
            WindowScrollDelta::Pixels { x, y } => {
                let cell_width =
                    f64::from(self.size.pixel_width()) / f64::from(self.size.columns().max(1));
                let cell_height =
                    f64::from(self.size.pixel_height()) / f64::from(self.size.rows().max(1));
                (x / cell_width.max(1.0), y / cell_height.max(1.0))
            }
        };
        let x_steps = take_scroll_steps(&mut self.wheel_x, x);
        let y_steps = take_scroll_steps(&mut self.wheel_y, y);

        if !mouse_reporting_enabled(modes) {
            return PtyScrollAction::ScrollDisplay(y_steps);
        }

        let Some(location) = terminal_mouse_location(modes, position, self.size) else {
            return PtyScrollAction::ReportToPty(Vec::new());
        };
        self.last_pointer_location = Some(location);
        let modifiers = modifier_code(modifiers);
        let mut reports = Vec::with_capacity((x_steps.abs() + y_steps.abs()) as usize);
        append_scroll_reports(&mut reports, modes, y_steps, 64, 65, modifiers, location);
        append_scroll_reports(&mut reports, modes, x_steps, 67, 66, modifiers, location);
        PtyScrollAction::ReportToPty(reports)
    }
}

pub(super) fn mouse_reporting_enabled(modes: TerminalInputModes) -> bool {
    (modes.sgr_pixel_mouse() || modes.sgr_mouse() || modes.urxvt_mouse()) && modes.mouse_tracking()
}

fn terminal_mouse_location(
    modes: TerminalInputModes,
    position: WindowPointerPosition,
    size: TerminalPtySize,
) -> Option<(u16, u16)> {
    if modes.sgr_pixel_mouse() {
        terminal_pixel_at(position, size)
    } else {
        terminal_cell_at(position, size)
    }
}

fn terminal_pixel_at(position: WindowPointerPosition, size: TerminalPtySize) -> Option<(u16, u16)> {
    if !position.x_px.is_finite()
        || !position.y_px.is_finite()
        || position.x_px < 0.0
        || position.y_px < 0.0
        || size.pixel_width() == 0
        || size.pixel_height() == 0
    {
        return None;
    }

    let x = (position.x_px.floor() as u32).min(u32::from(size.pixel_width() - 1)) as u16 + 1;
    let y = (position.y_px.floor() as u32).min(u32::from(size.pixel_height() - 1)) as u16 + 1;
    Some((x, y))
}

fn terminal_cell_at(position: WindowPointerPosition, size: TerminalPtySize) -> Option<(u16, u16)> {
    let point = terminal_selection_point_at(position, size)?;
    Some((point.column + 1, point.row + 1))
}

fn terminal_selection_point_at(
    position: WindowPointerPosition,
    size: TerminalPtySize,
) -> Option<TerminalSelectionPoint> {
    if !position.x_px.is_finite()
        || !position.y_px.is_finite()
        || position.x_px < 0.0
        || position.y_px < 0.0
        || size.pixel_width() == 0
        || size.pixel_height() == 0
    {
        return None;
    }

    let columns = size.columns().max(1);
    let rows = size.rows().max(1);
    let scaled_x = position.x_px * f64::from(columns) / f64::from(size.pixel_width());
    let scaled_y = position.y_px * f64::from(rows) / f64::from(size.pixel_height());
    let column = (scaled_x.floor() as u32).min(u32::from(columns - 1)) as u16;
    let row = (scaled_y.floor() as u32).min(u32::from(rows - 1)) as u16;
    let side = if scaled_x >= f64::from(columns) || scaled_x.fract() >= 0.5 {
        TerminalSelectionSide::Right
    } else {
        TerminalSelectionSide::Left
    };

    Some(TerminalSelectionPoint::new(column, row, side))
}

fn mouse_report(modes: TerminalInputModes, code: u8, cell: (u16, u16), released: bool) -> Vec<u8> {
    if modes.sgr_pixel_mouse() || modes.sgr_mouse() {
        format!(
            "\x1b[<{};{};{}{}",
            code,
            cell.0,
            cell.1,
            if released { 'm' } else { 'M' }
        )
        .into_bytes()
    } else {
        let code = if released { (code & !0b11) | 3 } else { code } + 32;
        format!("\x1b[{};{};{}M", code, cell.0, cell.1).into_bytes()
    }
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
    modes: TerminalInputModes,
    steps: i32,
    positive_code: u8,
    negative_code: u8,
    modifiers: u8,
    cell: (u16, u16),
) {
    let code = (if steps >= 0 {
        positive_code
    } else {
        negative_code
    }) + modifiers;
    for _ in 0..steps.unsigned_abs() {
        reports.push(mouse_report(modes, code, cell, false));
    }
}

fn named_key_bytes(
    modes: TerminalInputModes,
    modifiers: WindowInputModifiers,
    key: &WindowInputKey,
) -> Option<Vec<u8>> {
    let WindowInputKey::Named(key) = key else {
        return None;
    };

    let bytes = match key {
        WindowInputNamedKey::Enter => prefix_escape_if(b"\r".to_vec(), modifiers.alt_key()),
        WindowInputNamedKey::Tab if modifiers.shift_key() => {
            prefix_escape_if(b"\x1b[Z".to_vec(), modifiers.alt_key())
        }
        WindowInputNamedKey::Tab => prefix_escape_if(b"\t".to_vec(), modifiers.alt_key()),
        WindowInputNamedKey::Backspace => prefix_escape_if(vec![0x7F], modifiers.alt_key()),
        WindowInputNamedKey::Escape => prefix_escape_if(vec![0x1B], modifiers.alt_key()),
        WindowInputNamedKey::ArrowUp => cursor_key_sequence('A', modes.app_cursor(), modifiers),
        WindowInputNamedKey::ArrowDown => cursor_key_sequence('B', modes.app_cursor(), modifiers),
        WindowInputNamedKey::ArrowRight => cursor_key_sequence('C', modes.app_cursor(), modifiers),
        WindowInputNamedKey::ArrowLeft => cursor_key_sequence('D', modes.app_cursor(), modifiers),
        WindowInputNamedKey::Home => cursor_key_sequence('H', modes.app_cursor(), modifiers),
        WindowInputNamedKey::End => cursor_key_sequence('F', modes.app_cursor(), modifiers),
        WindowInputNamedKey::Insert => tilde_key_sequence(2, modifiers),
        WindowInputNamedKey::Delete => tilde_key_sequence(3, modifiers),
        WindowInputNamedKey::PageUp => tilde_key_sequence(5, modifiers),
        WindowInputNamedKey::PageDown => tilde_key_sequence(6, modifiers),
        WindowInputNamedKey::F1 => function_key_sequence('P', modifiers),
        WindowInputNamedKey::F2 => function_key_sequence('Q', modifiers),
        WindowInputNamedKey::F3 => function_key_sequence('R', modifiers),
        WindowInputNamedKey::F4 => function_key_sequence('S', modifiers),
        WindowInputNamedKey::F5 => tilde_key_sequence(15, modifiers),
        WindowInputNamedKey::F6 => tilde_key_sequence(17, modifiers),
        WindowInputNamedKey::F7 => tilde_key_sequence(18, modifiers),
        WindowInputNamedKey::F8 => tilde_key_sequence(19, modifiers),
        WindowInputNamedKey::F9 => tilde_key_sequence(20, modifiers),
        WindowInputNamedKey::F10 => tilde_key_sequence(21, modifiers),
        WindowInputNamedKey::F11 => tilde_key_sequence(23, modifiers),
        WindowInputNamedKey::F12 => tilde_key_sequence(24, modifiers),
        WindowInputNamedKey::CapsLock
        | WindowInputNamedKey::ScrollLock
        | WindowInputNamedKey::NumLock
        | WindowInputNamedKey::PrintScreen
        | WindowInputNamedKey::Pause
        | WindowInputNamedKey::ContextMenu
        | WindowInputNamedKey::Shift
        | WindowInputNamedKey::Control
        | WindowInputNamedKey::Alt
        | WindowInputNamedKey::Super => return None,
    };

    Some(bytes)
}

fn encode_kitty_key_event(
    modes: TerminalInputModes,
    modifiers: WindowInputModifiers,
    state: WindowInputElementState,
    repeat: bool,
    logical_key: &WindowInputKey,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    if state == WindowInputElementState::Released && !modes.kitty_report_event_types() {
        return None;
    }

    match logical_key {
        WindowInputKey::Named(key) => {
            kitty_named_key_bytes(modes, modifiers, state, repeat, *key, text)
        }
        WindowInputKey::Character(key) => {
            kitty_character_key_bytes(modes, modifiers, state, repeat, key, text)
        }
        WindowInputKey::Unidentified => None,
    }
}

fn kitty_character_key_bytes(
    modes: TerminalInputModes,
    modifiers: WindowInputModifiers,
    state: WindowInputElementState,
    repeat: bool,
    key: &str,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    if state == WindowInputElementState::Pressed
        && !modes.kitty_report_all_keys_as_escape_codes()
        && !modifiers.control_key()
        && !modifiers.alt_key()
        && !modifiers.super_key()
    {
        return text_bytes(&WindowInputKey::Character(key.to_string()), text);
    }

    let mut characters = key.chars();
    let shifted = characters.next()?;
    if characters.next().is_some() {
        return (state == WindowInputElementState::Pressed
            && !modes.kitty_report_all_keys_as_escape_codes())
        .then(|| text.unwrap_or(key).as_bytes().to_vec());
    }

    let event_type = kitty_event_type(modes, state, repeat);
    let is_legacy_key = shifted.is_ascii_alphanumeric() || shifted.is_ascii_punctuation();
    let use_legacy = !modes.kitty_report_alternate_keys()
        && event_type.is_empty()
        && is_legacy_key
        && !(modes.kitty_disambiguate_esc_codes()
            && (modifiers.control_key() || modifiers.alt_key()))
        && !modifiers.super_key()
        && !modes.kitty_report_all_keys_as_escape_codes();
    if use_legacy {
        return legacy_character_bytes(modifiers, shifted, text);
    }

    let base = kitty_unshift_ascii(shifted);
    let mut key_code = u32::from(base).to_string();
    if modes.kitty_report_alternate_keys() && base != shifted {
        key_code.push(':');
        key_code.push_str(&u32::from(shifted).to_string());
    }
    let modifiers = kitty_modifier_parameter(modifiers);
    let associated_text = kitty_associated_text(modes, state, text);
    Some(format!("\x1b[{key_code};{modifiers}{event_type}{associated_text}u").into_bytes())
}

fn kitty_named_key_bytes(
    modes: TerminalInputModes,
    modifiers: WindowInputModifiers,
    state: WindowInputElementState,
    repeat: bool,
    key: WindowInputNamedKey,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    let modifiers = kitty_modifier_parameter(modifiers);
    let event_type = kitty_event_type(modes, state, repeat);
    let associated_text = kitty_associated_text(modes, state, text);

    let bytes = match key {
        WindowInputNamedKey::Enter => {
            format!("\x1b[13;{modifiers}{event_type}{associated_text}u")
        }
        WindowInputNamedKey::Tab => {
            format!("\x1b[9;{modifiers}{event_type}{associated_text}u")
        }
        WindowInputNamedKey::Backspace => {
            format!("\x1b[127;{modifiers}{event_type}{associated_text}u")
        }
        WindowInputNamedKey::Escape => {
            format!("\x1b[27;{modifiers}{event_type}{associated_text}u")
        }
        WindowInputNamedKey::ArrowUp => format!("\x1b[1;{modifiers}{event_type}A"),
        WindowInputNamedKey::ArrowDown => format!("\x1b[1;{modifiers}{event_type}B"),
        WindowInputNamedKey::ArrowRight => format!("\x1b[1;{modifiers}{event_type}C"),
        WindowInputNamedKey::ArrowLeft => format!("\x1b[1;{modifiers}{event_type}D"),
        WindowInputNamedKey::Home => format!("\x1b[1;{modifiers}{event_type}H"),
        WindowInputNamedKey::End => format!("\x1b[1;{modifiers}{event_type}F"),
        WindowInputNamedKey::Insert => format!("\x1b[2;{modifiers}{event_type}~"),
        WindowInputNamedKey::Delete => format!("\x1b[3;{modifiers}{event_type}~"),
        WindowInputNamedKey::PageUp => format!("\x1b[5;{modifiers}{event_type}~"),
        WindowInputNamedKey::PageDown => format!("\x1b[6;{modifiers}{event_type}~"),
        WindowInputNamedKey::F1 => format!("\x1b[11;{modifiers}{event_type}~"),
        WindowInputNamedKey::F2 => format!("\x1b[12;{modifiers}{event_type}~"),
        WindowInputNamedKey::F3 => format!("\x1b[13;{modifiers}{event_type}~"),
        WindowInputNamedKey::F4 => format!("\x1b[14;{modifiers}{event_type}~"),
        WindowInputNamedKey::F5 => format!("\x1b[15;{modifiers}{event_type}~"),
        WindowInputNamedKey::F6 => format!("\x1b[17;{modifiers}{event_type}~"),
        WindowInputNamedKey::F7 => format!("\x1b[18;{modifiers}{event_type}~"),
        WindowInputNamedKey::F8 => format!("\x1b[19;{modifiers}{event_type}~"),
        WindowInputNamedKey::F9 => format!("\x1b[20;{modifiers}{event_type}~"),
        WindowInputNamedKey::F10 => format!("\x1b[21;{modifiers}{event_type}~"),
        WindowInputNamedKey::F11 => format!("\x1b[23;{modifiers}{event_type}~"),
        WindowInputNamedKey::F12 => format!("\x1b[24;{modifiers}{event_type}~"),
        WindowInputNamedKey::CapsLock
        | WindowInputNamedKey::ScrollLock
        | WindowInputNamedKey::NumLock
        | WindowInputNamedKey::PrintScreen
        | WindowInputNamedKey::Pause
        | WindowInputNamedKey::ContextMenu
        | WindowInputNamedKey::Shift
        | WindowInputNamedKey::Control
        | WindowInputNamedKey::Alt
        | WindowInputNamedKey::Super => {
            if !modes.kitty_report_all_keys_as_escape_codes() {
                return None;
            }
            let code = match key {
                WindowInputNamedKey::CapsLock => 57358,
                WindowInputNamedKey::ScrollLock => 57359,
                WindowInputNamedKey::NumLock => 57360,
                WindowInputNamedKey::PrintScreen => 57361,
                WindowInputNamedKey::Pause => 57362,
                WindowInputNamedKey::ContextMenu => 57363,
                WindowInputNamedKey::Shift => 57441,
                WindowInputNamedKey::Control => 57442,
                WindowInputNamedKey::Alt => 57443,
                WindowInputNamedKey::Super => 57444,
                _ => unreachable!(),
            };
            format!("\x1b[{code};{modifiers}{event_type}{associated_text}u")
        }
    };

    Some(bytes.into_bytes())
}

fn kitty_event_type(
    modes: TerminalInputModes,
    state: WindowInputElementState,
    repeat: bool,
) -> &'static str {
    if !modes.kitty_report_event_types() {
        return "";
    }
    match (state, repeat) {
        (WindowInputElementState::Released, _) => ":3",
        (WindowInputElementState::Pressed, true) => ":2",
        (WindowInputElementState::Pressed, false) => "",
    }
}

fn kitty_modifier_parameter(modifiers: WindowInputModifiers) -> u8 {
    1 + u8::from(modifiers.shift_key())
        + u8::from(modifiers.alt_key()) * 2
        + u8::from(modifiers.control_key()) * 4
        + u8::from(modifiers.super_key()) * 8
}

fn kitty_associated_text(
    modes: TerminalInputModes,
    state: WindowInputElementState,
    text: Option<&str>,
) -> String {
    if !modes.kitty_report_associated_text() || state == WindowInputElementState::Released {
        return String::new();
    }
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return String::new();
    };
    let codepoints = text
        .chars()
        .map(|character| u32::from(character).to_string())
        .collect::<Vec<_>>()
        .join(":");
    format!(";{codepoints}")
}

fn kitty_unshift_ascii(character: char) -> char {
    match character {
        'A'..='Z' => character.to_ascii_lowercase(),
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => character,
    }
}

fn legacy_character_bytes(
    modifiers: WindowInputModifiers,
    character: char,
    text: Option<&str>,
) -> Option<Vec<u8>> {
    let key = WindowInputKey::Character(character.to_string());
    if modifiers.control_key() {
        return ctrl_bytes_from_key(&key)
            .or_else(|| control_text_bytes(text))
            .map(|bytes| prefix_escape_if(bytes, modifiers.alt_key()));
    }
    if modifiers.alt_key() {
        return text_bytes(&key, text).map(|bytes| prefix_escape_if(bytes, true));
    }
    text_bytes(&key, text)
}

fn cursor_key_sequence(
    terminator: char,
    app_cursor: bool,
    modifiers: WindowInputModifiers,
) -> Vec<u8> {
    match keyboard_modifier_parameter(modifiers) {
        Some(modifier) => format!("\x1b[1;{modifier}{terminator}").into_bytes(),
        None if app_cursor => format!("\x1bO{terminator}").into_bytes(),
        None => format!("\x1b[{terminator}").into_bytes(),
    }
}

fn function_key_sequence(terminator: char, modifiers: WindowInputModifiers) -> Vec<u8> {
    match keyboard_modifier_parameter(modifiers) {
        Some(modifier) => format!("\x1b[1;{modifier}{terminator}").into_bytes(),
        None => format!("\x1bO{terminator}").into_bytes(),
    }
}

fn tilde_key_sequence(code: u8, modifiers: WindowInputModifiers) -> Vec<u8> {
    match keyboard_modifier_parameter(modifiers) {
        Some(modifier) => format!("\x1b[{code};{modifier}~").into_bytes(),
        None => format!("\x1b[{code}~").into_bytes(),
    }
}

fn keyboard_modifier_parameter(modifiers: WindowInputModifiers) -> Option<u8> {
    let bits = u8::from(modifiers.shift_key())
        + u8::from(modifiers.alt_key()) * 2
        + u8::from(modifiers.control_key()) * 4
        + u8::from(modifiers.super_key()) * 8;
    (bits != 0).then_some(bits + 1)
}

fn prefix_escape_if(mut bytes: Vec<u8>, prefix: bool) -> Vec<u8> {
    if prefix {
        bytes.insert(0, 0x1B);
    }
    bytes
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

fn control_text_bytes(text: Option<&str>) -> Option<Vec<u8>> {
    let bytes = text?.as_bytes();
    (bytes.len() == 1 && (bytes[0] <= 0x1F || bytes[0] == 0x7F)).then(|| bytes.to_vec())
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

    fn encoded_named_key(
        terminal_modes: TerminalInputModes,
        modifiers: WindowInputModifiers,
        key: WindowInputNamedKey,
    ) -> Vec<u8> {
        encode_key_event(
            terminal_modes,
            modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Named(key),
            None,
        )
        .unwrap()
    }

    fn kitty_modes(
        disambiguate: bool,
        event_types: bool,
        alternate_keys: bool,
        all_keys: bool,
        associated_text: bool,
    ) -> TerminalInputModes {
        TerminalInputModes::default().with_kitty_keyboard(
            disambiguate,
            event_types,
            alternate_keys,
            all_keys,
            associated_text,
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
        assert_eq!(
            encoded_named_key(
                modes(true, false, false, false, false, false),
                modifiers,
                WindowInputNamedKey::Home,
            ),
            b"\x1bOH",
        );
    }

    #[test]
    fn xterm_modifiers_are_encoded_for_navigation_keys() {
        let terminal_modes = modes(true, false, false, false, false, false);
        assert_eq!(
            encoded_named_key(
                terminal_modes,
                WindowInputModifiers::new(true, false, false, false),
                WindowInputNamedKey::ArrowLeft,
            ),
            b"\x1b[1;5D",
        );
        assert_eq!(
            encoded_named_key(
                terminal_modes,
                WindowInputModifiers::new(false, true, true, false),
                WindowInputNamedKey::End,
            ),
            b"\x1b[1;4F",
        );
        assert_eq!(
            encoded_named_key(
                terminal_modes,
                WindowInputModifiers::new(false, false, false, true),
                WindowInputNamedKey::PageDown,
            ),
            b"\x1b[6;9~",
        );
    }

    #[test]
    fn function_and_editing_keys_use_xterm_sequences() {
        let terminal_modes = modes(false, false, false, false, false, false);
        let none = WindowInputModifiers::new(false, false, false, false);
        let function_keys: &[(WindowInputNamedKey, &[u8])] = &[
            (WindowInputNamedKey::F1, b"\x1bOP"),
            (WindowInputNamedKey::F2, b"\x1bOQ"),
            (WindowInputNamedKey::F3, b"\x1bOR"),
            (WindowInputNamedKey::F4, b"\x1bOS"),
            (WindowInputNamedKey::F5, b"\x1b[15~"),
            (WindowInputNamedKey::F6, b"\x1b[17~"),
            (WindowInputNamedKey::F7, b"\x1b[18~"),
            (WindowInputNamedKey::F8, b"\x1b[19~"),
            (WindowInputNamedKey::F9, b"\x1b[20~"),
            (WindowInputNamedKey::F10, b"\x1b[21~"),
            (WindowInputNamedKey::F11, b"\x1b[23~"),
            (WindowInputNamedKey::F12, b"\x1b[24~"),
        ];
        for (key, expected) in function_keys {
            assert_eq!(encoded_named_key(terminal_modes, none, *key), *expected);
        }
        assert_eq!(
            encoded_named_key(
                terminal_modes,
                WindowInputModifiers::new(true, false, false, false),
                WindowInputNamedKey::F12,
            ),
            b"\x1b[24;5~",
        );
        assert_eq!(
            encoded_named_key(terminal_modes, none, WindowInputNamedKey::Insert),
            b"\x1b[2~",
        );
        assert_eq!(
            encoded_named_key(terminal_modes, none, WindowInputNamedKey::PageUp),
            b"\x1b[5~",
        );
    }

    #[test]
    fn shift_tab_and_alt_control_character_use_legacy_sequences() {
        let terminal_modes = modes(false, false, false, false, false, false);
        assert_eq!(
            encoded_named_key(
                terminal_modes,
                WindowInputModifiers::new(false, false, true, false),
                WindowInputNamedKey::Tab,
            ),
            b"\x1b[Z",
        );
        assert_eq!(
            encode_key_event(
                terminal_modes,
                WindowInputModifiers::new(true, true, false, false),
                WindowInputElementState::Pressed,
                &WindowInputKey::Character("a".to_string()),
                None,
            ),
            Some(vec![0x1B, 0x01]),
        );
    }

    #[test]
    fn ctrl_c_encodes_terminal_interrupt_even_without_a_logical_character() {
        let terminal_modes = modes(false, false, false, false, false, false);
        let control = WindowInputModifiers::new(true, false, false, false);

        assert_eq!(
            encode_key_event(
                terminal_modes,
                control,
                WindowInputElementState::Pressed,
                &WindowInputKey::Character("c".to_string()),
                None,
            ),
            Some(vec![0x03]),
        );
        assert_eq!(
            encode_key_event(
                terminal_modes,
                control,
                WindowInputElementState::Pressed,
                &WindowInputKey::Unidentified,
                Some("\x03"),
            ),
            Some(vec![0x03]),
        );
    }

    #[test]
    fn kitty_disambiguates_control_text_keys() {
        assert_eq!(
            encode_key_event_with_repeat(
                kitty_modes(true, false, false, false, false),
                WindowInputModifiers::new(true, false, false, false),
                WindowInputElementState::Pressed,
                false,
                &WindowInputKey::Character("i".into()),
                None,
            ),
            Some(b"\x1b[105;5u".to_vec())
        );
    }

    #[test]
    fn kitty_encodes_space_in_legacy_and_report_all_modes() {
        let space = WindowInputKey::Character(" ".into());
        let none = WindowInputModifiers::new(false, false, false, false);

        assert_eq!(
            encode_key_event_with_repeat(
                kitty_modes(true, false, false, false, false),
                none,
                WindowInputElementState::Pressed,
                false,
                &space,
                Some(" "),
            ),
            Some(b" ".to_vec())
        );
        assert_eq!(
            encode_key_event_with_repeat(
                kitty_modes(true, false, false, true, false),
                none,
                WindowInputElementState::Pressed,
                false,
                &space,
                Some(" "),
            ),
            Some(b"\x1b[32;1u".to_vec())
        );
    }

    #[test]
    fn kitty_reports_press_repeat_release_alternate_key_and_text() {
        let modes = kitty_modes(true, true, true, true, true);
        let modifiers = WindowInputModifiers::new(false, false, true, false);
        let key = WindowInputKey::Character("A".into());

        for (state, repeat, expected) in [
            (
                WindowInputElementState::Pressed,
                false,
                b"\x1b[97:65;2;65u".as_slice(),
            ),
            (
                WindowInputElementState::Pressed,
                true,
                b"\x1b[97:65;2:2;65u".as_slice(),
            ),
            (
                WindowInputElementState::Released,
                false,
                b"\x1b[97:65;2:3u".as_slice(),
            ),
        ] {
            assert_eq!(
                encode_key_event_with_repeat(modes, modifiers, state, repeat, &key, Some("A")),
                Some(expected.to_vec())
            );
        }
    }

    #[test]
    fn kitty_reports_named_key_events_and_suppresses_unsupported_releases() {
        let enter = WindowInputKey::Named(WindowInputNamedKey::Enter);
        let events = kitty_modes(true, true, false, false, false);
        let none = WindowInputModifiers::new(false, false, false, false);

        assert_eq!(
            encode_key_event_with_repeat(
                events,
                none,
                WindowInputElementState::Pressed,
                true,
                &enter,
                None,
            ),
            Some(b"\x1b[13;1:2u".to_vec())
        );
        assert_eq!(
            encode_key_event_with_repeat(
                events,
                none,
                WindowInputElementState::Released,
                false,
                &enter,
                None,
            ),
            Some(b"\x1b[13;1:3u".to_vec())
        );
        assert_eq!(
            encode_key_event_with_repeat(
                kitty_modes(true, false, false, false, false),
                none,
                WindowInputElementState::Released,
                false,
                &enter,
                None,
            ),
            None
        );
    }

    #[test]
    fn kitty_reports_extended_function_keys_only_when_requested() {
        let shift = WindowInputKey::Named(WindowInputNamedKey::Shift);
        let modifiers = WindowInputModifiers::new(false, false, true, false);

        assert_eq!(
            encode_key_event_with_repeat(
                kitty_modes(true, false, false, false, false),
                modifiers,
                WindowInputElementState::Pressed,
                false,
                &shift,
                None,
            ),
            None
        );
        assert_eq!(
            encode_key_event_with_repeat(
                kitty_modes(true, false, false, true, false),
                modifiers,
                WindowInputElementState::Pressed,
                false,
                &shift,
                None,
            ),
            Some(b"\x1b[57441;2u".to_vec())
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
    fn urxvt_mouse_encodes_decimal_click_drag_and_release_reports() {
        let size = TerminalPtySize::new(10, 20, 200, 100);
        let mut encoder = PtyMouseEncoder::new(size);
        let mouse_modes = TerminalInputModes::new(false, false, false, false, false, true, false)
            .with_urxvt_mouse(true);
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
            Some(b"\x1b[52;3;4M".to_vec())
        );
        assert_eq!(
            encoder.moved(
                mouse_modes,
                WindowPointerPosition::new(35.0, 45.0),
                modifiers,
            ),
            Some(b"\x1b[84;4;5M".to_vec())
        );
        assert_eq!(
            encoder.button(
                mouse_modes,
                WindowInputElementState::Released,
                WindowPointerButton::Primary,
                WindowPointerPosition::new(35.0, 45.0),
                modifiers,
            ),
            Some(b"\x1b[55;4;5M".to_vec())
        );
    }

    #[test]
    fn sgr_mouse_encoding_takes_precedence_over_urxvt_encoding() {
        let mut encoder = PtyMouseEncoder::new(TerminalPtySize::new(10, 20, 200, 100));
        let mouse_modes = modes(false, false, false, true, false, false).with_urxvt_mouse(true);

        assert_eq!(
            encoder.button(
                mouse_modes,
                WindowInputElementState::Pressed,
                WindowPointerButton::Primary,
                WindowPointerPosition::new(5.0, 5.0),
                WindowInputModifiers::new(false, false, false, false),
            ),
            Some(b"\x1b[<0;1;1M".to_vec())
        );
    }

    #[test]
    fn sgr_pixel_mouse_reports_physical_pixels_and_tracks_within_a_cell() {
        let size = TerminalPtySize::new(10, 20, 200, 100);
        let mut encoder = PtyMouseEncoder::new(size);
        let mouse_modes = TerminalInputModes::new(false, false, false, true, false, true, false)
            .with_urxvt_mouse(true)
            .with_sgr_pixel_mouse(true);

        assert_eq!(
            encoder.button(
                mouse_modes,
                WindowInputElementState::Pressed,
                WindowPointerButton::Primary,
                WindowPointerPosition::new(25.25, 35.25),
                WindowInputModifiers::new(false, false, false, false),
            ),
            Some(b"\x1b[<0;26;36M".to_vec())
        );
        assert_eq!(
            encoder.moved(
                mouse_modes,
                WindowPointerPosition::new(26.25, 35.75),
                WindowInputModifiers::new(false, false, false, false),
            ),
            Some(b"\x1b[<32;27;36M".to_vec())
        );
        assert_eq!(
            encoder.button(
                mouse_modes,
                WindowInputElementState::Released,
                WindowPointerButton::Primary,
                WindowPointerPosition::new(199.9, 99.9),
                WindowInputModifiers::new(false, false, false, false),
            ),
            Some(b"\x1b[<0;200;100m".to_vec())
        );
    }

    #[test]
    fn sgr_mouse_accumulates_pixel_scroll_into_wheel_reports() {
        let mut encoder = PtyMouseEncoder::new(TerminalPtySize::new(10, 20, 200, 100));
        let mouse_modes = modes(false, false, false, true, false, false);
        let modifiers = WindowInputModifiers::new(false, false, false, false);
        let position = WindowPointerPosition::new(5.0, 5.0);

        assert_eq!(
            encoder.scroll(
                mouse_modes,
                WindowScrollDelta::Pixels { x: 0.0, y: 5.0 },
                position,
                modifiers,
            ),
            PtyScrollAction::ReportToPty(Vec::new())
        );
        assert_eq!(
            encoder.scroll(
                mouse_modes,
                WindowScrollDelta::Pixels { x: 0.0, y: 5.0 },
                position,
                modifiers,
            ),
            PtyScrollAction::ReportToPty(vec![b"\x1b[<64;1;1M".to_vec()])
        );
    }

    #[test]
    fn scroll_without_mouse_reporting_moves_the_host_display() {
        let mut encoder = PtyMouseEncoder::new(TerminalPtySize::new(10, 20, 200, 100));
        let modifiers = WindowInputModifiers::new(false, false, false, false);
        let position = WindowPointerPosition::new(5.0, 5.0);

        assert_eq!(
            encoder.scroll(
                TerminalInputModes::default(),
                WindowScrollDelta::Lines { x: 3.0, y: 2.0 },
                position,
                modifiers,
            ),
            PtyScrollAction::ScrollDisplay(2)
        );
    }
}
