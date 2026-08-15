use crate::pty_host::cell_size::TerminalCellSize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalGeometricGlyph {
    BoxDrawing(BoxDrawingGlyph),
    Raster(TerminalRasterGlyph),
}

impl TerminalGeometricGlyph {
    pub fn from_char(c: char) -> Option<Self> {
        BoxDrawingGlyph::from_char(c)
            .map(Self::BoxDrawing)
            .or_else(|| raster_glyph_from_char(c).map(Self::Raster))
    }

    pub fn pixel_rects(
        self,
        cell_size: TerminalCellSize,
        column_span: u32,
    ) -> Vec<TerminalGeometricGlyphRect> {
        match self {
            Self::BoxDrawing(glyph) => glyph.pixel_rects(cell_size, column_span),
            Self::Raster(glyph) => glyph.pixel_rects(cell_size, column_span),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalGeometricGlyphRect {
    x_px: u32,
    y_px: u32,
    width_px: u32,
    height_px: u32,
}

impl TerminalGeometricGlyphRect {
    pub const fn new(x_px: u32, y_px: u32, width_px: u32, height_px: u32) -> Self {
        Self {
            x_px,
            y_px,
            width_px,
            height_px,
        }
    }

    pub const fn x_px(self) -> u32 {
        self.x_px
    }

    pub const fn y_px(self) -> u32 {
        self.y_px
    }

    pub const fn width_px(self) -> u32 {
        self.width_px
    }

    pub const fn height_px(self) -> u32 {
        self.height_px
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoxDrawingGlyph {
    Orthogonal(OrthogonalBoxGlyph),
    Diagonal(DiagonalBoxGlyph),
}

impl BoxDrawingGlyph {
    fn from_char(c: char) -> Option<Self> {
        match c {
            '─' => Some(Self::orthogonal(
                StrokeStyle::Light,
                StrokeStyle::Light,
                StrokeStyle::None,
                StrokeStyle::None,
            )),
            '│' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::None,
                StrokeStyle::Light,
                StrokeStyle::Light,
            )),
            '┌' | '╭' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::Light,
                StrokeStyle::None,
                StrokeStyle::Light,
            )),
            '┐' | '╮' => Some(Self::orthogonal(
                StrokeStyle::Light,
                StrokeStyle::None,
                StrokeStyle::None,
                StrokeStyle::Light,
            )),
            '└' | '╰' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::Light,
                StrokeStyle::Light,
                StrokeStyle::None,
            )),
            '┘' | '╯' => Some(Self::orthogonal(
                StrokeStyle::Light,
                StrokeStyle::None,
                StrokeStyle::Light,
                StrokeStyle::None,
            )),
            '├' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::Light,
                StrokeStyle::Light,
                StrokeStyle::Light,
            )),
            '┤' => Some(Self::orthogonal(
                StrokeStyle::Light,
                StrokeStyle::None,
                StrokeStyle::Light,
                StrokeStyle::Light,
            )),
            '┬' => Some(Self::orthogonal(
                StrokeStyle::Light,
                StrokeStyle::Light,
                StrokeStyle::None,
                StrokeStyle::Light,
            )),
            '┴' => Some(Self::orthogonal(
                StrokeStyle::Light,
                StrokeStyle::Light,
                StrokeStyle::Light,
                StrokeStyle::None,
            )),
            '┼' => Some(Self::orthogonal(
                StrokeStyle::Light,
                StrokeStyle::Light,
                StrokeStyle::Light,
                StrokeStyle::Light,
            )),
            '━' => Some(Self::orthogonal(
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
                StrokeStyle::None,
                StrokeStyle::None,
            )),
            '┃' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::None,
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
            )),
            '┏' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::Heavy,
                StrokeStyle::None,
                StrokeStyle::Heavy,
            )),
            '┓' => Some(Self::orthogonal(
                StrokeStyle::Heavy,
                StrokeStyle::None,
                StrokeStyle::None,
                StrokeStyle::Heavy,
            )),
            '┗' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
                StrokeStyle::None,
            )),
            '┛' => Some(Self::orthogonal(
                StrokeStyle::Heavy,
                StrokeStyle::None,
                StrokeStyle::Heavy,
                StrokeStyle::None,
            )),
            '┣' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
            )),
            '┫' => Some(Self::orthogonal(
                StrokeStyle::Heavy,
                StrokeStyle::None,
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
            )),
            '┳' => Some(Self::orthogonal(
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
                StrokeStyle::None,
                StrokeStyle::Heavy,
            )),
            '┻' => Some(Self::orthogonal(
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
                StrokeStyle::None,
            )),
            '╋' => Some(Self::orthogonal(
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
                StrokeStyle::Heavy,
            )),
            '═' => Some(Self::orthogonal(
                StrokeStyle::Double,
                StrokeStyle::Double,
                StrokeStyle::None,
                StrokeStyle::None,
            )),
            '║' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::None,
                StrokeStyle::Double,
                StrokeStyle::Double,
            )),
            '╔' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::Double,
                StrokeStyle::None,
                StrokeStyle::Double,
            )),
            '╗' => Some(Self::orthogonal(
                StrokeStyle::Double,
                StrokeStyle::None,
                StrokeStyle::None,
                StrokeStyle::Double,
            )),
            '╚' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::Double,
                StrokeStyle::Double,
                StrokeStyle::None,
            )),
            '╝' => Some(Self::orthogonal(
                StrokeStyle::Double,
                StrokeStyle::None,
                StrokeStyle::Double,
                StrokeStyle::None,
            )),
            '╠' => Some(Self::orthogonal(
                StrokeStyle::None,
                StrokeStyle::Double,
                StrokeStyle::Double,
                StrokeStyle::Double,
            )),
            '╣' => Some(Self::orthogonal(
                StrokeStyle::Double,
                StrokeStyle::None,
                StrokeStyle::Double,
                StrokeStyle::Double,
            )),
            '╦' => Some(Self::orthogonal(
                StrokeStyle::Double,
                StrokeStyle::Double,
                StrokeStyle::None,
                StrokeStyle::Double,
            )),
            '╩' => Some(Self::orthogonal(
                StrokeStyle::Double,
                StrokeStyle::Double,
                StrokeStyle::Double,
                StrokeStyle::None,
            )),
            '╬' => Some(Self::orthogonal(
                StrokeStyle::Double,
                StrokeStyle::Double,
                StrokeStyle::Double,
                StrokeStyle::Double,
            )),
            '╱' => Some(Self::Diagonal(DiagonalBoxGlyph::ForwardSlash)),
            '╲' => Some(Self::Diagonal(DiagonalBoxGlyph::BackSlash)),
            '╳' => Some(Self::Diagonal(DiagonalBoxGlyph::Cross)),
            _ => None,
        }
    }

    fn orthogonal(
        left: StrokeStyle,
        right: StrokeStyle,
        up: StrokeStyle,
        down: StrokeStyle,
    ) -> Self {
        Self::Orthogonal(OrthogonalBoxGlyph {
            left,
            right,
            up,
            down,
        })
    }

    fn pixel_rects(
        self,
        cell_size: TerminalCellSize,
        column_span: u32,
    ) -> Vec<TerminalGeometricGlyphRect> {
        match self {
            Self::Orthogonal(glyph) => glyph.pixel_rects(cell_size, column_span),
            Self::Diagonal(glyph) => glyph.pixel_rects(cell_size, column_span),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrthogonalBoxGlyph {
    left: StrokeStyle,
    right: StrokeStyle,
    up: StrokeStyle,
    down: StrokeStyle,
}

impl OrthogonalBoxGlyph {
    fn pixel_rects(
        self,
        cell_size: TerminalCellSize,
        column_span: u32,
    ) -> Vec<TerminalGeometricGlyphRect> {
        let width_px = cell_width_px(cell_size, column_span);
        let height_px = cell_size.height_px();
        let center_x = width_px / 2;
        let center_y = height_px / 2;
        let mut rects = Vec::new();

        append_horizontal_box_segment(
            &mut rects,
            0,
            (center_x + stroke_extent(self.left, cell_size)).min(width_px),
            center_y,
            width_px,
            height_px,
            self.left,
        );
        append_horizontal_box_segment(
            &mut rects,
            center_x.saturating_sub(stroke_extent(self.right, cell_size)),
            width_px,
            center_y,
            width_px,
            height_px,
            self.right,
        );
        append_vertical_box_segment(
            &mut rects,
            0,
            (center_y + stroke_extent(self.up, cell_size)).min(height_px),
            center_x,
            width_px,
            height_px,
            self.up,
        );
        append_vertical_box_segment(
            &mut rects,
            center_y.saturating_sub(stroke_extent(self.down, cell_size)),
            height_px,
            center_x,
            width_px,
            height_px,
            self.down,
        );

        rects
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeStyle {
    None,
    Light,
    Heavy,
    Double,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagonalBoxGlyph {
    ForwardSlash,
    BackSlash,
    Cross,
}

impl DiagonalBoxGlyph {
    fn pixel_rects(
        self,
        cell_size: TerminalCellSize,
        column_span: u32,
    ) -> Vec<TerminalGeometricGlyphRect> {
        let width_px = cell_width_px(cell_size, column_span);
        let height_px = cell_size.height_px();
        let raster = match self {
            Self::ForwardSlash => diagonal_raster(8, 8, true),
            Self::BackSlash => diagonal_raster(8, 8, false),
            Self::Cross => TerminalRasterGlyph::new(
                8,
                8,
                diagonal_raster(8, 8, true).mask() | diagonal_raster(8, 8, false).mask(),
            ),
        };
        raster.pixel_rects_for_size(width_px, height_px)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalRasterGlyph {
    columns: u8,
    rows: u8,
    mask: u64,
}

impl TerminalRasterGlyph {
    pub const fn new(columns: u8, rows: u8, mask: u64) -> Self {
        Self {
            columns,
            rows,
            mask,
        }
    }

    pub const fn mask(self) -> u64 {
        self.mask
    }

    fn pixel_rects(
        self,
        cell_size: TerminalCellSize,
        column_span: u32,
    ) -> Vec<TerminalGeometricGlyphRect> {
        self.pixel_rects_for_size(cell_width_px(cell_size, column_span), cell_size.height_px())
    }

    fn pixel_rects_for_size(
        self,
        width_px: u32,
        height_px: u32,
    ) -> Vec<TerminalGeometricGlyphRect> {
        let mut row_spans = Vec::new();
        let columns = u32::from(self.columns).max(1);
        let rows = u32::from(self.rows).max(1);

        for row in 0..rows {
            let y0 = scale_grid_axis(row, height_px, rows);
            let y1 = scale_grid_axis(row + 1, height_px, rows);
            if y1 <= y0 {
                continue;
            }
            let mut column = 0;
            while column < columns {
                if !raster_mask_contains(self.mask, columns, column, row) {
                    column += 1;
                    continue;
                }
                let start = column;
                column += 1;
                while column < columns && raster_mask_contains(self.mask, columns, column, row) {
                    column += 1;
                }
                let x0 = scale_grid_axis(start, width_px, columns);
                let x1 = scale_grid_axis(column, width_px, columns);
                if x1 > x0 {
                    row_spans.push(TerminalGeometricGlyphRect::new(x0, y0, x1 - x0, y1 - y0));
                }
            }
        }

        merge_adjacent_rect_rows(row_spans)
    }
}

fn merge_adjacent_rect_rows(
    rects: Vec<TerminalGeometricGlyphRect>,
) -> Vec<TerminalGeometricGlyphRect> {
    let mut merged: Vec<TerminalGeometricGlyphRect> = Vec::new();
    for rect in rects {
        if let Some(last) = merged.last_mut()
            && last.x_px == rect.x_px
            && last.width_px == rect.width_px
            && last.y_px + last.height_px == rect.y_px
        {
            last.height_px += rect.height_px;
            continue;
        }
        merged.push(rect);
    }
    merged
}

fn raster_glyph_from_char(c: char) -> Option<TerminalRasterGlyph> {
    match c {
        '█' => Some(fill_rect_glyph(8, 8, 0, 0, 8, 8)),
        '▀' => Some(fill_rect_glyph(8, 8, 0, 0, 8, 4)),
        '▄' => Some(fill_rect_glyph(8, 8, 0, 4, 8, 4)),
        '▁' => Some(fill_rect_glyph(8, 8, 0, 7, 8, 1)),
        '▂' => Some(fill_rect_glyph(8, 8, 0, 6, 8, 2)),
        '▃' => Some(fill_rect_glyph(8, 8, 0, 5, 8, 3)),
        '▅' => Some(fill_rect_glyph(8, 8, 0, 3, 8, 5)),
        '▆' => Some(fill_rect_glyph(8, 8, 0, 2, 8, 6)),
        '▇' => Some(fill_rect_glyph(8, 8, 0, 1, 8, 7)),
        '▉' => Some(fill_rect_glyph(8, 8, 0, 0, 7, 8)),
        '▊' => Some(fill_rect_glyph(8, 8, 0, 0, 6, 8)),
        '▋' => Some(fill_rect_glyph(8, 8, 0, 0, 5, 8)),
        '▌' => Some(fill_rect_glyph(8, 8, 0, 0, 4, 8)),
        '▍' => Some(fill_rect_glyph(8, 8, 0, 0, 3, 8)),
        '▎' => Some(fill_rect_glyph(8, 8, 0, 0, 2, 8)),
        '▏' => Some(fill_rect_glyph(8, 8, 0, 0, 1, 8)),
        '▐' => Some(fill_rect_glyph(8, 8, 4, 0, 4, 8)),
        '▔' => Some(fill_rect_glyph(8, 8, 0, 0, 8, 1)),
        '▕' => Some(fill_rect_glyph(8, 8, 7, 0, 1, 8)),
        '▖' => Some(fill_rect_glyph(2, 2, 0, 1, 1, 1)),
        '▗' => Some(fill_rect_glyph(2, 2, 1, 1, 1, 1)),
        '▘' => Some(fill_rect_glyph(2, 2, 0, 0, 1, 1)),
        '▙' => Some(TerminalRasterGlyph::new(
            2,
            2,
            fill_rect_mask(2, 2, 0, 0, 1, 1)
                | fill_rect_mask(2, 2, 0, 1, 1, 1)
                | fill_rect_mask(2, 2, 1, 1, 1, 1),
        )),
        '▚' => Some(TerminalRasterGlyph::new(
            2,
            2,
            fill_rect_mask(2, 2, 0, 0, 1, 1) | fill_rect_mask(2, 2, 1, 1, 1, 1),
        )),
        '▛' => Some(TerminalRasterGlyph::new(
            2,
            2,
            fill_rect_mask(2, 2, 0, 0, 1, 1)
                | fill_rect_mask(2, 2, 1, 0, 1, 1)
                | fill_rect_mask(2, 2, 0, 1, 1, 1),
        )),
        '▜' => Some(TerminalRasterGlyph::new(
            2,
            2,
            fill_rect_mask(2, 2, 0, 0, 1, 1)
                | fill_rect_mask(2, 2, 1, 0, 1, 1)
                | fill_rect_mask(2, 2, 1, 1, 1, 1),
        )),
        '▝' => Some(fill_rect_glyph(2, 2, 1, 0, 1, 1)),
        '▞' => Some(TerminalRasterGlyph::new(
            2,
            2,
            fill_rect_mask(2, 2, 1, 0, 1, 1) | fill_rect_mask(2, 2, 0, 1, 1, 1),
        )),
        '▟' => Some(TerminalRasterGlyph::new(
            2,
            2,
            fill_rect_mask(2, 2, 1, 0, 1, 1)
                | fill_rect_mask(2, 2, 0, 1, 1, 1)
                | fill_rect_mask(2, 2, 1, 1, 1, 1),
        )),
        '🮂' => Some(fill_rect_glyph(8, 8, 0, 0, 8, 2)),
        '🮃' => Some(fill_rect_glyph(8, 8, 0, 0, 8, 3)),
        '🮄' => Some(fill_rect_glyph(8, 8, 0, 0, 8, 5)),
        '🮅' => Some(fill_rect_glyph(8, 8, 0, 0, 8, 6)),
        '🮆' => Some(fill_rect_glyph(8, 8, 0, 0, 8, 7)),
        '🮇' => Some(fill_rect_glyph(8, 8, 6, 0, 2, 8)),
        '🮈' => Some(fill_rect_glyph(8, 8, 5, 0, 3, 8)),
        '🮉' => Some(fill_rect_glyph(8, 8, 3, 0, 5, 8)),
        '🮊' => Some(fill_rect_glyph(8, 8, 2, 0, 6, 8)),
        '🮋' => Some(fill_rect_glyph(8, 8, 1, 0, 7, 8)),
        _ => raster_glyph_from_char_range(c),
    }
}

fn raster_glyph_from_char_range(c: char) -> Option<TerminalRasterGlyph> {
    let codepoint = c as u32;

    if let Some(sextant_mask) = sextant_mask(codepoint) {
        return Some(TerminalRasterGlyph::new(2, 3, sextant_mask));
    }

    match codepoint {
        0x1FB82..=0x1FB86 => {
            let eighths = codepoint - 0x1FB82 + 2;
            Some(fill_rect_glyph(8, 8, 0, 0, 8, eighths))
        }
        0x1FB87..=0x1FB8B => {
            let eighths = codepoint - 0x1FB87 + 2;
            Some(fill_rect_glyph(8, 8, 8 - eighths, 0, eighths, 8))
        }
        _ => None,
    }
}

fn sextant_mask(codepoint: u32) -> Option<u64> {
    if !(0x1FB00..=0x1FB3B).contains(&codepoint) {
        return None;
    }

    let mut mask = codepoint - 0x1FB00 + 1;
    if codepoint >= 0x1FB14 {
        mask += 1;
    }
    if codepoint >= 0x1FB28 {
        mask += 1;
    }
    Some(u64::from(mask))
}

fn diagonal_raster(columns: u8, rows: u8, forward_slash: bool) -> TerminalRasterGlyph {
    let mut mask = 0;
    let columns_u32 = u32::from(columns).max(1);
    let rows_u32 = u32::from(rows).max(1);

    for row in 0..rows_u32 {
        let column = if forward_slash {
            (columns_u32 - 1).saturating_sub(row.saturating_mul(columns_u32 - 1) / rows_u32.max(1))
        } else {
            row.saturating_mul(columns_u32 - 1) / rows_u32.max(1)
        };
        mask |= cell_mask(columns_u32, column, row);
    }

    TerminalRasterGlyph::new(columns, rows, mask)
}

fn append_horizontal_box_segment(
    rects: &mut Vec<TerminalGeometricGlyphRect>,
    start_x: u32,
    end_x: u32,
    center_y: u32,
    width_px: u32,
    height_px: u32,
    style: StrokeStyle,
) {
    if style == StrokeStyle::None || end_x <= start_x {
        return;
    }
    match style {
        StrokeStyle::None => {}
        StrokeStyle::Light | StrokeStyle::Heavy => {
            let thickness = stroke_thickness(style, width_px, height_px);
            rects.push(TerminalGeometricGlyphRect::new(
                start_x,
                center_y.saturating_sub(thickness / 2),
                end_x - start_x,
                thickness,
            ));
        }
        StrokeStyle::Double => {
            let thickness = stroke_thickness(StrokeStyle::Light, width_px, height_px);
            let offset = double_stroke_offset(width_px, height_px);
            rects.push(TerminalGeometricGlyphRect::new(
                start_x,
                center_y.saturating_sub(offset + thickness),
                end_x - start_x,
                thickness,
            ));
            rects.push(TerminalGeometricGlyphRect::new(
                start_x,
                (center_y + offset).min(height_px.saturating_sub(1)),
                end_x - start_x,
                thickness,
            ));
        }
    }
}

fn append_vertical_box_segment(
    rects: &mut Vec<TerminalGeometricGlyphRect>,
    start_y: u32,
    end_y: u32,
    center_x: u32,
    width_px: u32,
    height_px: u32,
    style: StrokeStyle,
) {
    if style == StrokeStyle::None || end_y <= start_y {
        return;
    }
    match style {
        StrokeStyle::None => {}
        StrokeStyle::Light | StrokeStyle::Heavy => {
            let thickness = stroke_thickness(style, width_px, height_px);
            rects.push(TerminalGeometricGlyphRect::new(
                center_x.saturating_sub(thickness / 2),
                start_y,
                thickness,
                end_y - start_y,
            ));
        }
        StrokeStyle::Double => {
            let thickness = stroke_thickness(StrokeStyle::Light, width_px, height_px);
            let offset = double_stroke_offset(width_px, height_px);
            rects.push(TerminalGeometricGlyphRect::new(
                center_x.saturating_sub(offset + thickness),
                start_y,
                thickness,
                end_y - start_y,
            ));
            rects.push(TerminalGeometricGlyphRect::new(
                (center_x + offset).min(width_px.saturating_sub(1)),
                start_y,
                thickness,
                end_y - start_y,
            ));
        }
    }
}

fn stroke_extent(style: StrokeStyle, cell_size: TerminalCellSize) -> u32 {
    match style {
        StrokeStyle::None => 0,
        StrokeStyle::Light | StrokeStyle::Heavy | StrokeStyle::Double => {
            stroke_thickness(style, cell_size.width_px(), cell_size.height_px()) / 2
        }
    }
}

fn stroke_thickness(style: StrokeStyle, width_px: u32, height_px: u32) -> u32 {
    let base = width_px.min(height_px).max(1);
    match style {
        StrokeStyle::None => 0,
        StrokeStyle::Light | StrokeStyle::Double => (base / 8).max(1),
        StrokeStyle::Heavy => (base / 4).max(2),
    }
}

fn double_stroke_offset(width_px: u32, height_px: u32) -> u32 {
    stroke_thickness(StrokeStyle::Light, width_px, height_px).max(1)
}

fn cell_width_px(cell_size: TerminalCellSize, column_span: u32) -> u32 {
    column_span.max(1).saturating_mul(cell_size.width_px())
}

fn fill_rect_glyph(
    columns: u32,
    rows: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> TerminalRasterGlyph {
    TerminalRasterGlyph::new(
        columns as u8,
        rows as u8,
        fill_rect_mask(columns, rows, x, y, width, height),
    )
}

fn fill_rect_mask(columns: u32, rows: u32, x: u32, y: u32, width: u32, height: u32) -> u64 {
    let max_x = (x + width).min(columns);
    let max_y = (y + height).min(rows);
    let mut mask = 0;
    for row in y..max_y {
        for column in x..max_x {
            mask |= cell_mask(columns, column, row);
        }
    }
    mask
}

fn cell_mask(columns: u32, column: u32, row: u32) -> u64 {
    1u64 << (row * columns + column)
}

fn raster_mask_contains(mask: u64, columns: u32, column: u32, row: u32) -> bool {
    mask & cell_mask(columns, column, row) != 0
}

fn scale_grid_axis(index: u32, size_px: u32, cells: u32) -> u32 {
    size_px.saturating_mul(index) / cells.max(1)
}

#[cfg(test)]
mod tests {
    use super::{TerminalGeometricGlyph, TerminalGeometricGlyphRect};
    use crate::pty_host::cell_size::TerminalCellSize;

    #[test]
    fn classifies_terminal_geometric_glyphs() {
        assert!(matches!(
            TerminalGeometricGlyph::from_char('│'),
            Some(TerminalGeometricGlyph::BoxDrawing(_))
        ));
        assert!(matches!(
            TerminalGeometricGlyph::from_char('▟'),
            Some(TerminalGeometricGlyph::Raster(_))
        ));
        assert!(matches!(
            TerminalGeometricGlyph::from_char('\u{1FB02}'),
            Some(TerminalGeometricGlyph::Raster(_))
        ));
        assert_eq!(TerminalGeometricGlyph::from_char('▒'), None);
        assert_eq!(TerminalGeometricGlyph::from_char('a'), None);
    }

    #[test]
    fn lower_half_block_uses_bottom_half_of_cell() {
        let rects = TerminalGeometricGlyph::from_char('▄')
            .expect("block element")
            .pixel_rects(TerminalCellSize::new(8, 16), 1);

        assert_eq!(rects, vec![TerminalGeometricGlyphRect::new(0, 8, 8, 8)]);
    }

    #[test]
    fn box_drawing_uses_centered_strokes() {
        let rects = TerminalGeometricGlyph::from_char('│')
            .expect("box drawing")
            .pixel_rects(TerminalCellSize::new(8, 16), 1);

        assert_eq!(
            rects,
            vec![
                TerminalGeometricGlyphRect::new(4, 0, 1, 8),
                TerminalGeometricGlyphRect::new(4, 8, 1, 8),
            ]
        );
    }

    #[test]
    fn sextant_range_maps_to_normalized_two_by_three_grid() {
        let rects = TerminalGeometricGlyph::from_char('\u{1FB02}')
            .expect("sextant glyph")
            .pixel_rects(TerminalCellSize::new(8, 16), 1);

        assert_eq!(rects, vec![TerminalGeometricGlyphRect::new(0, 0, 8, 5)]);
    }

    #[test]
    fn sextant_gap_codepoints_follow_unicode_order_not_linear_bitmask() {
        let rects = TerminalGeometricGlyph::from_char('\u{1FB14}')
            .expect("sextant glyph after unicode gap")
            .pixel_rects(TerminalCellSize::new(8, 18), 1);

        assert_eq!(
            rects,
            vec![
                TerminalGeometricGlyphRect::new(4, 0, 4, 6),
                TerminalGeometricGlyphRect::new(0, 6, 4, 12),
            ]
        );
    }

    #[test]
    fn shade_glyphs_stay_on_font_path() {
        assert_eq!(TerminalGeometricGlyph::from_char('░'), None);
        assert_eq!(TerminalGeometricGlyph::from_char('▒'), None);
        assert_eq!(TerminalGeometricGlyph::from_char('▓'), None);
    }
}
