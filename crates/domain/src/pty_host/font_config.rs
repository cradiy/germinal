use crate::pty_host::{
	font_family::TerminalFontFamily, font_size::TerminalFontSize, scale_factor::TerminalScaleFactor,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalFontConfig {
	family: TerminalFontFamily,
	size:   TerminalFontSize,
}

impl TerminalFontConfig {
	pub const DEFAULT: Self =
		Self { family: TerminalFontFamily::DEFAULT, size: TerminalFontSize::DEFAULT };

	pub const fn new(family: TerminalFontFamily, size: TerminalFontSize) -> Self {
		Self { family, size }
	}

	pub const fn family(self) -> TerminalFontFamily { self.family }

	pub const fn size(self) -> TerminalFontSize { self.size }

	pub fn physical_px(self, scale_factor: TerminalScaleFactor) -> f32 {
		self.size.physical_px(scale_factor.value())
	}
}
