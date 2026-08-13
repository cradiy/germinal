use std::{
	cell::RefCell,
	collections::{BTreeSet, HashMap},
};

use germinal_ports::{
	rendering::{render_target_id::RenderTargetId, surface_snapshot::RenderSurfaceSnapshot},
	seq::Seq,
};

use crate::rendering::pty_surface::{
	crossfont_glyph_atlas::{WgpuCrossfontGlyphAtlasBuilder, WgpuCrossfontGlyphAtlasError},
	glyph_atlas::{WgpuDebugGlyphAtlasBuilder, WgpuTerminalGlyphAtlas, WgpuTerminalGlyphKey},
	glyph_atlas_texture::{WgpuTerminalGlyphAtlasTextureFactory, WgpuTerminalGlyphAtlasUploadBytes},
};

#[derive(Debug, Clone, PartialEq)]
pub struct WgpuTerminalGlyphAtlasFrame {
	pub target_id:    RenderTargetId,
	pub seq:          Seq,
	pub atlas:        WgpuTerminalGlyphAtlas,
	pub upload_bytes: Option<WgpuTerminalGlyphAtlasUploadBytes>,
	pub run_count:    usize,
	pub char_count:   usize,
	pub source:       WgpuTerminalGlyphAtlasSourceKind,
	pub cache_hit:    bool,
}

impl WgpuTerminalGlyphAtlasFrame {
	pub fn has_upload_work(&self) -> bool {
		self.upload_bytes.as_ref().is_some_and(|upload_bytes| !upload_bytes.is_empty())
	}

	pub fn glyph_count(&self) -> usize { self.atlas.entries.len() }

	pub fn atlas_width_px(&self) -> u32 { self.atlas.width_px }

	pub fn atlas_height_px(&self) -> u32 { self.atlas.height_px }

	pub fn upload_byte_len(&self) -> usize {
		self.upload_bytes.as_ref().map_or(0, WgpuTerminalGlyphAtlasUploadBytes::byte_len)
	}
}

#[derive(Debug, Clone)]
pub struct WgpuTerminalGlyphAtlasFrameBuilder {
	source:          WgpuTerminalGlyphAtlasSource,
	texture_factory: WgpuTerminalGlyphAtlasTextureFactory,
	cache:           RefCell<HashMap<RenderTargetId, WgpuTerminalGlyphAtlasCacheEntry>>,
}

impl WgpuTerminalGlyphAtlasFrameBuilder {
	pub fn new() -> Self { Self::debug_5x7() }

	pub fn debug_5x7() -> Self {
		Self {
			source:          WgpuTerminalGlyphAtlasSource::Debug5x7(WgpuDebugGlyphAtlasBuilder::new()),
			texture_factory: WgpuTerminalGlyphAtlasTextureFactory::new(),
			cache:           RefCell::new(HashMap::new()),
		}
	}

	pub fn crossfont(
		font_family: impl Into<String>,
		font_size_px: f32,
	) -> Result<Self, WgpuCrossfontGlyphAtlasError> {
		Ok(Self {
			source:          WgpuTerminalGlyphAtlasSource::Crossfont(
				WgpuCrossfontGlyphAtlasBuilder::new(font_family, font_size_px)?,
			),
			texture_factory: WgpuTerminalGlyphAtlasTextureFactory::new(),
			cache:           RefCell::new(HashMap::new()),
		})
	}

	pub fn with_crossfont_builder(crossfont_builder: WgpuCrossfontGlyphAtlasBuilder) -> Self {
		Self {
			source:          WgpuTerminalGlyphAtlasSource::Crossfont(crossfont_builder),
			texture_factory: WgpuTerminalGlyphAtlasTextureFactory::new(),
			cache:           RefCell::new(HashMap::new()),
		}
	}

	pub fn source_kind(&self) -> WgpuTerminalGlyphAtlasSourceKind { self.source.kind() }

	pub fn remove_render_target(&self, target_id: RenderTargetId) -> bool {
		self.cache.borrow_mut().remove(&target_id).is_some()
	}

	pub fn build(&self, surface_snapshot: &RenderSurfaceSnapshot) -> WgpuTerminalGlyphAtlasFrame {
		let texts: Vec<&str> = surface_snapshot
			.rows
			.iter()
			.flat_map(|row| row.runs.iter())
			.map(|run| run.text.as_str())
			.collect();

		let run_count = texts.len();
		let char_count = texts.iter().map(|text| text.chars().count()).sum();

		let glyphs = collect_glyphs(surface_snapshot);
		let cache_key = WgpuTerminalGlyphAtlasCacheKey {
			source: self.source.kind(),
			glyphs: glyphs.iter().copied().collect(),
		};

		let (atlas, cache_hit) =
			self.cached_or_build_atlas(surface_snapshot.target_id, &cache_key);

		let upload_bytes = self.texture_factory.build_upload_bytes(&atlas);

		WgpuTerminalGlyphAtlasFrame {
			target_id: surface_snapshot.target_id,
			seq: surface_snapshot.latest_seq,
			atlas,
			upload_bytes,
			run_count,
			char_count,
			source: self.source.kind(),
			cache_hit,
		}
	}

	fn cached_or_build_atlas(
		&self,
		target_id: RenderTargetId,
		cache_key: &WgpuTerminalGlyphAtlasCacheKey,
	) -> (WgpuTerminalGlyphAtlas, bool) {
		{
			let cache = self.cache.borrow();

			if let Some(entry) = cache.get(&target_id)
				&& entry.key.source == cache_key.source
				&& entry.key == *cache_key
			{
				return (entry.atlas.clone(), true);
			}
		}

		let atlas = self.source.build_for_glyphs(cache_key.glyphs.iter().copied());

		{
			let mut cache = self.cache.borrow_mut();

			cache.insert(
				target_id,
				WgpuTerminalGlyphAtlasCacheEntry { key: cache_key.clone(), atlas: atlas.clone() },
			);
		}

		(atlas, false)
	}
}

impl Default for WgpuTerminalGlyphAtlasFrameBuilder {
	fn default() -> Self { Self::new() }
}

#[derive(Debug, Clone)]
pub enum WgpuTerminalGlyphAtlasSource {
	Debug5x7(WgpuDebugGlyphAtlasBuilder),
	Crossfont(WgpuCrossfontGlyphAtlasBuilder),
}

impl WgpuTerminalGlyphAtlasSource {
	pub fn kind(&self) -> WgpuTerminalGlyphAtlasSourceKind {
		match self {
			Self::Debug5x7(_) => WgpuTerminalGlyphAtlasSourceKind::Debug5x7,
			Self::Crossfont(_) => WgpuTerminalGlyphAtlasSourceKind::Crossfont,
		}
	}

	pub fn build_for_chars<I>(&self, chars: I) -> WgpuTerminalGlyphAtlas
	where I: IntoIterator<Item = char> {
		match self {
			Self::Debug5x7(builder) => builder.build_for_chars(chars),
			Self::Crossfont(builder) => builder.build_for_chars(chars),
		}
	}

	pub fn build_for_glyphs<I>(&self, glyphs: I) -> WgpuTerminalGlyphAtlas
	where I: IntoIterator<Item = WgpuTerminalGlyphKey> {
		match self {
			Self::Debug5x7(builder) => builder.build_for_glyphs(glyphs),
			Self::Crossfont(builder) => builder.build_for_glyphs(glyphs),
		}
	}

	pub fn build_for_texts<I, S>(&self, texts: I) -> WgpuTerminalGlyphAtlas
	where
		I: IntoIterator<Item = S>,
		S: AsRef<str>,
	{
		match self {
			Self::Debug5x7(builder) => builder.build_for_texts(texts),
			Self::Crossfont(builder) => builder.build_for_texts(texts),
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuTerminalGlyphAtlasSourceKind {
	Debug5x7,
	Crossfont,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WgpuTerminalGlyphAtlasCacheKey {
	source: WgpuTerminalGlyphAtlasSourceKind,
	glyphs: Vec<WgpuTerminalGlyphKey>,
}

#[derive(Debug, Clone, PartialEq)]
struct WgpuTerminalGlyphAtlasCacheEntry {
	key:   WgpuTerminalGlyphAtlasCacheKey,
	atlas: WgpuTerminalGlyphAtlas,
}

fn collect_glyphs(surface_snapshot: &RenderSurfaceSnapshot) -> BTreeSet<WgpuTerminalGlyphKey> {
	let mut glyphs = BTreeSet::new();

	for row in &surface_snapshot.rows {
		for run in &row.runs {
			for c in run.text.chars() {
				glyphs.insert(WgpuTerminalGlyphKey::new(c, run.style.bold));
			}
		}
	}

	glyphs
}

#[cfg(test)]
mod tests {
	use germinal_ports::rendering::{
		frame_plan_builder::TextStyleDto,
		surface_snapshot::{RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot},
	};

	use super::*;

	#[test]
	fn builds_debug_glyph_atlas_frame_from_surface_snapshot() {
		let target_id = RenderTargetId::new(1);

		let snapshot = RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(9),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![
					RenderSurfaceRunSnapshot {
						x:     0,
						text:  "red".to_string(),
						style: TextStyleDto::plain(),
					},
					RenderSurfaceRunSnapshot {
						x:     4,
						text:  "green".to_string(),
						style: TextStyleDto::plain(),
					},
				],
			}],
			video_surfaces: vec![],
			dirty_rows: vec![0],
			cursor: None,
		};

		let builder = WgpuTerminalGlyphAtlasFrameBuilder::debug_5x7();
		let frame = builder.build(&snapshot);

		assert_eq!(frame.target_id, target_id);
		assert_eq!(frame.seq, Seq::new(9));
		assert_eq!(frame.source, WgpuTerminalGlyphAtlasSourceKind::Debug5x7);
		assert!(!frame.cache_hit);

		assert_eq!(frame.run_count, 2);
		assert_eq!(frame.char_count, 8);

		assert!(frame.glyph_count() > 0);
		assert!(frame.atlas.has_glyph('r'));
		assert!(frame.atlas.has_glyph('e'));
		assert!(frame.atlas.has_glyph('d'));
		assert!(frame.atlas.has_glyph('g'));

		assert!(frame.has_upload_work());
		assert!(frame.atlas_width_px() > 0);
		assert!(frame.atlas_height_px() > 0);
		assert_eq!(
			frame.upload_byte_len(),
			(frame.atlas_width_px() * frame.atlas_height_px() * 4) as usize
		);
	}

	#[test]
	fn second_build_with_same_chars_hits_cache() {
		let target_id = RenderTargetId::new(1);

		let snapshot = RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(9),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![RenderSurfaceRunSnapshot {
					x:     0,
					text:  "red".to_string(),
					style: TextStyleDto::plain(),
				}],
			}],
			video_surfaces: vec![],
			dirty_rows: vec![0],
			cursor: None,
		};

		let builder = WgpuTerminalGlyphAtlasFrameBuilder::debug_5x7();

		let first = builder.build(&snapshot);
		let second = builder.build(&snapshot);

		assert!(!first.cache_hit);
		assert!(second.cache_hit);
		assert_eq!(first.atlas, second.atlas);
	}

	#[test]
	fn changed_chars_miss_cache() {
		let target_id = RenderTargetId::new(1);

		let red_snapshot = RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![RenderSurfaceRunSnapshot {
					x:     0,
					text:  "red".to_string(),
					style: TextStyleDto::plain(),
				}],
			}],
			video_surfaces: vec![],
			dirty_rows: vec![0],
			cursor: None,
		};

		let blue_snapshot = RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(2),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![RenderSurfaceRunSnapshot {
					x:     0,
					text:  "blue".to_string(),
					style: TextStyleDto::plain(),
				}],
			}],
			video_surfaces: vec![],
			dirty_rows: vec![0],
			cursor: None,
		};

		let builder = WgpuTerminalGlyphAtlasFrameBuilder::debug_5x7();

		let first = builder.build(&red_snapshot);
		let second = builder.build(&blue_snapshot);

		assert!(!first.cache_hit);
		assert!(!second.cache_hit);
	}

	#[test]
	fn different_targets_keep_independent_cache_entries() {
		let snapshot = |target_id, text: &str| RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![RenderSurfaceRowSnapshot {
				y: 0,
				runs: vec![RenderSurfaceRunSnapshot {
					x: 0,
					text: text.to_string(),
					style: TextStyleDto::plain(),
				}],
			}],
			video_surfaces: vec![],
			dirty_rows: vec![0],
			cursor: None,
		};
		let first_target = snapshot(RenderTargetId::new(1), "red");
		let second_target = snapshot(RenderTargetId::new(2), "blue");
		let builder = WgpuTerminalGlyphAtlasFrameBuilder::debug_5x7();

		assert!(!builder.build(&first_target).cache_hit);
		assert!(!builder.build(&second_target).cache_hit);
		assert!(builder.build(&first_target).cache_hit);
		assert!(builder.build(&second_target).cache_hit);
	}

	#[test]
	fn removing_a_target_invalidates_only_its_cache_entry() {
		let snapshot = |target_id, text: &str| RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![RenderSurfaceRowSnapshot {
				y: 0,
				runs: vec![RenderSurfaceRunSnapshot {
					x: 0,
					text: text.to_string(),
					style: TextStyleDto::plain(),
				}],
			}],
			video_surfaces: vec![],
			dirty_rows: vec![0],
			cursor: None,
		};
		let first_target = snapshot(RenderTargetId::new(1), "red");
		let second_target = snapshot(RenderTargetId::new(2), "blue");
		let builder = WgpuTerminalGlyphAtlasFrameBuilder::debug_5x7();

		assert!(!builder.build(&first_target).cache_hit);
		assert!(!builder.build(&second_target).cache_hit);
		assert!(builder.remove_render_target(first_target.target_id));
		assert!(!builder.remove_render_target(first_target.target_id));
		assert!(!builder.build(&first_target).cache_hit);
		assert!(builder.build(&second_target).cache_hit);
	}

	#[test]
	fn changed_chars_replace_cache_instead_of_growing_union() {
		let target_id = RenderTargetId::new(1);

		let red_snapshot = RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![RenderSurfaceRunSnapshot {
					x:     0,
					text:  "red".to_string(),
					style: TextStyleDto::plain(),
				}],
			}],
			video_surfaces: vec![],
			dirty_rows: vec![0],
			cursor: None,
		};

		let blue_snapshot = RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(2),
			rows: vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![RenderSurfaceRunSnapshot {
					x:     0,
					text:  "blue".to_string(),
					style: TextStyleDto::plain(),
				}],
			}],
			video_surfaces: vec![],
			dirty_rows: vec![0],
			cursor: None,
		};

		let builder = WgpuTerminalGlyphAtlasFrameBuilder::debug_5x7();

		let _ = builder.build(&red_snapshot);
		let blue = builder.build(&blue_snapshot);

		assert!(!blue.cache_hit);
		assert!(blue.atlas.has_glyph('b'));
		assert!(blue.atlas.has_glyph('l'));
		assert!(!blue.atlas.has_glyph('r'));
	}

	#[test]
	fn empty_surface_snapshot_can_hit_cache_on_second_build() {
		let target_id = RenderTargetId::new(1);

		let snapshot = RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: Vec::new(),
			video_surfaces: vec![],
			dirty_rows: Vec::new(),
			cursor: None,
		};

		let builder = WgpuTerminalGlyphAtlasFrameBuilder::new();

		let first = builder.build(&snapshot);
		let second = builder.build(&snapshot);

		assert_eq!(first.target_id, target_id);
		assert_eq!(first.seq, Seq::new(1));
		assert_eq!(first.source, WgpuTerminalGlyphAtlasSourceKind::Debug5x7);

		assert_eq!(first.run_count, 0);
		assert_eq!(first.char_count, 0);
		assert_eq!(first.glyph_count(), 0);
		assert_eq!(first.upload_byte_len(), 0);
		assert!(!first.has_upload_work());
		assert!(!first.cache_hit);

		assert!(second.cache_hit);
	}

	#[test]
	fn source_kind_reports_debug_by_default() {
		let builder = WgpuTerminalGlyphAtlasFrameBuilder::new();

		assert_eq!(builder.source_kind(), WgpuTerminalGlyphAtlasSourceKind::Debug5x7);
	}
}
