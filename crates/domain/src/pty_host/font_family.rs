#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalFontFamily {
	name: &'static str,
}

impl TerminalFontFamily {
	pub const DEFAULT: Self = Self { name: platform_default_terminal_font_family() };

	pub const fn new(name: &'static str) -> Self { Self { name } }

	pub const fn name(self) -> &'static str { self.name }
}

#[cfg(windows)]
const fn platform_default_terminal_font_family() -> &'static str { "Consolas" }

#[cfg(target_os = "macos")]
const fn platform_default_terminal_font_family() -> &'static str { "Menlo" }

#[cfg(all(unix, not(target_os = "macos")))]
const fn platform_default_terminal_font_family() -> &'static str { "monospace" }

#[cfg(not(any(unix, windows)))]
const fn platform_default_terminal_font_family() -> &'static str { "monospace" }
