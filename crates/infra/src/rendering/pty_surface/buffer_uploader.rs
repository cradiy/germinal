use std::sync::Arc;

use wgpu::util::DeviceExt;

use crate::rendering::pty_surface::quad_vertex_buffer_builder::WgpuVertexBuffer;
pub use crate::rendering::pty_surface::quad_vertex_buffer_builder::{
	WGPU_VERTEX_KIND_BACKGROUND, WGPU_VERTEX_KIND_GLYPH, WGPU_VERTEX_KIND_UNDERLINE, WgpuGpuVertex,
};

#[derive(Debug, Clone, Default)]
pub struct WgpuBufferUploader;

impl WgpuBufferUploader {
	pub fn new() -> Self { Self }

	pub fn build_upload_bytes(&self, buffer: &WgpuVertexBuffer) -> WgpuBufferUploadBytes {
		WgpuBufferUploadBytes {
			vertex_data:  Arc::clone(&buffer.vertices),
			index_data:   Arc::clone(&buffer.indices),
			vertex_count: buffer.vertices.len() as u32,
			index_count:  buffer.indices.len() as u32,
		}
	}

	pub fn upload(&self, device: &wgpu::Device, buffer: &WgpuVertexBuffer) -> WgpuUploadedBuffers {
		let upload_bytes = self.build_upload_bytes(buffer);

		self.upload_bytes(device, &upload_bytes)
	}

	pub fn upload_bytes(
		&self,
		device: &wgpu::Device,
		upload_bytes: &WgpuBufferUploadBytes,
	) -> WgpuUploadedBuffers {
		let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
			label:    Some("germinal.wgpu.vertex_buffer"),
			contents: upload_bytes.vertex_bytes(),
			usage:    wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
		});

		let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
			label:    Some("germinal.wgpu.index_buffer"),
			contents: upload_bytes.index_bytes(),
			usage:    wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
		});

		WgpuUploadedBuffers {
			vertex_buffer,
			index_buffer,
			vertex_count: upload_bytes.vertex_count,
			index_count: upload_bytes.index_count,
			index_format: wgpu::IndexFormat::Uint32,
		}
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct WgpuBufferUploadBytes {
	pub vertex_data:  Arc<[WgpuGpuVertex]>,
	pub index_data:   Arc<[u32]>,
	pub vertex_count: u32,
	pub index_count:  u32,
}

impl WgpuBufferUploadBytes {
	pub fn is_empty(&self) -> bool { self.vertex_count == 0 && self.index_count == 0 }

	pub fn vertex_bytes(&self) -> &[u8] { bytes_of_slice(&self.vertex_data) }

	pub fn index_bytes(&self) -> &[u8] { bytes_of_slice(&self.index_data) }

	pub fn vertex_byte_len(&self) -> usize { self.vertex_bytes().len() }

	pub fn index_byte_len(&self) -> usize { self.index_bytes().len() }
}

#[derive(Debug)]
pub struct WgpuUploadedBuffers {
	pub vertex_buffer: wgpu::Buffer,
	pub index_buffer:  wgpu::Buffer,
	pub vertex_count:  u32,
	pub index_count:   u32,
	pub index_format:  wgpu::IndexFormat,
}

fn bytes_of_slice<T>(items: &[T]) -> &[u8] {
	let byte_len = std::mem::size_of_val(items);

	unsafe { std::slice::from_raw_parts(items.as_ptr() as *const u8, byte_len) }
}

#[cfg(test)]
mod tests {
	use germinal_ports::rendering::frame_plan_builder::{RgbColorDto, TextStyleDto};

	use super::*;
	use crate::rendering::pty_surface::{
		quad_vertex_buffer_builder::WgpuQuadVertexBufferBuilder,
		renderer_backend::{WgpuQuadDrawItem, WgpuQuadKind},
	};

	#[test]
	fn builds_upload_bytes_without_vertex_conversion_copy() {
		let style = TextStyleDto {
			foreground: Some(RgbColorDto::new(255, 0, 0)),
			background: None,
			bold:       true,
			italic:     false,
			underline:  false,
		};

		let quad_builder = WgpuQuadVertexBufferBuilder::new();

		let vertex_buffer = quad_builder.build(&[WgpuQuadDrawItem {
			kind: WgpuQuadKind::Glyph { c: 'r', bold: true },
			x_px: 10,
			y_px: 20,
			width_px: 8,
			height_px: 16,
			style,
		}]);

		let uploader = WgpuBufferUploader::new();
		let upload_bytes = uploader.build_upload_bytes(&vertex_buffer);

		assert_eq!(upload_bytes.vertex_count, 4);
		assert_eq!(upload_bytes.index_count, 6);

		assert_eq!(upload_bytes.vertex_data.len(), 4);
		assert_eq!(upload_bytes.index_data.len(), 6);

		assert_eq!(upload_bytes.vertex_byte_len(), 4 * WgpuGpuVertex::BYTE_SIZE);

		assert_eq!(upload_bytes.index_byte_len(), 6 * std::mem::size_of::<u32>());

		assert!(!upload_bytes.is_empty());
	}

	#[test]
	fn exposes_vertex_buffer_layout_matching_gpu_vertex() {
		let layout = WgpuGpuVertex::vertex_buffer_layout();

		assert_eq!(layout.array_stride, WgpuGpuVertex::BYTE_SIZE as wgpu::BufferAddress);

		assert_eq!(layout.step_mode, wgpu::VertexStepMode::Vertex);
		assert_eq!(layout.attributes.len(), 5);

		assert_eq!(layout.attributes[0].shader_location, 0);
		assert_eq!(layout.attributes[0].offset, 0);
		assert_eq!(layout.attributes[0].format, wgpu::VertexFormat::Float32x2);

		assert_eq!(layout.attributes[4].shader_location, 4);
		assert_eq!(layout.attributes[4].offset, 36);
		assert_eq!(layout.attributes[4].format, wgpu::VertexFormat::Uint32);
	}
}
