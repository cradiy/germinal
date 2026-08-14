use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use germinal_ports::{
    rendering::{
        render_target_id::RenderTargetId,
        surface_snapshot::{RenderSurfaceImageSnapshot, RenderSurfaceSnapshot},
    },
    seq::Seq,
};

use crate::rendering::pty_surface::{
    render_target_plan::WgpuTerminalRenderTargetPlan, renderer_backend::WgpuRendererConfig,
};

const IMAGE_VERTEX_COUNT: u32 = 6;
const IMAGE_SHADER_WGSL: &str = r#"
@group(0) @binding(0)
var image_texture: texture_2d<f32>;

@group(0) @binding(1)
var image_sampler: sampler;

struct VertexInput {
    @location(0) position_ndc: vec2<f32>,
    @location(1) uv: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position_ndc, 0.0, 1.0);
    output.uv = input.uv;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(image_texture, image_sampler, input.uv);
}
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WgpuImageLayer {
    BelowText,
    AboveText,
}

#[derive(Debug, Clone, Default)]
pub struct WgpuImageSurfaceRenderer {
    inner: Rc<RefCell<WgpuImageSurfaceRendererState>>,
}

#[derive(Debug, Default)]
struct WgpuImageSurfaceRendererState {
    pipeline: Option<WgpuImageSurfacePipeline>,
    textures: HashMap<(RenderTargetId, u64), WgpuImageTexture>,
}

impl WgpuImageSurfaceRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remove_render_target(&self, target_id: RenderTargetId) {
        self.inner
            .borrow_mut()
            .textures
            .retain(|(target, _), _| *target != target_id);
    }

    pub(crate) fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        snapshot: &RenderSurfaceSnapshot,
        plan: WgpuTerminalRenderTargetPlan,
        config: WgpuRendererConfig,
    ) -> WgpuImagePreparedFrame {
        self.sync_textures(device, queue, snapshot);
        let mut below_text = Vec::new();
        let mut above_text = Vec::new();

        if !plan.is_empty() {
            for image in &snapshot.image_surfaces {
                let Some(vertices) = vertices_for_image(image, plan, config) else {
                    continue;
                };
                let prepared = WgpuImagePrepared {
                    generation: image.image_generation,
                    vertices,
                };
                if image.z_index < 0 {
                    below_text.push(prepared);
                } else {
                    above_text.push(prepared);
                }
            }
        }

        WgpuImagePreparedFrame {
            target_id: snapshot.target_id,
            seq: snapshot.latest_seq,
            below_text,
            above_text,
        }
    }

    pub(crate) fn encode_layer(
        &self,
        context: WgpuImageEncodeContext<'_>,
        prepared: &WgpuImagePreparedFrame,
        layer: WgpuImageLayer,
    ) -> WgpuImageRenderResult {
        let WgpuImageEncodeContext {
            device,
            encoder,
            target_view,
            color_format,
            plan,
            load_op,
        } = context;
        let surfaces = match layer {
            WgpuImageLayer::BelowText => &prepared.below_text,
            WgpuImageLayer::AboveText => &prepared.above_text,
        };
        if surfaces.is_empty() || plan.is_empty() {
            return WgpuImageRenderResult::empty(prepared.target_id, prepared.seq);
        }

        let vertex_buffer = create_vertex_buffer(device, surfaces);
        let mut inner = self.inner.borrow_mut();
        let rebuild = inner
            .pipeline
            .as_ref()
            .map(|pipeline| pipeline.color_format != color_format)
            .unwrap_or(true);
        if rebuild {
            inner.pipeline = Some(WgpuImageSurfacePipeline::new(device, color_format));
        }
        let pipeline = inner.pipeline.as_ref().expect("image pipeline must exist");
        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: load_op,
                store: if plan.store {
                    wgpu::StoreOp::Store
                } else {
                    wgpu::StoreOp::Discard
                },
            },
        })];
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("germinal.image_surface.render_pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        plan.apply_viewport(&mut render_pass);
        render_pass.set_pipeline(&pipeline.render_pipeline);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));

        for (index, surface) in surfaces.iter().enumerate() {
            let Some(texture) = inner
                .textures
                .get(&(prepared.target_id, surface.generation))
            else {
                continue;
            };
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("germinal.image_surface.bind_group"),
                layout: &pipeline.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&texture.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&texture.sampler),
                    },
                ],
            });
            let first = index as u32 * IMAGE_VERTEX_COUNT;
            render_pass.set_bind_group(0, &bind_group, &[]);
            render_pass.draw(first..first + IMAGE_VERTEX_COUNT, 0..1);
        }
        drop(render_pass);

        WgpuImageRenderResult {
            target_id: prepared.target_id,
            seq: prepared.seq,
            encoded: true,
            draw_count: surfaces.len(),
            surface_count: surfaces.len(),
        }
    }

    fn sync_textures(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        snapshot: &RenderSurfaceSnapshot,
    ) {
        let live: HashSet<u64> = snapshot
            .image_surfaces
            .iter()
            .map(|image| image.image_generation)
            .collect();
        let mut inner = self.inner.borrow_mut();
        inner.textures.retain(|(target, generation), _| {
            *target != snapshot.target_id || live.contains(generation)
        });

        for image in &snapshot.image_surfaces {
            let key = (snapshot.target_id, image.image_generation);
            inner
                .textures
                .entry(key)
                .or_insert_with(|| upload_texture(device, queue, image));
        }
    }
}

pub(crate) struct WgpuImageEncodeContext<'a> {
    pub device: &'a wgpu::Device,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub target_view: &'a wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    pub plan: WgpuTerminalRenderTargetPlan,
    pub load_op: wgpu::LoadOp<wgpu::Color>,
}

#[derive(Debug, Clone)]
pub(crate) struct WgpuImagePreparedFrame {
    pub target_id: RenderTargetId,
    pub seq: Seq,
    below_text: Vec<WgpuImagePrepared>,
    above_text: Vec<WgpuImagePrepared>,
}

impl WgpuImagePreparedFrame {
    pub fn is_empty(&self) -> bool {
        self.below_text.is_empty() && self.above_text.is_empty()
    }
}

#[derive(Debug, Clone)]
struct WgpuImagePrepared {
    generation: u64,
    vertices: [WgpuImageVertex; IMAGE_VERTEX_COUNT as usize],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WgpuImageRenderResult {
    pub target_id: RenderTargetId,
    pub seq: Seq,
    pub encoded: bool,
    pub draw_count: usize,
    pub surface_count: usize,
}

impl WgpuImageRenderResult {
    fn empty(target_id: RenderTargetId, seq: Seq) -> Self {
        Self {
            target_id,
            seq,
            encoded: false,
            draw_count: 0,
            surface_count: 0,
        }
    }
}

#[derive(Debug)]
struct WgpuImageTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    sampler: wgpu::Sampler,
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &RenderSurfaceImageSnapshot,
) -> WgpuImageTexture {
    let size = wgpu::Extent3d {
        width: image.image_width_px,
        height: image.image_height_px,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("germinal.image_surface.texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &image.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.image_width_px * 4),
            rows_per_image: Some(image.image_height_px),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("germinal.image_surface.sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    WgpuImageTexture {
        _texture: texture,
        view,
        sampler,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WgpuImageVertex {
    position_ndc: [f32; 2],
    uv: [f32; 2],
}

impl WgpuImageVertex {
    const BYTE_SIZE: usize = 16;

    fn to_ne_bytes(self) -> [u8; Self::BYTE_SIZE] {
        let mut bytes = [0; Self::BYTE_SIZE];
        bytes[0..4].copy_from_slice(&self.position_ndc[0].to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.position_ndc[1].to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.uv[0].to_ne_bytes());
        bytes[12..16].copy_from_slice(&self.uv[1].to_ne_bytes());
        bytes
    }

    fn layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 8,
                shader_location: 1,
            },
        ];
        wgpu::VertexBufferLayout {
            array_stride: Self::BYTE_SIZE as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBUTES,
        }
    }
}

#[derive(Debug)]
struct WgpuImageSurfacePipeline {
    color_format: wgpu::TextureFormat,
    render_pipeline: wgpu::RenderPipeline,
    texture_bind_group_layout: wgpu::BindGroupLayout,
}

impl WgpuImageSurfacePipeline {
    fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("germinal.image_surface.shader"),
            source: wgpu::ShaderSource::Wgsl(IMAGE_SHADER_WGSL.into()),
        });
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("germinal.image_surface.bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("germinal.image_surface.pipeline_layout"),
            bind_group_layouts: &[Some(&texture_bind_group_layout)],
            immediate_size: 0,
        });
        let targets = [Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("germinal.image_surface.pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(WgpuImageVertex::layout())],
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
            color_format,
            render_pipeline,
            texture_bind_group_layout,
        }
    }
}

fn create_vertex_buffer(device: &wgpu::Device, surfaces: &[WgpuImagePrepared]) -> wgpu::Buffer {
    let mut bytes = Vec::with_capacity(
        surfaces.len() * IMAGE_VERTEX_COUNT as usize * WgpuImageVertex::BYTE_SIZE,
    );
    for surface in surfaces {
        for vertex in surface.vertices {
            bytes.extend_from_slice(&vertex.to_ne_bytes());
        }
    }
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("germinal.image_surface.vertices"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: true,
    });
    buffer
        .slice(..)
        .get_mapped_range_mut()
        .expect("mapped image vertex buffer must be writable")
        .copy_from_slice(&bytes);
    buffer.unmap();
    buffer
}

fn vertices_for_image(
    image: &RenderSurfaceImageSnapshot,
    plan: WgpuTerminalRenderTargetPlan,
    config: WgpuRendererConfig,
) -> Option<[WgpuImageVertex; IMAGE_VERTEX_COUNT as usize]> {
    if image.source_width_px == 0 || image.source_height_px == 0 {
        return None;
    }
    let width = if image.columns == 0 {
        image.source_width_px
    } else {
        image.columns.saturating_mul(config.cell_width_px)
    };
    let height = if image.rows == 0 {
        image.source_height_px
    } else {
        image.rows.saturating_mul(config.cell_height_px)
    };
    if width == 0 || height == 0 {
        return None;
    }

    let raw_left = config
        .content_origin_x
        .saturating_add(image.x_cell.saturating_mul(config.cell_width_px))
        .saturating_add(image.x_offset_px) as f32;
    let raw_top = config
        .content_origin_y
        .saturating_add(image.y_cell.saturating_mul(config.cell_height_px))
        .saturating_add(image.y_offset_px) as f32;
    let raw_right = raw_left + width as f32;
    let raw_bottom = raw_top + height as f32;
    let content_right = config
        .content_origin_x
        .saturating_add(config.content_width_px) as f32;
    let content_bottom = config
        .content_origin_y
        .saturating_add(config.content_height_px) as f32;
    let left_px = raw_left.max(config.content_origin_x as f32);
    let top_px = raw_top.max(config.content_origin_y as f32);
    let right_px = raw_right.min(content_right);
    let bottom_px = raw_bottom.min(content_bottom);
    if left_px >= right_px || top_px >= bottom_px {
        return None;
    }

    let source_left = image.source_x_px as f32 / image.image_width_px.max(1) as f32;
    let source_top = image.source_y_px as f32 / image.image_height_px.max(1) as f32;
    let source_width = image.source_width_px as f32 / image.image_width_px.max(1) as f32;
    let source_height = image.source_height_px as f32 / image.image_height_px.max(1) as f32;
    let u0 = source_left + ((left_px - raw_left) / width as f32) * source_width;
    let v0 = source_top + ((top_px - raw_top) / height as f32) * source_height;
    let u1 = source_left + ((right_px - raw_left) / width as f32) * source_width;
    let v1 = source_top + ((bottom_px - raw_top) / height as f32) * source_height;
    let viewport_width = plan.viewport_width_px().max(1.0);
    let viewport_height = plan.viewport_height_px().max(1.0);
    let left = left_px / viewport_width * 2.0 - 1.0;
    let right = right_px / viewport_width * 2.0 - 1.0;
    let top = 1.0 - top_px / viewport_height * 2.0;
    let bottom = 1.0 - bottom_px / viewport_height * 2.0;

    Some([
        WgpuImageVertex {
            position_ndc: [left, top],
            uv: [u0, v0],
        },
        WgpuImageVertex {
            position_ndc: [right, top],
            uv: [u1, v0],
        },
        WgpuImageVertex {
            position_ndc: [right, bottom],
            uv: [u1, v1],
        },
        WgpuImageVertex {
            position_ndc: [left, top],
            uv: [u0, v0],
        },
        WgpuImageVertex {
            position_ndc: [right, bottom],
            uv: [u1, v1],
        },
        WgpuImageVertex {
            position_ndc: [left, bottom],
            uv: [u0, v1],
        },
    ])
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn image() -> RenderSurfaceImageSnapshot {
        RenderSurfaceImageSnapshot {
            id: "1:1".to_owned(),
            image_generation: 1,
            x_cell: 2,
            y_cell: 3,
            x_offset_px: 0,
            y_offset_px: 0,
            columns: 4,
            rows: 2,
            source_x_px: 0,
            source_y_px: 0,
            source_width_px: 40,
            source_height_px: 20,
            image_width_px: 40,
            image_height_px: 20,
            z_index: 0,
            rgba: Arc::from(vec![0; 40 * 20 * 4]),
        }
    }

    #[test]
    fn maps_cell_placement_to_viewport_vertices() {
        let vertices = vertices_for_image(
            &image(),
            WgpuTerminalRenderTargetPlan::new(100, 100),
            WgpuRendererConfig {
                cell_width_px: 10,
                cell_height_px: 10,
                content_origin_x: 0,
                content_origin_y: 0,
                content_width_px: 100,
                content_height_px: 100,
                grid_columns: 10,
                grid_rows: 10,
                blinking_cursor_visible: true,
                ..WgpuRendererConfig::default()
            },
        )
        .unwrap();

        assert!((vertices[0].position_ndc[0] + 0.6).abs() < f32::EPSILON);
        assert!((vertices[0].position_ndc[1] - 0.4).abs() < 1e-6);
        assert!((vertices[2].position_ndc[0] - 0.2).abs() < 1e-6);
        assert!(vertices[2].position_ndc[1].abs() < f32::EPSILON);
        assert_eq!(vertices[0].uv, [0.0, 0.0]);
        assert_eq!(vertices[2].uv, [1.0, 1.0]);
    }

    #[test]
    fn clips_placement_and_source_uv_to_content_area() {
        let mut image = image();
        image.x_cell = 8;
        let vertices = vertices_for_image(
            &image,
            WgpuTerminalRenderTargetPlan::new(100, 100),
            WgpuRendererConfig {
                cell_width_px: 10,
                cell_height_px: 10,
                content_origin_x: 0,
                content_origin_y: 0,
                content_width_px: 100,
                content_height_px: 100,
                grid_columns: 10,
                grid_rows: 10,
                blinking_cursor_visible: true,
                ..WgpuRendererConfig::default()
            },
        )
        .unwrap();

        assert!((vertices[1].position_ndc[0] - 1.0).abs() < f32::EPSILON);
        assert!((vertices[1].position_ndc[1] - 0.4).abs() < 1e-6);
        assert_eq!(vertices[1].uv, [0.5, 0.0]);
    }
}
