use std::borrow::Cow;

use germinal_ports::rendering::frame_plan_builder::RgbColorDto;

const VISUAL_BELL_BORDER_PX: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuVisualBellFrame {
    pub width_px: u32,
    pub height_px: u32,
}

impl WgpuVisualBellFrame {
    pub const fn new(width_px: u32, height_px: u32) -> Self {
        Self {
            width_px,
            height_px,
        }
    }
}

#[derive(Clone)]
pub struct WgpuVisualBellRenderer {
    pipeline: wgpu::RenderPipeline,
}

impl WgpuVisualBellRenderer {
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        color: RgbColorDto,
    ) -> Self {
        let shader_source = solid_color_shader(VISUAL_BELL_SHADER, color);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("germinal.visual_bell.shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_source)),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("germinal.visual_bell.pipeline_layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let color_targets = [Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("germinal.visual_bell.pipeline"),
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
        frame: WgpuVisualBellFrame,
    ) -> usize {
        let rects = visual_bell_border_rects(frame);
        if rects.is_empty() {
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
            label: Some("germinal.visual_bell.render_pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        render_pass.set_pipeline(&self.pipeline);

        for rect in &rects {
            render_pass.set_viewport(
                rect.x_px as f32,
                rect.y_px as f32,
                rect.width_px as f32,
                rect.height_px as f32,
                0.0,
                1.0,
            );
            render_pass.set_scissor_rect(rect.x_px, rect.y_px, rect.width_px, rect.height_px);
            render_pass.draw(0..3, 0..1);
        }

        rects.len()
    }
}

impl std::fmt::Debug for WgpuVisualBellRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WgpuVisualBellRenderer")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VisualBellRect {
    x_px: u32,
    y_px: u32,
    width_px: u32,
    height_px: u32,
}

fn visual_bell_border_rects(frame: WgpuVisualBellFrame) -> Vec<VisualBellRect> {
    if frame.width_px == 0 || frame.height_px == 0 {
        return Vec::new();
    }

    let thickness = VISUAL_BELL_BORDER_PX
        .min(frame.width_px)
        .min(frame.height_px);
    vec![
        VisualBellRect {
            x_px: 0,
            y_px: 0,
            width_px: frame.width_px,
            height_px: thickness,
        },
        VisualBellRect {
            x_px: 0,
            y_px: frame.height_px - thickness,
            width_px: frame.width_px,
            height_px: thickness,
        },
        VisualBellRect {
            x_px: 0,
            y_px: thickness,
            width_px: thickness,
            height_px: frame.height_px.saturating_sub(thickness.saturating_mul(2)),
        },
        VisualBellRect {
            x_px: frame.width_px - thickness,
            y_px: thickness,
            width_px: thickness,
            height_px: frame.height_px.saturating_sub(thickness.saturating_mul(2)),
        },
    ]
    .into_iter()
    .filter(|rect| rect.width_px > 0 && rect.height_px > 0)
    .collect()
}

fn solid_color_shader(template: &str, color: RgbColorDto) -> String {
    let color = format!(
        "vec4<f32>({:.8}, {:.8}, {:.8}, 1.0)",
        f32::from(color.red) / 255.0,
        f32::from(color.green) / 255.0,
        f32::from(color.blue) / 255.0,
    );
    template.replace("BELL_COLOR", &color)
}

const VISUAL_BELL_SHADER: &str = r#"
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
    return BELL_COLOR;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_bell_uses_four_pixel_window_border() {
        assert_eq!(
            visual_bell_border_rects(WgpuVisualBellFrame::new(100, 80)),
            vec![
                VisualBellRect {
                    x_px: 0,
                    y_px: 0,
                    width_px: 100,
                    height_px: 4,
                },
                VisualBellRect {
                    x_px: 0,
                    y_px: 76,
                    width_px: 100,
                    height_px: 4,
                },
                VisualBellRect {
                    x_px: 0,
                    y_px: 4,
                    width_px: 4,
                    height_px: 72,
                },
                VisualBellRect {
                    x_px: 96,
                    y_px: 4,
                    width_px: 4,
                    height_px: 72,
                },
            ]
        );
    }

    #[test]
    fn visual_bell_skips_empty_windows() {
        assert!(visual_bell_border_rects(WgpuVisualBellFrame::new(0, 80)).is_empty());
        assert!(visual_bell_border_rects(WgpuVisualBellFrame::new(100, 0)).is_empty());
    }
}
