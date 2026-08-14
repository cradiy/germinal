#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursorStyle {
    pub shape: TerminalCursorShape,
    pub blinking: bool,
}

impl TerminalCursorStyle {
    pub const fn new(shape: TerminalCursorShape, blinking: bool) -> Self {
        Self { shape, blinking }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCursorShape {
    #[default]
    Block,
    Underline,
    Beam,
}
