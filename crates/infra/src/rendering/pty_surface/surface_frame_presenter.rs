use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

use germinal_ports::rendering::surface_snapshot::{
    RenderSurfaceRowSnapshot, RenderSurfaceSnapshot,
};

use crate::rendering::pty_surface::{
    background_shader_renderer::{WgpuBackgroundShaderFrame, WgpuBackgroundShaderRenderer},
    frame_renderer::{
        WgpuTerminalFrameRenderResult, WgpuTerminalFrameRenderer, WgpuTerminalGpuContext,
        WgpuTerminalRenderView,
    },
    pipeline_factory::WgpuTerminalPipeline,
    render_plugin::{WgpuPaneRenderContext, WgpuPaneRenderPlugin},
    render_target_plan::{WgpuTerminalClearColor, WgpuTerminalRenderTargetPlan},
    renderer_backend::WgpuRendererConfig,
    visual_bell_renderer::{WgpuVisualBellFrame, WgpuVisualBellRenderer},
    workspace_divider_renderer::WgpuWorkspaceDividerRenderer,
};

pub struct WgpuTerminalSurfaceFramePresenter {
    background_shader_renderer: Option<WgpuBackgroundShaderRenderer>,
    frame_renderer: WgpuTerminalFrameRenderer,
    divider_renderer: WgpuWorkspaceDividerRenderer,
    visual_bell_renderer: WgpuVisualBellRenderer,
    retained_frame: RefCell<Option<WgpuRetainedWorkspaceFrame>>,
}

impl WgpuTerminalSurfaceFramePresenter {
    pub fn new(
        frame_renderer: WgpuTerminalFrameRenderer,
        divider_renderer: WgpuWorkspaceDividerRenderer,
        visual_bell_renderer: WgpuVisualBellRenderer,
    ) -> Self {
        Self {
            background_shader_renderer: None,
            frame_renderer,
            divider_renderer,
            visual_bell_renderer,
            retained_frame: RefCell::new(None),
        }
    }

    pub fn with_background_shader(mut self, renderer: WgpuBackgroundShaderRenderer) -> Self {
        self.background_shader_renderer = Some(renderer);
        self
    }

    pub fn frame_renderer(&self) -> &WgpuTerminalFrameRenderer {
        &self.frame_renderer
    }

    pub fn frame_renderer_mut(&mut self) -> &mut WgpuTerminalFrameRenderer {
        &mut self.frame_renderer
    }

    pub fn present_workspace_frame(
        &self,
        input: WgpuTerminalWorkspacePresentInput<'_, '_>,
    ) -> Result<WgpuTerminalWorkspaceFramePresentResult, WgpuTerminalSurfaceFramePresentError> {
        let total_started_at = Instant::now();

        let acquire_started_at = Instant::now();
        let surface_texture = match input.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(surface_texture) => surface_texture,
            wgpu::CurrentSurfaceTexture::Suboptimal(surface_texture) => surface_texture,
            wgpu::CurrentSurfaceTexture::Timeout => {
                return Err(WgpuTerminalSurfaceFramePresentError::Timeout);
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                return Err(WgpuTerminalSurfaceFramePresentError::Occluded);
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                return Err(WgpuTerminalSurfaceFramePresentError::Outdated);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(WgpuTerminalSurfaceFramePresentError::Lost);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(WgpuTerminalSurfaceFramePresentError::Validation);
            }
        };
        let acquire_surface_texture = acquire_started_at.elapsed();

        let create_encoder_started_at = Instant::now();
        let mut command_encoder =
            input
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("germinal.terminal.command_encoder"),
                });
        let create_command_encoder = create_encoder_started_at.elapsed();

        let retained_enabled =
            retained_frame_eligible(&input, self.background_shader_renderer.is_some());
        if !retained_enabled {
            self.retained_frame.borrow_mut().take();
        }
        let mut retained_frame = retained_enabled.then(|| self.retained_frame.borrow_mut());
        let retained_action = retained_frame.as_mut().map(|cache| {
            let cache = cache.get_or_insert_with(|| {
                WgpuRetainedWorkspaceFrame::new(
                    input.device,
                    input.width_px,
                    input.height_px,
                    input.color_format,
                )
            });
            if !cache.matches(input.width_px, input.height_px, input.color_format) {
                *cache = WgpuRetainedWorkspaceFrame::new(
                    input.device,
                    input.width_px,
                    input.height_px,
                    input.color_format,
                );
            }
            retained_frame_action(cache.previous.as_ref(), &input.surfaces[0])
        });

        if let (Some(cache), Some(action)) = (retained_frame.as_mut(), retained_action) {
            prepare_retained_target(
                &mut command_encoder,
                cache.as_mut().expect("retained frame was initialized"),
                action,
                &input.surfaces[0],
            );
        }

        let create_view_started_at = Instant::now();
        let target_view = if let Some(cache) = retained_frame.as_ref() {
            cache
                .as_ref()
                .expect("retained frame was initialized")
                .current_texture()
                .create_view(&wgpu::TextureViewDescriptor::default())
        } else {
            surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let create_texture_view = create_view_started_at.elapsed();

        if retained_action == Some(WgpuRetainedFrameAction::Full) || retained_action.is_none() {
            clear_target_view(&mut command_encoder, &target_view, input.clear_color);
        }

        let background_draw_count =
            self.background_shader_renderer
                .as_ref()
                .map_or(0, |renderer| {
                    renderer.encode(
                        input.queue,
                        &mut command_encoder,
                        &target_view,
                        WgpuBackgroundShaderFrame {
                            width_px: input.width_px,
                            height_px: input.height_px,
                            elapsed_seconds: input.elapsed.as_secs_f32(),
                            opacity: input.background_opacity,
                        },
                    );
                    1
                });
        let render_to_view_started_at = Instant::now();
        let render_surfaces = input
            .surfaces
            .iter()
            .map(|surface| WgpuTerminalWorkspaceSurface {
                render_target_plan: retained_action
                    .and_then(WgpuRetainedFrameAction::damage_bounds)
                    .map_or(surface.render_target_plan, |(top, bottom)| {
                        surface.render_target_plan.with_scissor(
                            surface.render_target_plan.x_px,
                            surface.render_target_plan.y_px.saturating_add(top),
                            surface.render_target_plan.width_px,
                            bottom.saturating_sub(top),
                        )
                    }),
                surface_snapshot: surface.surface_snapshot,
                renderer_config: surface.renderer_config,
            })
            .collect::<Vec<_>>();
        let render_results = render_surfaces
            .iter()
            .filter(|_| retained_action != Some(WgpuRetainedFrameAction::Reuse))
            .map(|surface| {
                self.frame_renderer.render_to_view(
                    WgpuTerminalGpuContext {
                        device: input.device,
                        queue: input.queue,
                    },
                    WgpuTerminalRenderView {
                        command_encoder: &mut command_encoder,
                        target_view: &target_view,
                        render_target_plan: surface.render_target_plan,
                        pipeline: input.pipeline,
                        surface_snapshot: surface.surface_snapshot,
                        renderer_config: surface.renderer_config,
                    },
                )
            })
            .collect();
        let mut plugin_draw_count = 0;
        let mut plugin_redraw_requested = false;
        for plugin in input.render_plugins.iter_mut() {
            let Some(placement) = input
                .workspace_layout
                .iter()
                .find(|placement| placement.target_id == plugin.target_id())
                .copied()
            else {
                continue;
            };
            let result = plugin.render(WgpuPaneRenderContext {
                device: input.device,
                queue: input.queue,
                command_encoder: &mut command_encoder,
                target_view: &target_view,
                color_format: input.color_format,
                placement,
                scale_factor: input.scale_factor,
                elapsed: input.elapsed,
            });
            plugin_draw_count += usize::from(result.rendered);
            plugin_redraw_requested |= result.request_redraw;
        }
        let render_target_plans = input
            .workspace_layout
            .iter()
            .map(|placement| {
                WgpuTerminalRenderTargetPlan::new(placement.width_px, placement.height_px)
                    .with_origin(placement.x_px, placement.y_px)
            })
            .collect::<Vec<_>>();
        let divider_draw_count =
            self.divider_renderer
                .encode(&mut command_encoder, &target_view, &render_target_plans);
        let visual_bell_draw_count = input.visual_bell.map_or(0, |frame| {
            self.visual_bell_renderer
                .encode(&mut command_encoder, &target_view, frame)
        });
        let render_to_view = render_to_view_started_at.elapsed();

        if let Some(cache) = retained_frame.as_mut() {
            let cache = cache.as_mut().expect("retained frame was initialized");
            copy_texture(
                &mut command_encoder,
                cache.current_texture(),
                &surface_texture.texture,
                wgpu::Origin3d::ZERO,
                wgpu::Origin3d::ZERO,
                input.width_px,
                input.height_px,
            );
            cache.initialized[cache.current] = true;
            cache.previous = Some(WgpuRetainedWorkspaceSnapshot {
                snapshot: input.surfaces[0].surface_snapshot.clone(),
                renderer_config: input.surfaces[0].renderer_config,
            });
        }

        let submit_started_at = Instant::now();
        input.queue.submit(Some(command_encoder.finish()));
        let submit = submit_started_at.elapsed();

        let present_started_at = Instant::now();
        input.queue.present(surface_texture);
        let present = present_started_at.elapsed();

        Ok(WgpuTerminalWorkspaceFramePresentResult {
            render_results,
            background_draw_count,
            divider_draw_count,
            plugin_draw_count,
            plugin_redraw_requested,
            visual_bell_draw_count,
            acquired_surface_frame: true,
            submitted: true,
            presented: true,
            timings: WgpuTerminalSurfaceFrameTimings {
                acquire_surface_texture,
                create_texture_view,
                create_command_encoder,
                render_to_view,
                submit,
                present,
                total: total_started_at.elapsed(),
            },
        })
    }
}

fn clear_target_view(
    command_encoder: &mut wgpu::CommandEncoder,
    target_view: &wgpu::TextureView,
    clear_color: WgpuTerminalClearColor,
) {
    let color_attachment = Some(wgpu::RenderPassColorAttachment {
        view: target_view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(clear_color.into()),
            store: wgpu::StoreOp::Store,
        },
    });
    let color_attachments = [color_attachment];
    let render_pass = command_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("germinal.workspace.clear_pass"),
        color_attachments: &color_attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    drop(render_pass);
}

#[derive(Debug)]
struct WgpuRetainedWorkspaceFrame {
    textures: [wgpu::Texture; 2],
    initialized: [bool; 2],
    current: usize,
    width_px: u32,
    height_px: u32,
    format: wgpu::TextureFormat,
    previous: Option<WgpuRetainedWorkspaceSnapshot>,
}

impl WgpuRetainedWorkspaceFrame {
    fn new(
        device: &wgpu::Device,
        width_px: u32,
        height_px: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        let create_texture = || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("germinal.terminal.retained_frame"),
                size: wgpu::Extent3d {
                    width: width_px,
                    height: height_px,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        Self {
            textures: [create_texture(), create_texture()],
            initialized: [false; 2],
            current: 0,
            width_px,
            height_px,
            format,
            previous: None,
        }
    }

    fn matches(&self, width_px: u32, height_px: u32, format: wgpu::TextureFormat) -> bool {
        self.width_px == width_px && self.height_px == height_px && self.format == format
    }

    fn current_texture(&self) -> &wgpu::Texture {
        &self.textures[self.current]
    }
}

#[derive(Debug, Clone)]
struct WgpuRetainedWorkspaceSnapshot {
    snapshot: RenderSurfaceSnapshot,
    renderer_config: WgpuRendererConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WgpuRetainedFrameAction {
    Full,
    Reuse,
    Damage {
        top_px: u32,
        bottom_px: u32,
    },
    ScrollUp {
        rows: u32,
        preserved_rows: u32,
        top_px: u32,
        bottom_px: u32,
    },
}

impl WgpuRetainedFrameAction {
    fn damage_bounds(self) -> Option<(u32, u32)> {
        match self {
            Self::Damage { top_px, bottom_px }
            | Self::ScrollUp {
                top_px, bottom_px, ..
            } => Some((top_px, bottom_px)),
            Self::Full | Self::Reuse => None,
        }
    }
}

fn retained_frame_eligible(
    input: &WgpuTerminalWorkspacePresentInput<'_, '_>,
    has_background_shader: bool,
) -> bool {
    if !input.retain_terminal_frame
        || has_background_shader
        || input.background_opacity < 1.0
        || input.visual_bell.is_some()
        || !input.render_plugins.is_empty()
        || input.surfaces.len() != 1
        || input.workspace_layout.len() != 1
    {
        return false;
    }

    let surface = &input.surfaces[0];
    surface.render_target_plan.x_px == 0
        && surface.render_target_plan.y_px == 0
        && surface.render_target_plan.width_px == input.width_px
        && surface.render_target_plan.height_px == input.height_px
        && surface.renderer_config.background_alpha == u8::MAX
        && surface.surface_snapshot.image_surfaces.is_empty()
        && surface.surface_snapshot.video_surfaces.is_empty()
}

fn retained_frame_action(
    previous: Option<&WgpuRetainedWorkspaceSnapshot>,
    current: &WgpuTerminalWorkspaceSurface<'_>,
) -> WgpuRetainedFrameAction {
    let Some(previous) = previous else {
        return WgpuRetainedFrameAction::Full;
    };
    let snapshot = current.surface_snapshot;
    let config = current.renderer_config;
    if previous.snapshot.default_background != snapshot.default_background
        || stable_renderer_config(previous.renderer_config) != stable_renderer_config(config)
        || snapshot.dirty_rows.is_empty()
    {
        return WgpuRetainedFrameAction::Full;
    }

    if previous.snapshot.latest_seq == snapshot.latest_seq {
        if previous.snapshot == *snapshot && previous.renderer_config == config {
            return WgpuRetainedFrameAction::Reuse;
        }
        return cursor_damage_action(previous, snapshot, config);
    }

    if let Some((rows, preserved_rows)) =
        scroll_up_rows(&previous.snapshot, snapshot, config.grid_rows)
    {
        let mut top_row = preserved_rows;
        if let Some(cursor) = previous.snapshot.cursor
            && cursor.y >= rows
        {
            top_row = top_row.min(cursor.y - rows);
        }
        if let Some(cursor) = snapshot.cursor {
            top_row = top_row.min(cursor.y);
        }
        return WgpuRetainedFrameAction::ScrollUp {
            rows,
            preserved_rows,
            top_px: config.row_top_px(top_row),
            bottom_px: config
                .content_origin_y
                .saturating_add(config.content_height_px),
        };
    }

    damage_action_for_rows(config, snapshot.dirty_rows.iter().copied())
}

fn stable_renderer_config(mut config: WgpuRendererConfig) -> WgpuRendererConfig {
    config.blinking_cursor_visible = true;
    config.cursor_position_px = None;
    config
}

fn cursor_damage_action(
    previous: &WgpuRetainedWorkspaceSnapshot,
    snapshot: &RenderSurfaceSnapshot,
    config: WgpuRendererConfig,
) -> WgpuRetainedFrameAction {
    let mut rows = Vec::with_capacity(2);
    if let Some(cursor) = previous.snapshot.cursor {
        rows.push(cursor.y);
    }
    if let Some(cursor) = snapshot.cursor {
        rows.push(cursor.y);
    }
    if let Some((_, y_px)) = previous.renderer_config.cursor_position_px {
        rows.push(pixel_row(config, y_px));
    }
    if let Some((_, y_px)) = config.cursor_position_px {
        rows.push(pixel_row(config, y_px));
    }
    if rows.is_empty() {
        WgpuRetainedFrameAction::Full
    } else {
        damage_action_for_rows(config, rows)
    }
}

fn pixel_row(config: WgpuRendererConfig, y_px: u32) -> u32 {
    let relative = y_px.saturating_sub(config.content_origin_y);
    ((u64::from(relative) * u64::from(config.grid_rows.max(1)))
        / u64::from(config.content_height_px.max(1))) as u32
}

fn damage_action_for_rows(
    config: WgpuRendererConfig,
    rows: impl IntoIterator<Item = u32>,
) -> WgpuRetainedFrameAction {
    let mut rows = rows.into_iter().filter(|row| *row < config.grid_rows);
    let Some(first) = rows.next() else {
        return WgpuRetainedFrameAction::Reuse;
    };
    let (min_row, max_row) = rows.fold((first, first), |(min_row, max_row), row| {
        (min_row.min(row), max_row.max(row))
    });
    let top_px = config.row_top_px(min_row);
    let bottom_px = config
        .row_top_px(max_row)
        .saturating_add(config.row_height_px(max_row));
    if top_px <= config.content_origin_y
        && bottom_px
            >= config
                .content_origin_y
                .saturating_add(config.content_height_px)
    {
        WgpuRetainedFrameAction::Full
    } else {
        WgpuRetainedFrameAction::Damage { top_px, bottom_px }
    }
}

fn scroll_up_rows(
    previous: &RenderSurfaceSnapshot,
    current: &RenderSurfaceSnapshot,
    grid_rows: u32,
) -> Option<(u32, u32)> {
    (1..grid_rows)
        .map(|shift| {
            let preserved_rows = (0..grid_rows - shift)
                .take_while(|row| row_runs(previous, *row + shift) == row_runs(current, *row))
                .count() as u32;
            (shift, preserved_rows)
        })
        .filter(|(_, preserved_rows)| *preserved_rows >= grid_rows / 2)
        .max_by_key(|(_, preserved_rows)| *preserved_rows)
}

fn row_runs(
    snapshot: &RenderSurfaceSnapshot,
    y: u32,
) -> Option<&[germinal_ports::rendering::surface_snapshot::RenderSurfaceRunSnapshot]> {
    snapshot
        .rows
        .iter()
        .find(|row| row.y == y)
        .map(|row: &RenderSurfaceRowSnapshot| row.runs.as_slice())
}

fn prepare_retained_target(
    encoder: &mut wgpu::CommandEncoder,
    cache: &mut WgpuRetainedWorkspaceFrame,
    action: WgpuRetainedFrameAction,
    surface: &WgpuTerminalWorkspaceSurface<'_>,
) {
    let WgpuRetainedFrameAction::ScrollUp {
        rows,
        preserved_rows,
        ..
    } = action
    else {
        return;
    };
    let source = cache.current;
    let destination = 1 - source;
    if !cache.initialized[destination] {
        copy_texture(
            encoder,
            &cache.textures[source],
            &cache.textures[destination],
            wgpu::Origin3d::ZERO,
            wgpu::Origin3d::ZERO,
            cache.width_px,
            cache.height_px,
        );
        cache.initialized[destination] = true;
    }

    let config = surface.renderer_config;
    let source_y = config.row_top_px(rows);
    let destination_y = config.row_top_px(0);
    let height = config.row_offset_px(preserved_rows);
    if height > 0 {
        copy_texture(
            encoder,
            &cache.textures[source],
            &cache.textures[destination],
            wgpu::Origin3d {
                x: 0,
                y: source_y,
                z: 0,
            },
            wgpu::Origin3d {
                x: 0,
                y: destination_y,
                z: 0,
            },
            cache.width_px,
            height,
        );
    }
    cache.current = destination;
}

fn copy_texture(
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::Texture,
    destination: &wgpu::Texture,
    source_origin: wgpu::Origin3d,
    destination_origin: wgpu::Origin3d,
    width: u32,
    height: u32,
) {
    encoder.copy_texture_to_texture(
        wgpu::TexelCopyTextureInfo {
            texture: source,
            mip_level: 0,
            origin: source_origin,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyTextureInfo {
            texture: destination,
            mip_level: 0,
            origin: destination_origin,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

pub struct WgpuTerminalWorkspacePresentInput<'a, 'window> {
    pub surface: &'a wgpu::Surface<'window>,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub pipeline: &'a WgpuTerminalPipeline,
    pub surfaces: &'a [WgpuTerminalWorkspaceSurface<'a>],
    pub workspace_layout:
        &'a [germinal_ports::rendering::workspace_layout::RenderSurfacePlacement],
    pub render_plugins: &'a mut [WgpuPaneRenderPlugin],
    pub color_format: wgpu::TextureFormat,
    pub width_px: u32,
    pub height_px: u32,
    pub scale_factor: f64,
    pub elapsed: Duration,
    pub background_opacity: f32,
    pub visual_bell: Option<WgpuVisualBellFrame>,
    pub clear_color: WgpuTerminalClearColor,
    pub retain_terminal_frame: bool,
}

pub struct WgpuTerminalWorkspaceSurface<'a> {
    pub render_target_plan: WgpuTerminalRenderTargetPlan,
    pub surface_snapshot: &'a RenderSurfaceSnapshot,
    pub renderer_config: WgpuRendererConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WgpuTerminalSurfaceFrameTimings {
    pub acquire_surface_texture: Duration,
    pub create_texture_view: Duration,
    pub create_command_encoder: Duration,
    pub render_to_view: Duration,
    pub submit: Duration,
    pub present: Duration,
    pub total: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuTerminalWorkspaceFramePresentResult {
    pub render_results: Vec<WgpuTerminalFrameRenderResult>,
    pub background_draw_count: usize,
    pub divider_draw_count: usize,
    pub plugin_draw_count: usize,
    pub plugin_redraw_requested: bool,
    pub visual_bell_draw_count: usize,
    pub acquired_surface_frame: bool,
    pub submitted: bool,
    pub presented: bool,
    pub timings: WgpuTerminalSurfaceFrameTimings,
}

impl WgpuTerminalWorkspaceFramePresentResult {
    pub fn completed(&self) -> bool {
        self.acquired_surface_frame && self.submitted && self.presented
    }

    pub fn rendered(&self) -> bool {
        self.render_results
            .iter()
            .any(WgpuTerminalFrameRenderResult::rendered)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuTerminalSurfaceFramePresentError {
    Timeout,
    Occluded,
    Outdated,
    Lost,
    Validation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalSurfaceFramePresenterSpec {
    pub acquires_surface_texture: bool,
    pub creates_texture_view: bool,
    pub creates_command_encoder: bool,
    pub submits_command_buffer: bool,
    pub presents_surface_texture: bool,
}

impl WgpuTerminalSurfaceFramePresenterSpec {
    pub const fn new() -> Self {
        Self {
            acquires_surface_texture: true,
            creates_texture_view: true,
            creates_command_encoder: true,
            submits_command_buffer: true,
            presents_surface_texture: true,
        }
    }
}

impl Default for WgpuTerminalSurfaceFramePresenterSpec {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use germinal_ports::{
        pty_host::snapshot::TerminalSnapshotProvider,
        rendering::{
            frame_plan_builder::{RgbColorDto, TextStyleDto},
            render_target_id::RenderTargetId,
            surface_snapshot::{
                RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot, RenderSurfaceTextDecoration,
            },
        },
        seq::Seq,
    };

    use super::*;
    use crate::pty_host::alacritty_terminal_store::{AlacrittyTermSize, AlacrittyTerminalStore};

    fn config() -> WgpuRendererConfig {
        WgpuRendererConfig {
            content_width_px: 80,
            content_height_px: 64,
            grid_columns: 10,
            grid_rows: 4,
            ..WgpuRendererConfig::default()
        }
    }

    fn snapshot(seq: u64, lines: &[&str], dirty_rows: Vec<u32>) -> RenderSurfaceSnapshot {
        RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(seq),
            default_background: RgbColorDto::new(1, 2, 3),
            rows: lines
                .iter()
                .enumerate()
                .map(|(y, text)| RenderSurfaceRowSnapshot {
                    y: y as u32,
                    runs: vec![RenderSurfaceRunSnapshot {
                        x: 0,
                        text: (*text).to_string(),
                        style: TextStyleDto::plain(),
                        decoration: RenderSurfaceTextDecoration::default(),
                    }],
                })
                .collect(),
            video_surfaces: Vec::new(),
            image_surfaces: Vec::new(),
            dirty_rows,
            cursor: None,
            ime_preedit: None,
        }
    }

    fn action(
        previous: &WgpuRetainedWorkspaceSnapshot,
        current: &RenderSurfaceSnapshot,
    ) -> WgpuRetainedFrameAction {
        retained_frame_action(
            Some(previous),
            &WgpuTerminalWorkspaceSurface {
                render_target_plan: WgpuTerminalRenderTargetPlan::new(80, 64),
                surface_snapshot: current,
                renderer_config: config(),
            },
        )
    }

    #[test]
    fn detects_one_row_terminal_scroll_and_only_damages_exposed_row() {
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: snapshot(1, &["a", "b", "c", "d"], vec![0, 1, 2, 3]),
            renderer_config: config(),
        };
        let current = snapshot(2, &["b", "c", "d", "e"], vec![0, 1, 2, 3]);

        assert_eq!(
            action(&previous, &current),
            WgpuRetainedFrameAction::ScrollUp {
                rows: 1,
                preserved_rows: 3,
                top_px: 48,
                bottom_px: 64,
            }
        );
    }

    #[test]
    fn keeps_partial_non_scroll_damage_scissored_to_changed_rows() {
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: snapshot(1, &["a", "b", "c", "d"], vec![1, 2, 3]),
            renderer_config: config(),
        };
        let current = snapshot(2, &["a", "b", "changed", "d"], vec![1, 2, 3]);

        assert_eq!(
            action(&previous, &current),
            WgpuRetainedFrameAction::Damage {
                top_px: 16,
                bottom_px: 64,
            }
        );
    }

    #[test]
    fn reuses_retained_texture_when_frame_and_dynamic_config_are_unchanged() {
        let current = snapshot(3, &["a", "b", "c", "d"], vec![2]);
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: current.clone(),
            renderer_config: config(),
        };

        assert_eq!(action(&previous, &current), WgpuRetainedFrameAction::Reuse);
    }

    #[test]
    fn empty_damage_forces_a_full_retained_frame_rebuild() {
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: snapshot(1, &["a", "b", "c", "d"], vec![2]),
            renderer_config: config(),
        };
        let current = snapshot(2, &["a", "b", "c", "d"], Vec::new());

        assert_eq!(action(&previous, &current), WgpuRetainedFrameAction::Full);
    }

    #[test]
    fn detects_wrapped_ansi_benchmark_scroll_from_real_terminal_snapshots() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(66, 35));
        let target_id = RenderTargetId::new(9);
        for frame in 0..40 {
            let line = format!(
                "\x1b[38;2;70;170;240m{frame:08} Germinal Kitty Zellij | terminal rendering benchmark | Germinal Kitty Zellij | terminal rendering benchmark\x1b[0m\r\n"
            );
            store.apply_bytes(target_id, Seq::new(frame + 1), line.as_bytes());
        }
        let previous_snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        store.clear_damage_up_to(target_id, Seq::new(40));
        let line = "\x1b[38;2;70;170;240m00000040 Germinal Kitty Zellij | terminal rendering benchmark | Germinal Kitty Zellij | terminal rendering benchmark\x1b[0m\r\n";
        store.apply_bytes(target_id, Seq::new(41), line.as_bytes());
        let current = store.render_surface_snapshot_of(target_id).unwrap();
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: previous_snapshot,
            renderer_config: WgpuRendererConfig {
                content_width_px: 1271,
                content_height_px: 1523,
                grid_columns: 66,
                grid_rows: 35,
                ..WgpuRendererConfig::default()
            },
        };
        let current_config = previous.renderer_config;

        assert_eq!(current.dirty_rows.len(), 35);
        let actual = retained_frame_action(
            Some(&previous),
            &WgpuTerminalWorkspaceSurface {
                render_target_plan: WgpuTerminalRenderTargetPlan::new(1271, 1523),
                surface_snapshot: &current,
                renderer_config: current_config,
            },
        );
        assert!(
            matches!(actual, WgpuRetainedFrameAction::ScrollUp { rows: 2, .. }),
            "expected two-row scroll, got {actual:?}; detected={:?}",
            scroll_up_rows(&previous.snapshot, &current, current_config.grid_rows),
        );
    }
}
