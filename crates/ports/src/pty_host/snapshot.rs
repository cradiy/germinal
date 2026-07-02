use crate::{
	rendering::{
		frame_plan_builder::TextStyleDto,
		render_target_id::RenderTargetId,
		surface_snapshot::{RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot, RenderSurfaceSnapshot},
	},
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

	fn render_surface_snapshot_of(
		&self,
		render_target_id: RenderTargetId,
	) -> Option<RenderSurfaceSnapshot> {
		self.snapshot_of(render_target_id).map(render_surface_snapshot_from_terminal_snapshot)
	}

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

pub fn render_surface_snapshot_from_terminal_snapshot(
	snapshot: TerminalSnapshot,
) -> RenderSurfaceSnapshot {
	let mut runs_by_row = std::collections::BTreeMap::<u32, Vec<RenderSurfaceRunSnapshot>>::new();

	for run in snapshot.text_runs {
		runs_by_row.entry(run.y).or_default().push(RenderSurfaceRunSnapshot {
			x:     run.x,
			text:  run.text,
			style: run.style,
		});
	}

	for line in snapshot.lines {
		if line.text.is_empty() || runs_by_row.contains_key(&line.row) {
			continue;
		}

		runs_by_row.insert(line.row, vec![RenderSurfaceRunSnapshot {
			x:     0,
			text:  line.text,
			style: TextStyleDto::plain(),
		}]);
	}

	let rows = runs_by_row
		.into_iter()
		.map(|(y, mut runs)| {
			runs.sort_by_key(|run| run.x);
			RenderSurfaceRowSnapshot { y, runs }
		})
		.collect();

	RenderSurfaceSnapshot {
		target_id: snapshot.render_target_id,
		latest_seq: snapshot.latest_seq,
		rows,
		video_surfaces: Vec::new(),
		dirty_rows: snapshot.dirty_rows,
		cursor: None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::rendering::frame_plan_builder::RgbColorDto;

	#[test]
	fn render_surface_snapshot_prefers_styled_runs() {
		let snapshot = render_surface_snapshot_from_terminal_snapshot(TerminalSnapshot {
			render_target_id: RenderTargetId::new(1),
			latest_seq:       Seq::new(7),
			lines:            vec![TerminalLineSnapshot { row: 0, text: "plain".to_string() }],
			text_runs:        vec![TerminalTextRunSnapshot {
				x:     0,
				y:     0,
				text:  "styled".to_string(),
				style: TextStyleDto {
					foreground: Some(RgbColorDto::new(255, 0, 0)),
					background: None,
					bold:       true,
					italic:     false,
					underline:  false,
				},
			}],
			dirty_rows:       vec![0],
		});

		assert_eq!(snapshot.rows.len(), 1);
		assert_eq!(snapshot.rows[0].runs.len(), 1);
		assert_eq!(snapshot.rows[0].runs[0].text, "styled");
		assert!(snapshot.rows[0].runs[0].style.bold);
	}

	#[test]
	fn render_surface_snapshot_falls_back_to_plain_line() {
		let snapshot = render_surface_snapshot_from_terminal_snapshot(TerminalSnapshot {
			render_target_id: RenderTargetId::new(1),
			latest_seq:       Seq::new(3),
			lines:            vec![TerminalLineSnapshot { row: 2, text: "hello".to_string() }],
			text_runs:        vec![],
			dirty_rows:       vec![2],
		});

		assert_eq!(snapshot.rows.len(), 1);
		assert_eq!(snapshot.rows[0].y, 2);
		assert_eq!(snapshot.rows[0].runs.len(), 1);
		assert_eq!(snapshot.rows[0].runs[0].x, 0);
		assert_eq!(snapshot.rows[0].runs[0].text, "hello");
		assert_eq!(snapshot.rows[0].runs[0].style, TextStyleDto::plain());
	}
}
