use crate::{
	rendering::{frame_plan_builder::TextStyleDto, render_target_id::RenderTargetId},
	seq::Seq,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceSnapshot {
	pub target_id:      RenderTargetId,
	pub latest_seq:     Seq,
	pub rows:           Vec<RenderSurfaceRowSnapshot>,
	pub video_surfaces: Vec<RenderSurfaceVideoSurfaceSnapshot>,
	pub dirty_rows:     Vec<u32>,
	pub cursor:         Option<RenderSurfaceCursorSnapshot>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceVideoSurfaceSnapshot {
	pub id:        String,
	pub x_px:      u32,
	pub y_px:      u32,
	pub width_px:  u32,
	pub height_px: u32,
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
