use std::sync::Arc;

use crate::{
	rendering::{
		frame_plan_builder::{RgbColorDto, TextStyleDto},
		render_target_id::RenderTargetId,
	},
	seq::Seq,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceSnapshot {
	pub target_id:          RenderTargetId,
	pub latest_seq:         Seq,
	pub default_background: RgbColorDto,
	pub rows:               Vec<RenderSurfaceRowSnapshot>,
	pub video_surfaces:     Vec<RenderSurfaceVideoSurfaceSnapshot>,
	pub image_surfaces:     Vec<RenderSurfaceImageSnapshot>,
	pub dirty_rows:         Vec<u32>,
	pub cursor:             Option<RenderSurfaceCursorSnapshot>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceImageSnapshot {
	pub id:               String,
	pub image_generation: u64,
	pub x_cell:           u32,
	pub y_cell:           u32,
	pub x_offset_px:      u32,
	pub y_offset_px:      u32,
	pub columns:          u32,
	pub rows:             u32,
	pub source_x_px:      u32,
	pub source_y_px:      u32,
	pub source_width_px:  u32,
	pub source_height_px: u32,
	pub image_width_px:   u32,
	pub image_height_px:  u32,
	pub z_index:          i32,
	pub rgba:             Arc<[u8]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderSurfaceCursorSnapshot {
	pub x:       u32,
	pub y:       u32,
	pub focused: bool,
	pub shape:   RenderSurfaceCursorShape,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum RenderSurfaceCursorShape {
	#[default]
	Block,
	Underline,
	Beam,
	HollowBlock,
	Hidden,
}

pub trait RenderSurfaceSnapshotProvider {
	fn surface_snapshot_of(&self, target_id: RenderTargetId) -> Option<RenderSurfaceSnapshot>;
}
