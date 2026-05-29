use std::{
	cell::RefCell,
	collections::{BTreeMap, HashMap},
	rc::Rc,
};

use germinal_domain::{
	pty_host::width::{
		terminal_char_cell_advance, terminal_chars_cell_width, terminal_text_cell_width,
	},
	rendering::render_target_id::RenderTargetId,
	shared::seq::Seq,
};
use germinal_ports::rendering::{
	frame_plan_builder::{BuiltFramePlan, RenderCommandDto, TextStyleDto},
	frame_plan_presenter::FramePlanPresenter,
	surface_snapshot::{
		RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot, RenderSurfaceSnapshot,
		RenderSurfaceSnapshotProvider,
	},
};

#[derive(Debug, Clone, Default)]
pub struct TextSurfaceFramePlanPresenter {
	inner: Rc<RefCell<HashMap<RenderTargetId, TextSurface>>>,
}

impl TextSurfaceFramePlanPresenter {
	pub fn new() -> Self { Self::default() }

	pub fn surface_of(&self, target_id: RenderTargetId) -> Option<TextSurface> {
		let inner = self.inner.borrow();

		inner.get(&target_id).cloned()
	}
}

impl FramePlanPresenter for TextSurfaceFramePlanPresenter {
	fn present(&self, frame: &BuiltFramePlan) {
		let mut inner = self.inner.borrow_mut();

		let surface = inner.entry(frame.target_id).or_default();

		for command in &frame.commands {
			match command {
				RenderCommandDto::Clear => {
					surface.rows.clear();
				}
				RenderCommandDto::ClearLine { y } => {
					surface.rows.remove(y);
				}
				RenderCommandDto::TextRun { x, y, text } => {
					surface.apply_text_run(*x, *y, text, TextStyleDto::plain());
				}
				RenderCommandDto::StyledTextRun { x, y, text, style } => {
					surface.apply_text_run(*x, *y, text, *style);
				}
			}
		}

		surface.latest_seq = frame.seq;
	}
}

impl RenderSurfaceSnapshotProvider for TextSurfaceFramePlanPresenter {
	fn surface_snapshot_of(&self, target_id: RenderTargetId) -> Option<RenderSurfaceSnapshot> {
		let inner = self.inner.borrow();

		let surface = inner.get(&target_id)?;

		let rows = surface
			.rows
			.iter()
			.map(|(y, row)| RenderSurfaceRowSnapshot {
				y:    *y,
				runs: row
					.runs
					.iter()
					.map(|run| RenderSurfaceRunSnapshot {
						x:     run.x,
						text:  run.text.clone(),
						style: run.style,
					})
					.collect(),
			})
			.collect();

		Some(RenderSurfaceSnapshot { target_id, latest_seq: surface.latest_seq, rows })
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSurface {
	pub latest_seq: Seq,
	rows:           BTreeMap<u32, TextSurfaceRow>,
}

impl Default for TextSurface {
	fn default() -> Self { Self { latest_seq: Seq::ZERO, rows: BTreeMap::new() } }
}

impl TextSurface {
	pub fn text_at(&self, row: u32) -> Option<String> {
		self.rows.get(&row).map(TextSurfaceRow::text)
	}

	pub fn line_texts(&self) -> Vec<String> { self.rows.values().map(TextSurfaceRow::text).collect() }

	pub fn row_runs(&self, row: u32) -> Option<&[TextSurfaceRun]> {
		self.rows.get(&row).map(|row| row.runs.as_slice())
	}

	pub fn rows(&self) -> &BTreeMap<u32, TextSurfaceRow> { &self.rows }

	fn apply_text_run(&mut self, x: u32, y: u32, text: &str, style: TextStyleDto) {
		if text.is_empty() {
			return;
		}

		let row = self.rows.entry(y).or_default();
		row.apply_run(TextSurfaceRun { x, text: text.to_string(), style });

		if row.runs.is_empty() {
			self.rows.remove(&y);
		}
	}
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextSurfaceRow {
	runs: Vec<TextSurfaceRun>,
}

impl TextSurfaceRow {
	pub fn runs(&self) -> &[TextSurfaceRun] { &self.runs }

	pub fn text(&self) -> String {
		let mut result = String::new();

		for run in &self.runs {
			let target_len = run.x as usize;

			while terminal_text_cell_width(&result) < target_len as u32 {
				result.push(' ');
			}

			replace_text_at(&mut result, run.x as usize, &run.text);
		}

		result
	}

	fn apply_run(&mut self, run: TextSurfaceRun) {
		let run_start = run.x;
		let run_end = run.x + terminal_text_cell_width(&run.text);

		self.runs.retain(|existing| {
			let existing_start = existing.x;
			let existing_end = existing.x + terminal_text_cell_width(&existing.text);

			existing_end <= run_start || existing_start >= run_end
		});

		self.runs.push(run);
		self.runs.sort_by_key(|run| run.x);
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSurfaceRun {
	pub x:     u32,
	pub text:  String,
	pub style: TextStyleDto,
}

fn replace_text_at(target: &mut String, x: usize, text: &str) {
	let mut chars: Vec<char> = target.chars().collect();

	while terminal_text_cell_width_of_chars(&chars) < x as u32 {
		chars.push(' ');
	}

	let mut index = 0usize;
	let mut cell_x = 0u32;
	while index < chars.len() && cell_x < x as u32 {
		cell_x += terminal_char_cell_advance(chars[index]);
		index += 1;
	}

	let text_width = terminal_text_cell_width(text);
	let mut remove_count = 0usize;
	let mut removed_width = 0u32;
	while index + remove_count < chars.len() && removed_width < text_width {
		removed_width += terminal_char_cell_advance(chars[index + remove_count]);
		remove_count += 1;
	}

	chars.splice(index..index + remove_count, text.chars());

	*target = chars.into_iter().collect();
}

fn terminal_text_cell_width_of_chars(chars: &[char]) -> u32 { terminal_chars_cell_width(chars) }

#[cfg(test)]
mod tests {
	use germinal_ports::rendering::frame_plan_builder::{RgbColorDto, TextStyleDto};

	use super::*;

	#[test]
	fn applies_delta_frames_to_surface() {
		let presenter = TextSurfaceFramePlanPresenter::new();
		let target_id = RenderTargetId::new(1);

		presenter.present(&BuiltFramePlan {
			target_id,
			seq: Seq::new(1),
			commands: vec![
				RenderCommandDto::ClearLine { y: 0 },
				RenderCommandDto::TextRun { x: 0, y: 0, text: "red".to_string() },
				RenderCommandDto::ClearLine { y: 1 },
				RenderCommandDto::TextRun { x: 0, y: 1, text: "old".to_string() },
			],
		});

		presenter.present(&BuiltFramePlan {
			target_id,
			seq: Seq::new(2),
			commands: vec![RenderCommandDto::ClearLine { y: 1 }, RenderCommandDto::TextRun {
				x:    0,
				y:    1,
				text: "new".to_string(),
			}],
		});

		let surface = presenter.surface_of(target_id).unwrap();

		assert_eq!(surface.latest_seq, Seq::new(2));
		assert_eq!(surface.text_at(0), Some("red".to_string()));
		assert_eq!(surface.text_at(1), Some("new".to_string()));
		assert_eq!(surface.line_texts(), vec!["red".to_string(), "new".to_string()]);
	}

	#[test]
	fn clear_line_removes_row_from_surface() {
		let presenter = TextSurfaceFramePlanPresenter::new();
		let target_id = RenderTargetId::new(1);

		presenter.present(&BuiltFramePlan {
			target_id,
			seq: Seq::new(1),
			commands: vec![RenderCommandDto::TextRun { x: 0, y: 0, text: "hello".to_string() }],
		});

		presenter.present(&BuiltFramePlan {
			target_id,
			seq: Seq::new(2),
			commands: vec![RenderCommandDto::ClearLine { y: 0 }],
		});

		let surface = presenter.surface_of(target_id).unwrap();

		assert_eq!(surface.latest_seq, Seq::new(2));
		assert_eq!(surface.text_at(0), None);
		assert!(surface.line_texts().is_empty());
	}

	#[test]
	fn styled_text_run_updates_text_surface_and_preserves_style() {
		let presenter = TextSurfaceFramePlanPresenter::new();
		let target_id = RenderTargetId::new(1);

		let style = TextStyleDto {
			foreground: Some(RgbColorDto::new(255, 0, 0)),
			background: None,
			bold:       true,
			italic:     false,
			underline:  false,
		};

		presenter.present(&BuiltFramePlan {
			target_id,
			seq: Seq::new(1),
			commands: vec![RenderCommandDto::StyledTextRun {
				x: 0,
				y: 0,
				text: "styled".to_string(),
				style,
			}],
		});

		let surface = presenter.surface_of(target_id).unwrap();

		assert_eq!(surface.latest_seq, Seq::new(1));
		assert_eq!(surface.text_at(0), Some("styled".to_string()));

		let runs = surface.row_runs(0).unwrap();

		assert_eq!(runs.len(), 1);
		assert_eq!(runs[0].x, 0);
		assert_eq!(runs[0].text, "styled");
		assert_eq!(runs[0].style, style);
	}

	#[test]
	fn multiple_runs_on_same_row_keep_positions_and_styles() {
		let presenter = TextSurfaceFramePlanPresenter::new();
		let target_id = RenderTargetId::new(1);

		let red = TextStyleDto {
			foreground: Some(RgbColorDto::new(255, 0, 0)),
			background: None,
			bold:       true,
			italic:     false,
			underline:  false,
		};

		let plain = TextStyleDto::plain();

		presenter.present(&BuiltFramePlan {
			target_id,
			seq: Seq::new(1),
			commands: vec![
				RenderCommandDto::StyledTextRun {
					x:     0,
					y:     0,
					text:  "red".to_string(),
					style: red,
				},
				RenderCommandDto::StyledTextRun {
					x:     4,
					y:     0,
					text:  "plain".to_string(),
					style: plain,
				},
			],
		});

		let surface = presenter.surface_of(target_id).unwrap();

		assert_eq!(surface.text_at(0), Some("red plain".to_string()));

		let runs = surface.row_runs(0).unwrap();

		assert_eq!(runs.len(), 2);
		assert_eq!(runs[0].x, 0);
		assert_eq!(runs[0].text, "red");
		assert_eq!(runs[0].style, red);

		assert_eq!(runs[1].x, 4);
		assert_eq!(runs[1].text, "plain");
		assert_eq!(runs[1].style, plain);
	}

	#[test]
	fn exports_renderer_ready_surface_snapshot() {
		let presenter = TextSurfaceFramePlanPresenter::new();
		let target_id = RenderTargetId::new(1);

		let red = TextStyleDto {
			foreground: Some(RgbColorDto::new(255, 0, 0)),
			background: None,
			bold:       true,
			italic:     false,
			underline:  false,
		};

		presenter.present(&BuiltFramePlan {
			target_id,
			seq: Seq::new(9),
			commands: vec![
				RenderCommandDto::StyledTextRun {
					x:     0,
					y:     0,
					text:  "red".to_string(),
					style: red,
				},
				RenderCommandDto::TextRun { x: 4, y: 0, text: "plain".to_string() },
			],
		});

		let snapshot = presenter.surface_snapshot_of(target_id).expect("surface snapshot should exist");

		assert_eq!(snapshot.target_id, target_id);
		assert_eq!(snapshot.latest_seq, Seq::new(9));
		assert_eq!(snapshot.rows.len(), 1);

		let row = &snapshot.rows[0];

		assert_eq!(row.y, 0);
		assert_eq!(row.runs.len(), 2);

		assert_eq!(row.runs[0].x, 0);
		assert_eq!(row.runs[0].text, "red");
		assert_eq!(row.runs[0].style, red);

		assert_eq!(row.runs[1].x, 4);
		assert_eq!(row.runs[1].text, "plain");
		assert_eq!(row.runs[1].style, TextStyleDto::plain());
	}
}
