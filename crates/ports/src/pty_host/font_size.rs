#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalFontSize {
    logical_px: f32,
}

impl TerminalFontSize {
    pub const DEFAULT: Self = Self::new(16.0);

    pub const fn new(logical_px: f32) -> Self {
        Self { logical_px }
    }

    pub const fn logical_px(self) -> f32 {
        self.logical_px
    }

    pub fn physical_px(self, scale_factor: f64) -> f32 {
        let scale = scale_factor.max(0.1) as f32;
        (self.logical_px * scale).max(1.0)
    }
}
