use std::borrow::Cow;

use crate::rendering::pty_surface::{
	glyph_atlas_bind_group::WgpuTerminalGlyphAtlasBindGroupFactory,
	pipeline_spec::WgpuTerminalPipelineSpec,
};

#[derive(Debug, Clone)]
pub struct WgpuTerminalPipelineFactory {
	spec: WgpuTerminalPipelineSpec,
}

impl WgpuTerminalPipelineFactory {
	pub fn new(spec: WgpuTerminalPipelineSpec) -> Self { Self { spec } }

	pub fn spec(&self) -> WgpuTerminalPipelineSpec { self.spec }

	pub fn create(&self, device: &wgpu::Device) -> WgpuTerminalPipeline {
		let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label:  Some("germinal.terminal.shader"),
			source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(self.spec.shader.source)),
		});

		let viewport_bind_group_layout =
			device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
				label:   Some("germinal.terminal.viewport.bind_group_layout"),
				entries: &[wgpu::BindGroupLayoutEntry {
					binding:    self.spec.shader.viewport_binding,
					visibility: wgpu::ShaderStages::VERTEX,
					ty:         wgpu::BindingType::Buffer {
						ty:                 wgpu::BufferBindingType::Uniform,
						has_dynamic_offset: false,
						min_binding_size:   None,
					},
					count:      None,
				}],
			});

		let glyph_atlas_bind_group_factory = WgpuTerminalGlyphAtlasBindGroupFactory::new();

		let glyph_atlas_bind_group_layout = glyph_atlas_bind_group_factory.create_layout(device);

		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label:              Some("germinal.terminal.pipeline_layout"),
			bind_group_layouts: &[
				Some(&viewport_bind_group_layout),
				Some(&glyph_atlas_bind_group_layout),
			],
			immediate_size:     0,
		});

		let vertex_buffer_layout = self.spec.vertex_buffer_layout();
		let color_targets = [Some(self.spec.color_target_state())];

		let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label:          Some("germinal.terminal.render_pipeline"),
			layout:         Some(&pipeline_layout),
			vertex:         wgpu::VertexState {
				module:              &shader_module,
				entry_point:         Some(self.spec.shader.vertex_entry),
				buffers:             &[Some(vertex_buffer_layout)],
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			},
			fragment:       Some(wgpu::FragmentState {
				module:              &shader_module,
				entry_point:         Some(self.spec.shader.fragment_entry),
				targets:             &color_targets,
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			}),
			primitive:      self.spec.primitive_state(),
			depth_stencil:  None,
			multisample:    wgpu::MultisampleState::default(),
			multiview_mask: None,
			cache:          None,
		});

		WgpuTerminalPipeline {
			spec: self.spec,
			render_pipeline,
			viewport_bind_group_layout,
			glyph_atlas_bind_group_layout,
		}
	}
}

impl Default for WgpuTerminalPipelineFactory {
	fn default() -> Self {
		Self::new(WgpuTerminalPipelineSpec::new(wgpu::TextureFormat::Bgra8UnormSrgb))
	}
}

pub struct WgpuTerminalPipeline {
	pub spec:                          WgpuTerminalPipelineSpec,
	pub render_pipeline:               wgpu::RenderPipeline,
	pub viewport_bind_group_layout:    wgpu::BindGroupLayout,
	pub glyph_atlas_bind_group_layout: wgpu::BindGroupLayout,
}
