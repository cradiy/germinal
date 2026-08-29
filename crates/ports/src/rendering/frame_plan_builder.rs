pub use germinal_gnative_protocol::rendering::frame_plan_builder::{
    PIXEL_FILL_RECT_MARKER, RenderCommandDto, RgbColorDto, RgbaColorDto, TextStyleDto,
    decode_pixel_fill_rect_command, encode_pixel_fill_rect_command,
};

use crate::{rendering::render_target_id::RenderTargetId, seq::Seq};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildFramePlanTask {
    pub target_id: RenderTargetId,
    pub seq: Seq,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltFramePlan {
    pub target_id: RenderTargetId,
    pub seq: Seq,
    pub commands: Vec<RenderCommandDto>,
}

pub trait FramePlanBuilder {
    fn build(&self, task: BuildFramePlanTask) -> BuiltFramePlan;
}
