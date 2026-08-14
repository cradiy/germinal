use std::{cell::RefCell, rc::Rc};

use germinal_ports::rendering::{
    render_target_id::RenderTargetId,
    surface_snapshot::{RenderSurfaceSnapshot, RenderSurfaceVideoSurfaceSnapshot},
};

use crate::rendering::pty_surface::{
    render_target_plan::WgpuTerminalRenderTargetPlan,
    renderer_backend::WgpuRendererConfig,
    video_surface_frame::{
        WgpuVideoSurfaceColorMatrix, WgpuVideoSurfaceColorProfile, WgpuVideoSurfaceColorRange,
        WgpuVideoSurfaceFrame,
    },
    video_surface_registry::WgpuVideoSurfaceRegistry,
};

const VIDEO_SURFACE_VERTEX_COUNT: u32 = 6;
const NV12_VIDEO_SURFACE_SHADER_WGSL: &str = r#"
@group(0) @binding(0)
var y_plane: texture_2d<f32>;

@group(0) @binding(1)
var uv_plane: texture_2d<f32>;

@group(0) @binding(2)
var plane_sampler: sampler;

struct ColorConversionUniform {
    row0: vec4<f32>,
    row1: vec4<f32>,
    row2: vec4<f32>,
    offset: vec4<f32>,
}

@group(0) @binding(3)
var<uniform> color_conversion: ColorConversionUniform;

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
    let y = textureSample(y_plane, plane_sampler, input.uv).r;
    let uv = textureSample(uv_plane, plane_sampler, input.uv).rg;
    let yuv = vec4<f32>(y, uv, 1.0) + color_conversion.offset;

    let rgb = vec3<f32>(
        dot(color_conversion.row0, yuv),
        dot(color_conversion.row1, yuv),
        dot(color_conversion.row2, yuv)
    );

    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

#[derive(Debug, Clone, Default)]
pub struct WgpuVideoSurfaceRenderer {
    inner: Rc<RefCell<Option<WgpuVideoSurfacePipeline>>>,
}

impl WgpuVideoSurfaceRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn prepare(
        &self,
        surface_snapshot: &RenderSurfaceSnapshot,
        render_target_plan: WgpuTerminalRenderTargetPlan,
        renderer_config: WgpuRendererConfig,
        registry: &WgpuVideoSurfaceRegistry,
    ) -> WgpuVideoSurfacePreparedFrame {
        let mut surfaces = Vec::new();

        if render_target_plan.is_empty() {
            return WgpuVideoSurfacePreparedFrame {
                target_id: surface_snapshot.target_id,
                seq: surface_snapshot.latest_seq,
                surfaces,
            };
        }

        for surface in &surface_snapshot.video_surfaces {
            let Some(frame) = registry.attached_frame(surface_snapshot.target_id, &surface.id)
            else {
                continue;
            };
            let WgpuVideoSurfaceFrame::Nv12Gpu(ref nv12_frame) = frame else {
                continue;
            };

            let Some(vertices) =
                vertices_for_surface(surface, nv12_frame, render_target_plan, renderer_config)
            else {
                continue;
            };

            surfaces.push(WgpuVideoSurfacePrepared { frame, vertices });
        }

        WgpuVideoSurfacePreparedFrame {
            target_id: surface_snapshot.target_id,
            seq: surface_snapshot.latest_seq,
            surfaces,
        }
    }

    pub(crate) fn encode_prepared_frame(
        &self,
        device: &wgpu::Device,
        command_encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        render_target_plan: WgpuTerminalRenderTargetPlan,
        prepared: &WgpuVideoSurfacePreparedFrame,
    ) -> WgpuVideoSurfaceRenderResult {
        if prepared.is_empty() || render_target_plan.is_empty() {
            return WgpuVideoSurfaceRenderResult {
                target_id: prepared.target_id,
                seq: prepared.seq,
                encoded: false,
                draw_count: 0,
                surface_count: 0,
            };
        }

        let buffer = create_vertex_buffer(device, prepared);

        let color_attachment = Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: store_op_of(render_target_plan.store),
            },
        });
        let color_attachments = [color_attachment];

        self.with_pipeline(device, color_format, |pipeline| {
            let mut render_pass = command_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("germinal.video_surface.render_pass"),
                color_attachments: &color_attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            render_target_plan.apply_viewport(&mut render_pass);
            render_pass.set_pipeline(&pipeline.render_pipeline);
            render_pass.set_vertex_buffer(0, buffer.slice(..));

            for (index, surface) in prepared.surfaces.iter().enumerate() {
                let bind_group = bind_group_for_surface(device, pipeline, &surface.frame);
                let first_vertex = (index as u32) * VIDEO_SURFACE_VERTEX_COUNT;
                render_pass.set_bind_group(0, &bind_group, &[]);
                render_pass.draw(
                    first_vertex..first_vertex + VIDEO_SURFACE_VERTEX_COUNT,
                    0..1,
                );
            }

            drop(render_pass);

            WgpuVideoSurfaceRenderResult {
                target_id: prepared.target_id,
                seq: prepared.seq,
                encoded: true,
                draw_count: prepared.surfaces.len(),
                surface_count: prepared.surfaces.len(),
            }
        })
    }

    fn with_pipeline<T>(
        &self,
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        f: impl FnOnce(&WgpuVideoSurfacePipeline) -> T,
    ) -> T {
        let mut inner = self.inner.borrow_mut();
        let needs_rebuild = inner
            .as_ref()
            .map(|pipeline| pipeline.color_format != color_format)
            .unwrap_or(true);

        if needs_rebuild {
            *inner = Some(WgpuVideoSurfacePipeline::new(device, color_format));
        }

        f(inner.as_ref().expect("video surface pipeline should exist"))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WgpuVideoSurfacePreparedFrame {
    pub target_id: RenderTargetId,
    pub seq: germinal_ports::seq::Seq,
    pub surfaces: Vec<WgpuVideoSurfacePrepared>,
}

impl WgpuVideoSurfacePreparedFrame {
    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WgpuVideoSurfacePrepared {
    frame: WgpuVideoSurfaceFrame,
    vertices: [WgpuVideoSurfaceVertex; VIDEO_SURFACE_VERTEX_COUNT as usize],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WgpuVideoSurfaceRenderResult {
    pub target_id: RenderTargetId,
    pub seq: germinal_ports::seq::Seq,
    pub encoded: bool,
    pub draw_count: usize,
    pub surface_count: usize,
}

impl WgpuVideoSurfaceRenderResult {
    pub fn encoded(&self) -> bool {
        self.encoded
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WgpuVideoSurfaceVertex {
    position_ndc: [f32; 2],
    uv: [f32; 2],
}

impl WgpuVideoSurfaceVertex {
    const BYTE_SIZE: usize = 16;

    fn to_ne_bytes(self) -> [u8; Self::BYTE_SIZE] {
        let mut bytes = [0u8; Self::BYTE_SIZE];
        bytes[0..4].copy_from_slice(&self.position_ndc[0].to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.position_ndc[1].to_ne_bytes());
        bytes[8..12].copy_from_slice(&self.uv[0].to_ne_bytes());
        bytes[12..16].copy_from_slice(&self.uv[1].to_ne_bytes());
        bytes
    }

    fn vertex_buffer_layout<'a>() -> wgpu::VertexBufferLayout<'a> {
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

#[derive(Debug, Clone, Copy, PartialEq)]
struct WgpuVideoSurfaceColorConversionUniform {
    row0: [f32; 4],
    row1: [f32; 4],
    row2: [f32; 4],
    offset: [f32; 4],
}

impl WgpuVideoSurfaceColorConversionUniform {
    const BYTE_SIZE: usize = 64;

    fn from_profile(profile: WgpuVideoSurfaceColorProfile) -> Self {
        match (profile.range, profile.matrix) {
            (WgpuVideoSurfaceColorRange::Full, WgpuVideoSurfaceColorMatrix::Bt601) => Self {
                row0: [1.0, 0.0, 1.402, 0.0],
                row1: [1.0, -0.344_136, -0.714_136, 0.0],
                row2: [1.0, 1.772, 0.0, 0.0],
                offset: [0.0, -0.5, -0.5, 0.0],
            },
            (WgpuVideoSurfaceColorRange::Full, WgpuVideoSurfaceColorMatrix::Bt709) => Self {
                row0: [1.0, 0.0, 1.574_8, 0.0],
                row1: [1.0, -0.187_324, -0.468_124, 0.0],
                row2: [1.0, 1.855_6, 0.0, 0.0],
                offset: [0.0, -0.5, -0.5, 0.0],
            },
            (WgpuVideoSurfaceColorRange::Limited, WgpuVideoSurfaceColorMatrix::Bt601) => Self {
                row0: [1.164_383_5, 0.0, 1.596_026_8, 0.0],
                row1: [1.164_383_5, -0.391_762_3, -0.812_967_7, 0.0],
                row2: [1.164_383_5, 2.017_232_2, 0.0, 0.0],
                offset: [-0.062_745_1, -0.5, -0.5, 0.0],
            },
            (WgpuVideoSurfaceColorRange::Limited, WgpuVideoSurfaceColorMatrix::Bt709) => Self {
                row0: [1.164_383_5, 0.0, 1.792_741_1, 0.0],
                row1: [1.164_383_5, -0.213_248_6, -0.532_909_33, 0.0],
                row2: [1.164_383_5, 2.112_401_7, 0.0, 0.0],
                offset: [-0.062_745_1, -0.5, -0.5, 0.0],
            },
        }
    }

    fn to_ne_bytes(self) -> [u8; Self::BYTE_SIZE] {
        let mut bytes = [0u8; Self::BYTE_SIZE];
        let mut offset = 0usize;
        for value in self
            .row0
            .into_iter()
            .chain(self.row1)
            .chain(self.row2)
            .chain(self.offset)
        {
            bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
            offset += 4;
        }
        bytes
    }
}

#[derive(Debug)]
struct WgpuVideoSurfacePipeline {
    color_format: wgpu::TextureFormat,
    render_pipeline: wgpu::RenderPipeline,
    texture_bind_group_layout: wgpu::BindGroupLayout,
}

impl WgpuVideoSurfacePipeline {
    fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("germinal.video_surface.shader"),
            source: wgpu::ShaderSource::Wgsl(NV12_VIDEO_SURFACE_SHADER_WGSL.into()),
        });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("germinal.video_surface.texture.bind_group_layout"),
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
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("germinal.video_surface.pipeline_layout"),
            bind_group_layouts: &[Some(&texture_bind_group_layout)],
            immediate_size: 0,
        });

        let color_targets = [Some(wgpu::ColorTargetState {
            format: color_format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("germinal.video_surface.render_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                buffers: &[Some(WgpuVideoSurfaceVertex::vertex_buffer_layout())],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                targets: &color_targets,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
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

fn bind_group_for_surface(
    device: &wgpu::Device,
    pipeline: &WgpuVideoSurfacePipeline,
    frame: &WgpuVideoSurfaceFrame,
) -> wgpu::BindGroup {
    match frame {
        WgpuVideoSurfaceFrame::Nv12Gpu(frame) => {
            let color_conversion = create_color_conversion_buffer(device, frame.color_profile);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("germinal.video_surface.texture.bind_group"),
                layout: &pipeline.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&frame.y_plane),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&frame.uv_plane),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&frame.plane_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: color_conversion.as_entire_binding(),
                    },
                ],
            })
        }
        #[cfg(target_os = "linux")]
        WgpuVideoSurfaceFrame::Nv12DmaBuf(_) => {
            unreachable!("prepare filters out dma_buf frames until the importer is wired")
        }
    }
}

fn create_color_conversion_buffer(
    device: &wgpu::Device,
    profile: WgpuVideoSurfaceColorProfile,
) -> wgpu::Buffer {
    let bytes = WgpuVideoSurfaceColorConversionUniform::from_profile(profile).to_ne_bytes();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("germinal.video_surface.color_conversion.uniform"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: true,
    });
    buffer
        .slice(..)
        .get_mapped_range_mut()
        .expect("mapped_at_creation buffer must provide mutable mapped range")
        .copy_from_slice(&bytes);
    buffer.unmap();
    buffer
}

fn create_vertex_buffer(
    device: &wgpu::Device,
    prepared: &WgpuVideoSurfacePreparedFrame,
) -> wgpu::Buffer {
    let mut bytes = Vec::with_capacity(
        prepared.surfaces.len()
            * VIDEO_SURFACE_VERTEX_COUNT as usize
            * WgpuVideoSurfaceVertex::BYTE_SIZE,
    );

    for surface in &prepared.surfaces {
        for vertex in surface.vertices {
            bytes.extend_from_slice(&vertex.to_ne_bytes());
        }
    }

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("germinal.video_surface.vertex_buffer"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::VERTEX,
        mapped_at_creation: true,
    });
    buffer
        .slice(..)
        .get_mapped_range_mut()
        .expect("mapped_at_creation buffer must provide mutable mapped range")
        .copy_from_slice(&bytes);
    buffer.unmap();
    buffer
}

fn vertices_for_surface(
    surface: &RenderSurfaceVideoSurfaceSnapshot,
    frame: &crate::rendering::pty_surface::video_surface_frame::WgpuVideoSurfaceNv12GpuFrame,
    render_target_plan: WgpuTerminalRenderTargetPlan,
    renderer_config: WgpuRendererConfig,
) -> Option<[WgpuVideoSurfaceVertex; VIDEO_SURFACE_VERTEX_COUNT as usize]> {
    let fitted =
        layout_video_surface_rect(surface, frame.width_px, frame.height_px, renderer_config)?;
    if fitted.width_px == 0 || fitted.height_px == 0 {
        return None;
    }

    let x0 = fitted.x_px as f32;
    let y0 = fitted.y_px as f32;
    let x1 = x0 + fitted.width_px as f32;
    let y1 = y0 + fitted.height_px as f32;
    let viewport_width = render_target_plan.viewport_width_px().max(1.0);
    let viewport_height = render_target_plan.viewport_height_px().max(1.0);
    let left = pixel_x_to_ndc(x0, viewport_width);
    let right = pixel_x_to_ndc(x1, viewport_width);
    let top = pixel_y_to_ndc(y0, viewport_height);
    let bottom = pixel_y_to_ndc(y1, viewport_height);

    Some([
        WgpuVideoSurfaceVertex {
            position_ndc: [left, top],
            uv: [0.0, 0.0],
        },
        WgpuVideoSurfaceVertex {
            position_ndc: [right, top],
            uv: [1.0, 0.0],
        },
        WgpuVideoSurfaceVertex {
            position_ndc: [right, bottom],
            uv: [1.0, 1.0],
        },
        WgpuVideoSurfaceVertex {
            position_ndc: [left, top],
            uv: [0.0, 0.0],
        },
        WgpuVideoSurfaceVertex {
            position_ndc: [right, bottom],
            uv: [1.0, 1.0],
        },
        WgpuVideoSurfaceVertex {
            position_ndc: [left, bottom],
            uv: [0.0, 1.0],
        },
    ])
}

fn layout_video_surface_rect(
    surface: &RenderSurfaceVideoSurfaceSnapshot,
    frame_width_px: u32,
    frame_height_px: u32,
    renderer_config: WgpuRendererConfig,
) -> Option<RenderSurfaceVideoSurfaceSnapshot> {
    if surface.width_px == 0
        || surface.height_px == 0
        || frame_width_px == 0
        || frame_height_px == 0
    {
        return None;
    }

    let scaled_surface = scale_video_surface_rect(surface, renderer_config);
    Some(fit_video_frame_rect(
        scaled_surface,
        frame_width_px,
        frame_height_px,
    ))
}

fn scale_video_surface_rect(
    surface: &RenderSurfaceVideoSurfaceSnapshot,
    config: WgpuRendererConfig,
) -> RenderSurfaceVideoSurfaceSnapshot {
    RenderSurfaceVideoSurfaceSnapshot {
        id: surface.id.clone(),
        x_px: config.content_origin_x
            + scale_virtual_px(
                surface.x_px,
                config.content_width_px,
                pixel_virtual_width_px(config),
            ),
        y_px: config.content_origin_y
            + scale_virtual_px(
                surface.y_px,
                config.content_height_px,
                pixel_virtual_height_px(config),
            ),
        width_px: scale_virtual_px(
            surface.width_px,
            config.content_width_px,
            pixel_virtual_width_px(config),
        ),
        height_px: scale_virtual_px(
            surface.height_px,
            config.content_height_px,
            pixel_virtual_height_px(config),
        ),
    }
}

fn fit_video_frame_rect(
    container: RenderSurfaceVideoSurfaceSnapshot,
    frame_width_px: u32,
    frame_height_px: u32,
) -> RenderSurfaceVideoSurfaceSnapshot {
    let container_width = container.width_px.max(1);
    let container_height = container.height_px.max(1);
    let frame_width = frame_width_px.max(1);
    let frame_height = frame_height_px.max(1);

    let width_limited_height = rounded_ratio(container_width, frame_height, frame_width);

    let (fit_width, fit_height) = if width_limited_height <= container_height {
        (
            container_width,
            width_limited_height.min(container_height).max(1),
        )
    } else {
        let fit_width =
            rounded_ratio(container_height, frame_width, frame_height).min(container_width);
        (fit_width.max(1), container_height)
    };

    let offset_x = container.width_px.saturating_sub(fit_width) / 2;
    let offset_y = container.height_px.saturating_sub(fit_height) / 2;

    RenderSurfaceVideoSurfaceSnapshot {
        id: container.id,
        x_px: container.x_px.saturating_add(offset_x),
        y_px: container.y_px.saturating_add(offset_y),
        width_px: fit_width,
        height_px: fit_height,
    }
}

fn rounded_ratio(lhs: u32, numerator: u32, denominator: u32) -> u32 {
    let denominator = u64::from(denominator.max(1));
    let scaled = u64::from(lhs) * u64::from(numerator);
    let rounded = (scaled + denominator / 2) / denominator;
    rounded.min(u64::from(u32::MAX)) as u32
}

fn scale_virtual_px(value: u32, actual_content_px: u32, virtual_content_px: u32) -> u32 {
    let scaled = u64::from(value) * u64::from(actual_content_px);
    let rounded =
        (scaled + u64::from(virtual_content_px / 2)) / u64::from(virtual_content_px.max(1));
    rounded.min(u64::from(u32::MAX)) as u32
}

fn pixel_virtual_width_px(config: WgpuRendererConfig) -> u32 {
    config.grid_columns.saturating_mul(8).max(1)
}

fn pixel_virtual_height_px(config: WgpuRendererConfig) -> u32 {
    config.grid_rows.saturating_mul(16).max(1)
}

fn pixel_x_to_ndc(x_px: f32, viewport_width_px: f32) -> f32 {
    (x_px / viewport_width_px) * 2.0 - 1.0
}

fn pixel_y_to_ndc(y_px: f32, viewport_height_px: f32) -> f32 {
    1.0 - (y_px / viewport_height_px) * 2.0
}

fn store_op_of(store: bool) -> wgpu::StoreOp {
    if store {
        wgpu::StoreOp::Store
    } else {
        wgpu::StoreOp::Discard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_video_surface_rect_to_ndc_vertices() {
        let fitted = layout_video_surface_rect(
            &RenderSurfaceVideoSurfaceSnapshot {
                id: "player".to_string(),
                x_px: 8,
                y_px: 16,
                width_px: 80,
                height_px: 48,
            },
            160,
            96,
            WgpuRendererConfig {
                cell_width_px: 8,
                cell_height_px: 16,
                content_origin_x: 8,
                content_origin_y: 16,
                content_width_px: 144,
                content_height_px: 80,
                grid_columns: 18,
                grid_rows: 5,
                blinking_cursor_visible: true,
            },
        )
        .expect("expected fitted rect");

        let vertices = quad_vertices_for_rect(fitted, WgpuTerminalRenderTargetPlan::new(160, 96));
        assert_vertex_close(vertices[0], [-0.8, 0.3333333], [0.0, 0.0]);
        assert_vertex_close(vertices[1], [0.2, 0.3333333], [1.0, 0.0]);
        assert_vertex_close(vertices[5], [-0.8, -0.6666667], [0.0, 1.0]);
    }

    #[test]
    fn skips_empty_video_surface_rects() {
        let fitted = layout_video_surface_rect(
            &RenderSurfaceVideoSurfaceSnapshot {
                id: "player".to_string(),
                x_px: 0,
                y_px: 0,
                width_px: 0,
                height_px: 10,
            },
            10,
            10,
            WgpuRendererConfig::default(),
        );

        assert!(fitted.is_none());
    }

    #[test]
    fn fits_video_frame_inside_surface_without_stretching() {
        let fitted = fit_video_frame_rect(
            RenderSurfaceVideoSurfaceSnapshot {
                id: "player".to_string(),
                x_px: 100,
                y_px: 50,
                width_px: 400,
                height_px: 200,
            },
            1920,
            1080,
        );

        assert_eq!(fitted.x_px, 122);
        assert_eq!(fitted.y_px, 50);
        assert_eq!(fitted.width_px, 356);
        assert_eq!(fitted.height_px, 200);
    }

    #[test]
    fn scales_video_surface_like_pixel_rects_before_ndc_mapping() {
        let scaled = scale_video_surface_rect(
            &RenderSurfaceVideoSurfaceSnapshot {
                id: "player".to_string(),
                x_px: 80,
                y_px: 160,
                width_px: 400,
                height_px: 320,
            },
            WgpuRendererConfig {
                cell_width_px: 10,
                cell_height_px: 20,
                content_origin_x: 12,
                content_origin_y: 18,
                content_width_px: 1000,
                content_height_px: 600,
                grid_columns: 100,
                grid_rows: 50,
                blinking_cursor_visible: true,
            },
        );

        assert_eq!(scaled.x_px, 112);
        assert_eq!(scaled.y_px, 138);
        assert_eq!(scaled.width_px, 500);
        assert_eq!(scaled.height_px, 240);
    }

    #[test]
    fn limited_bt709_color_profile_expands_video_range() {
        let uniform =
            WgpuVideoSurfaceColorConversionUniform::from_profile(WgpuVideoSurfaceColorProfile {
                range: WgpuVideoSurfaceColorRange::Limited,
                matrix: WgpuVideoSurfaceColorMatrix::Bt709,
            });

        assert!((uniform.row0[0] - 1.164_383_5).abs() < 0.0001);
        assert!((uniform.row0[2] - 1.792_741_1).abs() < 0.0001);
        assert!((uniform.offset[0] + 0.062_745_1).abs() < 0.0001);
        assert!((uniform.offset[1] + 0.5).abs() < 0.0001);
        assert!((uniform.offset[2] + 0.5).abs() < 0.0001);
    }

    #[test]
    fn full_bt601_color_profile_keeps_legacy_coefficients() {
        let uniform =
            WgpuVideoSurfaceColorConversionUniform::from_profile(WgpuVideoSurfaceColorProfile {
                range: WgpuVideoSurfaceColorRange::Full,
                matrix: WgpuVideoSurfaceColorMatrix::Bt601,
            });

        assert!((uniform.row0[0] - 1.0).abs() < 0.0001);
        assert!((uniform.row0[2] - 1.402).abs() < 0.0001);
        assert!((uniform.row1[1] + 0.344_136).abs() < 0.0001);
        assert!((uniform.offset[0] - 0.0).abs() < 0.0001);
        assert!((uniform.offset[1] + 0.5).abs() < 0.0001);
        assert!((uniform.offset[2] + 0.5).abs() < 0.0001);
    }

    fn quad_vertices_for_rect(
        rect: RenderSurfaceVideoSurfaceSnapshot,
        render_target_plan: WgpuTerminalRenderTargetPlan,
    ) -> [WgpuVideoSurfaceVertex; VIDEO_SURFACE_VERTEX_COUNT as usize] {
        let x0 = rect.x_px as f32;
        let y0 = rect.y_px as f32;
        let x1 = x0 + rect.width_px as f32;
        let y1 = y0 + rect.height_px as f32;
        let viewport_width = render_target_plan.viewport_width_px().max(1.0);
        let viewport_height = render_target_plan.viewport_height_px().max(1.0);
        let left = pixel_x_to_ndc(x0, viewport_width);
        let right = pixel_x_to_ndc(x1, viewport_width);
        let top = pixel_y_to_ndc(y0, viewport_height);
        let bottom = pixel_y_to_ndc(y1, viewport_height);

        [
            WgpuVideoSurfaceVertex {
                position_ndc: [left, top],
                uv: [0.0, 0.0],
            },
            WgpuVideoSurfaceVertex {
                position_ndc: [right, top],
                uv: [1.0, 0.0],
            },
            WgpuVideoSurfaceVertex {
                position_ndc: [right, bottom],
                uv: [1.0, 1.0],
            },
            WgpuVideoSurfaceVertex {
                position_ndc: [left, top],
                uv: [0.0, 0.0],
            },
            WgpuVideoSurfaceVertex {
                position_ndc: [right, bottom],
                uv: [1.0, 1.0],
            },
            WgpuVideoSurfaceVertex {
                position_ndc: [left, bottom],
                uv: [0.0, 1.0],
            },
        ]
    }

    fn assert_vertex_close(
        vertex: WgpuVideoSurfaceVertex,
        expected_position: [f32; 2],
        expected_uv: [f32; 2],
    ) {
        assert!((vertex.position_ndc[0] - expected_position[0]).abs() < 0.0001);
        assert!((vertex.position_ndc[1] - expected_position[1]).abs() < 0.0001);
        assert!((vertex.uv[0] - expected_uv[0]).abs() < f32::EPSILON);
        assert!((vertex.uv[1] - expected_uv[1]).abs() < f32::EPSILON);
    }
}
