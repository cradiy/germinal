use germinal_domain::{rendering::render_target_id::RenderTargetId, shared::seq::Seq};

use crate::rendering::frame_plan_builder::TextStyleDto;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceSnapshot {
	pub target_id:  RenderTargetId,
	pub latest_seq: Seq,
	pub rows:       Vec<RenderSurfaceRowSnapshot>,
	pub cursor:     Option<RenderSurfaceCursorSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceRowSnapshot {
	pub y:    u32,
	pub runs: Vec<RenderSurfaceRunSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceRunSnapshot {
	pub x:     u32,
	pub text:  String,
	pub style: TextStyleDto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderSurfaceCursorSnapshot {
	pub x:       u32,
	pub y:       u32,
	pub focused: bool,
}

pub trait RenderSurfaceSnapshotProvider {
	fn surface_snapshot_of(&self, target_id: RenderTargetId) -> Option<RenderSurfaceSnapshot>;
}
