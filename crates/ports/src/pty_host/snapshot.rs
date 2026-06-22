use crate::{
	rendering::{frame_plan_builder::TextStyleDto, render_target_id::RenderTargetId},
	seq::Seq,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot {
	pub render_target_id: RenderTargetId,
	pub latest_seq:       Seq,

	/// Plain full-line text snapshot.
	///
	/// This is kept for simple terminal adapters and debugging.
	pub lines: Vec<TerminalLineSnapshot>,

	/// Styled text runs.
	///
	/// Renderers should prefer this when it is non-empty.
	pub text_runs: Vec<TerminalTextRunSnapshot>,

	pub dirty_rows: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalLineSnapshot {
	pub row:  u32,
	pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTextRunSnapshot {
	pub x:     u32,
	pub y:     u32,
	pub text:  String,
	pub style: TextStyleDto,
}

pub trait TerminalSnapshotProvider {
	fn snapshot_of(&self, render_target_id: RenderTargetId) -> Option<TerminalSnapshot>;

	fn snapshot_for_build(
		&self,
		render_target_id: RenderTargetId,
		build_seq: Seq,
	) -> Option<TerminalSnapshot> {
		let _ = build_seq;
		self.snapshot_of(render_target_id)
	}

	fn clear_damage_up_to(&self, render_target_id: RenderTargetId, presented_seq: Seq) {
		let _ = render_target_id;
		let _ = presented_seq;
	}
}
