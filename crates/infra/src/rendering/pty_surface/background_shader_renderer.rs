use std::borrow::Cow;

use thiserror::Error;

const BACKGROUND_SHADER_PREFIX: &str = r#"
struct GerminalBackgroundUniforms {
    resolution: vec2<f32>,
    time: f32,
    opacity: f32,
};

@group(0) @binding(0)
var<uniform> germinal_background: GerminalBackgroundUniforms;

struct GerminalBackgroundVertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn germinal_background_vs(@builtin(vertex_index) vertex_index: u32) -> GerminalBackgroundVertexOutput {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var output: GerminalBackgroundVertexOutput;
    output.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    return output;
}
"#;

const BACKGROUND_SHADER_SUFFIX: &str = r#"
@fragment
fn germinal_background_fs(
    @builtin(position) position: vec4<f32>,
) -> @location(0) vec4<f32> {
    let resolution = max(germinal_background.resolution, vec2<f32>(1.0));
    let uv = position.xy / resolution;
    let color = background(uv, germinal_background.time, resolution);
    let alpha = clamp(color.a * germinal_background.opacity, 0.0, 1.0);
    return vec4<f32>(color.rgb * alpha, alpha);
}
"#;

pub const STARFIELD_BACKGROUND_WGSL: &str = include_str!("shaders/starfield_background.wgsl");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuBackgroundShaderSource {
    label: String,
    source: String,
    animated: bool,
}

impl WgpuBackgroundShaderSource {
    pub fn new(label: impl Into<String>, source: impl Into<String>, animated: bool) -> Self {
        Self {
            label: label.into(),
            source: source.into(),
            animated,
        }
    }

    pub fn starfield() -> Self {
        Self::new("starfield", STARFIELD_BACKGROUND_WGSL, true)
    }

    pub fn with_animated(mut self, animated: bool) -> Self {
        self.animated = animated;
        self
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub const fn animated(&self) -> bool {
        self.animated
    }
}

#[derive(Debug, Error)]
pub enum WgpuBackgroundShaderError {
    #[error("background shader {label:?} failed validation: {message}")]
    Validation { label: String, message: String },
}

pub struct WgpuBackgroundShaderRenderer {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl WgpuBackgroundShaderRenderer {
    pub async fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        source: &WgpuBackgroundShaderSource,
    ) -> Result<Self, WgpuBackgroundShaderError> {
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("germinal.background.uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("germinal.background.bind_group_layout"),
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
            label: Some("germinal.background.bind_group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("germinal.background.pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(source.label()),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(compose_background_shader(
                source.source(),
            ))),
        });
        let targets = [Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("germinal.background.pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("germinal_background_vs"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("germinal_background_fs"),
                targets: &targets,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        if let Some(error) = error_scope.pop().await {
            return Err(WgpuBackgroundShaderError::Validation {
                label: source.label().to_string(),
                message: error.to_string(),
            });
        }

        Ok(Self {
            pipeline,
            uniforms,
            bind_group,
        })
    }

    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        command_encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        frame: WgpuBackgroundShaderFrame,
    ) {
        queue.write_buffer(&self.uniforms, 0, &frame.uniform_bytes());
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
            label: Some("germinal.background.pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuBackgroundShaderFrame {
    pub width_px: u32,
    pub height_px: u32,
    pub elapsed_seconds: f32,
    pub opacity: f32,
}

impl WgpuBackgroundShaderFrame {
    fn uniform_bytes(self) -> [u8; 16] {
        let values = [
            self.width_px.max(1) as f32,
            self.height_px.max(1) as f32,
            self.elapsed_seconds,
            self.opacity.clamp(0.0, 1.0),
        ];
        let mut bytes = [0_u8; 16];
        for (index, value) in values.into_iter().enumerate() {
            let offset = index * size_of::<f32>();
            bytes[offset..offset + size_of::<f32>()].copy_from_slice(&value.to_ne_bytes());
        }
        bytes
    }
}

fn compose_background_shader(source: &str) -> String {
    let mut shader = String::with_capacity(
        BACKGROUND_SHADER_PREFIX.len() + source.len() + BACKGROUND_SHADER_SUFFIX.len() + 2,
    );
    shader.push_str(BACKGROUND_SHADER_PREFIX);
    shader.push('\n');
    shader.push_str(source);
    shader.push('\n');
    shader.push_str(BACKGROUND_SHADER_SUFFIX);
    shader
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starfield_shader_implements_valid_background_contract() {
        let shader = compose_background_shader(STARFIELD_BACKGROUND_WGSL);
        let module =
            wgpu::naga::front::wgsl::parse_str(&shader).expect("starfield shader should parse");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("starfield shader should validate");
    }

    #[test]
    fn background_uniforms_use_four_floats() {
        let bytes = WgpuBackgroundShaderFrame {
            width_px: 1920,
            height_px: 1080,
            elapsed_seconds: 3.5,
            opacity: 0.8,
        }
        .uniform_bytes();

        let values = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&chunk| f32::from_ne_bytes(chunk))
            .collect::<Vec<_>>();
        assert_eq!(values, [1920.0, 1080.0, 3.5, 0.8]);
    }
}
