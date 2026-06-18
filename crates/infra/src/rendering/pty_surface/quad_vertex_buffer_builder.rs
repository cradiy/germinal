use std::sync::Arc;

use germinal_ports::rendering::frame_plan_builder::RgbColorDto;

use crate::rendering::pty_surface::renderer_backend::{WgpuQuadDrawItem, WgpuQuadKind};

#[derive(Debug, Clone, Default)]
pub struct WgpuQuadVertexBufferBuilder;

impl WgpuQuadVertexBufferBuilder {
	pub fn new() -> Self { Self }

	pub fn build(&self, quads: &[WgpuQuadDrawItem]) -> WgpuVertexBuffer {
		let mut vertices = Vec::with_capacity(quads.len() * 4);
		let mut indices = Vec::with_capacity(quads.len() * 6);

		for quad in quads {
			let base = vertices.len() as u32;

			vertices.extend(gpu_vertices_of_quad(*quad));
			indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
		}

		WgpuVertexBuffer { vertices: Arc::from(vertices), indices: Arc::from(indices) }
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct WgpuVertexBuffer {
	pub vertices: Arc<[WgpuGpuVertex]>,
	pub indices:  Arc<[u32]>,
}

impl Default for WgpuVertexBuffer {
	fn default() -> Self {
		Self {
			vertices: Arc::from(Vec::<WgpuGpuVertex>::new()),
			indices:  Arc::from(Vec::<u32>::new()),
		}
	}
}

impl WgpuVertexBuffer {
	pub fn is_empty(&self) -> bool { self.vertices.is_empty() && self.indices.is_empty() }

	pub fn quad_count(&self) -> usize { self.indices.len() / 6 }

	pub fn vertex_bytes(&self) -> &[u8] { bytes_of_slice(&self.vertices) }

	pub fn index_bytes(&self) -> &[u8] { bytes_of_slice(&self.indices) }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WgpuGpuVertex {
	pub position_px:     [f32; 2],
	pub uv:              [f32; 2],
	pub color:           [f32; 4],
	pub kind:            u32,
	pub glyph_codepoint: u32,
}

impl WgpuGpuVertex {
	pub const BYTE_SIZE: usize = std::mem::size_of::<Self>();

	pub fn from_vertex(vertex: WgpuVertex) -> Self {
		let (kind, glyph_codepoint) = gpu_kind_and_codepoint(vertex.kind);

		Self {
			position_px: [vertex.x_px, vertex.y_px],
			uv: [vertex.u, vertex.v],
			color: normalize_color(vertex.color),
			kind,
			glyph_codepoint,
		}
	}

	pub fn vertex_buffer_layout<'a>() -> wgpu::VertexBufferLayout<'a> {
		const ATTRIBUTES: [wgpu::VertexAttribute; 5] = [
			wgpu::VertexAttribute {
				offset:          0,
				shader_location: 0,
				format:          wgpu::VertexFormat::Float32x2,
			},
			wgpu::VertexAttribute {
				offset:          8,
				shader_location: 1,
				format:          wgpu::VertexFormat::Float32x2,
			},
			wgpu::VertexAttribute {
				offset:          16,
				shader_location: 2,
				format:          wgpu::VertexFormat::Float32x4,
			},
			wgpu::VertexAttribute {
				offset:          32,
				shader_location: 3,
				format:          wgpu::VertexFormat::Uint32,
			},
			wgpu::VertexAttribute {
				offset:          36,
				shader_location: 4,
				format:          wgpu::VertexFormat::Uint32,
			},
		];

		wgpu::VertexBufferLayout {
			array_stride: Self::BYTE_SIZE as wgpu::BufferAddress,
			step_mode:    wgpu::VertexStepMode::Vertex,
			attributes:   &ATTRIBUTES,
		}
	}
}

pub const WGPU_VERTEX_KIND_BACKGROUND: u32 = 0;
pub const WGPU_VERTEX_KIND_GLYPH: u32 = 1;
pub const WGPU_VERTEX_KIND_UNDERLINE: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuVertex {
	pub x_px:  f32,
	pub y_px:  f32,
	pub u:     f32,
	pub v:     f32,
	pub color: WgpuVertexColor,
	pub kind:  WgpuVertexKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuVertexColor {
	pub red:   u8,
	pub green: u8,
	pub blue:  u8,
	pub alpha: u8,
}

impl WgpuVertexColor {
	pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
		Self { red, green, blue, alpha }
	}

	pub const fn white() -> Self { Self::new(255, 255, 255, 255) }

	pub const fn black() -> Self { Self::new(0, 0, 0, 255) }

	pub const fn transparent() -> Self { Self::new(0, 0, 0, 0) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuVertexKind {
	Background,
	Glyph { c: char },
	Underline,
}

fn gpu_vertices_of_quad(quad: WgpuQuadDrawItem) -> [WgpuGpuVertex; 4] {
	let x0 = quad.x_px as f32;
	let y0 = quad.y_px as f32;
	let x1 = (quad.x_px + quad.width_px) as f32;
	let y1 = (quad.y_px + quad.height_px) as f32;

	let color = normalize_color(color_of_quad(quad));
	let (kind, glyph_codepoint) = gpu_kind_and_codepoint(kind_of_quad(quad.kind));

	[
		WgpuGpuVertex { position_px: [x0, y0], uv: [0.0, 0.0], color, kind, glyph_codepoint },
		WgpuGpuVertex { position_px: [x1, y0], uv: [1.0, 0.0], color, kind, glyph_codepoint },
		WgpuGpuVertex { position_px: [x1, y1], uv: [1.0, 1.0], color, kind, glyph_codepoint },
		WgpuGpuVertex { position_px: [x0, y1], uv: [0.0, 1.0], color, kind, glyph_codepoint },
	]
}

fn kind_of_quad(kind: WgpuQuadKind) -> WgpuVertexKind {
	match kind {
		WgpuQuadKind::Background => WgpuVertexKind::Background,
		WgpuQuadKind::Glyph { c } => WgpuVertexKind::Glyph { c },
		WgpuQuadKind::Underline => WgpuVertexKind::Underline,
	}
}

fn gpu_kind_and_codepoint(kind: WgpuVertexKind) -> (u32, u32) {
	match kind {
		WgpuVertexKind::Background => (WGPU_VERTEX_KIND_BACKGROUND, 0),
		WgpuVertexKind::Glyph { c } => (WGPU_VERTEX_KIND_GLYPH, c as u32),
		WgpuVertexKind::Underline => (WGPU_VERTEX_KIND_UNDERLINE, 0),
	}
}

fn color_of_quad(quad: WgpuQuadDrawItem) -> WgpuVertexColor {
	match quad.kind {
		WgpuQuadKind::Background => color_or(quad.style.background, WgpuVertexColor::transparent()),
		WgpuQuadKind::Glyph { .. } => color_or(quad.style.foreground, WgpuVertexColor::white()),
		WgpuQuadKind::Underline => color_or(quad.style.foreground, WgpuVertexColor::white()),
	}
}

fn color_or(color: Option<RgbColorDto>, fallback: WgpuVertexColor) -> WgpuVertexColor {
	match color {
		Some(color) => WgpuVertexColor::new(color.red, color.green, color.blue, 255),
		None => fallback,
	}
}

fn normalize_color(color: WgpuVertexColor) -> [f32; 4] {
	[
		srgb_u8_to_linear_f32(color.red),
		srgb_u8_to_linear_f32(color.green),
		srgb_u8_to_linear_f32(color.blue),
		color.alpha as f32 / 255.0,
	]
}

fn srgb_u8_to_linear_f32(component: u8) -> f32 {
	let srgb = component as f32 / 255.0;

	if srgb <= 0.04045 { srgb / 12.92 } else { ((srgb + 0.055) / 1.055).powf(2.4) }
}

fn bytes_of_slice<T>(items: &[T]) -> &[u8] {
	let byte_len = std::mem::size_of_val(items);

	unsafe { std::slice::from_raw_parts(items.as_ptr() as *const u8, byte_len) }
}

#[cfg(test)]
mod tests {
	use germinal_ports::rendering::frame_plan_builder::TextStyleDto;

	use super::*;

	#[test]
	fn builds_four_gpu_vertices_and_six_indices_for_one_quad() {
		let builder = WgpuQuadVertexBufferBuilder::new();

		let buffer = builder.build(&[WgpuQuadDrawItem {
			kind:      WgpuQuadKind::Glyph { c: 'a' },
			x_px:      10,
			y_px:      20,
			width_px:  8,
			height_px: 16,
			style:     TextStyleDto::plain(),
		}]);

		assert_eq!(buffer.vertices.len(), 4);
		assert_eq!(buffer.indices.as_ref(), &[0, 1, 2, 0, 2, 3]);
		assert_eq!(buffer.quad_count(), 1);

		assert_eq!(buffer.vertices[0].position_px, [10.0, 20.0]);
		assert_eq!(buffer.vertices[0].uv, [0.0, 0.0]);
		assert_eq!(buffer.vertices[0].kind, WGPU_VERTEX_KIND_GLYPH);
		assert_eq!(buffer.vertices[0].glyph_codepoint, 'a' as u32);

		assert_eq!(buffer.vertices[1].position_px, [18.0, 20.0]);
		assert_eq!(buffer.vertices[2].position_px, [18.0, 36.0]);
		assert_eq!(buffer.vertices[3].position_px, [10.0, 36.0]);
	}

	#[test]
	fn appends_indices_for_multiple_quads() {
		let builder = WgpuQuadVertexBufferBuilder::new();

		let buffer = builder.build(&[
			WgpuQuadDrawItem {
				kind:      WgpuQuadKind::Glyph { c: 'a' },
				x_px:      0,
				y_px:      0,
				width_px:  8,
				height_px: 16,
				style:     TextStyleDto::plain(),
			},
			WgpuQuadDrawItem {
				kind:      WgpuQuadKind::Glyph { c: 'b' },
				x_px:      8,
				y_px:      0,
				width_px:  8,
				height_px: 16,
				style:     TextStyleDto::plain(),
			},
		]);

		assert_eq!(buffer.vertices.len(), 8);
		assert_eq!(buffer.indices.as_ref(), &[0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
		assert_eq!(buffer.quad_count(), 2);
	}

	#[test]
	fn preserves_vertex_layout() {
		let layout = WgpuGpuVertex::vertex_buffer_layout();

		assert_eq!(layout.array_stride, WgpuGpuVertex::BYTE_SIZE as wgpu::BufferAddress);

		assert_eq!(layout.step_mode, wgpu::VertexStepMode::Vertex);
		assert_eq!(layout.attributes.len(), 5);
		assert_eq!(layout.attributes[0].format, wgpu::VertexFormat::Float32x2);
		assert_eq!(layout.attributes[4].format, wgpu::VertexFormat::Uint32);
	}

	#[test]
	fn converts_srgb_vertex_colors_to_linear_rgb() {
		let vertex = WgpuGpuVertex::from_vertex(WgpuVertex {
			x_px:  0.0,
			y_px:  0.0,
			u:     0.0,
			v:     0.0,
			color: WgpuVertexColor::new(128, 64, 255, 128),
			kind:  WgpuVertexKind::Background,
		});

		assert!((vertex.color[0] - 0.215_860_53).abs() < 0.000_001);
		assert!((vertex.color[1] - 0.051_269_468).abs() < 0.000_001);
		assert!((vertex.color[2] - 1.0).abs() < 0.000_001);
		assert!((vertex.color[3] - (128.0 / 255.0)).abs() < 0.000_001);
	}
}
