use crate::rendering::pty_surface::{
	buffer_uploader::WgpuGpuVertex, shader::WgpuTerminalShaderSpec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalPipelineSpec {
	pub shader:             WgpuTerminalShaderSpec,
	pub color_format:       wgpu::TextureFormat,
	pub primitive_topology: wgpu::PrimitiveTopology,
	pub front_face:         wgpu::FrontFace,
	pub cull_mode:          Option<wgpu::Face>,
	pub alpha_blending:     bool,
}

impl WgpuTerminalPipelineSpec {
	pub fn new(color_format: wgpu::TextureFormat) -> Self {
		Self {
			shader: WgpuTerminalShaderSpec::new(),
			color_format,
			primitive_topology: wgpu::PrimitiveTopology::TriangleList,
			front_face: wgpu::FrontFace::Ccw,
			cull_mode: None,
			alpha_blending: true,
		}
	}

	pub fn vertex_buffer_layout<'a>(&self) -> wgpu::VertexBufferLayout<'a> {
		let _ = self;
		WgpuGpuVertex::vertex_buffer_layout()
	}

	pub fn color_target_state(&self) -> wgpu::ColorTargetState {
		wgpu::ColorTargetState {
			format:     self.color_format,
			blend:      if self.alpha_blending { Some(wgpu::BlendState::ALPHA_BLENDING) } else { None },
			write_mask: wgpu::ColorWrites::ALL,
		}
	}

	pub fn primitive_state(&self) -> wgpu::PrimitiveState {
		wgpu::PrimitiveState {
			topology:           self.primitive_topology,
			strip_index_format: None,
			front_face:         self.front_face,
			cull_mode:          self.cull_mode,
			polygon_mode:       wgpu::PolygonMode::Fill,
			unclipped_depth:    false,
			conservative:       false,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn builds_default_terminal_pipeline_spec() {
		let spec = WgpuTerminalPipelineSpec::new(wgpu::TextureFormat::Bgra8UnormSrgb);

		assert_eq!(spec.color_format, wgpu::TextureFormat::Bgra8UnormSrgb);
		assert_eq!(spec.primitive_topology, wgpu::PrimitiveTopology::TriangleList);
		assert_eq!(spec.front_face, wgpu::FrontFace::Ccw);
		assert_eq!(spec.cull_mode, None);
		assert!(spec.alpha_blending);
	}

	#[test]
	fn exposes_vertex_buffer_layout_from_gpu_vertex() {
		let spec = WgpuTerminalPipelineSpec::new(wgpu::TextureFormat::Bgra8UnormSrgb);

		let layout = spec.vertex_buffer_layout();

		assert_eq!(layout.array_stride, WgpuGpuVertex::BYTE_SIZE as wgpu::BufferAddress);
		assert_eq!(layout.step_mode, wgpu::VertexStepMode::Vertex);
		assert_eq!(layout.attributes.len(), 5);
	}

	#[test]
	fn builds_color_target_state_with_alpha_blending() {
		let spec = WgpuTerminalPipelineSpec::new(wgpu::TextureFormat::Bgra8UnormSrgb);

		let color_target = spec.color_target_state();

		assert_eq!(color_target.format, wgpu::TextureFormat::Bgra8UnormSrgb);
		assert_eq!(color_target.blend, Some(wgpu::BlendState::ALPHA_BLENDING));
		assert_eq!(color_target.write_mask, wgpu::ColorWrites::ALL);
	}

	#[test]
	fn builds_triangle_list_primitive_state() {
		let spec = WgpuTerminalPipelineSpec::new(wgpu::TextureFormat::Bgra8UnormSrgb);

		let primitive = spec.primitive_state();

		assert_eq!(primitive.topology, wgpu::PrimitiveTopology::TriangleList);
		assert_eq!(primitive.strip_index_format, None);
		assert_eq!(primitive.front_face, wgpu::FrontFace::Ccw);
		assert_eq!(primitive.cull_mode, None);
		assert_eq!(primitive.polygon_mode, wgpu::PolygonMode::Fill);
	}
}
