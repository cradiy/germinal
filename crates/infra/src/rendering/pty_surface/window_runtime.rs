use std::{
	env,
	sync::Arc,
	time::{Duration, Instant},
};

use germinal_ports::{
	pty_host::{
		cell_size::TerminalCellSize, font_weight::TerminalFontWeight, profile::TerminalProfile,
		scale_factor::TerminalScaleFactor, size_info::TerminalSizeInfo,
		window_metrics::TerminalWindowMetrics, window_size::TerminalWindowSize,
	},
	rendering::{surface_snapshot::RenderSurfaceSnapshot, window_runtime::ITerminalWindowRuntime},
	seq::Seq,
};
use thiserror::Error;
use tracing::info;
use winit::window::{Window, WindowId};

#[cfg(target_os = "linux")]
use crate::rendering::pty_surface::video_surface_dmabuf_importer::{
	VideoSurfaceImportError, import_nv12_dmabuf_frame,
};
use crate::rendering::pty_surface::{
	crossfont_glyph_atlas::{
		WgpuCrossfontGlyphAtlasBuilder, WgpuCrossfontGlyphAtlasError, WgpuTerminalFontWeight,
	},
	frame_builder::WgpuTerminalFrameBuilder,
	frame_renderer::WgpuTerminalFrameRenderer,
	pipeline_factory::{WgpuTerminalPipeline, WgpuTerminalPipelineFactory},
	pipeline_spec::WgpuTerminalPipelineSpec,
	render_target_plan::WgpuTerminalRenderTargetPlan,
	renderer_backend::WgpuRendererConfig,
	surface_frame_presenter::{
		WgpuTerminalSurfaceFramePresentError, WgpuTerminalSurfaceFramePresenter,
		WgpuTerminalSurfacePresentInput,
	},
	video_surface_frame::WgpuVideoSurfaceNv12DmaBufFrame,
	video_surface_registry::WgpuVideoSurfaceRegistry,
};

#[derive(Debug, Error)]
pub enum WindowRuntimeError {
	#[error("failed to create GPU surface for the terminal window: {source}")]
	CreateSurface {
		#[source]
		source: wgpu::CreateSurfaceError,
	},
	#[error("failed to request a GPU adapter for the terminal window: {source}")]
	RequestAdapter {
		#[source]
		source: wgpu::RequestAdapterError,
	},
	#[error("failed to request a GPU device for the terminal window: {source}")]
	RequestDevice {
		#[source]
		source: wgpu::RequestDeviceError,
	},
	#[error("failed to get default surface config for terminal window size {width_px}x{height_px}")]
	MissingSurfaceConfig { width_px: u32, height_px: u32 },
	#[error("failed to build crossfont glyph atlas: {0}")]
	BuildGlyphAtlas(#[source] WgpuCrossfontGlyphAtlasError),
	#[error("failed to load crossfont metrics: {0}")]
	LoadCrossfontMetrics(#[source] WgpuCrossfontGlyphAtlasError),
	#[cfg(target_os = "linux")]
	#[error("failed to import an NV12 dma_buf video frame into the terminal renderer: {source}")]
	ImportVideoSurfaceFrame {
		#[source]
		source: VideoSurfaceImportError,
	},
}

pub struct WgpuTerminalWindowRuntime {
	window:         Arc<Window>,
	surface:        wgpu::Surface<'static>,
	device:         wgpu::Device,
	queue:          wgpu::Queue,
	surface_config: wgpu::SurfaceConfiguration,
	pipeline:       WgpuTerminalPipeline,
	presenter:      WgpuTerminalSurfaceFramePresenter,

	surface_snapshot: RenderSurfaceSnapshot,
	size_info:        TerminalSizeInfo,
	profile:          TerminalProfile,
	needs_redraw:     bool,
	perf:             WgpuTerminalRenderPerf,
}

#[derive(Debug, Clone, Copy)]
pub struct WgpuTerminalWindowRuntimeFactory {
	profile: TerminalProfile,
}

impl WgpuTerminalWindowRuntimeFactory {
	pub fn new(profile: TerminalProfile) -> Self { Self { profile } }

	pub fn create_window_runtime(
		&self,
		window: Arc<Window>,
	) -> Result<WgpuTerminalWindowRuntime, WindowRuntimeError> {
		pollster::block_on(WgpuTerminalWindowRuntime::new(window, self.profile))
	}
}

impl WgpuTerminalWindowRuntime {
	pub async fn new(
		window: Arc<Window>,
		profile: TerminalProfile,
	) -> Result<Self, WindowRuntimeError> {
		let instance = wgpu::Instance::default();

		let surface = instance
			.create_surface(Arc::clone(&window))
			.map_err(|source| WindowRuntimeError::CreateSurface { source })?;

		let size = window.inner_size();
		let width = size.width.max(1);
		let height = size.height.max(1);

		let adapter = instance
			.request_adapter(&wgpu::RequestAdapterOptions {
				power_preference:       wgpu::PowerPreference::HighPerformance,
				compatible_surface:     Some(&surface),
				force_fallback_adapter: false,
				apply_limit_buckets:    false,
			})
			.await
			.map_err(|source| WindowRuntimeError::RequestAdapter { source })?;

		let (device, queue) = adapter
			.request_device(&wgpu::DeviceDescriptor {
				label: Some("germinal.terminal.device"),
				..Default::default()
			})
			.await
			.map_err(|source| WindowRuntimeError::RequestDevice { source })?;

		let surface_config = surface
			.get_default_config(&adapter, width, height)
			.ok_or(WindowRuntimeError::MissingSurfaceConfig { width_px: width, height_px: height })?;

		surface.configure(&device, &surface_config);

		let pipeline_spec = WgpuTerminalPipelineSpec::new(surface_config.format);
		let pipeline_factory = WgpuTerminalPipelineFactory::new(pipeline_spec);
		let pipeline = pipeline_factory.create(&device);

		let profile =
			terminal_profile_from_alacritty_crossfont_metrics(profile, window.scale_factor())?;
		let size_info = terminal_size_info(profile, width, height, window.scale_factor());

		let frame_builder = build_terminal_frame_builder(
			profile,
			size_info,
			window.scale_factor(),
			device.limits().max_texture_dimension_2d,
		)?;
		let frame_renderer = WgpuTerminalFrameRenderer::new(frame_builder);
		let presenter = WgpuTerminalSurfaceFramePresenter::new(frame_renderer);

		let surface_snapshot = RenderSurfaceSnapshot {
			target_id:      germinal_ports::rendering::render_target_id::RenderTargetId::new(0),
			latest_seq:     Seq::ZERO,
			rows:           Vec::new(),
			video_surfaces: Vec::new(),
			dirty_rows:     Vec::new(),
			cursor:         None,
		};

		Ok(Self {
			window,
			surface,
			device,
			queue,
			surface_config,
			pipeline,
			presenter,
			surface_snapshot,
			size_info,
			profile,
			needs_redraw: false,
			perf: WgpuTerminalRenderPerf::new(),
		})
	}

	pub fn window_id(&self) -> WindowId { self.window.id() }

	pub fn window_size(&self) -> winit::dpi::PhysicalSize<u32> { self.window.inner_size() }

	pub fn render_target_id(&self) -> germinal_ports::rendering::render_target_id::RenderTargetId {
		self.surface_snapshot.target_id
	}

	pub fn video_surface_registry(&self) -> &WgpuVideoSurfaceRegistry {
		self.presenter.frame_renderer().video_surface_registry()
	}

	#[cfg(target_os = "linux")]
	pub fn import_video_surface_dma_buf_frame(
		&self,
		id: &str,
		frame: &WgpuVideoSurfaceNv12DmaBufFrame,
	) -> Result<bool, WindowRuntimeError> {
		if self.video_surface_registry().registration(self.render_target_id(), id).is_none() {
			return Ok(false);
		}

		let imported = import_nv12_dmabuf_frame(&self.device, frame)
			.map_err(|source| WindowRuntimeError::ImportVideoSurfaceFrame { source })?;
		let replaced =
			self.video_surface_registry().replace_nv12_frame(self.render_target_id(), id, imported);
		if !replaced {
			return Ok(false);
		}
		self.request_window_redraw();
		Ok(true)
	}

	pub fn request_window_redraw(&self) { self.window.request_redraw(); }

	pub fn set_surface_snapshot(&mut self, snapshot: RenderSurfaceSnapshot) {
		self.surface_snapshot = snapshot;
		self.request_redraw();
	}

	pub fn surface_snapshot_mut(&mut self) -> &mut RenderSurfaceSnapshot {
		&mut self.surface_snapshot
	}

	pub fn resize_surface_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
		if window_size.is_empty() {
			return self.terminal_size_info();
		}

		self.size_info = self.size_info_for_window_size(window_size);
		self.surface_config.width = self.size_info.window_size().width_px();
		self.surface_config.height = self.size_info.window_size().height_px();
		self.surface.configure(&self.device, &self.surface_config);

		self.request_redraw();

		self.terminal_size_info()
	}

	fn request_redraw(&mut self) { self.needs_redraw = true; }

	pub fn take_redraw_request(&mut self) -> bool {
		let needs_redraw = self.needs_redraw;
		self.needs_redraw = false;
		needs_redraw
	}

	pub fn terminal_size_info(&self) -> TerminalSizeInfo {
		self.size_info.debug_assert_consistent();
		self.size_info
	}

	fn size_info_for_window_size(&self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
		self.profile.size_info_for_window_metrics(TerminalWindowMetrics::new(
			window_size,
			TerminalScaleFactor::new(self.window.scale_factor()),
		))
	}

	pub fn render(&mut self) {
		let size_info = self.terminal_size_info();
		let render_target_plan = WgpuTerminalRenderTargetPlan::from_size_info(size_info);
		let renderer_config = WgpuRendererConfig::from(size_info);
		let row_count = self.surface_snapshot.rows.len() as u64;
		let run_count = self.surface_snapshot.rows.iter().map(|row| row.runs.len() as u64).sum();

		match self.presenter.present_surface_frame(WgpuTerminalSurfacePresentInput {
			surface: &self.surface,
			device: &self.device,
			queue: &self.queue,
			render_target_plan,
			pipeline: &self.pipeline,
			surface_snapshot: &self.surface_snapshot,
			renderer_config,
		}) {
			Ok(result) => {
				self.perf.record_frame(row_count, run_count, &result);
			}
			Err(error) => {
				self.perf.record_error(error);
				self.handle_present_error(error);
			}
		}
	}

	fn handle_present_error(&mut self, error: WgpuTerminalSurfaceFramePresentError) {
		match error {
			WgpuTerminalSurfaceFramePresentError::Outdated
			| WgpuTerminalSurfaceFramePresentError::Lost => {
				self.surface.configure(&self.device, &self.surface_config);
				self.request_redraw();
			}
			WgpuTerminalSurfaceFramePresentError::Timeout
			| WgpuTerminalSurfaceFramePresentError::Occluded
			| WgpuTerminalSurfaceFramePresentError::Validation => {}
		}
	}
}

fn build_terminal_frame_builder(
	profile: TerminalProfile,
	size_info: TerminalSizeInfo,
	scale_factor: f64,
	max_texture_dimension_2d: u32,
) -> Result<WgpuTerminalFrameBuilder, WindowRuntimeError> {
	let base = WgpuTerminalFrameBuilder::new(WgpuRendererConfig::from(size_info));

	let glyph_config = profile.glyph_render_config(size_info, TerminalScaleFactor::new(scale_factor));
	let terminal_cell_size = glyph_config.cell_size();
	let crossfont_builder = WgpuCrossfontGlyphAtlasBuilder::new(
		glyph_config.font_family_name(),
		glyph_config.font_size_px(),
	)
	.map_err(WindowRuntimeError::BuildGlyphAtlas)?
	.with_bold_font_weight(wgpu_font_weight_from_terminal(glyph_config.bold_font_weight()))
	.with_padding_px(2)
	.with_columns(16)
	.with_max_texture_dimension_2d(max_texture_dimension_2d)
	.with_cell_size_px(terminal_cell_size.width_px(), terminal_cell_size.height_px());

	Ok(base.with_crossfont_glyph_atlas_builder(crossfont_builder))
}

fn wgpu_font_weight_from_terminal(weight: TerminalFontWeight) -> WgpuTerminalFontWeight {
	match weight {
		TerminalFontWeight::Normal => WgpuTerminalFontWeight::Normal,
		TerminalFontWeight::Medium => WgpuTerminalFontWeight::Medium,
		TerminalFontWeight::Semibold => WgpuTerminalFontWeight::Semibold,
		TerminalFontWeight::Bold => WgpuTerminalFontWeight::Bold,
	}
}

fn terminal_profile_from_alacritty_crossfont_metrics(
	profile: TerminalProfile,
	scale_factor: f64,
) -> Result<TerminalProfile, WindowRuntimeError> {
	let scale_factor = TerminalScaleFactor::new(scale_factor);
	let font_px = profile.font_physical_px(scale_factor);
	let font_family = profile.font_family().name();
	let metrics = WgpuCrossfontGlyphAtlasBuilder::load_cell_metrics(font_family, font_px)
		.map_err(WindowRuntimeError::LoadCrossfontMetrics)?;

	Ok(
		profile
			.with_cell_size(TerminalCellSize::new(metrics.cell_width_px(), metrics.cell_height_px())),
	)
}

fn terminal_size_info(
	profile: TerminalProfile,
	width: u32,
	height: u32,
	scale_factor: f64,
) -> TerminalSizeInfo {
	profile.size_info_for_window_metrics(TerminalWindowMetrics::from_physical_size(
		width,
		height,
		scale_factor,
	))
}

const RENDER_PERF_LOG_INTERVAL: Duration = Duration::from_secs(1);
const RENDER_PERF_LOG_ENV: &str = "GERMINAL_RENDER_PERF_LOG";

struct WgpuTerminalRenderPerf {
	logging_enabled:            bool,
	started_at:                 Instant,
	last_log_at:                Instant,
	frame_count:                u64,
	error_count:                u64,
	row_count:                  u64,
	run_count:                  u64,
	quad_count:                 u64,
	vertex_count:               u64,
	glyph_count:                u64,
	prepare_time:               Duration,
	prepare_render_surface:     Duration,
	prepare_quads_clone:        Duration,
	prepare_vertex_build:       Duration,
	prepare_atlas_build:        Duration,
	prepare_uv_map:             Duration,
	prepare_upload_bytes:       Duration,
	upload_time:                Duration,
	encode_time:                Duration,
	render_total:               Duration,
	present_total:              Duration,
	publish_total:              Duration,
	prepare_max:                Duration,
	upload_max:                 Duration,
	render_max:                 Duration,
	present_max:                Duration,
	glyph_atlas_cpu_cache_hits: u64,
	glyph_atlas_gpu_cache_hits: u64,
}

impl WgpuTerminalRenderPerf {
	fn new() -> Self {
		let now = Instant::now();

		Self {
			logging_enabled:            render_perf_logging_enabled(),
			started_at:                 now,
			last_log_at:                now,
			frame_count:                0,
			error_count:                0,
			row_count:                  0,
			run_count:                  0,
			quad_count:                 0,
			vertex_count:               0,
			glyph_count:                0,
			prepare_time:               Duration::ZERO,
			prepare_render_surface:     Duration::ZERO,
			prepare_quads_clone:        Duration::ZERO,
			prepare_vertex_build:       Duration::ZERO,
			prepare_atlas_build:        Duration::ZERO,
			prepare_uv_map:             Duration::ZERO,
			prepare_upload_bytes:       Duration::ZERO,
			upload_time:                Duration::ZERO,
			encode_time:                Duration::ZERO,
			render_total:               Duration::ZERO,
			present_total:              Duration::ZERO,
			publish_total:              Duration::ZERO,
			prepare_max:                Duration::ZERO,
			upload_max:                 Duration::ZERO,
			render_max:                 Duration::ZERO,
			present_max:                Duration::ZERO,
			glyph_atlas_cpu_cache_hits: 0,
			glyph_atlas_gpu_cache_hits: 0,
		}
	}

	fn record_frame(
		&mut self,
		row_count: u64,
		run_count: u64,
		result: &crate::rendering::pty_surface::surface_frame_presenter::WgpuTerminalSurfaceFramePresentResult,
	) {
		if !self.logging_enabled {
			return;
		}

		self.frame_count += 1;
		self.row_count += row_count;
		self.run_count += run_count;
		self.quad_count += result.render_result.quad_count as u64;
		self.vertex_count += result.render_result.vertex_count as u64;
		self.glyph_count += result.render_result.glyph_count as u64;
		self.prepare_time += result.render_result.timings.prepare;
		self.prepare_render_surface += result.render_result.timings.prepared_frame.render_surface;
		self.prepare_quads_clone += result.render_result.timings.prepared_frame.quads_clone;
		self.prepare_vertex_build += result.render_result.timings.prepared_frame.vertex_build;
		self.prepare_atlas_build += result.render_result.timings.prepared_frame.atlas_build;
		self.prepare_uv_map += result.render_result.timings.prepared_frame.uv_map;
		self.prepare_upload_bytes += result.render_result.timings.prepared_frame.upload_bytes;
		self.upload_time += result.render_result.timings.upload;
		self.encode_time += result.render_result.timings.encode;
		self.render_total += result.render_result.timings.total;
		self.present_total += result.timings.render_to_view;
		self.publish_total += result.timings.total;
		self.prepare_max = self.prepare_max.max(result.render_result.timings.prepare);
		self.upload_max = self.upload_max.max(result.render_result.timings.upload);
		self.render_max = self.render_max.max(result.render_result.timings.total);
		self.present_max = self.present_max.max(result.timings.total);

		if result.render_result.glyph_atlas_cpu_cache_hit {
			self.glyph_atlas_cpu_cache_hits += 1;
		}

		if result.render_result.glyph_atlas_gpu_cache_hit {
			self.glyph_atlas_gpu_cache_hits += 1;
		}

		self.maybe_log();
	}

	fn record_error(&mut self, _error: WgpuTerminalSurfaceFramePresentError) {
		if !self.logging_enabled {
			return;
		}

		self.error_count += 1;
		self.maybe_log();
	}

	fn maybe_log(&mut self) {
		if self.last_log_at.elapsed() < RENDER_PERF_LOG_INTERVAL {
			return;
		}

		self.log_and_reset();
	}

	fn log_and_reset(&mut self) {
		if self.frame_count == 0 && self.error_count == 0 {
			self.last_log_at = Instant::now();
			return;
		}

		info!(
			"[render] frames={} errors={} rows/frame={} runs/frame={} quads/frame={} glyphs/frame={} \
			 prepare avg={} max={} prep_parts(surface/quads/vb/atlas/uv/bytes)={}/{}/{}/{}/{}/{} upload \
			 avg={} max={} render avg={} max={} surface avg={} max={} frame_total avg={} \
			 cpu_atlas_hit={}/{} gpu_atlas_hit={}/{} uptime={}",
			self.frame_count,
			self.error_count,
			self.row_count / self.frame_count.max(1),
			self.run_count / self.frame_count.max(1),
			self.quad_count / self.frame_count.max(1),
			self.glyph_count / self.frame_count.max(1),
			fmt_avg(self.prepare_time, self.frame_count),
			fmt_duration(self.prepare_max),
			fmt_avg(self.prepare_render_surface, self.frame_count),
			fmt_avg(self.prepare_quads_clone, self.frame_count),
			fmt_avg(self.prepare_vertex_build, self.frame_count),
			fmt_avg(self.prepare_atlas_build, self.frame_count),
			fmt_avg(self.prepare_uv_map, self.frame_count),
			fmt_avg(self.prepare_upload_bytes, self.frame_count),
			fmt_avg(self.upload_time, self.frame_count),
			fmt_duration(self.upload_max),
			fmt_avg(self.render_total, self.frame_count),
			fmt_duration(self.render_max),
			fmt_avg(self.present_total, self.frame_count),
			fmt_duration(self.present_max),
			fmt_avg(self.publish_total, self.frame_count),
			self.glyph_atlas_cpu_cache_hits,
			self.frame_count,
			self.glyph_atlas_gpu_cache_hits,
			self.frame_count,
			fmt_duration(self.started_at.elapsed()),
		);

		self.last_log_at = Instant::now();
		self.frame_count = 0;
		self.error_count = 0;
		self.row_count = 0;
		self.run_count = 0;
		self.quad_count = 0;
		self.vertex_count = 0;
		self.glyph_count = 0;
		self.prepare_time = Duration::ZERO;
		self.prepare_render_surface = Duration::ZERO;
		self.prepare_quads_clone = Duration::ZERO;
		self.prepare_vertex_build = Duration::ZERO;
		self.prepare_atlas_build = Duration::ZERO;
		self.prepare_uv_map = Duration::ZERO;
		self.prepare_upload_bytes = Duration::ZERO;
		self.upload_time = Duration::ZERO;
		self.encode_time = Duration::ZERO;
		self.render_total = Duration::ZERO;
		self.present_total = Duration::ZERO;
		self.publish_total = Duration::ZERO;
		self.prepare_max = Duration::ZERO;
		self.upload_max = Duration::ZERO;
		self.render_max = Duration::ZERO;
		self.present_max = Duration::ZERO;
		self.glyph_atlas_cpu_cache_hits = 0;
		self.glyph_atlas_gpu_cache_hits = 0;
	}
}

fn render_perf_logging_enabled() -> bool {
	env::var_os(RENDER_PERF_LOG_ENV).and_then(|value| value.into_string().ok()).is_some_and(|value| {
		matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
	})
}

fn fmt_avg(total: Duration, count: u64) -> String {
	if count == 0 {
		return "-".to_string();
	}

	fmt_duration(total / count as u32)
}

fn fmt_duration(duration: Duration) -> String {
	let micros = duration.as_micros();

	if micros < 1_000 {
		return format!("{micros}us");
	}

	let millis = duration.as_secs_f64() * 1_000.0;

	if millis < 1_000.0 {
		return format!("{millis:.2}ms");
	}

	format!("{:.2}s", duration.as_secs_f64())
}

impl ITerminalWindowRuntime for WgpuTerminalWindowRuntime {
	fn request_window_redraw(&self) { WgpuTerminalWindowRuntime::request_window_redraw(self) }

	fn set_surface_snapshot(&mut self, snapshot: RenderSurfaceSnapshot) {
		WgpuTerminalWindowRuntime::set_surface_snapshot(self, snapshot);
	}

	fn surface_snapshot_mut(&mut self) -> &mut RenderSurfaceSnapshot {
		WgpuTerminalWindowRuntime::surface_snapshot_mut(self)
	}

	fn resize_surface_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
		WgpuTerminalWindowRuntime::resize_surface_size_info(self, window_size)
	}

	fn take_redraw_request(&mut self) -> bool { WgpuTerminalWindowRuntime::take_redraw_request(self) }

	fn terminal_size_info(&self) -> TerminalSizeInfo {
		WgpuTerminalWindowRuntime::terminal_size_info(self)
	}

	fn render(&mut self) { WgpuTerminalWindowRuntime::render(self) }
}
