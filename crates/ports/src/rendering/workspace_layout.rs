use crate::{
    pty_host::window_size::TerminalWindowSize, rendering::render_target_id::RenderTargetId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderSurfacePlacement {
    pub target_id: RenderTargetId,
    pub x_px: u32,
    pub y_px: u32,
    pub width_px: u32,
    pub height_px: u32,
}

impl RenderSurfacePlacement {
    pub const fn new(
        target_id: RenderTargetId,
        x_px: u32,
        y_px: u32,
        width_px: u32,
        height_px: u32,
    ) -> Self {
        Self {
            target_id,
            x_px,
            y_px,
            width_px,
            height_px,
        }
    }

    pub const fn window_size(self) -> TerminalWindowSize {
        TerminalWindowSize::new(self.width_px, self.height_px)
    }
}
