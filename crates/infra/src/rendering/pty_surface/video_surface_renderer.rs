use std::{cell::RefCell, rc::Rc};

use germinal_ports::rendering::{
	render_target_id::RenderTargetId,
	surface_snapshot::{RenderSurfaceSnapshot, RenderSurfaceVideoSurfaceSnapshot},
};

use crate::rendering::pty_surface::{
	render_target_plan::WgpuTerminalRenderTargetPlan, renderer_backend::WgpuRendererConfig,
	video_surface_frame::WgpuVideoSurfaceFrame, video_surface_registry::WgpuVideoSurfaceRegistry,
};

const VIDEO_SURFACE_VERTEX_COUNT: u32 = 6;
const NV12_VIDEO_SURFACE_SHADER_WGSL: &str = r#"
@group(0) @binding(0)
var y_plane: texture_2d<f32>;

@group(0) @binding(1)
var uv_plane: texture_2d<f32>;

@group(0) @binding(2)
var plane_sampler: sampler;

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
    let uv = textureSample(uv_plane, plane_sampler, input.uv).rg - vec2<f32>(0.5, 0.5);

    let rgb = vec3<f32>(
        y + 1.402 * uv.y,
        y - 0.344136 * uv.x - 0.714136 * uv.y,
        y + 1.772 * uv.x
    );

    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

#[derive(Debug, Clone, Default)]
pub struct WgpuVideoSurfaceRenderer {
	inner: Rc<RefCell<Option<WgpuVideoSurfacePipeline>>>,
}

impl WgpuVideoSurfaceRenderer {
	pub fn new() -> Self { Self::default() }

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
			let Some(frame) = registry.attached_frame(surface_snapshot.target_id, &surface.id) else {
				continue;
			};
			if !matches!(frame, WgpuVideoSurfaceFrame::Nv12Gpu(_)) {
				continue;
			}

			let Some(vertices) = vertices_for_surface(surface, render_target_plan, renderer_config)
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
				target_id:     prepared.target_id,
				seq:           prepared.seq,
				encoded:       false,
				draw_count:    0,
				surface_count: 0,
			};
		}

		let buffer = create_vertex_buffer(device, prepared);

		let color_attachment = Some(wgpu::RenderPassColorAttachment {
			view:           target_view,
			depth_slice:    None,
			resolve_target: None,
			ops:            wgpu::Operations {
				load:  wgpu::LoadOp::Load,
				store: store_op_of(render_target_plan.store),
			},
		});
		let color_attachments = [color_attachment];

		self.with_pipeline(device, color_format, |pipeline| {
			let mut render_pass = command_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
				label:                    Some("germinal.video_surface.render_pass"),
				color_attachments:        &color_attachments,
				depth_stencil_attachment: None,
				timestamp_writes:         None,
				occlusion_query_set:      None,
				multiview_mask:           None,
			});

			render_pass.set_pipeline(&pipeline.render_pipeline);
			render_pass.set_vertex_buffer(0, buffer.slice(..));

			for (index, surface) in prepared.surfaces.iter().enumerate() {
				let bind_group = bind_group_for_surface(device, pipeline, &surface.frame);
				let first_vertex = (index as u32) * VIDEO_SURFACE_VERTEX_COUNT;
				render_pass.set_bind_group(0, &bind_group, &[]);
				render_pass.draw(first_vertex..first_vertex + VIDEO_SURFACE_VERTEX_COUNT, 0..1);
			}

			drop(render_pass);

			WgpuVideoSurfaceRenderResult {
				target_id:     prepared.target_id,
				seq:           prepared.seq,
				encoded:       true,
				draw_count:    prepared.surfaces.len(),
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
		let needs_rebuild =
			inner.as_ref().map(|pipeline| pipeline.color_format != color_format).unwrap_or(true);

		if needs_rebuild {
			*inner = Some(WgpuVideoSurfacePipeline::new(device, color_format));
		}

		f(inner.as_ref().expect("video surface pipeline should exist"))
	}
}

#[derive(Debug, Clone)]
pub(crate) struct WgpuVideoSurfacePreparedFrame {
	pub target_id: RenderTargetId,
	pub seq:       germinal_ports::seq::Seq,
	pub surfaces:  Vec<WgpuVideoSurfacePrepared>,
}

impl WgpuVideoSurfacePreparedFrame {
	pub fn is_empty(&self) -> bool { self.surfaces.is_empty() }
}

#[derive(Debug, Clone)]
pub(crate) struct WgpuVideoSurfacePrepared {
	frame:    WgpuVideoSurfaceFrame,
	vertices: [WgpuVideoSurfaceVertex; VIDEO_SURFACE_VERTEX_COUNT as usize],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WgpuVideoSurfaceRenderResult {
	pub target_id:     RenderTargetId,
	pub seq:           germinal_ports::seq::Seq,
	pub encoded:       bool,
	pub draw_count:    usize,
	pub surface_count: usize,
}

impl WgpuVideoSurfaceRenderResult {
	pub fn encoded(&self) -> bool { self.encoded }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WgpuVideoSurfaceVertex {
	position_ndc: [f32; 2],
	uv:           [f32; 2],
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
				format:          wgpu::VertexFormat::Float32x2,
				offset:          0,
				shader_location: 0,
			},
			wgpu::VertexAttribute {
				format:          wgpu::VertexFormat::Float32x2,
				offset:          8,
				shader_location: 1,
			},
		];

		wgpu::VertexBufferLayout {
			array_stride: Self::BYTE_SIZE as wgpu::BufferAddress,
			step_mode:    wgpu::VertexStepMode::Vertex,
			attributes:   &ATTRIBUTES,
		}
	}
}

#[derive(Debug)]
struct WgpuVideoSurfacePipeline {
	color_format:              wgpu::TextureFormat,
	render_pipeline:           wgpu::RenderPipeline,
	texture_bind_group_layout: wgpu::BindGroupLayout,
}

impl WgpuVideoSurfacePipeline {
	fn new(device: &wgpu::Device, color_format: wgpu::TextureFormat) -> Self {
		let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
			label:  Some("germinal.video_surface.shader"),
			source: wgpu::ShaderSource::Wgsl(NV12_VIDEO_SURFACE_SHADER_WGSL.into()),
		});

		let texture_bind_group_layout =
			device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
				label:   Some("germinal.video_surface.texture.bind_group_layout"),
				entries: &[
					wgpu::BindGroupLayoutEntry {
						binding:    0,
						visibility: wgpu::ShaderStages::FRAGMENT,
						ty:         wgpu::BindingType::Texture {
							sample_type:    wgpu::TextureSampleType::Float { filterable: true },
							view_dimension: wgpu::TextureViewDimension::D2,
							multisampled:   false,
						},
						count:      None,
					},
					wgpu::BindGroupLayoutEntry {
						binding:    1,
						visibility: wgpu::ShaderStages::FRAGMENT,
						ty:         wgpu::BindingType::Texture {
							sample_type:    wgpu::TextureSampleType::Float { filterable: true },
							view_dimension: wgpu::TextureViewDimension::D2,
							multisampled:   false,
						},
						count:      None,
					},
					wgpu::BindGroupLayoutEntry {
						binding:    2,
						visibility: wgpu::ShaderStages::FRAGMENT,
						ty:         wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
						count:      None,
					},
				],
			});

		let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
			label:              Some("germinal.video_surface.pipeline_layout"),
			bind_group_layouts: &[Some(&texture_bind_group_layout)],
			immediate_size:     0,
		});

		let color_targets = [Some(wgpu::ColorTargetState {
			format:     color_format,
			blend:      None,
			write_mask: wgpu::ColorWrites::ALL,
		})];

		let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
			label:          Some("germinal.video_surface.render_pipeline"),
			layout:         Some(&pipeline_layout),
			vertex:         wgpu::VertexState {
				module:              &shader_module,
				entry_point:         Some("vs_main"),
				buffers:             &[WgpuVideoSurfaceVertex::vertex_buffer_layout()],
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			},
			fragment:       Some(wgpu::FragmentState {
				module:              &shader_module,
				entry_point:         Some("fs_main"),
				targets:             &color_targets,
				compilation_options: wgpu::PipelineCompilationOptions::default(),
			}),
			primitive:      wgpu::PrimitiveState {
				topology:           wgpu::PrimitiveTopology::TriangleList,
				strip_index_format: None,
				front_face:         wgpu::FrontFace::Ccw,
				cull_mode:          None,
				polygon_mode:       wgpu::PolygonMode::Fill,
				unclipped_depth:    false,
				conservative:       false,
			},
			depth_stencil:  None,
			multisample:    wgpu::MultisampleState::default(),
			multiview_mask: None,
			cache:          None,
		});

		Self { color_format, render_pipeline, texture_bind_group_layout }
	}
}

fn bind_group_for_surface(
	device: &wgpu::Device,
	pipeline: &WgpuVideoSurfacePipeline,
	frame: &WgpuVideoSurfaceFrame,
) -> wgpu::BindGroup {
	match frame {
		WgpuVideoSurfaceFrame::Nv12Gpu(frame) => device.create_bind_group(&wgpu::BindGroupDescriptor {
			label:   Some("germinal.video_surface.texture.bind_group"),
			layout:  &pipeline.texture_bind_group_layout,
			entries: &[
				wgpu::BindGroupEntry {
					binding:  0,
					resource: wgpu::BindingResource::TextureView(&frame.y_plane),
				},
				wgpu::BindGroupEntry {
					binding:  1,
					resource: wgpu::BindingResource::TextureView(&frame.uv_plane),
				},
				wgpu::BindGroupEntry {
					binding:  2,
					resource: wgpu::BindingResource::Sampler(&frame.plane_sampler),
				},
			],
		}),
		#[cfg(target_os = "linux")]
		WgpuVideoSurfaceFrame::Nv12DmaBuf(_) => {
			unreachable!("prepare filters out dma_buf frames until the importer is wired")
		}
	}
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
		label:              Some("germinal.video_surface.vertex_buffer"),
		size:               bytes.len() as u64,
		usage:              wgpu::BufferUsages::VERTEX,
		mapped_at_creation: true,
	});
	buffer.slice(..).get_mapped_range_mut().copy_from_slice(&bytes);
	buffer.unmap();
	buffer
}

fn vertices_for_surface(
	surface: &RenderSurfaceVideoSurfaceSnapshot,
	render_target_plan: WgpuTerminalRenderTargetPlan,
	renderer_config: WgpuRendererConfig,
) -> Option<[WgpuVideoSurfaceVertex; VIDEO_SURFACE_VERTEX_COUNT as usize]> {
	if surface.width_px == 0 || surface.height_px == 0 {
		return None;
	}

	let x0 = renderer_config.content_origin_x.saturating_add(surface.x_px) as f32;
	let y0 = renderer_config.content_origin_y.saturating_add(surface.y_px) as f32;
	let x1 = x0 + surface.width_px as f32;
	let y1 = y0 + surface.height_px as f32;
	let viewport_width = render_target_plan.viewport_width_px().max(1.0);
	let viewport_height = render_target_plan.viewport_height_px().max(1.0);
	let left = pixel_x_to_ndc(x0, viewport_width);
	let right = pixel_x_to_ndc(x1, viewport_width);
	let top = pixel_y_to_ndc(y0, viewport_height);
	let bottom = pixel_y_to_ndc(y1, viewport_height);

	Some([
		WgpuVideoSurfaceVertex { position_ndc: [left, top], uv: [0.0, 0.0] },
		WgpuVideoSurfaceVertex { position_ndc: [right, top], uv: [1.0, 0.0] },
		WgpuVideoSurfaceVertex { position_ndc: [right, bottom], uv: [1.0, 1.0] },
		WgpuVideoSurfaceVertex { position_ndc: [left, top], uv: [0.0, 0.0] },
		WgpuVideoSurfaceVertex { position_ndc: [right, bottom], uv: [1.0, 1.0] },
		WgpuVideoSurfaceVertex { position_ndc: [left, bottom], uv: [0.0, 1.0] },
	])
}

fn pixel_x_to_ndc(x_px: f32, viewport_width_px: f32) -> f32 {
	(x_px / viewport_width_px) * 2.0 - 1.0
}

fn pixel_y_to_ndc(y_px: f32, viewport_height_px: f32) -> f32 {
	1.0 - (y_px / viewport_height_px) * 2.0
}

fn store_op_of(store: bool) -> wgpu::StoreOp {
	if store { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn maps_video_surface_rect_to_ndc_vertices() {
		let vertices = vertices_for_surface(
			&RenderSurfaceVideoSurfaceSnapshot {
				id:        "player".to_string(),
				x_px:      8,
				y_px:      16,
				width_px:  80,
				height_px: 48,
			},
			WgpuTerminalRenderTargetPlan::new(160, 96),
			WgpuRendererConfig {
				cell_width_px:     8,
				cell_height_px:    16,
				content_origin_x:  8,
				content_origin_y:  16,
				content_width_px:  144,
				content_height_px: 80,
				grid_columns:      18,
				grid_rows:         5,
			},
		);

		let vertices = vertices.expect("expected vertices");
		assert_vertex_close(vertices[0], [-0.8, 0.3333333], [0.0, 0.0]);
		assert_vertex_close(vertices[1], [0.2, 0.3333333], [1.0, 0.0]);
		assert_vertex_close(vertices[5], [-0.8, -0.6666667], [0.0, 1.0]);
	}

	#[test]
	fn skips_empty_video_surface_rects() {
		let vertices = vertices_for_surface(
			&RenderSurfaceVideoSurfaceSnapshot {
				id:        "player".to_string(),
				x_px:      0,
				y_px:      0,
				width_px:  0,
				height_px: 10,
			},
			WgpuTerminalRenderTargetPlan::new(160, 96),
			WgpuRendererConfig::default(),
		);

		assert!(vertices.is_none());
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
