use crate::rendering::pty_surface::render_pass_plan::{
    WgpuRenderPassCommand, WgpuTerminalRenderPassPlan,
};

pub trait WgpuTerminalRenderPassEncoder {
    fn set_pipeline(&mut self);
    fn set_bind_group(&mut self, index: u32, dynamic_offsets: &[u32]);
    fn set_vertex_buffer(&mut self, slot: u32);
    fn set_index_buffer(&mut self, format: wgpu::IndexFormat);
    fn draw_indexed(
        &mut self,
        indices: std::ops::Range<u32>,
        base_vertex: i32,
        instances: std::ops::Range<u32>,
    );
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WgpuTerminalRenderPassPlanEncoder;

impl WgpuTerminalRenderPassPlanEncoder {
    pub fn new() -> Self {
        Self
    }

    pub fn encode<E>(&self, plan: &WgpuTerminalRenderPassPlan, encoder: &mut E)
    where
        E: WgpuTerminalRenderPassEncoder,
    {
        for command in &plan.commands {
            match command {
                WgpuRenderPassCommand::SetPipeline => {
                    encoder.set_pipeline();
                }
                WgpuRenderPassCommand::SetBindGroup {
                    index,
                    dynamic_offsets,
                } => {
                    encoder.set_bind_group(*index, dynamic_offsets);
                }
                WgpuRenderPassCommand::SetVertexBuffer { slot } => {
                    encoder.set_vertex_buffer(*slot);
                }
                WgpuRenderPassCommand::SetIndexBuffer { format } => {
                    encoder.set_index_buffer(*format);
                }
                WgpuRenderPassCommand::DrawIndexed {
                    indices,
                    base_vertex,
                    instances,
                } => {
                    encoder.draw_indexed(indices.clone(), *base_vertex, instances.clone());
                }
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordingWgpuTerminalRenderPassEncoder {
    pub calls: Vec<RecordedWgpuRenderPassCall>,
}

impl RecordingWgpuTerminalRenderPassEncoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn call_count(&self) -> usize {
        self.calls.len()
    }

    pub fn draw_count(&self) -> usize {
        self.calls
            .iter()
            .filter(|call| matches!(call, RecordedWgpuRenderPassCall::DrawIndexed { .. }))
            .count()
    }

    pub fn clear(&mut self) {
        self.calls.clear();
    }
}

impl WgpuTerminalRenderPassEncoder for RecordingWgpuTerminalRenderPassEncoder {
    fn set_pipeline(&mut self) {
        self.calls.push(RecordedWgpuRenderPassCall::SetPipeline);
    }

    fn set_bind_group(&mut self, index: u32, dynamic_offsets: &[u32]) {
        self.calls.push(RecordedWgpuRenderPassCall::SetBindGroup {
            index,
            dynamic_offsets: dynamic_offsets.to_vec(),
        });
    }

    fn set_vertex_buffer(&mut self, slot: u32) {
        self.calls
            .push(RecordedWgpuRenderPassCall::SetVertexBuffer { slot });
    }

    fn set_index_buffer(&mut self, format: wgpu::IndexFormat) {
        self.calls
            .push(RecordedWgpuRenderPassCall::SetIndexBuffer { format });
    }

    fn draw_indexed(
        &mut self,
        indices: std::ops::Range<u32>,
        base_vertex: i32,
        instances: std::ops::Range<u32>,
    ) {
        self.calls.push(RecordedWgpuRenderPassCall::DrawIndexed {
            indices,
            base_vertex,
            instances,
        });
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedWgpuRenderPassCall {
    SetPipeline,
    SetBindGroup {
        index: u32,
        dynamic_offsets: Vec<u32>,
    },
    SetVertexBuffer {
        slot: u32,
    },
    SetIndexBuffer {
        format: wgpu::IndexFormat,
    },
    DrawIndexed {
        indices: std::ops::Range<u32>,
        base_vertex: i32,
        instances: std::ops::Range<u32>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rendering::pty_surface::{
        draw_indexed::WgpuDrawIndexedPlan, render_pass_plan::WgpuTerminalRenderPassPlan,
    };

    #[test]
    fn encodes_render_pass_plan_into_encoder_calls() {
        let draw_plan = WgpuDrawIndexedPlan {
            vertex_slot: 0,
            index_format: wgpu::IndexFormat::Uint32,
            index_count: 42,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
        };

        let plan = WgpuTerminalRenderPassPlan::new(draw_plan);

        let plan_encoder = WgpuTerminalRenderPassPlanEncoder::new();
        let mut encoder = RecordingWgpuTerminalRenderPassEncoder::new();

        plan_encoder.encode(&plan, &mut encoder);

        assert_eq!(encoder.call_count(), 5);
        assert_eq!(encoder.draw_count(), 1);

        assert_eq!(
            encoder.calls,
            vec![
                RecordedWgpuRenderPassCall::SetPipeline,
                RecordedWgpuRenderPassCall::SetBindGroup {
                    index: 0,
                    dynamic_offsets: Vec::new()
                },
                RecordedWgpuRenderPassCall::SetVertexBuffer { slot: 0 },
                RecordedWgpuRenderPassCall::SetIndexBuffer {
                    format: wgpu::IndexFormat::Uint32
                },
                RecordedWgpuRenderPassCall::DrawIndexed {
                    indices: 0..42,
                    base_vertex: 0,
                    instances: 0..1,
                },
            ]
        );
    }

    #[test]
    fn recorder_can_be_reused() {
        let draw_plan = WgpuDrawIndexedPlan {
            vertex_slot: 0,
            index_format: wgpu::IndexFormat::Uint32,
            index_count: 6,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
        };

        let plan = WgpuTerminalRenderPassPlan::new(draw_plan);

        let plan_encoder = WgpuTerminalRenderPassPlanEncoder::new();
        let mut encoder = RecordingWgpuTerminalRenderPassEncoder::new();

        plan_encoder.encode(&plan, &mut encoder);

        assert_eq!(encoder.call_count(), 5);

        encoder.clear();

        assert_eq!(encoder.call_count(), 0);
        assert_eq!(encoder.draw_count(), 0);

        plan_encoder.encode(&plan, &mut encoder);

        assert_eq!(encoder.call_count(), 5);
        assert_eq!(encoder.draw_count(), 1);
    }
}
