#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalScaleFactor {
	value: f64,
}

impl TerminalScaleFactor {
	pub const DEFAULT: Self = Self { value: 1.0 };

	pub fn new(value: f64) -> Self { Self { value: value.max(0.1) } }

	pub fn value(self) -> f64 { self.value }

	pub fn physical_px_from_logical_px(self, logical_px: u32) -> u32 {
		((logical_px as f64) * self.value).round().max(0.0) as u32
	}
}
