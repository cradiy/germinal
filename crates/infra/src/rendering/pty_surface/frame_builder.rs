use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use germinal_ports::{
    pty_host::width::terminal_text_cell_width,
    rendering::{
        render_target_id::RenderTargetId, renderer_backend::RendererBackend,
        surface_snapshot::RenderSurfaceSnapshot,
    },
    seq::Seq,
};

use crate::rendering::pty_surface::{
    buffer_uploader::{WgpuBufferUploadBytes, WgpuBufferUploader},
    crossfont_glyph_atlas::{WgpuCrossfontGlyphAtlasBuilder, WgpuCrossfontGlyphAtlasError},
    draw_indexed::WgpuDrawIndexedPlan,
    glyph_atlas_frame::{
        WgpuTerminalGlyphAtlasFrame, WgpuTerminalGlyphAtlasFrameBuilder,
        WgpuTerminalGlyphAtlasSourceKind,
    },
    glyph_uv_mapper::WgpuTerminalGlyphUvMapResult,
    quad_vertex_buffer_builder::{WgpuQuadVertexBufferBuilder, WgpuVertexBuffer},
    render_pass_plan::WgpuTerminalRenderPassPlan,
    renderer_backend::{WgpuRendererBackend, WgpuRendererConfig},
    shader::WgpuViewportUniform,
    video_surface_registry::WgpuVideoSurfaceRegistry,
    viewport_bind_group::{WgpuViewportBindGroupFactory, WgpuViewportUploadBytes},
};

#[derive(Debug, Clone)]
pub struct WgpuTerminalFrameBuilder {
    renderer_config: WgpuRendererConfig,
    glyph_atlas_frame_builder: WgpuTerminalGlyphAtlasFrameBuilder,
    vertex_buffer_builder: WgpuQuadVertexBufferBuilder,
    renderer_backend: Rc<RefCell<WgpuRendererBackend>>,
    video_surface_registry: WgpuVideoSurfaceRegistry,
    prepared_frame_cache: Rc<RefCell<HashMap<RenderTargetId, WgpuTerminalPreparedFrameCacheEntry>>>,
}

impl WgpuTerminalFrameBuilder {
    pub fn new(renderer_config: WgpuRendererConfig) -> Self {
        Self {
            renderer_config,
            glyph_atlas_frame_builder: WgpuTerminalGlyphAtlasFrameBuilder::debug_5x7(),
            vertex_buffer_builder: WgpuQuadVertexBufferBuilder::new(),
            renderer_backend: Rc::new(RefCell::new(WgpuRendererBackend::new(renderer_config))),
            video_surface_registry: WgpuVideoSurfaceRegistry::default(),
            prepared_frame_cache: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    pub fn with_glyph_atlas_frame_builder(
        mut self,
        glyph_atlas_frame_builder: WgpuTerminalGlyphAtlasFrameBuilder,
    ) -> Self {
        self.glyph_atlas_frame_builder = glyph_atlas_frame_builder;
        self
    }

    pub fn with_debug_5x7_glyph_atlas(mut self) -> Self {
        self.glyph_atlas_frame_builder = WgpuTerminalGlyphAtlasFrameBuilder::debug_5x7();
        self
    }

    pub fn with_crossfont_glyph_atlas(
        mut self,
        font_family: impl Into<String>,
        font_size_px: f32,
    ) -> Result<Self, WgpuCrossfontGlyphAtlasError> {
        self.glyph_atlas_frame_builder =
            WgpuTerminalGlyphAtlasFrameBuilder::crossfont(font_family, font_size_px)?;

        Ok(self)
    }

    pub fn with_crossfont_glyph_atlas_builder(
        mut self,
        crossfont_builder: WgpuCrossfontGlyphAtlasBuilder,
    ) -> Self {
        self.glyph_atlas_frame_builder =
            WgpuTerminalGlyphAtlasFrameBuilder::with_crossfont_builder(crossfont_builder);

        self
    }

    pub fn renderer_config(&self) -> WgpuRendererConfig {
        self.renderer_config
    }

    pub fn video_surface_registry(&self) -> &WgpuVideoSurfaceRegistry {
        &self.video_surface_registry
    }

    pub fn with_video_surface_registry(
        mut self,
        video_surface_registry: WgpuVideoSurfaceRegistry,
    ) -> Self {
        self.video_surface_registry = video_surface_registry;
        self
    }

    pub fn remove_render_target(&self, target_id: RenderTargetId) {
        self.glyph_atlas_frame_builder
            .remove_render_target(target_id);
        self.video_surface_registry.remove_render_target(target_id);
        self.prepared_frame_cache.borrow_mut().remove(&target_id);
    }

    pub fn glyph_atlas_source_kind(&self) -> WgpuTerminalGlyphAtlasSourceKind {
        self.glyph_atlas_frame_builder.source_kind()
    }

    pub fn build(
        &self,
        surface_snapshot: &RenderSurfaceSnapshot,
        viewport: WgpuViewportUniform,
    ) -> WgpuTerminalPreparedFrame {
        self.build_with_renderer_config(surface_snapshot, viewport, self.renderer_config)
    }

    pub fn build_with_renderer_config(
        &self,
        surface_snapshot: &RenderSurfaceSnapshot,
        viewport: WgpuViewportUniform,
        renderer_config: WgpuRendererConfig,
    ) -> WgpuTerminalPreparedFrame {
        let total_started_at = Instant::now();
        self.video_surface_registry.sync_snapshot(surface_snapshot);
        let cache_key = WgpuTerminalPreparedFrameCacheKey {
            seq: surface_snapshot.latest_seq,
            viewport,
            renderer_config,
            default_background: surface_snapshot.default_background,
            cursor: surface_snapshot.cursor,
            ime_preedit: surface_snapshot.ime_preedit.clone(),
        };
        if surface_snapshot.latest_seq.value() != 0
            && let Some(entry) = self
                .prepared_frame_cache
                .borrow()
                .get(&surface_snapshot.target_id)
            && entry.key == cache_key
        {
            let mut frame = entry.frame.clone();
            frame.glyph_atlas_frame.cache_hit = true;
            frame.timings = WgpuTerminalPreparedFrameTimings {
                total: total_started_at.elapsed(),
                ..Default::default()
            };
            return frame;
        }

        let atlas_build_started_at = Instant::now();
        let glyph_atlas_frame = self.glyph_atlas_frame_builder.build(surface_snapshot);
        let atlas_build_time = atlas_build_started_at.elapsed();

        let render_surface_started_at = Instant::now();
        let (render_surface_time, vertex_buffer, glyph_uv_map_result, vertex_build_time) = {
            let mut renderer = self.renderer_backend.borrow_mut();

            if renderer.config() != renderer_config {
                *renderer = WgpuRendererBackend::new(renderer_config);
            }
            renderer.set_text_shaping(
                self.glyph_atlas_frame_builder.source_kind()
                    == WgpuTerminalGlyphAtlasSourceKind::Crossfont,
            );
            renderer.set_ligatures(self.glyph_atlas_frame_builder.ligatures());
            renderer.set_underline_metrics(self.glyph_atlas_frame_builder.underline_metrics());
            renderer.set_strikeout_metrics(self.glyph_atlas_frame_builder.strikeout_metrics());

            renderer.render_surface(surface_snapshot);
            let render_surface_time = render_surface_started_at.elapsed();
            let (vertex_buffer, glyph_uv_map_result, vertex_build_time) =
                renderer.with_quads(|quads| {
                    let vertex_build_started_at = Instant::now();
                    let (vertex_buffer, glyph_uv_map_result) = self
                        .vertex_buffer_builder
                        .build_with_glyph_atlas(quads, glyph_atlas_frame.atlas.as_ref());
                    let vertex_build_time = vertex_build_started_at.elapsed();
                    (vertex_buffer, glyph_uv_map_result, vertex_build_time)
                });

            (
                render_surface_time,
                vertex_buffer,
                glyph_uv_map_result,
                vertex_build_time,
            )
        };

        let uv_map_started_at = Instant::now();
        let uv_map_time = uv_map_started_at.elapsed();

        let upload_bytes_started_at = Instant::now();
        let uploader = WgpuBufferUploader::new();
        let upload_bytes = uploader.build_upload_bytes(&vertex_buffer);
        let upload_bytes_time = upload_bytes_started_at.elapsed();

        let draw_plan_started_at = Instant::now();
        let draw_plan = WgpuDrawIndexedPlan::from_upload_bytes(&upload_bytes);
        let render_pass_plan = draw_plan.map(WgpuTerminalRenderPassPlan::new);
        let draw_plan_time = draw_plan_started_at.elapsed();

        let renderer_lines_started_at = Instant::now();
        let renderer_lines = if cfg!(test) {
            renderer_lines_of(surface_snapshot)
        } else {
            Vec::new()
        };
        let renderer_lines_time = renderer_lines_started_at.elapsed();

        let viewport_upload_started_at = Instant::now();
        let viewport_factory = WgpuViewportBindGroupFactory::new();
        let viewport_upload_bytes = viewport_factory.build_upload_bytes(viewport);
        let viewport_upload_time = viewport_upload_started_at.elapsed();

        let timings = WgpuTerminalPreparedFrameTimings {
            render_surface: render_surface_time,
            quads_clone: Duration::ZERO,
            vertex_build: vertex_build_time,
            atlas_build: atlas_build_time,
            uv_map: uv_map_time,
            upload_bytes: upload_bytes_time,
            draw_plan: draw_plan_time,
            renderer_lines: renderer_lines_time,
            viewport_upload: viewport_upload_time,
            total: total_started_at.elapsed(),
        };

        let frame = WgpuTerminalPreparedFrame {
            target_id: surface_snapshot.target_id,
            seq: surface_snapshot.latest_seq,
            viewport,
            vertex_buffer,
            upload_bytes,
            viewport_upload_bytes,
            draw_plan,
            render_pass_plan,
            renderer_lines,
            glyph_count: glyph_uv_map_result.glyph_vertices / 4,
            glyph_atlas_frame,
            glyph_uv_map_result,
            timings,
        };
        if surface_snapshot.latest_seq.value() != 0 {
            self.prepared_frame_cache.borrow_mut().insert(
                surface_snapshot.target_id,
                WgpuTerminalPreparedFrameCacheEntry {
                    key: cache_key,
                    frame: frame.clone(),
                },
            );
        }
        frame
    }
}

#[derive(Debug, Clone, PartialEq)]
struct WgpuTerminalPreparedFrameCacheKey {
    seq: Seq,
    viewport: WgpuViewportUniform,
    renderer_config: WgpuRendererConfig,
    default_background: germinal_ports::rendering::frame_plan_builder::RgbColorDto,
    cursor: Option<germinal_ports::rendering::surface_snapshot::RenderSurfaceCursorSnapshot>,
    ime_preedit:
        Option<germinal_ports::rendering::surface_snapshot::RenderSurfaceImePreeditSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
struct WgpuTerminalPreparedFrameCacheEntry {
    key: WgpuTerminalPreparedFrameCacheKey,
    frame: WgpuTerminalPreparedFrame,
}

impl Default for WgpuTerminalFrameBuilder {
    fn default() -> Self {
        Self::new(WgpuRendererConfig::default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WgpuTerminalPreparedFrameTimings {
    pub render_surface: Duration,
    pub quads_clone: Duration,
    pub vertex_build: Duration,
    pub atlas_build: Duration,
    pub uv_map: Duration,
    pub upload_bytes: Duration,
    pub draw_plan: Duration,
    pub renderer_lines: Duration,
    pub viewport_upload: Duration,
    pub total: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WgpuTerminalPreparedFrame {
    pub target_id: RenderTargetId,
    pub seq: Seq,
    pub viewport: WgpuViewportUniform,
    pub vertex_buffer: WgpuVertexBuffer,
    pub upload_bytes: WgpuBufferUploadBytes,
    pub viewport_upload_bytes: WgpuViewportUploadBytes,

    /// Compatibility field for old tests/debug output.
    pub draw_plan: Option<WgpuDrawIndexedPlan>,

    pub render_pass_plan: Option<WgpuTerminalRenderPassPlan>,

    /// Compatibility field for old tests/debug output.
    pub renderer_lines: Vec<String>,

    pub glyph_count: usize,
    pub glyph_atlas_frame: WgpuTerminalGlyphAtlasFrame,
    pub glyph_uv_map_result: WgpuTerminalGlyphUvMapResult,
    pub timings: WgpuTerminalPreparedFrameTimings,
}

impl WgpuTerminalPreparedFrame {
    pub fn quad_count(&self) -> usize {
        self.vertex_buffer.vertices.len() / 4
    }

    pub fn vertex_count(&self) -> usize {
        self.vertex_buffer.vertices.len()
    }

    pub fn index_count(&self) -> usize {
        self.vertex_buffer.indices.len()
    }

    pub fn has_draw_work(&self) -> bool {
        !self.upload_bytes.is_empty() && self.render_pass_plan.is_some()
    }

    pub fn has_glyph_atlas_work(&self) -> bool {
        self.glyph_atlas_frame.has_upload_work()
    }

    pub fn has_mapped_glyph_uvs(&self) -> bool {
        self.glyph_uv_map_result.mapped_all()
    }

    pub fn glyph_atlas_source_kind(&self) -> WgpuTerminalGlyphAtlasSourceKind {
        self.glyph_atlas_frame.source
    }
}

fn renderer_lines_of(surface_snapshot: &RenderSurfaceSnapshot) -> Vec<String> {
    surface_snapshot
        .rows
        .iter()
        .map(|row| {
            let mut line = String::new();
            let mut cursor_x = 0usize;

            for run in &row.runs {
                let run_x = run.x as usize;

                if run_x > cursor_x {
                    line.push_str(&" ".repeat(run_x - cursor_x));
                    cursor_x = run_x;
                }

                line.push_str(&run.text);
                cursor_x += terminal_text_cell_width(&run.text) as usize;
            }

            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use germinal_ports::{
        rendering::{
            frame_plan_builder::{RgbColorDto, TextStyleDto},
            render_target_id::RenderTargetId,
            surface_snapshot::{
                RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot, RenderSurfaceRowSnapshot,
                RenderSurfaceRunSnapshot, RenderSurfaceSnapshot,
            },
        },
        seq::Seq,
    };

    use super::*;

    fn debug_snapshot(seq: u64) -> RenderSurfaceSnapshot {
        RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(seq),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "cached frame".to_string(),
                    style: TextStyleDto::plain(),
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![0],
            cursor: None,
            ime_preedit: None,
        }
    }

    #[test]
    fn unchanged_snapshot_reuses_prepared_vertex_and_atlas_storage() {
        let builder = WgpuTerminalFrameBuilder::default();
        let snapshot = debug_snapshot(1);
        let viewport = WgpuViewportUniform::new(640.0, 480.0);

        let first = builder.build(&snapshot, viewport);
        let second = builder.build(&snapshot, viewport);

        assert!(Arc::ptr_eq(
            &first.vertex_buffer.vertices,
            &second.vertex_buffer.vertices,
        ));
        assert!(Arc::ptr_eq(
            &first.vertex_buffer.indices,
            &second.vertex_buffer.indices,
        ));
        assert!(Arc::ptr_eq(
            &first.glyph_atlas_frame.atlas,
            &second.glyph_atlas_frame.atlas,
        ));
        assert!(second.glyph_atlas_frame.cache_hit);
        assert_eq!(
            second.timings.render_surface,
            Duration::ZERO,
            "cache hits must not repeat CPU surface rendering",
        );
        assert_eq!(second.timings.vertex_build, Duration::ZERO);
        assert_eq!(second.timings.atlas_build, Duration::ZERO);
    }

    #[test]
    fn changed_cursor_state_invalidates_prepared_frame_cache() {
        let builder = WgpuTerminalFrameBuilder::default();
        let mut snapshot = debug_snapshot(1);
        let viewport = WgpuViewportUniform::new(640.0, 480.0);
        let first = builder.build(&snapshot, viewport);
        snapshot.cursor = Some(RenderSurfaceCursorSnapshot {
            x: 2,
            y: 0,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        });

        let second = builder.build(&snapshot, viewport);

        assert!(!Arc::ptr_eq(
            &first.vertex_buffer.vertices,
            &second.vertex_buffer.vertices,
        ));
    }

    #[test]
    fn crossfont_frame_maps_joined_emoji_and_contextual_script_glyphs() {
        let builder = WgpuTerminalFrameBuilder::default()
            .with_crossfont_glyph_atlas("monospace", 16.0)
            .expect("the platform monospace font should load");
        let snapshot = RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "👩\u{200d}💻 سلام".to_string(),
                    style: TextStyleDto::plain(),
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: Some(RenderSurfaceCursorSnapshot {
                x: 3,
                y: 0,
                focused: true,
                shape: RenderSurfaceCursorShape::Block,
                blinking: false,
            }),
            ime_preedit: None,
        };

        let frame = builder.build(&snapshot, WgpuViewportUniform::new(640.0, 480.0));

        assert_eq!(frame.renderer_lines, ["👩\u{200d}💻 سلام"]);
        assert_eq!(frame.glyph_uv_map_result.missing_vertices, 0);
        assert!(frame.has_mapped_glyph_uvs());
        assert!(frame.glyph_atlas_frame.atlas.non_zero_pixel_count() > 0);
    }
}
