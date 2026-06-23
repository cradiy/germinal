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
		frame_plan_builder::{
			RenderCommandDto, RgbColorDto, RgbaColorDto, TextStyleDto, decode_pixel_fill_rect_command,
		},
		render_target_id::RenderTargetId,
		renderer_backend::RendererBackend,
		surface_snapshot::{
			RenderSurfaceCursorSnapshot, RenderSurfaceRowSnapshot, RenderSurfaceSnapshot,
		},
	},
	seq::Seq,
};

const CURSOR_OUTLINE_THICKNESS_PX: u32 = 2;
const CURSOR_COLOR: RgbColorDto = RgbColorDto::new(235, 235, 235);
const PIXEL_RECT_VIRTUAL_CELL_WIDTH_PX: u32 = 8;
const PIXEL_RECT_VIRTUAL_CELL_HEIGHT_PX: u32 = 16;

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
		let mut pixel_quads = Vec::new();
		let snapshot_rows: BTreeMap<u32, &_> = snapshot
			.rows
			.iter()
			.filter_map(|row| {
				if append_pixel_rect_quads_from_row(&mut pixel_quads, row, config) {
					None
				} else {
					Some((row.y, row))
				}
			})
			.collect();
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
		let mut quads = Vec::with_capacity(pixel_quads.len() + total_row_quads + cursor_quads.len());
		quads.extend(pixel_quads);
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

fn append_pixel_rect_quads_from_row(
	quads: &mut Vec<WgpuQuadDrawItem>,
	row: &RenderSurfaceRowSnapshot,
	config: WgpuRendererConfig,
) -> bool {
	let mut found = false;
	for run in &row.runs {
		if let Some(RenderCommandDto::PixelFillRect { x_px, y_px, width_px, height_px, color }) =
			decode_pixel_fill_rect_command(&run.text)
		{
			let x_px = config.content_origin_x
				+ scale_virtual_px(x_px, config.cell_width_px, PIXEL_RECT_VIRTUAL_CELL_WIDTH_PX);
			let y_px = config.content_origin_y
				+ scale_virtual_px(y_px, config.cell_height_px, PIXEL_RECT_VIRTUAL_CELL_HEIGHT_PX);
			let width_px =
				scale_virtual_px(width_px, config.cell_width_px, PIXEL_RECT_VIRTUAL_CELL_WIDTH_PX);
			let height_px =
				scale_virtual_px(height_px, config.cell_height_px, PIXEL_RECT_VIRTUAL_CELL_HEIGHT_PX);
			quads.push(WgpuQuadDrawItem::pixel_rect(x_px, y_px, width_px, height_px, color));
			found = true;
		}
	}
	found
}

fn scale_virtual_px(value: u32, actual_cell_px: u32, virtual_cell_px: u32) -> u32 {
	let scaled = u64::from(value) * u64::from(actual_cell_px);
	let rounded = (scaled + u64::from(virtual_cell_px / 2)) / u64::from(virtual_cell_px.max(1));
	rounded.min(u64::from(u32::MAX)) as u32
}

fn render_row(row: &RenderSurfaceRowSnapshot, config: WgpuRendererConfig) -> WgpuRenderedRow {
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
			let glyph = WgpuGlyphDrawItem { x, y: row.y, c, cell_width, style: run.style };
			draw_row.glyphs.push(glyph);
			if run.style.background.is_some() {
				background_quads
					.push(WgpuQuadDrawItem::background(x, row.y, cell_width, config, run.style));
			}
			if c != ' ' {
				glyph_quads.push(WgpuQuadDrawItem::glyph(glyph, config));
			}
			if run.style.underline {
				underline_quads.push(WgpuQuadDrawItem::underline(x, row.y, cell_width, config, run.style));
			}
			x += cell_width;
		}
	}
	WgpuRenderedRow { draw_row, background_quads, glyph_quads, underline_quads }
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

	pub fn pixel_rect_quads(&self) -> Vec<WgpuQuadDrawItem> {
		self
			.quads
			.iter()
			.copied()
			.filter(|quad| matches!(quad.kind, WgpuQuadKind::PixelRect { .. }))
			.collect()
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

	pub fn pixel_rect(
		x_px: u32,
		y_px: u32,
		width_px: u32,
		height_px: u32,
		color: RgbaColorDto,
	) -> Self {
		Self {
			kind: WgpuQuadKind::PixelRect { color },
			x_px,
			y_px,
			width_px: width_px.max(1),
			height_px: height_px.max(1),
			style: TextStyleDto::plain(),
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuQuadKind {
	Background,
	Glyph { c: char, bold: bool },
	Underline,
	PixelRect { color: RgbaColorDto },
}
