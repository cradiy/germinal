use std::borrow::Cow;

use eros::Context;
use germinal::app;
use germinal_domain::workspace::entity::workspace::Workspace;
use germinal_infra::rendering::pty_surface::render_plugin::{
    WgpuPaneInputResult, WgpuPaneRenderContext, WgpuPaneRenderError, WgpuPaneRenderPlugin,
    WgpuPaneRenderResult, WgpuPaneRenderer, WgpuPaneResizeEvent,
};
use germinal_ports::{
    event::{
        runtime_event::RuntimeEvent,
        window_input_event::{
            WindowInputElementState, WindowInputEvent, WindowInputKey, WindowPointerButton,
        },
    },
    rendering::render_target_id::RenderTargetId,
};
use tracing::info;
use winit::event_loop::EventLoop;

const GPU_PANE_TARGET: RenderTargetId = RenderTargetId::new(1);

fn main() -> eros::Result<()> {
    let (config, paths) = app::load_or_create_config().context("failed to load Germinal config")?;
    app::init_logging(&config.logging, &paths).context("failed to initialize Germinal logging")?;

    let event_loop = EventLoop::<RuntimeEvent>::with_user_event()
        .build()
        .context("failed to create Germinal event loop")?;
    let plugins = vec![WgpuPaneRenderPlugin::new(
        GPU_PANE_TARGET,
        AnimatedGpuPane::default(),
    )];
    let mut app = app::App::new_with_workspace_and_wgpu_panes(
        event_loop.create_proxy(),
        config,
        Workspace::two_pane(),
        plugins,
    )
    .context("failed to create wgpu pane example")?;

    info!(
        target_id = GPU_PANE_TARGET.value(),
        "starting Germinal wgpu pane example"
    );
    app.run(event_loop)
        .context("failed to run Germinal wgpu pane example")?;
    Ok(())
}

struct AnimatedGpuPane {
    gpu: Option<DemoGpu>,
    pointer: Option<(f32, f32)>,
    focused: bool,
    animate: bool,
    pulse: f32,
    resize_count: u32,
}

impl Default for AnimatedGpuPane {
    fn default() -> Self {
        Self {
            gpu: None,
            pointer: None,
            focused: false,
            animate: true,
            pulse: 0.0,
            resize_count: 0,
        }
    }
}

impl WgpuPaneRenderer for AnimatedGpuPane {
    fn render(
        &mut self,
        mut context: WgpuPaneRenderContext<'_>,
    ) -> Result<WgpuPaneRenderResult, WgpuPaneRenderError> {
        let gpu = self
            .gpu
            .get_or_insert_with(|| DemoGpu::new(context.device, context.color_format));
        let pointer = self.pointer.unwrap_or((
            context.placement.width_px as f32 * 0.5,
            context.placement.height_px as f32 * 0.5,
        ));
        let time = if self.animate {
            context.elapsed.as_secs_f32()
        } else {
            0.0
        };
        let parameters = [
            context.placement.x_px as f32,
            context.placement.y_px as f32,
            context.placement.width_px as f32,
            context.placement.height_px as f32,
            pointer.0,
            pointer.1,
            time,
            f32::from(self.focused),
            self.pulse,
            self.resize_count as f32,
            context.scale_factor as f32,
            0.0,
        ];
        context
            .queue
            .write_buffer(&gpu.parameters, 0, &parameter_bytes(parameters));

        let mut render_pass = context.begin_render_pass(Some("germinal.example.wgpu_pane.pass"));
        render_pass.set_pipeline(&gpu.pipeline);
        render_pass.set_bind_group(0, &gpu.bind_group, &[]);
        render_pass.draw(0..3, 0..1);

        Ok(if self.animate {
            WgpuPaneRenderResult::redraw()
        } else {
            WgpuPaneRenderResult::idle()
        })
    }

    fn input(&mut self, event: &WindowInputEvent) -> WgpuPaneInputResult {
        match event {
            WindowInputEvent::FocusChanged(focused) => self.focused = *focused,
            WindowInputEvent::PointerMoved { position, .. } => {
                self.pointer = Some((position.x_px as f32, position.y_px as f32));
            }
            WindowInputEvent::PointerLeft => self.pointer = None,
            WindowInputEvent::PointerButton {
                state,
                button: WindowPointerButton::Primary,
                position,
                ..
            } => {
                self.pointer = Some((position.x_px as f32, position.y_px as f32));
                self.pulse = if *state == WindowInputElementState::Pressed {
                    1.0
                } else {
                    0.0
                };
            }
            WindowInputEvent::Key {
                state: WindowInputElementState::Pressed,
                logical_key: WindowInputKey::Character(character),
                ..
            } if character == " " => self.animate = !self.animate,
            _ => {}
        }
        WgpuPaneInputResult::redraw()
    }

    fn resize(&mut self, _event: WgpuPaneResizeEvent) -> WgpuPaneInputResult {
        self.resize_count = self.resize_count.wrapping_add(1);
        WgpuPaneInputResult::redraw()
    }
}

struct DemoGpu {
    pipeline: wgpu::RenderPipeline,
    parameters: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl DemoGpu {
    fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let parameters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("germinal.example.wgpu_pane.parameters"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("germinal.example.wgpu_pane.bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("germinal.example.wgpu_pane.bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: parameters.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("germinal.example.wgpu_pane.pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("germinal.example.wgpu_pane.shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(SHADER)),
        });
        let targets = [Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("germinal.example.wgpu_pane.pipeline"),
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
                targets: &targets,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            pipeline,
            parameters,
            bind_group,
        }
    }
}

fn parameter_bytes(values: [f32; 12]) -> [u8; 48] {
    let mut bytes = [0_u8; 48];
    for (index, value) in values.into_iter().enumerate() {
        let offset = index * size_of::<f32>();
        bytes[offset..offset + size_of::<f32>()].copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

const SHADER: &str = r#"
struct Parameters {
    rect: vec4<f32>,
    pointer_time_focus: vec4<f32>,
    state: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> parameters: Parameters;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return output;
}

fn palette(t: f32) -> vec3<f32> {
    let a = vec3<f32>(0.10, 0.12, 0.20);
    let b = vec3<f32>(0.26, 0.38, 0.52);
    let c = vec3<f32>(0.72, 0.55, 0.92);
    return a + b * cos(6.28318 * (c * t + vec3<f32>(0.00, 0.12, 0.24)));
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = max(parameters.rect.zw, vec2<f32>(1.0));
    let uv = (position.xy - parameters.rect.xy) / size;
    let pointer = parameters.pointer_time_focus.xy / size;
    let time = parameters.pointer_time_focus.z;
    let focused = parameters.pointer_time_focus.w;
    let pulse = parameters.state.x;
    let resize_wave = parameters.state.y * 0.035;
    let scale_factor = parameters.state.z;

    let aspect = size.x / size.y;
    let centered = (uv - vec2<f32>(0.5)) * vec2<f32>(aspect, 1.0);
    let pointer_delta = (uv - pointer) * vec2<f32>(aspect, 1.0);
    let orbit = length(centered) - (0.24 + 0.035 * sin(time * 1.7));
    let ring = exp(-95.0 * abs(orbit));
    let pointer_glow = exp(-8.0 * length(pointer_delta));
    let grid_x = smoothstep(0.97, 1.0, cos((uv.x + resize_wave) * size.x / (22.0 * scale_factor)));
    let grid_y = smoothstep(0.97, 1.0, cos((uv.y - resize_wave) * size.y / (22.0 * scale_factor)));
    let grid = max(grid_x, grid_y) * 0.08;

    var color = palette(uv.x * 0.32 + uv.y * 0.18 + time * 0.045);
    color *= 0.45 + 0.35 * (1.0 - length(centered));
    color += vec3<f32>(0.15, 0.62, 1.00) * ring;
    color += vec3<f32>(0.95, 0.35, 0.75) * pointer_glow * (0.35 + pulse * 0.65);
    color += vec3<f32>(grid);
    color += focused * vec3<f32>(0.035, 0.055, 0.08);
    return vec4<f32>(color, 1.0);
}
"#;
