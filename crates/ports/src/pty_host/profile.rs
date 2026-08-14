use crate::pty_host::{
    cell_size::TerminalCellSize,
    font_config::TerminalFontConfig,
    font_family::TerminalFontFamily,
    font_size::TerminalFontSize,
    font_weight::TerminalFontWeight,
    glyph_render_config::TerminalGlyphRenderConfig,
    scale_factor::TerminalScaleFactor,
    size_info::{TerminalSizeConfig, TerminalSizeInfo},
    window_metrics::TerminalWindowMetrics,
    window_size::TerminalWindowSize,
};

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalProfile {
    font_config: TerminalFontConfig,
    size_config: TerminalSizeConfig,
}

impl TerminalProfile {
    pub fn new(
        font_family: TerminalFontFamily,
        font_size: TerminalFontSize,
        size_config: TerminalSizeConfig,
    ) -> Self {
        Self {
            font_config: TerminalFontConfig::new(font_family, font_size),
            size_config,
        }
    }

    pub fn font_config(&self) -> &TerminalFontConfig {
        &self.font_config
    }

    pub fn font_family(&self) -> &TerminalFontFamily {
        self.font_config.family()
    }

    pub const fn font_size(&self) -> TerminalFontSize {
        self.font_config.size()
    }

    pub const fn bold_font_weight(&self) -> TerminalFontWeight {
        self.font_config.bold_weight()
    }

    pub const fn size_config(&self) -> TerminalSizeConfig {
        self.size_config
    }

    pub fn with_cell_size(self, cell_size: TerminalCellSize) -> Self {
        Self {
            size_config: self.size_config.with_cell_size(cell_size),
            ..self
        }
    }

    pub fn with_bold_font_weight(self, bold_weight: TerminalFontWeight) -> Self {
        Self {
            font_config: self.font_config.with_bold_weight(bold_weight),
            ..self
        }
    }

    pub fn size_info(
        &self,
        window_size: TerminalWindowSize,
        scale_factor: TerminalScaleFactor,
    ) -> TerminalSizeInfo {
        self.size_info_for_window_metrics(TerminalWindowMetrics::new(window_size, scale_factor))
    }

    pub fn size_info_for_window_metrics(&self, metrics: TerminalWindowMetrics) -> TerminalSizeInfo {
        TerminalSizeInfo::from_scaled_config(
            metrics.window_size(),
            self.size_config,
            metrics.scale_factor(),
        )
    }

    pub fn glyph_render_config(
        &self,
        size_info: TerminalSizeInfo,
        scale_factor: TerminalScaleFactor,
    ) -> TerminalGlyphRenderConfig {
        TerminalGlyphRenderConfig::from_size_info(self.font_config.clone(), size_info, scale_factor)
    }

    pub fn font_physical_px(&self, scale_factor: TerminalScaleFactor) -> f32 {
        self.font_config.physical_px(scale_factor)
    }
}

impl Default for TerminalProfile {
    fn default() -> Self {
        Self {
            font_config: TerminalFontConfig::default(),
            size_config: TerminalSizeConfig::DEFAULT,
        }
    }
}
