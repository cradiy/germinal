use std::{
    cell::RefCell,
    time::{Duration, Instant},
};

use germinal_ports::pty_host::hyperlink::TerminalHyperlink;
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
    renderer_backend::{WgpuRendererConfig, WgpuRendererDamage},
    visual_bell_renderer::{WgpuVisualBellFrame, WgpuVisualBellRenderer},
    workspace_divider_renderer::WgpuWorkspaceDividerRenderer,
};

pub struct WgpuTerminalSurfaceFramePresenter {
    background_shader_renderer: Option<WgpuBackgroundShaderRenderer>,
    frame_renderer: WgpuTerminalFrameRenderer,
    divider_renderer: WgpuWorkspaceDividerRenderer,
    visual_bell_renderer: WgpuVisualBellRenderer,
    retained_frame: RefCell<Option<WgpuRetainedWorkspaceFrame>>,
    palette_chrome: RefCell<Option<super::palette_chrome::PaletteChrome>>,
    palette_frame_renderer: Option<WgpuTerminalFrameRenderer>,
    sidebar_frame_renderer: Option<WgpuTerminalFrameRenderer>,
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
            palette_chrome: RefCell::new(None),
            palette_frame_renderer: None,
            sidebar_frame_renderer: None,
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

    pub(super) fn set_palette_frame_renderer(
        &mut self,
        renderer: Option<WgpuTerminalFrameRenderer>,
    ) {
        self.palette_frame_renderer = renderer;
    }

    pub(super) fn set_sidebar_frame_renderer(
        &mut self,
        renderer: Option<WgpuTerminalFrameRenderer>,
    ) {
        self.sidebar_frame_renderer = renderer;
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
                            elapsed_seconds: input.background_elapsed.as_secs_f32(),
                            opacity: input.background_opacity,
                        },
                    );
                    1
                });
        let render_to_view_started_at = Instant::now();
        let damage = retained_action.and_then(WgpuRetainedFrameAction::renderer_damage);
        let vertical_bounds = damage.map(WgpuRendererDamage::vertical_bounds);
        let mut render_results: Vec<_> = input
            .surfaces
            .iter()
            .filter(|_| retained_action != Some(WgpuRetainedFrameAction::Reuse))
            .filter(|surface| {
                surface.surface_snapshot.target_id
                    != germinal_ports::rendering::command_palette::COMMAND_PALETTE_TARGET_ID
                    && surface.surface_snapshot.target_id
                        != germinal_ports::rendering::tab_bar::SIDEBAR_RENDER_TARGET_ID
                    && surface.surface_snapshot.target_id
                        != germinal_ports::rendering::toolbar::TOOLBAR_TARGET_ID
                    && surface.surface_snapshot.target_id
                        != germinal_ports::rendering::tab_bar::TAB_BAR_RENDER_TARGET_ID
            })
            .map(|surface| {
                let render_target_plan =
                    vertical_bounds.map_or(surface.render_target_plan, |(top_px, bottom_px)| {
                        surface.render_target_plan.with_scissor(
                            surface.render_target_plan.x_px,
                            surface.render_target_plan.y_px.saturating_add(top_px),
                            surface.render_target_plan.width_px,
                            bottom_px.saturating_sub(top_px),
                        )
                    });
                self.frame_renderer.render_to_view(
                    WgpuTerminalGpuContext {
                        device: input.device,
                        queue: input.queue,
                    },
                    WgpuTerminalRenderView {
                        command_encoder: &mut command_encoder,
                        target_view: &target_view,
                        render_target_plan,
                        pipeline: input.pipeline,
                        surface_snapshot: surface.surface_snapshot,
                        renderer_config: surface.renderer_config,
                        damage,
                        hyperlink_highlights: surface.hyperlink_highlights,
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
        for surface in input.surfaces.iter().filter(|surface| {
            surface.surface_snapshot.target_id
                == germinal_ports::rendering::command_palette::COMMAND_PALETTE_TARGET_ID
                || surface.surface_snapshot.target_id
                    == germinal_ports::rendering::tab_bar::SIDEBAR_RENDER_TARGET_ID
                || surface.surface_snapshot.target_id
                    == germinal_ports::rendering::toolbar::TOOLBAR_TARGET_ID
                || surface.surface_snapshot.target_id
                    == germinal_ports::rendering::tab_bar::TAB_BAR_RENDER_TARGET_ID
        }) {
            let toolbar = surface.surface_snapshot.target_id
                == germinal_ports::rendering::toolbar::TOOLBAR_TARGET_ID;
            let tabs = surface.surface_snapshot.target_id
                == germinal_ports::rendering::tab_bar::TAB_BAR_RENDER_TARGET_ID;
            if !toolbar && !tabs {
                self.palette_chrome
                    .borrow_mut()
                    .get_or_insert_with(|| {
                        super::palette_chrome::PaletteChrome::new(input.device, input.color_format)
                    })
                    .encode(
                        input.device,
                        &mut command_encoder,
                        &target_view,
                        [input.width_px, input.height_px],
                        surface,
                    );
            }
            render_results.push(
                (if toolbar || tabs {
                    &self.frame_renderer
                } else if surface.surface_snapshot.target_id
                    == germinal_ports::rendering::tab_bar::SIDEBAR_RENDER_TARGET_ID
                {
                    self.sidebar_frame_renderer
                        .as_ref()
                        .unwrap_or(&self.frame_renderer)
                } else {
                    self.palette_frame_renderer
                        .as_ref()
                        .unwrap_or(&self.frame_renderer)
                })
                .render_to_view(
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
                        damage: None,
                        hyperlink_highlights: &[],
                    },
                ),
            );
        }
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
                hyperlink_highlights: input.surfaces[0].hyperlink_highlights.to_vec(),
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
    hyperlink_highlights: Vec<TerminalHyperlink>,
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
    fn renderer_damage(self) -> Option<WgpuRendererDamage> {
        match self {
            Self::Damage { top_px, bottom_px } => {
                Some(WgpuRendererDamage::vertical(top_px, bottom_px))
            }
            Self::ScrollUp {
                rows,
                preserved_rows,
                top_px,
                bottom_px,
            } => Some(WgpuRendererDamage::scrolled_up(
                top_px,
                bottom_px,
                rows,
                preserved_rows,
            )),
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
        || previous.snapshot.ime_preedit != snapshot.ime_preedit
        || visible_scrollback(previous.snapshot.scrollback)
            != visible_scrollback(snapshot.scrollback)
    {
        return WgpuRetainedFrameAction::Full;
    }

    if previous.hyperlink_highlights != current.hyperlink_highlights {
        let cursor_rows = cursor_damage_rows(previous, snapshot, config);
        return damage_action_for_rows(
            config,
            snapshot
                .dirty_rows
                .iter()
                .copied()
                .chain(
                    previous
                        .hyperlink_highlights
                        .iter()
                        .chain(current.hyperlink_highlights)
                        .map(|hyperlink| hyperlink.y),
                )
                .chain(cursor_rows),
        );
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
        // The retained texture contains the animated cursor, not just text. Its copied
        // pixels need not land on the logical cursor row used by scroll damage below.
        if cursor_is_between_rows(&previous.snapshot, previous.renderer_config)
            || cursor_is_between_rows(snapshot, config)
        {
            return WgpuRetainedFrameAction::Full;
        }
        // A single texture translation must preserve every copied row boundary.
        // Fractional cell heights can distribute rounding differently after scrolling.
        let source_top = config.row_offset_px(rows);
        if (0..=preserved_rows).any(|row| {
            config.row_offset_px(row + rows).saturating_sub(source_top) != config.row_offset_px(row)
        }) {
            return WgpuRetainedFrameAction::Full;
        }
        let mut top_row = preserved_rows;
        if let Some(cursor) = previous.snapshot.cursor
            && cursor.y >= rows
        {
            top_row = top_row.min(cursor.y - rows);
        }
        if let Some(cursor) = snapshot.cursor {
            top_row = top_row.min(cursor.y);
        }
        // Hover stays in viewport coordinates even when identical links scroll
        // beneath the pointer. Repaint both copied highlights and their current rows.
        for row in previous
            .hyperlink_highlights
            .iter()
            .filter_map(|link| link.y.checked_sub(rows))
            .chain(current.hyperlink_highlights.iter().map(|link| link.y))
        {
            top_row = top_row.min(row);
        }
        if top_row == 0 {
            // No copied content survives this repaint, so avoid the texture copy
            // and row-cache translation entirely.
            return WgpuRetainedFrameAction::Full;
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

    let cursor_rows = cursor_damage_rows(previous, snapshot, config);
    damage_action_for_rows(
        config,
        snapshot.dirty_rows.iter().copied().chain(cursor_rows),
    )
}

fn cursor_is_between_rows(snapshot: &RenderSurfaceSnapshot, config: WgpuRendererConfig) -> bool {
    match (snapshot.cursor, config.cursor_position_px) {
        (Some(cursor), Some((_, y_px))) => y_px != config.row_top_px(cursor.y),
        _ => false,
    }
}

fn visible_scrollback(
    scrollback: Option<
        germinal_ports::rendering::surface_snapshot::RenderSurfaceScrollbackSnapshot,
    >,
) -> Option<germinal_ports::rendering::surface_snapshot::RenderSurfaceScrollbackSnapshot> {
    scrollback.filter(|scrollback| scrollback.display_offset > 0)
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
    let rows = cursor_damage_rows(previous, snapshot, config);
    if rows.is_empty() {
        WgpuRetainedFrameAction::Full
    } else {
        damage_action_for_rows(config, rows)
    }
}

fn cursor_damage_rows(
    previous: &WgpuRetainedWorkspaceSnapshot,
    snapshot: &RenderSurfaceSnapshot,
    config: WgpuRendererConfig,
) -> Vec<u32> {
    let mut rows = Vec::with_capacity(6);
    if let Some(cursor) = previous.snapshot.cursor {
        rows.push(cursor.y);
    }
    if let Some(cursor) = snapshot.cursor {
        rows.push(cursor.y);
    }
    if let (Some(cursor), Some((_, y_px))) = (
        previous.snapshot.cursor,
        previous.renderer_config.cursor_position_px,
    ) {
        rows.extend(cursor_pixel_rows(previous.renderer_config, cursor.y, y_px));
    }
    if let (Some(cursor), Some((_, y_px))) = (snapshot.cursor, config.cursor_position_px) {
        rows.extend(cursor_pixel_rows(config, cursor.y, y_px));
    }
    rows
}

fn cursor_pixel_rows(config: WgpuRendererConfig, cursor_row: u32, y_px: u32) -> [u32; 2] {
    let bottom_px = y_px.saturating_add(config.row_height_px(cursor_row).saturating_sub(1));
    [pixel_row(config, y_px), pixel_row(config, bottom_px)]
}

fn pixel_row(config: WgpuRendererConfig, y_px: u32) -> u32 {
    let relative = u64::from(y_px.saturating_sub(config.content_origin_y));
    let rows = u64::from(config.grid_rows.max(1));
    let height = u64::from(config.content_height_px.max(1));
    relative
        .saturating_add(1)
        .saturating_mul(rows)
        .saturating_sub(1)
        .div_euclid(height)
        .min(u64::from(u32::MAX)) as u32
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
        .filter_map(|shift| {
            let current_first = row_runs(current, 0)?;
            let previous_first = row_runs(previous, shift)?;
            if previous_first != current_first {
                return None;
            }
            let preserved_rows = 1
                + (1..grid_rows - shift)
                    .take_while(|row| row_runs(previous, *row + shift) == row_runs(current, *row))
                    .count() as u32;
            Some((shift, preserved_rows))
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
    let config = surface.renderer_config;
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
    } else {
        // The other texture may predate a full repaint (for example an OSC 11
        // background change). Scrolled rows and damage do not cover vertical padding.
        let top = config.content_origin_y.min(cache.height_px);
        let bottom = config
            .content_origin_y
            .saturating_add(config.content_height_px)
            .min(cache.height_px);
        for (y, height) in [(0, top), (bottom, cache.height_px - bottom)] {
            if height > 0 {
                let origin = wgpu::Origin3d { x: 0, y, z: 0 };
                copy_texture(
                    encoder,
                    &cache.textures[source],
                    &cache.textures[destination],
                    origin,
                    origin,
                    cache.width_px,
                    height,
                );
            }
        }
    }

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
    pub background_elapsed: Duration,
    pub background_opacity: f32,
    pub visual_bell: Option<WgpuVisualBellFrame>,
    pub clear_color: WgpuTerminalClearColor,
    pub retain_terminal_frame: bool,
}

pub struct WgpuTerminalWorkspaceSurface<'a> {
    pub render_target_plan: WgpuTerminalRenderTargetPlan,
    pub surface_snapshot: &'a RenderSurfaceSnapshot,
    pub renderer_config: WgpuRendererConfig,
    pub hyperlink_highlights: &'a [TerminalHyperlink],
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
    mod gpu_replay;

    use germinal_ports::{
        pty_host::snapshot::TerminalSnapshotProvider,
        rendering::{
            frame_plan_builder::{RgbColorDto, TextStyleDto},
            render_target_id::RenderTargetId,
            surface_snapshot::{
                RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot,
                RenderSurfaceImePreeditSnapshot, RenderSurfaceRowSnapshot,
                RenderSurfaceRunSnapshot, RenderSurfaceScrollbackSnapshot,
                RenderSurfaceTextDecoration,
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
            image_surfaces: Vec::new(),
            dirty_rows,
            scrollback: None,
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
                hyperlink_highlights: &[],
            },
        )
    }

    #[test]
    fn detects_one_row_terminal_scroll_and_only_damages_exposed_row() {
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: snapshot(1, &["a", "b", "c", "d"], vec![0, 1, 2, 3]),
            renderer_config: config(),
            hyperlink_highlights: Vec::new(),
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
    fn scroll_copy_requires_matching_source_and_destination_row_geometry() {
        for (height, lines, can_copy) in [
            (63, ["b", "c", "d", "e"], false),
            (64, ["b", "c", "d", "e"], true),
            (65, ["b", "c", "d", "e"], false),
            (66, ["b", "c", "d", "e"], false),
            (66, ["c", "d", "e", "f"], true),
        ] {
            let config = WgpuRendererConfig {
                content_height_px: height,
                ..config()
            };
            let previous = WgpuRetainedWorkspaceSnapshot {
                snapshot: snapshot(1, &["a", "b", "c", "d"], vec![0, 1, 2, 3]),
                renderer_config: config,
                hyperlink_highlights: Vec::new(),
            };
            let current = snapshot(2, &lines, vec![0, 1, 2, 3]);
            let action = retained_frame_action(
                Some(&previous),
                &WgpuTerminalWorkspaceSurface {
                    render_target_plan: WgpuTerminalRenderTargetPlan::new(80, height),
                    surface_snapshot: &current,
                    renderer_config: config,
                    hyperlink_highlights: &[],
                },
            );
            if can_copy {
                assert!(
                    matches!(action, WgpuRetainedFrameAction::ScrollUp { .. }),
                    "height={height}, lines={lines:?}: {action:?}"
                );
            } else {
                assert_eq!(
                    action,
                    WgpuRetainedFrameAction::Full,
                    "height={height}, lines={lines:?}"
                );
            }
        }
    }

    #[test]
    fn stationary_hover_damages_copied_and_current_highlights_after_scroll() {
        for (hover_y, damage_top) in [(0, 0), (1, 0), (2, 16), (3, 32)] {
            let highlights = vec![TerminalHyperlink {
                uri: "https://example.com".to_string(),
                x: 0,
                y: hover_y,
                columns: 4,
            }];
            let previous = WgpuRetainedWorkspaceSnapshot {
                snapshot: snapshot(1, &["link", "link", "link", "link"], vec![0, 1, 2, 3]),
                renderer_config: config(),
                hyperlink_highlights: highlights.clone(),
            };
            let current = snapshot(2, &["link", "link", "link", "link"], vec![0, 1, 2, 3]);
            let action = retained_frame_action(
                Some(&previous),
                &WgpuTerminalWorkspaceSurface {
                    render_target_plan: WgpuTerminalRenderTargetPlan::new(80, 64),
                    surface_snapshot: &current,
                    renderer_config: config(),
                    hyperlink_highlights: &highlights,
                },
            );
            assert_eq!(
                action,
                if damage_top == 0 {
                    WgpuRetainedFrameAction::Full
                } else {
                    WgpuRetainedFrameAction::ScrollUp {
                        rows: 1,
                        preserved_rows: 3,
                        top_px: damage_top,
                        bottom_px: 64,
                    }
                },
                "hover row {hover_y}"
            );
        }
    }

    #[test]
    fn scrolling_during_cursor_animation_redraws_instead_of_copying_cursor_pixels() {
        let mut old = snapshot(1, &["a", "b", "c", "d"], vec![0, 1, 2, 3]);
        old.cursor = Some(RenderSurfaceCursorSnapshot {
            x: 0,
            y: 3,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        });
        let mut current = snapshot(2, &["b", "c", "d", "e"], vec![0, 1, 2, 3]);
        current.cursor = old.cursor;
        for (old_y, new_y) in [(8, 48), (48, 8), (8, 24)] {
            let previous = WgpuRetainedWorkspaceSnapshot {
                snapshot: old.clone(),
                renderer_config: WgpuRendererConfig {
                    cursor_position_px: Some((0, old_y)),
                    ..config()
                },
                hyperlink_highlights: Vec::new(),
            };
            assert_eq!(
                retained_frame_action(
                    Some(&previous),
                    &WgpuTerminalWorkspaceSurface {
                        render_target_plan: WgpuTerminalRenderTargetPlan::new(80, 64),
                        surface_snapshot: &current,
                        renderer_config: WgpuRendererConfig {
                            cursor_position_px: Some((0, new_y)),
                            ..config()
                        },
                        hyperlink_highlights: &[],
                    }
                ),
                WgpuRetainedFrameAction::Full
            );
        }
        let settled = WgpuRetainedWorkspaceSnapshot {
            snapshot: old,
            renderer_config: WgpuRendererConfig {
                cursor_position_px: Some((0, 48)),
                ..config()
            },
            hyperlink_highlights: Vec::new(),
        };
        assert_eq!(
            action(&settled, &current),
            WgpuRetainedFrameAction::ScrollUp {
                rows: 1,
                preserved_rows: 3,
                top_px: 32,
                bottom_px: 64,
            }
        );
    }

    #[test]
    fn retained_cursor_mask_matches_full_redraw_over_scroll_and_animation_sequences() {
        // A CPU model of one cursor column: texture copies and scissored repainting.
        // This checks pixel coverage, not GPU rasterization or text rendering.
        for height in [10, 63, 64, 65] {
            for origin in [0, 3] {
                let base = WgpuRendererConfig {
                    content_height_px: height,
                    content_origin_y: origin,
                    ..config()
                };
                let mut current = snapshot(1, &["a", "b", "c", "d"], vec![0, 1, 2, 3]);
                current.cursor = Some(RenderSurfaceCursorSnapshot {
                    x: 0,
                    y: 3,
                    focused: true,
                    shape: RenderSurfaceCursorShape::Block,
                    blinking: true,
                });
                let mut previous = None;
                let mut retained = vec![false; (origin + height) as usize];
                for frame in 0..180 {
                    if frame % 3 == 0 {
                        current.latest_seq = Seq::new(current.latest_seq.value() + 1);
                        current.rows.rotate_left(1);
                        for (y, row) in current.rows.iter_mut().enumerate() {
                            row.y = y as u32;
                        }
                        current.rows[3].runs[0].text = format!("Compiling crate {frame}");
                    }
                    let logical_y = base.row_top_px(3);
                    let animated_y = match frame % 6 {
                        0 => origin,
                        1 => origin + height / 3,
                        2 => origin + height / 2,
                        _ => logical_y,
                    };
                    let renderer_config = WgpuRendererConfig {
                        cursor_position_px: Some((0, animated_y)),
                        blinking_cursor_visible: frame % 11 != 0,
                        ..base
                    };
                    let action = retained_frame_action(
                        previous.as_ref(),
                        &WgpuTerminalWorkspaceSurface {
                            render_target_plan: WgpuTerminalRenderTargetPlan::new(
                                80,
                                origin + height,
                            ),
                            surface_snapshot: &current,
                            renderer_config,
                            hyperlink_highlights: &[],
                        },
                    );
                    let mut reference = vec![false; retained.len()];
                    if renderer_config.blinking_cursor_visible {
                        let end = (animated_y + base.row_height_px(3)).min(origin + height);
                        reference[animated_y as usize..end as usize].fill(true);
                    }
                    let damage = match action {
                        WgpuRetainedFrameAction::Full => Some((0, origin + height)),
                        WgpuRetainedFrameAction::Reuse => None,
                        WgpuRetainedFrameAction::Damage { top_px, bottom_px } => {
                            Some((top_px, bottom_px))
                        }
                        WgpuRetainedFrameAction::ScrollUp {
                            rows,
                            preserved_rows,
                            top_px,
                            bottom_px,
                        } => {
                            let source = base.row_top_px(rows) as usize;
                            let count = base.row_offset_px(preserved_rows) as usize;
                            retained.copy_within(source..source + count, origin as usize);
                            Some((top_px, bottom_px))
                        }
                    };
                    if let Some((top, bottom)) = damage {
                        retained[top as usize..bottom as usize]
                            .copy_from_slice(&reference[top as usize..bottom as usize]);
                    }
                    assert_eq!(
                        retained, reference,
                        "height={height} origin={origin} frame={frame} action={action:?}"
                    );
                    previous = Some(WgpuRetainedWorkspaceSnapshot {
                        snapshot: current.clone(),
                        renderer_config,
                        hyperlink_highlights: Vec::new(),
                    });
                }
            }
        }
    }

    #[test]
    fn keeps_partial_non_scroll_damage_scissored_to_changed_rows() {
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: snapshot(1, &["a", "b", "c", "d"], vec![1, 2, 3]),
            renderer_config: config(),
            hyperlink_highlights: Vec::new(),
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
    fn terminal_output_damage_clears_the_previous_cursor_row() {
        let mut previous_snapshot = snapshot(1, &["a", "b", "c", "d"], vec![1]);
        previous_snapshot.cursor = Some(RenderSurfaceCursorSnapshot {
            x: 2,
            y: 1,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        });
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: previous_snapshot,
            renderer_config: config(),
            hyperlink_highlights: Vec::new(),
        };
        let mut current = snapshot(2, &["a", "b", "c", "changed"], vec![3]);
        current.cursor = Some(RenderSurfaceCursorSnapshot {
            x: 0,
            y: 3,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        });

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
            hyperlink_highlights: Vec::new(),
        };

        assert_eq!(action(&previous, &current), WgpuRetainedFrameAction::Reuse);
    }

    #[test]
    fn upward_cursor_motion_clears_the_full_previous_cursor_height() {
        let mut current = snapshot(3, &["a", "b", "c"], vec![0]);
        current.cursor = Some(RenderSurfaceCursorSnapshot {
            x: 0,
            y: 0,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        });
        let motion_config = WgpuRendererConfig {
            content_height_px: 10,
            grid_rows: 3,
            ..config()
        };
        let mut previous_config = motion_config;
        previous_config.cursor_position_px = Some((0, 1));
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: current.clone(),
            renderer_config: previous_config,
            hyperlink_highlights: Vec::new(),
        };
        let mut current_config = motion_config;
        current_config.cursor_position_px = Some((0, 0));

        assert_eq!(
            retained_frame_action(
                Some(&previous),
                &WgpuTerminalWorkspaceSurface {
                    render_target_plan: WgpuTerminalRenderTargetPlan::new(80, 10),
                    surface_snapshot: &current,
                    renderer_config: current_config,
                    hyperlink_highlights: &[],
                },
            ),
            WgpuRetainedFrameAction::Damage {
                top_px: 0,
                bottom_px: 6,
            }
        );
    }

    #[test]
    fn hyperlink_hover_damages_all_old_and_new_segment_rows() {
        let current = snapshot(3, &["a", "link", "link", "d"], vec![2]);
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: current.clone(),
            renderer_config: config(),
            hyperlink_highlights: vec![TerminalHyperlink {
                uri: "https://example.com".to_string(),
                x: 0,
                y: 1,
                columns: 4,
            }],
        };
        let new_highlights = [TerminalHyperlink {
            uri: "https://example.com".to_string(),
            x: 0,
            y: 2,
            columns: 4,
        }];

        assert_eq!(
            retained_frame_action(
                Some(&previous),
                &WgpuTerminalWorkspaceSurface {
                    render_target_plan: WgpuTerminalRenderTargetPlan::new(80, 64),
                    surface_snapshot: &current,
                    renderer_config: config(),
                    hyperlink_highlights: &new_highlights,
                },
            ),
            WgpuRetainedFrameAction::Damage {
                top_px: 16,
                bottom_px: 48,
            }
        );
    }

    #[test]
    fn hidden_live_scrollbar_does_not_turn_output_into_a_full_frame() {
        let mut previous_snapshot = snapshot(1, &["a", "b", "c", "d"], vec![2]);
        previous_snapshot.scrollback = Some(RenderSurfaceScrollbackSnapshot {
            display_offset: 0,
            history_size: 10,
        });
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: previous_snapshot,
            renderer_config: config(),
            hyperlink_highlights: Vec::new(),
        };
        let mut current = snapshot(2, &["a", "b", "changed", "d"], vec![2]);
        current.scrollback = Some(RenderSurfaceScrollbackSnapshot {
            display_offset: 0,
            history_size: 11,
        });

        assert_eq!(
            action(&previous, &current),
            WgpuRetainedFrameAction::Damage {
                top_px: 32,
                bottom_px: 48,
            }
        );
    }

    #[test]
    fn moving_a_visible_scrollbar_rebuilds_the_retained_frame() {
        let mut previous_snapshot = snapshot(1, &["a", "b", "c", "d"], vec![0, 1, 2, 3]);
        previous_snapshot.scrollback = Some(RenderSurfaceScrollbackSnapshot {
            display_offset: 8,
            history_size: 10,
        });
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: previous_snapshot,
            renderer_config: config(),
            hyperlink_highlights: Vec::new(),
        };
        let mut current = snapshot(2, &["a", "b", "c", "d"], vec![0, 1, 2, 3]);
        current.scrollback = Some(RenderSurfaceScrollbackSnapshot {
            display_offset: 7,
            history_size: 10,
        });

        assert_eq!(action(&previous, &current), WgpuRetainedFrameAction::Full);
    }

    #[test]
    fn empty_damage_forces_a_full_retained_frame_rebuild() {
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: snapshot(1, &["a", "b", "c", "d"], vec![2]),
            renderer_config: config(),
            hyperlink_highlights: Vec::new(),
        };
        let current = snapshot(2, &["a", "b", "c", "d"], Vec::new());

        assert_eq!(action(&previous, &current), WgpuRetainedFrameAction::Full);
    }

    #[test]
    fn full_screen_clear_is_not_mistaken_for_scroll_through_absent_rows() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(10, 4));
        let target_id = RenderTargetId::new(8);
        store.apply_bytes(target_id, Seq::new(1), b"header\r\nGPU data");
        let previous_snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        store.clear_damage_up_to(target_id, Seq::new(1));

        store.apply_bytes(target_id, Seq::new(2), b"\x1b[?25l\x1b[H\x1b[2J");
        let current = store.render_surface_snapshot_of(target_id).unwrap();
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: previous_snapshot,
            renderer_config: config(),
            hyperlink_highlights: Vec::new(),
        };

        assert!(current.rows.is_empty());
        assert_eq!(action(&previous, &current), WgpuRetainedFrameAction::Full);
    }

    #[test]
    fn changing_ime_preedit_forces_a_full_retained_frame_rebuild() {
        let mut current = snapshot(1, &["a", "b", "c", "d"], vec![1]);
        current.cursor = Some(RenderSurfaceCursorSnapshot {
            x: 2,
            y: 1,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        });
        current.ime_preedit = Some(RenderSurfaceImePreeditSnapshot {
            text: "拼音".to_string(),
            cursor_range: Some((6, 6)),
        });
        let mut previous_snapshot = current.clone();
        previous_snapshot.ime_preedit = Some(RenderSurfaceImePreeditSnapshot {
            text: "拼".to_string(),
            cursor_range: Some((3, 3)),
        });
        let previous = WgpuRetainedWorkspaceSnapshot {
            snapshot: previous_snapshot,
            renderer_config: config(),
            hyperlink_highlights: Vec::new(),
        };

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
            hyperlink_highlights: Vec::new(),
        };
        let current_config = previous.renderer_config;

        assert_eq!(current.dirty_rows.len(), 35);
        let actual = retained_frame_action(
            Some(&previous),
            &WgpuTerminalWorkspaceSurface {
                render_target_plan: WgpuTerminalRenderTargetPlan::new(1271, 1523),
                surface_snapshot: &current,
                renderer_config: current_config,
                hyperlink_highlights: &[],
            },
        );
        assert!(
            matches!(actual, WgpuRetainedFrameAction::ScrollUp { rows: 2, .. }),
            "expected two-row scroll, got {actual:?}; detected={:?}",
            scroll_up_rows(&previous.snapshot, &current, current_config.grid_rows),
        );
    }
}
