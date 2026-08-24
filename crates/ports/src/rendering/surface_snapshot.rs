use std::sync::Arc;

use crate::{
    pty_host::width::terminal_char_cell_width,
    rendering::{
        frame_plan_builder::{RgbColorDto, TextStyleDto},
        render_target_id::RenderTargetId,
    },
    seq::Seq,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceSnapshot {
    pub target_id: RenderTargetId,
    pub latest_seq: Seq,
    pub default_background: RgbColorDto,
    pub rows: Vec<RenderSurfaceRowSnapshot>,
    pub video_surfaces: Vec<RenderSurfaceVideoSurfaceSnapshot>,
    pub image_surfaces: Vec<RenderSurfaceImageSnapshot>,
    pub dirty_rows: Vec<u32>,
    pub cursor: Option<RenderSurfaceCursorSnapshot>,
    pub ime_preedit: Option<RenderSurfaceImePreeditSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceImePreeditSnapshot {
    pub text: String,
    pub cursor_range: Option<(usize, usize)>,
}

impl RenderSurfaceImePreeditSnapshot {
    pub fn cursor_cell(
        &self,
        origin: RenderSurfaceCursorSnapshot,
        columns: u32,
        rows: u32,
    ) -> Option<(u32, u32)> {
        let (_, cursor_byte) = self.cursor_range?;
        let cursor_byte = cursor_byte.min(self.text.len());
        let columns = columns.max(1);
        let rows = rows.max(1);
        let mut x = origin.x.min(columns - 1);
        let mut y = origin.y.min(rows - 1);

        for (byte_index, character) in self.text.char_indices() {
            if byte_index >= cursor_byte {
                break;
            }
            let width = terminal_char_cell_width(character);
            if width == 0 {
                continue;
            }
            if x.saturating_add(width) > columns {
                x = 0;
                y = y.saturating_add(1);
            }
            if y >= rows {
                return Some((columns - 1, rows - 1));
            }
            x = x.saturating_add(width);
            if x >= columns {
                x = 0;
                y = y.saturating_add(1);
            }
        }

        Some((x.min(columns - 1), y.min(rows - 1)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceRowSnapshot {
    pub y: u32,
    pub runs: Vec<RenderSurfaceRunSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceRunSnapshot {
    pub x: u32,
    pub text: String,
    pub style: TextStyleDto,
    pub decoration: RenderSurfaceTextDecoration,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RenderSurfaceTextDecoration {
    pub underline: RenderSurfaceUnderlineStyle,
    pub underline_color: Option<RgbColorDto>,
    pub strikeout: bool,
    pub dim: bool,
    pub hidden: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum RenderSurfaceUnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceVideoSurfaceSnapshot {
    pub id: String,
    pub x_px: u32,
    pub y_px: u32,
    pub width_px: u32,
    pub height_px: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderSurfaceImageSnapshot {
    pub id: String,
    pub image_id: u32,
    pub image_generation: u64,
    pub x_cell: i32,
    pub y_cell: i32,
    pub x_offset_px: u32,
    pub y_offset_px: u32,
    pub columns: u32,
    pub rows: u32,
    pub source_x_px: u32,
    pub source_y_px: u32,
    pub source_width_px: u32,
    pub source_height_px: u32,
    pub image_width_px: u32,
    pub image_height_px: u32,
    pub clip_top_cell: Option<i32>,
    pub clip_bottom_cell: Option<i32>,
    pub z_index: i32,
    pub rgba: Arc<[u8]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderSurfaceCursorSnapshot {
    pub x: u32,
    pub y: u32,
    pub focused: bool,
    pub shape: RenderSurfaceCursorShape,
    pub blinking: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum RenderSurfaceCursorShape {
    #[default]
    Block,
    Underline,
    Beam,
    HollowBlock,
    Hidden,
}

pub trait RenderSurfaceSnapshotProvider {
    fn surface_snapshot_of(&self, target_id: RenderTargetId) -> Option<RenderSurfaceSnapshot>;
}

#[cfg(test)]
mod tests {
    use super::{
        RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot, RenderSurfaceImePreeditSnapshot,
    };

    fn origin() -> RenderSurfaceCursorSnapshot {
        RenderSurfaceCursorSnapshot {
            x: 3,
            y: 0,
            focused: true,
            shape: RenderSurfaceCursorShape::Block,
            blinking: false,
        }
    }

    #[test]
    fn ime_cursor_cell_wraps_wide_characters_by_terminal_cell_width() {
        let preedit = RenderSurfaceImePreeditSnapshot {
            text: "你a".to_string(),
            cursor_range: Some((3, 3)),
        };

        assert_eq!(preedit.cursor_cell(origin(), 4, 2), Some((2, 1)));
    }

    #[test]
    fn ime_cursor_cell_stays_at_origin_for_a_zero_length_selection() {
        let preedit = RenderSurfaceImePreeditSnapshot {
            text: "你a".to_string(),
            cursor_range: Some((0, 0)),
        };

        assert_eq!(preedit.cursor_cell(origin(), 4, 2), Some((3, 0)));
    }

    #[test]
    fn ime_cursor_cell_is_hidden_when_the_ime_omits_its_cursor() {
        let preedit = RenderSurfaceImePreeditSnapshot {
            text: "你a".to_string(),
            cursor_range: None,
        };

        assert_eq!(preedit.cursor_cell(origin(), 4, 2), None);
    }
}
