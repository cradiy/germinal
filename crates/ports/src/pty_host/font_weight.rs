#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalFontWeight {
    Normal,
    Medium,
    Semibold,
    Bold,
}

impl TerminalFontWeight {
    pub const DEFAULT_BOLD: Self = Self::Medium;
}
