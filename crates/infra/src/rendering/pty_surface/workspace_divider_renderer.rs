use std::borrow::Cow;

use germinal_ports::rendering::frame_plan_builder::RgbColorDto;

use crate::rendering::pty_surface::render_target_plan::WgpuTerminalRenderTargetPlan;

#[derive(Clone)]
pub struct WgpuWorkspaceDividerRenderer {
    pipeline: wgpu::RenderPipeline,
}

impl WgpuWorkspaceDividerRenderer {
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        color: RgbColorDto,
    ) -> Self {
        let shader_source = solid_color_shader(WORKSPACE_DIVIDER_SHADER, color);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("germinal.workspace.divider.shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_source)),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("germinal.workspace.divider.pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let color_targets = [Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("germinal.workspace.divider.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &color_targets,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self { pipeline }
    }

    pub fn encode(
        &self,
        command_encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        targets: &[WgpuTerminalRenderTargetPlan],
    ) -> usize {
        let dividers = workspace_divider_rects(targets);
        if dividers.is_empty() {
            return 0;
        }

        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut render_pass = command_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("germinal.workspace.divider.render_pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        render_pass.set_pipeline(&self.pipeline);

        for divider in &dividers {
            render_pass.set_viewport(
                divider.x_px as f32,
                divider.y_px as f32,
                divider.width_px as f32,
                divider.height_px as f32,
                0.0,
                1.0,
            );
            render_pass.set_scissor_rect(
                divider.x_px,
                divider.y_px,
                divider.width_px,
                divider.height_px,
            );
            render_pass.draw(0..3, 0..1);
        }

        dividers.len()
    }
}

impl std::fmt::Debug for WgpuWorkspaceDividerRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WgpuWorkspaceDividerRenderer")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkspaceDividerRect {
    x_px: u32,
    y_px: u32,
    width_px: u32,
    height_px: u32,
}

fn workspace_divider_rects(targets: &[WgpuTerminalRenderTargetPlan]) -> Vec<WorkspaceDividerRect> {
    let mut dividers = Vec::new();

    for (index, first) in targets.iter().enumerate() {
        for second in &targets[index + 1..] {
            if let Some(divider) = vertical_divider(*first, *second) {
                dividers.push(divider);
            }
            if let Some(divider) = horizontal_divider(*first, *second) {
                dividers.push(divider);
            }
        }
    }

    dividers
}

fn vertical_divider(
    first: WgpuTerminalRenderTargetPlan,
    second: WgpuTerminalRenderTargetPlan,
) -> Option<WorkspaceDividerRect> {
    let first_right = first.x_px.saturating_add(first.width_px);
    let second_right = second.x_px.saturating_add(second.width_px);
    let boundary = if first_right == second.x_px {
        first_right
    } else if second_right == first.x_px {
        second_right
    } else {
        return None;
    };
    let start = first.y_px.max(second.y_px);
    let end = first
        .y_px
        .saturating_add(first.height_px)
        .min(second.y_px.saturating_add(second.height_px));
    (end > start).then_some(WorkspaceDividerRect {
        x_px: boundary.saturating_sub(1),
        y_px: start,
        width_px: 2,
        height_px: end - start,
    })
}

fn horizontal_divider(
    first: WgpuTerminalRenderTargetPlan,
    second: WgpuTerminalRenderTargetPlan,
) -> Option<WorkspaceDividerRect> {
    let first_bottom = first.y_px.saturating_add(first.height_px);
    let second_bottom = second.y_px.saturating_add(second.height_px);
    let boundary = if first_bottom == second.y_px {
        first_bottom
    } else if second_bottom == first.y_px {
        second_bottom
    } else {
        return None;
    };
    let start = first.x_px.max(second.x_px);
    let end = first
        .x_px
        .saturating_add(first.width_px)
        .min(second.x_px.saturating_add(second.width_px));
    (end > start).then_some(WorkspaceDividerRect {
        x_px: start,
        y_px: boundary.saturating_sub(1),
        width_px: end - start,
        height_px: 2,
    })
}

fn solid_color_shader(template: &str, color: RgbColorDto) -> String {
    let color = format!(
        "vec4<f32>({:.8}, {:.8}, {:.8}, 1.0)",
        f32::from(color.red) / 255.0,
        f32::from(color.green) / 255.0,
        f32::from(color.blue) / 255.0,
    );
    template.replace("DIVIDER_COLOR", &color)
}

const WORKSPACE_DIVIDER_SHADER: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return DIVIDER_COLOR;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_vertical_divider_for_adjacent_targets() {
        let targets = [
            WgpuTerminalRenderTargetPlan::new(50, 40),
            WgpuTerminalRenderTargetPlan::new(51, 40).with_origin(50, 0),
        ];

        assert_eq!(
            workspace_divider_rects(&targets),
            vec![WorkspaceDividerRect {
                x_px: 49,
                y_px: 0,
                width_px: 2,
                height_px: 40
            }]
        );
    }

    #[test]
    fn single_target_has_no_divider() {
        assert!(workspace_divider_rects(&[WgpuTerminalRenderTargetPlan::new(80, 40)]).is_empty());
    }
}
