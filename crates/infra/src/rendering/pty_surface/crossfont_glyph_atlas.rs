use std::{cell::RefCell, collections::HashMap, rc::Rc};

use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Color, Family, FeatureTag, FontFeatures, FontSystem,
    Metrics, Shaping, Style as CosmicStyle, SwashCache, Weight as CosmicWeight, Wrap, fontdb,
    rustybuzz,
};
use crossfont::{
    BitmapBuffer, FontDesc, FontKey, GlyphKey, Rasterize, Rasterizer, Size, Slant, Style, Weight,
};
use germinal_ports::pty_host::width::terminal_char_cell_width;
use germinal_ports::pty_host::{
    font_config::TerminalFontConfig, font_face::TerminalFontFace, font_weight::TerminalFontWeight,
};
use thiserror::Error;
use tracing::warn;

use crate::rendering::pty_surface::{
    glyph_atlas::{
        WgpuTerminalGlyphAtlas, WgpuTerminalGlyphAtlasEntry, WgpuTerminalGlyphKey,
        WgpuTerminalGlyphUvRect,
    },
    text_shaping::TerminalTextSegment,
};

#[derive(Debug, Error)]
pub enum WgpuCrossfontGlyphAtlasError {
    #[error("crossfont rasterizer failed: {0}")]
    Rasterizer(#[source] crossfont::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuCrossfontCellMetrics {
    cell_width_px: u32,
    cell_height_px: u32,
    baseline_y_px: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuCrossfontUnderlineMetrics {
    offset_y_px: u32,
    thickness_px: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuCrossfontStrikeoutMetrics {
    offset_y_px: u32,
    thickness_px: u32,
}

impl WgpuCrossfontStrikeoutMetrics {
    pub const fn new(offset_y_px: u32, thickness_px: u32) -> Self {
        Self {
            offset_y_px,
            thickness_px,
        }
    }

    pub const fn offset_y_px(self) -> u32 {
        self.offset_y_px
    }

    pub const fn thickness_px(self) -> u32 {
        self.thickness_px
    }
}

impl WgpuCrossfontUnderlineMetrics {
    pub const fn new(offset_y_px: u32, thickness_px: u32) -> Self {
        Self {
            offset_y_px,
            thickness_px,
        }
    }

    pub const fn offset_y_px(self) -> u32 {
        self.offset_y_px
    }

    pub const fn thickness_px(self) -> u32 {
        self.thickness_px
    }
}

impl WgpuCrossfontCellMetrics {
    pub const fn new(cell_width_px: u32, cell_height_px: u32, baseline_y_px: i32) -> Self {
        Self {
            cell_width_px,
            cell_height_px,
            baseline_y_px,
        }
    }

    pub const fn cell_width_px(self) -> u32 {
        self.cell_width_px
    }

    pub const fn cell_height_px(self) -> u32 {
        self.cell_height_px
    }

    pub const fn baseline_y_px(self) -> i32 {
        self.baseline_y_px
    }
}

#[derive(Clone)]
pub struct WgpuCrossfontGlyphAtlasBuilder {
    font_family: String,
    font_faces: WgpuCrossfontFontFaces,
    font_size_px: f32,
    bold_weight: WgpuTerminalFontWeight,
    padding_px: u32,
    columns: u32,
    max_texture_dimension_2d: u32,
    cell_width_px: Option<u32>,
    cell_height_px: Option<u32>,
    ligatures: bool,
    backend: Rc<RefCell<Option<WgpuCrossfontGlyphBackend>>>,
}

impl std::fmt::Debug for WgpuCrossfontGlyphAtlasBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WgpuCrossfontGlyphAtlasBuilder")
            .field("font_family", &self.font_family)
            .field("font_size_px", &self.font_size_px)
            .field("bold_weight", &self.bold_weight)
            .field("padding_px", &self.padding_px)
            .field("columns", &self.columns)
            .field("max_texture_dimension_2d", &self.max_texture_dimension_2d)
            .field("cell_width_px", &self.cell_width_px)
            .field("cell_height_px", &self.cell_height_px)
            .field("ligatures", &self.ligatures)
            .finish()
    }
}

impl WgpuCrossfontGlyphAtlasBuilder {
    pub fn new(
        font_family: impl Into<String>,
        font_size_px: f32,
    ) -> Result<Self, WgpuCrossfontGlyphAtlasError> {
        let font_family = font_family.into();
        let font_faces = WgpuCrossfontFontFaces::new(font_family.clone());
        Self::from_font_faces(font_faces, font_size_px)
    }

    pub fn from_terminal_font_config(
        font_config: &TerminalFontConfig,
        font_size_px: f32,
    ) -> Result<Self, WgpuCrossfontGlyphAtlasError> {
        Ok(Self::from_font_faces(
            WgpuCrossfontFontFaces::from_terminal(font_config),
            font_size_px,
        )?
        .with_ligatures(font_config.ligatures()))
    }

    fn from_font_faces(
        font_faces: WgpuCrossfontFontFaces,
        font_size_px: f32,
    ) -> Result<Self, WgpuCrossfontGlyphAtlasError> {
        let font_family = font_faces.normal.family.clone();
        let bold_weight = font_faces.bold_weight;
        let backend = WgpuCrossfontGlyphBackend::new(font_faces.clone(), font_size_px)?;

        Ok(Self {
            font_family,
            font_faces,
            font_size_px,
            bold_weight,
            padding_px: 1,
            columns: 16,
            max_texture_dimension_2d: u32::MAX,
            cell_width_px: None,
            cell_height_px: None,
            ligatures: true,
            backend: Rc::new(RefCell::new(Some(backend))),
        })
    }

    pub fn with_padding_px(mut self, padding_px: u32) -> Self {
        self.padding_px = padding_px;
        self
    }

    pub fn with_bold_font_weight(mut self, bold_weight: WgpuTerminalFontWeight) -> Self {
        self.bold_weight = bold_weight;
        self.font_faces.bold_weight = bold_weight;

        if let Ok(backend) =
            WgpuCrossfontGlyphBackend::new(self.font_faces.clone(), self.font_size_px)
        {
            *self.backend.borrow_mut() = Some(backend);
        }

        self
    }

    pub fn with_columns(mut self, columns: u32) -> Self {
        self.columns = columns.max(1);
        self
    }

    pub fn with_max_texture_dimension_2d(mut self, max_texture_dimension_2d: u32) -> Self {
        self.max_texture_dimension_2d = max_texture_dimension_2d.max(1);
        self
    }

    pub fn with_cell_size_px(mut self, cell_width_px: u32, cell_height_px: u32) -> Self {
        self.cell_width_px = Some(cell_width_px.max(1));
        self.cell_height_px = Some(cell_height_px.max(1));
        self
    }

    pub fn with_ligatures(mut self, ligatures: bool) -> Self {
        self.ligatures = ligatures;
        self
    }

    pub fn load_cell_metrics(
        font_family: impl Into<String>,
        font_size_px: f32,
    ) -> Result<WgpuCrossfontCellMetrics, WgpuCrossfontGlyphAtlasError> {
        let backend = WgpuCrossfontGlyphBackend::new(
            WgpuCrossfontFontFaces::new(font_family.into()),
            font_size_px,
        )?;

        Ok(WgpuCrossfontCellMetrics::new(
            backend.base_cell_width_px().max(1),
            backend.base_cell_height_px().max(1),
            backend.baseline_y_px(),
        ))
    }

    pub fn load_cell_metrics_for_font_config(
        font_config: &TerminalFontConfig,
        font_size_px: f32,
    ) -> Result<WgpuCrossfontCellMetrics, WgpuCrossfontGlyphAtlasError> {
        let backend = WgpuCrossfontGlyphBackend::new(
            WgpuCrossfontFontFaces::from_terminal(font_config),
            font_size_px,
        )?;

        Ok(WgpuCrossfontCellMetrics::new(
            backend.base_cell_width_px().max(1),
            backend.base_cell_height_px().max(1),
            backend.baseline_y_px(),
        ))
    }

    pub fn font_family(&self) -> &str {
        &self.font_family
    }

    pub fn font_size_px(&self) -> f32 {
        self.font_size_px
    }

    pub fn padding_px(&self) -> u32 {
        self.padding_px
    }

    pub fn columns(&self) -> u32 {
        self.columns
    }

    pub fn cell_width_px(&self) -> Option<u32> {
        self.cell_width_px
    }

    pub fn cell_height_px(&self) -> Option<u32> {
        self.cell_height_px
    }

    pub const fn ligatures(&self) -> bool {
        self.ligatures
    }

    pub fn underline_metrics(&self) -> Option<WgpuCrossfontUnderlineMetrics> {
        self.backend
            .borrow()
            .as_ref()
            .map(WgpuCrossfontGlyphBackend::underline_metrics)
    }

    pub fn strikeout_metrics(&self) -> Option<WgpuCrossfontStrikeoutMetrics> {
        self.backend
            .borrow()
            .as_ref()
            .map(WgpuCrossfontGlyphBackend::strikeout_metrics)
    }

    pub fn build_for_texts<I, S>(&self, texts: I) -> WgpuTerminalGlyphAtlas
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut segments = std::collections::BTreeMap::new();
        for text in texts {
            for segment in
                crate::rendering::pty_surface::text_shaping::terminal_text_segments_with_ligatures(
                    text.as_ref(),
                    germinal_ports::rendering::frame_plan_builder::TextStyleDto::plain(),
                    self.ligatures,
                )
            {
                segments.entry(segment.glyph_key).or_insert(segment);
            }
        }

        self.build_for_segments(segments.values())
    }

    pub fn build_for_chars<I>(&self, chars: I) -> WgpuTerminalGlyphAtlas
    where
        I: IntoIterator<Item = char>,
    {
        self.build_for_glyphs(chars.into_iter().map(WgpuTerminalGlyphKey::plain))
    }

    pub fn build_for_glyphs<I>(&self, glyphs: I) -> WgpuTerminalGlyphAtlas
    where
        I: IntoIterator<Item = WgpuTerminalGlyphKey>,
    {
        let glyphs: Vec<WgpuTerminalGlyphKey> = glyphs
            .into_iter()
            .filter(|glyph| glyph.character().is_some_and(|c| !c.is_control()))
            .collect();

        if glyphs.is_empty() {
            return WgpuTerminalGlyphAtlas::empty();
        }

        let mut backend_ref = self.backend.borrow_mut();
        let Some(backend) = backend_ref.as_mut() else {
            return WgpuTerminalGlyphAtlas::empty();
        };

        let base_cell_width = self
            .cell_width_px
            .unwrap_or_else(|| backend.base_cell_width_px().max(1));
        let base_cell_height = self
            .cell_height_px
            .unwrap_or_else(|| backend.base_cell_height_px().max(1));
        let baseline_y_px = backend.baseline_y_px();

        let glyphs: Vec<RasterizedTerminalGlyph> = glyphs
            .into_iter()
            .map(|glyph| backend.rasterize_terminal_glyph(glyph))
            .collect();

        build_atlas_from_rasterized_glyphs(
            glyphs,
            base_cell_width,
            base_cell_height,
            baseline_y_px,
            self.padding_px,
            self.columns,
            self.max_texture_dimension_2d,
        )
    }

    pub fn build_for_segments<'a>(
        &self,
        segments: impl IntoIterator<Item = &'a TerminalTextSegment>,
    ) -> WgpuTerminalGlyphAtlas {
        let segments: Vec<&TerminalTextSegment> = segments.into_iter().collect();
        if segments.is_empty() {
            return WgpuTerminalGlyphAtlas::empty();
        }

        let mut backend_ref = self.backend.borrow_mut();
        let Some(backend) = backend_ref.as_mut() else {
            return WgpuTerminalGlyphAtlas::empty();
        };
        let base_cell_width = self
            .cell_width_px
            .unwrap_or_else(|| backend.base_cell_width_px().max(1));
        let base_cell_height = self
            .cell_height_px
            .unwrap_or_else(|| backend.base_cell_height_px().max(1));
        let baseline_y_px = backend.baseline_y_px();
        let glyphs = segments
            .into_iter()
            .map(|segment| {
                backend.rasterize_terminal_segment(segment, base_cell_width, base_cell_height)
            })
            .collect();

        build_atlas_from_rasterized_glyphs(
            glyphs,
            base_cell_width,
            base_cell_height,
            baseline_y_px,
            self.padding_px,
            self.columns,
            self.max_texture_dimension_2d,
        )
    }
}

#[derive(Debug, Clone)]
struct WgpuCrossfontFontFace {
    family: String,
    style: Option<String>,
}

impl WgpuCrossfontFontFace {
    fn from_terminal(face: &TerminalFontFace) -> Self {
        Self {
            family: face.family().name().to_owned(),
            style: face.style().map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone)]
struct WgpuCrossfontFontFaces {
    normal: WgpuCrossfontFontFace,
    bold: Option<WgpuCrossfontFontFace>,
    italic: Option<WgpuCrossfontFontFace>,
    bold_italic: Option<WgpuCrossfontFontFace>,
    fallbacks: Vec<String>,
    bold_weight: WgpuTerminalFontWeight,
}

impl WgpuCrossfontFontFaces {
    fn new(normal_family: String) -> Self {
        Self {
            normal: WgpuCrossfontFontFace {
                family: normal_family,
                style: None,
            },
            bold: None,
            italic: None,
            bold_italic: None,
            fallbacks: Vec::new(),
            bold_weight: WgpuTerminalFontWeight::default_bold(),
        }
    }

    fn from_terminal(config: &TerminalFontConfig) -> Self {
        Self {
            normal: WgpuCrossfontFontFace::from_terminal(config.normal()),
            bold: config.bold().map(WgpuCrossfontFontFace::from_terminal),
            italic: config.italic().map(WgpuCrossfontFontFace::from_terminal),
            bold_italic: config
                .bold_italic()
                .map(WgpuCrossfontFontFace::from_terminal),
            fallbacks: config
                .fallbacks()
                .iter()
                .map(|family| family.name().to_owned())
                .collect(),
            bold_weight: wgpu_font_weight_from_terminal(config.bold_weight()),
        }
    }

    fn face_for_style(&self, bold: bool, italic: bool) -> &WgpuCrossfontFontFace {
        match (bold, italic) {
            (false, false) => &self.normal,
            (true, false) => self.bold.as_ref().unwrap_or(&self.normal),
            (false, true) => self.italic.as_ref().unwrap_or(&self.normal),
            (true, true) => self
                .bold_italic
                .as_ref()
                .or(self.bold.as_ref())
                .or(self.italic.as_ref())
                .unwrap_or(&self.normal),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuTerminalFontWeight {
    Normal,
    Medium,
    Semibold,
    Bold,
}

impl WgpuTerminalFontWeight {
    pub const fn default_bold() -> Self {
        Self::Semibold
    }
}

struct WgpuCrossfontGlyphBackend {
    rasterizer: Rasterizer,
    primary: LoadedFontFaces,
    primary_coverage: HashMap<String, Option<FontCoverage>>,
    fallbacks: Vec<LoadedFallbackFont>,
    emoji_font_key: Option<FontKey>,
    size: Size,
    average_advance_px: u32,
    line_height_px: u32,
    baseline_y_px: i32,
    underline_metrics: WgpuCrossfontUnderlineMetrics,
    strikeout_metrics: WgpuCrossfontStrikeoutMetrics,
    glyph_cache: HashMap<WgpuTerminalGlyphKey, RasterizedTerminalGlyph>,
    cluster_rasterizer: Option<CosmicTextClusterRasterizer>,
    cluster_font_faces: WgpuCrossfontFontFaces,
}

impl WgpuCrossfontGlyphBackend {
    fn new(
        font_faces: WgpuCrossfontFontFaces,
        font_size_px: f32,
    ) -> Result<Self, WgpuCrossfontGlyphAtlasError> {
        let mut rasterizer = Rasterizer::new().map_err(WgpuCrossfontGlyphAtlasError::Rasterizer)?;
        let size = Size::from_px(font_size_px);
        let normal = load_required_face(
            &mut rasterizer,
            &font_faces.normal,
            size,
            Slant::Normal,
            Weight::Normal,
        )
        .map_err(WgpuCrossfontGlyphAtlasError::Rasterizer)?;
        let bold = load_primary_bold_face(&mut rasterizer, &font_faces, size)
            .unwrap_or_else(|| normal.clone());
        let italic = load_primary_italic_face(&mut rasterizer, &font_faces, size)
            .unwrap_or_else(|| normal.clone());
        let bold_italic = load_primary_bold_italic_face(&mut rasterizer, &font_faces, size)
            .unwrap_or_else(|| bold.clone());
        let primary = LoadedFontFaces {
            normal,
            bold,
            italic,
            bold_italic,
        };
        let mut primary_coverage = HashMap::new();
        for face in [
            &primary.normal,
            &primary.bold,
            &primary.italic,
            &primary.bold_italic,
        ] {
            primary_coverage
                .entry(face.family.clone())
                .or_insert_with(|| load_font_coverage(&face.family));
        }
        let fallbacks = font_faces
            .fallbacks
            .iter()
            .filter_map(|family| {
                load_fallback_faces(&mut rasterizer, family, size, font_faces.bold_weight)
            })
            .collect();
        let emoji_font_key = load_optional_font(&mut rasterizer, "Noto Color Emoji", size);

        // Match Alacritty's GlyphCache::load_font_metrics: load one glyph
        // from the face before asking crossfont for metrics.  Some backends
        // only finalize size/face metrics after the first glyph load; reading
        // metrics before that can produce a too-small cell advance, which makes
        // terminal columns collapse and text overlap.
        rasterizer
            .get_glyph(GlyphKey {
                font_key: primary.normal.key,
                character: 'm',
                size,
            })
            .map_err(WgpuCrossfontGlyphAtlasError::Rasterizer)?;

        let metrics = rasterizer.metrics(primary.normal.key, size).ok();
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
        let underline_metrics = metrics
            .as_ref()
            .map(|metrics| {
                crossfont_underline_metrics(
                    line_height_px,
                    baseline_y_px,
                    metrics.underline_position,
                    metrics.underline_thickness,
                )
            })
            .unwrap_or_else(|| fallback_underline_metrics(line_height_px));
        let strikeout_metrics = metrics
            .as_ref()
            .map(|metrics| {
                crossfont_strikeout_metrics(
                    line_height_px,
                    baseline_y_px,
                    metrics.strikeout_position,
                    metrics.strikeout_thickness,
                )
            })
            .unwrap_or_else(|| fallback_strikeout_metrics(line_height_px));

        Ok(Self {
            rasterizer,
            primary,
            primary_coverage,
            fallbacks,
            emoji_font_key,
            size,
            average_advance_px,
            line_height_px,
            baseline_y_px,
            underline_metrics,
            strikeout_metrics,
            glyph_cache: HashMap::new(),
            cluster_rasterizer: None,
            cluster_font_faces: font_faces,
        })
    }

    fn base_cell_width_px(&self) -> u32 {
        self.average_advance_px
    }

    fn base_cell_height_px(&self) -> u32 {
        self.line_height_px
    }

    fn baseline_y_px(&self) -> i32 {
        self.baseline_y_px
    }

    fn underline_metrics(&self) -> WgpuCrossfontUnderlineMetrics {
        self.underline_metrics
    }

    fn strikeout_metrics(&self) -> WgpuCrossfontStrikeoutMetrics {
        self.strikeout_metrics
    }

    fn rasterize_terminal_glyph(
        &mut self,
        glyph_key: WgpuTerminalGlyphKey,
    ) -> RasterizedTerminalGlyph {
        if let Some(glyph) = self.glyph_cache.get(&glyph_key) {
            return glyph.clone();
        }

        let font_key = self.font_key_for_glyph(glyph_key);

        let glyph = rasterize_terminal_glyph(&mut self.rasterizer, font_key, self.size, glyph_key);
        self.glyph_cache.insert(glyph_key, glyph.clone());
        glyph
    }

    fn rasterize_terminal_segment(
        &mut self,
        segment: &TerminalTextSegment,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> RasterizedTerminalGlyph {
        if let Some(glyph) = self.glyph_cache.get(&segment.glyph_key) {
            return glyph.clone();
        }

        let glyph = if segment.shaped {
            self.cluster_rasterizer
                .get_or_insert_with(|| {
                    CosmicTextClusterRasterizer::new(
                        self.cluster_font_faces.clone(),
                        self.size.as_px(),
                        self.baseline_y_px,
                    )
                })
                .rasterize(segment, cell_width_px, cell_height_px)
        } else {
            self.rasterize_terminal_glyph(segment.glyph_key)
        };
        self.glyph_cache.insert(segment.glyph_key, glyph.clone());
        glyph
    }

    fn font_key_for_glyph(&self, glyph: WgpuTerminalGlyphKey) -> FontKey {
        let character = glyph
            .character()
            .expect("character glyph required by the crossfont rasterizer");
        let primary = self.primary.for_glyph(glyph);
        if is_emoji_presentation_candidate(character) {
            return self
                .fallback_key_for_glyph(glyph)
                .or(self.emoji_font_key)
                .unwrap_or(primary.key);
        }

        let primary_has_glyph = self
            .primary_coverage
            .get(&primary.family)
            .and_then(Option::as_ref)
            .is_none_or(|coverage| coverage.contains(character));
        if primary_has_glyph {
            return primary.key;
        }

        self.fallback_key_for_glyph(glyph).unwrap_or(primary.key)
    }

    fn fallback_key_for_glyph(&self, glyph: WgpuTerminalGlyphKey) -> Option<FontKey> {
        let character = glyph.character()?;
        self.fallbacks
            .iter()
            .find(|fallback| fallback.coverage.contains(character))
            .map(|fallback| fallback.faces.for_glyph(glyph).key)
    }
}

#[derive(Debug, Clone)]
struct LoadedFontFace {
    family: String,
    key: FontKey,
}

#[derive(Debug, Clone)]
struct LoadedFontFaces {
    normal: LoadedFontFace,
    bold: LoadedFontFace,
    italic: LoadedFontFace,
    bold_italic: LoadedFontFace,
}

impl LoadedFontFaces {
    fn for_glyph(&self, glyph: WgpuTerminalGlyphKey) -> &LoadedFontFace {
        match (glyph.bold(), glyph.italic()) {
            (false, false) => &self.normal,
            (true, false) => &self.bold,
            (false, true) => &self.italic,
            (true, true) => &self.bold_italic,
        }
    }
}

#[derive(Clone)]
struct LoadedFallbackFont {
    coverage: FontCoverage,
    faces: LoadedFontFaces,
}

#[derive(Clone)]
enum FontCoverage {
    #[cfg(not(any(target_os = "macos", windows)))]
    Fontconfig(crossfont::ft::fc::CharSet),
    All,
}

impl FontCoverage {
    fn contains(&self, character: char) -> bool {
        match self {
            #[cfg(not(any(target_os = "macos", windows)))]
            Self::Fontconfig(charset) => charset.has_char(character),
            Self::All => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RasterizedTerminalGlyph {
    key: WgpuTerminalGlyphKey,
    cell_width: u32,
    width_px: u32,
    height_px: u32,
    left_px: i32,
    top_px: i32,
    advance_px: i32,
    pixels: Vec<u8>,
    is_color: bool,
    direct_draw_offset: Option<(i32, i32)>,
}

fn rasterize_terminal_glyph(
    rasterizer: &mut Rasterizer,
    font_key: FontKey,
    size: Size,
    key: WgpuTerminalGlyphKey,
) -> RasterizedTerminalGlyph {
    let c = key
        .character()
        .expect("character glyph required by the crossfont rasterizer");
    let cell_width = terminal_char_cell_width(c).max(1);
    let glyph_key = GlyphKey {
        character: c,
        font_key,
        size,
    };

    let glyph = match rasterizer.get_glyph(glyph_key) {
        Ok(glyph) => glyph,
        Err(source) => {
            warn!(
                codepoint = c as u32,
                character = %c.escape_unicode(),
                %source,
                "failed to rasterize terminal glyph"
            );
            let replacement = ['\u{fffd}', '?'].into_iter().find_map(|character| {
                rasterizer
                    .get_glyph(GlyphKey {
                        character,
                        font_key,
                        size,
                    })
                    .ok()
            });
            let Some(glyph) = replacement else {
                return RasterizedTerminalGlyph {
                    key,
                    cell_width,
                    width_px: 1,
                    height_px: 1,
                    left_px: 0,
                    top_px: 0,
                    advance_px: 0,
                    pixels: vec![0, 0, 0, 0],
                    is_color: false,
                    direct_draw_offset: None,
                };
            };
            glyph
        }
    };

    let width_px = glyph.width.max(0) as u32;
    let height_px = glyph.height.max(0) as u32;
    let is_color = is_color_glyph_buffer(&glyph.buffer, c);
    let pixels = glyph_rgba_pixels(&glyph.buffer, width_px, height_px, is_color);

    RasterizedTerminalGlyph {
        key,
        cell_width,
        width_px,
        height_px,
        left_px: glyph.left,
        top_px: glyph.top,
        advance_px: glyph.advance.0,
        pixels,
        is_color,
        direct_draw_offset: None,
    }
}

struct CosmicTextClusterRasterizer {
    font_system: FontSystem,
    swash_cache: SwashCache,
    font_faces: WgpuCrossfontFontFaces,
    font_size_px: f32,
    baseline_y_px: i32,
}

impl CosmicTextClusterRasterizer {
    fn new(font_faces: WgpuCrossfontFontFaces, font_size_px: f32, baseline_y_px: i32) -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            font_faces,
            font_size_px,
            baseline_y_px,
        }
    }

    fn rasterize(
        &mut self,
        segment: &TerminalTextSegment,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> RasterizedTerminalGlyph {
        let face = self
            .font_faces
            .face_for_style(segment.glyph_key.bold(), segment.glyph_key.italic());
        let primary_family = face.family.clone();
        let weight = cosmic_weight(segment.glyph_key.bold(), self.font_faces.bold_weight);
        let style = if segment.glyph_key.italic() {
            CosmicStyle::Italic
        } else {
            CosmicStyle::Normal
        };
        let mut drawn_pixels = if segment.ligature {
            self.draw_ligature(&segment.text, &primary_family, weight, style)
        } else {
            self.draw_cluster(
                &segment.text,
                &primary_family,
                weight,
                style,
                cell_height_px,
            )
        };
        if drawn_pixels.is_empty() && segment.ligature {
            drawn_pixels = self.draw_cluster(
                &segment.text,
                &primary_family,
                weight,
                style,
                cell_height_px,
            );
        }
        if drawn_pixels.is_empty() && is_emoji_text_cluster(&segment.text) {
            let fallback_families: Vec<String> = self
                .font_faces
                .fallbacks
                .iter()
                .cloned()
                .chain(
                    [
                        "Noto Emoji",
                        "Symbola",
                        "Segoe UI Emoji",
                        "Apple Color Emoji",
                    ]
                    .into_iter()
                    .map(str::to_owned),
                )
                .collect();
            for family in fallback_families {
                drawn_pixels =
                    self.draw_cluster(&segment.text, &family, weight, style, cell_height_px);
                if !drawn_pixels.is_empty() {
                    break;
                }
            }
        }
        if drawn_pixels.is_empty()
            && segment.text.chars().any(|character| {
                !character.is_whitespace() && terminal_char_cell_width(character) > 0
            })
        {
            let escaped_text = segment
                .text
                .chars()
                .flat_map(char::escape_unicode)
                .collect::<String>();
            warn!(
                text = %escaped_text,
                primary_family,
                "font shaper produced no pixels for a visible terminal cluster"
            );
            drawn_pixels =
                self.draw_cluster("\u{fffd}", &primary_family, weight, style, cell_height_px);
        }

        let is_color = is_emoji_text_cluster(&segment.text)
            && drawn_pixels
                .iter()
                .any(|(_, _, rgba)| rgba[3] > 0 && (rgba[0] != rgba[1] || rgba[1] != rgba[2]));
        rasterized_cluster_from_pixels(
            segment,
            cell_width_px,
            cell_height_px,
            is_color,
            drawn_pixels,
        )
    }

    fn draw_ligature(
        &mut self,
        text: &str,
        family: &str,
        weight: CosmicWeight,
        style: CosmicStyle,
    ) -> Vec<(i32, i32, [u8; 4])> {
        let families = [fontdb_family(family)];
        let query = fontdb::Query {
            families: &families,
            weight,
            stretch: fontdb::Stretch::Normal,
            style,
        };
        let Some(font_id) = self.font_system.db().query(&query) else {
            return Vec::new();
        };
        let Some(font) = self.font_system.get_font(font_id) else {
            return Vec::new();
        };

        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let features = [
            rustybuzz_feature(b"liga"),
            rustybuzz_feature(b"clig"),
            rustybuzz_feature(b"calt"),
        ];
        let glyph_buffer = rustybuzz::shape(font.rustybuzz(), &features, buffer);
        let scale = self.font_size_px / font.rustybuzz().units_per_em() as f32;
        let mut pen_x = 0_i32;
        let mut pen_y = 0_i32;
        let mut drawn_pixels = Vec::new();

        for (info, position) in glyph_buffer
            .glyph_infos()
            .iter()
            .zip(glyph_buffer.glyph_positions())
        {
            let glyph_x = (pen_x + position.x_offset) as f32 * scale;
            let glyph_y = -(pen_y + position.y_offset) as f32 * scale;
            let (cache_key, physical_x, physical_y) = CacheKey::new(
                font_id,
                info.glyph_id as u16,
                self.font_size_px,
                (glyph_x, glyph_y),
                CacheKeyFlags::empty(),
            );
            self.swash_cache.with_pixels(
                &mut self.font_system,
                cache_key,
                Color::rgb(255, 255, 255),
                |x, y, color| {
                    drawn_pixels.push((
                        physical_x + x,
                        self.baseline_y_px + physical_y + y,
                        color.as_rgba(),
                    ));
                },
            );
            pen_x += position.x_advance;
            pen_y += position.y_advance;
        }

        drawn_pixels
    }

    fn draw_cluster(
        &mut self,
        text: &str,
        family: &str,
        weight: CosmicWeight,
        style: CosmicStyle,
        cell_height_px: u32,
    ) -> Vec<(i32, i32, [u8; 4])> {
        let mut font_features = FontFeatures::new();
        font_features
            .enable(FeatureTag::STANDARD_LIGATURES)
            .enable(FeatureTag::CONTEXTUAL_LIGATURES)
            .enable(FeatureTag::CONTEXTUAL_ALTERNATES);
        let attrs = Attrs::new()
            .family(cosmic_family(family))
            .weight(weight)
            .style(style)
            .font_features(font_features);
        let metrics = Metrics::new(self.font_size_px, cell_height_px as f32);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        let mut buffer = buffer.borrow_with(&mut self.font_system);
        buffer.set_wrap(Wrap::None);
        buffer.set_size(None, Some(cell_height_px as f32));
        buffer.set_text(text, &attrs, Shaping::Advanced);
        buffer.shape_until_scroll(false);

        let mut drawn_pixels = Vec::new();
        buffer.draw(
            &mut self.swash_cache,
            Color::rgb(255, 255, 255),
            |x, y, width, height, color| {
                for offset_y in 0..height as i32 {
                    for offset_x in 0..width as i32 {
                        drawn_pixels.push((x + offset_x, y + offset_y, color.as_rgba()));
                    }
                }
            },
        );
        drawn_pixels
    }
}

fn rustybuzz_feature(tag: &[u8; 4]) -> rustybuzz::Feature {
    rustybuzz::Feature::new(
        rustybuzz::ttf_parser::Tag::from_bytes(tag),
        1,
        0..usize::MAX,
    )
}

fn rasterized_cluster_from_pixels(
    segment: &TerminalTextSegment,
    cell_width_px: u32,
    cell_height_px: u32,
    is_color: bool,
    drawn_pixels: Vec<(i32, i32, [u8; 4])>,
) -> RasterizedTerminalGlyph {
    let Some(min_x) = drawn_pixels.iter().map(|(x, _, _)| *x).min() else {
        return empty_cluster_glyph(segment, cell_width_px);
    };
    let min_y = drawn_pixels.iter().map(|(_, y, _)| *y).min().unwrap_or(0);
    let max_x = drawn_pixels
        .iter()
        .map(|(x, _, _)| *x)
        .max()
        .unwrap_or(min_x);
    let max_y = drawn_pixels
        .iter()
        .map(|(_, y, _)| *y)
        .max()
        .unwrap_or(min_y);
    let width_px = (max_x - min_x + 1).max(1) as u32;
    let height_px = (max_y - min_y + 1).max(1) as u32;
    let mut pixels = vec![0_u8; (width_px * height_px * 4) as usize];

    for (x, y, rgba) in drawn_pixels {
        let pixel_x = (x - min_x) as u32;
        let pixel_y = (y - min_y) as u32;
        let index = ((pixel_y * width_px + pixel_x) * 4) as usize;
        let source = if is_color { rgba } else { [0, 0, 0, rgba[3]] };
        alpha_blend_pixel(&mut pixels[index..index + 4], source);
    }

    RasterizedTerminalGlyph {
        key: segment.glyph_key,
        cell_width: segment.cell_width.max(1),
        width_px,
        height_px,
        left_px: min_x,
        top_px: 0,
        advance_px: (segment.cell_width.max(1) * cell_width_px) as i32,
        pixels,
        is_color,
        direct_draw_offset: Some((min_x, min_y.clamp(0, cell_height_px as i32))),
    }
}

fn empty_cluster_glyph(
    segment: &TerminalTextSegment,
    cell_width_px: u32,
) -> RasterizedTerminalGlyph {
    RasterizedTerminalGlyph {
        key: segment.glyph_key,
        cell_width: segment.cell_width.max(1),
        width_px: 1,
        height_px: 1,
        left_px: 0,
        top_px: 0,
        advance_px: (segment.cell_width.max(1) * cell_width_px) as i32,
        pixels: vec![0, 0, 0, 0],
        is_color: false,
        direct_draw_offset: Some((0, 0)),
    }
}

fn alpha_blend_pixel(destination: &mut [u8], source: [u8; 4]) {
    let source_alpha = u16::from(source[3]);
    let destination_alpha = u16::from(destination[3]);
    let inverse_source_alpha = 255 - source_alpha;
    let output_alpha = source_alpha + destination_alpha * inverse_source_alpha / 255;
    if output_alpha == 0 {
        return;
    }

    for channel in 0..3 {
        let source_value = u16::from(source[channel]) * source_alpha;
        let destination_value =
            u16::from(destination[channel]) * destination_alpha * inverse_source_alpha / 255;
        destination[channel] = ((source_value + destination_value) / output_alpha) as u8;
    }
    destination[3] = output_alpha as u8;
}

fn cosmic_weight(bold: bool, configured: WgpuTerminalFontWeight) -> CosmicWeight {
    if !bold {
        return CosmicWeight::NORMAL;
    }

    match configured {
        WgpuTerminalFontWeight::Normal => CosmicWeight::NORMAL,
        WgpuTerminalFontWeight::Medium => CosmicWeight::MEDIUM,
        WgpuTerminalFontWeight::Semibold => CosmicWeight::SEMIBOLD,
        WgpuTerminalFontWeight::Bold => CosmicWeight::BOLD,
    }
}

fn cosmic_family(family: &str) -> Family<'_> {
    match family.to_ascii_lowercase().as_str() {
        "monospace" => Family::Monospace,
        "sans-serif" => Family::SansSerif,
        "serif" => Family::Serif,
        "cursive" => Family::Cursive,
        "fantasy" => Family::Fantasy,
        _ => Family::Name(family),
    }
}

fn fontdb_family(family: &str) -> fontdb::Family<'_> {
    match family.to_ascii_lowercase().as_str() {
        "monospace" => fontdb::Family::Monospace,
        "sans-serif" => fontdb::Family::SansSerif,
        "serif" => fontdb::Family::Serif,
        "cursive" => fontdb::Family::Cursive,
        "fantasy" => fontdb::Family::Fantasy,
        _ => fontdb::Family::Name(family),
    }
}

fn is_emoji_text_cluster(text: &str) -> bool {
    text.contains('\u{fe0f}')
        || text.contains('\u{200d}')
        || text.chars().any(is_emoji_presentation_candidate)
}

fn build_atlas_from_rasterized_glyphs(
    mut glyphs: Vec<RasterizedTerminalGlyph>,
    base_cell_width: u32,
    base_cell_height: u32,
    baseline_y_px: i32,
    padding_px: u32,
    columns: u32,
    max_texture_dimension_2d: u32,
) -> WgpuTerminalGlyphAtlas {
    if glyphs.is_empty() {
        return WgpuTerminalGlyphAtlas::empty();
    }

    // A single malformed fallback glyph must not make wgpu create a texture
    // larger than the selected adapter supports. Keep the atlas at a practical
    // upper bound as well; a 4096x4096 RGBA atlas already holds tens of
    // thousands of ordinary terminal glyphs and uses 64 MiB at the limit.
    let max_texture_dimension_2d = max_texture_dimension_2d.clamp(1, 4096);
    let padding_px = padding_px.min(max_texture_dimension_2d.saturating_sub(1));
    let max_bitmap_dimension = max_texture_dimension_2d
        .saturating_sub(padding_px.saturating_mul(2))
        .max(1);
    for glyph in &mut glyphs {
        if glyph.width_px.max(1) <= max_bitmap_dimension
            && glyph.height_px.max(1) <= max_bitmap_dimension
        {
            continue;
        }

        warn!(
            codepoint = glyph.key.packed_id(),
            width_px = glyph.width_px,
            height_px = glyph.height_px,
            max_bitmap_dimension,
            "discarding oversized rasterized glyph bitmap"
        );
        glyph.width_px = 1;
        glyph.height_px = 1;
        glyph.left_px = 0;
        glyph.top_px = 0;
        glyph.pixels = vec![0, 0, 0, 0];
        glyph.is_color = false;
        glyph.direct_draw_offset = None;
    }

    // Alacritty never scales a rasterized glyph bitmap into the terminal cell.
    // The terminal cell decides layout; the glyph bitmap is placed at its native
    // rasterized size using font bearings/baseline.  The previous implementation
    // stored a whole terminal-cell-sized rectangle in the atlas and mapped that
    // rectangle onto a terminal-cell quad, which stretched/squashed every glyph
    // and made the text look short, fat, and blurry.
    let max_bitmap_width = glyphs
        .iter()
        .map(|glyph| glyph.width_px.max(1))
        .max()
        .unwrap_or(base_cell_width.max(1));
    let max_bitmap_height = glyphs
        .iter()
        .map(|glyph| glyph.height_px.max(1))
        .max()
        .unwrap_or(base_cell_height.max(1));

    let cell_stride_width = max_bitmap_width + padding_px;
    let cell_stride_height = max_bitmap_height + padding_px;
    let layout = GlyphAtlasGridLayout::new(
        glyphs.len() as u32,
        columns,
        cell_stride_width,
        cell_stride_height,
        padding_px,
        max_texture_dimension_2d,
    );

    let atlas_width = layout.atlas_width;
    let atlas_height = layout.atlas_height;

    let pixel_len = u64::from(atlas_width)
        .saturating_mul(u64::from(atlas_height))
        .saturating_mul(4);
    let Ok(pixel_len) = usize::try_from(pixel_len) else {
        warn!(
            atlas_width,
            atlas_height, "glyph atlas allocation is too large"
        );
        return WgpuTerminalGlyphAtlas::empty();
    };
    let mut pixels = vec![0u8; pixel_len];
    let mut entries = HashMap::new();

    if layout.glyph_capacity < glyphs.len() as u32 {
        warn!(
            glyph_count = glyphs.len(),
            glyph_capacity = layout.glyph_capacity,
            atlas_width,
            atlas_height,
            "glyph atlas reached the device-safe capacity; excess glyphs were skipped"
        );
    }

    for (index, glyph) in glyphs
        .into_iter()
        .take(layout.glyph_capacity as usize)
        .enumerate()
    {
        let index = index as u32;
        let col = index % layout.columns;
        let row = index / layout.columns;

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
            terminal_glyph_draw_offset(&glyph, base_cell_width, baseline_y_px);

        entries.insert(
            glyph.key,
            WgpuTerminalGlyphAtlasEntry {
                codepoint: glyph.key.packed_id(),
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
            },
        );
    }

    WgpuTerminalGlyphAtlas {
        width_px: atlas_width,
        height_px: atlas_height,
        pixels,
        entries,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GlyphAtlasGridLayout {
    columns: u32,
    row_count: u32,
    atlas_width: u32,
    atlas_height: u32,
    glyph_capacity: u32,
}

impl GlyphAtlasGridLayout {
    fn new(
        glyph_count: u32,
        preferred_columns: u32,
        cell_stride_width: u32,
        cell_stride_height: u32,
        padding_px: u32,
        max_texture_dimension_2d: u32,
    ) -> Self {
        let glyph_count = glyph_count.max(1);
        let usable_dimension = max_texture_dimension_2d.max(padding_px.saturating_add(1));
        let usable_width = usable_dimension.saturating_sub(padding_px).max(1);
        let usable_height = usable_dimension.saturating_sub(padding_px).max(1);
        let max_columns = (usable_width / cell_stride_width).max(1).min(glyph_count);
        let max_rows = (usable_height / cell_stride_height).max(1);
        let glyph_capacity = glyph_count.min(max_columns.saturating_mul(max_rows));
        let preferred_columns = preferred_columns.max(1).min(glyph_capacity);
        let min_columns_needed = glyph_capacity.div_ceil(max_rows).max(1);
        let columns = preferred_columns
            .max(min_columns_needed)
            .min(max_columns)
            .max(1);
        let row_count = glyph_capacity.div_ceil(columns);
        let atlas_width = padding_px.saturating_add(columns.saturating_mul(cell_stride_width));
        let atlas_height = padding_px.saturating_add(row_count.saturating_mul(cell_stride_height));

        Self {
            columns,
            row_count,
            atlas_width,
            atlas_height,
            glyph_capacity,
        }
    }
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
                pixels[dst_index..dst_index + 4]
                    .copy_from_slice(&glyph.pixels[src_index..src_index + 4]);
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

fn crossfont_underline_metrics(
    cell_height_px: u32,
    baseline_y_px: i32,
    underline_position: f32,
    underline_thickness: f32,
) -> WgpuCrossfontUnderlineMetrics {
    let thickness_px = underline_thickness.round().max(1.0) as u32;
    let underline_center_y = baseline_y_px as f32 - underline_position;
    let offset_y_px = (underline_center_y - thickness_px as f32 / 2.0)
        .round()
        .clamp(0.0, cell_height_px.saturating_sub(thickness_px) as f32)
        as u32;

    WgpuCrossfontUnderlineMetrics::new(offset_y_px, thickness_px)
}

fn fallback_underline_metrics(cell_height_px: u32) -> WgpuCrossfontUnderlineMetrics {
    let thickness_px = cell_height_px.div_ceil(16).max(1);
    WgpuCrossfontUnderlineMetrics::new(
        cell_height_px.saturating_sub(thickness_px.saturating_add(1)),
        thickness_px,
    )
}

fn crossfont_strikeout_metrics(
    cell_height_px: u32,
    baseline_y_px: i32,
    strikeout_position: f32,
    strikeout_thickness: f32,
) -> WgpuCrossfontStrikeoutMetrics {
    let thickness_px = strikeout_thickness.round().max(1.0) as u32;
    let strikeout_center_y = baseline_y_px as f32 - strikeout_position;
    let offset_y_px = (strikeout_center_y - thickness_px as f32 / 2.0)
        .round()
        .clamp(0.0, cell_height_px.saturating_sub(thickness_px) as f32)
        as u32;

    WgpuCrossfontStrikeoutMetrics::new(offset_y_px, thickness_px)
}

fn fallback_strikeout_metrics(cell_height_px: u32) -> WgpuCrossfontStrikeoutMetrics {
    let thickness_px = cell_height_px.div_ceil(16).max(1);
    WgpuCrossfontStrikeoutMetrics::new(cell_height_px.saturating_mul(9) / 16, thickness_px)
}

fn terminal_glyph_draw_offset(
    glyph: &RasterizedTerminalGlyph,
    base_cell_width: u32,
    baseline_y_px: i32,
) -> (i32, i32) {
    if let Some(offset) = glyph.direct_draw_offset {
        return offset;
    }
    let terminal_width = base_cell_width as i32 * glyph.cell_width.max(1) as i32;

    // Positive left bearings are honored.  Negative bearings are allowed to
    // overhang like Alacritty instead of being baked into the atlas and scaled.
    let draw_x = glyph.left_px;

    // Alacritty places bitmap glyphs relative to the font baseline derived from
    // crossfont metrics. The terminal cell decides layout; the bitmap keeps its
    // native size and font bearing.
    let draw_y = baseline_y_px - glyph.top_px;

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

fn load_required_face(
    rasterizer: &mut Rasterizer,
    face: &WgpuCrossfontFontFace,
    size: Size,
    slant: Slant,
    weight: Weight,
) -> Result<LoadedFontFace, crossfont::Error> {
    let font_desc = FontDesc::new(
        face.family.clone(),
        face.style
            .as_ref()
            .map_or(Style::Description { slant, weight }, |style| {
                Style::Specific(style.clone())
            }),
    );
    rasterizer
        .load_font(&font_desc, size)
        .map(|key| LoadedFontFace {
            family: face.family.clone(),
            key,
        })
}

fn load_primary_bold_face(
    rasterizer: &mut Rasterizer,
    faces: &WgpuCrossfontFontFaces,
    size: Size,
) -> Option<LoadedFontFace> {
    if let Some(face) = faces.bold.as_ref() {
        return load_required_face(rasterizer, face, size, Slant::Normal, Weight::Bold).ok();
    }

    load_font_for_terminal_weight(rasterizer, &faces.normal.family, size, faces.bold_weight).map(
        |key| LoadedFontFace {
            family: faces.normal.family.clone(),
            key,
        },
    )
}

fn load_primary_italic_face(
    rasterizer: &mut Rasterizer,
    faces: &WgpuCrossfontFontFaces,
    size: Size,
) -> Option<LoadedFontFace> {
    let face = faces.italic.as_ref().unwrap_or(&faces.normal);
    load_required_face(rasterizer, face, size, Slant::Italic, Weight::Normal).ok()
}

fn load_primary_bold_italic_face(
    rasterizer: &mut Rasterizer,
    faces: &WgpuCrossfontFontFaces,
    size: Size,
) -> Option<LoadedFontFace> {
    let face = faces.bold_italic.as_ref().unwrap_or(&faces.normal);
    load_required_face(
        rasterizer,
        face,
        size,
        Slant::Italic,
        crossfont_weight(faces.bold_weight),
    )
    .ok()
}

fn load_fallback_faces(
    rasterizer: &mut Rasterizer,
    family: &str,
    size: Size,
    bold_weight: WgpuTerminalFontWeight,
) -> Option<LoadedFallbackFont> {
    let face = WgpuCrossfontFontFace {
        family: family.to_owned(),
        style: None,
    };
    let normal = load_required_face(rasterizer, &face, size, Slant::Normal, Weight::Normal).ok()?;
    let bold = load_font_for_terminal_weight(rasterizer, family, size, bold_weight)
        .map(|key| LoadedFontFace {
            family: family.to_owned(),
            key,
        })
        .unwrap_or_else(|| normal.clone());
    let italic = load_required_face(rasterizer, &face, size, Slant::Italic, Weight::Normal)
        .unwrap_or_else(|_| normal.clone());
    let bold_italic = load_required_face(
        rasterizer,
        &face,
        size,
        Slant::Italic,
        crossfont_weight(bold_weight),
    )
    .unwrap_or_else(|_| bold.clone());

    Some(LoadedFallbackFont {
        coverage: load_font_coverage(family).unwrap_or_else(FontCoverage::all),
        faces: LoadedFontFaces {
            normal,
            bold,
            italic,
            bold_italic,
        },
    })
}

impl FontCoverage {
    fn all() -> Self {
        Self::All
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn load_font_coverage(family: &str) -> Option<FontCoverage> {
    use crossfont::ft::fc::{Config, MatchKind, Pattern, font_match};

    let config = Config::get_current();
    let mut pattern = Pattern::new();
    pattern.add_family(family);
    pattern.config_substitute(config, MatchKind::Pattern);
    pattern.default_substitute();
    let matched = font_match(config, &pattern)?;
    matched
        .get_charset()
        .map(ToOwned::to_owned)
        .map(FontCoverage::Fontconfig)
}

#[cfg(any(target_os = "macos", windows))]
fn load_font_coverage(_family: &str) -> Option<FontCoverage> {
    None
}

fn crossfont_weight(weight: WgpuTerminalFontWeight) -> Weight {
    match weight {
        WgpuTerminalFontWeight::Normal | WgpuTerminalFontWeight::Medium => Weight::Normal,
        WgpuTerminalFontWeight::Semibold | WgpuTerminalFontWeight::Bold => Weight::Bold,
    }
}

fn wgpu_font_weight_from_terminal(weight: TerminalFontWeight) -> WgpuTerminalFontWeight {
    match weight {
        TerminalFontWeight::Normal => WgpuTerminalFontWeight::Normal,
        TerminalFontWeight::Medium => WgpuTerminalFontWeight::Medium,
        TerminalFontWeight::Semibold => WgpuTerminalFontWeight::Semibold,
        TerminalFontWeight::Bold => WgpuTerminalFontWeight::Bold,
    }
}

fn load_optional_font(rasterizer: &mut Rasterizer, family: &str, size: Size) -> Option<FontKey> {
    load_font_with_weight(rasterizer, family, size, Weight::Normal)
}

fn load_font_with_weight(
    rasterizer: &mut Rasterizer,
    family: &str,
    size: Size,
    weight: Weight,
) -> Option<FontKey> {
    let font_desc = FontDesc::new(
        family.to_owned(),
        Style::Description {
            slant: Slant::Normal,
            weight,
        },
    );

    rasterizer.load_font(&font_desc, size).ok()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn load_font_for_terminal_weight(
    rasterizer: &mut Rasterizer,
    family: &str,
    size: Size,
    weight: WgpuTerminalFontWeight,
) -> Option<FontKey> {
    let style_names: &[&str] = match weight {
        WgpuTerminalFontWeight::Normal => &["Regular", "Book"],
        WgpuTerminalFontWeight::Medium => &["Medium"],
        WgpuTerminalFontWeight::Semibold => &["Semibold", "SemiBold", "DemiBold", "Demi Bold"],
        WgpuTerminalFontWeight::Bold => &["Bold"],
    };

    for style_name in style_names {
        if let Some(font_key) = load_font_with_style_name(rasterizer, family, size, style_name) {
            return Some(font_key);
        }
    }

    match weight {
        WgpuTerminalFontWeight::Normal => {
            load_font_with_weight(rasterizer, family, size, Weight::Normal)
        }
        WgpuTerminalFontWeight::Medium => {
            load_font_with_weight(rasterizer, family, size, Weight::Normal)
        }
        WgpuTerminalFontWeight::Semibold | WgpuTerminalFontWeight::Bold => {
            load_font_with_weight(rasterizer, family, size, Weight::Bold)
        }
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn load_font_for_terminal_weight(
    rasterizer: &mut Rasterizer,
    family: &str,
    size: Size,
    weight: WgpuTerminalFontWeight,
) -> Option<FontKey> {
    match weight {
        WgpuTerminalFontWeight::Normal | WgpuTerminalFontWeight::Medium => {
            load_font_with_weight(rasterizer, family, size, Weight::Normal)
        }
        WgpuTerminalFontWeight::Semibold | WgpuTerminalFontWeight::Bold => {
            load_font_with_weight(rasterizer, family, size, Weight::Bold)
        }
    }
}

fn load_font_with_style_name(
    rasterizer: &mut Rasterizer,
    family: &str,
    size: Size,
    style_name: &str,
) -> Option<FontKey> {
    let font_desc = FontDesc::new(family.to_owned(), Style::Specific(style_name.to_owned()));

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

#[cfg(test)]
mod tests {
    use super::{
        FontCoverage, GlyphAtlasGridLayout, RasterizedTerminalGlyph, WgpuCrossfontFontFaces,
        WgpuCrossfontGlyphAtlasBuilder, WgpuCrossfontGlyphBackend, WgpuCrossfontStrikeoutMetrics,
        WgpuCrossfontUnderlineMetrics, WgpuTerminalGlyphKey, crossfont_strikeout_metrics,
        crossfont_underline_metrics, terminal_glyph_draw_offset,
    };

    #[test]
    fn crossfont_backend_interprets_font_size_as_physical_pixels() {
        let backend = WgpuCrossfontGlyphBackend::new(
            WgpuCrossfontFontFaces::new("monospace".to_owned()),
            18.0,
        )
        .expect("the platform monospace font should load");

        assert!((backend.size.as_px() - 18.0).abs() < 0.01);
    }

    #[test]
    fn converts_crossfont_underline_metrics_to_cell_pixels() {
        let metrics = crossfont_underline_metrics(32, 25, -3.0, 1.6);

        assert_eq!(metrics, WgpuCrossfontUnderlineMetrics::new(27, 2));
    }

    #[test]
    fn converts_crossfont_strikeout_metrics_to_cell_pixels() {
        let metrics = crossfont_strikeout_metrics(32, 25, 9.0, 1.6);

        assert_eq!(metrics, WgpuCrossfontStrikeoutMetrics::new(15, 2));
    }

    #[test]
    fn short_text_glyph_preserves_font_baseline_offset() {
        let glyph = RasterizedTerminalGlyph {
            key: WgpuTerminalGlyphKey::styled('"', false, false),
            cell_width: 1,
            width_px: 4,
            height_px: 6,
            left_px: 3,
            top_px: 18,
            advance_px: 12,
            pixels: vec![],
            is_color: false,
            direct_draw_offset: None,
        };

        assert_eq!(terminal_glyph_draw_offset(&glyph, 12, 24), (3, 6));
    }

    #[test]
    fn rasterizes_normal_bold_italic_and_bold_italic_glyphs() {
        let glyphs = [
            WgpuTerminalGlyphKey::styled('A', false, false),
            WgpuTerminalGlyphKey::styled('A', true, false),
            WgpuTerminalGlyphKey::styled('A', false, true),
            WgpuTerminalGlyphKey::styled('A', true, true),
        ];
        let atlas = WgpuCrossfontGlyphAtlasBuilder::new("monospace", 16.0)
            .expect("the platform monospace font should load")
            .build_for_glyphs(glyphs);

        for glyph in glyphs {
            assert!(atlas.has_glyph_key(glyph));
        }
    }

    #[test]
    fn includes_zero_width_glyphs_in_the_atlas() {
        let builder = WgpuCrossfontGlyphAtlasBuilder::new("monospace", 16.0)
            .expect("the platform monospace font should load");
        let segment = crate::rendering::pty_surface::text_shaping::terminal_text_segments(
            "中\u{301}",
            germinal_ports::rendering::frame_plan_builder::TextStyleDto::plain(),
        )
        .remove(0);
        let atlas = builder.build_for_texts(["中\u{301}"]);

        assert!(segment.shaped);
        assert!(atlas.has_glyph_key(segment.glyph_key));
    }

    #[test]
    fn shapes_joined_emoji_into_one_atlas_entry() {
        let builder = WgpuCrossfontGlyphAtlasBuilder::new("monospace", 16.0)
            .expect("the platform monospace font should load");
        let segment = crate::rendering::pty_surface::text_shaping::terminal_text_segments(
            "👩\u{200d}💻",
            germinal_ports::rendering::frame_plan_builder::TextStyleDto::plain(),
        )
        .remove(0);
        let atlas = builder.build_for_texts([segment.text.as_str()]);
        let entry = atlas.entry_for_key(segment.glyph_key).unwrap();

        assert!(segment.shaped);
        assert!(entry.width_px > 1);
        assert!(entry.height_px > 1);
        assert!(atlas.non_zero_pixel_count() > 0);
    }

    #[test]
    fn all_font_coverage_accepts_every_unicode_character() {
        assert!(FontCoverage::all().contains('🙂'));
        assert!(FontCoverage::all().contains('界'));
    }

    #[test]
    fn grows_columns_to_stay_within_texture_height_limit() {
        let layout = GlyphAtlasGridLayout::new(4600, 16, 20, 30, 2, 8192);

        assert!(layout.columns > 16);
        assert!(layout.atlas_width <= 8192);
        assert!(layout.atlas_height <= 8192);
    }

    #[test]
    fn caps_columns_when_width_limit_is_tighter_than_preference() {
        let layout = GlyphAtlasGridLayout::new(1000, 16, 700, 10, 2, 8192);

        assert_eq!(layout.columns, 11);
        assert!(layout.atlas_width <= 8192);
    }

    #[test]
    fn caps_glyph_count_when_the_texture_cannot_hold_every_entry() {
        let layout = GlyphAtlasGridLayout::new(100, 16, 20, 30, 2, 64);

        assert_eq!(layout.glyph_capacity, 6);
        assert!(layout.atlas_width <= 64);
        assert!(layout.atlas_height <= 64);
    }
}
