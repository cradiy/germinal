use std::sync::Arc;

use germinal_domain::pty_host::{
	cell_size::TerminalCellSize, font_weight::TerminalFontWeight, profile::TerminalProfile,
	scale_factor::TerminalScaleFactor, size_info::TerminalSizeInfo,
	window_metrics::TerminalWindowMetrics, window_size::TerminalWindowSize,
};
use germinal_ports::rendering::surface_snapshot::RenderSurfaceSnapshot;
use winit::window::{Window, WindowId};

use crate::rendering::pty_surface::{
	crossfont_glyph_atlas::{WgpuCrossfontGlyphAtlasBuilder, WgpuTerminalFontWeight},
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
};

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
}

impl WgpuTerminalWindowRuntime {
	pub async fn new(window: Arc<Window>) -> Result<Self, String> {
		let instance = wgpu::Instance::default();

		let surface = instance.create_surface(Arc::clone(&window)).expect("failed to create surface");

		let size = window.inner_size();
		let width = size.width.max(1);
		let height = size.height.max(1);

		let adapter = instance
			.request_adapter(&wgpu::RequestAdapterOptions {
				power_preference:       wgpu::PowerPreference::HighPerformance,
				compatible_surface:     Some(&surface),
				force_fallback_adapter: false,
			})
			.await
			.map_err(|error| error.to_string())?;

		let (device, queue) = adapter
			.request_device(&wgpu::DeviceDescriptor {
				label: Some("germinal.terminal.device"),
				..Default::default()
			})
			.await
			.map_err(|error| error.to_string())?;

		let surface_config = surface
			.get_default_config(&adapter, width, height)
			.ok_or_else(|| "failed to get default surface config".to_string())?;

		surface.configure(&device, &surface_config);

		let pipeline_spec = WgpuTerminalPipelineSpec::new(surface_config.format);
		let pipeline_factory = WgpuTerminalPipelineFactory::new(pipeline_spec);
		let pipeline = pipeline_factory.create(&device);

		let profile = terminal_profile_from_alacritty_crossfont_metrics(
			TerminalProfile::DEFAULT,
			window.scale_factor(),
		)?;
		let size_info = terminal_size_info(profile, width, height, window.scale_factor());

		let frame_builder = build_terminal_frame_builder(profile, size_info, window.scale_factor())?;
		let frame_renderer = WgpuTerminalFrameRenderer::new(frame_builder);
		let presenter = WgpuTerminalSurfaceFramePresenter::new(frame_renderer);

		let surface_snapshot = RenderSurfaceSnapshot {
			target_id:  germinal_domain::rendering::render_target_id::RenderTargetId::new(0),
			latest_seq: germinal_domain::shared::seq::Seq::ZERO,
			rows:       Vec::new(),
			cursor:     None,
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
		})
	}

	pub fn window_id(&self) -> WindowId { self.window.id() }

	pub fn window_size(&self) -> winit::dpi::PhysicalSize<u32> { self.window.inner_size() }

	pub fn request_window_redraw(&self) { self.window.request_redraw(); }

	pub fn set_surface_snapshot(&mut self, snapshot: RenderSurfaceSnapshot) {
		self.surface_snapshot = snapshot;
		self.request_redraw();
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

		match self.presenter.present_surface_frame(WgpuTerminalSurfacePresentInput {
			surface: &self.surface,
			device: &self.device,
			queue: &self.queue,
			render_target_plan,
			pipeline: &self.pipeline,
			surface_snapshot: &self.surface_snapshot,
			renderer_config,
		}) {
			Ok(_) => {}
			Err(error) => {
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
) -> Result<WgpuTerminalFrameBuilder, String> {
	let base = WgpuTerminalFrameBuilder::new(WgpuRendererConfig::from(size_info));

	let glyph_config = profile.glyph_render_config(size_info, TerminalScaleFactor::new(scale_factor));
	let terminal_cell_size = glyph_config.cell_size();
	let crossfont_builder = WgpuCrossfontGlyphAtlasBuilder::new(
		glyph_config.font_family_name(),
		glyph_config.font_size_px(),
	)
	.map_err(|error| format!("crossfont font load failed: {error:?}"))?
	.with_bold_font_weight(wgpu_font_weight_from_terminal(glyph_config.bold_font_weight()))
	.with_padding_px(2)
	.with_columns(16)
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
) -> Result<TerminalProfile, String> {
	let scale_factor = TerminalScaleFactor::new(scale_factor);
	let font_px = profile.font_physical_px(scale_factor);
	let font_family = profile.font_family().name();
	let metrics = WgpuCrossfontGlyphAtlasBuilder::load_cell_metrics(font_family, font_px)
		.map_err(|error| format!("crossfont metrics load failed: {error:?}"))?;

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
