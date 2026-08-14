use std::sync::Arc;

use crate::rendering::pty_surface::{
    glyph_atlas::{
        WgpuTerminalGlyphAtlas, WgpuTerminalGlyphAtlasEntry, WgpuTerminalGlyphKey,
        WgpuTerminalGlyphUvRect,
    },
    quad_vertex_buffer_builder::{WGPU_VERTEX_KIND_GLYPH, WgpuGpuVertex, WgpuVertexBuffer},
};

#[derive(Debug, Clone, Copy, Default)]
pub struct WgpuTerminalGlyphUvMapper;

impl WgpuTerminalGlyphUvMapper {
    pub fn new() -> Self {
        Self
    }

    pub fn apply_glyph_uvs(
        &self,
        vertex_buffer: &mut WgpuVertexBuffer,
        atlas: &WgpuTerminalGlyphAtlas,
    ) -> WgpuTerminalGlyphUvMapResult {
        let vertices = Arc::make_mut(&mut vertex_buffer.vertices);

        let mut glyph_vertices = 0usize;
        let mut mapped_vertices = 0usize;
        let mut missing_vertices = 0usize;

        let mut index = 0usize;

        while index + 3 < vertices.len() {
            if vertices[index].kind != WGPU_VERTEX_KIND_GLYPH {
                index += 4;
                continue;
            }

            glyph_vertices += 4;

            let Some(glyph_key) =
                WgpuTerminalGlyphKey::from_packed_id(vertices[index].glyph_codepoint)
            else {
                missing_vertices += 4;
                index += 4;
                continue;
            };

            let Some(entry) = atlas.entry_for_key(glyph_key) else {
                missing_vertices += 4;
                index += 4;
                continue;
            };

            write_quad_uv(&mut vertices[index..index + 4], entry.uv);
            write_quad_geometry(&mut vertices[index..index + 4], entry);

            mapped_vertices += 4;
            index += 4;
        }

        WgpuTerminalGlyphUvMapResult {
            glyph_vertices,
            mapped_vertices,
            missing_vertices,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WgpuTerminalGlyphUvMapResult {
    pub glyph_vertices: usize,
    pub mapped_vertices: usize,
    pub missing_vertices: usize,
}

impl WgpuTerminalGlyphUvMapResult {
    pub fn mapped_all(&self) -> bool {
        self.glyph_vertices > 0
            && self.mapped_vertices == self.glyph_vertices
            && self.missing_vertices == 0
    }

    pub fn has_missing(&self) -> bool {
        self.missing_vertices > 0
    }
}

fn write_quad_geometry(vertices: &mut [WgpuGpuVertex], entry: &WgpuTerminalGlyphAtlasEntry) {
    let base_x = vertices[0].position_px[0];
    let base_y = vertices[0].position_px[1];
    let x0 = base_x + entry.draw_offset_x_px as f32;
    let y0 = base_y + entry.draw_offset_y_px as f32;
    let x1 = x0 + entry.draw_width_px.max(1) as f32;
    let y1 = y0 + entry.draw_height_px.max(1) as f32;

    vertices[0].position_px = [x0, y0];
    vertices[1].position_px = [x1, y0];
    vertices[2].position_px = [x1, y1];
    vertices[3].position_px = [x0, y1];
}

fn write_quad_uv(vertices: &mut [WgpuGpuVertex], uv: WgpuTerminalGlyphUvRect) {
    vertices[0].uv = [uv.min_u, uv.min_v];
    vertices[1].uv = [uv.max_u, uv.min_v];
    vertices[2].uv = [uv.max_u, uv.max_v];
    vertices[3].uv = [uv.min_u, uv.max_v];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rendering::pty_surface::{
        glyph_atlas::WgpuDebugGlyphAtlasBuilder,
        quad_vertex_buffer_builder::{
            WGPU_VERTEX_KIND_BACKGROUND, WGPU_VERTEX_KIND_GLYPH, WgpuGpuVertex, WgpuVertexBuffer,
        },
    };

    #[test]
    fn maps_glyph_quad_uvs_from_atlas_entry() {
        let atlas = WgpuDebugGlyphAtlasBuilder::new().build_for_texts(["red"]);

        let mut vertex_buffer = WgpuVertexBuffer {
            vertices: Arc::from(vec![
                glyph_vertex('r'),
                glyph_vertex('r'),
                glyph_vertex('r'),
                glyph_vertex('r'),
            ]),
            indices: Arc::from(vec![0, 1, 2, 0, 2, 3]),
        };

        let mapper = WgpuTerminalGlyphUvMapper::new();
        let result = mapper.apply_glyph_uvs(&mut vertex_buffer, &atlas);

        assert_eq!(
            result,
            WgpuTerminalGlyphUvMapResult {
                glyph_vertices: 4,
                mapped_vertices: 4,
                missing_vertices: 0,
            }
        );

        assert!(result.mapped_all());

        let entry = atlas.entry('r').expect("r glyph should exist");

        assert_eq!(
            vertex_buffer.vertices[0].uv,
            [entry.uv.min_u, entry.uv.min_v]
        );
        assert_eq!(
            vertex_buffer.vertices[1].uv,
            [entry.uv.max_u, entry.uv.min_v]
        );
        assert_eq!(
            vertex_buffer.vertices[2].uv,
            [entry.uv.max_u, entry.uv.max_v]
        );
        assert_eq!(
            vertex_buffer.vertices[3].uv,
            [entry.uv.min_u, entry.uv.max_v]
        );
    }

    #[test]
    fn reports_missing_glyph_uvs() {
        let atlas = WgpuDebugGlyphAtlasBuilder::new().build_for_texts(["red"]);

        let mut vertex_buffer = WgpuVertexBuffer {
            vertices: Arc::from(vec![
                glyph_vertex('z'),
                glyph_vertex('z'),
                glyph_vertex('z'),
                glyph_vertex('z'),
            ]),
            indices: Arc::from(vec![0, 1, 2, 0, 2, 3]),
        };

        let mapper = WgpuTerminalGlyphUvMapper::new();
        let result = mapper.apply_glyph_uvs(&mut vertex_buffer, &atlas);

        assert_eq!(
            result,
            WgpuTerminalGlyphUvMapResult {
                glyph_vertices: 4,
                mapped_vertices: 0,
                missing_vertices: 4,
            }
        );

        assert!(result.has_missing());

        for vertex in vertex_buffer.vertices.iter() {
            assert_eq!(vertex.uv, [0.0, 0.0]);
        }
    }

    #[test]
    fn skips_non_glyph_quads() {
        let atlas = WgpuDebugGlyphAtlasBuilder::new().build_for_texts(["red"]);

        let mut vertex_buffer = WgpuVertexBuffer {
            vertices: Arc::from(vec![
                background_vertex(),
                background_vertex(),
                background_vertex(),
                background_vertex(),
            ]),
            indices: Arc::from(vec![0, 1, 2, 0, 2, 3]),
        };

        let mapper = WgpuTerminalGlyphUvMapper::new();
        let result = mapper.apply_glyph_uvs(&mut vertex_buffer, &atlas);

        assert_eq!(
            result,
            WgpuTerminalGlyphUvMapResult {
                glyph_vertices: 0,
                mapped_vertices: 0,
                missing_vertices: 0,
            }
        );
    }

    fn glyph_vertex(c: char) -> WgpuGpuVertex {
        WgpuGpuVertex {
            position_px: [0.0, 0.0],
            uv: [0.0, 0.0],
            color: [1.0, 1.0, 1.0, 1.0],
            kind: WGPU_VERTEX_KIND_GLYPH,
            glyph_codepoint: c as u32,
        }
    }

    fn background_vertex() -> WgpuGpuVertex {
        WgpuGpuVertex {
            position_px: [0.0, 0.0],
            uv: [0.0, 0.0],
            color: [0.0, 0.0, 0.0, 1.0],
            kind: WGPU_VERTEX_KIND_BACKGROUND,
            glyph_codepoint: 0,
        }
    }
}
