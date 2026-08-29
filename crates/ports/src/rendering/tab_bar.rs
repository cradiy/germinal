use serde::{Deserialize, Serialize};

use super::render_target_id::RenderTargetId;
use crate::pty_host::terminal_progress::TerminalProgress;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabBarPosition {
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabBarStyle {
    #[default]
    Fade,
    Powerline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabBarSnapshot {
    pub titles: Vec<String>,
    pub progresses: Vec<Option<TerminalProgress>>,
    pub render_target_ids: Vec<RenderTargetId>,
    pub active_tab_index: usize,
    pub position: TabBarPosition,
    pub style: TabBarStyle,
}
