#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalHyperlink {
    pub uri: String,
    pub x: u32,
    pub y: u32,
    pub columns: u32,
}

impl TerminalHyperlink {
    pub fn contains(&self, x: u32, y: u32) -> bool {
        self.y == y && x >= self.x && x < self.x.saturating_add(self.columns)
    }
}
