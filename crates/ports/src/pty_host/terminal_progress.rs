#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalProgress {
    Normal(u8),
    Error(u8),
    Indeterminate,
    Warning(u8),
}

impl TerminalProgress {
    pub const fn percentage(self) -> Option<u8> {
        match self {
            Self::Normal(percentage) | Self::Error(percentage) | Self::Warning(percentage) => {
                Some(percentage)
            }
            Self::Indeterminate => None,
        }
    }
}
