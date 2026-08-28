use std::time::{Duration, Instant};

use germinal_ports::{
    rendering::{render_target_id::RenderTargetId, surface_snapshot::RenderSurfaceSnapshot},
    seq::Seq,
};

#[cfg(target_os = "linux")]
use crate::rendering::pty_surface::video_surface_dmabuf_importer::import_nv12_dmabuf_frame;
use crate::rendering::pty_surface::{
    buffer_uploader::WgpuBufferUploader,
    command_encoder_adapter::{
        WgpuTerminalCommandEncoderAdapter, WgpuTerminalCommandEncoderResult, WgpuTerminalTextLayer,
    },
    frame_builder::{
        WgpuTerminalFrameBuilder, WgpuTerminalPreparedFrame, WgpuTerminalPreparedFrameTimings,
    },
    frame_upload_plan::{
        WgpuTerminalFrameUploadPlan, WgpuTerminalUploadContext, WgpuTerminalUploadedFrame,
    },
    glyph_atlas_gpu_cache::WgpuTerminalGlyphAtlasGpuCache,
    image_surface_renderer::{WgpuImageEncodeContext, WgpuImageLayer, WgpuImageSurfaceRenderer},
    pipeline_factory::WgpuTerminalPipeline,
    render_target_plan::WgpuTerminalRenderTargetPlan,
    renderer_backend::WgpuRendererConfig,
    shader::WgpuViewportUniform,
    video_surface_frame::WgpuVideoSurfaceFrame,
    video_surface_registry::WgpuVideoSurfaceRegistry,
    video_surface_renderer::WgpuVideoSurfaceRenderer,
};

#[derive(Debug, Clone)]
pub struct WgpuTerminalFrameRenderer {
    frame_builder: WgpuTerminalFrameBuilder,
    buffer_uploader: WgpuBufferUploader,
    command_encoder_adapter: WgpuTerminalCommandEncoderAdapter,
    glyph_atlas_gpu_cache: WgpuTerminalGlyphAtlasGpuCache,
    video_surface_renderer: WgpuVideoSurfaceRenderer,
    image_surface_renderer: WgpuImageSurfaceRenderer,
}

impl WgpuTerminalFrameRenderer {
    pub fn new(frame_builder: WgpuTerminalFrameBuilder) -> Self {
        Self {
            frame_builder,
            buffer_uploader: WgpuBufferUploader::new(),
            command_encoder_adapter: WgpuTerminalCommandEncoderAdapter::new(),
            glyph_atlas_gpu_cache: WgpuTerminalGlyphAtlasGpuCache::new(),
            video_surface_renderer: WgpuVideoSurfaceRenderer::new(),
            image_surface_renderer: WgpuImageSurfaceRenderer::new(),
        }
    }

    pub fn frame_builder(&self) -> &WgpuTerminalFrameBuilder {
        &self.frame_builder
    }

    pub fn video_surface_registry(&self) -> &WgpuVideoSurfaceRegistry {
        self.frame_builder.video_surface_registry()
    }

    pub fn replace_frame_builder(&mut self, frame_builder: WgpuTerminalFrameBuilder) {
        let video_surface_registry = self.frame_builder.video_surface_registry().clone();
        self.frame_builder = frame_builder.with_video_surface_registry(video_surface_registry);
        self.glyph_atlas_gpu_cache = WgpuTerminalGlyphAtlasGpuCache::new();
    }

    pub fn release_render_target_cache(&self, target_id: RenderTargetId) {
        self.frame_builder.release_render_target_cache(target_id);
        self.buffer_uploader.remove_render_target(target_id);
        self.glyph_atlas_gpu_cache.remove_render_target(target_id);
        self.image_surface_renderer.remove_render_target(target_id);
    }

    pub fn remove_render_target(&self, target_id: RenderTargetId) {
        self.release_render_target_cache(target_id);
        self.frame_builder
            .video_surface_registry()
            .remove_render_target(target_id);
    }

    pub fn prepare(
        &self,
        surface_snapshot: &RenderSurfaceSnapshot,
        render_target_plan: WgpuTerminalRenderTargetPlan,
    ) -> WgpuTerminalPreparedFrame {
        self.prepare_with_renderer_config(
            surface_snapshot,
            render_target_plan,
            self.frame_builder.renderer_config(),
        )
    }

    pub fn prepare_with_renderer_config(
        &self,
        surface_snapshot: &RenderSurfaceSnapshot,
        render_target_plan: WgpuTerminalRenderTargetPlan,
        renderer_config: WgpuRendererConfig,
    ) -> WgpuTerminalPreparedFrame {
        self.frame_builder.build_with_renderer_config(
            surface_snapshot,
            WgpuViewportUniform::new(
                render_target_plan.viewport_width_px(),
                render_target_plan.viewport_height_px(),
            ),
            renderer_config,
        )
    }

    pub fn build_upload_plan<'a>(
        &self,
        prepared: &'a WgpuTerminalPreparedFrame,
    ) -> WgpuTerminalFrameUploadPlan<'a> {
        WgpuTerminalFrameUploadPlan::from_prepared_frame(prepared)
    }

    pub fn upload<'a>(
        &self,
        gpu: WgpuTerminalGpuContext<'_>,
        pipeline: &WgpuTerminalPipeline,
        prepared: &'a WgpuTerminalPreparedFrame,
        upload_plan: &WgpuTerminalFrameUploadPlan<'a>,
    ) -> WgpuTerminalUploadedFrame<'a> {
        upload_plan.upload(WgpuTerminalUploadContext {
            device: gpu.device,
            queue: gpu.queue,
            buffer_uploader: &self.buffer_uploader,
            viewport_bind_group_layout: &pipeline.viewport_bind_group_layout,
            viewport_binding: pipeline.spec.shader.viewport_binding,
            glyph_atlas_bind_group_layout: &pipeline.glyph_atlas_bind_group_layout,
            prepared,
            glyph_atlas_gpu_cache: Some(&self.glyph_atlas_gpu_cache),
        })
    }

    pub fn encode_uploaded_frame(
        &self,
        command_encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        render_target_plan: WgpuTerminalRenderTargetPlan,
        pipeline: &WgpuTerminalPipeline,
        uploaded_frame: &WgpuTerminalUploadedFrame<'_>,
    ) -> WgpuTerminalCommandEncoderResult {
        self.command_encoder_adapter.encode_frame(
            command_encoder,
            target_view,
            render_target_plan,
            pipeline,
            uploaded_frame,
        )
    }

    pub fn render_to_view(
        &self,
        gpu: WgpuTerminalGpuContext<'_>,
        view: WgpuTerminalRenderView<'_>,
    ) -> WgpuTerminalFrameRenderResult {
        let total_started_at = Instant::now();

        if view.render_target_plan.is_empty() {
            return WgpuTerminalFrameRenderResult {
                target_id: view.surface_snapshot.target_id,
                seq: view.surface_snapshot.latest_seq,
                prepared: false,
                uploaded: false,
                encoded: false,
                command_count: 0,
                draw_count: 0,
                index_count: 0,
                vertex_count: 0,
                quad_count: 0,
                glyph_count: 0,
                glyph_atlas_uploaded: false,
                glyph_atlas_cpu_cache_hit: false,
                glyph_atlas_gpu_cache_hit: false,
                video_surface_count: 0,
                video_draw_count: 0,
                image_surface_count: 0,
                image_draw_count: 0,
                timings: WgpuTerminalFrameRenderTimings {
                    total: total_started_at.elapsed(),
                    ..Default::default()
                },
            };
        }

        let prepare_started_at = Instant::now();
        resolve_dma_buf_video_frames(
            view.surface_snapshot,
            gpu.device,
            self.video_surface_registry(),
        );
        let prepared = self.prepare_with_renderer_config(
            view.surface_snapshot,
            view.render_target_plan,
            view.renderer_config,
        );
        let prepare_time = prepare_started_at.elapsed();
        let prepared_frame_timings = prepared.timings;
        let prepared_video_frame = self.video_surface_renderer.prepare(
            view.surface_snapshot,
            view.render_target_plan,
            view.renderer_config,
            self.video_surface_registry(),
        );
        let has_video_draw_work = !prepared_video_frame.is_empty();
        let prepared_image_frame = self.image_surface_renderer.prepare(
            gpu.device,
            gpu.queue,
            view.surface_snapshot,
            view.render_target_plan,
            view.renderer_config,
        );
        let has_image_draw_work = !prepared_image_frame.is_empty();

        let upload_plan_started_at = Instant::now();
        let upload_plan = self.build_upload_plan(&prepared);
        let upload_plan_time = upload_plan_started_at.elapsed();

        if !upload_plan.has_draw_work() && !has_video_draw_work && !has_image_draw_work {
            return WgpuTerminalFrameRenderResult {
                target_id: prepared.target_id,
                seq: prepared.seq,
                prepared: true,
                uploaded: false,
                encoded: false,
                command_count: 0,
                draw_count: 0,
                index_count: 0,
                vertex_count: prepared.vertex_count(),
                quad_count: prepared.quad_count(),
                glyph_count: prepared.glyph_count,
                glyph_atlas_uploaded: false,
                glyph_atlas_cpu_cache_hit: prepared.glyph_atlas_frame.cache_hit,
                glyph_atlas_gpu_cache_hit: false,
                video_surface_count: 0,
                video_draw_count: 0,
                image_surface_count: 0,
                image_draw_count: 0,
                timings: WgpuTerminalFrameRenderTimings {
                    prepare: prepare_time,
                    upload_plan: upload_plan_time,
                    prepared_frame: prepared_frame_timings,
                    total: total_started_at.elapsed(),
                    ..Default::default()
                },
            };
        }

        let (uploaded_frame, upload_time, glyph_atlas_uploaded, glyph_atlas_gpu_cache_hit) =
            if upload_plan.has_draw_work() {
                let upload_started_at = Instant::now();
                let uploaded_frame = self.upload(gpu, view.pipeline, &prepared, &upload_plan);
                let upload_time = upload_started_at.elapsed();
                let glyph_atlas_uploaded = uploaded_frame.has_glyph_atlas_bind_group();
                let glyph_atlas_gpu_cache_hit = uploaded_frame.glyph_atlas_gpu_cache_hit;
                (
                    Some(uploaded_frame),
                    upload_time,
                    glyph_atlas_uploaded,
                    glyph_atlas_gpu_cache_hit,
                )
            } else {
                (None, Duration::ZERO, false, false)
            };

        let encode_started_at = Instant::now();
        let split_text_layers = prepared_image_frame.has_below_cell_background_images()
            || prepared_image_frame.has_below_text_images();
        let first_text_result = if let Some(uploaded_frame) = uploaded_frame.as_ref() {
            self.command_encoder_adapter.encode_frame_layer(
                view.command_encoder,
                view.target_view,
                view.render_target_plan,
                view.pipeline,
                uploaded_frame,
                if split_text_layers {
                    WgpuTerminalTextLayer::SurfaceBackground
                } else {
                    WgpuTerminalTextLayer::All
                },
            )
        } else {
            WgpuTerminalCommandEncoderResult::empty(prepared.target_id, prepared.seq)
        };
        let below_background_load_op = if first_text_result.encoded_frame {
            wgpu::LoadOp::Load
        } else {
            view.render_target_plan.wgpu_load_op()
        };
        let below_background_image_result = self.image_surface_renderer.encode_layer(
            WgpuImageEncodeContext {
                device: gpu.device,
                encoder: view.command_encoder,
                target_view: view.target_view,
                color_format: view.pipeline.spec.color_format,
                plan: view.render_target_plan,
                load_op: below_background_load_op,
            },
            &prepared_image_frame,
            WgpuImageLayer::BelowCellBackground,
        );
        let cell_background_result = if split_text_layers {
            if let Some(uploaded_frame) = uploaded_frame.as_ref() {
                self.command_encoder_adapter.encode_frame_layer(
                    view.command_encoder,
                    view.target_view,
                    view.render_target_plan.with_load_op(
                        crate::rendering::pty_surface::render_target_plan::WgpuTerminalLoadOp::Load,
                    ),
                    view.pipeline,
                    uploaded_frame,
                    WgpuTerminalTextLayer::CellBackground,
                )
            } else {
                WgpuTerminalCommandEncoderResult::empty(prepared.target_id, prepared.seq)
            }
        } else {
            WgpuTerminalCommandEncoderResult::empty(prepared.target_id, prepared.seq)
        };
        let below_text_load_op = if first_text_result.encoded_frame
            || below_background_image_result.encoded
            || cell_background_result.encoded_frame
        {
            wgpu::LoadOp::Load
        } else {
            view.render_target_plan.wgpu_load_op()
        };
        let below_text_image_result = self.image_surface_renderer.encode_layer(
            WgpuImageEncodeContext {
                device: gpu.device,
                encoder: view.command_encoder,
                target_view: view.target_view,
                color_format: view.pipeline.spec.color_format,
                plan: view.render_target_plan,
                load_op: below_text_load_op,
            },
            &prepared_image_frame,
            WgpuImageLayer::BelowText,
        );
        let foreground_text_render_target_plan = view.render_target_plan.with_load_op(
            crate::rendering::pty_surface::render_target_plan::WgpuTerminalLoadOp::Load,
        );
        let foreground_text_result = if split_text_layers {
            if let Some(uploaded_frame) = uploaded_frame.as_ref() {
                self.command_encoder_adapter.encode_frame_layer(
                    view.command_encoder,
                    view.target_view,
                    foreground_text_render_target_plan,
                    view.pipeline,
                    uploaded_frame,
                    WgpuTerminalTextLayer::Foreground,
                )
            } else {
                WgpuTerminalCommandEncoderResult::empty(prepared.target_id, prepared.seq)
            }
        } else {
            WgpuTerminalCommandEncoderResult::empty(prepared.target_id, prepared.seq)
        };
        let above_load_op = if below_background_image_result.encoded
            || first_text_result.encoded_frame
            || cell_background_result.encoded_frame
            || below_text_image_result.encoded
            || foreground_text_result.encoded_frame
        {
            wgpu::LoadOp::Load
        } else {
            view.render_target_plan.wgpu_load_op()
        };
        let above_image_result = self.image_surface_renderer.encode_layer(
            WgpuImageEncodeContext {
                device: gpu.device,
                encoder: view.command_encoder,
                target_view: view.target_view,
                color_format: view.pipeline.spec.color_format,
                plan: view.render_target_plan,
                load_op: above_load_op,
            },
            &prepared_image_frame,
            WgpuImageLayer::AboveText,
        );
        let video_result = self.video_surface_renderer.encode_prepared_frame(
            gpu.device,
            view.command_encoder,
            view.target_view,
            view.pipeline.spec.color_format,
            view.render_target_plan,
            &prepared_video_frame,
        );
        let encode_time = encode_started_at.elapsed();

        WgpuTerminalFrameRenderResult {
            target_id: prepared.target_id,
            seq: prepared.seq,
            prepared: true,
            uploaded: upload_plan.has_draw_work(),
            encoded: first_text_result.encoded_frame
                || cell_background_result.encoded_frame
                || foreground_text_result.encoded_frame
                || below_background_image_result.encoded
                || below_text_image_result.encoded
                || above_image_result.encoded
                || video_result.encoded(),
            command_count: first_text_result.command_count
                + cell_background_result.command_count
                + foreground_text_result.command_count
                + usize::from(below_background_image_result.encoded)
                + usize::from(below_text_image_result.encoded)
                + usize::from(above_image_result.encoded)
                + usize::from(video_result.encoded()),
            draw_count: first_text_result.draw_count
                + cell_background_result.draw_count
                + foreground_text_result.draw_count
                + below_background_image_result.draw_count
                + below_text_image_result.draw_count
                + above_image_result.draw_count
                + video_result.draw_count,
            index_count: first_text_result.index_count
                + cell_background_result.index_count
                + foreground_text_result.index_count,
            vertex_count: prepared.vertex_count(),
            quad_count: prepared.quad_count(),
            glyph_count: prepared.glyph_count,
            glyph_atlas_uploaded,
            glyph_atlas_cpu_cache_hit: prepared.glyph_atlas_frame.cache_hit,
            glyph_atlas_gpu_cache_hit,
            video_surface_count: video_result.surface_count,
            video_draw_count: video_result.draw_count,
            image_surface_count: below_background_image_result.surface_count
                + below_text_image_result.surface_count
                + above_image_result.surface_count,
            image_draw_count: below_background_image_result.draw_count
                + below_text_image_result.draw_count
                + above_image_result.draw_count,
            timings: WgpuTerminalFrameRenderTimings {
                prepare: prepare_time,
                upload_plan: upload_plan_time,
                upload: upload_time,
                encode: encode_time,
                prepared_frame: prepared_frame_timings,
                total: total_started_at.elapsed(),
            },
        }
    }
}

#[cfg(target_os = "linux")]
fn resolve_dma_buf_video_frames(
    surface_snapshot: &RenderSurfaceSnapshot,
    device: &wgpu::Device,
    registry: &WgpuVideoSurfaceRegistry,
) {
    for surface in &surface_snapshot.video_surfaces {
        let Some(frame) = registry.attached_frame(surface_snapshot.target_id, &surface.id) else {
            continue;
        };
        let WgpuVideoSurfaceFrame::Nv12DmaBuf(frame) = frame else {
            continue;
        };
        let Ok(imported) = import_nv12_dmabuf_frame(device, &frame) else {
            continue;
        };
        registry.replace_nv12_frame(surface_snapshot.target_id, &surface.id, imported);
    }
}

#[cfg(not(target_os = "linux"))]
fn resolve_dma_buf_video_frames(
    _surface_snapshot: &RenderSurfaceSnapshot,
    _device: &wgpu::Device,
    _registry: &WgpuVideoSurfaceRegistry,
) {
}

#[derive(Clone, Copy)]
pub struct WgpuTerminalGpuContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
}

pub struct WgpuTerminalRenderView<'a> {
    pub command_encoder: &'a mut wgpu::CommandEncoder,
    pub target_view: &'a wgpu::TextureView,
    pub render_target_plan: WgpuTerminalRenderTargetPlan,
    pub pipeline: &'a WgpuTerminalPipeline,
    pub surface_snapshot: &'a RenderSurfaceSnapshot,
    pub renderer_config: WgpuRendererConfig,
}

impl Default for WgpuTerminalFrameRenderer {
    fn default() -> Self {
        Self::new(WgpuTerminalFrameBuilder::default())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WgpuTerminalFrameRenderTimings {
    pub prepare: Duration,
    pub upload_plan: Duration,
    pub upload: Duration,
    pub encode: Duration,
    pub prepared_frame: WgpuTerminalPreparedFrameTimings,
    pub total: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalFrameRenderResult {
    pub target_id: RenderTargetId,
    pub seq: Seq,
    pub prepared: bool,
    pub uploaded: bool,
    pub encoded: bool,
    pub command_count: usize,
    pub draw_count: usize,
    pub index_count: u32,
    pub vertex_count: usize,
    pub quad_count: usize,
    pub glyph_count: usize,
    pub glyph_atlas_uploaded: bool,
    pub glyph_atlas_cpu_cache_hit: bool,
    pub glyph_atlas_gpu_cache_hit: bool,
    pub video_surface_count: usize,
    pub video_draw_count: usize,
    pub image_surface_count: usize,
    pub image_draw_count: usize,
    pub timings: WgpuTerminalFrameRenderTimings,
}

impl WgpuTerminalFrameRenderResult {
    pub fn rendered(&self) -> bool {
        self.prepared && self.encoded
    }
}
