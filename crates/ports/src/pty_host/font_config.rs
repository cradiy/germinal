use crate::pty_host::{
    font_face::TerminalFontFace, font_family::TerminalFontFamily, font_size::TerminalFontSize,
    font_weight::TerminalFontWeight, scale_factor::TerminalScaleFactor,
};

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalFontConfig {
    normal: TerminalFontFace,
    bold: Option<TerminalFontFace>,
    italic: Option<TerminalFontFace>,
    bold_italic: Option<TerminalFontFace>,
    fallbacks: Vec<TerminalFontFamily>,
    size: TerminalFontSize,
    bold_weight: TerminalFontWeight,
    ligatures: bool,
}

impl TerminalFontConfig {
    pub fn new(family: TerminalFontFamily, size: TerminalFontSize) -> Self {
        Self {
            normal: TerminalFontFace::new(family),
            bold: None,
            italic: None,
            bold_italic: None,
            fallbacks: Vec::new(),
            size,
            bold_weight: TerminalFontWeight::DEFAULT_BOLD,
            ligatures: true,
        }
    }

    pub fn family(&self) -> &TerminalFontFamily {
        self.normal.family()
    }

    pub fn normal(&self) -> &TerminalFontFace {
        &self.normal
    }

    pub fn bold(&self) -> Option<&TerminalFontFace> {
        self.bold.as_ref()
    }

    pub fn italic(&self) -> Option<&TerminalFontFace> {
        self.italic.as_ref()
    }

    pub fn bold_italic(&self) -> Option<&TerminalFontFace> {
        self.bold_italic.as_ref()
    }

    pub fn fallbacks(&self) -> &[TerminalFontFamily] {
        &self.fallbacks
    }

    pub const fn size(&self) -> TerminalFontSize {
        self.size
    }

    pub const fn bold_weight(&self) -> TerminalFontWeight {
        self.bold_weight
    }

    pub const fn ligatures(&self) -> bool {
        self.ligatures
    }

    pub fn with_bold_weight(self, bold_weight: TerminalFontWeight) -> Self {
        Self {
            bold_weight,
            ..self
        }
    }

    pub fn with_ligatures(self, ligatures: bool) -> Self {
        Self { ligatures, ..self }
    }

    pub fn with_faces(
        self,
        normal: TerminalFontFace,
        bold: Option<TerminalFontFace>,
        italic: Option<TerminalFontFace>,
        bold_italic: Option<TerminalFontFace>,
        fallbacks: Vec<TerminalFontFamily>,
    ) -> Self {
        Self {
            normal,
            bold,
            italic,
            bold_italic,
            fallbacks,
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
