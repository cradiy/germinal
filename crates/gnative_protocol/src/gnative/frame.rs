use germinal_domain::gshell::vo::gshell_id::GShellId;
use serde::{Deserialize, Serialize};

use crate::{rendering::frame_plan_builder::RenderCommandDto, seq::Seq};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeFrame {
    pub gshell_id: GShellId,
    pub seq: Seq,
    pub commands: Vec<RenderCommandDto>,
    pub cursor: Option<GNativeFrameCursor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeFrameCursor {
    pub x: u32,
    pub y: u32,
}
