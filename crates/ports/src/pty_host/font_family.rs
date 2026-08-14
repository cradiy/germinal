#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFontFamily {
    name: String,
}

impl TerminalFontFamily {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Default for TerminalFontFamily {
    fn default() -> Self {
        Self::new(platform_default_terminal_font_family())
    }
}

#[cfg(windows)]
const fn platform_default_terminal_font_family() -> &'static str {
    "Consolas"
}

#[cfg(target_os = "macos")]
const fn platform_default_terminal_font_family() -> &'static str {
    "Menlo"
}

#[cfg(all(unix, not(target_os = "macos")))]
const fn platform_default_terminal_font_family() -> &'static str {
    "monospace"
}

#[cfg(not(any(unix, windows)))]
const fn platform_default_terminal_font_family() -> &'static str {
    "monospace"
}
