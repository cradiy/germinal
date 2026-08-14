use std::{
    collections::{HashMap, VecDeque},
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
    rendering::{
        render_target_id::RenderTargetId, surface_snapshot::RenderSurfaceSnapshot,
        window_runtime::ITerminalWindowRuntime, workspace_layout::RenderSurfacePlacement,
    },
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
    render_target_plan::{WgpuTerminalLoadOp, WgpuTerminalRenderTargetPlan},
    renderer_backend::WgpuRendererConfig,
    surface_frame_presenter::{
        WgpuTerminalSurfaceFramePresentError, WgpuTerminalSurfaceFramePresenter,
        WgpuTerminalWorkspacePresentInput, WgpuTerminalWorkspaceSurface,
    },
    video_surface_frame::WgpuVideoSurfaceNv12DmaBufFrame,
    video_surface_registry::WgpuVideoSurfaceRegistry,
    workspace_divider_renderer::WgpuWorkspaceDividerRenderer,
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
    window: Arc<Window>,
    base_title: String,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    pipeline: WgpuTerminalPipeline,
    presenter: WgpuTerminalSurfaceFramePresenter,

    surface_snapshots: HashMap<RenderTargetId, RenderSurfaceSnapshot>,
    workspace_layout: Vec<RenderSurfacePlacement>,
    size_info: TerminalSizeInfo,
    profile: TerminalProfile,
    needs_redraw: bool,
    ui_stats: WindowUiStats,
    perf: WgpuTerminalRenderPerf,
}

#[derive(Debug, Clone)]
pub struct WgpuTerminalWindowRuntimeFactory {
    profile: TerminalProfile,
    base_title: String,
}

impl WgpuTerminalWindowRuntimeFactory {
    pub fn new(profile: TerminalProfile, base_title: String) -> Self {
        Self {
            profile,
            base_title,
        }
    }

    pub fn create_window_runtime(
        &self,
        window: Arc<Window>,
    ) -> Result<WgpuTerminalWindowRuntime, WindowRuntimeError> {
        pollster::block_on(WgpuTerminalWindowRuntime::new(
            window,
            self.profile,
            self.base_title.clone(),
        ))
    }
}

impl WgpuTerminalWindowRuntime {
    pub async fn new(
        window: Arc<Window>,
        profile: TerminalProfile,
        base_title: String,
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
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
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

        let surface_config = surface.get_default_config(&adapter, width, height).ok_or(
            WindowRuntimeError::MissingSurfaceConfig {
                width_px: width,
                height_px: height,
            },
        )?;

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
        let divider_renderer = WgpuWorkspaceDividerRenderer::new(&device, surface_config.format);
        let presenter = WgpuTerminalSurfaceFramePresenter::new(frame_renderer, divider_renderer);

        Ok(Self {
            window,
            base_title,
            surface,
            device,
            queue,
            surface_config,
            pipeline,
            presenter,
            surface_snapshots: HashMap::new(),
            workspace_layout: Vec::new(),
            size_info,
            profile,
            needs_redraw: false,
            ui_stats: WindowUiStats::new(),
            perf: WgpuTerminalRenderPerf::new(),
        })
    }

    pub fn window_id(&self) -> WindowId {
        self.window.id()
    }

    pub fn window_size(&self) -> winit::dpi::PhysicalSize<u32> {
        self.window.inner_size()
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
        let targets = self
            .surface_snapshots
            .values()
            .filter(|snapshot| {
                snapshot
                    .video_surfaces
                    .iter()
                    .any(|surface| surface.id == id)
            })
            .map(|snapshot| snapshot.target_id)
            .collect::<Vec<_>>();
        let mut replaced_any = false;

        for target_id in targets {
            let imported = import_nv12_dmabuf_frame(&self.device, frame)
                .map_err(|source| WindowRuntimeError::ImportVideoSurfaceFrame { source })?;
            replaced_any |= self
                .video_surface_registry()
                .replace_nv12_frame(target_id, id, imported);
        }
        if replaced_any {
            self.request_window_redraw();
        }
        Ok(replaced_any)
    }

    pub fn request_window_redraw(&self) {
        self.window.request_redraw();
    }

    pub fn set_surface_snapshot(&mut self, snapshot: RenderSurfaceSnapshot) {
        self.surface_snapshots.insert(snapshot.target_id, snapshot);
        self.request_redraw();
    }

    pub fn remove_render_target(&mut self, target_id: RenderTargetId) {
        self.surface_snapshots.remove(&target_id);
        self.workspace_layout
            .retain(|placement| placement.target_id != target_id);
        self.presenter
            .frame_renderer()
            .remove_render_target(target_id);
        self.request_redraw();
    }

    pub fn surface_snapshots_mut(&mut self) -> Vec<&mut RenderSurfaceSnapshot> {
        self.surface_snapshots.values_mut().collect()
    }

    pub fn set_workspace_layout(&mut self, placements: Vec<RenderSurfacePlacement>) {
        self.workspace_layout = placements;
        self.request_redraw();
    }

    pub fn resize_surface_size_info(
        &mut self,
        window_size: TerminalWindowSize,
    ) -> TerminalSizeInfo {
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

    fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    pub fn take_redraw_request(&mut self) -> bool {
        let needs_redraw = self.needs_redraw;
        self.needs_redraw = false;
        needs_redraw
    }

    pub fn terminal_size_info(&self) -> TerminalSizeInfo {
        self.size_info.debug_assert_consistent();
        self.size_info
    }

    pub fn terminal_size_info_for_window_size(
        &self,
        window_size: TerminalWindowSize,
    ) -> TerminalSizeInfo {
        self.size_info_for_window_size(window_size)
    }

    fn size_info_for_window_size(&self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
        self.profile
            .size_info_for_window_metrics(TerminalWindowMetrics::new(
                window_size,
                TerminalScaleFactor::new(self.window.scale_factor()),
            ))
    }

    pub fn render(&mut self) {
        let surfaces = self
            .workspace_layout
            .iter()
            .filter_map(|placement| {
                let surface_snapshot = self.surface_snapshots.get(&placement.target_id)?;
                let size_info = self.terminal_size_info_for_window_size(placement.window_size());
                Some(WgpuTerminalWorkspaceSurface {
                    render_target_plan: WgpuTerminalRenderTargetPlan::new(
                        placement.width_px,
                        placement.height_px,
                    )
                    .with_origin(placement.x_px, placement.y_px)
                    .with_load_op(WgpuTerminalLoadOp::Load),
                    surface_snapshot,
                    renderer_config: WgpuRendererConfig::from(size_info),
                })
            })
            .collect::<Vec<_>>();
        let row_count = surfaces
            .iter()
            .map(|surface| surface.surface_snapshot.rows.len() as u64)
            .sum();
        let run_count = surfaces
            .iter()
            .flat_map(|surface| &surface.surface_snapshot.rows)
            .map(|row| row.runs.len() as u64)
            .sum();

        match self
            .presenter
            .present_workspace_frame(WgpuTerminalWorkspacePresentInput {
                surface: &self.surface,
                device: &self.device,
                queue: &self.queue,
                pipeline: &self.pipeline,
                surfaces: &surfaces,
            }) {
            Ok(result) => {
                if let Some(title) = self
                    .ui_stats
                    .record_presented_frame(Instant::now(), &self.base_title)
                {
                    self.window.set_title(&title);
                }
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

    let glyph_config =
        profile.glyph_render_config(size_info, TerminalScaleFactor::new(scale_factor));
    let terminal_cell_size = glyph_config.cell_size();
    let crossfont_builder = WgpuCrossfontGlyphAtlasBuilder::new(
        glyph_config.font_family_name(),
        glyph_config.font_size_px(),
    )
    .map_err(WindowRuntimeError::BuildGlyphAtlas)?
    .with_bold_font_weight(wgpu_font_weight_from_terminal(
        glyph_config.bold_font_weight(),
    ))
    .with_padding_px(2)
    .with_columns(16)
    .with_max_texture_dimension_2d(max_texture_dimension_2d)
    .with_cell_size_px(
        terminal_cell_size.width_px(),
        terminal_cell_size.height_px(),
    );

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

    Ok(profile.with_cell_size(TerminalCellSize::new(
        metrics.cell_width_px(),
        metrics.cell_height_px(),
    )))
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
const UI_FPS_WINDOW: Duration = Duration::from_secs(1);
const UI_TITLE_UPDATE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug)]
struct WindowUiStats {
    frame_count: u64,
    presented_frames: VecDeque<Instant>,
    last_title_update: Option<Instant>,
}

impl WindowUiStats {
    fn new() -> Self {
        Self {
            frame_count: 0,
            presented_frames: VecDeque::new(),
            last_title_update: None,
        }
    }

    fn record_presented_frame(&mut self, now: Instant, base_title: &str) -> Option<String> {
        self.frame_count += 1;
        self.presented_frames.push_back(now);
        while let Some(oldest) = self.presented_frames.front().copied() {
            if now.saturating_duration_since(oldest) <= UI_FPS_WINDOW {
                break;
            }
            self.presented_frames.pop_front();
        }

        if self.last_title_update.is_some_and(|updated_at| {
            now.saturating_duration_since(updated_at) < UI_TITLE_UPDATE_INTERVAL
        }) {
            return None;
        }

        self.last_title_update = Some(now);
        Some(format!(
            "{base_title} | ui_frame={} ui_fps={:.1}",
            self.frame_count,
            self.presented_frames.len() as f32 / UI_FPS_WINDOW.as_secs_f32()
        ))
    }
}

struct WgpuTerminalRenderPerf {
    logging_enabled: bool,
    started_at: Instant,
    last_log_at: Instant,
    frame_count: u64,
    error_count: u64,
    row_count: u64,
    run_count: u64,
    quad_count: u64,
    vertex_count: u64,
    glyph_count: u64,
    prepare_time: Duration,
    prepare_render_surface: Duration,
    prepare_quads_clone: Duration,
    prepare_vertex_build: Duration,
    prepare_atlas_build: Duration,
    prepare_uv_map: Duration,
    prepare_upload_bytes: Duration,
    upload_time: Duration,
    encode_time: Duration,
    render_total: Duration,
    present_total: Duration,
    publish_total: Duration,
    prepare_max: Duration,
    upload_max: Duration,
    render_max: Duration,
    present_max: Duration,
    glyph_atlas_cpu_cache_hits: u64,
    glyph_atlas_gpu_cache_hits: u64,
}

impl WgpuTerminalRenderPerf {
    fn new() -> Self {
        let now = Instant::now();

        Self {
            logging_enabled: render_perf_logging_enabled(),
            started_at: now,
            last_log_at: now,
            frame_count: 0,
            error_count: 0,
            row_count: 0,
            run_count: 0,
            quad_count: 0,
            vertex_count: 0,
            glyph_count: 0,
            prepare_time: Duration::ZERO,
            prepare_render_surface: Duration::ZERO,
            prepare_quads_clone: Duration::ZERO,
            prepare_vertex_build: Duration::ZERO,
            prepare_atlas_build: Duration::ZERO,
            prepare_uv_map: Duration::ZERO,
            prepare_upload_bytes: Duration::ZERO,
            upload_time: Duration::ZERO,
            encode_time: Duration::ZERO,
            render_total: Duration::ZERO,
            present_total: Duration::ZERO,
            publish_total: Duration::ZERO,
            prepare_max: Duration::ZERO,
            upload_max: Duration::ZERO,
            render_max: Duration::ZERO,
            present_max: Duration::ZERO,
            glyph_atlas_cpu_cache_hits: 0,
            glyph_atlas_gpu_cache_hits: 0,
        }
    }

    fn record_frame(
        &mut self,
        row_count: u64,
        run_count: u64,
        result: &crate::rendering::pty_surface::surface_frame_presenter::WgpuTerminalWorkspaceFramePresentResult,
    ) {
        if !self.logging_enabled {
            return;
        }

        self.frame_count += 1;
        self.row_count += row_count;
        self.run_count += run_count;
        for render_result in &result.render_results {
            self.quad_count += render_result.quad_count as u64;
            self.vertex_count += render_result.vertex_count as u64;
            self.glyph_count += render_result.glyph_count as u64;
            self.prepare_time += render_result.timings.prepare;
            self.prepare_render_surface += render_result.timings.prepared_frame.render_surface;
            self.prepare_quads_clone += render_result.timings.prepared_frame.quads_clone;
            self.prepare_vertex_build += render_result.timings.prepared_frame.vertex_build;
            self.prepare_atlas_build += render_result.timings.prepared_frame.atlas_build;
            self.prepare_uv_map += render_result.timings.prepared_frame.uv_map;
            self.prepare_upload_bytes += render_result.timings.prepared_frame.upload_bytes;
            self.upload_time += render_result.timings.upload;
            self.encode_time += render_result.timings.encode;
            self.render_total += render_result.timings.total;
            self.prepare_max = self.prepare_max.max(render_result.timings.prepare);
            self.upload_max = self.upload_max.max(render_result.timings.upload);
            self.render_max = self.render_max.max(render_result.timings.total);
            if render_result.glyph_atlas_cpu_cache_hit {
                self.glyph_atlas_cpu_cache_hits += 1;
            }
            if render_result.glyph_atlas_gpu_cache_hit {
                self.glyph_atlas_gpu_cache_hits += 1;
            }
        }
        self.present_total += result.timings.render_to_view;
        self.publish_total += result.timings.total;
        self.present_max = self.present_max.max(result.timings.total);

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
    env::var_os(RENDER_PERF_LOG_ENV)
        .and_then(|value| value.into_string().ok())
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
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
    fn request_window_redraw(&self) {
        WgpuTerminalWindowRuntime::request_window_redraw(self)
    }

    fn set_surface_snapshot(&mut self, snapshot: RenderSurfaceSnapshot) {
        WgpuTerminalWindowRuntime::set_surface_snapshot(self, snapshot);
    }

    fn remove_render_target(&mut self, target_id: RenderTargetId) {
        WgpuTerminalWindowRuntime::remove_render_target(self, target_id);
    }

    fn surface_snapshots_mut(&mut self) -> Vec<&mut RenderSurfaceSnapshot> {
        WgpuTerminalWindowRuntime::surface_snapshots_mut(self)
    }

    fn set_workspace_layout(&mut self, placements: Vec<RenderSurfacePlacement>) {
        WgpuTerminalWindowRuntime::set_workspace_layout(self, placements);
    }

    fn resize_surface_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
        WgpuTerminalWindowRuntime::resize_surface_size_info(self, window_size)
    }

    fn terminal_size_info_for_window_size(
        &self,
        window_size: TerminalWindowSize,
    ) -> TerminalSizeInfo {
        WgpuTerminalWindowRuntime::terminal_size_info_for_window_size(self, window_size)
    }

    fn take_redraw_request(&mut self) -> bool {
        WgpuTerminalWindowRuntime::take_redraw_request(self)
    }

    fn terminal_size_info(&self) -> TerminalSizeInfo {
        WgpuTerminalWindowRuntime::terminal_size_info(self)
    }

    fn render(&mut self) {
        WgpuTerminalWindowRuntime::render(self)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::WindowUiStats;

    #[test]
    fn window_ui_stats_formats_presented_frame_fps() {
        let mut stats = WindowUiStats::new();
        let start = Instant::now();

        let title = stats
            .record_presented_frame(start, "germinal")
            .expect("first frame should update title");
        assert_eq!(title, "germinal | ui_frame=1 ui_fps=1.0");

        assert!(
            stats
                .record_presented_frame(start + Duration::from_millis(100), "germinal")
                .is_none()
        );

        let title = stats
            .record_presented_frame(start + Duration::from_millis(300), "germinal")
            .expect("title should refresh after throttle interval");
        assert_eq!(title, "germinal | ui_frame=3 ui_fps=3.0");
    }

    #[test]
    fn window_ui_stats_drops_frames_outside_fps_window() {
        let mut stats = WindowUiStats::new();
        let start = Instant::now();

        stats.record_presented_frame(start, "germinal");
        let title = stats
            .record_presented_frame(start + Duration::from_millis(1250), "germinal")
            .expect("title should refresh after throttle interval");
        assert_eq!(title, "germinal | ui_frame=2 ui_fps=1.0");
    }
}
