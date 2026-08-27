use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use germinal_ports::{
    pty_host::{
        cell_size::TerminalCellSize, color_theme::TerminalColorTheme,
        render_viewport::TerminalRenderViewport, size_info::TerminalSizeInfo,
        terminal_geometric_glyph::TerminalGeometricGlyph, width::terminal_char_cell_width,
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
            RenderSurfaceSnapshot, RenderSurfaceTextDecoration, RenderSurfaceUnderlineStyle,
        },
    },
    seq::Seq,
};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::rendering::pty_surface::{
    crossfont_glyph_atlas::{WgpuCrossfontStrikeoutMetrics, WgpuCrossfontUnderlineMetrics},
    glyph_atlas::WgpuTerminalGlyphKey,
    text_shaping::{terminal_grapheme_cell_width, terminal_text_segments_with_ligatures},
};

const CURSOR_COLOR: RgbColorDto = RgbColorDto::new(235, 235, 235);
const CURSOR_TEXT_COLOR: RgbColorDto = RgbColorDto::new(0, 0, 0);
const PIXEL_RECT_VIRTUAL_CELL_WIDTH_PX: u32 = 8;
const PIXEL_RECT_VIRTUAL_CELL_HEIGHT_PX: u32 = 16;

fn fallback_underline_metrics(row_height_px: u32) -> WgpuCrossfontUnderlineMetrics {
    let thickness_px = row_height_px.div_ceil(16).max(1);
    WgpuCrossfontUnderlineMetrics::new(
        row_height_px.saturating_sub(thickness_px.saturating_add(1)),
        thickness_px,
    )
}

fn fallback_strikeout_metrics(row_height_px: u32) -> WgpuCrossfontStrikeoutMetrics {
    WgpuCrossfontStrikeoutMetrics::new(
        row_height_px.saturating_mul(9) / 16,
        row_height_px.div_ceil(16).max(1),
    )
}

#[derive(Debug, Clone, Copy, Default)]
struct WgpuTextDecorationMetrics {
    underline: Option<WgpuCrossfontUnderlineMetrics>,
    strikeout: Option<WgpuCrossfontStrikeoutMetrics>,
}

#[derive(Debug, Clone, Copy)]
struct WgpuTextDecorationSpan {
    x: u32,
    y: u32,
    cell_width: u32,
}

#[derive(Debug, Clone)]
pub struct WgpuRendererBackend {
    inner: RefCell<WgpuRendererState>,
    text_shaping: Cell<bool>,
    ligatures: Cell<bool>,
    underline_metrics: Cell<Option<WgpuCrossfontUnderlineMetrics>>,
    strikeout_metrics: Cell<Option<WgpuCrossfontStrikeoutMetrics>>,
}

impl WgpuRendererBackend {
    pub fn new(config: WgpuRendererConfig) -> Self {
        Self {
            inner: RefCell::new(WgpuRendererState {
                config,
                ..WgpuRendererState::default()
            }),
            text_shaping: Cell::new(false),
            ligatures: Cell::new(true),
            underline_metrics: Cell::new(None),
            strikeout_metrics: Cell::new(None),
        }
    }

    pub fn config(&self) -> WgpuRendererConfig {
        self.inner.borrow().config
    }

    pub fn set_text_shaping(&self, enabled: bool) {
        self.text_shaping.set(enabled);
    }

    pub fn set_ligatures(&self, enabled: bool) {
        self.ligatures.set(enabled);
    }

    pub fn set_underline_metrics(&self, metrics: Option<WgpuCrossfontUnderlineMetrics>) {
        self.underline_metrics.set(metrics);
    }

    pub fn set_strikeout_metrics(&self, metrics: Option<WgpuCrossfontStrikeoutMetrics>) {
        self.strikeout_metrics.set(metrics);
    }

    fn text_decoration_metrics(&self) -> WgpuTextDecorationMetrics {
        WgpuTextDecorationMetrics {
            underline: self.underline_metrics.get(),
            strikeout: self.strikeout_metrics.get(),
        }
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
                let rendered_row = render_row(
                    row,
                    config,
                    self.text_shaping.get(),
                    self.ligatures.get(),
                    self.text_decoration_metrics(),
                );
                inner
                    .draw_rows
                    .insert(row_y, Arc::clone(&rendered_row.draw_row));
                inner.rendered_rows.insert(row_y, rendered_row);
            } else {
                inner.draw_rows.remove(&row_y);
                inner.rendered_rows.remove(&row_y);
            }
        }

        let mut ime_rows = Vec::new();
        let mut cursor_quads = Vec::new();
        let mut cursor_text_quads = Vec::new();
        if let (Some(cursor), Some(preedit)) = (snapshot.cursor, snapshot.ime_preedit.as_ref()) {
            ime_rows = render_ime_preedit(
                preedit,
                cursor,
                snapshot.default_background,
                config,
                self.text_shaping.get(),
                self.ligatures.get(),
                self.text_decoration_metrics(),
            );
            if let Some((x, y)) = preedit.cursor_cell(cursor, config.grid_columns, config.grid_rows)
            {
                append_cursor_quads(
                    &mut cursor_quads,
                    None,
                    RenderSurfaceCursorSnapshot {
                        x,
                        y,
                        focused: cursor.focused,
                        shape: RenderSurfaceCursorShape::Beam,
                        blinking: false,
                    },
                    config,
                );
            }
        } else if let Some(cursor) = snapshot.cursor {
            let cursor_row = inner.rendered_rows.get(&cursor.y);
            append_cursor_quads(&mut cursor_quads, cursor_row, cursor, config);
            append_cursor_text_quads(&mut cursor_text_quads, cursor_row, cursor, config);
        }

        let total_row_quads: usize = inner
            .rendered_rows
            .values()
            .map(|row| {
                row.background_quads.len() + row.glyph_quads.len() + row.underline_quads.len()
            })
            .sum();
        let ime_quad_count = ime_rows
            .iter()
            .map(|row| {
                row.background_quads.len() + row.glyph_quads.len() + row.underline_quads.len()
            })
            .sum::<usize>();
        let mut quads = Vec::with_capacity(
            1 + pixel_quads.len()
                + total_row_quads
                + ime_quad_count
                + cursor_quads.len()
                + cursor_text_quads.len(),
        );
        quads.push(WgpuQuadDrawItem::surface_background(
            config,
            snapshot.default_background,
        ));
        quads.extend(pixel_quads);
        for row in inner.rendered_rows.values() {
            quads.extend(row.background_quads.iter().copied());
        }
        for row in &ime_rows {
            quads.extend(row.background_quads.iter().copied());
        }
        for row in inner.rendered_rows.values() {
            quads.extend(row.glyph_quads.iter().copied());
        }
        for row in inner.rendered_rows.values() {
            quads.extend(row.underline_quads.iter().copied());
        }
        for row in &ime_rows {
            quads.extend(row.glyph_quads.iter().copied());
        }
        for row in &ime_rows {
            quads.extend(row.underline_quads.iter().copied());
        }
        quads.extend(cursor_quads);
        quads.extend(cursor_text_quads);

        inner.render_count += 1;
        inner.last_target_id = Some(snapshot.target_id);
        inner.last_seq = Some(snapshot.latest_seq);
        inner.quads = quads;
    }
}

fn render_ime_preedit(
    preedit: &germinal_ports::rendering::surface_snapshot::RenderSurfaceImePreeditSnapshot,
    cursor: RenderSurfaceCursorSnapshot,
    default_background: RgbColorDto,
    config: WgpuRendererConfig,
    text_shaping: bool,
    ligatures: bool,
    decoration_metrics: WgpuTextDecorationMetrics,
) -> Vec<WgpuRenderedRow> {
    if preedit.text.is_empty() || config.grid_columns == 0 || config.grid_rows == 0 {
        return Vec::new();
    }

    let selection = preedit.cursor_range.map(|(start, end)| {
        (
            start.min(end).min(preedit.text.len()),
            start.max(end).min(preedit.text.len()),
        )
    });
    let mut rows = BTreeMap::<u32, RenderSurfaceRowSnapshot>::new();
    let mut x = cursor.x.min(config.grid_columns - 1);
    let mut y = cursor.y.min(config.grid_rows - 1);
    let mut last_run = None::<(u32, usize)>;

    for (byte_index, character) in preedit.text.char_indices() {
        let width = terminal_char_cell_width(character);
        if width == 0 {
            if let Some((row_y, run_index)) = last_run
                && let Some(run) = rows
                    .get_mut(&row_y)
                    .and_then(|row| row.runs.get_mut(run_index))
            {
                run.text.push(character);
            }
            continue;
        }
        if x.saturating_add(width) > config.grid_columns {
            x = 0;
            y = y.saturating_add(1);
        }
        if y >= config.grid_rows {
            break;
        }

        let character_end = byte_index + character.len_utf8();
        let selected = selection
            .is_some_and(|(start, end)| start != end && byte_index < end && character_end > start);
        let style = if selected {
            TextStyleDto {
                foreground: Some(config.cursor_text_color),
                background: Some(config.cursor_color),
                bold: false,
                italic: false,
                underline: true,
            }
        } else {
            TextStyleDto {
                foreground: Some(config.cursor_color),
                background: Some(default_background),
                bold: false,
                italic: false,
                underline: true,
            }
        };
        let runs = &mut rows
            .entry(y)
            .or_insert_with(|| RenderSurfaceRowSnapshot {
                y,
                runs: Vec::new(),
            })
            .runs;
        runs.push(
            germinal_ports::rendering::surface_snapshot::RenderSurfaceRunSnapshot {
                x,
                text: character.to_string(),
                style,
                decoration: Default::default(),
            },
        );
        last_run = Some((y, runs.len() - 1));

        x = x.saturating_add(width);
        if x >= config.grid_columns {
            x = 0;
            y = y.saturating_add(1);
        }
    }

    rows.values()
        .map(|row| render_row(row, config, text_shaping, ligatures, decoration_metrics))
        .collect()
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

fn render_row(
    row: &RenderSurfaceRowSnapshot,
    config: WgpuRendererConfig,
    text_shaping: bool,
    ligatures: bool,
    decoration_metrics: WgpuTextDecorationMetrics,
) -> WgpuRenderedRow {
    if text_shaping {
        return render_shaped_row(row, config, ligatures, decoration_metrics);
    }

    let mut draw_row = WgpuDrawRow {
        y: row.y,
        glyphs: Vec::new(),
    };
    let mut background_quads = Vec::new();
    let mut glyph_quads = Vec::new();
    let mut underline_quads = Vec::new();
    let occupied_cells = occupied_text_cells(row, config.grid_columns);

    for run in &row.runs {
        let glyph_style = dimmed_style(run.style, run.decoration.dim);
        let mut x = run.x;
        let mut previous_glyph_x = None;
        for c in run.text.nfc() {
            let cell_width = terminal_char_cell_width(c);
            if cell_width == 0 {
                if run.decoration.hidden {
                    continue;
                }
                let Some(glyph_x) = previous_glyph_x else {
                    continue;
                };
                let glyph = WgpuGlyphDrawItem {
                    x: glyph_x,
                    y: row.y,
                    c,
                    cell_width: 0,
                    style: glyph_style,
                };
                draw_row.glyphs.push(glyph);
                glyph_quads.push(WgpuQuadDrawItem::glyph(glyph, config));
                continue;
            }
            let glyph = WgpuGlyphDrawItem {
                x,
                y: row.y,
                c,
                cell_width,
                style: glyph_style,
            };
            if run.style.background.is_some() {
                background_quads.push(WgpuQuadDrawItem::background(
                    x, row.y, cell_width, config, run.style,
                ));
            }
            if !run.decoration.hidden {
                draw_row.glyphs.push(glyph);
                if let Some(geometric_glyph) = TerminalGeometricGlyph::from_char(c) {
                    append_terminal_geometric_glyph_quads(
                        &mut glyph_quads,
                        glyph,
                        config,
                        geometric_glyph,
                    );
                } else if c != ' ' {
                    glyph_quads.push(WgpuQuadDrawItem::glyph_with_cell_span(
                        glyph,
                        available_glyph_cell_span(&occupied_cells, x, cell_width),
                        config,
                    ));
                }
                append_text_decoration_quads(
                    &mut underline_quads,
                    WgpuTextDecorationSpan {
                        x,
                        y: row.y,
                        cell_width,
                    },
                    config,
                    glyph_style,
                    run.decoration,
                    decoration_metrics,
                );
            }
            previous_glyph_x = Some(x);
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

fn render_shaped_row(
    row: &RenderSurfaceRowSnapshot,
    config: WgpuRendererConfig,
    ligatures: bool,
    decoration_metrics: WgpuTextDecorationMetrics,
) -> WgpuRenderedRow {
    let mut draw_row = WgpuDrawRow {
        y: row.y,
        glyphs: Vec::new(),
    };
    let mut background_quads = Vec::new();
    let mut glyph_quads = Vec::new();
    let mut underline_quads = Vec::new();
    let occupied_cells = occupied_text_cells(row, config.grid_columns);

    for run in &row.runs {
        let glyph_style = dimmed_style(run.style, run.decoration.dim);
        let mut x = run.x;
        let mut previous_glyph_x = None;
        for segment in terminal_text_segments_with_ligatures(&run.text, run.style, ligatures) {
            let segment_x = if segment.cell_width == 0 {
                previous_glyph_x.unwrap_or(x)
            } else {
                x
            };

            if segment.cell_width > 0 {
                if run.style.background.is_some() {
                    background_quads.push(WgpuQuadDrawItem::background(
                        segment_x,
                        row.y,
                        segment.cell_width,
                        config,
                        run.style,
                    ));
                }
                if !run.decoration.hidden {
                    append_text_decoration_quads(
                        &mut underline_quads,
                        WgpuTextDecorationSpan {
                            x: segment_x,
                            y: row.y,
                            cell_width: segment.cell_width,
                        },
                        config,
                        glyph_style,
                        run.decoration,
                        decoration_metrics,
                    );
                }
            }

            if !run.decoration.hidden {
                let mut grapheme_x = segment_x;
                for grapheme in segment.text.graphemes(true) {
                    let grapheme_width = terminal_grapheme_cell_width(grapheme);
                    for (character_index, character) in grapheme.chars().enumerate() {
                        draw_row.glyphs.push(WgpuGlyphDrawItem {
                            x: grapheme_x,
                            y: row.y,
                            c: character,
                            cell_width: if character_index == 0 {
                                grapheme_width
                            } else {
                                0
                            },
                            style: glyph_style,
                        });
                    }
                    if grapheme_width > 0 {
                        previous_glyph_x = Some(grapheme_x);
                        grapheme_x += grapheme_width;
                    }
                }
            }

            let Some(character) = segment.text.chars().next() else {
                continue;
            };
            let glyph = WgpuGlyphDrawItem {
                x: segment_x,
                y: row.y,
                c: character,
                cell_width: segment.cell_width,
                style: glyph_style,
            };
            if run.decoration.hidden {
                // Hidden text still consumes its normal terminal cell span.
            } else if segment.shaped {
                glyph_quads.push(WgpuQuadDrawItem::glyph_with_key_and_cell_span(
                    glyph,
                    segment.glyph_key,
                    available_glyph_cell_span(&occupied_cells, segment_x, segment.cell_width),
                    config,
                ));
            } else if let Some(geometric_glyph) = TerminalGeometricGlyph::from_char(character) {
                append_terminal_geometric_glyph_quads(
                    &mut glyph_quads,
                    glyph,
                    config,
                    geometric_glyph,
                );
            } else if character != ' ' {
                glyph_quads.push(WgpuQuadDrawItem::glyph_with_cell_span(
                    glyph,
                    available_glyph_cell_span(&occupied_cells, segment_x, segment.cell_width),
                    config,
                ));
            }

            x = x.saturating_add(segment.cell_width);
        }
    }

    WgpuRenderedRow {
        draw_row: Arc::new(draw_row),
        background_quads,
        glyph_quads,
        underline_quads,
    }
}

fn occupied_text_cells(row: &RenderSurfaceRowSnapshot, columns: u32) -> Vec<bool> {
    let mut occupied = vec![false; columns as usize];
    for run in &row.runs {
        let mut x = run.x;
        for grapheme in run.text.graphemes(true) {
            let cell_width = terminal_grapheme_cell_width(grapheme);
            if cell_width == 0 {
                continue;
            }
            let blank = matches!(grapheme, " " | "\u{2002}");
            if !blank {
                for cell_x in x..x.saturating_add(cell_width).min(columns) {
                    occupied[cell_x as usize] = true;
                }
            }
            x = x.saturating_add(cell_width);
        }
    }
    occupied
}

fn available_glyph_cell_span(occupied_cells: &[bool], x: u32, cell_width: u32) -> u32 {
    let cell_width = cell_width.max(1);
    let mut end = x.saturating_add(cell_width) as usize;
    while end < occupied_cells.len() && !occupied_cells[end] {
        end += 1;
    }
    (end.saturating_sub(x as usize) as u32).max(cell_width)
}

fn dimmed_style(mut style: TextStyleDto, dim: bool) -> TextStyleDto {
    if !dim {
        return style;
    }

    let foreground = style.foreground.unwrap_or(RgbColorDto::new(255, 255, 255));
    style.foreground = Some(RgbColorDto::new(
        (u16::from(foreground.red) * 2 / 3) as u8,
        (u16::from(foreground.green) * 2 / 3) as u8,
        (u16::from(foreground.blue) * 2 / 3) as u8,
    ));
    style
}

fn append_text_decoration_quads(
    quads: &mut Vec<WgpuQuadDrawItem>,
    span: WgpuTextDecorationSpan,
    config: WgpuRendererConfig,
    style: TextStyleDto,
    decoration: RenderSurfaceTextDecoration,
    metrics: WgpuTextDecorationMetrics,
) {
    let underline = if decoration.underline == RenderSurfaceUnderlineStyle::None && style.underline
    {
        RenderSurfaceUnderlineStyle::Single
    } else {
        decoration.underline
    };
    let underline_style = TextStyleDto {
        foreground: decoration.underline_color.or(style.foreground),
        ..style
    };

    if underline != RenderSurfaceUnderlineStyle::None {
        let base = WgpuQuadDrawItem::underline(
            span.x,
            span.y,
            span.cell_width,
            config,
            underline_style,
            metrics.underline,
        );
        match underline {
            RenderSurfaceUnderlineStyle::None => {}
            RenderSurfaceUnderlineStyle::Single => quads.push(base),
            RenderSurfaceUnderlineStyle::Double => {
                let gap = base.height_px.max(1);
                let upper_y = base.y_px.saturating_sub(base.height_px.saturating_add(gap));
                quads.push(WgpuQuadDrawItem::line_rect(
                    base.x_px,
                    upper_y,
                    base.width_px,
                    base.height_px,
                    underline_style,
                ));
                quads.push(base);
            }
            RenderSurfaceUnderlineStyle::Dotted => {
                let dot = base.height_px.max(1);
                let pitch = dot.saturating_mul(2).max(2);
                let end_x = base.x_px.saturating_add(base.width_px);
                let mut dot_x = base.x_px;
                while dot_x < end_x {
                    quads.push(WgpuQuadDrawItem::line_rect(
                        dot_x,
                        base.y_px,
                        dot.min(end_x - dot_x),
                        dot,
                        underline_style,
                    ));
                    dot_x = dot_x.saturating_add(pitch);
                }
            }
            RenderSurfaceUnderlineStyle::Dashed => {
                let dash = base.height_px.saturating_mul(4).max(4);
                let pitch = dash.saturating_add(base.height_px.saturating_mul(2).max(2));
                let end_x = base.x_px.saturating_add(base.width_px);
                let mut dash_x = base.x_px;
                while dash_x < end_x {
                    quads.push(WgpuQuadDrawItem::line_rect(
                        dash_x,
                        base.y_px,
                        dash.min(end_x - dash_x),
                        base.height_px,
                        underline_style,
                    ));
                    dash_x = dash_x.saturating_add(pitch);
                }
            }
            RenderSurfaceUnderlineStyle::Curly => {
                let unit = base.height_px.max(1);
                let top = base.y_px.saturating_sub(unit.saturating_mul(2));
                let offsets = [unit, 0, unit, unit.saturating_mul(2)];
                let end_x = base.x_px.saturating_add(base.width_px);
                let mut wave_x = base.x_px;
                let mut step = 0_usize;
                while wave_x < end_x {
                    quads.push(WgpuQuadDrawItem::line_rect(
                        wave_x,
                        top.saturating_add(offsets[step % offsets.len()]),
                        unit.min(end_x - wave_x),
                        unit,
                        underline_style,
                    ));
                    wave_x = wave_x.saturating_add(unit);
                    step += 1;
                }
            }
        }
    }

    if decoration.strikeout {
        quads.push(WgpuQuadDrawItem::strikeout(
            span.x,
            span.y,
            span.cell_width,
            config,
            style,
            metrics.strikeout,
        ));
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
    row: Option<&WgpuRenderedRow>,
    cursor: RenderSurfaceCursorSnapshot,
    config: WgpuRendererConfig,
) {
    if cursor.shape == RenderSurfaceCursorShape::Hidden
        || (cursor.focused && cursor.blinking && !config.blinking_cursor_visible)
    {
        return;
    }

    let shape = if cursor.focused {
        cursor.shape
    } else {
        RenderSurfaceCursorShape::HollowBlock
    };

    let (cursor_x, cursor_cell_width) = cursor_cell_span(row, cursor.x);
    let (x, y) = config.cursor_position_px.unwrap_or_else(|| {
        (
            config.content_origin_x + cursor_x * config.cell_width_px,
            config.row_top_px(cursor.y),
        )
    });
    let w = cursor_cell_width * config.cell_width_px.max(1);
    let h = config.row_height_px(cursor.y);
    let style = TextStyleDto {
        foreground: Some(config.cursor_color),
        background: None,
        bold: false,
        italic: false,
        underline: false,
    };
    let vertical_stroke = config.cell_width_px.div_ceil(8).max(1);
    let horizontal_stroke = h.div_ceil(8).max(1);
    match shape {
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
            let outline_stroke = 2.min(w).min(h);
            quads.push(WgpuQuadDrawItem::solid_rect(x, y, w, outline_stroke, style));
            quads.push(WgpuQuadDrawItem::solid_rect(
                x,
                y + h.saturating_sub(outline_stroke),
                w,
                outline_stroke,
                style,
            ));
            quads.push(WgpuQuadDrawItem::solid_rect(x, y, outline_stroke, h, style));
            quads.push(WgpuQuadDrawItem::solid_rect(
                x + w.saturating_sub(outline_stroke),
                y,
                outline_stroke,
                h,
                style,
            ));
        }
        RenderSurfaceCursorShape::Hidden => {}
    }
}

fn cursor_cell_span(row: Option<&WgpuRenderedRow>, cursor_x: u32) -> (u32, u32) {
    row.and_then(|row| {
        row.draw_row.glyphs().iter().find_map(|glyph| {
            let width = glyph.cell_width.max(1);
            (cursor_x >= glyph.x && cursor_x < glyph.x.saturating_add(width))
                .then_some((glyph.x, width))
        })
    })
    .unwrap_or((cursor_x, 1))
}

fn append_cursor_text_quads(
    quads: &mut Vec<WgpuQuadDrawItem>,
    row: Option<&WgpuRenderedRow>,
    cursor: RenderSurfaceCursorSnapshot,
    config: WgpuRendererConfig,
) {
    if !cursor.focused
        || cursor.shape != RenderSurfaceCursorShape::Block
        || (cursor.blinking && !config.blinking_cursor_visible)
        || config.cursor_position_px.is_some()
    {
        return;
    }

    let Some(glyph) = row.and_then(|row| {
        row.draw_row.glyphs().iter().copied().find(|glyph| {
            cursor.x >= glyph.x && cursor.x < glyph.x.saturating_add(glyph.cell_width.max(1))
        })
    }) else {
        return;
    };
    if glyph.c == ' ' {
        return;
    }

    let glyph = WgpuGlyphDrawItem {
        style: TextStyleDto {
            foreground: Some(config.cursor_text_color),
            background: None,
            ..glyph.style
        },
        ..glyph
    };
    if let Some(geometric_glyph) = TerminalGeometricGlyph::from_char(glyph.c) {
        append_terminal_geometric_glyph_quads(quads, glyph, config, geometric_glyph);
    } else {
        quads.push(WgpuQuadDrawItem::glyph(glyph, config));
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
    pub blinking_cursor_visible: bool,
    pub cursor_color: RgbColorDto,
    pub cursor_text_color: RgbColorDto,
    pub cursor_position_px: Option<(u32, u32)>,
    pub background_alpha: u8,
}
impl WgpuRendererConfig {
    pub fn with_blinking_cursor_visible(mut self, visible: bool) -> Self {
        self.blinking_cursor_visible = visible;
        self
    }

    pub fn with_color_theme(mut self, theme: TerminalColorTheme) -> Self {
        self.cursor_color = theme.cursor;
        self.cursor_text_color = theme.cursor_text;
        self
    }

    pub fn with_background_opacity(mut self, opacity: f32) -> Self {
        self.background_alpha = opacity_alpha(opacity);
        self
    }

    pub fn with_cursor_position_cells(mut self, x: f32, y: f32) -> Self {
        let x_px = self.content_origin_x as f32 + x * self.cell_width_px as f32;
        self.cursor_position_px = Some((x_px.round() as u32, self.row_position_px(y)));
        self
    }

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
            blinking_cursor_visible: true,
            cursor_color: CURSOR_COLOR,
            cursor_text_color: CURSOR_TEXT_COLOR,
            cursor_position_px: None,
            background_alpha: u8::MAX,
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
            blinking_cursor_visible: true,
            cursor_color: CURSOR_COLOR,
            cursor_text_color: CURSOR_TEXT_COLOR,
            cursor_position_px: None,
            background_alpha: u8::MAX,
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

    pub(crate) fn row_top_px(self, row: u32) -> u32 {
        if row >= self.grid_rows.max(1) {
            return self
                .content_origin_y
                .saturating_add(row.saturating_mul(self.cell_height_px));
        }

        self.content_origin_y
            .saturating_add(self.row_offset_px(row))
    }

    pub(crate) fn row_height_px(self, row: u32) -> u32 {
        if row >= self.grid_rows.max(1) {
            return self.cell_height_px.max(1);
        }

        self.row_offset_px(row.saturating_add(1))
            .saturating_sub(self.row_offset_px(row))
            .max(1)
    }

    pub(crate) fn row_offset_px(self, row: u32) -> u32 {
        let rows = self.grid_rows.max(1);
        let row = row.min(rows);
        ((u64::from(row) * u64::from(self.content_height_px)) / u64::from(rows))
            .min(u64::from(u32::MAX)) as u32
    }

    fn row_position_px(self, row: f32) -> u32 {
        if !row.is_finite() {
            return self.content_origin_y;
        }
        let rows = self.grid_rows.max(1);
        let row = row.max(0.0);
        if row >= rows as f32 {
            return self.content_origin_y.saturating_add(
                (row * self.cell_height_px as f32)
                    .round()
                    .clamp(0.0, u32::MAX as f32) as u32,
            );
        }

        let row_index = row.floor() as u32;
        let row_top = self.row_offset_px(row_index);
        let row_height = self
            .row_offset_px(row_index.saturating_add(1))
            .saturating_sub(row_top);
        let row_offset = row_top as f32 + row.fract() * row_height as f32;
        self.content_origin_y
            .saturating_add(row_offset.round() as u32)
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
            blinking_cursor_visible: true,
            cursor_color: CURSOR_COLOR,
            cursor_text_color: CURSOR_TEXT_COLOR,
            cursor_position_px: None,
            background_alpha: u8::MAX,
        }
    }
}

fn opacity_alpha(opacity: f32) -> u8 {
    if opacity.is_finite() {
        (opacity.clamp(0.0, 1.0) * f32::from(u8::MAX)).round() as u8
    } else {
        u8::MAX
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
        let mut text = String::new();
        let mut next_x = 0_u32;
        for glyph in &self.glyphs {
            if glyph.cell_width == 0 {
                text.push(glyph.c);
                continue;
            }
            while next_x < glyph.x {
                text.push(' ');
                next_x += 1;
            }
            text.push(glyph.c);
            next_x = glyph.x.saturating_add(glyph.cell_width.max(1));
        }
        text
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
    pub alpha: u8,
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
            alpha: config.background_alpha,
        }
    }

    pub fn glyph(glyph: WgpuGlyphDrawItem, config: WgpuRendererConfig) -> Self {
        Self::glyph_with_cell_span(glyph, glyph.cell_width, config)
    }

    fn glyph_with_cell_span(
        glyph: WgpuGlyphDrawItem,
        available_cell_width: u32,
        config: WgpuRendererConfig,
    ) -> Self {
        Self::glyph_with_key(
            glyph,
            WgpuTerminalGlyphKey::styled(glyph.c, glyph.style.bold, glyph.style.italic),
            available_cell_width,
            config,
        )
    }

    fn glyph_with_key(
        glyph: WgpuGlyphDrawItem,
        glyph_key: WgpuTerminalGlyphKey,
        available_cell_width: u32,
        config: WgpuRendererConfig,
    ) -> Self {
        Self::glyph_with_key_and_cell_span(glyph, glyph_key, available_cell_width, config)
    }

    fn glyph_with_key_and_cell_span(
        glyph: WgpuGlyphDrawItem,
        glyph_key: WgpuTerminalGlyphKey,
        available_cell_width: u32,
        config: WgpuRendererConfig,
    ) -> Self {
        Self {
            kind: WgpuQuadKind::Glyph { glyph_key },
            x_px: glyph.pixel_x(config),
            y_px: glyph.pixel_y(config),
            width_px: available_cell_width.max(1) * config.cell_width_px,
            height_px: config.row_height_px(glyph.y),
            style: glyph.style,
            alpha: u8::MAX,
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
            alpha: u8::MAX,
        }
    }

    pub fn underline(
        x: u32,
        y: u32,
        cell_width: u32,
        config: WgpuRendererConfig,
        style: TextStyleDto,
        metrics: Option<WgpuCrossfontUnderlineMetrics>,
    ) -> Self {
        let row_height_px = config.row_height_px(y);
        let metrics = metrics.unwrap_or_else(|| fallback_underline_metrics(row_height_px));
        let thickness_px = metrics.thickness_px().min(row_height_px).max(1);
        let offset_y_px = metrics
            .offset_y_px()
            .min(row_height_px.saturating_sub(thickness_px));
        Self {
            kind: WgpuQuadKind::Underline,
            x_px: config.content_origin_x + x * config.cell_width_px,
            y_px: config.row_top_px(y).saturating_add(offset_y_px),
            width_px: cell_width.max(1) * config.cell_width_px,
            height_px: thickness_px,
            style,
            alpha: u8::MAX,
        }
    }

    fn strikeout(
        x: u32,
        y: u32,
        cell_width: u32,
        config: WgpuRendererConfig,
        style: TextStyleDto,
        metrics: Option<WgpuCrossfontStrikeoutMetrics>,
    ) -> Self {
        let row_height_px = config.row_height_px(y);
        let metrics = metrics.unwrap_or_else(|| fallback_strikeout_metrics(row_height_px));
        let thickness_px = metrics.thickness_px().min(row_height_px).max(1);
        let offset_y_px = metrics
            .offset_y_px()
            .min(row_height_px.saturating_sub(thickness_px));
        Self::line_rect(
            config.content_origin_x + x * config.cell_width_px,
            config.row_top_px(y).saturating_add(offset_y_px),
            cell_width.max(1) * config.cell_width_px,
            thickness_px,
            style,
        )
    }

    fn line_rect(x_px: u32, y_px: u32, width_px: u32, height_px: u32, style: TextStyleDto) -> Self {
        Self {
            kind: WgpuQuadKind::Underline,
            x_px,
            y_px,
            width_px: width_px.max(1),
            height_px: height_px.max(1),
            style,
            alpha: u8::MAX,
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
            alpha: u8::MAX,
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
            alpha: u8::MAX,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuQuadKind {
    Background,
    Glyph { glyph_key: WgpuTerminalGlyphKey },
    Underline,
    Geometric,
    PixelRect { color: RgbaColorDto },
}

#[cfg(test)]
mod tests {
    use germinal_ports::{
        pty_host::{
            cell_size::TerminalCellSize,
            color_theme::TerminalColorTheme,
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
                RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot,
                RenderSurfaceImePreeditSnapshot, RenderSurfaceRowSnapshot,
                RenderSurfaceRunSnapshot, RenderSurfaceSnapshot, RenderSurfaceTextDecoration,
                RenderSurfaceUnderlineStyle,
            },
        },
        seq::Seq,
    };

    use super::{
        CURSOR_COLOR, WgpuQuadDrawItem, WgpuQuadKind, WgpuRendererBackend, WgpuRendererConfig,
        append_pixel_rect_quads_from_row, available_glyph_cell_span, occupied_text_cells,
    };
    use crate::rendering::pty_surface::{
        crossfont_glyph_atlas::{WgpuCrossfontStrikeoutMetrics, WgpuCrossfontUnderlineMetrics},
        glyph_atlas::WgpuTerminalGlyphKey,
    };

    fn pixel_row(command: RenderCommandDto) -> RenderSurfaceRowSnapshot {
        let text = encode_pixel_fill_rect_command(&command).expect("pixel command should encode");
        RenderSurfaceRowSnapshot {
            y: 0,
            runs: vec![RenderSurfaceRunSnapshot {
                x: 0,
                text,
                style: Default::default(),
                decoration: Default::default(),
            }],
        }
    }

    fn render_decorated_run(
        backend: &WgpuRendererBackend,
        style: TextStyleDto,
        decoration: RenderSurfaceTextDecoration,
    ) {
        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(99),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "x".to_string(),
                    style,
                    decoration,
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: None,
            ime_preedit: None,
        });
    }

    #[test]
    fn glyph_span_uses_snapshot_gaps_across_style_runs() {
        let row = RenderSurfaceRowSnapshot {
            y: 0,
            runs: vec![
                RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "\u{e5ff}".to_owned(),
                    style: TextStyleDto::plain(),
                    decoration: Default::default(),
                },
                RenderSurfaceRunSnapshot {
                    x: 2,
                    text: "name".to_owned(),
                    style: TextStyleDto {
                        bold: true,
                        ..TextStyleDto::plain()
                    },
                    decoration: Default::default(),
                },
            ],
        };
        let occupied = occupied_text_cells(&row, 8);

        assert_eq!(available_glyph_cell_span(&occupied, 0, 1), 2);

        let occupied_next = occupied_text_cells(
            &RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "♥),".to_owned(),
                    style: TextStyleDto::plain(),
                    decoration: Default::default(),
                }],
            },
            8,
        );
        assert_eq!(available_glyph_cell_span(&occupied_next, 0, 1), 1);
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
        let backend = WgpuRendererBackend::new(
            WgpuRendererConfig::from(size_info).with_background_opacity(0.5),
        );
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
            ime_preedit: None,
        });

        let backgrounds = backend.state().background_quads();
        assert_eq!(backgrounds.len(), 1);
        assert_eq!(backgrounds[0].x_px, 0);
        assert_eq!(backgrounds[0].y_px, 0);
        assert_eq!(backgrounds[0].width_px, 100);
        assert_eq!(backgrounds[0].height_px, 35);
        assert_eq!(backgrounds[0].style.background, Some(background));
        assert_eq!(backgrounds[0].alpha, 128);
    }

    #[test]
    fn underlines_use_the_active_font_metrics() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
        backend.set_underline_metrics(Some(WgpuCrossfontUnderlineMetrics::new(11, 3)));
        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "a".to_string(),
                    style: TextStyleDto {
                        underline: true,
                        ..TextStyleDto::plain()
                    },
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: None,
            ime_preedit: None,
        });

        let underlines = backend.state().underline_quads();
        assert_eq!(underlines.len(), 1);
        assert_eq!(underlines[0].y_px, 11);
        assert_eq!(underlines[0].height_px, 3);
    }

    #[test]
    fn modern_underlines_and_strikeout_generate_distinct_geometry() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig {
            cell_width_px: 12,
            cell_height_px: 20,
            content_width_px: 12,
            content_height_px: 20,
            ..WgpuRendererConfig::default()
        });
        backend.set_underline_metrics(Some(WgpuCrossfontUnderlineMetrics::new(16, 2)));
        backend.set_strikeout_metrics(Some(WgpuCrossfontStrikeoutMetrics::new(9, 2)));
        let color = RgbColorDto::new(12, 34, 56);

        for (underline, minimum_quads) in [
            (RenderSurfaceUnderlineStyle::Double, 2),
            (RenderSurfaceUnderlineStyle::Curly, 4),
            (RenderSurfaceUnderlineStyle::Dotted, 3),
            (RenderSurfaceUnderlineStyle::Dashed, 1),
        ] {
            render_decorated_run(
                &backend,
                TextStyleDto {
                    foreground: Some(color),
                    ..TextStyleDto::plain()
                },
                RenderSurfaceTextDecoration {
                    underline,
                    underline_color: Some(color),
                    strikeout: true,
                    ..Default::default()
                },
            );
            let lines = backend.state().underline_quads();
            assert!(lines.len() > minimum_quads);
            assert!(
                lines
                    .iter()
                    .all(|quad| quad.style.foreground == Some(color))
            );
            assert!(lines.iter().any(|quad| quad.y_px == 9));
        }
    }

    #[test]
    fn hidden_text_keeps_background_without_emitting_glyphs() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
        render_decorated_run(
            &backend,
            TextStyleDto {
                foreground: Some(RgbColorDto::new(210, 210, 210)),
                background: Some(RgbColorDto::new(20, 30, 40)),
                ..TextStyleDto::plain()
            },
            RenderSurfaceTextDecoration {
                hidden: true,
                ..Default::default()
            },
        );

        let state = backend.state();
        assert!(state.glyph_quads().is_empty());
        assert_eq!(state.background_quads().len(), 2);
    }

    #[test]
    fn dim_text_reduces_the_glyph_foreground_intensity() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
        render_decorated_run(
            &backend,
            TextStyleDto {
                foreground: Some(RgbColorDto::new(150, 120, 90)),
                ..TextStyleDto::plain()
            },
            RenderSurfaceTextDecoration {
                dim: true,
                ..Default::default()
            },
        );

        assert_eq!(
            backend.state().glyph_quads()[0].style.foreground,
            Some(RgbColorDto::new(100, 80, 60))
        );
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
                blinking: false,
            }),
            ime_preedit: None,
        });

        let cursors = backend.state().geometric_quads();
        assert_eq!(cursors.len(), 1);
        assert_eq!(cursors[0].x_px, 16);
        assert_eq!(cursors[0].y_px, 48);
        assert_eq!(cursors[0].width_px, 8);
        assert_eq!(cursors[0].height_px, 16);
    }

    #[test]
    fn cursor_motion_position_overrides_the_terminal_cell_position() {
        let backend = WgpuRendererBackend::new(
            WgpuRendererConfig::default().with_cursor_position_cells(1.5, 2.25),
        );

        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: Some(RenderSurfaceCursorSnapshot {
                x: 4,
                y: 5,
                focused: true,
                shape: RenderSurfaceCursorShape::Block,
                blinking: false,
            }),
            ime_preedit: None,
        });

        let cursors = backend.state().geometric_quads();
        assert_eq!((cursors[0].x_px, cursors[0].y_px), (12, 36));
    }

    #[test]
    fn cursor_motion_uses_the_same_distributed_row_positions_as_text() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(100, 35),
            TerminalCellSize::new(8, 16),
            TerminalPadding::ZERO,
        );
        let config = WgpuRendererConfig::from(size_info);

        assert_eq!(config.row_top_px(1), 17);
        assert_eq!(
            config
                .with_cursor_position_cells(1.5, 1.0)
                .cursor_position_px,
            Some((12, 17))
        );
        assert_eq!(
            config
                .with_cursor_position_cells(1.5, 0.5)
                .cursor_position_px,
            Some((12, 9))
        );
    }

    #[test]
    fn block_cursor_uses_theme_colors_and_repaints_covered_text() {
        let color_theme = TerminalColorTheme {
            cursor: RgbColorDto::new(12, 34, 56),
            cursor_text: RgbColorDto::new(210, 220, 230),
            ..TerminalColorTheme::default()
        };
        let backend =
            WgpuRendererBackend::new(WgpuRendererConfig::default().with_color_theme(color_theme));
        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: color_theme.background,
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "x".to_string(),
                    style: TextStyleDto::plain(),
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: Some(RenderSurfaceCursorSnapshot {
                x: 0,
                y: 0,
                focused: true,
                shape: RenderSurfaceCursorShape::Block,
                blinking: false,
            }),
            ime_preedit: None,
        });

        let state = backend.state();
        let cursor = state
            .geometric_quads()
            .into_iter()
            .find(|quad| quad.width_px == 8 && quad.height_px == 16)
            .expect("block cursor should render");
        assert_eq!(cursor.style.foreground, Some(color_theme.cursor));
        assert_eq!(
            state
                .glyph_quads()
                .last()
                .expect("cursor text should render")
                .style
                .foreground,
            Some(color_theme.cursor_text)
        );
    }

    #[test]
    fn block_cursor_covers_the_full_width_of_a_cjk_glyph() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "功".to_string(),
                    style: TextStyleDto::plain(),
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: Some(RenderSurfaceCursorSnapshot {
                x: 0,
                y: 0,
                focused: true,
                shape: RenderSurfaceCursorShape::Block,
                blinking: false,
            }),
            ime_preedit: None,
        });

        let cursor = backend
            .state()
            .geometric_quads()
            .into_iter()
            .find(|quad| quad.height_px == 16)
            .expect("block cursor should render");
        assert_eq!(cursor.x_px, 0);
        assert_eq!(cursor.width_px, 16);
    }

    #[test]
    fn zero_width_glyphs_overlay_their_base_character_without_advancing() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
        let text = "e\u{301}中\u{301}x";
        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: text.to_string(),
                    style: TextStyleDto::plain(),
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: None,
            ime_preedit: None,
        });

        let state = backend.state();
        let glyphs = state.glyphs();
        assert_eq!(state.line_texts(), vec!["é中\u{301}x"]);
        assert_eq!(glyphs.len(), 4);
        assert_eq!(
            (glyphs[0].c, glyphs[0].x, glyphs[0].cell_width),
            ('é', 0, 1)
        );
        assert_eq!(
            (glyphs[1].c, glyphs[1].x, glyphs[1].cell_width),
            ('中', 1, 2)
        );
        assert_eq!(
            (glyphs[2].c, glyphs[2].x, glyphs[2].cell_width),
            ('\u{301}', 1, 0)
        );
        assert_eq!(
            (glyphs[3].c, glyphs[3].x, glyphs[3].cell_width),
            ('x', 3, 1)
        );
    }

    #[test]
    fn shaped_segments_keep_terminal_cell_positions_and_use_cluster_glyphs() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig::default());
        backend.set_text_shaping(true);
        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![RenderSurfaceRowSnapshot {
                y: 0,
                runs: vec![RenderSurfaceRunSnapshot {
                    x: 0,
                    text: "👩\u{200d}💻سلامx".to_string(),
                    style: TextStyleDto::plain(),
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: None,
            ime_preedit: None,
        });

        let state = backend.state();
        assert_eq!(state.line_texts(), ["👩\u{200d}💻سلامx"]);
        let row = state.row(0).unwrap();
        assert_eq!(row.glyphs()[0].x, 0);
        assert_eq!(row.glyphs()[0].cell_width, 2);
        assert_eq!(row.glyphs()[3].x, 2);
        assert_eq!(row.glyphs()[4].x, 3);
        assert_eq!(row.glyphs()[5].x, 4);
        assert_eq!(row.glyphs()[6].x, 5);
        assert_eq!(row.glyphs()[7].x, 6);

        let glyph_quads = state.glyph_quads();
        assert_eq!(glyph_quads.len(), 3);
        assert!(matches!(
            glyph_quads[0].kind,
            WgpuQuadKind::Glyph { glyph_key } if glyph_key.is_cluster()
        ));
        assert!(matches!(
            glyph_quads[1].kind,
            WgpuQuadKind::Glyph { glyph_key } if glyph_key.is_cluster()
        ));
        assert_eq!(glyph_quads[0].width_px, 16);
        assert_eq!(glyph_quads[1].x_px, 16);
        assert_eq!(glyph_quads[1].width_px, 32);
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
                    blinking: false,
                }),
                ime_preedit: None,
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
    fn unfocused_cursor_renders_as_a_non_blinking_hollow_block() {
        let backend = WgpuRendererBackend::new(
            WgpuRendererConfig::default().with_blinking_cursor_visible(false),
        );

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
                blinking: true,
            }),
            ime_preedit: None,
        });

        let cursors = backend.state().geometric_quads();
        assert_eq!(cursors.len(), 4);
        assert_eq!((cursors[0].x_px, cursors[0].y_px), (16, 48));
        assert_eq!((cursors[0].width_px, cursors[0].height_px), (8, 2));
        assert_eq!((cursors[1].x_px, cursors[1].y_px), (16, 62));
        assert_eq!((cursors[1].width_px, cursors[1].height_px), (8, 2));
        assert_eq!((cursors[2].x_px, cursors[2].y_px), (16, 48));
        assert_eq!((cursors[2].width_px, cursors[2].height_px), (2, 16));
        assert_eq!((cursors[3].x_px, cursors[3].y_px), (22, 48));
        assert_eq!((cursors[3].width_px, cursors[3].height_px), (2, 16));
    }

    #[test]
    fn blinking_cursor_is_hidden_during_the_off_phase() {
        let backend = WgpuRendererBackend::new(
            WgpuRendererConfig::default().with_blinking_cursor_visible(false),
        );

        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background: RgbColorDto::new(0, 0, 0),
            rows: vec![],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: Some(RenderSurfaceCursorSnapshot {
                x: 0,
                y: 0,
                focused: true,
                shape: RenderSurfaceCursorShape::Block,
                blinking: true,
            }),
            ime_preedit: None,
        });

        assert!(backend.state().geometric_quads().is_empty());
    }

    #[test]
    fn ime_preedit_wraps_wide_glyphs_and_renders_selection_and_caret() {
        let backend = WgpuRendererBackend::new(WgpuRendererConfig {
            cell_width_px: 8,
            cell_height_px: 16,
            content_origin_x: 0,
            content_origin_y: 0,
            content_width_px: 32,
            content_height_px: 32,
            grid_columns: 4,
            grid_rows: 2,
            blinking_cursor_visible: true,
            ..WgpuRendererConfig::default()
        });
        let default_background = RgbColorDto::new(0, 0, 0);

        backend.render_surface(&RenderSurfaceSnapshot {
            target_id: RenderTargetId::new(1),
            latest_seq: Seq::new(1),
            default_background,
            rows: vec![],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: vec![],
            cursor: Some(RenderSurfaceCursorSnapshot {
                x: 3,
                y: 0,
                focused: true,
                shape: RenderSurfaceCursorShape::Block,
                blinking: false,
            }),
            ime_preedit: Some(RenderSurfaceImePreeditSnapshot {
                text: "你a".to_string(),
                cursor_range: Some((0, 3)),
            }),
        });

        let state = backend.state();
        let glyphs = state.glyph_quads();
        assert_eq!(glyphs.len(), 2);
        assert_eq!(
            glyphs[0].kind,
            WgpuQuadKind::Glyph {
                glyph_key: WgpuTerminalGlyphKey::styled('你', false, false),
            }
        );
        assert_eq!(
            (glyphs[0].x_px, glyphs[0].y_px, glyphs[0].width_px),
            (0, 16, 16)
        );
        assert_eq!(
            glyphs[1].kind,
            WgpuQuadKind::Glyph {
                glyph_key: WgpuTerminalGlyphKey::styled('a', false, false),
            }
        );
        assert_eq!(
            (glyphs[1].x_px, glyphs[1].y_px, glyphs[1].width_px),
            (16, 16, 8)
        );

        let backgrounds = state.background_quads();
        assert_eq!(backgrounds.len(), 3);
        assert_eq!(backgrounds[1].style.background, Some(CURSOR_COLOR));
        assert_eq!(backgrounds[2].style.background, Some(default_background));
        assert_eq!(state.underline_quads().len(), 2);

        let caret = state.geometric_quads();
        assert_eq!(caret.len(), 1);
        assert_eq!((caret[0].x_px, caret[0].y_px), (16, 16));
        assert_eq!((caret[0].width_px, caret[0].height_px), (1, 16));
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
            blinking_cursor_visible: true,
            ..WgpuRendererConfig::default()
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
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: Vec::new(),
            cursor: None,
            ime_preedit: None,
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
            blinking_cursor_visible: true,
            ..WgpuRendererConfig::default()
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
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: Vec::new(),
            cursor: None,
            ime_preedit: None,
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
            blinking_cursor_visible: true,
            ..WgpuRendererConfig::default()
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
                    decoration: Default::default(),
                }],
            }],
            video_surfaces: vec![],
            image_surfaces: vec![],
            dirty_rows: Vec::new(),
            cursor: None,
            ime_preedit: None,
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
