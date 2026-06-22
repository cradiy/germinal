use germinal_domain::pty_host::terminal_size::TerminalGridSize;
use serde::{Deserialize, Serialize};

use crate::pty_host::{cell_size::TerminalCellSize, content_size::TerminalContentSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalPtySize {
	rows:         u16,
	columns:      u16,
	pixel_width:  u16,
	pixel_height: u16,
}

impl TerminalPtySize {
	pub const fn new(rows: u16, columns: u16, pixel_width: u16, pixel_height: u16) -> Self {
		Self { rows, columns, pixel_width, pixel_height }
	}

	pub const fn rows(self) -> u16 { self.rows }

	pub const fn columns(self) -> u16 { self.columns }

	pub const fn pixel_width(self) -> u16 { self.pixel_width }

	pub const fn pixel_height(self) -> u16 { self.pixel_height }

	pub fn from_content_pixels(content: TerminalContentSize, cell: TerminalCellSize) -> Self {
		let grid = terminal_grid_size_from_content_pixels(content, cell);

		Self {
			rows:         u16_saturating(grid.rows() as u32),
			columns:      u16_saturating(grid.columns() as u32),
			pixel_width:  u16_saturating(content.width_px()),
			pixel_height: u16_saturating(content.height_px()),
		}
	}
}

pub fn terminal_grid_size_from_content_pixels(
	content: TerminalContentSize,
	cell: TerminalCellSize,
) -> TerminalGridSize {
	let columns = (content.width_px() / cell.width_px()).max(1) as usize;
	let rows = (content.height_px() / cell.height_px()).max(1) as usize;

	TerminalGridSize::new(columns, rows)
}

fn u16_saturating(value: u32) -> u16 { value.min(u16::MAX as u32) as u16 }
