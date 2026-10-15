use std::ops::Range;

use crate::rendering::pty_surface::buffer_uploader::{WgpuBufferUploadBytes, WgpuUploadedBuffers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuDrawIndexedPlan {
    pub vertex_slot: u32,
    pub index_format: wgpu::IndexFormat,
    pub index_count: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
    pub instance_count: u32,
}

impl WgpuDrawIndexedPlan {
    pub fn from_upload_bytes(upload_bytes: &WgpuBufferUploadBytes) -> Option<Self> {
        if upload_bytes.vertex_count == 0 || upload_bytes.index_count == 0 {
            return None;
        }

        Some(Self {
            vertex_slot: 0,
            index_format: wgpu::IndexFormat::Uint32,
            index_count: upload_bytes.index_count,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
        })
    }

    pub fn from_uploaded_buffers(uploaded_buffers: &WgpuUploadedBuffers) -> Option<Self> {
        if uploaded_buffers.vertex_count == 0 || uploaded_buffers.index_count == 0 {
            return None;
        }

        Some(Self {
            vertex_slot: 0,
            index_format: uploaded_buffers.index_format,
            index_count: uploaded_buffers.index_count,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
        })
    }

    pub fn index_range(&self) -> Range<u32> {
        0..self.index_count
    }

    pub fn instance_range(&self) -> Range<u32> {
        self.first_instance..self.first_instance + self.instance_count
    }

    pub fn encode<'a>(
        &self,
        render_pass: &mut wgpu::RenderPass<'a>,
        uploaded_buffers: &'a WgpuUploadedBuffers,
    ) {
        assert_eq!(
            self.index_format, uploaded_buffers.index_format,
            "draw plan index format must match uploaded index buffer format"
        );

        assert!(
            self.index_count <= uploaded_buffers.index_count,
            "draw plan index count must not exceed uploaded index count"
        );

        render_pass.set_vertex_buffer(self.vertex_slot, uploaded_buffers.vertex_buffer.slice(..));

        render_pass.set_index_buffer(
            uploaded_buffers.index_buffer.slice(..),
            uploaded_buffers.index_format,
        );

        render_pass.draw_indexed(self.index_range(), self.base_vertex, self.instance_range());
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WgpuDrawIndexedStats {
    pub draw_count: u64,
    pub last_index_count: u32,
    pub last_instance_count: u32,
}

impl WgpuDrawIndexedStats {
    pub fn record(&mut self, plan: WgpuDrawIndexedPlan) {
        self.draw_count += 1;
        self.last_index_count = plan.index_count;
        self.last_instance_count = plan.instance_count;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::rendering::pty_surface::buffer_uploader::WgpuGpuVertex;

    #[test]
    fn returns_none_for_empty_upload_bytes() {
        let upload_bytes = WgpuBufferUploadBytes {
            vertex_data: Arc::from(Vec::<WgpuGpuVertex>::new()),
            index_data: Arc::from(Vec::<u32>::new()),
            vertex_count: 0,
            index_count: 0,
        };

        assert_eq!(WgpuDrawIndexedPlan::from_upload_bytes(&upload_bytes), None);
    }
}
