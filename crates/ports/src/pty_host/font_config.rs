use crate::pty_host::{
    font_family::TerminalFontFamily, font_size::TerminalFontSize, font_weight::TerminalFontWeight,
    scale_factor::TerminalScaleFactor,
};

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalFontConfig {
    family: TerminalFontFamily,
    size: TerminalFontSize,
    bold_weight: TerminalFontWeight,
}

impl TerminalFontConfig {
    pub fn new(family: TerminalFontFamily, size: TerminalFontSize) -> Self {
        Self {
            family,
            size,
            bold_weight: TerminalFontWeight::DEFAULT_BOLD,
        }
    }

    pub fn family(&self) -> &TerminalFontFamily {
        &self.family
    }

    pub const fn size(&self) -> TerminalFontSize {
        self.size
    }

    pub const fn bold_weight(&self) -> TerminalFontWeight {
        self.bold_weight
    }

    pub fn with_bold_weight(self, bold_weight: TerminalFontWeight) -> Self {
        Self {
            bold_weight,
            ..self
        }
    }

    pub fn physical_px(&self, scale_factor: TerminalScaleFactor) -> f32 {
        self.size.physical_px(scale_factor.value())
    }
}

impl Default for TerminalFontConfig {
    fn default() -> Self {
        Self::new(TerminalFontFamily::default(), TerminalFontSize::DEFAULT)
    }
}
