use std::collections::{BTreeMap, BTreeSet};

use germinal_ports::{
	pty_host::snapshot::{TerminalSnapshot, TerminalSnapshotProvider},
	rendering::frame_plan_builder::{
		BuildFramePlanTask, BuiltFramePlan, FramePlanBuilder, RenderCommandDto,
	},
};

#[derive(Debug, Clone)]
pub struct SnapshotFramePlanBuilder<P> {
	snapshot_provider: P,
}

impl<P> SnapshotFramePlanBuilder<P> {
	pub fn new(snapshot_provider: P) -> Self { Self { snapshot_provider } }
}

impl<P> FramePlanBuilder for SnapshotFramePlanBuilder<P>
where P: TerminalSnapshotProvider
{
	fn build(&self, task: BuildFramePlanTask) -> BuiltFramePlan {
		let mut commands = Vec::new();

		match self.snapshot_provider.snapshot_for_build(task.target_id, task.seq) {
			Some(snapshot) => {
				append_snapshot_commands(&mut commands, snapshot);
			}
			None => {
				commands.push(RenderCommandDto::Clear);
				commands.push(RenderCommandDto::TextRun {
					x:    0,
					y:    0,
					text: format!(
						"target={} seq={} terminal_snapshot=<missing>",
						task.target_id.value(),
						task.seq.value(),
					),
				});
			}
		}

		BuiltFramePlan { target_id: task.target_id, seq: task.seq, commands }
	}
}

fn append_snapshot_commands(commands: &mut Vec<RenderCommandDto>, snapshot: TerminalSnapshot) {
	let line_by_row: BTreeMap<u32, String> =
		snapshot.lines.into_iter().map(|line| (line.row, line.text)).collect();

	let mut runs_by_row: BTreeMap<u32, Vec<_>> = BTreeMap::new();

	for run in snapshot.text_runs {
		runs_by_row.entry(run.y).or_default().push(run);
	}

	let rows_to_render = rows_to_render(snapshot.dirty_rows, &line_by_row, &runs_by_row);

	for row in rows_to_render {
		commands.push(RenderCommandDto::ClearLine { y: row });

		if let Some(runs) = runs_by_row.get(&row) {
			for run in runs {
				if run.text.is_empty() {
					continue;
				}

				commands.push(RenderCommandDto::StyledTextRun {
					x:     run.x,
					y:     run.y,
					text:  run.text.clone(),
					style: run.style,
				});
			}

			continue;
		}

		if let Some(text) = line_by_row.get(&row)
			&& !text.is_empty()
		{
			commands.push(RenderCommandDto::TextRun { x: 0, y: row, text: text.clone() });
		}
	}
}

fn rows_to_render<T>(
	dirty_rows: Vec<u32>,
	line_by_row: &BTreeMap<u32, String>,
	runs_by_row: &BTreeMap<u32, Vec<T>>,
) -> BTreeSet<u32> {
	if !dirty_rows.is_empty() {
		return dirty_rows.into_iter().collect();
	}

	let mut rows = BTreeSet::new();

	rows.extend(line_by_row.keys().copied());
	rows.extend(runs_by_row.keys().copied());

	rows
}

#[cfg(test)]
mod tests {
	use germinal_domain::{rendering::render_target_id::RenderTargetId, shared::seq::Seq};
	use germinal_ports::{
		pty_host::snapshot::{
			TerminalLineSnapshot, TerminalSnapshot, TerminalSnapshotProvider, TerminalTextRunSnapshot,
		},
		rendering::frame_plan_builder::{RgbColorDto, TextStyleDto},
	};

	use super::*;

	#[derive(Debug, Clone)]
	struct TestSnapshotProvider;

	impl TerminalSnapshotProvider for TestSnapshotProvider {
		fn snapshot_of(&self, render_target_id: RenderTargetId) -> Option<TerminalSnapshot> {
			Some(TerminalSnapshot {
				render_target_id,
				latest_seq: Seq::new(7),
				lines: vec![
					TerminalLineSnapshot { row: 0, text: "hello".to_string() },
					TerminalLineSnapshot { row: 1, text: "world".to_string() },
					TerminalLineSnapshot { row: 2, text: "clean".to_string() },
				],
				text_runs: vec![],
				dirty_rows: vec![0, 1],
			})
		}
	}

	#[test]
	fn builds_frame_only_from_dirty_terminal_snapshot_rows() {
		let builder = SnapshotFramePlanBuilder::new(TestSnapshotProvider);

		let frame = builder
			.build(BuildFramePlanTask { target_id: RenderTargetId::new(1), seq: Seq::new(7) });

		assert_eq!(frame.target_id, RenderTargetId::new(1));
		assert_eq!(frame.seq, Seq::new(7));

		assert_eq!(frame.commands, vec![
			RenderCommandDto::ClearLine { y: 0 },
			RenderCommandDto::TextRun { x: 0, y: 0, text: "hello".to_string() },
			RenderCommandDto::ClearLine { y: 1 },
			RenderCommandDto::TextRun { x: 0, y: 1, text: "world".to_string() },
		]);
	}

	#[derive(Debug, Clone)]
	struct DirtyEmptyLineSnapshotProvider;

	impl TerminalSnapshotProvider for DirtyEmptyLineSnapshotProvider {
		fn snapshot_of(&self, render_target_id: RenderTargetId) -> Option<TerminalSnapshot> {
			Some(TerminalSnapshot {
				render_target_id,
				latest_seq: Seq::new(7),
				lines: vec![
					TerminalLineSnapshot { row: 0, text: "hello".to_string() },
					TerminalLineSnapshot { row: 1, text: "world".to_string() },
				],
				text_runs: vec![],
				dirty_rows: vec![0, 2],
			})
		}
	}

	#[test]
	fn dirty_empty_line_still_generates_clear_line() {
		let builder = SnapshotFramePlanBuilder::new(DirtyEmptyLineSnapshotProvider);

		let frame = builder
			.build(BuildFramePlanTask { target_id: RenderTargetId::new(1), seq: Seq::new(7) });

		assert_eq!(frame.commands, vec![
			RenderCommandDto::ClearLine { y: 0 },
			RenderCommandDto::TextRun { x: 0, y: 0, text: "hello".to_string() },
			RenderCommandDto::ClearLine { y: 2 },
		]);
	}

	#[derive(Debug, Clone)]
	struct EmptyDirtyRowsSnapshotProvider;

	impl TerminalSnapshotProvider for EmptyDirtyRowsSnapshotProvider {
		fn snapshot_of(&self, render_target_id: RenderTargetId) -> Option<TerminalSnapshot> {
			Some(TerminalSnapshot {
				render_target_id,
				latest_seq: Seq::new(7),
				lines: vec![
					TerminalLineSnapshot { row: 0, text: "hello".to_string() },
					TerminalLineSnapshot { row: 1, text: "world".to_string() },
				],
				text_runs: vec![],
				dirty_rows: vec![],
			})
		}
	}

	#[test]
	fn empty_dirty_rows_falls_back_to_all_rows() {
		let builder = SnapshotFramePlanBuilder::new(EmptyDirtyRowsSnapshotProvider);

		let frame = builder
			.build(BuildFramePlanTask { target_id: RenderTargetId::new(1), seq: Seq::new(7) });

		assert_eq!(frame.commands, vec![
			RenderCommandDto::ClearLine { y: 0 },
			RenderCommandDto::TextRun { x: 0, y: 0, text: "hello".to_string() },
			RenderCommandDto::ClearLine { y: 1 },
			RenderCommandDto::TextRun { x: 0, y: 1, text: "world".to_string() },
		]);
	}

	#[derive(Debug, Clone)]
	struct StyledSnapshotProvider;

	impl TerminalSnapshotProvider for StyledSnapshotProvider {
		fn snapshot_of(&self, render_target_id: RenderTargetId) -> Option<TerminalSnapshot> {
			Some(TerminalSnapshot {
				render_target_id,
				latest_seq: Seq::new(8),
				lines: vec![TerminalLineSnapshot { row: 0, text: "red plain".to_string() }],
				text_runs: vec![
					TerminalTextRunSnapshot {
						x:     0,
						y:     0,
						text:  "red".to_string(),
						style: TextStyleDto {
							foreground: Some(RgbColorDto::new(255, 0, 0)),
							background: None,
							bold:       true,
							italic:     false,
							underline:  false,
						},
					},
					TerminalTextRunSnapshot {
						x:     4,
						y:     0,
						text:  "plain".to_string(),
						style: TextStyleDto::plain(),
					},
				],
				dirty_rows: vec![0],
			})
		}
	}

	#[test]
	fn styled_runs_take_priority_over_plain_line_text() {
		let builder = SnapshotFramePlanBuilder::new(StyledSnapshotProvider);

		let frame = builder
			.build(BuildFramePlanTask { target_id: RenderTargetId::new(1), seq: Seq::new(8) });

		assert_eq!(frame.commands, vec![
			RenderCommandDto::ClearLine { y: 0 },
			RenderCommandDto::StyledTextRun {
				x:     0,
				y:     0,
				text:  "red".to_string(),
				style: TextStyleDto {
					foreground: Some(RgbColorDto::new(255, 0, 0)),
					background: None,
					bold:       true,
					italic:     false,
					underline:  false,
				},
			},
			RenderCommandDto::StyledTextRun {
				x:     4,
				y:     0,
				text:  "plain".to_string(),
				style: TextStyleDto::plain(),
			},
		]);
	}
}
