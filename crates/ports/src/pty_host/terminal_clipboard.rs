#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalClipboard {
    Clipboard,
    Selection,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalOsc52Mode {
    Disabled,
    #[default]
    OnlyCopy,
    OnlyPaste,
    CopyPaste,
}
