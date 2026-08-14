use crate::pty_host::font_family::TerminalFontFamily;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFontFace {
    family: TerminalFontFamily,
    style: Option<String>,
}

impl TerminalFontFace {
    pub fn new(family: TerminalFontFamily) -> Self {
        Self {
            family,
            style: None,
        }
    }

    pub fn with_style(mut self, style: impl Into<String>) -> Self {
        let style = style.into();
        self.style = (!style.trim().is_empty()).then_some(style);
        self
    }

    pub fn family(&self) -> &TerminalFontFamily {
        &self.family
    }

    pub fn style(&self) -> Option<&str> {
        self.style.as_deref()
    }
}

impl Default for TerminalFontFace {
    fn default() -> Self {
        Self::new(TerminalFontFamily::default())
    }
}
