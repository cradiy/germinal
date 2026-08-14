#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowInputElementState {
    Pressed,
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowInputModifiers {
    control: bool,
    alt: bool,
    shift: bool,
    super_key: bool,
}

impl WindowInputModifiers {
    pub fn new(control: bool, alt: bool, shift: bool, super_key: bool) -> Self {
        Self {
            control,
            alt,
            shift,
            super_key,
        }
    }

    pub fn control_key(&self) -> bool {
        self.control
    }

    pub fn alt_key(&self) -> bool {
        self.alt
    }

    pub fn shift_key(&self) -> bool {
        self.shift
    }

    pub fn super_key(&self) -> bool {
        self.super_key
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPointerPosition {
    pub x_px: f64,
    pub y_px: f64,
}

impl WindowPointerPosition {
    pub const fn new(x_px: f64, y_px: f64) -> Self {
        Self { x_px, y_px }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowPointerButton {
    Primary,
    Secondary,
    Middle,
    Back,
    Forward,
    Other(u16),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowScrollDelta {
    Lines { x: f32, y: f32 },
    Pixels { x: f64, y: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowInputNamedKey {
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
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
    Insert,
    Delete,
    PageUp,
    PageDown,
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    ContextMenu,
    Shift,
    Control,
    Alt,
    Super,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowInputKey {
    Named(WindowInputNamedKey),
    Character(String),
    Unidentified,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowInputEvent {
    ModifiersChanged(WindowInputModifiers),
    FocusChanged(bool),
    Key {
        state: WindowInputElementState,
        repeat: bool,
        logical_key: WindowInputKey,
        text: Option<String>,
    },
    Ime(String),
    Paste(String),
    PointerMoved {
        position: WindowPointerPosition,
        modifiers: WindowInputModifiers,
    },
    PointerLeft,
    PointerButton {
        state: WindowInputElementState,
        button: WindowPointerButton,
        position: WindowPointerPosition,
        modifiers: WindowInputModifiers,
    },
    Scroll {
        delta: WindowScrollDelta,
        position: WindowPointerPosition,
        modifiers: WindowInputModifiers,
    },
}
