use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

const APP_CURSOR: u32 = 1 << 0;
const BRACKETED_PASTE: u32 = 1 << 1;
const FOCUS_IN_OUT: u32 = 1 << 2;
const SGR_MOUSE: u32 = 1 << 3;
const MOUSE_REPORT_CLICK: u32 = 1 << 4;
const MOUSE_DRAG: u32 = 1 << 5;
const MOUSE_MOTION: u32 = 1 << 6;
const KITTY_DISAMBIGUATE_ESC_CODES: u32 = 1 << 7;
const KITTY_REPORT_EVENT_TYPES: u32 = 1 << 8;
const KITTY_REPORT_ALTERNATE_KEYS: u32 = 1 << 9;
const KITTY_REPORT_ALL_KEYS_AS_ESC: u32 = 1 << 10;
const KITTY_REPORT_ASSOCIATED_TEXT: u32 = 1 << 11;
const URXVT_MOUSE: u32 = 1 << 12;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerminalInputModes(u32);

impl TerminalInputModes {
    pub const fn new(
        app_cursor: bool,
        bracketed_paste: bool,
        focus_in_out: bool,
        sgr_mouse: bool,
        mouse_report_click: bool,
        mouse_drag: bool,
        mouse_motion: bool,
    ) -> Self {
        let mut bits = 0;
        if app_cursor {
            bits |= APP_CURSOR;
        }
        if bracketed_paste {
            bits |= BRACKETED_PASTE;
        }
        if focus_in_out {
            bits |= FOCUS_IN_OUT;
        }
        if sgr_mouse {
            bits |= SGR_MOUSE;
        }
        if mouse_report_click {
            bits |= MOUSE_REPORT_CLICK;
        }
        if mouse_drag {
            bits |= MOUSE_DRAG;
        }
        if mouse_motion {
            bits |= MOUSE_MOTION;
        }
        Self(bits)
    }

    pub const fn app_cursor(self) -> bool {
        self.0 & APP_CURSOR != 0
    }

    pub const fn bracketed_paste(self) -> bool {
        self.0 & BRACKETED_PASTE != 0
    }

    pub const fn focus_in_out(self) -> bool {
        self.0 & FOCUS_IN_OUT != 0
    }

    pub const fn sgr_mouse(self) -> bool {
        self.0 & SGR_MOUSE != 0
    }

    pub const fn urxvt_mouse(self) -> bool {
        self.0 & URXVT_MOUSE != 0
    }

    pub const fn with_urxvt_mouse(mut self, enabled: bool) -> Self {
        if enabled {
            self.0 |= URXVT_MOUSE;
        } else {
            self.0 &= !URXVT_MOUSE;
        }
        self
    }

    pub const fn mouse_report_click(self) -> bool {
        self.0 & MOUSE_REPORT_CLICK != 0
    }

    pub const fn mouse_drag(self) -> bool {
        self.0 & MOUSE_DRAG != 0
    }

    pub const fn mouse_motion(self) -> bool {
        self.0 & MOUSE_MOTION != 0
    }

    pub const fn mouse_tracking(self) -> bool {
        self.mouse_report_click() || self.mouse_drag() || self.mouse_motion()
    }

    pub const fn with_kitty_keyboard(
        mut self,
        disambiguate_esc_codes: bool,
        report_event_types: bool,
        report_alternate_keys: bool,
        report_all_keys_as_escape_codes: bool,
        report_associated_text: bool,
    ) -> Self {
        if disambiguate_esc_codes {
            self.0 |= KITTY_DISAMBIGUATE_ESC_CODES;
        }
        if report_event_types {
            self.0 |= KITTY_REPORT_EVENT_TYPES;
        }
        if report_alternate_keys {
            self.0 |= KITTY_REPORT_ALTERNATE_KEYS;
        }
        if report_all_keys_as_escape_codes {
            self.0 |= KITTY_REPORT_ALL_KEYS_AS_ESC;
        }
        if report_associated_text {
            self.0 |= KITTY_REPORT_ASSOCIATED_TEXT;
        }
        self
    }

    pub const fn kitty_keyboard(self) -> bool {
        self.0
            & (KITTY_DISAMBIGUATE_ESC_CODES
                | KITTY_REPORT_EVENT_TYPES
                | KITTY_REPORT_ALTERNATE_KEYS
                | KITTY_REPORT_ALL_KEYS_AS_ESC
                | KITTY_REPORT_ASSOCIATED_TEXT)
            != 0
    }

    pub const fn kitty_disambiguate_esc_codes(self) -> bool {
        self.0 & KITTY_DISAMBIGUATE_ESC_CODES != 0
    }

    pub const fn kitty_report_event_types(self) -> bool {
        self.0 & KITTY_REPORT_EVENT_TYPES != 0
    }

    pub const fn kitty_report_alternate_keys(self) -> bool {
        self.0 & KITTY_REPORT_ALTERNATE_KEYS != 0
    }

    pub const fn kitty_report_all_keys_as_escape_codes(self) -> bool {
        self.0 & KITTY_REPORT_ALL_KEYS_AS_ESC != 0
    }

    pub const fn kitty_report_associated_text(self) -> bool {
        self.0 & KITTY_REPORT_ASSOCIATED_TEXT != 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct TerminalInputModeState {
    bits: Arc<AtomicU32>,
}

impl TerminalInputModeState {
    pub fn load(&self) -> TerminalInputModes {
        TerminalInputModes(self.bits.load(Ordering::Acquire))
    }

    pub fn store(&self, modes: TerminalInputModes) {
        self.bits.store(modes.0, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::{TerminalInputModeState, TerminalInputModes};

    #[test]
    fn shared_state_publishes_input_modes() {
        let state = TerminalInputModeState::default();
        let reader = state.clone();
        let modes = TerminalInputModes::new(true, true, true, true, true, false, false)
            .with_urxvt_mouse(true)
            .with_kitty_keyboard(true, true, true, true, true);

        state.store(modes);

        assert_eq!(reader.load(), modes);
        assert!(reader.load().mouse_tracking());
        assert!(reader.load().urxvt_mouse());
        assert!(reader.load().kitty_keyboard());
        assert!(reader.load().kitty_disambiguate_esc_codes());
        assert!(reader.load().kitty_report_event_types());
        assert!(reader.load().kitty_report_alternate_keys());
        assert!(reader.load().kitty_report_all_keys_as_escape_codes());
        assert!(reader.load().kitty_report_associated_text());
    }
}
