#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalFontSize {
	point_size: f32,
}

impl TerminalFontSize {
	pub const DEFAULT: Self = Self::from_points(16.0);

	pub const fn from_points(point_size: f32) -> Self { Self { point_size } }

	pub const fn point_size(self) -> f32 { self.point_size }

	pub fn physical_px(self, scale_factor: f64) -> f32 {
		let scale = scale_factor.max(0.1) as f32;
		(self.point_size * scale).max(1.0)
	}
}
