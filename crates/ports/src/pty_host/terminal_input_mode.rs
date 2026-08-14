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
        let modes = TerminalInputModes::new(true, true, true, true, true, false, false);

        state.store(modes);

        assert_eq!(reader.load(), modes);
        assert!(reader.load().mouse_tracking());
    }
}
