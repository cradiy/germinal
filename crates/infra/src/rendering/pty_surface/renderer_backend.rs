use std::{
	cell::RefCell,
	collections::{BTreeMap, BTreeSet},
};

use germinal_ports::{
	pty_host::{
		render_viewport::TerminalRenderViewport, size_info::TerminalSizeInfo,
		width::terminal_char_cell_width,
	},
	rendering::{
		frame_plan_builder::{RgbColorDto, TextStyleDto},
		render_target_id::RenderTargetId,
		renderer_backend::RendererBackend,
		surface_snapshot::{RenderSurfaceCursorSnapshot, RenderSurfaceSnapshot},
	},
	seq::Seq,
};

const CURSOR_OUTLINE_THICKNESS_PX: u32 = 2;
const CURSOR_COLOR: RgbColorDto = RgbColorDto::new(235, 235, 235);

#[derive(Debug, Clone)]
pub struct WgpuRendererBackend {
	inner: RefCell<WgpuRendererState>,
}

impl WgpuRendererBackend {
	pub fn new(config: WgpuRendererConfig) -> Self {
		Self { inner: RefCell::new(WgpuRendererState { config, ..WgpuRendererState::default() }) }
	}

	pub fn config(&self) -> WgpuRendererConfig { self.inner.borrow().config }

	pub fn quads(&self) -> Vec<WgpuQuadDrawItem> { self.inner.borrow().quads.clone() }

	pub fn state(&self) -> WgpuRendererState { self.inner.borrow().clone() }
}

impl RendererBackend for WgpuRendererBackend {
	fn render_surface(&self, snapshot: &RenderSurfaceSnapshot) {
		let mut inner = self.inner.borrow_mut();
		let config = inner.config;
		let full_rerender =
			snapshot.dirty_rows.is_empty() || inner.last_target_id != Some(snapshot.target_id);
		let snapshot_rows: BTreeMap<u32, &_> = snapshot.rows.iter().map(|row| (row.y, row)).collect();
		let dirty_rows: BTreeSet<u32> = if full_rerender {
			snapshot_rows.keys().copied().collect()
		} else {
			snapshot.dirty_rows.iter().copied().collect()
		};

		if full_rerender {
			inner.rendered_rows.clear();
			inner.draw_rows.clear();
		}

		for row_y in dirty_rows {
			if let Some(row) = snapshot_rows.get(&row_y) {
				let rendered_row = render_row(row, config);
				inner.draw_rows.insert(row_y, rendered_row.draw_row.clone());
				inner.rendered_rows.insert(row_y, rendered_row);
			} else {
				inner.draw_rows.remove(&row_y);
				inner.rendered_rows.remove(&row_y);
			}
		}

		let mut cursor_quads = Vec::new();
		if let Some(cursor) = snapshot.cursor {
			append_cursor_quads(&mut cursor_quads, cursor, config);
		}

		let total_row_quads: usize = inner
			.rendered_rows
			.values()
			.map(|row| row.background_quads.len() + row.glyph_quads.len() + row.underline_quads.len())
			.sum();
		let mut quads = Vec::with_capacity(total_row_quads + cursor_quads.len());

		for row in inner.rendered_rows.values() {
			quads.extend(row.background_quads.iter().copied());
		}
		for row in inner.rendered_rows.values() {
			quads.extend(row.glyph_quads.iter().copied());
		}
		for row in inner.rendered_rows.values() {
			quads.extend(row.underline_quads.iter().copied());
		}
		quads.extend(cursor_quads);

		inner.render_count += 1;
		inner.last_target_id = Some(snapshot.target_id);
		inner.last_seq = Some(snapshot.latest_seq);
		inner.quads = quads;
	}
}

fn render_row(
	row: &germinal_ports::rendering::surface_snapshot::RenderSurfaceRowSnapshot,
	config: WgpuRendererConfig,
) -> WgpuRenderedRow {
	let mut draw_row = WgpuDrawRow { y: row.y, glyphs: Vec::new() };
	let mut background_quads = Vec::new();
	let mut glyph_quads = Vec::new();
	let mut underline_quads = Vec::new();

	for run in &row.runs {
		let mut x = run.x;

		for c in run.text.chars() {
			let cell_width = terminal_char_cell_width(c);

			if cell_width == 0 {
				continue;
			}

			let y = row.y;
			let glyph = WgpuGlyphDrawItem { x, y, c, cell_width, style: run.style };

			draw_row.glyphs.push(glyph);

			if run.style.background.is_some() {
				background_quads.push(WgpuQuadDrawItem::background(x, y, cell_width, config, run.style));
			}

			if is_builtin_box_drawing(c) {
				append_box_drawing_quads(&mut glyph_quads, glyph, config);
			} else if is_builtin_block_element(c) {
				append_block_element_quads(&mut glyph_quads, glyph, config);
			} else if c != ' ' {
				glyph_quads.push(WgpuQuadDrawItem::glyph(glyph, config));
			}

			if run.style.underline {
				underline_quads.push(WgpuQuadDrawItem::underline(x, y, cell_width, config, run.style));
			}

			x += cell_width;
		}
	}

	WgpuRenderedRow { draw_row, background_quads, glyph_quads, underline_quads }
}

fn is_builtin_box_drawing(c: char) -> bool {
	matches!(
		c,
		'─'
			| '│'
			| '┌'
			| '┐'
			| '└'
			| '┘'
			| '├'
			| '┤'
			| '┬'
			| '┴'
			| '┼'
			| '╭'
			| '╮'
			| '╰'
			| '╯'
	)
}

fn append_box_drawing_quads(
	quads: &mut Vec<WgpuQuadDrawItem>,
	glyph: WgpuGlyphDrawItem,
	config: WgpuRendererConfig,
) {
	let x = glyph.pixel_x(config);
	let y = glyph.pixel_y(config);
	let w = glyph.pixel_width(config);
	let h = config.cell_height_px;
	let thickness = (config.cell_width_px.min(config.cell_height_px) / 8).max(1);
	let cx = x + w / 2;
	let cy = y + h / 2;

	let (left, right, up, down) = box_drawing_connections(glyph.c);

	if left {
		quads.push(WgpuQuadDrawItem::solid_rect(
			x,
			cy.saturating_sub(thickness / 2),
			(w / 2 + thickness / 2).max(1),
			thickness,
			glyph.style,
		));
	}

	if right {
		quads.push(WgpuQuadDrawItem::solid_rect(
			cx.saturating_sub(thickness / 2),
			cy.saturating_sub(thickness / 2),
			(w - w / 2 + thickness / 2).max(1),
			thickness,
			glyph.style,
		));
	}

	if up {
		quads.push(WgpuQuadDrawItem::solid_rect(
			cx.saturating_sub(thickness / 2),
			y,
			thickness,
			(h / 2 + thickness / 2).max(1),
			glyph.style,
		));
	}

	if down {
		quads.push(WgpuQuadDrawItem::solid_rect(
			cx.saturating_sub(thickness / 2),
			cy.saturating_sub(thickness / 2),
			thickness,
			(h - h / 2 + thickness / 2).max(1),
			glyph.style,
		));
	}
}

fn append_cursor_quads(
	quads: &mut Vec<WgpuQuadDrawItem>,
	cursor: RenderSurfaceCursorSnapshot,
	config: WgpuRendererConfig,
) {
	let x = config.content_origin_x + cursor.x * config.cell_width_px;
	let y = config.content_origin_y + cursor.y * config.cell_height_px;
	let w = config.cell_width_px.max(1);
	let h = config.cell_height_px.max(1);
	let style = TextStyleDto {
		foreground: Some(CURSOR_COLOR),
		background: None,
		bold:       false,
		italic:     false,
		underline:  false,
	};

	if cursor.focused {
		quads.push(WgpuQuadDrawItem::solid_rect(x, y, w, h, style));
		return;
	}

	let thickness = CURSOR_OUTLINE_THICKNESS_PX.min(w).min(h).max(1);
	quads.push(WgpuQuadDrawItem::solid_rect(x, y, w, thickness, style));
	quads.push(WgpuQuadDrawItem::solid_rect(x, y + h.saturating_sub(thickness), w, thickness, style));
	quads.push(WgpuQuadDrawItem::solid_rect(x, y, thickness, h, style));
	quads.push(WgpuQuadDrawItem::solid_rect(x + w.saturating_sub(thickness), y, thickness, h, style));
}

fn box_drawing_connections(c: char) -> (bool, bool, bool, bool) {
	match c {
		'─' => (true, true, false, false),
		'│' => (false, false, true, true),
		'┌' | '╭' => (false, true, false, true),
		'┐' | '╮' => (true, false, false, true),
		'└' | '╰' => (false, true, true, false),
		'┘' | '╯' => (true, false, true, false),
		'├' => (false, true, true, true),
		'┤' => (true, false, true, true),
		'┬' => (true, true, false, true),
		'┴' => (true, true, true, false),
		'┼' => (true, true, true, true),
		_ => (false, false, false, false),
	}
}

fn is_builtin_block_element(c: char) -> bool {
	matches!(
		c,
		'▀'
			| '▄'
			| '█'
			| '▁'
			| '▂'
			| '▃'
			| '▅'
			| '▆'
			| '▇'
			| '▉'
			| '▊'
			| '▋'
			| '▌'
			| '▍'
			| '▎'
			| '▏'
			| '▐'
			| '▔'
			| '▕'
			| '▖'
			| '▗'
			| '▘'
			| '▙'
			| '▚'
			| '▛'
			| '▜'
			| '▝'
			| '▞'
			| '▟'
			| '🮂'
			| '🮃'
			| '🮄'
			| '🮅'
			| '🮆'
			| '🮇'
			| '🮈'
			| '🮉'
			| '🮊'
			| '🮋'
	)
}

fn append_block_element_quads(
	quads: &mut Vec<WgpuQuadDrawItem>,
	glyph: WgpuGlyphDrawItem,
	config: WgpuRendererConfig,
) {
	let x = glyph.pixel_x(config);
	let y = glyph.pixel_y(config);
	let w = glyph.pixel_width(config);
	let h = config.cell_height_px;

	match glyph.c {
		'█' => quads.push(WgpuQuadDrawItem::solid_rect(x, y, w, h, glyph.style)),
		'▀' => quads.push(WgpuQuadDrawItem::solid_rect(x, y, w, h.div_ceil(2), glyph.style)),
		'▄' => {
			let fill_h = h.div_ceil(2);
			quads.push(WgpuQuadDrawItem::solid_rect(
				x,
				y + h.saturating_sub(fill_h),
				w,
				fill_h,
				glyph.style,
			));
		}
		'▐' => {
			let fill_w = w.div_ceil(2);
			quads.push(WgpuQuadDrawItem::solid_rect(
				x + w.saturating_sub(fill_w),
				y,
				fill_w,
				h,
				glyph.style,
			));
		}
		'▔' => quads.push(WgpuQuadDrawItem::solid_rect(x, y, w, h.div_ceil(8), glyph.style)),
		'▕' => {
			let fill_w = w.div_ceil(8);
			quads.push(WgpuQuadDrawItem::solid_rect(
				x + w.saturating_sub(fill_w),
				y,
				fill_w,
				h,
				glyph.style,
			));
		}
		'▖' => {
			let fill_w = w.div_ceil(2);
			let fill_h = h.div_ceil(2);
			quads.push(WgpuQuadDrawItem::solid_rect(
				x,
				y + h.saturating_sub(fill_h),
				fill_w,
				fill_h,
				glyph.style,
			));
		}
		'▗' => {
			let fill_w = w.div_ceil(2);
			let fill_h = h.div_ceil(2);
			quads.push(WgpuQuadDrawItem::solid_rect(
				x + w.saturating_sub(fill_w),
				y + h.saturating_sub(fill_h),
				fill_w,
				fill_h,
				glyph.style,
			));
		}
		'▘' => {
			let fill_w = w.div_ceil(2);
			let fill_h = h.div_ceil(2);
			quads.push(WgpuQuadDrawItem::solid_rect(x, y, fill_w, fill_h, glyph.style));
		}
		'▝' => {
			let fill_w = w.div_ceil(2);
			let fill_h = h.div_ceil(2);
			quads.push(WgpuQuadDrawItem::solid_rect(
				x + w.saturating_sub(fill_w),
				y,
				fill_w,
				fill_h,
				glyph.style,
			));
		}
		'▚' => {
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▘', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▗', ..glyph }, config);
		}
		'▞' => {
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▝', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▖', ..glyph }, config);
		}
		'▙' => {
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▘', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▖', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▗', ..glyph }, config);
		}
		'▛' => {
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▘', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▖', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▝', ..glyph }, config);
		}
		'▜' => {
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▘', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▝', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▗', ..glyph }, config);
		}
		'▟' => {
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▝', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▖', ..glyph }, config);
			append_block_element_quads(quads, WgpuGlyphDrawItem { c: '▗', ..glyph }, config);
		}
		'🮂' | '🮃' | '🮄' | '🮅' | '🮆' => {
			let eighths = match glyph.c {
				'🮂' => 2,
				'🮃' => 3,
				'🮄' => 5,
				'🮅' => 6,
				'🮆' => 7,
				_ => unreachable!(),
			};
			let fill_h = (h * eighths).div_ceil(8);
			quads.push(WgpuQuadDrawItem::solid_rect(x, y, w, fill_h, glyph.style));
		}
		'🮇' | '🮈' | '🮉' | '🮊' | '🮋' => {
			let eighths = match glyph.c {
				'🮇' => 2,
				'🮈' => 3,
				'🮉' => 5,
				'🮊' => 6,
				'🮋' => 7,
				_ => unreachable!(),
			};
			let fill_w = (w * eighths).div_ceil(8);
			quads.push(WgpuQuadDrawItem::solid_rect(
				x + w.saturating_sub(fill_w),
				y,
				fill_w,
				h,
				glyph.style,
			));
		}
		'▁' | '▂' | '▃' | '▅' | '▆' | '▇' => {
			let eighths = match glyph.c {
				'▁' => 1,
				'▂' => 2,
				'▃' => 3,
				'▅' => 5,
				'▆' => 6,
				'▇' => 7,
				_ => unreachable!(),
			};
			let fill_h = (h * eighths).div_ceil(8);
			quads.push(WgpuQuadDrawItem::solid_rect(
				x,
				y + h.saturating_sub(fill_h),
				w,
				fill_h,
				glyph.style,
			));
		}
		'▉' | '▊' | '▋' | '▌' | '▍' | '▎' | '▏' => {
			let eighths = match glyph.c {
				'▉' => 7,
				'▊' => 6,
				'▋' => 5,
				'▌' => 4,
				'▍' => 3,
				'▎' => 2,
				'▏' => 1,
				_ => unreachable!(),
			};
			let fill_w = (w * eighths).div_ceil(8);
			quads.push(WgpuQuadDrawItem::solid_rect(x, y, fill_w, h, glyph.style));
		}
		_ => {}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuRendererConfig {
	pub cell_width_px:    u32,
	pub cell_height_px:   u32,
	pub content_origin_x: u32,
	pub content_origin_y: u32,
}

impl WgpuRendererConfig {
	pub fn from_render_viewport(viewport: TerminalRenderViewport) -> Self {
		let cell_size = viewport.cell_size();

		Self {
			cell_width_px:    cell_size.width_px(),
			cell_height_px:   cell_size.height_px(),
			content_origin_x: viewport.origin_x_px(),
			content_origin_y: viewport.origin_y_px(),
		}
	}
}

impl From<TerminalRenderViewport> for WgpuRendererConfig {
	fn from(viewport: TerminalRenderViewport) -> Self { Self::from_render_viewport(viewport) }
}

impl From<TerminalSizeInfo> for WgpuRendererConfig {
	fn from(size_info: TerminalSizeInfo) -> Self {
		Self::from_render_viewport(size_info.render_viewport())
	}
}

impl Default for WgpuRendererConfig {
	fn default() -> Self {
		Self { cell_width_px: 8, cell_height_px: 16, content_origin_x: 0, content_origin_y: 0 }
	}
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WgpuRendererState {
	pub config:         WgpuRendererConfig,
	pub render_count:   u64,
	pub last_target_id: Option<RenderTargetId>,
	pub last_seq:       Option<Seq>,
	rendered_rows:      BTreeMap<u32, WgpuRenderedRow>,
	draw_rows:          BTreeMap<u32, WgpuDrawRow>,
	quads:              Vec<WgpuQuadDrawItem>,
}

impl WgpuRendererState {
	pub fn row(&self, y: u32) -> Option<&WgpuDrawRow> { self.draw_rows.get(&y) }

	pub fn rows(&self) -> &BTreeMap<u32, WgpuDrawRow> { &self.draw_rows }

	pub fn glyphs(&self) -> Vec<WgpuGlyphDrawItem> {
		self.draw_rows.values().flat_map(|row| row.glyphs.iter().copied()).collect()
	}

	pub fn quads(&self) -> &[WgpuQuadDrawItem] { &self.quads }

	pub fn background_quads(&self) -> Vec<WgpuQuadDrawItem> {
		self.quads.iter().copied().filter(|quad| quad.kind == WgpuQuadKind::Background).collect()
	}

	pub fn glyph_quads(&self) -> Vec<WgpuQuadDrawItem> {
		self
			.quads
			.iter()
			.copied()
			.filter(|quad| matches!(quad.kind, WgpuQuadKind::Glyph { .. }))
			.collect()
	}

	pub fn underline_quads(&self) -> Vec<WgpuQuadDrawItem> {
		self.quads.iter().copied().filter(|quad| quad.kind == WgpuQuadKind::Underline).collect()
	}

	pub fn line_texts(&self) -> Vec<String> {
		self.draw_rows.values().map(|row| row.text()).collect()
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WgpuRenderedRow {
	draw_row:         WgpuDrawRow,
	background_quads: Vec<WgpuQuadDrawItem>,
	glyph_quads:      Vec<WgpuQuadDrawItem>,
	underline_quads:  Vec<WgpuQuadDrawItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuDrawRow {
	pub y:  u32,
	glyphs: Vec<WgpuGlyphDrawItem>,
}

impl WgpuDrawRow {
	pub fn glyphs(&self) -> &[WgpuGlyphDrawItem] { &self.glyphs }

	pub fn text(&self) -> String {
		let mut chars = Vec::new();

		for glyph in &self.glyphs {
			let index = glyph.x as usize;

			while chars.len() < index {
				chars.push(' ');
			}

			if index < chars.len() {
				chars[index] = glyph.c;
			} else {
				chars.push(glyph.c);
			}

			if glyph.cell_width > 1 {
				while chars.len() < index + glyph.cell_width as usize {
					chars.push(' ');
				}
			}
		}

		chars.into_iter().collect()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuGlyphDrawItem {
	pub x:          u32,
	pub y:          u32,
	pub c:          char,
	pub cell_width: u32,
	pub style:      TextStyleDto,
}

impl WgpuGlyphDrawItem {
	pub fn pixel_x(&self, config: WgpuRendererConfig) -> u32 {
		config.content_origin_x + self.x * config.cell_width_px
	}

	pub fn pixel_y(&self, config: WgpuRendererConfig) -> u32 {
		config.content_origin_y + self.y * config.cell_height_px
	}

	pub fn pixel_width(&self, config: WgpuRendererConfig) -> u32 {
		self.cell_width.max(1) * config.cell_width_px
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuQuadDrawItem {
	pub kind:      WgpuQuadKind,
	pub x_px:      u32,
	pub y_px:      u32,
	pub width_px:  u32,
	pub height_px: u32,
	pub style:     TextStyleDto,
}

impl WgpuQuadDrawItem {
	pub fn glyph(glyph: WgpuGlyphDrawItem, config: WgpuRendererConfig) -> Self {
		Self {
			kind:      WgpuQuadKind::Glyph { c: glyph.c, bold: glyph.style.bold },
			x_px:      glyph.pixel_x(config),
			y_px:      glyph.pixel_y(config),
			width_px:  glyph.pixel_width(config),
			height_px: config.cell_height_px,
			style:     glyph.style,
		}
	}

	pub fn background(
		x: u32,
		y: u32,
		cell_width: u32,
		config: WgpuRendererConfig,
		style: TextStyleDto,
	) -> Self {
		Self {
			kind: WgpuQuadKind::Background,
			x_px: config.content_origin_x + x * config.cell_width_px,
			y_px: config.content_origin_y + y * config.cell_height_px,
			width_px: cell_width.max(1) * config.cell_width_px,
			height_px: config.cell_height_px,
			style,
		}
	}

	pub fn underline(
		x: u32,
		y: u32,
		cell_width: u32,
		config: WgpuRendererConfig,
		style: TextStyleDto,
	) -> Self {
		Self {
			kind: WgpuQuadKind::Underline,
			x_px: config.content_origin_x + x * config.cell_width_px,
			y_px: config.content_origin_y
				+ y * config.cell_height_px
				+ config.cell_height_px.saturating_sub(2),
			width_px: cell_width.max(1) * config.cell_width_px,
			height_px: 1,
			style,
		}
	}

	pub fn solid_rect(
		x_px: u32,
		y_px: u32,
		width_px: u32,
		height_px: u32,
		style: TextStyleDto,
	) -> Self {
		Self {
			kind: WgpuQuadKind::Underline,
			x_px,
			y_px,
			width_px: width_px.max(1),
			height_px: height_px.max(1),
			style,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuQuadKind {
	Background,
	Glyph { c: char, bold: bool },
	Underline,
}

#[cfg(test)]
mod tests {
	use germinal_ports::rendering::{
		frame_plan_builder::{RgbColorDto, TextStyleDto},
		surface_snapshot::{RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot},
	};

	use super::*;

	#[test]
	fn converts_surface_snapshot_to_glyph_draw_items() {
		let backend = WgpuRendererBackend::new(WgpuRendererConfig {
			cell_width_px:    9,
			cell_height_px:   18,
			content_origin_x: 0,
			content_origin_y: 0,
		});

		let target_id = RenderTargetId::new(1);

		let red = TextStyleDto {
			foreground: Some(RgbColorDto::new(255, 0, 0)),
			background: None,
			bold:       true,
			italic:     false,
			underline:  false,
		};

		backend.render_surface(&RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(7),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    2,
				runs: vec![RenderSurfaceRunSnapshot { x: 4, text: "red".to_string(), style: red }],
			}],
			dirty_rows: Vec::new(),
			cursor: None,
		});

		let state = backend.state();

		assert_eq!(state.render_count, 1);
		assert_eq!(state.last_target_id, Some(target_id));
		assert_eq!(state.last_seq, Some(Seq::new(7)));
		assert_eq!(state.config.cell_width_px, 9);
		assert_eq!(state.config.cell_height_px, 18);

		let row = state.row(2).expect("row 2 should exist");

		assert_eq!(row.text(), "    red");
		assert_eq!(row.glyphs().len(), 3);

		assert_eq!(row.glyphs()[0], WgpuGlyphDrawItem {
			x:          4,
			y:          2,
			c:          'r',
			cell_width: 1,
			style:      red,
		});

		assert_eq!(row.glyphs()[0].pixel_x(state.config), 36);
		assert_eq!(row.glyphs()[0].pixel_y(state.config), 36);

		let glyph_quads = state.glyph_quads();

		assert_eq!(glyph_quads.len(), 3);
		assert_eq!(glyph_quads[0].kind, WgpuQuadKind::Glyph { c: 'r', bold: true });
		assert_eq!(glyph_quads[0].x_px, 36);
		assert_eq!(glyph_quads[0].y_px, 36);
		assert_eq!(glyph_quads[0].width_px, 9);
		assert_eq!(glyph_quads[0].height_px, 18);
		assert_eq!(glyph_quads[0].style, red);
	}

	#[test]
	fn preserves_styles_per_glyph() {
		let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
		let target_id = RenderTargetId::new(1);

		let underline = TextStyleDto {
			foreground: None,
			background: None,
			bold:       false,
			italic:     false,
			underline:  true,
		};

		backend.render_surface(&RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![RenderSurfaceRunSnapshot {
					x:     0,
					text:  "under".to_string(),
					style: underline,
				}],
			}],
			dirty_rows: Vec::new(),
			cursor: None,
		});

		let state = backend.state();
		let glyphs = state.glyphs();

		assert_eq!(glyphs.len(), 5);
		assert!(glyphs.iter().all(|glyph| glyph.style.underline));
		assert!(glyphs.iter().all(|glyph| !glyph.style.bold));

		let underline_quads = state.underline_quads();

		assert_eq!(underline_quads.len(), 5);
		assert!(underline_quads.iter().all(|quad| quad.kind == WgpuQuadKind::Underline));
		assert!(underline_quads.iter().all(|quad| quad.height_px == 1));
	}

	#[test]
	fn later_run_overwrites_text_when_rows_are_read_back() {
		let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
		let target_id = RenderTargetId::new(1);

		backend.render_surface(&RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![
					RenderSurfaceRunSnapshot {
						x:     0,
						text:  "hello".to_string(),
						style: TextStyleDto::plain(),
					},
					RenderSurfaceRunSnapshot {
						x:     1,
						text:  "a".to_string(),
						style: TextStyleDto::plain(),
					},
				],
			}],
			dirty_rows: Vec::new(),
			cursor: None,
		});

		let state = backend.state();
		let row = state.row(0).expect("row 0 should exist");

		assert_eq!(row.text(), "hallo");
	}

	#[test]
	fn dirty_row_render_keeps_untouched_rows() {
		let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
		let target_id = RenderTargetId::new(1);

		backend.render_surface(&RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![
				RenderSurfaceRowSnapshot {
					y:    0,
					runs: vec![RenderSurfaceRunSnapshot {
						x:     0,
						text:  "hello".to_string(),
						style: TextStyleDto::plain(),
					}],
				},
				RenderSurfaceRowSnapshot {
					y:    1,
					runs: vec![RenderSurfaceRunSnapshot {
						x:     0,
						text:  "world".to_string(),
						style: TextStyleDto::plain(),
					}],
				},
			],
			dirty_rows: Vec::new(),
			cursor: None,
		});

		backend.render_surface(&RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(2),
			rows: vec![
				RenderSurfaceRowSnapshot {
					y:    0,
					runs: vec![RenderSurfaceRunSnapshot {
						x:     0,
						text:  "hello".to_string(),
						style: TextStyleDto::plain(),
					}],
				},
				RenderSurfaceRowSnapshot {
					y:    1,
					runs: vec![RenderSurfaceRunSnapshot {
						x:     0,
						text:  "there".to_string(),
						style: TextStyleDto::plain(),
					}],
				},
			],
			dirty_rows: vec![1],
			cursor: None,
		});

		let state = backend.state();

		assert_eq!(state.row(0).expect("row 0 should exist").text(), "hello");
		assert_eq!(state.row(1).expect("row 1 should exist").text(), "there");
	}

	#[test]
	fn generates_background_glyph_and_underline_quads() {
		let backend = WgpuRendererBackend::new(WgpuRendererConfig {
			cell_width_px:    10,
			cell_height_px:   20,
			content_origin_x: 0,
			content_origin_y: 0,
		});

		let target_id = RenderTargetId::new(1);

		let style = TextStyleDto {
			foreground: Some(RgbColorDto::new(255, 0, 0)),
			background: Some(RgbColorDto::new(0, 0, 255)),
			bold:       true,
			italic:     false,
			underline:  true,
		};

		backend.render_surface(&RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    3,
				runs: vec![RenderSurfaceRunSnapshot { x: 2, text: "ab".to_string(), style }],
			}],
			dirty_rows: Vec::new(),
			cursor: None,
		});

		let state = backend.state();

		let background_quads = state.background_quads();
		let glyph_quads = state.glyph_quads();
		let underline_quads = state.underline_quads();

		assert_eq!(background_quads.len(), 2);
		assert_eq!(glyph_quads.len(), 2);
		assert_eq!(underline_quads.len(), 2);

		assert_eq!(background_quads[0], WgpuQuadDrawItem {
			kind: WgpuQuadKind::Background,
			x_px: 20,
			y_px: 60,
			width_px: 10,
			height_px: 20,
			style
		});

		assert_eq!(glyph_quads[0], WgpuQuadDrawItem {
			kind: WgpuQuadKind::Glyph { c: 'a', bold: true },
			x_px: 20,
			y_px: 60,
			width_px: 10,
			height_px: 20,
			style
		});

		assert_eq!(underline_quads[0], WgpuQuadDrawItem {
			kind: WgpuQuadKind::Underline,
			x_px: 20,
			y_px: 78,
			width_px: 10,
			height_px: 1,
			style
		});

		assert_eq!(state.quads().len(), 6);

		assert_eq!(state.quads()[0].kind, WgpuQuadKind::Background);
		assert_eq!(state.quads()[1].kind, WgpuQuadKind::Background);
		assert_eq!(state.quads()[2].kind, WgpuQuadKind::Glyph { c: 'a', bold: true });
		assert_eq!(state.quads()[3].kind, WgpuQuadKind::Glyph { c: 'b', bold: true });
		assert_eq!(state.quads()[4].kind, WgpuQuadKind::Underline);
		assert_eq!(state.quads()[5].kind, WgpuQuadKind::Underline);
	}

	#[test]
	fn renders_block_elements_as_crisp_rects_instead_of_font_glyphs() {
		let backend = WgpuRendererBackend::new(WgpuRendererConfig {
			cell_width_px:    8,
			cell_height_px:   16,
			content_origin_x: 0,
			content_origin_y: 0,
		});

		let style = TextStyleDto {
			foreground: Some(RgbColorDto::new(255, 255, 255)),
			background: None,
			bold:       false,
			italic:     false,
			underline:  false,
		};

		backend.render_surface(&RenderSurfaceSnapshot {
			target_id:  RenderTargetId::new(1),
			latest_seq: Seq::new(1),
			rows:       vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![RenderSurfaceRunSnapshot { x: 0, text: "▄".to_string(), style }],
			}],
			dirty_rows: Vec::new(),
			cursor:     None,
		});

		let state = backend.state();

		assert!(state.glyph_quads().is_empty());

		let block_quads = state.underline_quads();
		assert_eq!(block_quads.len(), 1);
		assert_eq!(block_quads[0].x_px, 0);
		assert_eq!(block_quads[0].y_px, 8);
		assert_eq!(block_quads[0].width_px, 8);
		assert_eq!(block_quads[0].height_px, 8);
	}

	#[test]
	fn renders_upper_quarter_legacy_block_as_top_aligned_rect() {
		let backend = WgpuRendererBackend::new(WgpuRendererConfig {
			cell_width_px:    8,
			cell_height_px:   16,
			content_origin_x: 0,
			content_origin_y: 0,
		});

		let style = TextStyleDto::plain();

		backend.render_surface(&RenderSurfaceSnapshot {
			target_id:  RenderTargetId::new(1),
			latest_seq: Seq::new(1),
			rows:       vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![RenderSurfaceRunSnapshot { x: 0, text: "🮂".to_string(), style }],
			}],
			dirty_rows: Vec::new(),
			cursor:     None,
		});

		let state = backend.state();
		assert!(state.glyph_quads().is_empty());

		let block_quads = state.underline_quads();
		assert_eq!(block_quads.len(), 1);
		assert_eq!(block_quads[0].x_px, 0);
		assert_eq!(block_quads[0].y_px, 0);
		assert_eq!(block_quads[0].width_px, 8);
		assert_eq!(block_quads[0].height_px, 4);
	}
}
