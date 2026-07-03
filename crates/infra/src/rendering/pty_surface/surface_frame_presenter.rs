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

	pub fn present_surface_frame(
		&self,
		input: WgpuTerminalSurfacePresentInput<'_, '_>,
	) -> Result<WgpuTerminalSurfaceFramePresentResult, WgpuTerminalSurfaceFramePresentError> {
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

		let render_to_view_started_at = Instant::now();
		let render_result = self.frame_renderer.render_to_view(
			WgpuTerminalGpuContext { device: input.device, queue: input.queue },
			WgpuTerminalRenderView {
				command_encoder:    &mut command_encoder,
				target_view:        &target_view,
				render_target_plan: input.render_target_plan,
				pipeline:           input.pipeline,
				surface_snapshot:   input.surface_snapshot,
				renderer_config:    input.renderer_config,
			},
		);
		let render_to_view = render_to_view_started_at.elapsed();

		let submit_started_at = Instant::now();
		input.queue.submit(Some(command_encoder.finish()));
		let submit = submit_started_at.elapsed();

		let present_started_at = Instant::now();
		input.queue.present(surface_texture);
		let present = present_started_at.elapsed();

		Ok(WgpuTerminalSurfaceFramePresentResult {
			render_result,
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

pub struct WgpuTerminalSurfacePresentInput<'a, 'window> {
	pub surface:            &'a wgpu::Surface<'window>,
	pub device:             &'a wgpu::Device,
	pub queue:              &'a wgpu::Queue,
	pub render_target_plan: WgpuTerminalRenderTargetPlan,
	pub pipeline:           &'a WgpuTerminalPipeline,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalSurfaceFramePresentResult {
	pub render_result:          WgpuTerminalFrameRenderResult,
	pub acquired_surface_frame: bool,
	pub submitted:              bool,
	pub presented:              bool,
	pub timings:                WgpuTerminalSurfaceFrameTimings,
}

impl WgpuTerminalSurfaceFramePresentResult {
	pub fn completed(&self) -> bool {
		self.acquired_surface_frame && self.submitted && self.presented
	}

	pub fn rendered(&self) -> bool { self.render_result.rendered() }
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
