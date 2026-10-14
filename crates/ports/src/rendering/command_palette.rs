#[derive(Debug, Clone)]
pub struct CommandPaletteSnapshot {
    pub query: String,
    pub preedit: String,
    pub preedit_cursor: Option<usize>,
    pub entries: Vec<CommandPaletteEntry>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct CommandPaletteEntry {
    pub title: String,
    pub shortcut: String,
}
use super::render_target_id::RenderTargetId;

pub const COMMAND_PALETTE_TARGET_ID: RenderTargetId = RenderTargetId::new(u64::MAX - 1);
