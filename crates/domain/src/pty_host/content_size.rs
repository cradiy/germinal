#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalContentSize {
	width_px:  u32,
	height_px: u32,
}

impl TerminalContentSize {
	pub const fn new(width_px: u32, height_px: u32) -> Self { Self { width_px, height_px } }

	pub const fn width_px(self) -> u32 { self.width_px }

	pub const fn height_px(self) -> u32 { self.height_px }

	pub const fn is_empty(self) -> bool { self.width_px == 0 || self.height_px == 0 }
}
