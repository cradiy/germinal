use crate::pty_host::{scale_factor::TerminalScaleFactor, window_size::TerminalWindowSize};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalWindowMetrics {
    window_size: TerminalWindowSize,
    scale_factor: TerminalScaleFactor,
}

impl TerminalWindowMetrics {
    pub const fn new(window_size: TerminalWindowSize, scale_factor: TerminalScaleFactor) -> Self {
        Self {
            window_size,
            scale_factor,
        }
    }

    pub fn from_physical_size(width_px: u32, height_px: u32, scale_factor: f64) -> Self {
        Self::new(
            TerminalWindowSize::new(width_px, height_px),
            TerminalScaleFactor::new(scale_factor),
        )
    }

    pub const fn window_size(self) -> TerminalWindowSize {
        self.window_size
    }

    pub const fn scale_factor(self) -> TerminalScaleFactor {
        self.scale_factor
    }
}
