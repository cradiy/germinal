use crate::rendering::pty_surface::{
    buffer_uploader::WgpuGpuVertex, shader::WgpuTerminalShaderSpec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalPipelineSpec {
    pub shader: WgpuTerminalShaderSpec,
    pub color_format: wgpu::TextureFormat,
    pub primitive_topology: wgpu::PrimitiveTopology,
    pub front_face: wgpu::FrontFace,
    pub cull_mode: Option<wgpu::Face>,
    pub alpha_blending: bool,
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
            format: self.color_format,
            blend: if self.alpha_blending {
                Some(wgpu::BlendState::ALPHA_BLENDING)
            } else {
                None
            },
            write_mask: wgpu::ColorWrites::ALL,
        }
    }

    pub fn primitive_state(&self) -> wgpu::PrimitiveState {
        wgpu::PrimitiveState {
            topology: self.primitive_topology,
            strip_index_format: None,
            front_face: self.front_face,
            cull_mode: self.cull_mode,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        }
    }
}
