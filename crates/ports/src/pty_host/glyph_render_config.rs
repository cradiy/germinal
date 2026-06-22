use crate::pty_host::{
	cell_size::TerminalCellSize, font_config::TerminalFontConfig, font_weight::TerminalFontWeight,
	scale_factor::TerminalScaleFactor, size_info::TerminalSizeInfo,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerminalGlyphRenderConfig {
	font_config:  TerminalFontConfig,
	cell_size:    TerminalCellSize,
	font_size_px: f32,
}

impl TerminalGlyphRenderConfig {
	pub fn new(
		font_config: TerminalFontConfig,
		cell_size: TerminalCellSize,
		scale_factor: TerminalScaleFactor,
	) -> Self {
		Self { font_size_px: font_config.physical_px(scale_factor), font_config, cell_size }
	}

	pub fn from_size_info(
		font_config: TerminalFontConfig,
		size_info: TerminalSizeInfo,
		scale_factor: TerminalScaleFactor,
	) -> Self {
		Self::new(font_config, size_info.cell_size(), scale_factor)
	}

	pub const fn font_config(self) -> TerminalFontConfig { self.font_config }

	pub const fn cell_size(self) -> TerminalCellSize { self.cell_size }

	pub const fn font_family_name(self) -> &'static str { self.font_config.family().name() }

	pub fn font_size_px(self) -> f32 { self.font_size_px }

	pub const fn bold_font_weight(self) -> TerminalFontWeight { self.font_config.bold_weight() }
}
