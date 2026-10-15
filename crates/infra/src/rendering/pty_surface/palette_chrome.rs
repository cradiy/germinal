use wgpu::util::DeviceExt;

use super::surface_frame_presenter::WgpuTerminalWorkspaceSurface;

pub(super) struct PaletteChrome {
    pipeline: wgpu::RenderPipeline,
}

impl PaletteChrome {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("germinal.command_palette.chrome"),
            source: wgpu::ShaderSource::Wgsl(include_str!("palette_chrome.wgsl").into()),
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("germinal.command_palette.chrome"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 48,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline }
    }

    pub(super) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        window: [u32; 2],
        surface: &WgpuTerminalWorkspaceSurface<'_>,
    ) {
        let plan = surface.render_target_plan;
        let cw = surface.renderer_config.cell_width_px as f32;
        let ch = surface.renderer_config.cell_height_px as f32;
        let [x, y, w, h] = [
            plan.x_px as f32,
            plan.y_px as f32,
            plan.width_px as f32,
            plan.height_px as f32,
        ];
        let scale = ch / 24.0;
        let background = surface.surface_snapshot.default_background;
        let base = [
            f32::from(background.red) / 255.0,
            f32::from(background.green) / 255.0,
            f32::from(background.blue) / 255.0,
            1.0,
        ];
        let lighter = |amount: f32| {
            [
                base[0] + amount * (1.0 - base[0]),
                base[1] + amount * (1.0 - base[1]),
                base[2] + amount * (1.0 - base[2]),
                1.0,
            ]
        };
        let mut instances = Vec::<f32>::new();
        let mut rect = |bounds: [f32; 4], mut color: [f32; 4], radius: f32, softness: f32| {
            for channel in &mut color[..3] {
                *channel = if *channel <= 0.04045 {
                    *channel / 12.92
                } else {
                    ((*channel + 0.055) / 1.055).powf(2.4)
                };
            }
            instances.extend(bounds);
            instances.extend(color);
            instances.extend([radius, softness, window[0] as f32, window[1] as f32]);
        };
        let sidebar = surface.surface_snapshot.target_id
            == germinal_ports::rendering::tab_bar::SIDEBAR_RENDER_TARGET_ID;
        if sidebar {
            rect([x, y, w, h], base, 0.0, 0.5);
            rect([x + w - scale, y, scale, h], lighter(0.08), 0.0, 0.5);
        } else {
            rect(
                [0.0, 0.0, window[0] as f32, window[1] as f32],
                [0.0, 0.0, 0.0, 0.28],
                0.0,
                0.5,
            );
            rect(
                [x - 12.0 * scale, y, w + 24.0 * scale, h + 24.0 * scale],
                [0.0, 0.0, 0.0, 0.28],
                24.0 * scale,
                12.0 * scale,
            );
            rect([x, y, w, h], lighter(0.13), 12.0 * scale, 0.7);
            rect(
                [x + scale, y + scale, w - 2.0 * scale, h - 2.0 * scale],
                base,
                11.0 * scale,
                0.7,
            );
            for line_y in [y + ch * 3.0, y + h - ch * 3.0] {
                rect(
                    [x + scale, line_y, w - 2.0 * scale, scale],
                    lighter(0.1),
                    0.0,
                    0.5,
                );
            }
        }
        for row in &surface.surface_snapshot.rows {
            let row_y = y + row.y as f32 * ch;
            let hovered = sidebar && row.runs.first().is_some_and(|run| run.style.italic);
            if hovered || row.runs.first().is_some_and(|run| run.style.bold) {
                rect(
                    [x + cw, row_y - ch * 0.25, w - cw * 2.0, ch * 1.5],
                    lighter(if hovered {
                        0.055
                    } else if sidebar {
                        0.035
                    } else {
                        0.08
                    }),
                    7.0 * scale,
                    0.7,
                );
            }
            if sidebar {
                rect(
                    [x + cw * 1.7, row_y - ch * 0.05, cw * 2.2, ch * 1.1],
                    lighter(0.025),
                    ch * 0.55,
                    0.7,
                );
                continue;
            }
            for run in row.runs.iter().skip(1) {
                if run.text.trim().is_empty() {
                    continue;
                }
                let width = germinal_ports::pty_host::width::terminal_text_cell_width(&run.text)
                    as f32
                    * cw;
                rect(
                    [
                        x + run.x as f32 * cw - cw * 0.4,
                        row_y - ch * 0.05,
                        width + cw * 0.8,
                        ch * 1.1,
                    ],
                    lighter(0.055),
                    4.0 * scale,
                    0.7,
                );
            }
        }
        let bytes: Vec<u8> = instances
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("germinal.command_palette.rects"),
            contents: &bytes,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("germinal.command_palette.chrome"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..6, 0..(instances.len() / 12) as u32);
    }
}
