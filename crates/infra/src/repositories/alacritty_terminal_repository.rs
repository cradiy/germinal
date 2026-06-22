use std::{
	cell::RefCell,
	collections::{BTreeMap, BTreeSet, HashMap},
	rc::Rc,
	sync::mpsc::{self, Receiver, Sender},
};

use alacritty_terminal::{
	event::{Event, EventListener},
	grid::Dimensions,
	term::{Config, Term, TermDamage, TermMode, cell::Flags, color::Colors},
	vte::ansi::{Color, NamedColor, Processor, Rgb, StdSyncHandler},
};
use germinal_ports::{
	pty_host::{
		snapshot::{
			TerminalLineSnapshot, TerminalSnapshot, TerminalSnapshotProvider, TerminalTextRunSnapshot,
		},
		width::terminal_char_cell_width,
	},
	rendering::{
		frame_plan_builder::{RgbColorDto, TextStyleDto},
		render_target_id::RenderTargetId,
	},
	seq::Seq,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlacrittyTermSize {
	columns:      usize,
	screen_lines: usize,
}

impl AlacrittyTermSize {
	pub const fn new(columns: usize, screen_lines: usize) -> Self { Self { columns, screen_lines } }

	pub const fn columns(self) -> usize { self.columns }

	pub const fn screen_lines(self) -> usize { self.screen_lines }
}

impl Default for AlacrittyTermSize {
	fn default() -> Self { Self { columns: 80, screen_lines: 24 } }
}

impl Dimensions for AlacrittyTermSize {
	fn total_lines(&self) -> usize { self.screen_lines }

	fn screen_lines(&self) -> usize { self.screen_lines }

	fn columns(&self) -> usize { self.columns }
}

#[derive(Clone)]
struct PtyWriteEventListener {
	pending_writes: Sender<Vec<u8>>,
}

impl PtyWriteEventListener {
	fn new(pending_writes: Sender<Vec<u8>>) -> Self { Self { pending_writes } }
}

impl EventListener for PtyWriteEventListener {
	fn send_event(&self, event: Event) {
		let Event::PtyWrite(text) = event else {
			return;
		};

		let _ = self.pending_writes.send(text.into_bytes());
	}
}

#[derive(Clone)]
pub struct AlacrittyTerminalRepository {
	inner: Rc<RefCell<HashMap<RenderTargetId, AlacrittyTermState>>>,
	size:  AlacrittyTermSize,
}

impl AlacrittyTerminalRepository {
	pub fn new() -> Self { Self::with_size(AlacrittyTermSize::default()) }

	pub fn with_size(size: AlacrittyTermSize) -> Self {
		Self { inner: Rc::new(RefCell::new(HashMap::new())), size }
	}

	pub fn size(&self) -> AlacrittyTermSize { self.size }

	pub fn apply_bytes(
		&self,
		render_target_id: RenderTargetId,
		seq: Seq,
		bytes: &[u8],
	) -> AlacrittyTermApplyStats {
		let mut inner = self.inner.borrow_mut();

		let state = inner.entry(render_target_id).or_insert_with(|| AlacrittyTermState::new(self.size));

		state.processor.advance(&mut state.term, bytes);

		state.latest_seq = seq;
		state.total_bytes += bytes.len() as u64;
		state.chunk_count += 1;

		AlacrittyTermApplyStats {
			latest_seq:  state.latest_seq,
			total_bytes: state.total_bytes,
			chunk_count: state.chunk_count,
		}
	}

	pub fn resize(
		&self,
		render_target_id: RenderTargetId,
		seq: Seq,
		size: AlacrittyTermSize,
	) -> AlacrittyTermApplyStats {
		let mut inner = self.inner.borrow_mut();

		let state = inner.entry(render_target_id).or_insert_with(|| AlacrittyTermState::new(size));

		state.resize(size);
		state.latest_seq = seq;

		AlacrittyTermApplyStats {
			latest_seq:  state.latest_seq,
			total_bytes: state.total_bytes,
			chunk_count: state.chunk_count,
		}
	}

	fn snapshot_from_state(
		render_target_id: RenderTargetId,
		state: &mut AlacrittyTermState,
	) -> TerminalSnapshot {
		let (lines, text_runs) = visible_lines_and_runs(&state.term);
		let dirty_rows = dirty_rows_of(state.term.damage(), state.size.screen_lines());

		TerminalSnapshot {
			render_target_id,
			latest_seq: state.latest_seq,
			lines,
			text_runs,
			dirty_rows,
		}
	}

	pub fn stats_of(&self, render_target_id: RenderTargetId) -> Option<AlacrittyTermApplyStats> {
		let inner = self.inner.borrow();

		let state = inner.get(&render_target_id)?;

		Some(AlacrittyTermApplyStats {
			latest_seq:  state.latest_seq,
			total_bytes: state.total_bytes,
			chunk_count: state.chunk_count,
		})
	}

	pub fn take_pending_pty_writes(&self, render_target_id: RenderTargetId) -> Vec<Vec<u8>> {
		let mut inner = self.inner.borrow_mut();

		let Some(state) = inner.get_mut(&render_target_id) else {
			return Vec::new();
		};

		state.take_pending_writes()
	}

	pub fn cursor_position_1_based(
		&self,
		render_target_id: RenderTargetId,
	) -> Option<(usize, usize)> {
		let inner = self.inner.borrow();

		let state = inner.get(&render_target_id)?;
		if !state.term.mode().contains(TermMode::SHOW_CURSOR) {
			return None;
		}

		let point = state.term.grid().cursor.point;

		Some((point.line.0 as usize + 1, point.column.0 + 1))
	}

	pub fn cursor_position_0_based(&self, render_target_id: RenderTargetId) -> Option<(u32, u32)> {
		let inner = self.inner.borrow();

		let state = inner.get(&render_target_id)?;
		if !state.term.mode().contains(TermMode::SHOW_CURSOR) {
			return None;
		}
		let point = state.term.grid().cursor.point;

		let row = u32::try_from(point.line.0).ok()?;
		let col = u32::try_from(point.column.0).ok()?;

		Some((col, row))
	}
}

impl Default for AlacrittyTerminalRepository {
	fn default() -> Self { Self::new() }
}

impl TerminalSnapshotProvider for AlacrittyTerminalRepository {
	fn snapshot_of(&self, render_target_id: RenderTargetId) -> Option<TerminalSnapshot> {
		let mut inner = self.inner.borrow_mut();

		let state = inner.get_mut(&render_target_id)?;

		Some(Self::snapshot_from_state(render_target_id, state))
	}

	fn snapshot_for_build(
		&self,
		render_target_id: RenderTargetId,
		build_seq: Seq,
	) -> Option<TerminalSnapshot> {
		let _ = build_seq;

		let mut inner = self.inner.borrow_mut();

		let state = inner.get_mut(&render_target_id)?;

		Some(Self::snapshot_from_state(render_target_id, state))
	}

	fn clear_damage_up_to(&self, render_target_id: RenderTargetId, presented_seq: Seq) {
		let mut inner = self.inner.borrow_mut();

		let Some(state) = inner.get_mut(&render_target_id) else {
			return;
		};

		if state.latest_seq <= presented_seq {
			state.term.reset_damage();
		}
	}
}

pub struct AlacrittyTermState {
	term:             Term<PtyWriteEventListener>,
	pending_write_rx: Receiver<Vec<u8>>,
	processor:        Processor<StdSyncHandler>,
	size:             AlacrittyTermSize,
	latest_seq:       Seq,
	total_bytes:      u64,
	chunk_count:      u64,
}

impl AlacrittyTermState {
	fn new(size: AlacrittyTermSize) -> Self {
		let (pending_write_tx, pending_write_rx) = mpsc::channel();
		let event_listener = PtyWriteEventListener::new(pending_write_tx);
		let mut term = Term::new(Config::default(), &size, event_listener.clone());

		term.reset_damage();

		Self {
			term,
			pending_write_rx,
			processor: Processor::<StdSyncHandler>::new(),
			size,
			latest_seq: Seq::ZERO,
			total_bytes: 0,
			chunk_count: 0,
		}
	}

	fn resize(&mut self, size: AlacrittyTermSize) {
		if self.size == size {
			return;
		}

		self.size = size;
		self.term.resize(self.size);
	}

	fn take_pending_writes(&mut self) -> Vec<Vec<u8>> {
		let mut writes = Vec::new();

		while let Ok(bytes) = self.pending_write_rx.try_recv() {
			writes.push(bytes);
		}

		writes
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlacrittyTermApplyStats {
	pub latest_seq:  Seq,
	pub total_bytes: u64,
	pub chunk_count: u64,
}

#[derive(Debug, Clone)]
struct StyledCell {
	col:   u32,
	c:     char,
	style: TextStyleDto,
}

fn visible_lines_and_runs(
	term: &Term<PtyWriteEventListener>,
) -> (Vec<TerminalLineSnapshot>, Vec<TerminalTextRunSnapshot>) {
	let renderable = term.renderable_content();
	let mut cells_by_row: BTreeMap<u32, Vec<StyledCell>> = BTreeMap::new();

	for indexed in renderable.display_iter {
		let raw_row = indexed.point.line.0;
		let raw_col = indexed.point.column.0;
		let cell = indexed.cell;

		let Some(row) = u32::try_from(raw_row).ok() else {
			continue;
		};

		let col = raw_col as u32;

		if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
			continue;
		}

		cells_by_row.entry(row).or_default().push(StyledCell {
			col,
			c: cell.c,
			style: style_of_cell(cell.fg, cell.bg, cell.flags, renderable.colors),
		});
	}

	let mut lines = Vec::new();
	let mut text_runs = Vec::new();

	for (row, mut cells) in cells_by_row {
		cells.sort_by_key(|cell| cell.col);

		let line_text = line_text_from_cells(&cells);

		if !line_text.is_empty() {
			lines.push(TerminalLineSnapshot { row, text: line_text });
		}

		text_runs.extend(styled_runs_from_cells(row, &cells));
	}

	(lines, text_runs)
}

fn line_text_from_cells(cells: &[StyledCell]) -> String {
	let mut text = String::new();
	let mut next_col = 0_u32;

	for cell in cells {
		while next_col < cell.col {
			text.push(' ');
			next_col += 1;
		}

		text.push(cell.c);
		next_col = cell.col + terminal_char_cell_width(cell.c).max(1);
	}

	text.trim_end().to_string()
}

fn styled_runs_from_cells(row: u32, cells: &[StyledCell]) -> Vec<TerminalTextRunSnapshot> {
	let mut runs = Vec::new();

	let mut current_x = 0_u32;
	let mut current_next_x = 0_u32;
	let mut current_text = String::new();
	let mut current_style = None::<TextStyleDto>;

	for cell in cells {
		let style = cell.style;

		if cell.c == ' ' && !style_has_visible_content(style) {
			continue;
		}

		let cell_width = terminal_char_cell_width(cell.c).max(1);
		let is_contiguous = current_style.is_some() && cell.col == current_next_x;

		match current_style {
			None => {
				current_x = cell.col;
				current_next_x = cell.col + cell_width;
				current_text.push(cell.c);
				current_style = Some(style);
			}
			Some(existing_style) if existing_style == style && is_contiguous => {
				current_text.push(cell.c);
				current_next_x = cell.col + cell_width;
			}
			Some(existing_style) => {
				push_run_if_not_blank(&mut runs, current_x, row, &current_text, existing_style);

				current_x = cell.col;
				current_next_x = cell.col + cell_width;
				current_text.clear();
				current_text.push(cell.c);
				current_style = Some(style);
			}
		}
	}

	if let Some(style) = current_style {
		push_run_if_not_blank(&mut runs, current_x, row, &current_text, style);
	}

	runs
}

fn push_run_if_not_blank(
	runs: &mut Vec<TerminalTextRunSnapshot>,
	x: u32,
	y: u32,
	text: &str,
	style: TextStyleDto,
) {
	if text.is_empty() {
		return;
	}

	if text.trim().is_empty() && !style_has_visible_content(style) {
		return;
	}

	runs.push(TerminalTextRunSnapshot { x, y, text: text.to_string(), style });
}

fn style_has_visible_content(style: TextStyleDto) -> bool {
	style.background.is_some() || style.underline || style.bold || style.italic
}

fn style_of_cell(fg: Color, bg: Color, flags: Flags, colors: &Colors) -> TextStyleDto {
	let mut foreground = color_to_rgb(fg, colors);
	let mut background = color_to_rgb(bg, colors);

	if flags.contains(Flags::INVERSE) {
		std::mem::swap(&mut foreground, &mut background);
	}

	TextStyleDto {
		foreground,
		background,
		bold: flags.contains(Flags::BOLD),
		italic: flags.contains(Flags::ITALIC),
		underline: flags.contains(Flags::UNDERLINE),
	}
}

fn color_to_rgb(color: Color, colors: &Colors) -> Option<RgbColorDto> {
	match color {
		Color::Spec(rgb) => Some(rgb_to_dto(rgb)),
		Color::Named(named) => named_color_to_rgb(named, colors),
		Color::Indexed(index) => indexed_color_to_rgb(index, colors),
	}
}

fn named_color_to_rgb(color: NamedColor, colors: &Colors) -> Option<RgbColorDto> {
	if let Some(rgb) = colors[color] {
		return Some(rgb_to_dto(rgb));
	}

	match color {
		NamedColor::Black => Some(RgbColorDto::new(0, 0, 0)),
		NamedColor::Red => Some(RgbColorDto::new(205, 49, 49)),
		NamedColor::Green => Some(RgbColorDto::new(13, 188, 121)),
		NamedColor::Yellow => Some(RgbColorDto::new(229, 229, 16)),
		NamedColor::Blue => Some(RgbColorDto::new(36, 114, 200)),
		NamedColor::Magenta => Some(RgbColorDto::new(188, 63, 188)),
		NamedColor::Cyan => Some(RgbColorDto::new(17, 168, 205)),
		NamedColor::White => Some(RgbColorDto::new(229, 229, 229)),
		NamedColor::BrightBlack => Some(RgbColorDto::new(102, 102, 102)),
		NamedColor::BrightRed => Some(RgbColorDto::new(241, 76, 76)),
		NamedColor::BrightGreen => Some(RgbColorDto::new(35, 209, 139)),
		NamedColor::BrightYellow => Some(RgbColorDto::new(245, 245, 67)),
		NamedColor::BrightBlue => Some(RgbColorDto::new(59, 142, 234)),
		NamedColor::BrightMagenta => Some(RgbColorDto::new(214, 112, 214)),
		NamedColor::BrightCyan => Some(RgbColorDto::new(41, 184, 219)),
		NamedColor::BrightWhite => Some(RgbColorDto::new(255, 255, 255)),
		NamedColor::Foreground => Some(RgbColorDto::new(229, 229, 229)),
		NamedColor::Background => Some(RgbColorDto::new(0, 0, 0)),
		_ => None,
	}
}

fn indexed_color_to_rgb(index: u8, colors: &Colors) -> Option<RgbColorDto> {
	if let Some(rgb) = colors[index as usize] {
		return Some(rgb_to_dto(rgb));
	}

	match index {
		0 => Some(RgbColorDto::new(0, 0, 0)),
		1 => Some(RgbColorDto::new(205, 49, 49)),
		2 => Some(RgbColorDto::new(13, 188, 121)),
		3 => Some(RgbColorDto::new(229, 229, 16)),
		4 => Some(RgbColorDto::new(36, 114, 200)),
		5 => Some(RgbColorDto::new(188, 63, 188)),
		6 => Some(RgbColorDto::new(17, 168, 205)),
		7 => Some(RgbColorDto::new(229, 229, 229)),
		8 => Some(RgbColorDto::new(102, 102, 102)),
		9 => Some(RgbColorDto::new(241, 76, 76)),
		10 => Some(RgbColorDto::new(35, 209, 139)),
		11 => Some(RgbColorDto::new(245, 245, 67)),
		12 => Some(RgbColorDto::new(59, 142, 234)),
		13 => Some(RgbColorDto::new(214, 112, 214)),
		14 => Some(RgbColorDto::new(41, 184, 219)),
		15 => Some(RgbColorDto::new(255, 255, 255)),
		16..=231 => {
			let cube_index = index - 16;

			let red_level = cube_index / 36;
			let green_level = (cube_index % 36) / 6;
			let blue_level = cube_index % 6;

			Some(RgbColorDto::new(
				ansi_256_cube_component(red_level),
				ansi_256_cube_component(green_level),
				ansi_256_cube_component(blue_level),
			))
		}
		232..=255 => {
			let level = 8 + 10 * (index - 232);

			Some(RgbColorDto::new(level, level, level))
		}
	}
}

fn rgb_to_dto(rgb: Rgb) -> RgbColorDto { RgbColorDto::new(rgb.r, rgb.g, rgb.b) }

fn ansi_256_cube_component(level: u8) -> u8 {
	match level {
		0 => 0,
		1..=5 => 55 + 40 * level,
		_ => unreachable!("ANSI 256-color cube level must be 0..=5"),
	}
}

fn dirty_rows_of(damage: TermDamage<'_>, screen_lines: usize) -> Vec<u32> {
	match damage {
		TermDamage::Full => (0..screen_lines as u32).collect(),
		TermDamage::Partial(lines) => {
			let mut rows = BTreeSet::new();

			for line in lines {
				let row = line.line as u32;
				rows.insert(row);

				if row > 0 {
					rows.insert(row - 1);
				}

				if row + 1 < screen_lines as u32 {
					rows.insert(row + 1);
				}
			}

			rows.into_iter().collect()
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn applies_bytes_and_exports_snapshot() {
		let store = AlacrittyTerminalRepository::new();
		let target_id = RenderTargetId::new(1);

		store.apply_bytes(target_id, Seq::new(1), b"\x1b[31mred\x1b[0m\r\nhello\r\nworld");

		let snapshot = store.snapshot_of(target_id).unwrap();

		let texts: Vec<String> = snapshot.lines.iter().map(|line| line.text.clone()).collect();

		assert_eq!(snapshot.latest_seq, Seq::new(1));
		assert!(texts.iter().any(|line| line == "red"));
		assert!(texts.iter().any(|line| line == "hello"));
		assert!(texts.iter().any(|line| line == "world"));
		assert!(!snapshot.dirty_rows.is_empty());
		assert!(!snapshot.text_runs.is_empty());
	}

	#[test]
	fn exports_red_bold_text_run() {
		let store = AlacrittyTerminalRepository::new();
		let target_id = RenderTargetId::new(1);

		store.apply_bytes(target_id, Seq::new(1), b"\x1b[31;1mred\x1b[0m");

		let snapshot = store.snapshot_of(target_id).unwrap();

		let red_run =
			snapshot.text_runs.iter().find(|run| run.text == "red").expect("red run should exist");

		assert_eq!(red_run.x, 0);
		assert_eq!(red_run.y, 0);
		assert!(red_run.style.foreground.is_some());
		assert!(red_run.style.bold);
	}

	#[test]
	fn clear_damage_up_to_resets_full_damage() {
		let store = AlacrittyTerminalRepository::new();
		let target_id = RenderTargetId::new(1);

		store.apply_bytes(target_id, Seq::new(1), b"hello");

		let before = store.snapshot_of(target_id).unwrap();
		assert!(!before.dirty_rows.is_empty());

		store.clear_damage_up_to(target_id, Seq::new(1));

		let after = store.snapshot_of(target_id).unwrap();

		assert!(
			after.dirty_rows.iter().all(|row| *row < store.size().screen_lines() as u32),
			"dirty rows should stay inside screen bounds: {:?}",
			after.dirty_rows,
		);
	}

	#[test]
	fn clear_damage_up_to_does_not_clear_newer_seq() {
		let store = AlacrittyTerminalRepository::new();
		let target_id = RenderTargetId::new(1);

		store.apply_bytes(target_id, Seq::new(2), b"hello");
		store.clear_damage_up_to(target_id, Seq::new(1));

		let snapshot = store.snapshot_of(target_id).unwrap();

		assert_eq!(snapshot.latest_seq, Seq::new(2));
		assert!(!snapshot.dirty_rows.is_empty());
	}

	#[test]
	fn resize_updates_snapshot_bounds() {
		let store = AlacrittyTerminalRepository::new();
		let target_id = RenderTargetId::new(1);

		store.apply_bytes(target_id, Seq::new(1), b"hello");
		store.resize(target_id, Seq::new(2), AlacrittyTermSize::new(100, 40));

		let snapshot = store.snapshot_of(target_id).unwrap();

		assert_eq!(snapshot.latest_seq, Seq::new(2));
		assert!(
			snapshot.dirty_rows.iter().all(|row| *row < 40),
			"dirty rows should stay inside resized screen bounds: {:?}",
			snapshot.dirty_rows,
		);
	}

	#[test]
	fn resolves_default_background_to_black_run() {
		let store = AlacrittyTerminalRepository::new();
		let target_id = RenderTargetId::new(1);

		store.apply_bytes(target_id, Seq::new(1), b"\x1b[40m \x1b[0m");

		let snapshot = store.snapshot_of(target_id).expect("snapshot should exist");
		let run = snapshot
			.text_runs
			.iter()
			.find(|run| run.y == 0 && run.x == 0)
			.expect("background run should exist");

		assert!(!run.text.is_empty());
		assert!(run.text.chars().all(|c| c == ' '));
		assert_eq!(run.style.background, Some(RgbColorDto::new(0, 0, 0)));
	}

	#[test]
	fn expands_partial_damage_to_neighbor_rows() {
		let store = AlacrittyTerminalRepository::with_size(AlacrittyTermSize::new(4, 4));
		let target_id = RenderTargetId::new(1);

		let _ = store.snapshot_of(target_id);

		store.apply_bytes(target_id, Seq::new(1), b"a");

		let snapshot = store.snapshot_of(target_id).expect("snapshot should exist");

		assert!(snapshot.dirty_rows.contains(&0));
		assert!(snapshot.dirty_rows.contains(&1));
	}

	#[test]
	fn inverse_text_swaps_default_foreground_and_background() {
		let store = AlacrittyTerminalRepository::new();
		let target_id = RenderTargetId::new(1);

		store.apply_bytes(target_id, Seq::new(1), b"\x1b[7mfoo\x1b[0m");

		let snapshot = store.snapshot_of(target_id).expect("snapshot should exist");
		let run =
			snapshot.text_runs.iter().find(|run| run.text == "foo").expect("inverse run should exist");

		assert_eq!(run.style.foreground, Some(RgbColorDto::new(0, 0, 0)));
		assert_eq!(run.style.background, Some(RgbColorDto::new(229, 229, 229)));
	}
}
