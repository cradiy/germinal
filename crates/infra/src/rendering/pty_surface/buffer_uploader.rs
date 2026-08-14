use std::{cell::RefCell, collections::HashMap, sync::Arc};

use germinal_ports::rendering::render_target_id::RenderTargetId;

use crate::rendering::pty_surface::quad_vertex_buffer_builder::WgpuVertexBuffer;
pub use crate::rendering::pty_surface::quad_vertex_buffer_builder::{
    WGPU_VERTEX_KIND_BACKGROUND, WGPU_VERTEX_KIND_GLYPH, WGPU_VERTEX_KIND_UNDERLINE, WgpuGpuVertex,
};

#[derive(Debug, Clone, Default)]
pub struct WgpuBufferUploader {
    cached_buffers: RefCell<HashMap<RenderTargetId, WgpuBufferUploadCache>>,
}

impl WgpuBufferUploader {
    pub fn new() -> Self {
        Self {
            cached_buffers: RefCell::new(HashMap::new()),
        }
    }

    pub fn build_upload_bytes(&self, buffer: &WgpuVertexBuffer) -> WgpuBufferUploadBytes {
        WgpuBufferUploadBytes {
            vertex_data: Arc::clone(&buffer.vertices),
            index_data: Arc::clone(&buffer.indices),
            vertex_count: buffer.vertices.len() as u32,
            index_count: buffer.indices.len() as u32,
        }
    }

    pub fn remove_render_target(&self, target_id: RenderTargetId) -> bool {
        self.cached_buffers
            .borrow_mut()
            .remove(&target_id)
            .is_some()
    }

    pub fn upload_bytes(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_id: RenderTargetId,
        upload_bytes: &WgpuBufferUploadBytes,
    ) -> WgpuUploadedBuffers {
        let mut cache = self.cached_buffers.borrow_mut();
        let vertex_byte_len = upload_bytes.vertex_byte_len() as u64;
        let index_byte_len = upload_bytes.index_byte_len() as u64;

        let cached = cache
            .entry(target_id)
            .or_insert_with(|| WgpuBufferUploadCache::new(device, vertex_byte_len, index_byte_len));

        if cached.vertex_capacity_bytes < vertex_byte_len
            || cached.index_capacity_bytes < index_byte_len
        {
            *cached = WgpuBufferUploadCache::new(
                device,
                cached.vertex_capacity_bytes.max(vertex_byte_len),
                cached.index_capacity_bytes.max(index_byte_len),
            );
        }

        queue.write_buffer(&cached.vertex_buffer, 0, upload_bytes.vertex_bytes());
        queue.write_buffer(&cached.index_buffer, 0, upload_bytes.index_bytes());

        WgpuUploadedBuffers {
            vertex_buffer: Arc::clone(&cached.vertex_buffer),
            index_buffer: Arc::clone(&cached.index_buffer),
            vertex_count: upload_bytes.vertex_count,
            index_count: upload_bytes.index_count,
            index_format: wgpu::IndexFormat::Uint32,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WgpuBufferUploadBytes {
    pub vertex_data: Arc<Vec<WgpuGpuVertex>>,
    pub index_data: Arc<Vec<u32>>,
    pub vertex_count: u32,
    pub index_count: u32,
}

impl WgpuBufferUploadBytes {
    pub fn is_empty(&self) -> bool {
        self.vertex_count == 0 && self.index_count == 0
    }

    pub fn vertex_bytes(&self) -> &[u8] {
        bytes_of_slice(self.vertex_data.as_slice())
    }

    pub fn index_bytes(&self) -> &[u8] {
        bytes_of_slice(self.index_data.as_slice())
    }

    pub fn vertex_byte_len(&self) -> usize {
        self.vertex_bytes().len()
    }

    pub fn index_byte_len(&self) -> usize {
        self.index_bytes().len()
    }
}

#[derive(Debug)]
pub struct WgpuUploadedBuffers {
    pub vertex_buffer: Arc<wgpu::Buffer>,
    pub index_buffer: Arc<wgpu::Buffer>,
    pub vertex_count: u32,
    pub index_count: u32,
    pub index_format: wgpu::IndexFormat,
}

#[derive(Debug, Clone)]
struct WgpuBufferUploadCache {
    vertex_buffer: Arc<wgpu::Buffer>,
    index_buffer: Arc<wgpu::Buffer>,
    vertex_capacity_bytes: u64,
    index_capacity_bytes: u64,
}

impl WgpuBufferUploadCache {
    fn new(device: &wgpu::Device, vertex_capacity_bytes: u64, index_capacity_bytes: u64) -> Self {
        Self {
            vertex_buffer: Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("germinal.wgpu.vertex_buffer"),
                size: vertex_capacity_bytes.max(1),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })),
            index_buffer: Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("germinal.wgpu.index_buffer"),
                size: index_capacity_bytes.max(1),
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })),
            vertex_capacity_bytes,
            index_capacity_bytes,
        }
    }
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
            bold: true,
            italic: false,
            underline: false,
        };

        let quad_builder = WgpuQuadVertexBufferBuilder::new();

        let vertex_buffer = quad_builder.build(&[WgpuQuadDrawItem {
            kind: WgpuQuadKind::Glyph {
                c: 'r',
                bold: true,
                italic: false,
            },
            x_px: 10,
            y_px: 20,
            width_px: 8,
            height_px: 16,
            style,
            alpha: u8::MAX,
        }]);

        let uploader = WgpuBufferUploader::new();
        let upload_bytes = uploader.build_upload_bytes(&vertex_buffer);

        assert_eq!(upload_bytes.vertex_count, 4);
        assert_eq!(upload_bytes.index_count, 6);

        assert_eq!(upload_bytes.vertex_data.len(), 4);
        assert_eq!(upload_bytes.index_data.len(), 6);

        assert_eq!(upload_bytes.vertex_byte_len(), 4 * WgpuGpuVertex::BYTE_SIZE);

        assert_eq!(
            upload_bytes.index_byte_len(),
            6 * std::mem::size_of::<u32>()
        );

        assert!(!upload_bytes.is_empty());
    }

    #[test]
    fn exposes_vertex_buffer_layout_matching_gpu_vertex() {
        let layout = WgpuGpuVertex::vertex_buffer_layout();

        assert_eq!(
            layout.array_stride,
            WgpuGpuVertex::BYTE_SIZE as wgpu::BufferAddress
        );

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
