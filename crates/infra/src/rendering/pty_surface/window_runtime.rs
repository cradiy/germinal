use std::{
    collections::{HashMap, HashSet},
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use germinal_ports::{
    pty_host::{
        cell_size::TerminalCellSize,
        color_theme::TerminalColorTheme,
        profile::TerminalProfile,
        scale_factor::TerminalScaleFactor,
        size_info::TerminalSizeInfo,
        terminal_progress::TerminalProgress,
        width::{terminal_char_cell_width, terminal_text_cell_width},
        window_metrics::TerminalWindowMetrics,
        window_size::TerminalWindowSize,
    },
    rendering::{
        frame_plan_builder::{RgbColorDto, TextStyleDto},
        render_target_id::RenderTargetId,
        surface_snapshot::{
            RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot, RenderSurfaceRowSnapshot,
            RenderSurfaceRunSnapshot, RenderSurfaceSnapshot, merge_surface_dirty_rows,
        },
        tab_bar::{TabBarPosition, TabBarSnapshot},
        window_runtime::ITerminalWindowRuntime,
        workspace_layout::RenderSurfacePlacement,
    },
    seq::Seq,
};
use thiserror::Error;
use tracing::{error, info, warn};
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    window::{UserAttentionType, Window, WindowId},
};

#[cfg(target_os = "linux")]
use crate::rendering::pty_surface::video_surface_dmabuf_importer::{
    VideoSurfaceImportError, import_nv12_dmabuf_frame,
};
use crate::rendering::pty_surface::{
    background_shader_renderer::{
        WgpuBackgroundShaderError, WgpuBackgroundShaderRenderer, WgpuBackgroundShaderSource,
    },
    crossfont_glyph_atlas::{WgpuCrossfontGlyphAtlasBuilder, WgpuCrossfontGlyphAtlasError},
    frame_builder::WgpuTerminalFrameBuilder,
    frame_renderer::WgpuTerminalFrameRenderer,
    pipeline_factory::{WgpuTerminalPipeline, WgpuTerminalPipelineFactory},
    pipeline_spec::WgpuTerminalPipelineSpec,
    render_plugin::{WgpuPaneRenderPlugin, WgpuPaneResizeEvent},
    render_target_plan::{
        WgpuTerminalClearColor, WgpuTerminalLoadOp, WgpuTerminalRenderTargetPlan,
    },
    renderer_backend::WgpuRendererConfig,
    surface_frame_presenter::{
        WgpuTerminalSurfaceFramePresentError, WgpuTerminalSurfaceFramePresenter,
        WgpuTerminalWorkspacePresentInput, WgpuTerminalWorkspaceSurface,
    },
    video_surface_frame::WgpuVideoSurfaceNv12DmaBufFrame,
    video_surface_registry::WgpuVideoSurfaceRegistry,
    visual_bell_renderer::{WgpuVisualBellFrame, WgpuVisualBellRenderer},
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
    #[error("failed to create terminal background shader: {0}")]
    CreateBackgroundShader(#[source] WgpuBackgroundShaderError),
    #[cfg(target_os = "linux")]
    #[error("failed to import an NV12 dma_buf video frame into the terminal renderer: {source}")]
    ImportVideoSurfaceFrame {
        #[source]
        source: VideoSurfaceImportError,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WgpuTerminalPowerPreference {
    #[default]
    High,
    Low,
}

impl WgpuTerminalPowerPreference {
    fn wgpu(self) -> wgpu::PowerPreference {
        match self {
            Self::High => wgpu::PowerPreference::HighPerformance,
            Self::Low => wgpu::PowerPreference::LowPower,
        }
    }
}

pub fn detect_terminal_power_preference() -> WgpuTerminalPowerPreference {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: terminal_wgpu_backends(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapters = pollster::block_on(instance.enumerate_adapters(terminal_wgpu_backends()));
    terminal_power_preference_for_device_types(
        adapters
            .into_iter()
            .map(|adapter| adapter.get_info().device_type),
    )
}

fn terminal_power_preference_for_device_types(
    device_types: impl IntoIterator<Item = wgpu::DeviceType>,
) -> WgpuTerminalPowerPreference {
    if device_types
        .into_iter()
        .any(|device_type| device_type == wgpu::DeviceType::DiscreteGpu)
    {
        WgpuTerminalPowerPreference::High
    } else {
        WgpuTerminalPowerPreference::Low
    }
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
    pending_surface_dirty_rows: HashMap<RenderTargetId, Vec<u32>>,
    workspace_layout: Vec<RenderSurfacePlacement>,
    tab_bar: Option<TabBarSnapshot>,
    tab_bar_surface: Option<WgpuTabBarSurface>,
    tab_bar_generation: u64,
    size_info: TerminalSizeInfo,
    profile: TerminalProfile,
    scale_factor: TerminalScaleFactor,
    color_theme: TerminalColorTheme,
    background_opacity: f32,
    background_shader_enabled: bool,
    background_shader_animated: bool,
    window_occluded: bool,
    retain_terminal_frame: bool,
    next_background_frame_at: Option<Instant>,
    needs_redraw: bool,
    display_refresh_rate_millihertz: Option<u32>,
    frame_interval: Duration,
    next_present_at: Instant,
    visual_bell_until: Option<Instant>,
    cursor_blink_interval: Duration,
    cursor_blink_signature: Option<CursorBlinkSignature>,
    cursor_blink_epoch: Instant,
    next_cursor_blink_at: Option<Instant>,
    cursor_motion_duration: Duration,
    cursor_motion_on_input: bool,
    cursor_motion_on_enter: bool,
    cursor_motions: HashMap<RenderTargetId, CursorMotion>,
    next_cursor_motion_frame_at: Option<Instant>,
    perf: WgpuTerminalRenderPerf,
    render_plugins: Vec<WgpuPaneRenderPlugin>,
    started_at: Instant,
}

fn accumulate_pending_surface_damage(
    pending_by_target: &mut HashMap<RenderTargetId, Vec<u32>>,
    snapshot: &mut RenderSurfaceSnapshot,
) {
    if let Some(pending) = pending_by_target.get(&snapshot.target_id) {
        merge_surface_dirty_rows(&mut snapshot.dirty_rows, pending);
    }
    pending_by_target.insert(snapshot.target_id, snapshot.dirty_rows.clone());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorBlinkSignature {
    target_id: RenderTargetId,
    x: u32,
    y: u32,
    shape: RenderSurfaceCursorShape,
}

#[derive(Debug, Clone, Copy)]
struct CursorMotion {
    from_x: f32,
    from_y: f32,
    to_x: f32,
    to_y: f32,
    started_at: Instant,
    duration: Duration,
    waiting_for_line_feed: bool,
}

fn is_enter_cursor_motion(
    previous: RenderSurfaceCursorSnapshot,
    next: RenderSurfaceCursorSnapshot,
) -> bool {
    next.x == 0 && (previous.x != next.x || previous.y != next.y)
}

fn build_cursor_motion(
    existing: Option<CursorMotion>,
    previous: RenderSurfaceCursorSnapshot,
    next: RenderSurfaceCursorSnapshot,
    now: Instant,
    duration: Duration,
    frame_interval: Duration,
) -> CursorMotion {
    let coalesce_delay = frame_interval.min(Duration::from_millis(8));
    let line_feed_follows_carriage_return = existing.is_some_and(|motion| {
        motion.waiting_for_line_feed
            && previous.x == 0
            && next.x == 0
            && previous.y != next.y
            && now <= motion.started_at
    });
    if line_feed_follows_carriage_return {
        let motion = existing.expect("coalesced line feed requires an existing cursor motion");
        return CursorMotion {
            to_x: next.x as f32,
            to_y: next.y as f32,
            waiting_for_line_feed: false,
            ..motion
        };
    }

    let (from_x, from_y) = existing
        .map(|motion| motion.position_at(now).0)
        .unwrap_or((previous.x as f32, previous.y as f32));
    let waiting_for_line_feed = previous.x > 0 && next.x == 0 && previous.y == next.y;
    CursorMotion {
        from_x,
        from_y,
        to_x: next.x as f32,
        to_y: next.y as f32,
        started_at: if waiting_for_line_feed {
            now + coalesce_delay
        } else {
            now
        },
        duration,
        waiting_for_line_feed,
    }
}

impl CursorMotion {
    fn position_at(self, now: Instant) -> ((f32, f32), bool) {
        if self.duration.is_zero() {
            return ((self.to_x, self.to_y), false);
        }
        let progress = now.saturating_duration_since(self.started_at).as_secs_f32()
            / self.duration.as_secs_f32();
        if progress >= 1.0 {
            return ((self.to_x, self.to_y), false);
        }

        let remaining = 1.0 - progress.clamp(0.0, 1.0);
        let eased = 1.0 - remaining * remaining * remaining;
        (
            (
                self.from_x + (self.to_x - self.from_x) * eased,
                self.from_y + (self.to_y - self.from_y) * eased,
            ),
            true,
        )
    }
}

fn normalized_opacity(opacity: f32) -> f32 {
    if opacity.is_finite() {
        opacity.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn terminal_wgpu_backends() -> wgpu::Backends {
    // GL is a secondary wgpu backend and initializes an additional driver stack even when Vulkan
    // is selected. Germinal's Linux dma-buf video path also requires Vulkan.
    wgpu::Backends::PRIMARY
}

const LOW_LATENCY_SURFACE_FRAME_QUEUE: u32 = 1;
const DEFAULT_CURSOR_MOTION_DURATION: Duration = Duration::from_millis(80);

fn frame_interval_for_refresh_rate(refresh_rate_millihertz: Option<u32>) -> Duration {
    let refresh_rate_millihertz = refresh_rate_millihertz
        .filter(|rate| *rate > 0)
        .unwrap_or(60_000);
    Duration::from_nanos(1_000_000_000_000_u64 / u64::from(refresh_rate_millihertz))
}

fn display_refresh_rate_millihertz(window: &Window) -> Option<u32> {
    window
        .current_monitor()
        .and_then(|monitor| monitor.refresh_rate_millihertz())
        .filter(|rate| *rate > 0)
}

fn transparent_surface_alpha_mode(
    supported: &[wgpu::CompositeAlphaMode],
) -> wgpu::CompositeAlphaMode {
    if supported.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
        wgpu::CompositeAlphaMode::PreMultiplied
    } else {
        wgpu::CompositeAlphaMode::Auto
    }
}

fn ime_cursor_area(
    placement: RenderSurfacePlacement,
    size_info: TerminalSizeInfo,
    snapshot: &RenderSurfaceSnapshot,
) -> Option<(PhysicalPosition<u32>, PhysicalSize<u32>)> {
    let cursor = snapshot.cursor?;
    let viewport = size_info.render_viewport();
    let cell_size = viewport.cell_size();
    let (cursor_x, cursor_y) = snapshot
        .ime_preedit
        .as_ref()
        .and_then(|preedit| {
            preedit.cursor_cell(cursor, viewport.columns() as u32, viewport.rows() as u32)
        })
        .unwrap_or((cursor.x, cursor.y));
    let x = placement
        .x_px
        .saturating_add(viewport.origin_x_px())
        .saturating_add(cursor_x.saturating_mul(cell_size.width_px()));
    let grid_rows = (viewport.rows() as u32).max(1);
    let row_offset = |row: u32| {
        ((u64::from(row.min(grid_rows)) * u64::from(size_info.content_height_px()))
            / u64::from(grid_rows)) as u32
    };
    let row_top = row_offset(cursor_y);
    let row_height = row_offset(cursor_y.saturating_add(1))
        .saturating_sub(row_top)
        .max(1);
    let y = placement
        .y_px
        .saturating_add(viewport.origin_y_px())
        .saturating_add(row_top);

    Some((
        PhysicalPosition::new(x, y),
        PhysicalSize::new(cell_size.width_px().max(1), row_height),
    ))
}

#[derive(Debug, Clone)]
pub struct WgpuTerminalWindowRuntimeFactory {
    profile: TerminalProfile,
    base_title: String,
    cursor_blink_interval: Duration,
    cursor_motion_duration: Duration,
    cursor_motion_on_input: bool,
    cursor_motion_on_enter: bool,
    color_theme: TerminalColorTheme,
    background_opacity: f32,
    background_shader: Option<WgpuBackgroundShaderSource>,
    power_preference: WgpuTerminalPowerPreference,
}

struct WgpuTerminalWindowRuntimeOptions {
    profile: TerminalProfile,
    base_title: String,
    cursor_blink_interval: Duration,
    cursor_motion_duration: Duration,
    cursor_motion_on_input: bool,
    cursor_motion_on_enter: bool,
    color_theme: TerminalColorTheme,
    background_opacity: f32,
    render_plugins: Vec<WgpuPaneRenderPlugin>,
    background_shader: Option<WgpuBackgroundShaderSource>,
    power_preference: WgpuTerminalPowerPreference,
}

impl WgpuTerminalWindowRuntimeFactory {
    pub fn new(
        profile: TerminalProfile,
        base_title: String,
        cursor_blink_interval: Duration,
        cursor_motion_duration: Duration,
        color_theme: TerminalColorTheme,
        background_opacity: f32,
    ) -> Self {
        Self {
            profile,
            base_title,
            cursor_blink_interval,
            cursor_motion_duration,
            cursor_motion_on_input: true,
            cursor_motion_on_enter: true,
            color_theme,
            background_opacity: normalized_opacity(background_opacity),
            background_shader: None,
            power_preference: WgpuTerminalPowerPreference::default(),
        }
    }

    pub fn with_background_shader(mut self, shader: WgpuBackgroundShaderSource) -> Self {
        self.background_shader = Some(shader);
        self
    }

    pub fn with_cursor_motion_modes(mut self, on_input: bool, on_enter: bool) -> Self {
        self.cursor_motion_on_input = on_input;
        self.cursor_motion_on_enter = on_enter;
        self
    }

    pub fn with_power_preference(mut self, power_preference: WgpuTerminalPowerPreference) -> Self {
        self.power_preference = power_preference;
        self
    }

    pub fn create_window_runtime(
        &self,
        window: Arc<Window>,
    ) -> Result<WgpuTerminalWindowRuntime, WindowRuntimeError> {
        self.create_window_runtime_with_plugins(window, Vec::new())
    }

    pub fn create_window_runtime_with_plugins(
        &self,
        window: Arc<Window>,
        render_plugins: Vec<WgpuPaneRenderPlugin>,
    ) -> Result<WgpuTerminalWindowRuntime, WindowRuntimeError> {
        pollster::block_on(WgpuTerminalWindowRuntime::new_with_options(
            window,
            WgpuTerminalWindowRuntimeOptions {
                profile: self.profile.clone(),
                base_title: self.base_title.clone(),
                cursor_blink_interval: self.cursor_blink_interval,
                cursor_motion_duration: self.cursor_motion_duration,
                cursor_motion_on_input: self.cursor_motion_on_input,
                cursor_motion_on_enter: self.cursor_motion_on_enter,
                color_theme: self.color_theme,
                background_opacity: self.background_opacity,
                render_plugins,
                background_shader: self.background_shader.clone(),
                power_preference: self.power_preference,
            },
        ))
    }
}

impl WgpuTerminalWindowRuntime {
    pub async fn new(
        window: Arc<Window>,
        profile: TerminalProfile,
        base_title: String,
        cursor_blink_interval: Duration,
        color_theme: TerminalColorTheme,
        background_opacity: f32,
    ) -> Result<Self, WindowRuntimeError> {
        Self::new_with_render_plugins(
            window,
            profile,
            base_title,
            cursor_blink_interval,
            color_theme,
            background_opacity,
            Vec::new(),
        )
        .await
    }

    pub async fn new_with_render_plugins(
        window: Arc<Window>,
        profile: TerminalProfile,
        base_title: String,
        cursor_blink_interval: Duration,
        color_theme: TerminalColorTheme,
        background_opacity: f32,
        render_plugins: Vec<WgpuPaneRenderPlugin>,
    ) -> Result<Self, WindowRuntimeError> {
        Self::new_with_options(
            window,
            WgpuTerminalWindowRuntimeOptions {
                profile,
                base_title,
                cursor_blink_interval,
                cursor_motion_duration: DEFAULT_CURSOR_MOTION_DURATION,
                cursor_motion_on_input: true,
                cursor_motion_on_enter: true,
                color_theme,
                background_opacity,
                render_plugins,
                background_shader: None,
                power_preference: WgpuTerminalPowerPreference::default(),
            },
        )
        .await
    }

    async fn new_with_options(
        window: Arc<Window>,
        options: WgpuTerminalWindowRuntimeOptions,
    ) -> Result<Self, WindowRuntimeError> {
        let WgpuTerminalWindowRuntimeOptions {
            profile,
            base_title,
            cursor_blink_interval,
            cursor_motion_duration,
            cursor_motion_on_input,
            cursor_motion_on_enter,
            color_theme,
            background_opacity,
            render_plugins,
            background_shader,
            power_preference,
        } = options;
        let background_opacity = normalized_opacity(background_opacity);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: terminal_wgpu_backends(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let surface = instance
            .create_surface(Arc::clone(&window))
            .map_err(|source| WindowRuntimeError::CreateSurface { source })?;

        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: power_preference.wgpu(),
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

        let adapter_info = adapter.get_info();
        let device_limits = device.limits();
        info!(
            adapter = %adapter_info.name,
            backend = ?adapter_info.backend,
            device_type = ?adapter_info.device_type,
            driver = %adapter_info.driver,
            driver_info = %adapter_info.driver_info,
            vendor_id = adapter_info.vendor,
            device_id = adapter_info.device,
            max_texture_dimension_2d = device_limits.max_texture_dimension_2d,
            "selected terminal GPU adapter"
        );
        device.on_uncaptured_error(Arc::new(|source| {
            error!(?source, "uncaptured terminal GPU error");
        }));
        device.set_device_lost_callback(|reason, message| {
            error!(?reason, %message, "terminal GPU device was lost");
        });

        let mut surface_config = surface.get_default_config(&adapter, width, height).ok_or(
            WindowRuntimeError::MissingSurfaceConfig {
                width_px: width,
                height_px: height,
            },
        )?;
        // Terminal frames are cheap and input latency matters more than CPU/GPU overlap. Keeping a
        // single monitor refresh in flight prevents a completed frame from waiting behind another
        // frame in the swapchain queue.
        surface_config.desired_maximum_frame_latency = LOW_LATENCY_SURFACE_FRAME_QUEUE;
        let surface_capabilities = surface.get_capabilities(&adapter);
        let retain_terminal_frame = adapter_info.device_type == wgpu::DeviceType::Cpu
            && surface_capabilities
                .usages
                .contains(wgpu::TextureUsages::COPY_DST);
        if retain_terminal_frame {
            surface_config.usage |= wgpu::TextureUsages::COPY_DST;
            info!("enabled retained terminal frames for CPU software rendering");
        }
        if background_opacity < 1.0 {
            let alpha_modes = surface_capabilities.alpha_modes;
            surface_config.alpha_mode = transparent_surface_alpha_mode(&alpha_modes);
            if surface_config.alpha_mode != wgpu::CompositeAlphaMode::PreMultiplied {
                warn!(
                    ?alpha_modes,
                    selected = ?surface_config.alpha_mode,
                    "surface does not expose premultiplied alpha; window transparency is platform-dependent"
                );
            }
        }

        surface.configure(&device, &surface_config);

        let pipeline_spec = WgpuTerminalPipelineSpec::new(surface_config.format);
        let pipeline_factory = WgpuTerminalPipelineFactory::new(pipeline_spec);
        let pipeline = pipeline_factory.create(&device);

        let scale_factor = TerminalScaleFactor::new(window.scale_factor());
        let profile = terminal_profile_from_alacritty_crossfont_metrics(profile, scale_factor)?;
        let size_info = terminal_size_info(&profile, width, height, scale_factor);

        let frame_builder = build_terminal_frame_builder(
            &profile,
            size_info,
            scale_factor,
            device_limits.max_texture_dimension_2d,
            color_theme,
        )?;
        let frame_renderer = WgpuTerminalFrameRenderer::new(frame_builder);
        let divider_renderer = WgpuWorkspaceDividerRenderer::new(
            &device,
            surface_config.format,
            color_theme.inactive_border,
        );
        let visual_bell_renderer =
            WgpuVisualBellRenderer::new(&device, surface_config.format, color_theme.bell_border);
        let background_shader_animated = background_shader
            .as_ref()
            .is_some_and(WgpuBackgroundShaderSource::animated);
        let background_shader_renderer = match background_shader.as_ref() {
            Some(source) => Some(
                WgpuBackgroundShaderRenderer::new(&device, surface_config.format, source)
                    .await
                    .map_err(WindowRuntimeError::CreateBackgroundShader)?,
            ),
            None => None,
        };
        let background_shader_enabled = background_shader_renderer.is_some();
        let mut presenter = WgpuTerminalSurfaceFramePresenter::new(
            frame_renderer,
            divider_renderer,
            visual_bell_renderer,
        );
        if let Some(renderer) = background_shader_renderer {
            presenter = presenter.with_background_shader(renderer);
        }

        let now = Instant::now();
        let display_refresh_rate_millihertz = display_refresh_rate_millihertz(window.as_ref());
        let frame_interval = frame_interval_for_refresh_rate(display_refresh_rate_millihertz);
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
            pending_surface_dirty_rows: HashMap::new(),
            workspace_layout: Vec::new(),
            tab_bar: None,
            tab_bar_surface: None,
            tab_bar_generation: 0,
            size_info,
            profile,
            scale_factor,
            color_theme,
            background_opacity,
            background_shader_enabled,
            background_shader_animated,
            window_occluded: false,
            retain_terminal_frame,
            next_background_frame_at: background_shader_animated.then_some(now),
            needs_redraw: false,
            display_refresh_rate_millihertz,
            frame_interval,
            next_present_at: now,
            visual_bell_until: None,
            cursor_blink_interval: cursor_blink_interval.max(Duration::from_millis(1)),
            cursor_blink_signature: None,
            cursor_blink_epoch: now,
            next_cursor_blink_at: None,
            cursor_motion_duration,
            cursor_motion_on_input,
            cursor_motion_on_enter,
            cursor_motions: HashMap::new(),
            next_cursor_motion_frame_at: None,
            perf: WgpuTerminalRenderPerf::new(),
            render_plugins,
            started_at: now,
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

    pub fn set_surface_snapshot(&mut self, mut snapshot: RenderSurfaceSnapshot) {
        let visible = render_target_is_visible(&self.workspace_layout, snapshot.target_id);
        if visible {
            self.update_cursor_motion(&snapshot);
        } else {
            self.cursor_motions.remove(&snapshot.target_id);
            if self.cursor_motions.is_empty() {
                self.next_cursor_motion_frame_at = None;
            }
        }
        accumulate_pending_surface_damage(&mut self.pending_surface_dirty_rows, &mut snapshot);
        self.surface_snapshots.insert(snapshot.target_id, snapshot);
        if visible {
            self.schedule_redraw();
        }
    }

    pub fn update_ime_cursor_area(&self, target_id: RenderTargetId) -> bool {
        let Some(placement) = self
            .workspace_layout
            .iter()
            .find(|placement| placement.target_id == target_id)
        else {
            return false;
        };
        let Some(snapshot) = self.surface_snapshots.get(&target_id) else {
            return false;
        };
        let size_info = self.terminal_size_info_for_window_size(placement.window_size());
        let Some((position, size)) = ime_cursor_area(*placement, size_info, snapshot) else {
            return false;
        };

        self.window.set_ime_cursor_area(position, size);
        true
    }

    pub fn remove_render_target(&mut self, target_id: RenderTargetId) {
        self.surface_snapshots.remove(&target_id);
        self.cursor_motions.remove(&target_id);
        self.pending_surface_dirty_rows.remove(&target_id);
        self.workspace_layout
            .retain(|placement| placement.target_id != target_id);
        self.presenter
            .frame_renderer()
            .remove_render_target(target_id);
        self.render_plugins
            .retain(|plugin| plugin.target_id() != target_id);
        self.schedule_redraw();
    }

    pub fn surface_snapshots_mut(&mut self) -> Vec<&mut RenderSurfaceSnapshot> {
        self.surface_snapshots.values_mut().collect()
    }

    pub fn set_workspace_layout(&mut self, placements: Vec<RenderSurfacePlacement>) {
        let visible_targets = placements
            .iter()
            .map(|placement| placement.target_id)
            .collect::<HashSet<_>>();
        let hidden_targets = self
            .surface_snapshots
            .keys()
            .filter(|target_id| !visible_targets.contains(target_id))
            .copied()
            .collect::<Vec<_>>();
        for target_id in hidden_targets {
            self.presenter
                .frame_renderer()
                .release_render_target_cache(target_id);
            self.cursor_motions.remove(&target_id);
        }
        if self.cursor_motions.is_empty() {
            self.next_cursor_motion_frame_at = None;
        }

        let scale_factor = self.scale_factor.value();
        for plugin in &mut self.render_plugins {
            let Some(placement) = placements
                .iter()
                .find(|placement| placement.target_id == plugin.target_id())
                .copied()
            else {
                continue;
            };
            let _ = plugin.resize(WgpuPaneResizeEvent {
                placement,
                scale_factor,
            });
        }
        self.workspace_layout = placements;
        self.schedule_redraw();
    }

    pub fn route_wgpu_pane_input(
        &mut self,
        target_id: RenderTargetId,
        event: &germinal_ports::event::window_input_event::WindowInputEvent,
    ) -> bool {
        let Some(plugin) = self
            .render_plugins
            .iter_mut()
            .find(|plugin| plugin.target_id() == target_id)
        else {
            return false;
        };
        let result = plugin.input(event);
        if result.request_redraw {
            self.schedule_redraw();
        }
        true
    }

    pub fn set_tab_bar(&mut self, tab_bar: Option<TabBarSnapshot>) {
        self.tab_bar = tab_bar;
        self.rebuild_tab_bar_surface();
        self.schedule_redraw();
    }

    fn rebuild_tab_bar_surface(&mut self) {
        self.tab_bar_generation = self.tab_bar_generation.wrapping_add(1).max(1);
        self.tab_bar_surface = self.tab_bar.as_ref().and_then(|tab_bar| {
            let mut surface = build_tab_bar_surface(tab_bar, self.size_info, self.color_theme)?;
            surface.snapshot.latest_seq = Seq::new(self.tab_bar_generation);
            Some(surface)
        });
    }

    pub fn set_window_title(&mut self, title: &str) {
        if self.base_title == title {
            return;
        }

        self.base_title.clear();
        self.base_title.push_str(title);
        self.window.set_title(title);
    }

    pub fn ring_bell(&mut self, visual_duration: Duration, request_attention: bool) {
        if !visual_duration.is_zero() {
            let until = Instant::now() + visual_duration;
            self.visual_bell_until = Some(
                self.visual_bell_until
                    .map_or(until, |current| current.max(until)),
            );
            self.schedule_redraw();
        }

        if request_attention {
            self.window
                .request_user_attention(Some(UserAttentionType::Informational));
        }
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
        self.rebuild_tab_bar_surface();

        self.schedule_redraw();

        self.terminal_size_info()
    }

    pub fn update_scale_factor(
        &mut self,
        scale_factor: f64,
    ) -> Result<TerminalSizeInfo, WindowRuntimeError> {
        let scale_factor = TerminalScaleFactor::new(scale_factor);
        if self.scale_factor == scale_factor {
            return Ok(self.terminal_size_info());
        }

        let profile =
            terminal_profile_from_alacritty_crossfont_metrics(self.profile.clone(), scale_factor)?;
        let window_size = self.window.inner_size();
        let size_info = terminal_size_info(
            &profile,
            window_size.width.max(1),
            window_size.height.max(1),
            scale_factor,
        );
        let frame_builder = build_terminal_frame_builder(
            &profile,
            size_info,
            scale_factor,
            self.device.limits().max_texture_dimension_2d,
            self.color_theme,
        )?;

        self.presenter
            .frame_renderer_mut()
            .replace_frame_builder(frame_builder);
        self.profile = profile;
        self.scale_factor = scale_factor;
        self.size_info = size_info;
        self.rebuild_tab_bar_surface();

        for plugin in &mut self.render_plugins {
            let Some(placement) = self
                .workspace_layout
                .iter()
                .find(|placement| placement.target_id == plugin.target_id())
                .copied()
            else {
                continue;
            };
            let _ = plugin.resize(WgpuPaneResizeEvent {
                placement,
                scale_factor: scale_factor.value(),
            });
        }

        self.schedule_redraw();
        Ok(self.terminal_size_info())
    }

    pub fn schedule_redraw(&mut self) {
        self.needs_redraw = true;
    }

    pub fn set_window_occluded(&mut self, occluded: bool) {
        if self.window_occluded == occluded {
            return;
        }

        self.window_occluded = occluded;
        if occluded {
            self.next_background_frame_at = None;
            return;
        }

        let now = Instant::now();
        self.next_present_at = now;
        self.next_background_frame_at = self.background_shader_animated.then_some(now);
        self.next_cursor_motion_frame_at = (!self.cursor_motions.is_empty()).then_some(now);
        self.schedule_redraw();
    }

    pub fn refresh_display_timing(&mut self) -> bool {
        let Some(refresh_rate_millihertz) = display_refresh_rate_millihertz(self.window.as_ref())
        else {
            return false;
        };
        if self.display_refresh_rate_millihertz == Some(refresh_rate_millihertz) {
            return false;
        }

        self.display_refresh_rate_millihertz = Some(refresh_rate_millihertz);
        self.frame_interval = frame_interval_for_refresh_rate(Some(refresh_rate_millihertz));
        self.next_present_at = Instant::now();
        true
    }

    pub fn take_redraw_request(&mut self) -> bool {
        if self.window_occluded || !self.needs_redraw || Instant::now() < self.next_present_at {
            return false;
        }
        self.needs_redraw = false;
        true
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
                self.scale_factor,
            ))
    }

    pub fn render(&mut self) {
        if self.window_occluded {
            return;
        }

        if self.display_refresh_rate_millihertz.is_none() {
            self.refresh_display_timing();
        }
        let now = Instant::now();
        let cursor_motion_positions = self
            .cursor_motions
            .iter()
            .filter_map(|(target_id, motion)| {
                let (position, active) = motion.position_at(now);
                active.then_some((*target_id, position))
            })
            .collect::<HashMap<_, _>>();
        self.cursor_motions
            .retain(|_, motion| motion.position_at(now).1);
        let visual_bell = self.visual_bell_frame(now);
        let blinking_cursor_visible = self.blinking_cursor_frame(now);
        let terminal_background_opacity = if self.background_shader_enabled {
            0.0
        } else {
            self.background_opacity
        };
        let render_plugin_targets = self
            .render_plugins
            .iter()
            .map(WgpuPaneRenderPlugin::target_id)
            .collect::<Vec<_>>();
        let profile = &self.profile;
        let scale_factor = self.scale_factor;
        let mut surfaces =
            self.workspace_layout
                .iter()
                .filter_map(|placement| {
                    if render_plugin_targets.contains(&placement.target_id) {
                        return None;
                    }
                    let surface_snapshot = self.surface_snapshots.get(&placement.target_id)?;
                    let size_info = profile.size_info_for_window_metrics(
                        TerminalWindowMetrics::new(placement.window_size(), scale_factor),
                    );
                    let mut renderer_config = WgpuRendererConfig::from(size_info)
                        .with_color_theme(self.color_theme)
                        .with_background_opacity(terminal_background_opacity)
                        .with_blinking_cursor_visible(blinking_cursor_visible);
                    if let Some((x, y)) = cursor_motion_positions.get(&placement.target_id) {
                        renderer_config = renderer_config.with_cursor_position_cells(*x, *y);
                    }
                    Some(WgpuTerminalWorkspaceSurface {
                        render_target_plan: WgpuTerminalRenderTargetPlan::new(
                            placement.width_px,
                            placement.height_px,
                        )
                        .with_origin(placement.x_px, placement.y_px)
                        .with_load_op(WgpuTerminalLoadOp::Load),
                        surface_snapshot,
                        renderer_config,
                    })
                })
                .collect::<Vec<_>>();
        if let Some(tab_bar_surface) = self.tab_bar_surface.as_ref() {
            let size_info =
                self.terminal_size_info_for_window_size(tab_bar_surface.placement.window_size());
            surfaces.push(WgpuTerminalWorkspaceSurface {
                render_target_plan: WgpuTerminalRenderTargetPlan::new(
                    tab_bar_surface.placement.width_px,
                    tab_bar_surface.placement.height_px,
                )
                .with_origin(
                    tab_bar_surface.placement.x_px,
                    tab_bar_surface.placement.y_px,
                )
                .with_load_op(WgpuTerminalLoadOp::Load),
                surface_snapshot: &tab_bar_surface.snapshot,
                renderer_config: WgpuRendererConfig::from(size_info)
                    .with_color_theme(self.color_theme)
                    .with_background_opacity(terminal_background_opacity)
                    .with_blinking_cursor_visible(blinking_cursor_visible),
            });
        }
        let row_count = surfaces
            .iter()
            .map(|surface| surface.surface_snapshot.rows.len() as u64)
            .sum();
        let surface_target_ids = surfaces
            .iter()
            .map(|surface| surface.surface_snapshot.target_id)
            .collect::<Vec<_>>();
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
                workspace_layout: &self.workspace_layout,
                render_plugins: &mut self.render_plugins,
                color_format: self.surface_config.format,
                width_px: self.surface_config.width,
                height_px: self.surface_config.height,
                scale_factor: self.scale_factor.value(),
                elapsed: self.started_at.elapsed(),
                background_opacity: self.background_opacity,
                visual_bell,
                clear_color: if self.background_shader_enabled || self.background_opacity < 1.0 {
                    WgpuTerminalClearColor::transparent()
                } else {
                    WgpuTerminalClearColor::black()
                },
                retain_terminal_frame: self.retain_terminal_frame,
            }) {
            Ok(result) => {
                if result.completed() {
                    for target_id in surface_target_ids {
                        self.pending_surface_dirty_rows.remove(&target_id);
                    }
                }
                if result.plugin_redraw_requested {
                    self.schedule_redraw();
                }
                self.perf.record_frame(row_count, run_count, &result);
            }
            Err(error) => {
                self.perf.record_error(error);
                self.handle_present_error(error);
            }
        }
        self.next_present_at = now + self.frame_interval;
        self.next_background_frame_at = self
            .background_shader_animated
            .then_some(now + self.frame_interval);
        self.next_cursor_motion_frame_at =
            (!self.cursor_motions.is_empty()).then_some(now + self.frame_interval);
    }

    pub fn next_render_deadline(&self) -> Option<Instant> {
        if self.window_occluded {
            return None;
        }

        [
            self.next_cursor_blink_at,
            self.next_cursor_motion_frame_at,
            self.next_background_frame_at,
            self.needs_redraw.then_some(self.next_present_at),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn take_due_render_deadline(&mut self, now: Instant) -> bool {
        if self.window_occluded {
            return false;
        }

        let cursor_due = self
            .next_cursor_blink_at
            .is_some_and(|deadline| deadline <= now);
        if cursor_due {
            self.next_cursor_blink_at = Some(now + self.cursor_blink_interval);
        }

        let cursor_motion_due = self
            .next_cursor_motion_frame_at
            .is_some_and(|deadline| deadline <= now);
        if cursor_motion_due {
            self.next_cursor_motion_frame_at = Some(now + self.frame_interval);
        }

        let background_due = self
            .next_background_frame_at
            .is_some_and(|deadline| deadline <= now);
        if background_due {
            self.next_background_frame_at = Some(now + self.frame_interval);
        }

        let redraw_due = self.needs_redraw && self.next_present_at <= now;
        cursor_due || cursor_motion_due || background_due || redraw_due
    }

    fn update_cursor_motion(&mut self, snapshot: &RenderSurfaceSnapshot) {
        let target_id = snapshot.target_id;
        let previous = self
            .surface_snapshots
            .get(&target_id)
            .and_then(|snapshot| snapshot.cursor);
        let next = snapshot.cursor;
        let Some((previous, next)) = previous.zip(next) else {
            self.cursor_motions.remove(&target_id);
            return;
        };
        if self.cursor_motion_duration.is_zero()
            || !previous.focused
            || !next.focused
            || previous.shape == RenderSurfaceCursorShape::Hidden
            || next.shape == RenderSurfaceCursorShape::Hidden
        {
            self.cursor_motions.remove(&target_id);
            return;
        }
        if previous.x == next.x && previous.y == next.y {
            return;
        }
        let motion_enabled = if is_enter_cursor_motion(previous, next) {
            self.cursor_motion_on_enter
        } else {
            self.cursor_motion_on_input
        };
        if !motion_enabled {
            self.cursor_motions.remove(&target_id);
            return;
        }

        let now = Instant::now();
        let motion = build_cursor_motion(
            self.cursor_motions.get(&target_id).copied(),
            previous,
            next,
            now,
            self.cursor_motion_duration,
            self.frame_interval,
        );
        self.cursor_motions.insert(target_id, motion);
    }

    fn blinking_cursor_frame(&mut self, now: Instant) -> bool {
        let signature = self.workspace_layout.iter().find_map(|placement| {
            let cursor = self.surface_snapshots.get(&placement.target_id)?.cursor?;
            (cursor.focused && cursor.blinking && cursor.shape != RenderSurfaceCursorShape::Hidden)
                .then_some(CursorBlinkSignature {
                    target_id: placement.target_id,
                    x: cursor.x,
                    y: cursor.y,
                    shape: cursor.shape,
                })
        });

        let Some(signature) = signature else {
            self.cursor_blink_signature = None;
            self.next_cursor_blink_at = None;
            return true;
        };

        if self.cursor_blink_signature != Some(signature) {
            self.cursor_blink_signature = Some(signature);
            self.cursor_blink_epoch = now;
        }

        let (visible, next_deadline) =
            cursor_blink_phase(self.cursor_blink_epoch, now, self.cursor_blink_interval);
        self.next_cursor_blink_at = Some(next_deadline);

        visible
    }

    fn visual_bell_frame(&mut self, now: Instant) -> Option<WgpuVisualBellFrame> {
        let until = self.visual_bell_until?;
        if now >= until {
            self.visual_bell_until = None;
            return None;
        }

        self.schedule_redraw();
        Some(WgpuVisualBellFrame::new(
            self.surface_config.width,
            self.surface_config.height,
        ))
    }

    fn handle_present_error(&mut self, error: WgpuTerminalSurfaceFramePresentError) {
        match error {
            WgpuTerminalSurfaceFramePresentError::Outdated
            | WgpuTerminalSurfaceFramePresentError::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                self.schedule_redraw();
            }
            WgpuTerminalSurfaceFramePresentError::Timeout
            | WgpuTerminalSurfaceFramePresentError::Occluded
            | WgpuTerminalSurfaceFramePresentError::Validation => {}
        }
    }
}

fn render_target_is_visible(
    placements: &[RenderSurfacePlacement],
    target_id: RenderTargetId,
) -> bool {
    placements
        .iter()
        .any(|placement| placement.target_id == target_id)
}

fn cursor_blink_phase(epoch: Instant, now: Instant, interval: Duration) -> (bool, Instant) {
    let elapsed = now.saturating_duration_since(epoch);
    let interval_nanos = interval.as_nanos().max(1);
    let phase = elapsed.as_nanos() / interval_nanos;
    let phase_elapsed_nanos = elapsed.as_nanos() % interval_nanos;
    let remaining_nanos = interval_nanos.saturating_sub(phase_elapsed_nanos);
    let remaining = Duration::from_nanos(u64::try_from(remaining_nanos).unwrap_or(u64::MAX));

    (phase.is_multiple_of(2), now + remaining)
}

const TAB_BAR_RENDER_TARGET_ID: RenderTargetId = RenderTargetId::new(u64::MAX);
const TAB_BAR_LEFT_EDGE: &str = "";
const TAB_BAR_RIGHT_EDGE: &str = "";
const TAB_BAR_OUTER_MARGIN: u32 = 1;
const TAB_BAR_TAB_GAP: u32 = 1;
const TAB_BAR_TITLE_PADDING: u32 = 2;

struct WgpuTabBarSurface {
    placement: RenderSurfacePlacement,
    snapshot: RenderSurfaceSnapshot,
}

#[derive(Debug, Clone, Copy)]
struct TabBarPalette {
    background: RgbColorDto,
    inactive_foreground: RgbColorDto,
    inactive_background: RgbColorDto,
    active_background: RgbColorDto,
    active_foreground: RgbColorDto,
}

fn build_tab_bar_surface(
    tab_bar: &TabBarSnapshot,
    window_size_info: TerminalSizeInfo,
    color_theme: TerminalColorTheme,
) -> Option<WgpuTabBarSurface> {
    if tab_bar.titles.len() < 2 || tab_bar.active_tab_index >= tab_bar.titles.len() {
        return None;
    }

    let window_size = window_size_info.window_size();
    let bar_height_px = window_size_info
        .cell_size()
        .height_px()
        .min(window_size.height_px().saturating_sub(1));
    if bar_height_px == 0 {
        return None;
    }
    let y_px = match tab_bar.position {
        TabBarPosition::Top => 0,
        TabBarPosition::Bottom => window_size.height_px().saturating_sub(bar_height_px),
    };
    let columns = (window_size.width_px() / window_size_info.cell_size().width_px().max(1)).max(1);
    let palette = tab_bar_palette(color_theme);
    let tab_count = tab_bar.titles.len() as u32;
    let max_tab_width = columns
        .saturating_sub(TAB_BAR_OUTER_MARGIN.saturating_mul(2))
        .checked_div(tab_count)
        .unwrap_or(0)
        .max(1);
    let mut x = TAB_BAR_OUTER_MARGIN.min(columns);
    let mut runs = tab_bar_texture_runs(columns, palette);
    runs.reserve(tab_bar.titles.len().saturating_mul(3));

    for (index, title) in tab_bar.titles.iter().enumerate() {
        let active = index == tab_bar.active_tab_index;
        let progress = tab_bar.progresses.get(index).copied().flatten();
        let progress_label = progress.map(tab_progress_label).unwrap_or_default();
        let progress_width =
            terminal_text_cell_width(&progress_label).saturating_add(u32::from(progress.is_some()));
        let edge_width = u32::from(active).saturating_mul(2);
        let reserved_width = edge_width
            .saturating_add(TAB_BAR_TITLE_PADDING.saturating_mul(2))
            .saturating_add(progress_width)
            .saturating_add(TAB_BAR_TAB_GAP);
        let available_width = columns.saturating_sub(x);
        let title_budget = max_tab_width
            .min(available_width)
            .saturating_sub(reserved_width);
        if title_budget == 0 {
            continue;
        }

        let title = truncate_title_to_cells(title, title_budget);
        let content = format!("  {title}");
        let content_width = terminal_text_cell_width(&content);
        let tab_background = if active {
            palette.active_background
        } else {
            palette.inactive_background
        };

        if active {
            runs.push(RenderSurfaceRunSnapshot {
                x,
                text: TAB_BAR_LEFT_EDGE.to_string(),
                style: tab_bar_style(palette.active_background, palette.background, false),
                decoration: Default::default(),
            });
            x = x.saturating_add(1);
        }

        runs.push(RenderSurfaceRunSnapshot {
            x,
            text: content,
            style: if active {
                tab_bar_style(palette.active_foreground, palette.active_background, true)
            } else {
                tab_bar_style(
                    palette.inactive_foreground,
                    palette.inactive_background,
                    false,
                )
            },
            decoration: Default::default(),
        });
        x = x.saturating_add(content_width);

        if let Some(progress) = progress {
            let text = format!(" {progress_label}");
            let text_width = terminal_text_cell_width(&text);
            runs.push(RenderSurfaceRunSnapshot {
                x,
                text,
                style: tab_bar_style(
                    tab_progress_color(progress, color_theme),
                    tab_background,
                    true,
                ),
                decoration: Default::default(),
            });
            x = x.saturating_add(text_width);
        }

        runs.push(RenderSurfaceRunSnapshot {
            x,
            text: "  ".to_owned(),
            style: tab_bar_style(
                if active {
                    palette.active_foreground
                } else {
                    palette.inactive_foreground
                },
                tab_background,
                active,
            ),
            decoration: Default::default(),
        });
        x = x.saturating_add(TAB_BAR_TITLE_PADDING);

        if active {
            runs.push(RenderSurfaceRunSnapshot {
                x,
                text: TAB_BAR_RIGHT_EDGE.to_string(),
                style: tab_bar_style(palette.active_background, palette.background, false),
                decoration: Default::default(),
            });
            x = x.saturating_add(1);
        }

        x = x.saturating_add(TAB_BAR_TAB_GAP);
    }

    Some(WgpuTabBarSurface {
        placement: RenderSurfacePlacement::new(
            TAB_BAR_RENDER_TARGET_ID,
            0,
            y_px,
            window_size.width_px(),
            bar_height_px,
        ),
        snapshot: RenderSurfaceSnapshot {
            target_id: TAB_BAR_RENDER_TARGET_ID,
            latest_seq: Seq::new(0),
            default_background: palette.background,
            rows: vec![RenderSurfaceRowSnapshot { y: 0, runs }],
            video_surfaces: Vec::new(),
            image_surfaces: Vec::new(),
            dirty_rows: Vec::new(),
            cursor: None,
            ime_preedit: None,
        },
    })
}

fn tab_progress_label(progress: TerminalProgress) -> String {
    match progress {
        TerminalProgress::Normal(percentage) => format!("{percentage}%"),
        TerminalProgress::Error(percentage) => format!("×{percentage}%"),
        TerminalProgress::Indeterminate => "…".to_owned(),
        TerminalProgress::Warning(percentage) => format!("!{percentage}%"),
    }
}

fn tab_progress_color(progress: TerminalProgress, color_theme: TerminalColorTheme) -> RgbColorDto {
    match progress {
        TerminalProgress::Normal(_) => color_theme.palette[14],
        TerminalProgress::Error(_) => color_theme.palette[9],
        TerminalProgress::Indeterminate => color_theme.palette[13],
        TerminalProgress::Warning(_) => color_theme.palette[11],
    }
}

fn tab_bar_style(foreground: RgbColorDto, background: RgbColorDto, bold: bool) -> TextStyleDto {
    TextStyleDto {
        foreground: Some(foreground),
        background: Some(background),
        bold,
        italic: false,
        underline: false,
    }
}

fn tab_bar_palette(color_theme: TerminalColorTheme) -> TabBarPalette {
    TabBarPalette {
        background: color_theme.tab_bar_background,
        inactive_foreground: color_theme.inactive_tab_foreground,
        inactive_background: color_theme.inactive_tab_background,
        active_background: color_theme.active_tab_background,
        active_foreground: color_theme.active_tab_foreground,
    }
}

fn tab_bar_texture_runs(columns: u32, palette: TabBarPalette) -> Vec<RenderSurfaceRunSnapshot> {
    const TEXTURE_WIDTH: u32 = 2;
    const TEXTURE_STRENGTH: [u16; 12] = [1, 3, 1, 2, 0, 2, 1, 3, 0, 1, 2, 1];

    (0..columns)
        .step_by(TEXTURE_WIDTH as usize)
        .map(|x| {
            let width = TEXTURE_WIDTH.min(columns.saturating_sub(x));
            let strength = TEXTURE_STRENGTH[(x / TEXTURE_WIDTH) as usize % TEXTURE_STRENGTH.len()];
            let color = mix_rgb(
                palette.background,
                contrasting_color(palette.background),
                strength,
                255,
            );
            RenderSurfaceRunSnapshot {
                x,
                text: " ".repeat(width as usize),
                style: tab_bar_style(color, color, false),
                decoration: Default::default(),
            }
        })
        .collect()
}

fn contrasting_color(color: RgbColorDto) -> RgbColorDto {
    let luminance =
        u32::from(color.red) * 299 + u32::from(color.green) * 587 + u32::from(color.blue) * 114;
    if luminance < 140_000 {
        RgbColorDto::new(255, 255, 255)
    } else {
        RgbColorDto::new(0, 0, 0)
    }
}

fn mix_rgb(from: RgbColorDto, to: RgbColorDto, amount: u16, total: u16) -> RgbColorDto {
    fn mix_channel(from: u8, to: u8, amount: u16, total: u16) -> u8 {
        let inverse = total.saturating_sub(amount);
        ((u32::from(from) * u32::from(inverse) + u32::from(to) * u32::from(amount))
            / u32::from(total.max(1))) as u8
    }

    RgbColorDto::new(
        mix_channel(from.red, to.red, amount, total),
        mix_channel(from.green, to.green, amount, total),
        mix_channel(from.blue, to.blue, amount, total),
    )
}

fn truncate_title_to_cells(title: &str, max_width: u32) -> String {
    let mut width: u32 = 0;
    title
        .chars()
        .take_while(|character| {
            let character_width = terminal_char_cell_width(*character);
            if width.saturating_add(character_width) > max_width {
                return false;
            }
            width = width.saturating_add(character_width);
            true
        })
        .collect()
}

fn build_terminal_frame_builder(
    profile: &TerminalProfile,
    size_info: TerminalSizeInfo,
    scale_factor: TerminalScaleFactor,
    max_texture_dimension_2d: u32,
    color_theme: TerminalColorTheme,
) -> Result<WgpuTerminalFrameBuilder, WindowRuntimeError> {
    let base = WgpuTerminalFrameBuilder::new(
        WgpuRendererConfig::from(size_info).with_color_theme(color_theme),
    );

    let glyph_config = profile.glyph_render_config(size_info, scale_factor);
    let terminal_cell_size = glyph_config.cell_size();
    let crossfont_builder = WgpuCrossfontGlyphAtlasBuilder::from_terminal_font_config(
        glyph_config.font_config(),
        glyph_config.font_size_px(),
    )
    .map_err(WindowRuntimeError::BuildGlyphAtlas)?
    .with_padding_px(2)
    .with_columns(16)
    .with_max_texture_dimension_2d(max_texture_dimension_2d)
    .with_cell_size_px(
        terminal_cell_size.width_px(),
        terminal_cell_size.height_px(),
    );

    Ok(base.with_crossfont_glyph_atlas_builder(crossfont_builder))
}

fn terminal_profile_from_alacritty_crossfont_metrics(
    profile: TerminalProfile,
    scale_factor: TerminalScaleFactor,
) -> Result<TerminalProfile, WindowRuntimeError> {
    let font_px = profile.font_physical_px(scale_factor);
    let metrics = WgpuCrossfontGlyphAtlasBuilder::load_cell_metrics_for_font_config(
        profile.font_config(),
        font_px,
    )
    .map_err(WindowRuntimeError::LoadCrossfontMetrics)?;

    Ok(profile.with_cell_size(TerminalCellSize::new(
        metrics.cell_width_px(),
        metrics.cell_height_px(),
    )))
}

fn terminal_size_info(
    profile: &TerminalProfile,
    width: u32,
    height: u32,
    scale_factor: TerminalScaleFactor,
) -> TerminalSizeInfo {
    profile.size_info_for_window_metrics(TerminalWindowMetrics::new(
        TerminalWindowSize::new(width, height),
        scale_factor,
    ))
}

const RENDER_PERF_LOG_INTERVAL: Duration = Duration::from_secs(1);
const RENDER_PERF_LOG_ENV: &str = "GERMINAL_RENDER_PERF_LOG";

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

    fn schedule_redraw(&mut self) {
        WgpuTerminalWindowRuntime::schedule_redraw(self)
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

    fn set_tab_bar(&mut self, tab_bar: Option<TabBarSnapshot>) {
        WgpuTerminalWindowRuntime::set_tab_bar(self, tab_bar);
    }

    fn set_window_title(&mut self, title: &str) {
        WgpuTerminalWindowRuntime::set_window_title(self, title);
    }

    fn ring_bell(&mut self, visual_duration: Duration, request_attention: bool) {
        WgpuTerminalWindowRuntime::ring_bell(self, visual_duration, request_attention);
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

    use germinal_ports::{
        pty_host::{
            cell_size::TerminalCellSize,
            color_theme::TerminalColorTheme,
            size_info::{TerminalPadding, TerminalSizeInfo},
            terminal_progress::TerminalProgress,
            window_size::TerminalWindowSize,
        },
        rendering::{
            frame_plan_builder::RgbColorDto,
            render_target_id::RenderTargetId,
            surface_snapshot::{
                RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot,
                RenderSurfaceImePreeditSnapshot, RenderSurfaceSnapshot,
            },
            tab_bar::{TabBarPosition, TabBarSnapshot},
            workspace_layout::RenderSurfacePlacement,
        },
        seq::Seq,
    };
    use winit::dpi::{PhysicalPosition, PhysicalSize};

    use super::{
        CursorMotion, TAB_BAR_LEFT_EDGE, TAB_BAR_RIGHT_EDGE, WgpuTerminalPowerPreference,
        accumulate_pending_surface_damage, build_cursor_motion, build_tab_bar_surface,
        cursor_blink_phase, frame_interval_for_refresh_rate, ime_cursor_area,
        is_enter_cursor_motion, normalized_opacity, render_target_is_visible,
        terminal_power_preference_for_device_types, terminal_wgpu_backends,
        transparent_surface_alpha_mode,
    };

    #[test]
    fn render_target_visibility_follows_workspace_layout() {
        let visible_target = RenderTargetId::new(1);
        let hidden_target = RenderTargetId::new(2);
        let placements = [RenderSurfacePlacement::new(visible_target, 0, 0, 640, 480)];

        assert!(render_target_is_visible(&placements, visible_target));
        assert!(!render_target_is_visible(&placements, hidden_target));
    }

    #[test]
    fn cursor_motion_uses_time_based_easing_and_finishes_exactly_at_target() {
        let started_at = Instant::now();
        let motion = CursorMotion {
            from_x: 0.0,
            from_y: 2.0,
            to_x: 4.0,
            to_y: 6.0,
            started_at,
            duration: Duration::from_millis(80),
            waiting_for_line_feed: false,
        };

        assert_eq!(motion.position_at(started_at), ((0.0, 2.0), true));
        let (middle, active) = motion.position_at(started_at + Duration::from_millis(40));
        assert!(active);
        assert_eq!(middle, (3.5, 5.5));
        assert_eq!(
            motion.position_at(started_at + Duration::from_millis(80)),
            ((4.0, 6.0), false)
        );
    }

    #[test]
    fn carriage_return_and_line_feed_share_one_diagonal_cursor_motion() {
        let cursor = |x, y| RenderSurfaceCursorSnapshot {
            x,
            y,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        };
        let started_at = Instant::now();
        let frame_interval = Duration::from_millis(6);
        let duration = Duration::from_millis(80);
        let carriage_return = build_cursor_motion(
            None,
            cursor(12, 4),
            cursor(0, 4),
            started_at,
            duration,
            frame_interval,
        );
        assert!(carriage_return.waiting_for_line_feed);
        assert_eq!(carriage_return.position_at(started_at).0, (12.0, 4.0));

        let combined = build_cursor_motion(
            Some(carriage_return),
            cursor(0, 4),
            cursor(0, 5),
            started_at + Duration::from_millis(1),
            duration,
            frame_interval,
        );
        assert!(!combined.waiting_for_line_feed);
        assert_eq!((combined.from_x, combined.from_y), (12.0, 4.0));
        assert_eq!((combined.to_x, combined.to_y), (0.0, 5.0));
        assert_eq!(combined.started_at, carriage_return.started_at);
    }

    #[test]
    fn input_and_enter_cursor_motions_are_classified_independently() {
        let cursor = |x, y| RenderSurfaceCursorSnapshot {
            x,
            y,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        };

        assert!(!is_enter_cursor_motion(cursor(4, 2), cursor(5, 2)));
        assert!(!is_enter_cursor_motion(cursor(5, 2), cursor(4, 2)));
        assert!(is_enter_cursor_motion(cursor(4, 2), cursor(0, 2)));
        assert!(is_enter_cursor_motion(cursor(0, 2), cursor(0, 3)));
        assert!(is_enter_cursor_motion(cursor(4, 2), cursor(0, 3)));
    }

    #[test]
    fn standalone_carriage_return_waits_for_only_one_coalesce_deadline() {
        let cursor = |x, y| RenderSurfaceCursorSnapshot {
            x,
            y,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        };
        let observed_at = Instant::now();
        let frame_interval = Duration::from_millis(6);
        let motion = build_cursor_motion(
            None,
            cursor(12, 4),
            cursor(0, 4),
            observed_at,
            Duration::from_millis(80),
            frame_interval,
        );

        assert_eq!(motion.started_at, observed_at + frame_interval);
        assert_eq!(motion.position_at(observed_at).0, (12.0, 4.0));
        assert_eq!(motion.position_at(motion.started_at).0, (12.0, 4.0));
        assert!(
            motion
                .position_at(motion.started_at + Duration::from_millis(1))
                .0
                .0
                < 12.0
        );
    }

    #[test]
    fn vertical_move_after_coalesce_deadline_retargets_current_motion() {
        let cursor = |x, y| RenderSurfaceCursorSnapshot {
            x,
            y,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        };
        let observed_at = Instant::now();
        let frame_interval = Duration::from_millis(6);
        let duration = Duration::from_millis(80);
        let carriage_return = build_cursor_motion(
            None,
            cursor(12, 4),
            cursor(0, 4),
            observed_at,
            duration,
            frame_interval,
        );
        let vertical_move_at = carriage_return.started_at + Duration::from_millis(1);
        let expected_origin = carriage_return.position_at(vertical_move_at).0;

        let retargeted = build_cursor_motion(
            Some(carriage_return),
            cursor(0, 4),
            cursor(0, 5),
            vertical_move_at,
            duration,
            frame_interval,
        );

        assert!(!retargeted.waiting_for_line_feed);
        assert_eq!((retargeted.from_x, retargeted.from_y), expected_origin);
        assert_ne!((retargeted.from_x, retargeted.from_y), (12.0, 4.0));
        assert_eq!((retargeted.to_x, retargeted.to_y), (0.0, 5.0));
        assert_eq!(retargeted.started_at, vertical_move_at);
    }

    #[test]
    fn pending_surface_damage_survives_snapshot_replacement_before_present() {
        let target_id = RenderTargetId::new(1);
        let snapshot = |seq, dirty_rows| RenderSurfaceSnapshot {
            target_id,
            latest_seq: Seq::new(seq),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: Vec::new(),
            video_surfaces: Vec::new(),
            image_surfaces: Vec::new(),
            dirty_rows,
            cursor: None,
            ime_preedit: None,
        };
        let mut pending = std::collections::HashMap::new();
        let mut first = snapshot(1, vec![1, 2]);
        let mut second = snapshot(2, vec![4, 5]);

        accumulate_pending_surface_damage(&mut pending, &mut first);
        accumulate_pending_surface_damage(&mut pending, &mut second);

        assert_eq!(second.dirty_rows, vec![1, 2, 4, 5]);
        assert_eq!(pending.get(&target_id), Some(&vec![1, 2, 4, 5]));
    }

    #[test]
    fn terminal_uses_primary_wgpu_backends_without_gl() {
        let backends = terminal_wgpu_backends();

        assert_eq!(backends, wgpu::Backends::PRIMARY);
        assert!(!backends.contains(wgpu::Backends::GL));
    }

    #[test]
    fn first_run_power_detection_prefers_discrete_and_falls_back_to_low_power() {
        assert_eq!(
            terminal_power_preference_for_device_types([
                wgpu::DeviceType::IntegratedGpu,
                wgpu::DeviceType::DiscreteGpu,
            ]),
            WgpuTerminalPowerPreference::High
        );
        assert_eq!(
            terminal_power_preference_for_device_types([wgpu::DeviceType::IntegratedGpu]),
            WgpuTerminalPowerPreference::Low
        );
        assert_eq!(
            terminal_power_preference_for_device_types([]),
            WgpuTerminalPowerPreference::Low
        );
    }

    #[test]
    fn frame_interval_tracks_the_display_refresh_rate() {
        assert_eq!(
            frame_interval_for_refresh_rate(Some(60_000)),
            Duration::from_nanos(16_666_666),
        );
        assert_eq!(
            frame_interval_for_refresh_rate(Some(120_000)),
            Duration::from_nanos(8_333_333),
        );
        assert_eq!(
            frame_interval_for_refresh_rate(None),
            Duration::from_nanos(16_666_666),
        );
        assert_eq!(
            frame_interval_for_refresh_rate(Some(0)),
            Duration::from_nanos(16_666_666),
        );
    }
    #[test]
    fn opacity_is_clamped_and_non_finite_values_fall_back_to_opaque() {
        assert_eq!(normalized_opacity(-0.5), 0.0);
        assert_eq!(normalized_opacity(0.75), 0.75);
        assert_eq!(normalized_opacity(1.5), 1.0);
        assert_eq!(normalized_opacity(f32::NAN), 1.0);
    }

    #[test]
    fn transparency_prefers_premultiplied_surface_alpha() {
        assert_eq!(
            transparent_surface_alpha_mode(&[
                wgpu::CompositeAlphaMode::Opaque,
                wgpu::CompositeAlphaMode::PreMultiplied,
            ]),
            wgpu::CompositeAlphaMode::PreMultiplied
        );
        assert_eq!(
            transparent_surface_alpha_mode(&[wgpu::CompositeAlphaMode::Opaque]),
            wgpu::CompositeAlphaMode::Auto
        );
    }

    #[test]
    fn cursor_blink_phase_alternates_and_reports_the_next_deadline() {
        let epoch = Instant::now();
        let interval = Duration::from_millis(750);

        assert_eq!(
            cursor_blink_phase(epoch, epoch, interval),
            (true, epoch + interval)
        );
        assert_eq!(
            cursor_blink_phase(epoch, epoch + interval, interval),
            (false, epoch + interval * 2)
        );
        assert_eq!(
            cursor_blink_phase(epoch, epoch + interval * 2, interval),
            (true, epoch + interval * 3)
        );
    }

    #[test]
    fn ime_cursor_area_tracks_wrapped_preedit_in_a_partial_height_pane() {
        let target_id = RenderTargetId::new(9);
        let placement = RenderSurfacePlacement::new(target_id, 400, 20, 32, 35);
        let size_info = TerminalSizeInfo::new(
            placement.window_size(),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let snapshot = RenderSurfaceSnapshot {
            target_id,
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![],
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
            ime_preedit: Some(RenderSurfaceImePreeditSnapshot {
                text: "你".to_string(),
                cursor_range: Some((3, 3)),
            }),
        };

        assert_eq!(
            ime_cursor_area(placement, size_info, &snapshot),
            Some((PhysicalPosition::new(416, 37), PhysicalSize::new(8, 18)))
        );
    }

    #[test]
    fn tab_bar_renders_titles_without_numeric_prefixes() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(800, 100),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let surface = build_tab_bar_surface(
            &TabBarSnapshot {
                titles: vec!["shell".to_string(), "nvim".to_string()],
                progresses: vec![None, None],
                render_target_ids: vec![RenderTargetId::new(1), RenderTargetId::new(2)],
                active_tab_index: 1,
                position: TabBarPosition::Bottom,
            },
            size_info,
            TerminalColorTheme::default(),
        )
        .expect("multiple tabs should produce a tab bar");

        assert_eq!(surface.placement.y_px, 84);
        assert_eq!(surface.placement.height_px, 16);
        let runs = &surface.snapshot.rows[0].runs;
        let rendered_text = runs.iter().map(|run| run.text.as_str()).collect::<String>();
        assert!(rendered_text.contains("shell"));
        assert!(rendered_text.contains("nvim"));
        assert!(!rendered_text.contains("1:"));
        assert!(!rendered_text.contains("2:"));
        let left_edge = runs
            .iter()
            .find(|run| run.text == TAB_BAR_LEFT_EDGE)
            .expect("active tab should have a left edge");
        let active_title = runs
            .iter()
            .find(|run| run.text.trim() == "nvim")
            .expect("active title should be rendered");
        let right_edge = runs
            .iter()
            .find(|run| run.text == TAB_BAR_RIGHT_EDGE)
            .expect("active tab should have a right edge");
        assert_eq!(left_edge.x, 11);
        assert!(active_title.style.bold);
        assert!(right_edge.x < 30, "tabs should keep their natural width");
    }

    #[test]
    fn top_tab_bar_starts_at_the_window_origin() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(800, 100),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let surface = build_tab_bar_surface(
            &TabBarSnapshot {
                titles: vec!["shell".to_string(), "nvim".to_string()],
                progresses: vec![None, None],
                render_target_ids: vec![RenderTargetId::new(1), RenderTargetId::new(2)],
                active_tab_index: 0,
                position: TabBarPosition::Top,
            },
            size_info,
            TerminalColorTheme::default(),
        )
        .expect("multiple tabs should produce a tab bar");

        assert_eq!(surface.placement.y_px, 0);
    }

    #[test]
    fn tab_bar_uses_kitty_theme_colors() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(800, 100),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let color_theme = TerminalColorTheme {
            tab_bar_background: RgbColorDto::new(18, 30, 42),
            active_tab_background: RgbColorDto::new(50, 60, 70),
            active_tab_foreground: RgbColorDto::new(240, 241, 242),
            inactive_tab_foreground: RgbColorDto::new(120, 130, 140),
            ..TerminalColorTheme::default()
        };
        let surface = build_tab_bar_surface(
            &TabBarSnapshot {
                titles: vec!["~/one".to_string(), "nvim".to_string()],
                progresses: vec![None, None],
                render_target_ids: vec![RenderTargetId::new(1), RenderTargetId::new(2)],
                active_tab_index: 1,
                position: TabBarPosition::Bottom,
            },
            size_info,
            color_theme,
        )
        .expect("multiple tabs should produce a themed tab bar");

        assert_eq!(
            surface.snapshot.default_background,
            color_theme.tab_bar_background
        );
        let active_title = surface.snapshot.rows[0]
            .runs
            .iter()
            .find(|run| run.text.trim() == "nvim")
            .expect("active title should be rendered");
        assert_eq!(
            active_title.style.foreground,
            Some(color_theme.active_tab_foreground)
        );
        assert_eq!(
            active_title.style.background,
            Some(color_theme.active_tab_background)
        );
        assert!(surface.snapshot.rows[0].runs.len() > 10);
    }

    #[test]
    fn tab_bar_renders_themed_progress_states_separately_from_titles() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(800, 100),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let color_theme = TerminalColorTheme::default();
        let surface = build_tab_bar_surface(
            &TabBarSnapshot {
                titles: vec!["cargo".to_string(), "download".to_string()],
                progresses: vec![
                    Some(TerminalProgress::Normal(42)),
                    Some(TerminalProgress::Warning(7)),
                ],
                render_target_ids: vec![RenderTargetId::new(1), RenderTargetId::new(2)],
                active_tab_index: 0,
                position: TabBarPosition::Bottom,
            },
            size_info,
            color_theme,
        )
        .expect("multiple tabs should produce a tab bar");

        let runs = &surface.snapshot.rows[0].runs;
        let normal = runs
            .iter()
            .find(|run| run.text.trim() == "42%")
            .expect("normal progress should be rendered");
        let warning = runs
            .iter()
            .find(|run| run.text.trim() == "!7%")
            .expect("warning progress should be rendered");
        assert_eq!(normal.style.foreground, Some(color_theme.palette[14]));
        assert_eq!(warning.style.foreground, Some(color_theme.palette[11]));
        assert!(runs.iter().any(|run| run.text.trim() == "cargo"));
        assert!(runs.iter().any(|run| run.text.trim() == "download"));
    }
}
