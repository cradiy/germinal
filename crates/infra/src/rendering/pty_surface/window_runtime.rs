use std::{
    collections::HashMap,
    env,
    sync::Arc,
    time::{Duration, Instant},
};

use germinal_ports::{
    pty_host::{
        cell_size::TerminalCellSize,
        font_weight::TerminalFontWeight,
        profile::TerminalProfile,
        scale_factor::TerminalScaleFactor,
        size_info::TerminalSizeInfo,
        width::{terminal_char_cell_width, terminal_text_cell_width},
        window_metrics::TerminalWindowMetrics,
        window_size::TerminalWindowSize,
    },
    rendering::{
        frame_plan_builder::{RgbColorDto, TextStyleDto},
        render_target_id::RenderTargetId,
        surface_snapshot::{
            RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot, RenderSurfaceSnapshot,
        },
        tab_bar::{TabBarPosition, TabBarSnapshot},
        window_runtime::ITerminalWindowRuntime,
        workspace_layout::RenderSurfacePlacement,
    },
    seq::Seq,
};
use thiserror::Error;
use tracing::info;
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    window::{UserAttentionType, Window, WindowId},
};

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
    tab_bar: Option<TabBarSnapshot>,
    size_info: TerminalSizeInfo,
    profile: TerminalProfile,
    needs_redraw: bool,
    visual_bell_until: Option<Instant>,
    perf: WgpuTerminalRenderPerf,
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
            self.profile.clone(),
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
        let size_info = terminal_size_info(&profile, width, height, window.scale_factor());

        let frame_builder = build_terminal_frame_builder(
            &profile,
            size_info,
            window.scale_factor(),
            device.limits().max_texture_dimension_2d,
        )?;
        let frame_renderer = WgpuTerminalFrameRenderer::new(frame_builder);
        let divider_renderer = WgpuWorkspaceDividerRenderer::new(&device, surface_config.format);
        let visual_bell_renderer = WgpuVisualBellRenderer::new(&device, surface_config.format);
        let presenter = WgpuTerminalSurfaceFramePresenter::new(
            frame_renderer,
            divider_renderer,
            visual_bell_renderer,
        );

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
            tab_bar: None,
            size_info,
            profile,
            needs_redraw: false,
            visual_bell_until: None,
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

    pub fn set_tab_bar(&mut self, tab_bar: Option<TabBarSnapshot>) {
        self.tab_bar = tab_bar;
        self.request_redraw();
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
            self.request_redraw();
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
        let visual_bell = self.visual_bell_frame(Instant::now());
        let mut surfaces = self
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
        let tab_bar_surface = self.tab_bar.as_ref().and_then(|tab_bar| {
            let terminal_background = tab_bar
                .render_target_ids
                .get(tab_bar.active_tab_index)
                .and_then(|target_id| self.surface_snapshots.get(target_id))
                .map(|snapshot| snapshot.default_background);
            build_tab_bar_surface(tab_bar, self.size_info, terminal_background)
        });
        if let Some(tab_bar_surface) = tab_bar_surface.as_ref() {
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
                renderer_config: WgpuRendererConfig::from(size_info),
            });
        }
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
                visual_bell,
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

    fn visual_bell_frame(&mut self, now: Instant) -> Option<WgpuVisualBellFrame> {
        let until = self.visual_bell_until?;
        if now >= until {
            self.visual_bell_until = None;
            return None;
        }

        self.request_redraw();
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
                self.request_redraw();
            }
            WgpuTerminalSurfaceFramePresentError::Timeout
            | WgpuTerminalSurfaceFramePresentError::Occluded
            | WgpuTerminalSurfaceFramePresentError::Validation => {}
        }
    }
}

const TAB_BAR_RENDER_TARGET_ID: RenderTargetId = RenderTargetId::new(u64::MAX);
const TAB_BAR_FALLBACK_BACKGROUND: RgbColorDto = RgbColorDto::new(30, 32, 44);
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
    active_background: RgbColorDto,
    active_foreground: RgbColorDto,
}

fn build_tab_bar_surface(
    tab_bar: &TabBarSnapshot,
    window_size_info: TerminalSizeInfo,
    terminal_background: Option<RgbColorDto>,
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
    let palette = tab_bar_palette(terminal_background.unwrap_or(TAB_BAR_FALLBACK_BACKGROUND));
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
        let edge_width = u32::from(active).saturating_mul(2);
        let reserved_width = edge_width
            .saturating_add(TAB_BAR_TITLE_PADDING.saturating_mul(2))
            .saturating_add(TAB_BAR_TAB_GAP);
        let available_width = columns.saturating_sub(x);
        let title_budget = max_tab_width
            .min(available_width)
            .saturating_sub(reserved_width);
        if title_budget == 0 {
            continue;
        }

        let title = truncate_title_to_cells(title, title_budget);
        let content = format!("  {title}  ");
        let content_width = terminal_text_cell_width(&content);

        if active {
            runs.push(RenderSurfaceRunSnapshot {
                x,
                text: TAB_BAR_LEFT_EDGE.to_string(),
                style: tab_bar_style(palette.active_background, palette.background, false),
            });
            x = x.saturating_add(1);
        }

        runs.push(RenderSurfaceRunSnapshot {
            x,
            text: content,
            style: if active {
                tab_bar_style(palette.active_foreground, palette.active_background, true)
            } else {
                tab_bar_style(palette.inactive_foreground, palette.background, false)
            },
        });
        x = x.saturating_add(content_width);

        if active {
            runs.push(RenderSurfaceRunSnapshot {
                x,
                text: TAB_BAR_RIGHT_EDGE.to_string(),
                style: tab_bar_style(palette.active_background, palette.background, false),
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

fn tab_bar_style(foreground: RgbColorDto, background: RgbColorDto, bold: bool) -> TextStyleDto {
    TextStyleDto {
        foreground: Some(foreground),
        background: Some(background),
        bold,
        italic: false,
        underline: false,
    }
}

fn tab_bar_palette(terminal_background: RgbColorDto) -> TabBarPalette {
    let contrast = contrasting_color(terminal_background);
    let background = mix_rgb(terminal_background, contrast, 12, 255);
    let active_background = mix_rgb(terminal_background, contrast, 48, 255);

    TabBarPalette {
        background,
        inactive_foreground: mix_rgb(terminal_background, contrast, 132, 255),
        active_background,
        active_foreground: mix_rgb(
            active_background,
            contrasting_color(active_background),
            220,
            255,
        ),
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
    profile: &TerminalProfile,
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
    use germinal_ports::{
        pty_host::{
            cell_size::TerminalCellSize,
            size_info::{TerminalPadding, TerminalSizeInfo},
            window_size::TerminalWindowSize,
        },
        rendering::{
            frame_plan_builder::RgbColorDto,
            render_target_id::RenderTargetId,
            surface_snapshot::{
                RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot,
                RenderSurfaceImePreeditSnapshot, RenderSurfaceSnapshot,
            },
            workspace_layout::RenderSurfacePlacement,
        },
        seq::Seq,
    };
    use winit::dpi::{PhysicalPosition, PhysicalSize};

    use super::{
        TAB_BAR_FALLBACK_BACKGROUND, TAB_BAR_LEFT_EDGE, TAB_BAR_RIGHT_EDGE, build_tab_bar_surface,
        ime_cursor_area,
    };
    use germinal_ports::rendering::tab_bar::{TabBarPosition, TabBarSnapshot};

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
                render_target_ids: vec![RenderTargetId::new(1), RenderTargetId::new(2)],
                active_tab_index: 1,
                position: TabBarPosition::Bottom,
            },
            size_info,
            None,
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
                render_target_ids: vec![RenderTargetId::new(1), RenderTargetId::new(2)],
                active_tab_index: 0,
                position: TabBarPosition::Top,
            },
            size_info,
            None,
        )
        .expect("multiple tabs should produce a tab bar");

        assert_eq!(surface.placement.y_px, 0);
    }

    #[test]
    fn tab_bar_palette_is_derived_from_the_active_terminal_background() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(800, 100),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let terminal_background = RgbColorDto::new(18, 30, 42);
        let surface = build_tab_bar_surface(
            &TabBarSnapshot {
                titles: vec!["~/one".to_string(), "nvim".to_string()],
                render_target_ids: vec![RenderTargetId::new(1), RenderTargetId::new(2)],
                active_tab_index: 1,
                position: TabBarPosition::Bottom,
            },
            size_info,
            Some(terminal_background),
        )
        .expect("multiple tabs should produce a themed tab bar");

        assert_ne!(
            surface.snapshot.default_background,
            TAB_BAR_FALLBACK_BACKGROUND
        );
        assert!(surface.snapshot.default_background.red < 40);
        assert!(surface.snapshot.default_background.green < 50);
        assert!(surface.snapshot.default_background.blue < 60);
        assert!(surface.snapshot.rows[0].runs.len() > 10);
    }
}
