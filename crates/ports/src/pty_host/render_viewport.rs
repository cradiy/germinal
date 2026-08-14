use germinal_domain::pty_host::terminal_size::TerminalGridSize;

use crate::pty_host::{cell_size::TerminalCellSize, size_info::TerminalPadding};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalGridPixelRect {
    x_px: u32,
    y_px: u32,
    width_px: u32,
    height_px: u32,
}

impl TerminalGridPixelRect {
    pub const fn new(x_px: u32, y_px: u32, width_px: u32, height_px: u32) -> Self {
        Self {
            x_px,
            y_px,
            width_px,
            height_px,
        }
    }

    pub const fn x_px(self) -> u32 {
        self.x_px
    }

    pub const fn y_px(self) -> u32 {
        self.y_px
    }

    pub const fn width_px(self) -> u32 {
        self.width_px
    }

    pub const fn height_px(self) -> u32 {
        self.height_px
    }

    pub fn right_px(self) -> u32 {
        self.x_px.saturating_add(self.width_px)
    }

    pub fn bottom_px(self) -> u32 {
        self.y_px.saturating_add(self.height_px)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalRenderViewport {
    cell_size: TerminalCellSize,
    origin: TerminalPadding,
    grid_size: TerminalGridSize,
}

impl TerminalRenderViewport {
    pub const fn new(
        cell_size: TerminalCellSize,
        origin: TerminalPadding,
        grid_size: TerminalGridSize,
    ) -> Self {
        Self {
            cell_size,
            origin,
            grid_size,
        }
    }

    pub const fn cell_size(self) -> TerminalCellSize {
        self.cell_size
    }

    pub const fn origin(self) -> TerminalPadding {
        self.origin
    }

    pub const fn grid_size(self) -> TerminalGridSize {
        self.grid_size
    }

    pub const fn origin_x_px(self) -> u32 {
        self.origin.x_px()
    }

    pub const fn origin_y_px(self) -> u32 {
        self.origin.y_px()
    }

    pub const fn columns(self) -> usize {
        self.grid_size.columns()
    }

    pub const fn rows(self) -> usize {
        self.grid_size.rows()
    }

    pub fn grid_width_px(self) -> u32 {
        (self.grid_size.columns() as u32).saturating_mul(self.cell_size.width_px())
    }

    pub fn grid_height_px(self) -> u32 {
        (self.grid_size.rows() as u32).saturating_mul(self.cell_size.height_px())
    }

    pub fn grid_rect(self) -> TerminalGridPixelRect {
        TerminalGridPixelRect::new(
            self.origin_x_px(),
            self.origin_y_px(),
            self.grid_width_px(),
            self.grid_height_px(),
        )
    }
}
