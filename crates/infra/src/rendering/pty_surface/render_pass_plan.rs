use std::ops::Range;

use crate::rendering::pty_surface::draw_indexed::WgpuDrawIndexedPlan;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuTerminalRenderPassPlan {
    pub commands: Vec<WgpuRenderPassCommand>,
}

impl WgpuTerminalRenderPassPlan {
    pub fn new(draw_plan: WgpuDrawIndexedPlan) -> Self {
        Self {
            commands: vec![
                WgpuRenderPassCommand::SetPipeline,
                WgpuRenderPassCommand::SetBindGroup {
                    index: 0,
                    dynamic_offsets: Vec::new(),
                },
                WgpuRenderPassCommand::SetVertexBuffer {
                    slot: draw_plan.vertex_slot,
                },
                WgpuRenderPassCommand::SetIndexBuffer {
                    format: draw_plan.index_format,
                },
                WgpuRenderPassCommand::DrawIndexed {
                    indices: draw_plan.index_range(),
                    base_vertex: draw_plan.base_vertex,
                    instances: draw_plan.instance_range(),
                },
            ],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn has_draw_indexed(&self) -> bool {
        self.commands
            .iter()
            .any(|command| matches!(command, WgpuRenderPassCommand::DrawIndexed { .. }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WgpuRenderPassCommand {
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
        indices: Range<u32>,
        base_vertex: i32,
        instances: Range<u32>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WgpuRenderPassPlanRecorder {
    pub recorded: Vec<WgpuRenderPassCommand>,
}

impl WgpuRenderPassPlanRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_plan(&mut self, plan: &WgpuTerminalRenderPassPlan) {
        self.recorded.extend(plan.commands.iter().cloned());
    }

    pub fn command_count(&self) -> usize {
        self.recorded.len()
    }

    pub fn draw_count(&self) -> usize {
        self.recorded
            .iter()
            .filter(|command| matches!(command, WgpuRenderPassCommand::DrawIndexed { .. }))
            .count()
    }

    pub fn clear(&mut self) {
        self.recorded.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_render_pass_plan_from_draw_indexed_plan() {
        let draw_plan = WgpuDrawIndexedPlan {
            vertex_slot: 0,
            index_format: wgpu::IndexFormat::Uint32,
            index_count: 42,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
        };

        let plan = WgpuTerminalRenderPassPlan::new(draw_plan);

        assert_eq!(plan.len(), 5);
        assert!(!plan.is_empty());
        assert!(plan.has_draw_indexed());

        assert_eq!(plan.commands[0], WgpuRenderPassCommand::SetPipeline);

        assert_eq!(
            plan.commands[1],
            WgpuRenderPassCommand::SetBindGroup {
                index: 0,
                dynamic_offsets: Vec::new(),
            }
        );

        assert_eq!(
            plan.commands[2],
            WgpuRenderPassCommand::SetVertexBuffer { slot: 0 }
        );

        assert_eq!(
            plan.commands[3],
            WgpuRenderPassCommand::SetIndexBuffer {
                format: wgpu::IndexFormat::Uint32,
            }
        );

        assert_eq!(
            plan.commands[4],
            WgpuRenderPassCommand::DrawIndexed {
                indices: 0..42,
                base_vertex: 0,
                instances: 0..1,
            }
        );
    }

    #[test]
    fn recorder_records_render_pass_plan_commands() {
        let draw_plan = WgpuDrawIndexedPlan {
            vertex_slot: 0,
            index_format: wgpu::IndexFormat::Uint32,
            index_count: 6,
            base_vertex: 0,
            first_instance: 0,
            instance_count: 1,
        };

        let plan = WgpuTerminalRenderPassPlan::new(draw_plan);

        let mut recorder = WgpuRenderPassPlanRecorder::new();

        recorder.record_plan(&plan);

        assert_eq!(recorder.command_count(), 5);
        assert_eq!(recorder.draw_count(), 1);
        assert_eq!(recorder.recorded, plan.commands);

        recorder.clear();

        assert_eq!(recorder.command_count(), 0);
        assert_eq!(recorder.draw_count(), 0);
    }
}
