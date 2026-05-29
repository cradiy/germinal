use std::{
	cell::RefCell,
	collections::{BTreeSet, HashMap},
	sync::Arc,
};

use crossfont::{
	BitmapBuffer, FontDesc, FontKey, GlyphKey, Rasterize, Rasterizer, Size, Slant, Style, Weight,
};
use germinal_domain::pty_host::width::terminal_char_cell_width;

use crate::rendering::pty_surface::glyph_atlas::{
	WgpuTerminalGlyphAtlas, WgpuTerminalGlyphAtlasEntry, WgpuTerminalGlyphUvRect,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WgpuCrossfontGlyphAtlasError {
	Rasterizer(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuCrossfontCellMetrics {
	cell_width_px:  u32,
	cell_height_px: u32,
	baseline_y_px:  i32,
}

impl WgpuCrossfontCellMetrics {
	pub const fn new(cell_width_px: u32, cell_height_px: u32, baseline_y_px: i32) -> Self {
		Self { cell_width_px, cell_height_px, baseline_y_px }
	}

	pub const fn cell_width_px(self) -> u32 { self.cell_width_px }

	pub const fn cell_height_px(self) -> u32 { self.cell_height_px }

	pub const fn baseline_y_px(self) -> i32 { self.baseline_y_px }
}

#[derive(Clone)]
pub struct WgpuCrossfontGlyphAtlasBuilder {
	font_family:    String,
	font_size_px:   f32,
	padding_px:     u32,
	columns:        u32,
	cell_width_px:  Option<u32>,
	cell_height_px: Option<u32>,
	backend:        Arc<RefCell<Option<WgpuCrossfontGlyphBackend>>>,
}

impl std::fmt::Debug for WgpuCrossfontGlyphAtlasBuilder {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("WgpuCrossfontGlyphAtlasBuilder")
			.field("font_family", &self.font_family)
			.field("font_size_px", &self.font_size_px)
			.field("padding_px", &self.padding_px)
			.field("columns", &self.columns)
			.field("cell_width_px", &self.cell_width_px)
			.field("cell_height_px", &self.cell_height_px)
			.finish()
	}
}

impl WgpuCrossfontGlyphAtlasBuilder {
	pub fn new(
		font_family: impl Into<String>,
		font_size_px: f32,
	) -> Result<Self, WgpuCrossfontGlyphAtlasError> {
		let font_family = font_family.into();
		let backend = WgpuCrossfontGlyphBackend::new(font_family.clone(), font_size_px)?;

		Ok(Self {
			font_family,
			font_size_px,
			padding_px: 1,
			columns: 16,
			cell_width_px: None,
			cell_height_px: None,
			backend: Arc::new(RefCell::new(Some(backend))),
		})
	}

	pub fn with_padding_px(mut self, padding_px: u32) -> Self {
		self.padding_px = padding_px;
		self
	}

	pub fn with_columns(mut self, columns: u32) -> Self {
		self.columns = columns.max(1);
		self
	}

	pub fn with_cell_size_px(mut self, cell_width_px: u32, cell_height_px: u32) -> Self {
		self.cell_width_px = Some(cell_width_px.max(1));
		self.cell_height_px = Some(cell_height_px.max(1));
		self
	}

	pub fn load_cell_metrics(
		font_family: impl Into<String>,
		font_size_px: f32,
	) -> Result<WgpuCrossfontCellMetrics, WgpuCrossfontGlyphAtlasError> {
		let backend = WgpuCrossfontGlyphBackend::new(font_family.into(), font_size_px)?;

		Ok(WgpuCrossfontCellMetrics::new(
			backend.base_cell_width_px().max(1),
			backend.base_cell_height_px().max(1),
			backend.baseline_y_px(),
		))
	}

	pub fn font_family(&self) -> &str { &self.font_family }

	pub fn font_size_px(&self) -> f32 { self.font_size_px }

	pub fn padding_px(&self) -> u32 { self.padding_px }

	pub fn columns(&self) -> u32 { self.columns }

	pub fn cell_width_px(&self) -> Option<u32> { self.cell_width_px }

	pub fn cell_height_px(&self) -> Option<u32> { self.cell_height_px }

	pub fn build_for_texts<I, S>(&self, texts: I) -> WgpuTerminalGlyphAtlas
	where
		I: IntoIterator<Item = S>,
		S: AsRef<str>,
	{
		let mut chars = BTreeSet::new();

		for text in texts {
			for c in text.as_ref().chars() {
				if terminal_char_cell_width(c) > 0 {
					chars.insert(c);
				}
			}
		}

		self.build_for_chars(chars)
	}

	pub fn build_for_chars<I>(&self, chars: I) -> WgpuTerminalGlyphAtlas
	where I: IntoIterator<Item = char> {
		let chars: Vec<char> = chars.into_iter().filter(|c| terminal_char_cell_width(*c) > 0).collect();

		if chars.is_empty() {
			return WgpuTerminalGlyphAtlas::empty();
		}

		let mut backend_ref = self.backend.borrow_mut();
		let Some(backend) = backend_ref.as_mut() else {
			return WgpuTerminalGlyphAtlas::empty();
		};

		let base_cell_width = self.cell_width_px.unwrap_or_else(|| backend.base_cell_width_px().max(1));
		let base_cell_height =
			self.cell_height_px.unwrap_or_else(|| backend.base_cell_height_px().max(1));
		let baseline_y_px = backend.baseline_y_px();

		let glyphs: Vec<RasterizedTerminalGlyph> =
			chars.into_iter().map(|c| backend.rasterize_terminal_glyph(c)).collect();

		build_atlas_from_rasterized_glyphs(
			glyphs,
			base_cell_width,
			base_cell_height,
			baseline_y_px,
			self.padding_px,
			self.columns,
		)
	}
}

struct WgpuCrossfontGlyphBackend {
	rasterizer:         Rasterizer,
	font_key:           FontKey,
	emoji_font_key:     Option<FontKey>,
	size:               Size,
	average_advance_px: u32,
	line_height_px:     u32,
	baseline_y_px:      i32,
	glyph_cache:        HashMap<char, RasterizedTerminalGlyph>,
}

impl WgpuCrossfontGlyphBackend {
	fn new(font_family: String, font_size_px: f32) -> Result<Self, WgpuCrossfontGlyphAtlasError> {
		let mut rasterizer = Rasterizer::new()
			.map_err(|err| WgpuCrossfontGlyphAtlasError::Rasterizer(format!("{err:?}")))?;
		let size = Size::new(font_size_px);
		let font_desc = FontDesc::new(font_family, Style::Description {
			slant:  Slant::Normal,
			weight: Weight::Normal,
		});
		let font_key = rasterizer
			.load_font(&font_desc, size)
			.map_err(|err| WgpuCrossfontGlyphAtlasError::Rasterizer(format!("{err:?}")))?;
		let emoji_font_key = load_optional_font(&mut rasterizer, "Noto Color Emoji", size);

		// Match Alacritty's GlyphCache::load_font_metrics: load one glyph
		// from the face before asking crossfont for metrics.  Some backends
		// only finalize size/face metrics after the first glyph load; reading
		// metrics before that can produce a too-small cell advance, which makes
		// terminal columns collapse and text overlap.
		rasterizer
			.get_glyph(GlyphKey { font_key, character: 'm', size })
			.map_err(|err| WgpuCrossfontGlyphAtlasError::Rasterizer(format!("{err:?}")))?;

		let metrics = rasterizer.metrics(font_key, size).ok();
		let average_advance_px = metrics
			.as_ref()
			.map(|metrics| alacritty_cell_axis_px(metrics.average_advance))
			.unwrap_or(1)
			.max(1);
		let line_height_px = metrics
			.as_ref()
			.map(|metrics| alacritty_cell_axis_px(metrics.line_height))
			.unwrap_or(1)
			.max(1);
		let baseline_y_px = metrics
			.as_ref()
			.map(|metrics| alacritty_baseline_y_px(line_height_px, metrics.descent))
			.unwrap_or_else(|| ((line_height_px as f32) * 0.80).round() as i32);

		Ok(Self {
			rasterizer,
			font_key,
			emoji_font_key,
			size,
			average_advance_px,
			line_height_px,
			baseline_y_px,
			glyph_cache: HashMap::new(),
		})
	}

	fn base_cell_width_px(&self) -> u32 { self.average_advance_px }

	fn base_cell_height_px(&self) -> u32 { self.line_height_px }

	fn baseline_y_px(&self) -> i32 { self.baseline_y_px }

	fn rasterize_terminal_glyph(&mut self, c: char) -> RasterizedTerminalGlyph {
		if let Some(glyph) = self.glyph_cache.get(&c) {
			return glyph.clone();
		}

		let font_key = if is_emoji_presentation_candidate(c) {
			self.emoji_font_key.unwrap_or(self.font_key)
		} else {
			self.font_key
		};

		let glyph = rasterize_terminal_glyph(&mut self.rasterizer, font_key, self.size, c);
		self.glyph_cache.insert(c, glyph.clone());
		glyph
	}
}

#[derive(Debug, Clone, PartialEq)]
struct RasterizedTerminalGlyph {
	c:          char,
	cell_width: u32,
	width_px:   u32,
	height_px:  u32,
	left_px:    i32,
	top_px:     i32,
	advance_px: i32,
	pixels:     Vec<u8>,
	is_color:   bool,
}

fn rasterize_terminal_glyph(
	rasterizer: &mut Rasterizer,
	font_key: FontKey,
	size: Size,
	c: char,
) -> RasterizedTerminalGlyph {
	let cell_width = terminal_char_cell_width(c).max(1);
	let glyph_key = GlyphKey { character: c, font_key, size };

	let Ok(glyph) = rasterizer.get_glyph(glyph_key) else {
		return RasterizedTerminalGlyph {
			c,
			cell_width,
			width_px: 1,
			height_px: 1,
			left_px: 0,
			top_px: 0,
			advance_px: 0,
			pixels: vec![0, 0, 0, 0],
			is_color: false,
		};
	};

	let width_px = glyph.width.max(0) as u32;
	let height_px = glyph.height.max(0) as u32;
	let is_color = is_color_glyph_buffer(&glyph.buffer, c);
	let pixels = glyph_rgba_pixels(&glyph.buffer, width_px, height_px, is_color);

	RasterizedTerminalGlyph {
		c,
		cell_width,
		width_px,
		height_px,
		left_px: glyph.left,
		top_px: glyph.top,
		advance_px: glyph.advance.0,
		pixels,
		is_color,
	}
}

fn build_atlas_from_rasterized_glyphs(
	glyphs: Vec<RasterizedTerminalGlyph>,
	base_cell_width: u32,
	base_cell_height: u32,
	baseline_y_px: i32,
	padding_px: u32,
	columns: u32,
) -> WgpuTerminalGlyphAtlas {
	if glyphs.is_empty() {
		return WgpuTerminalGlyphAtlas::empty();
	}

	// Alacritty never scales a rasterized glyph bitmap into the terminal cell.
	// The terminal cell decides layout; the glyph bitmap is placed at its native
	// rasterized size using font bearings/baseline.  The previous implementation
	// stored a whole terminal-cell-sized rectangle in the atlas and mapped that
	// rectangle onto a terminal-cell quad, which stretched/squashed every glyph
	// and made the text look short, fat, and blurry.
	let max_bitmap_width =
		glyphs.iter().map(|glyph| glyph.width_px.max(1)).max().unwrap_or(base_cell_width.max(1));
	let max_bitmap_height =
		glyphs.iter().map(|glyph| glyph.height_px.max(1)).max().unwrap_or(base_cell_height.max(1));

	let cell_stride_width = max_bitmap_width + padding_px;
	let cell_stride_height = max_bitmap_height + padding_px;
	let row_count = ((glyphs.len() as u32) + columns - 1) / columns;

	let atlas_width = padding_px + columns * cell_stride_width;
	let atlas_height = padding_px + row_count * cell_stride_height;

	let mut pixels = vec![0u8; (atlas_width * atlas_height * 4) as usize];
	let mut entries = HashMap::new();

	for (index, glyph) in glyphs.into_iter().enumerate() {
		let index = index as u32;
		let col = index % columns;
		let row = index / columns;

		let cell_x = padding_px + col * cell_stride_width;
		let cell_y = padding_px + row * cell_stride_height;
		let bitmap_width = glyph.width_px.max(1);
		let bitmap_height = glyph.height_px.max(1);

		write_crossfont_glyph_pixels_tight(&glyph, cell_x, cell_y, atlas_width, &mut pixels);

		let uv = WgpuTerminalGlyphUvRect {
			min_u: cell_x as f32 / atlas_width as f32,
			min_v: cell_y as f32 / atlas_height as f32,
			max_u: (cell_x + bitmap_width) as f32 / atlas_width as f32,
			max_v: (cell_y + bitmap_height) as f32 / atlas_height as f32,
		};

		let (draw_offset_x_px, draw_offset_y_px) =
			terminal_glyph_draw_offset(&glyph, base_cell_width, base_cell_height, baseline_y_px);

		entries.insert(glyph.c, WgpuTerminalGlyphAtlasEntry {
			codepoint: glyph.c as u32,
			x_px: cell_x,
			y_px: cell_y,
			width_px: bitmap_width,
			height_px: bitmap_height,
			advance_px: glyph.advance_px as f32,
			uv,
			draw_offset_x_px,
			draw_offset_y_px,
			draw_width_px: bitmap_width,
			draw_height_px: bitmap_height,
			is_color: glyph.is_color,
		});
	}

	WgpuTerminalGlyphAtlas { width_px: atlas_width, height_px: atlas_height, pixels, entries }
}

fn glyph_rgba_pixels(
	buffer: &BitmapBuffer,
	width_px: u32,
	height_px: u32,
	is_color: bool,
) -> Vec<u8> {
	if width_px == 0 || height_px == 0 {
		return Vec::new();
	}

	let pixel_count = (width_px * height_px) as usize;

	match buffer {
		BitmapBuffer::Rgb(buffer) => buffer
			.chunks_exact(3)
			.take(pixel_count)
			.flat_map(|rgb| {
				if is_color {
					[rgb[0], rgb[1], rgb[2], 255]
				} else {
					let alpha = rgb.iter().copied().max().unwrap_or(0);
					[0, 0, 0, alpha]
				}
			})
			.collect(),
		BitmapBuffer::Rgba(buffer) => buffer
			.chunks_exact(4)
			.take(pixel_count)
			.flat_map(|rgba| {
				if is_color {
					[rgba[0], rgba[1], rgba[2], rgba[3]]
				} else {
					let rgb_alpha = rgba[0..3].iter().copied().max().unwrap_or(0);
					let alpha = rgba[3].max(rgb_alpha);
					[0, 0, 0, alpha]
				}
			})
			.collect(),
	}
}

fn write_crossfont_glyph_pixels_tight(
	glyph: &RasterizedTerminalGlyph,
	cell_x: u32,
	cell_y: u32,
	atlas_width: u32,
	pixels: &mut [u8],
) {
	if glyph.width_px == 0 || glyph.height_px == 0 || glyph.pixels.is_empty() {
		return;
	}

	for src_y in 0..glyph.height_px {
		for src_x in 0..glyph.width_px {
			let dst_x = cell_x + src_x;
			let dst_y = cell_y + src_y;
			let src_index = ((src_y * glyph.width_px + src_x) * 4) as usize;
			let dst_index = ((dst_y * atlas_width + dst_x) * 4) as usize;

			if src_index + 3 < glyph.pixels.len() && dst_index + 3 < pixels.len() {
				pixels[dst_index..dst_index + 4].copy_from_slice(&glyph.pixels[src_index..src_index + 4]);
			}
		}
	}
}

fn alacritty_cell_axis_px(value: f64) -> u32 {
	if !value.is_finite() {
		return 1;
	}

	value.floor().max(1.0) as u32
}

fn alacritty_baseline_y_px(cell_height_px: u32, descent: f32) -> i32 {
	let baseline = cell_height_px as f64 + descent as f64;

	if baseline.is_finite() {
		baseline.round() as i32
	} else {
		((cell_height_px as f64) * 0.80).round() as i32
	}
}

fn terminal_glyph_draw_offset(
	glyph: &RasterizedTerminalGlyph,
	base_cell_width: u32,
	base_cell_height: u32,
	baseline_y_px: i32,
) -> (i32, i32) {
	let terminal_width = base_cell_width as i32 * glyph.cell_width.max(1) as i32;

	// Positive left bearings are honored.  Negative bearings are allowed to
	// overhang like Alacritty instead of being baked into the atlas and scaled.
	let draw_x = glyph.left_px;

	// Alacritty places bitmap glyphs relative to the font baseline derived from
	// crossfont metrics. The terminal cell decides layout; the bitmap keeps its
	// native size and font bearing.
	let mut draw_y = baseline_y_px - glyph.top_px;

	// Keep unusually small symbols visually centered inside the cell without
	// scaling their bitmap.
	if glyph.height_px > 0 && glyph.height_px < base_cell_height / 2 {
		draw_y = ((base_cell_height as i32 - glyph.height_px as i32) / 2).max(draw_y);
	}

	// Center color emoji if it is narrower than its terminal cell span.  Text
	// glyphs keep their font bearing.
	let draw_x = if glyph.is_color && (glyph.width_px as i32) < terminal_width {
		(terminal_width - glyph.width_px as i32) / 2
	} else {
		draw_x
	};

	(draw_x, draw_y)
}

fn is_color_glyph_buffer(buffer: &BitmapBuffer, c: char) -> bool {
	// Crossfont can return RGBA buffers for regular antialiased glyphs on
	// some platforms/backends. Treating every RGBA glyph as a color glyph makes
	// normal prompt symbols ignore the ANSI foreground color and look like a
	// colored rectangle/background. Alacritty only preserves embedded color for
	// emoji/color-presentation glyphs; normal glyphs remain alpha masks tinted
	// by the cell foreground.
	is_emoji_presentation_candidate(c)
		&& matches!(buffer, BitmapBuffer::Rgb(_) | BitmapBuffer::Rgba(_))
}

fn load_optional_font(rasterizer: &mut Rasterizer, family: &str, size: Size) -> Option<FontKey> {
	let font_desc = FontDesc::new(family.to_owned(), Style::Description {
		slant:  Slant::Normal,
		weight: Weight::Normal,
	});

	rasterizer.load_font(&font_desc, size).ok()
}

fn is_emoji_presentation_candidate(c: char) -> bool {
	// Do not treat the whole Dingbats/Misc Symbols ranges as color emoji.
	// Powerline/prompt symbols such as `❯` live around U+276F and must stay
	// normal text glyphs tinted by the terminal foreground color.  Without a
	// grapheme-level VS16 parser we only route the dedicated emoji planes to the
	// color emoji font.
	matches!(c as u32, 0x1F000..=0x1FAFF)
}
