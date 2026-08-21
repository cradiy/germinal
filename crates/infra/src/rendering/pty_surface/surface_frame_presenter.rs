use std::time::{Duration, Instant};

use germinal_ports::rendering::surface_snapshot::RenderSurfaceSnapshot;

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
        }
    }

    pub fn with_background_shader(mut self, renderer: WgpuBackgroundShaderRenderer) -> Self {
        self.background_shader_renderer = Some(renderer);
        self
    }

    pub fn frame_renderer(&self) -> &WgpuTerminalFrameRenderer {
        &self.frame_renderer
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

        let create_view_started_at = Instant::now();
        let target_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let create_texture_view = create_view_started_at.elapsed();

        let create_encoder_started_at = Instant::now();
        let mut command_encoder =
            input
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("germinal.terminal.command_encoder"),
                });
        let create_command_encoder = create_encoder_started_at.elapsed();
        clear_target_view(&mut command_encoder, &target_view, input.clear_color);

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
        let render_results = input
            .surfaces
            .iter()
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
