use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use germinal_ports::{
    pty_host::{
        cell_size::TerminalCellSize, render_viewport::TerminalRenderViewport,
        size_info::TerminalSizeInfo, terminal_geometric_glyph::TerminalGeometricGlyph,
        width::terminal_char_cell_width,
    },
    rendering::{
        frame_plan_builder::{
            RenderCommandDto, RgbColorDto, RgbaColorDto, TextStyleDto,
            decode_pixel_fill_rect_command,
        },
        render_target_id::RenderTargetId,
        renderer_backend::RendererBackend,
        surface_snapshot::{
            RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot, RenderSurfaceRowSnapshot,
            RenderSurfaceSnapshot,
        },
    },
    seq::Seq,
};

const CURSOR_COLOR: RgbColorDto = RgbColorDto::new(235, 235, 235);
const PIXEL_RECT_VIRTUAL_CELL_WIDTH_PX: u32 = 8;
const PIXEL_RECT_VIRTUAL_CELL_HEIGHT_PX: u32 = 16;

#[derive(Debug, Clone)]
pub struct WgpuRendererBackend {
    inner: RefCell<WgpuRendererState>,
}

impl WgpuRendererBackend {
    pub fn new(config: WgpuRendererConfig) -> Self {
        Self {
            inner: RefCell::new(WgpuRendererState {
                config,
                ..WgpuRendererState::default()
            }),
        }
    }

    pub fn config(&self) -> WgpuRendererConfig {
        self.inner.borrow().config
    }

    pub fn with_quads<T>(&self, f: impl FnOnce(&[WgpuQuadDrawItem]) -> T) -> T {
        let inner = self.inner.borrow();
        f(inner.quads())
    }

    pub fn state(&self) -> WgpuRendererState {
        self.inner.borrow().clone()
    }
}

impl RendererBackend for WgpuRendererBackend {
    fn render_surface(&self, snapshot: &RenderSurfaceSnapshot) {
        let mut inner = self.inner.borrow_mut();
        let config = inner.config;
        let full_rerender =
            snapshot.dirty_rows.is_empty() || inner.last_target_id != Some(snapshot.target_id);
        let mut pixel_quads = Vec::new();
        let snapshot_rows: BTreeMap<u32, &_> = snapshot
            .rows
            .iter()
            .filter_map(|row| {
                if append_pixel_rect_quads_from_row(&mut pixel_quads, row, config) {
                    None
                } else {
                    Some((row.y, row))
                }
            })
            .collect();
        let dirty_rows: BTreeSet<u32> = if full_rerender {
            snapshot_rows.keys().copied().collect()
        } else {
            snapshot.dirty_rows.iter().copied().collect()
        };

        if full_rerender {
            inner.rendered_rows.clear();
            inner.draw_rows.clear();
        }

        for row_y in dirty_rows {
            if let Some(row) = snapshot_rows.get(&row_y) {
                let rendered_row = render_row(row, config);
                inner
                    .draw_rows
                    .insert(row_y, Arc::clone(&rendered_row.draw_row));
                inner.rendered_rows.insert(row_y, rendered_row);
            } else {
                inner.draw_rows.remove(&row_y);
                inner.rendered_rows.remove(&row_y);
            }
        }

        let mut cursor_quads = Vec::new();
        if let Some(cursor) = snapshot.cursor {
            append_cursor_quads(&mut cursor_quads, cursor, config);
        }

        let total_row_quads: usize = inner
            .rendered_rows
            .values()
            .map(|row| {
                row.background_quads.len() + row.glyph_quads.len() + row.underline_quads.len()
            })
            .sum();
        let mut quads =
            Vec::with_capacity(1 + pixel_quads.len() + total_row_quads + cursor_quads.len());
        quads.push(WgpuQuadDrawItem::surface_background(
            config,
            snapshot.default_background,
        ));
        quads.extend(pixel_quads);
        for row in inner.rendered_rows.values() {
            quads.extend(row.background_quads.iter().copied());
        }
        for row in inner.rendered_rows.values() {
            quads.extend(row.glyph_quads.iter().copied());
        }
        for row in inner.rendered_rows.values() {
            quads.extend(row.underline_quads.iter().copied());
        }
        quads.extend(cursor_quads);

        inner.render_count += 1;
        inner.last_target_id = Some(snapshot.target_id);
        inner.last_seq = Some(snapshot.latest_seq);
        inner.quads = quads;
    }
}

fn append_pixel_rect_quads_from_row(
    quads: &mut Vec<WgpuQuadDrawItem>,
    row: &RenderSurfaceRowSnapshot,
    config: WgpuRendererConfig,
) -> bool {
    let mut found = false;
    for run in &row.runs {
        if let Some(RenderCommandDto::PixelFillRect {
            x_px,
            y_px,
            width_px,
            height_px,
            color,
        }) = decode_pixel_fill_rect_command(&run.text)
        {
            let x_px = config.content_origin_x
                + scale_virtual_px(
                    x_px,
                    config.content_width_px,
                    config.pixel_virtual_width_px(),
                );
            let y_px = config.content_origin_y
                + scale_virtual_px(
                    y_px,
                    config.content_height_px,
                    config.pixel_virtual_height_px(),
                );
            let width_px = scale_virtual_px(
                width_px,
                config.content_width_px,
                config.pixel_virtual_width_px(),
            );
            let height_px = scale_virtual_px(
                height_px,
                config.content_height_px,
                config.pixel_virtual_height_px(),
            );
            quads.push(WgpuQuadDrawItem::pixel_rect(
                x_px, y_px, width_px, height_px, color,
            ));
            found = true;
        }
    }
    found
}

fn scale_virtual_px(value: u32, actual_content_px: u32, virtual_content_px: u32) -> u32 {
    let scaled = u64::from(value) * u64::from(actual_content_px);
    let rounded =
        (scaled + u64::from(virtual_content_px / 2)) / u64::from(virtual_content_px.max(1));
    rounded.min(u64::from(u32::MAX)) as u32
}

fn render_row(row: &RenderSurfaceRowSnapshot, config: WgpuRendererConfig) -> WgpuRenderedRow {
    let mut draw_row = WgpuDrawRow {
        y: row.y,
        glyphs: Vec::new(),
    };
    let mut background_quads = Vec::new();
    let mut glyph_quads = Vec::new();
    let mut underline_quads = Vec::new();

    for run in &row.runs {
        let mut x = run.x;
        for c in run.text.chars() {
            let cell_width = terminal_char_cell_width(c);
            if cell_width == 0 {
                continue;
            }
            let glyph = WgpuGlyphDrawItem {
                x,
                y: row.y,
                c,
                cell_width,
                style: run.style,
            };
            draw_row.glyphs.push(glyph);
            if run.style.background.is_some() {
                background_quads.push(WgpuQuadDrawItem::background(
                    x, row.y, cell_width, config, run.style,
                ));
            }
            if let Some(geometric_glyph) = TerminalGeometricGlyph::from_char(c) {
                append_terminal_geometric_glyph_quads(
                    &mut glyph_quads,
                    glyph,
                    config,
                    geometric_glyph,
                );
            } else if c != ' ' {
                glyph_quads.push(WgpuQuadDrawItem::glyph(glyph, config));
            }
            if run.style.underline {
                underline_quads.push(WgpuQuadDrawItem::underline(
                    x, row.y, cell_width, config, run.style,
                ));
            }
            x += cell_width;
        }
    }
    WgpuRenderedRow {
        draw_row: Arc::new(draw_row),
        background_quads,
        glyph_quads,
        underline_quads,
    }
}

fn append_terminal_geometric_glyph_quads(
    quads: &mut Vec<WgpuQuadDrawItem>,
    glyph: WgpuGlyphDrawItem,
    config: WgpuRendererConfig,
    geometric_glyph: TerminalGeometricGlyph,
) {
    let glyph_x = glyph.pixel_x(config);
    let glyph_y = glyph.pixel_y(config);
    for rect in geometric_glyph.pixel_rects(config.cell_size_for_row(glyph.y), glyph.cell_width) {
        quads.push(WgpuQuadDrawItem::solid_rect(
            glyph_x + rect.x_px(),
            glyph_y + rect.y_px(),
            rect.width_px(),
            rect.height_px(),
            glyph.style,
        ));
    }
}

fn append_cursor_quads(
    quads: &mut Vec<WgpuQuadDrawItem>,
    cursor: RenderSurfaceCursorSnapshot,
    config: WgpuRendererConfig,
) {
    if !cursor.focused {
        return;
    }

    let x = config.content_origin_x + cursor.x * config.cell_width_px;
    let y = config.row_top_px(cursor.y);
    let w = config.cell_width_px.max(1);
    let h = config.row_height_px(cursor.y);
    let style = TextStyleDto {
        foreground: Some(CURSOR_COLOR),
        background: None,
        bold: false,
        italic: false,
        underline: false,
    };
    let vertical_stroke = w.div_ceil(8).max(1);
    let horizontal_stroke = h.div_ceil(8).max(1);
    match cursor.shape {
        RenderSurfaceCursorShape::Block => {
            quads.push(WgpuQuadDrawItem::solid_rect(x, y, w, h, style));
        }
        RenderSurfaceCursorShape::Underline => {
            quads.push(WgpuQuadDrawItem::solid_rect(
                x,
                y + h.saturating_sub(horizontal_stroke),
                w,
                horizontal_stroke,
                style,
            ));
        }
        RenderSurfaceCursorShape::Beam => {
            quads.push(WgpuQuadDrawItem::solid_rect(
                x,
                y,
                vertical_stroke,
                h,
                style,
            ));
        }
        RenderSurfaceCursorShape::HollowBlock => {
            quads.push(WgpuQuadDrawItem::solid_rect(
                x,
                y,
                w,
                horizontal_stroke,
                style,
            ));
            quads.push(WgpuQuadDrawItem::solid_rect(
                x,
                y + h.saturating_sub(horizontal_stroke),
                w,
                horizontal_stroke,
                style,
            ));
            quads.push(WgpuQuadDrawItem::solid_rect(
                x,
                y,
                vertical_stroke,
                h,
                style,
            ));
            quads.push(WgpuQuadDrawItem::solid_rect(
                x + w.saturating_sub(vertical_stroke),
                y,
                vertical_stroke,
                h,
                style,
            ));
        }
        RenderSurfaceCursorShape::Hidden => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuRendererConfig {
    pub cell_width_px: u32,
    pub cell_height_px: u32,
    pub content_origin_x: u32,
    pub content_origin_y: u32,
    pub content_width_px: u32,
    pub content_height_px: u32,
    pub grid_columns: u32,
    pub grid_rows: u32,
}
impl WgpuRendererConfig {
    pub fn from_render_viewport(viewport: TerminalRenderViewport) -> Self {
        let cell_size = viewport.cell_size();
        Self {
            cell_width_px: cell_size.width_px(),
            cell_height_px: cell_size.height_px(),
            content_origin_x: viewport.origin_x_px(),
            content_origin_y: viewport.origin_y_px(),
            content_width_px: viewport.grid_width_px(),
            content_height_px: viewport.grid_height_px(),
            grid_columns: viewport.columns() as u32,
            grid_rows: viewport.rows() as u32,
        }
    }

    pub fn from_size_info(size_info: TerminalSizeInfo) -> Self {
        let viewport = size_info.render_viewport();
        let cell_size = viewport.cell_size();
        Self {
            cell_width_px: cell_size.width_px(),
            cell_height_px: cell_size.height_px(),
            content_origin_x: viewport.origin_x_px(),
            content_origin_y: viewport.origin_y_px(),
            content_width_px: size_info.content_width_px(),
            content_height_px: size_info.content_height_px(),
            grid_columns: viewport.columns() as u32,
            grid_rows: viewport.rows() as u32,
        }
    }

    fn pixel_virtual_width_px(self) -> u32 {
        self.grid_columns
            .saturating_mul(PIXEL_RECT_VIRTUAL_CELL_WIDTH_PX)
            .max(1)
    }

    fn pixel_virtual_height_px(self) -> u32 {
        self.grid_rows
            .saturating_mul(PIXEL_RECT_VIRTUAL_CELL_HEIGHT_PX)
            .max(1)
    }

    fn cell_size_for_row(self, row: u32) -> TerminalCellSize {
        TerminalCellSize::new(self.cell_width_px, self.row_height_px(row))
    }

    fn row_top_px(self, row: u32) -> u32 {
        if row >= self.grid_rows.max(1) {
            return self
                .content_origin_y
                .saturating_add(row.saturating_mul(self.cell_height_px));
        }

        self.content_origin_y
            .saturating_add(self.row_offset_px(row))
    }

    fn row_height_px(self, row: u32) -> u32 {
        if row >= self.grid_rows.max(1) {
            return self.cell_height_px.max(1);
        }

        self.row_offset_px(row.saturating_add(1))
            .saturating_sub(self.row_offset_px(row))
            .max(1)
    }

    fn row_offset_px(self, row: u32) -> u32 {
        let rows = self.grid_rows.max(1);
        let row = row.min(rows);
        ((u64::from(row) * u64::from(self.content_height_px)) / u64::from(rows))
            .min(u64::from(u32::MAX)) as u32
    }
}
impl From<TerminalRenderViewport> for WgpuRendererConfig {
    fn from(viewport: TerminalRenderViewport) -> Self {
        Self::from_render_viewport(viewport)
    }
}
impl From<TerminalSizeInfo> for WgpuRendererConfig {
    fn from(size_info: TerminalSizeInfo) -> Self {
        Self::from_size_info(size_info)
    }
}
impl Default for WgpuRendererConfig {
    fn default() -> Self {
        Self {
            cell_width_px: 8,
            cell_height_px: 16,
            content_origin_x: 0,
            content_origin_y: 0,
            content_width_px: 8,
            content_height_px: 16,
            grid_columns: 1,
            grid_rows: 1,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WgpuRendererState {
    pub config: WgpuRendererConfig,
    pub render_count: u64,
    pub last_target_id: Option<RenderTargetId>,
    pub last_seq: Option<Seq>,
    rendered_rows: BTreeMap<u32, WgpuRenderedRow>,
    draw_rows: BTreeMap<u32, Arc<WgpuDrawRow>>,
    quads: Vec<WgpuQuadDrawItem>,
}
impl WgpuRendererState {
    pub fn row(&self, y: u32) -> Option<&WgpuDrawRow> {
        self.draw_rows.get(&y).map(Arc::as_ref)
    }

    pub fn rows(&self) -> &BTreeMap<u32, Arc<WgpuDrawRow>> {
        &self.draw_rows
    }

    pub fn glyphs(&self) -> Vec<WgpuGlyphDrawItem> {
        self.draw_rows
            .values()
            .flat_map(|row| row.glyphs.iter().copied())
            .collect()
    }

    pub fn quads(&self) -> &[WgpuQuadDrawItem] {
        &self.quads
    }

    pub fn background_quads(&self) -> Vec<WgpuQuadDrawItem> {
        self.quads
            .iter()
            .copied()
            .filter(|quad| quad.kind == WgpuQuadKind::Background)
            .collect()
    }

    pub fn glyph_quads(&self) -> Vec<WgpuQuadDrawItem> {
        self.quads
            .iter()
            .copied()
            .filter(|quad| matches!(quad.kind, WgpuQuadKind::Glyph { .. }))
            .collect()
    }

    pub fn underline_quads(&self) -> Vec<WgpuQuadDrawItem> {
        self.quads
            .iter()
            .copied()
            .filter(|quad| quad.kind == WgpuQuadKind::Underline)
            .collect()
    }

    pub fn geometric_quads(&self) -> Vec<WgpuQuadDrawItem> {
        self.quads
            .iter()
            .copied()
            .filter(|quad| quad.kind == WgpuQuadKind::Geometric)
            .collect()
    }

    pub fn pixel_rect_quads(&self) -> Vec<WgpuQuadDrawItem> {
        self.quads
            .iter()
            .copied()
            .filter(|quad| matches!(quad.kind, WgpuQuadKind::PixelRect { .. }))
            .collect()
    }

    pub fn line_texts(&self) -> Vec<String> {
        self.draw_rows.values().map(|row| row.text()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WgpuRenderedRow {
    draw_row: Arc<WgpuDrawRow>,
    background_quads: Vec<WgpuQuadDrawItem>,
    glyph_quads: Vec<WgpuQuadDrawItem>,
    underline_quads: Vec<WgpuQuadDrawItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuDrawRow {
    pub y: u32,
    glyphs: Vec<WgpuGlyphDrawItem>,
}
impl WgpuDrawRow {
    pub fn glyphs(&self) -> &[WgpuGlyphDrawItem] {
        &self.glyphs
    }

    pub fn text(&self) -> String {
        let mut chars = Vec::new();
        for glyph in &self.glyphs {
            let index = glyph.x as usize;
            while chars.len() < index {
                chars.push(' ');
            }
            if index < chars.len() {
                chars[index] = glyph.c;
            } else {
                chars.push(glyph.c);
            }
        }
        chars.into_iter().collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuGlyphDrawItem {
    pub x: u32,
    pub y: u32,
    pub c: char,
    pub cell_width: u32,
    pub style: TextStyleDto,
}
impl WgpuGlyphDrawItem {
    pub fn pixel_x(&self, config: WgpuRendererConfig) -> u32 {
        config.content_origin_x + self.x * config.cell_width_px
    }

    pub fn pixel_y(&self, config: WgpuRendererConfig) -> u32 {
        config.row_top_px(self.y)
    }

    pub fn pixel_width(&self, config: WgpuRendererConfig) -> u32 {
        self.cell_width.max(1) * config.cell_width_px
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuQuadDrawItem {
    pub kind: WgpuQuadKind,
    pub x_px: u32,
    pub y_px: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub style: TextStyleDto,
}
impl WgpuQuadDrawItem {
    fn surface_background(config: WgpuRendererConfig, color: RgbColorDto) -> Self {
        Self {
            kind: WgpuQuadKind::Background,
            x_px: 0,
            y_px: 0,
            width_px: config
                .content_width_px
                .saturating_add(config.content_origin_x.saturating_mul(2)),
            height_px: config
                .content_height_px
                .saturating_add(config.content_origin_y.saturating_mul(2)),
            style: TextStyleDto {
                background: Some(color),
                ..TextStyleDto::plain()
            },
        }
    }

    pub fn glyph(glyph: WgpuGlyphDrawItem, config: WgpuRendererConfig) -> Self {
        Self {
            kind: WgpuQuadKind::Glyph {
                c: glyph.c,
                bold: glyph.style.bold,
            },
            x_px: glyph.pixel_x(config),
            y_px: glyph.pixel_y(config),
            width_px: glyph.pixel_width(config),
            height_px: config.row_height_px(glyph.y),
            style: glyph.style,
        }
    }

    pub fn background(
        x: u32,
        y: u32,
        cell_width: u32,
        config: WgpuRendererConfig,
        style: TextStyleDto,
    ) -> Self {
        Self {
            kind: WgpuQuadKind::Background,
            x_px: config.content_origin_x + x * config.cell_width_px,
            y_px: config.row_top_px(y),
            width_px: cell_width.max(1) * config.cell_width_px,
            height_px: config.row_height_px(y),
            style,
        }
    }

    pub fn underline(
        x: u32,
        y: u32,
        cell_width: u32,
        config: WgpuRendererConfig,
        style: TextStyleDto,
    ) -> Self {
        Self {
            kind: WgpuQuadKind::Underline,
            x_px: config.content_origin_x + x * config.cell_width_px,
            y_px: config
                .row_top_px(y)
                .saturating_add(config.row_height_px(y).saturating_sub(2)),
            width_px: cell_width.max(1) * config.cell_width_px,
            height_px: 1,
            style,
        }
    }

    pub fn solid_rect(
        x_px: u32,
        y_px: u32,
        width_px: u32,
        height_px: u32,
        style: TextStyleDto,
    ) -> Self {
        Self {
            kind: WgpuQuadKind::Geometric,
            x_px,
            y_px,
            width_px: width_px.max(1),
            height_px: height_px.max(1),
            style,
        }
    }

    pub fn pixel_rect(
        x_px: u32,
        y_px: u32,
        width_px: u32,
        height_px: u32,
        color: RgbaColorDto,
    ) -> Self {
        Self {
            kind: WgpuQuadKind::PixelRect { color },
            x_px,
            y_px,
            width_px: width_px.max(1),
            height_px: height_px.max(1),
            style: TextStyleDto::plain(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuQuadKind {
    Background,
    Glyph { c: char, bold: bool },
    Underline,
    Geometric,
    PixelRect { color: RgbaColorDto },
}

#[cfg(test)]
mod tests {
    use germinal_ports::{
        pty_host::{
            cell_size::TerminalCellSize,
            size_info::{TerminalPadding, TerminalSizeInfo},
            window_size::TerminalWindowSize,
        },
        rendering::{
            frame_plan_builder::{
                RenderCommandDto, RgbColorDto, RgbaColorDto, TextStyleDto,
                encode_pixel_fill_rect_command,
            },
            render_target_id::RenderTargetId,
            renderer_backend::RendererBackend,
            surface_snapshot::{
                RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot, RenderSurfaceRowSnapshot,
                RenderSurfaceRunSnapshot, RenderSurfaceSnapshot,
            },
        },
        seq::Seq,
    };

    use super::{
        WgpuQuadDrawItem, WgpuQuadKind, WgpuRendererBackend, WgpuRendererConfig,
        append_pixel_rect_quads_from_row,
    };

    fn pixel_row(command: RenderCommandDto) -> RenderSurfaceRowSnapshot {
        let text = encode_pixel_fill_rect_command(&command).expect("pixel command should encode");
        RenderSurfaceRowSnapshot {
            y: 0,
            runs: vec![RenderSurfaceRunSnapshot {
                x: 0,
                text,
                style: Default::default(),
            }],
        }
    }

    #[test]
    fn pixel_rects_fill_full_content_size_when_window_has_partial_cells() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(1000, 610),
            TerminalCellSize::new(12, 24),
            TerminalPadding::ZERO,
        );
        let config = WgpuRendererConfig::from(size_info);
        let row = pixel_row(RenderCommandDto::PixelFillRect {
            x_px: 0,
            y_px: 0,
            width_px: size_info.grid_size().columns() as u32 * 8,
            height_px: size_info.grid_size().rows() as u32 * 16,
            color: RgbaColorDto::opaque(1, 2, 3),
        });
        let mut quads = Vec::new();

        assert!(append_pixel_rect_quads_from_row(&mut quads, &row, config));
        assert_eq!(quads.len(), 1);
        assert_eq!(
            quads[0].kind,
            WgpuQuadKind::PixelRect {
                color: RgbaColorDto::opaque(1, 2, 3)
            }
        );
        assert_eq!(quads[0].x_px, 0);
        assert_eq!(quads[0].y_px, 0);
        assert_eq!(quads[0].width_px, 1000);
        assert_eq!(quads[0].height_px, 610);
    }

    #[test]
    fn pixel_rects_respect_padding_origin_while_filling_content_area() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(1000, 610),
            TerminalCellSize::new(12, 24),
            TerminalPadding::new(10, 6),
        );
        let config = WgpuRendererConfig::from(size_info);
        let row = pixel_row(RenderCommandDto::PixelFillRect {
            x_px: 0,
            y_px: 0,
            width_px: size_info.grid_size().columns() as u32 * 8,
            height_px: size_info.grid_size().rows() as u32 * 16,
            color: RgbaColorDto::opaque(9, 8, 7),
        });
        let mut quads = Vec::new();

        assert!(append_pixel_rect_quads_from_row(&mut quads, &row, config));
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].x_px, 10);
        assert_eq!(quads[0].y_px, 6);
        assert_eq!(quads[0].width_px, 980);
        assert_eq!(quads[0].height_px, 598);
    }

    #[test]
    fn surface_background_covers_partial_cell_remainder() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(100, 35),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let backend = WgpuRendererBackend::new(WgpuRendererConfig::from(size_info));
        let background = RgbColorDto::new(20, 21, 30);
        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: background,
            rows: vec![],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: None,
        });

        let backgrounds = backend.state().background_quads();
        assert_eq!(backgrounds.len(), 1);
        assert_eq!(backgrounds[0].x_px, 0);
        assert_eq!(backgrounds[0].y_px, 0);
        assert_eq!(backgrounds[0].width_px, 100);
        assert_eq!(backgrounds[0].height_px, 35);
        assert_eq!(backgrounds[0].style.background, Some(background));
    }

    #[test]
    fn rows_distribute_partial_cell_remainder_to_reach_the_viewport_bottom() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(100, 35),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let config = WgpuRendererConfig::from(size_info);

        assert_eq!(config.grid_rows, 2);
        assert_eq!(config.row_top_px(0), 0);
        assert_eq!(config.row_height_px(0), 17);
        assert_eq!(config.row_top_px(1), 17);
        assert_eq!(config.row_height_px(1), 18);
        assert_eq!(config.row_top_px(1) + config.row_height_px(1), 35);
    }

    #[test]
    fn last_row_background_reaches_the_viewport_bottom() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(100, 35),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let config = WgpuRendererConfig::from(size_info);
        let quad = WgpuQuadDrawItem::background(
            0,
            1,
            12,
            config,
            TextStyleDto {
                background: Some(RgbColorDto::new(20, 21, 30)),
                ..TextStyleDto::plain()
            },
        );

        assert_eq!(quad.y_px, 17);
        assert_eq!(quad.height_px, 18);
        assert_eq!(quad.y_px + quad.height_px, 35);
    }

    #[test]
    fn focused_cursor_renders_as_a_solid_cell() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());

        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: Some(RenderSurfaceCursorSnapshot {
                x: 2,
                y: 3,
                focused: true,
                shape: RenderSurfaceCursorShape::Block,
            }),
        });

        let cursors = backend.state().geometric_quads();
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].x_px, 16);
        assert_eq!(cursors[0].y_px, 48);
        assert_eq!(cursors[0].width_px, 8);
        assert_eq!(cursors[0].height_px, 16);
    }

    #[test]
    fn renders_beam_underline_hollow_and_hidden_cursor_shapes() {
        let render = |shape| {
            let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
            backend.render_surface(&RenderSurfaceSnapshot {
                target_id: RenderTargetId::new(1),
                latest_seq: Seq::new(1),
                default_background: RgbColorDto::new(0, 0, 0),
                rows: vec![],
                video_surfaces: vec![],
                image_surfaces: vec![],
                dirty_rows: vec![],
                cursor: Some(RenderSurfaceCursorSnapshot {
                    x: 2,
                    y: 3,
                    focused: true,
                    shape,
                }),
            });
            backend.state().geometric_quads()
        };

        let beam = render(RenderSurfaceCursorShape::Beam);
        assert_eq!(beam.len(), 1);
        assert_eq!((beam[0].width_px, beam[0].height_px), (1, 16));

        let underline = render(RenderSurfaceCursorShape::Underline);
        assert_eq!(underline.len(), 1);
        assert_eq!((underline[0].width_px, underline[0].height_px), (8, 2));
        assert_eq!(underline[0].y_px, 62);

        assert_eq!(render(RenderSurfaceCursorShape::HollowBlock).len(), 4);
        assert!(render(RenderSurfaceCursorShape::Hidden).is_empty());
    }

    #[test]
    fn unfocused_cursor_is_hidden() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());

        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: Some(RenderSurfaceCursorSnapshot {
                x: 2,
                y: 3,
                focused: false,
                shape: RenderSurfaceCursorShape::Block,
            }),
        });

        assert!(backend.state().geometric_quads().is_empty());
    }

    #[test]
    fn renders_block_elements_as_geometry_instead_of_font_glyphs() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig {
            cell_width_px: 8,
            cell_height_px: 16,
            content_origin_x: 0,
            content_origin_y: 0,
            content_width_px: 8,
            content_height_px: 16,
            grid_columns: 1,
            grid_rows: 1,
        });

        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "▄".to_string(),
                    style: TextStyleDto::plain(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: Vec::new(),
            cursor: None,
        });

        let state = backend.state();
        assert!(state.glyph_quads().is_empty());

        let block_quads = state.geometric_quads();
        assert_eq!(block_quads.len(), 1);
        assert_eq!(block_quads[0].x_px, 0);
        assert_eq!(block_quads[0].y_px, 8);
        assert_eq!(block_quads[0].width_px, 8);
        assert_eq!(block_quads[0].height_px, 8);
    }

    #[test]
    fn renders_box_drawing_as_geometry_instead_of_font_glyphs() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig {
            cell_width_px: 8,
            cell_height_px: 16,
            content_origin_x: 0,
            content_origin_y: 0,
            content_width_px: 8,
            content_height_px: 16,
            grid_columns: 1,
            grid_rows: 1,
        });

        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "│".to_string(),
                    style: TextStyleDto::plain(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: Vec::new(),
            cursor: None,
        });

        let state = backend.state();
        assert!(state.glyph_quads().is_empty());

        let line_quads = state.geometric_quads();
        assert_eq!(line_quads.len(), 2);
        assert!(line_quads.iter().all(|quad| quad.width_px == 1));
        assert_eq!(line_quads[0].x_px, 4);
        assert_eq!(line_quads[0].y_px, 0);
    }

    #[test]
    fn renders_sextants_as_geometry_instead_of_font_glyphs() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig {
            cell_width_px: 8,
            cell_height_px: 16,
            content_origin_x: 0,
            content_origin_y: 0,
            content_width_px: 8,
            content_height_px: 16,
            grid_columns: 1,
            grid_rows: 1,
        });

        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "\u{1FB02}".to_string(),
                    style: TextStyleDto::plain(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: Vec::new(),
            cursor: None,
        });

        let state = backend.state();
        assert!(state.glyph_quads().is_empty());

        let sextant_quads = state.geometric_quads();
        assert_eq!(
            sextant_quads,
            vec![WgpuQuadDrawItem::solid_rect(
                0,
                0,
                8,
                5,
                TextStyleDto::plain()
            )]
        );
    }
}
