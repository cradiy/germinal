use germinal_domain::pty_host::terminal_size::TerminalGridSize;

use crate::pty_host::{
	cell_size::TerminalCellSize,
	content_size::TerminalContentSize,
	render_viewport::TerminalRenderViewport,
	scale_factor::TerminalScaleFactor,
	terminal_size::{TerminalPtySize, terminal_grid_size_from_content_pixels},
	window_size::TerminalWindowSize,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPadding {
	x_px: u32,
	y_px: u32,
}

impl TerminalPadding {
	pub const ZERO: Self = Self { x_px: 0, y_px: 0 };

	pub const fn new(x_px: u32, y_px: u32) -> Self { Self { x_px, y_px } }

	pub const fn x_px(self) -> u32 { self.x_px }

	pub const fn y_px(self) -> u32 { self.y_px }

	pub const fn add(self, other: Self) -> Self {
		Self { x_px: self.x_px.saturating_add(other.x_px), y_px: self.y_px.saturating_add(other.y_px) }
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSizeConfig {
	cell_size:       TerminalCellSize,
	padding:         TerminalPadding,
	dynamic_padding: bool,
}

impl TerminalSizeConfig {
	pub const DEFAULT: Self = Self {
		cell_size:       TerminalCellSize::new(12, 24),
		padding:         TerminalPadding::ZERO,
		dynamic_padding: false,
	};

	pub const fn new(
		cell_size: TerminalCellSize,
		padding: TerminalPadding,
		dynamic_padding: bool,
	) -> Self {
		Self { cell_size, padding, dynamic_padding }
	}

	pub const fn cell_size(self) -> TerminalCellSize { self.cell_size }

	pub fn with_cell_size(self, cell_size: TerminalCellSize) -> Self { Self { cell_size, ..self } }

	pub const fn padding(self) -> TerminalPadding { self.padding }

	pub const fn dynamic_padding(self) -> bool { self.dynamic_padding }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSizeInfo {
	window_size:        TerminalWindowSize,
	cell_size:          TerminalCellSize,
	configured_padding: TerminalPadding,
	dynamic_padding:    TerminalPadding,
	total_padding:      TerminalPadding,
	content_size:       TerminalContentSize,
	grid_size:          TerminalGridSize,
	pty_size:           TerminalPtySize,
}

impl TerminalSizeInfo {
	pub fn new(
		window_size: TerminalWindowSize,
		cell_size: TerminalCellSize,
		padding: TerminalPadding,
	) -> Self {
		Self::from_config(window_size, TerminalSizeConfig::new(cell_size, padding, false))
	}

	pub fn from_config(window_size: TerminalWindowSize, config: TerminalSizeConfig) -> Self {
		Self::from_scaled_config(window_size, config, TerminalScaleFactor::DEFAULT)
	}

	pub fn from_scaled_config(
		window_size: TerminalWindowSize,
		config: TerminalSizeConfig,
		scale_factor: TerminalScaleFactor,
	) -> Self {
		let configured_padding = TerminalPadding::new(
			scale_factor.physical_px_from_logical_px(config.padding().x_px()),
			scale_factor.physical_px_from_logical_px(config.padding().y_px()),
		);
		let initial_content_width_px =
			content_axis_px(window_size.width_px(), configured_padding.x_px());
		let initial_content_height_px =
			content_axis_px(window_size.height_px(), configured_padding.y_px());
		let initial_content_size =
			TerminalContentSize::new(initial_content_width_px, initial_content_height_px);
		let grid_size =
			terminal_grid_size_from_content_pixels(initial_content_size, config.cell_size());

		let dynamic_padding = if config.dynamic_padding() {
			dynamic_padding_for_grid(initial_content_size, config.cell_size(), grid_size)
		} else {
			TerminalPadding::ZERO
		};

		let total_padding = configured_padding.add(dynamic_padding);
		let content_width_px = content_axis_px(window_size.width_px(), total_padding.x_px());
		let content_height_px = content_axis_px(window_size.height_px(), total_padding.y_px());
		let content_size = TerminalContentSize::new(content_width_px, content_height_px);
		let grid_size = terminal_grid_size_from_content_pixels(content_size, config.cell_size());

		// Keep the pixel dimensions identical to the drawable content size used for
		// the grid. This mirrors the single source of truth used by Alacritty's
		// SizeInfo while allowing the cell metrics to be resolved before layout.
		let pty_size = TerminalPtySize::from_content_pixels(content_size, config.cell_size());

		let size_info = Self {
			window_size,
			cell_size: config.cell_size(),
			configured_padding,
			dynamic_padding,
			total_padding,
			content_size,
			grid_size,
			pty_size,
		};
		size_info.debug_assert_consistent();

		size_info
	}

	pub fn zero_padding(window_size: TerminalWindowSize, cell_size: TerminalCellSize) -> Self {
		Self::new(window_size, cell_size, TerminalPadding::ZERO)
	}

	pub const fn window_size(self) -> TerminalWindowSize { self.window_size }

	pub const fn cell_size(self) -> TerminalCellSize { self.cell_size }

	pub const fn padding(self) -> TerminalPadding { self.total_padding }

	pub const fn configured_padding(self) -> TerminalPadding { self.configured_padding }

	pub const fn dynamic_padding(self) -> TerminalPadding { self.dynamic_padding }

	pub const fn content_size(self) -> TerminalContentSize { self.content_size }

	pub const fn grid_size(self) -> TerminalGridSize { self.grid_size }

	pub const fn pty_size(self) -> TerminalPtySize { self.pty_size }

	pub fn content_width_px(self) -> u32 { self.content_size.width_px() }

	pub fn content_height_px(self) -> u32 { self.content_size.height_px() }

	pub fn render_viewport(self) -> TerminalRenderViewport {
		TerminalRenderViewport::new(self.cell_size, self.total_padding, self.grid_size)
	}

	pub fn grid_width_px(self) -> u32 {
		(self.grid_size.columns() as u32).saturating_mul(self.cell_size.width_px())
	}

	pub fn grid_height_px(self) -> u32 {
		(self.grid_size.rows() as u32).saturating_mul(self.cell_size.height_px())
	}

	pub fn is_consistent(self) -> bool {
		let expected_grid_size =
			terminal_grid_size_from_content_pixels(self.content_size, self.cell_size);
		let expected_pty_size = TerminalPtySize::from_content_pixels(self.content_size, self.cell_size);
		let viewport = self.render_viewport();

		self.grid_size == expected_grid_size
			&& self.pty_size == expected_pty_size
			&& (self.pty_size.rows() as usize) == self.grid_size.rows()
			&& (self.pty_size.columns() as usize) == self.grid_size.columns()
			&& viewport.grid_size() == self.grid_size
			&& viewport.cell_size() == self.cell_size
			&& viewport.grid_width_px() == self.grid_width_px()
			&& viewport.grid_height_px() == self.grid_height_px()
	}

	pub fn debug_assert_consistent(self) {
		debug_assert!(self.is_consistent(), "TerminalSizeInfo derived values are inconsistent");
	}
}

fn dynamic_padding_for_grid(
	content_size: TerminalContentSize,
	cell_size: TerminalCellSize,
	grid_size: TerminalGridSize,
) -> TerminalPadding {
	let used_width = (grid_size.columns() as u32).saturating_mul(cell_size.width_px());
	let used_height = (grid_size.rows() as u32).saturating_mul(cell_size.height_px());
	let extra_x = content_size.width_px().saturating_sub(used_width) / 2;
	let extra_y = content_size.height_px().saturating_sub(used_height) / 2;

	TerminalPadding::new(extra_x, extra_y)
}

fn content_axis_px(axis_px: u32, padding_px: u32) -> u32 {
	axis_px.saturating_sub(padding_px.saturating_mul(2)).max(1)
}
