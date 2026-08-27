use std::{
    cell::RefCell,
    sync::{Arc, LazyLock},
};

use germinal_ports::rendering::frame_plan_builder::RgbColorDto;

use crate::rendering::pty_surface::{
    glyph_atlas::{WgpuTerminalGlyphAtlas, WgpuTerminalGlyphAtlasEntry, WgpuTerminalGlyphKey},
    glyph_uv_mapper::WgpuTerminalGlyphUvMapResult,
    renderer_backend::{WgpuQuadDrawItem, WgpuQuadKind},
};

#[derive(Debug, Clone, Default)]
pub struct WgpuQuadVertexBufferBuilder {
    cached_indices: RefCell<Option<(usize, Arc<Vec<u32>>)>>,
}

impl WgpuQuadVertexBufferBuilder {
    pub fn new() -> Self {
        Self {
            cached_indices: RefCell::new(None),
        }
    }

    pub fn build(&self, quads: &[WgpuQuadDrawItem]) -> WgpuVertexBuffer {
        self.build_with_glyph_atlas(quads, &WgpuTerminalGlyphAtlas::empty())
            .0
    }

    pub fn build_with_glyph_atlas(
        &self,
        quads: &[WgpuQuadDrawItem],
        atlas: &WgpuTerminalGlyphAtlas,
    ) -> (WgpuVertexBuffer, WgpuTerminalGlyphUvMapResult) {
        let mut vertices = Vec::with_capacity(quads.len() * 4);
        let mut glyph_uv_map_result = WgpuTerminalGlyphUvMapResult::default();
        for quad in quads {
            vertices.extend(gpu_vertices_of_quad(*quad, atlas, &mut glyph_uv_map_result));
        }
        (
            WgpuVertexBuffer {
                vertices: Arc::new(vertices),
                indices: self.cached_indices_for_quad_count(quads.len()),
            },
            glyph_uv_map_result,
        )
    }

    fn cached_indices_for_quad_count(&self, quad_count: usize) -> Arc<Vec<u32>> {
        if let Some((cached_quad_count, cached_indices)) = self.cached_indices.borrow().as_ref()
            && *cached_quad_count == quad_count
        {
            return Arc::clone(cached_indices);
        }
        let indices = Arc::new(build_quad_indices(quad_count));
        *self.cached_indices.borrow_mut() = Some((quad_count, Arc::clone(&indices)));
        indices
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WgpuVertexBuffer {
    pub vertices: Arc<Vec<WgpuGpuVertex>>,
    pub indices: Arc<Vec<u32>>,
}
impl Default for WgpuVertexBuffer {
    fn default() -> Self {
        Self {
            vertices: Arc::new(Vec::new()),
            indices: Arc::new(Vec::new()),
        }
    }
}
impl WgpuVertexBuffer {
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() && self.indices.is_empty()
    }

    pub fn quad_count(&self) -> usize {
        self.indices.len() / 6
    }

    pub fn vertex_bytes(&self) -> &[u8] {
        bytes_of_slice(self.vertices.as_slice())
    }

    pub fn index_bytes(&self) -> &[u8] {
        bytes_of_slice(self.indices.as_slice())
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WgpuGpuVertex {
    pub position_px: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub kind: u32,
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
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 8,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 16,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x4,
            },
            wgpu::VertexAttribute {
                offset: 32,
                shader_location: 3,
                format: wgpu::VertexFormat::Uint32,
            },
            wgpu::VertexAttribute {
                offset: 36,
                shader_location: 4,
                format: wgpu::VertexFormat::Uint32,
            },
        ];
        wgpu::VertexBufferLayout {
            array_stride: Self::BYTE_SIZE as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBUTES,
        }
    }
}

pub const WGPU_VERTEX_KIND_BACKGROUND: u32 = 0;
pub const WGPU_VERTEX_KIND_GLYPH: u32 = 1;
pub const WGPU_VERTEX_KIND_UNDERLINE: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuVertex {
    pub x_px: f32,
    pub y_px: f32,
    pub u: f32,
    pub v: f32,
    pub color: WgpuVertexColor,
    pub kind: WgpuVertexKind,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuVertexColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}
impl WgpuVertexColor {
    pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    pub const fn white() -> Self {
        Self::new(255, 255, 255, 255)
    }

    pub const fn black() -> Self {
        Self::new(0, 0, 0, 255)
    }

    pub const fn transparent() -> Self {
        Self::new(0, 0, 0, 0)
    }

    pub const fn with_alpha(self, alpha: u8) -> Self {
        Self { alpha, ..self }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuVertexKind {
    Background,
    Glyph { glyph_key: WgpuTerminalGlyphKey },
    Underline,
    Geometric,
}

fn gpu_vertices_of_quad(
    quad: WgpuQuadDrawItem,
    atlas: &WgpuTerminalGlyphAtlas,
    glyph_uv_map_result: &mut WgpuTerminalGlyphUvMapResult,
) -> [WgpuGpuVertex; 4] {
    let (kind, glyph_codepoint) = gpu_kind_and_codepoint(kind_of_quad(quad.kind));
    let color = normalize_color(color_of_quad(quad));
    let (x0, y0, x1, y1, uv) = glyph_quad_geometry_and_uv(quad, atlas, glyph_uv_map_result)
        .unwrap_or((
            quad.x_px as f32,
            quad.y_px as f32,
            (quad.x_px + quad.width_px) as f32,
            (quad.y_px + quad.height_px) as f32,
            default_uv(),
        ));
    [
        WgpuGpuVertex {
            position_px: [x0, y0],
            uv: [uv[0], uv[1]],
            color,
            kind,
            glyph_codepoint,
        },
        WgpuGpuVertex {
            position_px: [x1, y0],
            uv: [uv[2], uv[1]],
            color,
            kind,
            glyph_codepoint,
        },
        WgpuGpuVertex {
            position_px: [x1, y1],
            uv: [uv[2], uv[3]],
            color,
            kind,
            glyph_codepoint,
        },
        WgpuGpuVertex {
            position_px: [x0, y1],
            uv: [uv[0], uv[3]],
            color,
            kind,
            glyph_codepoint,
        },
    ]
}

fn glyph_quad_geometry_and_uv(
    quad: WgpuQuadDrawItem,
    atlas: &WgpuTerminalGlyphAtlas,
    glyph_uv_map_result: &mut WgpuTerminalGlyphUvMapResult,
) -> Option<(f32, f32, f32, f32, [f32; 4])> {
    let WgpuQuadKind::Glyph { glyph_key } = quad.kind else {
        return None;
    };
    glyph_uv_map_result.glyph_vertices += 4;
    let Some(entry) = atlas.entry_for_key(glyph_key) else {
        glyph_uv_map_result.missing_vertices += 4;
        return None;
    };
    glyph_uv_map_result.mapped_vertices += 4;
    Some(mapped_glyph_quad_geometry_and_uv(quad, entry))
}
fn mapped_glyph_quad_geometry_and_uv(
    quad: WgpuQuadDrawItem,
    entry: &WgpuTerminalGlyphAtlasEntry,
) -> (f32, f32, f32, f32, [f32; 4]) {
    let (offset_x, offset_y, draw_width, draw_height) =
        glyph_draw_geometry(quad.available_width_px, entry);
    let x0 = quad.x_px as f32 + offset_x;
    let y0 = quad.y_px as f32 + offset_y;
    let x1 = x0 + draw_width;
    let y1 = y0 + draw_height;
    (
        x0,
        y0,
        x1,
        y1,
        [
            entry.uv.min_u,
            entry.uv.min_v,
            entry.uv.max_u,
            entry.uv.max_v,
        ],
    )
}

pub(crate) fn glyph_draw_geometry(
    available_width_px: u32,
    entry: &WgpuTerminalGlyphAtlasEntry,
) -> (f32, f32, f32, f32) {
    let native_width_px = entry.draw_width_px.max(1) as f32;
    let native_height_px = entry.draw_height_px.max(1) as f32;
    let native_offset_x_px = entry.draw_offset_x_px as f32;
    let native_offset_y_px = entry.draw_offset_y_px as f32;
    let maximum_width_px = available_width_px
        .saturating_sub(entry.draw_offset_x_px.max(0) as u32)
        .max(1) as f32;

    // The row layout supplies every blank cell available before the next occupied cell. Preserve
    // native font geometry whenever it fits; otherwise scale the entire glyph proportionally so it
    // cannot overwrite following text. The decision depends only on geometry and row occupancy.
    if native_width_px - maximum_width_px <= 2.0 {
        return (
            native_offset_x_px,
            native_offset_y_px,
            native_width_px,
            native_height_px,
        );
    }

    let scale = maximum_width_px / native_width_px;
    let draw_height_px = native_height_px * scale;
    let native_bottom_px = native_offset_y_px + native_height_px;
    (
        native_offset_x_px * scale,
        native_bottom_px - draw_height_px,
        maximum_width_px,
        draw_height_px,
    )
}
fn default_uv() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}
fn kind_of_quad(kind: WgpuQuadKind) -> WgpuVertexKind {
    match kind {
        WgpuQuadKind::Background | WgpuQuadKind::PixelRect { .. } => WgpuVertexKind::Background,
        WgpuQuadKind::Glyph { glyph_key } => WgpuVertexKind::Glyph { glyph_key },
        WgpuQuadKind::Underline => WgpuVertexKind::Underline,
        WgpuQuadKind::Geometric => WgpuVertexKind::Geometric,
    }
}
fn gpu_kind_and_codepoint(kind: WgpuVertexKind) -> (u32, u32) {
    match kind {
        WgpuVertexKind::Background => (WGPU_VERTEX_KIND_BACKGROUND, 0),
        WgpuVertexKind::Glyph { glyph_key } => (WGPU_VERTEX_KIND_GLYPH, glyph_key.packed_id()),
        WgpuVertexKind::Underline => (WGPU_VERTEX_KIND_UNDERLINE, 0),
        WgpuVertexKind::Geometric => (WGPU_VERTEX_KIND_UNDERLINE, 0),
    }
}
fn color_of_quad(quad: WgpuQuadDrawItem) -> WgpuVertexColor {
    match quad.kind {
        WgpuQuadKind::Background => {
            color_or(quad.style.background, WgpuVertexColor::transparent()).with_alpha(quad.alpha)
        }
        WgpuQuadKind::Glyph { .. } | WgpuQuadKind::Underline | WgpuQuadKind::Geometric => {
            color_or(quad.style.foreground, WgpuVertexColor::white())
        }
        WgpuQuadKind::PixelRect { color } => {
            WgpuVertexColor::new(color.red, color.green, color.blue, color.alpha)
        }
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
    static SRGB_TO_LINEAR_LUT: LazyLock<[f32; 256]> = LazyLock::new(|| {
        std::array::from_fn(|component| {
            let srgb = component as f32 / 255.0;
            if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            }
        })
    });
    SRGB_TO_LINEAR_LUT[component as usize]
}
fn build_quad_indices(quad_count: usize) -> Vec<u32> {
    let mut indices = Vec::with_capacity(quad_count * 6);
    for quad_index in 0..quad_count {
        let base = (quad_index * 4) as u32;
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    indices
}
fn bytes_of_slice<T>(items: &[T]) -> &[u8] {
    let byte_len = std::mem::size_of_val(items);
    unsafe { std::slice::from_raw_parts(items.as_ptr() as *const u8, byte_len) }
}

#[cfg(test)]
mod tests {
    use germinal_ports::rendering::frame_plan_builder::{RgbColorDto, TextStyleDto};

    use super::{WgpuQuadVertexBufferBuilder, glyph_draw_geometry};
    use crate::rendering::pty_surface::glyph_atlas::{
        WgpuTerminalGlyphAtlas, WgpuTerminalGlyphAtlasEntry, WgpuTerminalGlyphKey,
        WgpuTerminalGlyphUvRect,
    };
    use crate::rendering::pty_surface::renderer_backend::{WgpuQuadDrawItem, WgpuQuadKind};

    #[test]
    fn preserves_background_alpha_in_gpu_vertices() {
        let quad = WgpuQuadDrawItem {
            kind: WgpuQuadKind::Background,
            x_px: 0,
            y_px: 0,
            width_px: 80,
            available_width_px: 80,
            height_px: 24,
            style: TextStyleDto {
                background: Some(RgbColorDto::new(20, 40, 60)),
                ..TextStyleDto::plain()
            },
            alpha: 128,
        };

        let buffer = WgpuQuadVertexBufferBuilder::new().build(&[quad]);

        assert_eq!(buffer.vertices.len(), 4);
        assert!(
            buffer
                .vertices
                .iter()
                .all(|vertex| (vertex.color[3] - 128.0 / 255.0).abs() < f32::EPSILON)
        );
    }

    #[test]
    fn glyph_scaling_depends_on_available_geometry_not_codepoint() {
        let entry = WgpuTerminalGlyphAtlasEntry {
            codepoint: 0,
            x_px: 0,
            y_px: 0,
            width_px: 30,
            height_px: 20,
            advance_px: 18.0,
            uv: WgpuTerminalGlyphUvRect {
                min_u: 0.0,
                min_v: 0.0,
                max_u: 1.0,
                max_v: 1.0,
            },
            draw_offset_x_px: 0,
            draw_offset_y_px: 4,
            draw_width_px: 30,
            draw_height_px: 20,
            is_color: false,
        };

        assert_eq!(glyph_draw_geometry(36, &entry), (0.0, 4.0, 30.0, 20.0));
        assert_eq!(glyph_draw_geometry(18, &entry), (0.0, 12.0, 18.0, 12.0));
    }

    #[test]
    fn missing_glyph_fallback_uses_logical_width_not_blank_span() {
        let quad = WgpuQuadDrawItem {
            kind: WgpuQuadKind::Glyph {
                glyph_key: WgpuTerminalGlyphKey::plain('x'),
            },
            x_px: 10,
            y_px: 20,
            width_px: 18,
            available_width_px: 180,
            height_px: 30,
            style: TextStyleDto::plain(),
            alpha: u8::MAX,
        };

        let (buffer, result) = WgpuQuadVertexBufferBuilder::new()
            .build_with_glyph_atlas(&[quad], &WgpuTerminalGlyphAtlas::empty());

        assert!(result.has_missing());
        assert_eq!(
            buffer.vertices[1].position_px[0] - buffer.vertices[0].position_px[0],
            18.0
        );
    }
}
