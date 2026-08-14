use wgpu::util::DeviceExt;

use crate::rendering::pty_surface::shader::WgpuViewportUniform;

#[derive(Debug, Clone, Default)]
pub struct WgpuViewportBindGroupFactory;

impl WgpuViewportBindGroupFactory {
    pub fn new() -> Self {
        Self
    }

    pub fn build_upload_bytes(&self, viewport: WgpuViewportUniform) -> WgpuViewportUploadBytes {
        WgpuViewportUploadBytes {
            bytes: viewport.as_std140_bytes().to_vec(),
            width_px: viewport.width_px,
            height_px: viewport.height_px,
        }
    }

    pub fn create(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        binding: u32,
        viewport: WgpuViewportUniform,
    ) -> WgpuViewportBindGroup {
        let upload_bytes = self.build_upload_bytes(viewport);

        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("germinal.terminal.viewport.uniform_buffer"),
            contents: &upload_bytes.bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("germinal.terminal.viewport.bind_group"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            }],
        });

        WgpuViewportBindGroup {
            viewport,
            buffer,
            bind_group,
            binding,
            byte_len: upload_bytes.bytes.len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WgpuViewportUploadBytes {
    pub bytes: Vec<u8>,
    pub width_px: f32,
    pub height_px: f32,
}

impl WgpuViewportUploadBytes {
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

#[derive(Debug)]
pub struct WgpuViewportBindGroup {
    pub viewport: WgpuViewportUniform,
    pub buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
    pub binding: u32,
    pub byte_len: usize,
}

impl WgpuViewportBindGroup {
    pub fn update(&mut self, queue: &wgpu::Queue, viewport: WgpuViewportUniform) {
        let bytes = viewport.as_std140_bytes();

        queue.write_buffer(&self.buffer, 0, &bytes);

        self.viewport = viewport;
        self.byte_len = bytes.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_viewport_upload_bytes() {
        let factory = WgpuViewportBindGroupFactory::new();

        let upload_bytes = factory.build_upload_bytes(WgpuViewportUniform::new(1280.0, 720.0));

        assert_eq!(upload_bytes.width_px, 1280.0);
        assert_eq!(upload_bytes.height_px, 720.0);
        assert_eq!(upload_bytes.byte_len(), 8);
        assert!(!upload_bytes.is_empty());

        let width = f32::from_ne_bytes(upload_bytes.bytes[0..4].try_into().unwrap());

        let height = f32::from_ne_bytes(upload_bytes.bytes[4..8].try_into().unwrap());

        assert_eq!(width, 1280.0);
        assert_eq!(height, 720.0);
    }

    #[test]
    fn viewport_upload_bytes_match_uniform_serialization() {
        let factory = WgpuViewportBindGroupFactory::new();
        let viewport = WgpuViewportUniform::new(1920.0, 1080.0);

        let upload_bytes = factory.build_upload_bytes(viewport);

        assert_eq!(upload_bytes.bytes, viewport.as_std140_bytes().to_vec());
    }
}
