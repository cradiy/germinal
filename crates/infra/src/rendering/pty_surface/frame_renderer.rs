use std::time::{Duration, Instant};

use germinal_domain::{rendering::render_target_id::RenderTargetId, shared::seq::Seq};
use germinal_ports::rendering::surface_snapshot::RenderSurfaceSnapshot;

use crate::rendering::pty_surface::{
	command_encoder_adapter::{WgpuTerminalCommandEncoderAdapter, WgpuTerminalCommandEncoderResult},
	frame_builder::{
		WgpuTerminalFrameBuilder, WgpuTerminalPreparedFrame, WgpuTerminalPreparedFrameTimings,
	},
	frame_upload_plan::{
		WgpuTerminalFrameUploadPlan, WgpuTerminalUploadContext, WgpuTerminalUploadedFrame,
	},
	glyph_atlas_gpu_cache::WgpuTerminalGlyphAtlasGpuCache,
	pipeline_factory::WgpuTerminalPipeline,
	render_target_plan::WgpuTerminalRenderTargetPlan,
	renderer_backend::WgpuRendererConfig,
	shader::WgpuViewportUniform,
};

#[derive(Debug, Clone)]
pub struct WgpuTerminalFrameRenderer {
	frame_builder:           WgpuTerminalFrameBuilder,
	command_encoder_adapter: WgpuTerminalCommandEncoderAdapter,
	glyph_atlas_gpu_cache:   WgpuTerminalGlyphAtlasGpuCache,
}

impl WgpuTerminalFrameRenderer {
	pub fn new(frame_builder: WgpuTerminalFrameBuilder) -> Self {
		Self {
			frame_builder,
			command_encoder_adapter: WgpuTerminalCommandEncoderAdapter::new(),
			glyph_atlas_gpu_cache: WgpuTerminalGlyphAtlasGpuCache::new(),
		}
	}

	pub fn frame_builder(&self) -> &WgpuTerminalFrameBuilder { &self.frame_builder }

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

	pub fn build_upload_plan(
		&self,
		prepared: &WgpuTerminalPreparedFrame,
	) -> WgpuTerminalFrameUploadPlan {
		WgpuTerminalFrameUploadPlan::from_prepared_frame(prepared)
	}

	pub fn upload(
		&self,
		gpu: WgpuTerminalGpuContext<'_>,
		pipeline: &WgpuTerminalPipeline,
		prepared: &WgpuTerminalPreparedFrame,
		upload_plan: &WgpuTerminalFrameUploadPlan,
	) -> WgpuTerminalUploadedFrame {
		upload_plan.upload(WgpuTerminalUploadContext {
			device: gpu.device,
			queue: gpu.queue,
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
		uploaded_frame: &WgpuTerminalUploadedFrame,
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
				target_id:                 view.surface_snapshot.target_id,
				seq:                       view.surface_snapshot.latest_seq,
				prepared:                  false,
				uploaded:                  false,
				encoded:                   false,
				command_count:             0,
				draw_count:                0,
				index_count:               0,
				vertex_count:              0,
				quad_count:                0,
				glyph_count:               0,
				glyph_atlas_uploaded:      false,
				glyph_atlas_cpu_cache_hit: false,
				glyph_atlas_gpu_cache_hit: false,
				timings:                   WgpuTerminalFrameRenderTimings {
					total: total_started_at.elapsed(),
					..Default::default()
				},
			};
		}

		let prepare_started_at = Instant::now();
		let prepared = self.prepare_with_renderer_config(
			view.surface_snapshot,
			view.render_target_plan,
			view.renderer_config,
		);
		let prepare_time = prepare_started_at.elapsed();
		let prepared_frame_timings = prepared.timings;

		let upload_plan_started_at = Instant::now();
		let upload_plan = self.build_upload_plan(&prepared);
		let upload_plan_time = upload_plan_started_at.elapsed();

		if !upload_plan.has_draw_work() {
			return WgpuTerminalFrameRenderResult {
				target_id:                 prepared.target_id,
				seq:                       prepared.seq,
				prepared:                  true,
				uploaded:                  false,
				encoded:                   false,
				command_count:             0,
				draw_count:                0,
				index_count:               0,
				vertex_count:              prepared.vertex_count(),
				quad_count:                prepared.quad_count(),
				glyph_count:               prepared.glyph_count,
				glyph_atlas_uploaded:      false,
				glyph_atlas_cpu_cache_hit: prepared.glyph_atlas_frame.cache_hit,
				glyph_atlas_gpu_cache_hit: false,
				timings:                   WgpuTerminalFrameRenderTimings {
					prepare: prepare_time,
					upload_plan: upload_plan_time,
					prepared_frame: prepared_frame_timings,
					total: total_started_at.elapsed(),
					..Default::default()
				},
			};
		}

		let upload_started_at = Instant::now();
		let uploaded_frame = self.upload(gpu, view.pipeline, &prepared, &upload_plan);
		let upload_time = upload_started_at.elapsed();

		let glyph_atlas_uploaded = uploaded_frame.has_glyph_atlas_bind_group();

		let glyph_atlas_gpu_cache_hit = uploaded_frame.glyph_atlas_gpu_cache_hit;

		let encode_started_at = Instant::now();
		let encode_result = self.encode_uploaded_frame(
			view.command_encoder,
			view.target_view,
			view.render_target_plan,
			view.pipeline,
			&uploaded_frame,
		);
		let encode_time = encode_started_at.elapsed();

		WgpuTerminalFrameRenderResult {
			target_id: prepared.target_id,
			seq: prepared.seq,
			prepared: true,
			uploaded: true,
			encoded: encode_result.encoded_frame,
			command_count: encode_result.command_count,
			draw_count: encode_result.draw_count,
			index_count: encode_result.index_count,
			vertex_count: prepared.vertex_count(),
			quad_count: prepared.quad_count(),
			glyph_count: prepared.glyph_count,
			glyph_atlas_uploaded,
			glyph_atlas_cpu_cache_hit: prepared.glyph_atlas_frame.cache_hit,
			glyph_atlas_gpu_cache_hit,
			timings: WgpuTerminalFrameRenderTimings {
				prepare:        prepare_time,
				upload_plan:    upload_plan_time,
				upload:         upload_time,
				encode:         encode_time,
				prepared_frame: prepared_frame_timings,
				total:          total_started_at.elapsed(),
			},
		}
	}
}

#[derive(Clone, Copy)]
pub struct WgpuTerminalGpuContext<'a> {
	pub device: &'a wgpu::Device,
	pub queue:  &'a wgpu::Queue,
}

pub struct WgpuTerminalRenderView<'a> {
	pub command_encoder:    &'a mut wgpu::CommandEncoder,
	pub target_view:        &'a wgpu::TextureView,
	pub render_target_plan: WgpuTerminalRenderTargetPlan,
	pub pipeline:           &'a WgpuTerminalPipeline,
	pub surface_snapshot:   &'a RenderSurfaceSnapshot,
	pub renderer_config:    WgpuRendererConfig,
}

impl Default for WgpuTerminalFrameRenderer {
	fn default() -> Self { Self::new(WgpuTerminalFrameBuilder::default()) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WgpuTerminalFrameRenderTimings {
	pub prepare:        Duration,
	pub upload_plan:    Duration,
	pub upload:         Duration,
	pub encode:         Duration,
	pub prepared_frame: WgpuTerminalPreparedFrameTimings,
	pub total:          Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalFrameRenderResult {
	pub target_id:                 RenderTargetId,
	pub seq:                       Seq,
	pub prepared:                  bool,
	pub uploaded:                  bool,
	pub encoded:                   bool,
	pub command_count:             usize,
	pub draw_count:                usize,
	pub index_count:               u32,
	pub vertex_count:              usize,
	pub quad_count:                usize,
	pub glyph_count:               usize,
	pub glyph_atlas_uploaded:      bool,
	pub glyph_atlas_cpu_cache_hit: bool,
	pub glyph_atlas_gpu_cache_hit: bool,
	pub timings:                   WgpuTerminalFrameRenderTimings,
}

impl WgpuTerminalFrameRenderResult {
	pub fn rendered(&self) -> bool { self.prepared && self.uploaded && self.encoded }
}
