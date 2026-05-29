use std::{
	collections::{BTreeSet, HashMap},
	sync::Arc,
};

use fontdue::{Font, FontSettings};
use unicode_width::UnicodeWidthChar;

use crate::rendering::pty_surface::glyph_atlas::{
	WgpuTerminalGlyphAtlas, WgpuTerminalGlyphAtlasEntry, WgpuTerminalGlyphUvRect,
};

#[derive(Clone)]
pub struct WgpuFontdueGlyphAtlasBuilder {
	fonts:          Arc<[Font]>,
	font_size_px:   f32,
	padding_px:     u32,
	columns:        u32,
	cell_width_px:  Option<u32>,
	cell_height_px: Option<u32>,
}

impl WgpuFontdueGlyphAtlasBuilder {
	pub fn from_bytes(
		font_bytes: impl AsRef<[u8]>,
		font_size_px: f32,
	) -> Result<Self, WgpuFontdueGlyphAtlasError> {
		Self::from_bytes_collection([font_bytes], font_size_px)
	}

	pub fn from_bytes_collection<I, B>(
		font_bytes_collection: I,
		font_size_px: f32,
	) -> Result<Self, WgpuFontdueGlyphAtlasError>
	where
		I: IntoIterator<Item = B>,
		B: AsRef<[u8]>,
	{
		let mut fonts = Vec::new();
		let mut first_error = None::<String>;

		for font_bytes in font_bytes_collection {
			match Font::from_bytes(font_bytes.as_ref().to_vec(), FontSettings::default()) {
				Ok(font) => fonts.push(font),
				Err(message) => {
					first_error.get_or_insert_with(|| message.to_string());
				}
			}
		}

		if fonts.is_empty() {
			return Err(WgpuFontdueGlyphAtlasError::InvalidFont {
				message: first_error.unwrap_or_else(|| "no usable font bytes".to_string()),
			});
		}

		Ok(Self {
			fonts: Arc::from(fonts),
			font_size_px,
			padding_px: 1,
			columns: 16,
			cell_width_px: None,
			cell_height_px: None,
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

	pub fn font_size_px(&self) -> f32 { self.font_size_px }

	pub fn padding_px(&self) -> u32 { self.padding_px }

	pub fn columns(&self) -> u32 { self.columns }

	pub fn cell_width_px(&self) -> Option<u32> { self.cell_width_px }

	pub fn cell_height_px(&self) -> Option<u32> { self.cell_height_px }

	pub fn font_count(&self) -> usize { self.fonts.len() }

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
		let glyphs: Vec<RasterizedGlyph> = chars
			.into_iter()
			.filter(|c| terminal_char_cell_width(*c) > 0)
			.map(|c| self.rasterize_char(c))
			.collect();

		if glyphs.is_empty() {
			return WgpuTerminalGlyphAtlas {
				width_px:  0,
				height_px: 0,
				pixels:    Vec::new(),
				entries:   HashMap::new(),
			};
		}

		let base_cell_width = self.cell_width_px.unwrap_or_else(|| {
			glyphs
				.iter()
				.filter(|glyph| glyph.cell_width == 1)
				.map(|glyph| glyph.width_px)
				.max()
				.unwrap_or(1)
				.max(1)
		});

		let base_cell_height = self
			.cell_height_px
			.unwrap_or_else(|| glyphs.iter().map(|glyph| glyph.height_px).max().unwrap_or(1).max(1));

		let glyph_cell_height = base_cell_height;
		let baseline_y_px = self.baseline_y_px(glyph_cell_height);

		let max_glyph_cell_width = glyphs
			.iter()
			.map(|glyph| base_cell_width * glyph.cell_width)
			.max()
			.unwrap_or(base_cell_width);

		let cell_stride_width = max_glyph_cell_width + self.padding_px;
		let cell_stride_height = glyph_cell_height + self.padding_px;

		let row_count = ((glyphs.len() as u32) + self.columns - 1) / self.columns;

		let atlas_width = self.padding_px + self.columns * cell_stride_width;
		let atlas_height = self.padding_px + row_count * cell_stride_height;

		let mut pixels = vec![0u8; (atlas_width * atlas_height) as usize];
		let mut entries = HashMap::new();

		for (index, glyph) in glyphs.into_iter().enumerate() {
			let index = index as u32;
			let col = index % self.columns;
			let row = index / self.columns;

			let cell_x = self.padding_px + col * cell_stride_width;
			let cell_y = self.padding_px + row * cell_stride_height;
			let glyph_cell_width = base_cell_width * glyph.cell_width;

			write_glyph_pixels_by_metrics(
				&glyph,
				cell_x,
				cell_y,
				glyph_cell_width,
				glyph_cell_height,
				baseline_y_px,
				atlas_width,
				&mut pixels,
			);

			let uv = WgpuTerminalGlyphUvRect {
				min_u: cell_x as f32 / atlas_width as f32,
				min_v: cell_y as f32 / atlas_height as f32,
				max_u: (cell_x + glyph_cell_width) as f32 / atlas_width as f32,
				max_v: (cell_y + glyph_cell_height) as f32 / atlas_height as f32,
			};

			entries.insert(glyph.c, WgpuTerminalGlyphAtlasEntry {
				codepoint: glyph.c as u32,
				x_px: cell_x,
				y_px: cell_y,
				width_px: glyph_cell_width,
				height_px: glyph_cell_height,
				advance_px: glyph.advance_px,
				uv,
			});
		}

		WgpuTerminalGlyphAtlas { width_px: atlas_width, height_px: atlas_height, pixels, entries }
	}

	fn rasterize_char(&self, c: char) -> RasterizedGlyph {
		let font = self.font_for_char(c).unwrap_or(&self.fonts[0]);
		let (metrics, bitmap) = font.rasterize(c, self.font_size_px);
		let cell_width = terminal_char_cell_width(c).max(1);

		let width_px = metrics.width as u32;
		let height_px = metrics.height as u32;

		if width_px == 0 || height_px == 0 || bitmap.is_empty() {
			return RasterizedGlyph {
				c,
				cell_width,
				width_px: 1,
				height_px: 1,
				xmin_px: metrics.xmin,
				ymin_px: metrics.ymin,
				advance_px: metrics.advance_width,
				pixels: vec![0],
			};
		}

		RasterizedGlyph {
			c,
			cell_width,
			width_px,
			height_px,
			xmin_px: metrics.xmin,
			ymin_px: metrics.ymin,
			advance_px: metrics.advance_width,
			pixels: bitmap,
		}
	}

	fn font_for_char(&self, c: char) -> Option<&Font> {
		self.fonts.iter().find(|font| font.lookup_glyph_index(c) != 0)
	}

	fn baseline_y_px(&self, cell_height_px: u32) -> f32 {
		let Some(line_metrics) = self.fonts[0].horizontal_line_metrics(self.font_size_px) else {
			return cell_height_px as f32 * 0.8;
		};

		let line_height = line_metrics.ascent - line_metrics.descent;
		let top_padding = ((cell_height_px as f32 - line_height) / 2.0).max(0.0);

		top_padding + line_metrics.ascent
	}
}

impl std::fmt::Debug for WgpuFontdueGlyphAtlasBuilder {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("WgpuFontdueGlyphAtlasBuilder")
			.field("font_count", &self.fonts.len())
			.field("font_size_px", &self.font_size_px)
			.field("padding_px", &self.padding_px)
			.field("columns", &self.columns)
			.field("cell_width_px", &self.cell_width_px)
			.field("cell_height_px", &self.cell_height_px)
			.finish()
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WgpuFontdueGlyphAtlasError {
	InvalidFont { message: String },
}

#[derive(Debug, Clone, PartialEq)]
struct RasterizedGlyph {
	c:          char,
	cell_width: u32,
	width_px:   u32,
	height_px:  u32,
	xmin_px:    i32,
	ymin_px:    i32,
	advance_px: f32,
	pixels:     Vec<u8>,
}

fn write_glyph_pixels_by_metrics(
	glyph: &RasterizedGlyph,
	cell_x_px: u32,
	cell_y_px: u32,
	cell_width_px: u32,
	cell_height_px: u32,
	baseline_y_px: f32,
	atlas_width_px: u32,
	atlas_pixels: &mut [u8],
) {
	let glyph_width = glyph.width_px.max(1) as f32;
	let horizontal_padding = ((cell_width_px as f32 - glyph_width) / 2.0).max(0.0);
	let glyph_left_px = horizontal_padding + glyph.xmin_px as f32;
	let glyph_top_px = baseline_y_px - glyph.ymin_px as f32 - glyph.height_px as f32;

	blit_glyph_clipped(
		glyph,
		cell_x_px as i32 + glyph_left_px.round() as i32,
		cell_y_px as i32 + glyph_top_px.round() as i32,
		cell_x_px,
		cell_y_px,
		cell_width_px,
		cell_height_px,
		atlas_width_px,
		atlas_pixels,
	);
}

#[allow(clippy::too_many_arguments)]
fn blit_glyph_clipped(
	glyph: &RasterizedGlyph,
	dst_origin_x_px: i32,
	dst_origin_y_px: i32,
	cell_x_px: u32,
	cell_y_px: u32,
	cell_width_px: u32,
	cell_height_px: u32,
	atlas_width_px: u32,
	atlas_pixels: &mut [u8],
) {
	let cell_left = cell_x_px as i32;
	let cell_top = cell_y_px as i32;
	let cell_right = cell_left + cell_width_px as i32;
	let cell_bottom = cell_top + cell_height_px as i32;

	for src_y in 0..glyph.height_px as i32 {
		for src_x in 0..glyph.width_px as i32 {
			let dst_x = dst_origin_x_px + src_x;
			let dst_y = dst_origin_y_px + src_y;

			if dst_x < cell_left || dst_x >= cell_right || dst_y < cell_top || dst_y >= cell_bottom {
				continue;
			}

			let src_offset = (src_y as u32 * glyph.width_px + src_x as u32) as usize;
			let dst_offset = (dst_y as u32 * atlas_width_px + dst_x as u32) as usize;

			atlas_pixels[dst_offset] = glyph.pixels[src_offset];
		}
	}
}

fn terminal_char_cell_width(c: char) -> u32 { c.width().unwrap_or(0).min(2) as u32 }

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn invalid_font_bytes_return_error() {
		let result = WgpuFontdueGlyphAtlasBuilder::from_bytes(b"not a font", 16.0);

		assert!(result.is_err());
	}

	#[test]
	fn builder_config_is_exposed() {
		let Some(font_bytes) = try_load_system_font() else {
			return;
		};

		let builder = WgpuFontdueGlyphAtlasBuilder::from_bytes(font_bytes, 18.0)
			.expect("system font should load")
			.with_padding_px(2)
			.with_columns(8)
			.with_cell_size_px(12, 24);

		assert_eq!(builder.font_size_px(), 18.0);
		assert_eq!(builder.padding_px(), 2);
		assert_eq!(builder.columns(), 8);
		assert_eq!(builder.cell_width_px(), Some(12));
		assert_eq!(builder.cell_height_px(), Some(24));
	}

	#[test]
	fn builds_fontdue_atlas_when_system_font_exists() {
		let Some(font_bytes) = try_load_system_font() else {
			return;
		};

		let builder = WgpuFontdueGlyphAtlasBuilder::from_bytes(font_bytes, 18.0)
			.expect("system font should load")
			.with_cell_size_px(12, 24);

		let atlas = builder.build_for_texts(["red green under", "Germinal wgpu terminal smoke 123"]);

		assert!(!atlas.is_empty());
		assert!(atlas.width_px > 0);
		assert!(atlas.height_px > 0);
		assert_eq!(atlas.pixel_count(), (atlas.width_px * atlas.height_px) as usize);
		assert!(atlas.non_zero_pixel_count() > 0);

		assert!(atlas.has_glyph('r'));
		assert!(atlas.has_glyph('G'));
		assert!(atlas.has_glyph('1'));

		for entry in atlas.entries.values() {
			assert!(entry.uv.is_normalized());
			assert_eq!(entry.height_px, 24);
		}
	}

	fn try_load_system_font() -> Option<Vec<u8>> {
		let mut database = fontdb::Database::new();
		database.load_system_fonts();

		let query =
			fontdb::Query { families: &[fontdb::Family::Name("monospace")], ..fontdb::Query::default() };

		let face_id = database.query(&query)?;
		database.with_face_data(face_id, |data, _face_index| data.to_vec())
	}
}
