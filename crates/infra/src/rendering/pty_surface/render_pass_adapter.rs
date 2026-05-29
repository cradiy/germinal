use crate::rendering::pty_surface::{
	buffer_uploader::WgpuUploadedBuffers, glyph_atlas_bind_group::WgpuTerminalGlyphAtlasBindGroup,
	pipeline_factory::WgpuTerminalPipeline, render_pass_encoder::WgpuTerminalRenderPassEncoder,
	viewport_bind_group::WgpuViewportBindGroup,
};

pub struct WgpuTerminalRenderPassAdapter<'pass, 'resource> {
	render_pass:            &'pass mut wgpu::RenderPass<'resource>,
	pipeline:               &'resource WgpuTerminalPipeline,
	viewport_bind_group:    &'resource WgpuViewportBindGroup,
	glyph_atlas_bind_group: Option<&'resource WgpuTerminalGlyphAtlasBindGroup>,
	uploaded_buffers:       &'resource WgpuUploadedBuffers,
}

impl<'pass, 'resource> WgpuTerminalRenderPassAdapter<'pass, 'resource> {
	pub fn new(
		render_pass: &'pass mut wgpu::RenderPass<'resource>,
		pipeline: &'resource WgpuTerminalPipeline,
		viewport_bind_group: &'resource WgpuViewportBindGroup,
		glyph_atlas_bind_group: Option<&'resource WgpuTerminalGlyphAtlasBindGroup>,
		uploaded_buffers: &'resource WgpuUploadedBuffers,
	) -> Self {
		Self { render_pass, pipeline, viewport_bind_group, glyph_atlas_bind_group, uploaded_buffers }
	}
}

impl<'pass, 'resource> WgpuTerminalRenderPassEncoder
	for WgpuTerminalRenderPassAdapter<'pass, 'resource>
{
	fn set_pipeline(&mut self) { self.render_pass.set_pipeline(&self.pipeline.render_pipeline); }

	fn set_bind_group(&mut self, index: u32, dynamic_offsets: &[u32]) {
		if index == 0 {
			self.render_pass.set_bind_group(
				0,
				Some(&self.viewport_bind_group.bind_group),
				dynamic_offsets,
			);

			if let Some(glyph_atlas_bind_group) = self.glyph_atlas_bind_group {
				self.render_pass.set_bind_group(1, Some(&glyph_atlas_bind_group.bind_group), &[]);
			}

			return;
		}

		if index == 1 {
			if let Some(glyph_atlas_bind_group) = self.glyph_atlas_bind_group {
				self.render_pass.set_bind_group(
					1,
					Some(&glyph_atlas_bind_group.bind_group),
					dynamic_offsets,
				);
			}
		}
	}

	fn set_vertex_buffer(&mut self, slot: u32) {
		self.render_pass.set_vertex_buffer(slot, self.uploaded_buffers.vertex_buffer.slice(..));
	}

	fn set_index_buffer(&mut self, format: wgpu::IndexFormat) {
		self.render_pass.set_index_buffer(self.uploaded_buffers.index_buffer.slice(..), format);
	}

	fn draw_indexed(
		&mut self,
		indices: std::ops::Range<u32>,
		base_vertex: i32,
		instances: std::ops::Range<u32>,
	) {
		self.render_pass.draw_indexed(indices, base_vertex, instances);
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalRenderPassAdapterSpec {
	pub bind_group_index:             u32,
	pub vertex_slot:                  u32,
	pub index_format:                 wgpu::IndexFormat,
	pub glyph_atlas_bind_group_index: u32,
}

impl WgpuTerminalRenderPassAdapterSpec {
	pub const fn new() -> Self {
		Self {
			bind_group_index:             0,
			vertex_slot:                  0,
			index_format:                 wgpu::IndexFormat::Uint32,
			glyph_atlas_bind_group_index: 1,
		}
	}
}

impl Default for WgpuTerminalRenderPassAdapterSpec {
	fn default() -> Self { Self::new() }
}
