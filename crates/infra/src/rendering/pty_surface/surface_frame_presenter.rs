use std::time::{Duration, Instant};

use germinal_ports::rendering::surface_snapshot::RenderSurfaceSnapshot;

use crate::rendering::pty_surface::{
	frame_renderer::{
		WgpuTerminalFrameRenderResult, WgpuTerminalFrameRenderer, WgpuTerminalGpuContext,
		WgpuTerminalRenderView,
	},
	pipeline_factory::WgpuTerminalPipeline,
	render_target_plan::WgpuTerminalRenderTargetPlan,
	renderer_backend::WgpuRendererConfig,
};

#[derive(Debug, Clone)]
pub struct WgpuTerminalSurfaceFramePresenter {
	frame_renderer: WgpuTerminalFrameRenderer,
}

impl WgpuTerminalSurfaceFramePresenter {
	pub fn new(frame_renderer: WgpuTerminalFrameRenderer) -> Self { Self { frame_renderer } }

	pub fn frame_renderer(&self) -> &WgpuTerminalFrameRenderer { &self.frame_renderer }

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
		let target_view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
		let create_texture_view = create_view_started_at.elapsed();

		let create_encoder_started_at = Instant::now();
		let mut command_encoder =
			input.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
				label: Some("germinal.terminal.command_encoder"),
			});
		let create_command_encoder = create_encoder_started_at.elapsed();
		clear_target_view(&mut command_encoder, &target_view);

		let render_to_view_started_at = Instant::now();
		let render_results = input
			.surfaces
			.iter()
			.map(|surface| {
				self.frame_renderer.render_to_view(
					WgpuTerminalGpuContext { device: input.device, queue: input.queue },
					WgpuTerminalRenderView {
						command_encoder:    &mut command_encoder,
						target_view:        &target_view,
						render_target_plan: surface.render_target_plan,
						pipeline:           input.pipeline,
						surface_snapshot:   surface.surface_snapshot,
						renderer_config:    surface.renderer_config,
					},
				)
			})
			.collect();
		let render_to_view = render_to_view_started_at.elapsed();

		let submit_started_at = Instant::now();
		input.queue.submit(Some(command_encoder.finish()));
		let submit = submit_started_at.elapsed();

		let present_started_at = Instant::now();
		input.queue.present(surface_texture);
		let present = present_started_at.elapsed();

		Ok(WgpuTerminalWorkspaceFramePresentResult {
			render_results,
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

fn clear_target_view(command_encoder: &mut wgpu::CommandEncoder, target_view: &wgpu::TextureView) {
	let color_attachment = Some(wgpu::RenderPassColorAttachment {
		view: target_view,
		depth_slice: None,
		resolve_target: None,
		ops: wgpu::Operations {
			load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
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
	pub surface:  &'a wgpu::Surface<'window>,
	pub device:   &'a wgpu::Device,
	pub queue:    &'a wgpu::Queue,
	pub pipeline: &'a WgpuTerminalPipeline,
	pub surfaces: &'a [WgpuTerminalWorkspaceSurface<'a>],
}

pub struct WgpuTerminalWorkspaceSurface<'a> {
	pub render_target_plan: WgpuTerminalRenderTargetPlan,
	pub surface_snapshot:   &'a RenderSurfaceSnapshot,
	pub renderer_config:    WgpuRendererConfig,
}

impl Default for WgpuTerminalSurfaceFramePresenter {
	fn default() -> Self { Self::new(WgpuTerminalFrameRenderer::default()) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WgpuTerminalSurfaceFrameTimings {
	pub acquire_surface_texture: Duration,
	pub create_texture_view:     Duration,
	pub create_command_encoder:  Duration,
	pub render_to_view:          Duration,
	pub submit:                  Duration,
	pub present:                 Duration,
	pub total:                   Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuTerminalWorkspaceFramePresentResult {
	pub render_results:         Vec<WgpuTerminalFrameRenderResult>,
	pub acquired_surface_frame: bool,
	pub submitted:              bool,
	pub presented:              bool,
	pub timings:                WgpuTerminalSurfaceFrameTimings,
}

impl WgpuTerminalWorkspaceFramePresentResult {
	pub fn completed(&self) -> bool {
		self.acquired_surface_frame && self.submitted && self.presented
	}

	pub fn rendered(&self) -> bool {
		self.render_results.iter().any(WgpuTerminalFrameRenderResult::rendered)
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
	pub creates_texture_view:     bool,
	pub creates_command_encoder:  bool,
	pub submits_command_buffer:   bool,
	pub presents_surface_texture: bool,
}

impl WgpuTerminalSurfaceFramePresenterSpec {
	pub const fn new() -> Self {
		Self {
			acquires_surface_texture: true,
			creates_texture_view:     true,
			creates_command_encoder:  true,
			submits_command_buffer:   true,
			presents_surface_texture: true,
		}
	}
}

impl Default for WgpuTerminalSurfaceFramePresenterSpec {
	fn default() -> Self { Self::new() }
}
