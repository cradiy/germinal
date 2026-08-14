use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    rc::Rc,
};

use germinal_ports::{
    pty_host::width::{
        terminal_char_cell_advance, terminal_chars_cell_width, terminal_text_cell_width,
    },
    rendering::{
        frame_plan_builder::{
            BuiltFramePlan, RenderCommandDto, RgbColorDto, TextStyleDto,
            encode_pixel_fill_rect_command,
        },
        frame_plan_presenter::FramePlanPresenter,
        render_target_id::RenderTargetId,
        surface_snapshot::{
            RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot, RenderSurfaceSnapshot,
            RenderSurfaceSnapshotProvider, RenderSurfaceVideoSurfaceSnapshot,
        },
    },
    seq::Seq,
};

const PIXEL_RECT_ROW_BASE: u32 = u32::MAX - 4096;

#[derive(Debug, Clone, Default)]
pub struct TextSurfaceFramePlanPresenter {
    inner: Rc<RefCell<HashMap<RenderTargetId, TextSurface>>>,
}

impl TextSurfaceFramePlanPresenter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn surface_of(&self, target_id: RenderTargetId) -> Option<TextSurface> {
        self.inner.borrow().get(&target_id).cloned()
    }
}

impl FramePlanPresenter for TextSurfaceFramePlanPresenter {
    fn present(&self, frame: &BuiltFramePlan) -> bool {
        let mut inner = self.inner.borrow_mut();
        if inner
            .get(&frame.target_id)
            .is_some_and(|surface| surface.latest_seq >= frame.seq)
        {
            return false;
        }
        let surface = inner.entry(frame.target_id).or_default();

        if let Some((full_damage, dirty_rows)) = try_present_fast(surface, frame) {
            surface.latest_seq = frame.seq;
            surface.latest_dirty_rows = if full_damage { Vec::new() } else { dirty_rows };
            return true;
        }

        present_incremental(surface, frame);
        surface.latest_seq = frame.seq;
        true
    }
}

fn present_incremental(surface: &mut TextSurface, frame: &BuiltFramePlan) {
    let mut dirty_rows = BTreeSet::new();
    let mut full_damage = false;

    for command in &frame.commands {
        match command {
            RenderCommandDto::Clear => {
                surface.rows.clear();
                surface.pixel_rects.clear();
                surface.video_surfaces.clear();
                full_damage = true;
            }
            RenderCommandDto::ClearLine { y } => {
                dirty_rows.insert(*y);
                surface.rows.remove(y);
            }
            RenderCommandDto::TextRun { x, y, text } => {
                dirty_rows.insert(*y);
                surface.apply_text_run(*x, *y, text, TextStyleDto::plain());
            }
            RenderCommandDto::StyledTextRun { x, y, text, style } => {
                dirty_rows.insert(*y);
                surface.apply_text_run(*x, *y, text, *style);
            }
            RenderCommandDto::PixelFillRect { .. } => {
                surface.pixel_rects.push(command.clone());
                full_damage = true;
            }
            RenderCommandDto::VideoSurface { .. } => {
                surface
                    .video_surfaces
                    .push(video_surface_snapshot_of(command));
                full_damage = true;
            }
        }
    }

    surface.latest_dirty_rows = if full_damage {
        Vec::new()
    } else {
        dirty_rows.into_iter().collect()
    };
}

fn try_present_fast(surface: &mut TextSurface, frame: &BuiltFramePlan) -> Option<(bool, Vec<u32>)> {
    let mut dirty_rows = BTreeSet::new();
    let mut staged_rows: BTreeMap<u32, Vec<TextSurfaceRun>> = BTreeMap::new();
    let mut staged_pixel_rects = Vec::<RenderCommandDto>::new();
    let mut staged_video_surfaces = Vec::<RenderSurfaceVideoSurfaceSnapshot>::new();
    let mut cleared_rows = BTreeSet::new();
    let mut full_damage = false;

    for command in &frame.commands {
        match command {
            RenderCommandDto::Clear => {
                surface.rows.clear();
                surface.pixel_rects.clear();
                surface.video_surfaces.clear();
                staged_rows.clear();
                staged_pixel_rects.clear();
                staged_video_surfaces.clear();
                cleared_rows.clear();
                dirty_rows.clear();
                full_damage = true;
            }
            RenderCommandDto::ClearLine { y } => {
                dirty_rows.insert(*y);
                cleared_rows.insert(*y);
                staged_rows.insert(*y, Vec::new());
            }
            RenderCommandDto::TextRun { x, y, text } => {
                if !full_damage && !cleared_rows.contains(y) {
                    return None;
                }
                dirty_rows.insert(*y);
                staged_rows.entry(*y).or_default().push(TextSurfaceRun {
                    x: *x,
                    text: text.clone(),
                    style: TextStyleDto::plain(),
                });
            }
            RenderCommandDto::StyledTextRun { x, y, text, style } => {
                if !full_damage && !cleared_rows.contains(y) {
                    return None;
                }
                dirty_rows.insert(*y);
                staged_rows.entry(*y).or_default().push(TextSurfaceRun {
                    x: *x,
                    text: text.clone(),
                    style: *style,
                });
            }
            RenderCommandDto::PixelFillRect { .. } => {
                if !full_damage {
                    return None;
                }
                staged_pixel_rects.push(command.clone());
            }
            RenderCommandDto::VideoSurface { .. } => {
                if !full_damage {
                    return None;
                }
                staged_video_surfaces.push(video_surface_snapshot_of(command));
            }
        }
    }

    for (y, mut runs) in staged_rows {
        if runs.is_empty() {
            surface.rows.remove(&y);
            continue;
        }
        runs.sort_by_key(|run| run.x);
        surface.rows.insert(y, TextSurfaceRow { runs });
    }
    if full_damage {
        surface.pixel_rects = staged_pixel_rects;
        surface.video_surfaces = staged_video_surfaces;
    }

    Some((
        full_damage,
        if full_damage {
            Vec::new()
        } else {
            dirty_rows.into_iter().collect()
        },
    ))
}

impl RenderSurfaceSnapshotProvider for TextSurfaceFramePlanPresenter {
    fn surface_snapshot_of(&self, target_id: RenderTargetId) -> Option<RenderSurfaceSnapshot> {
        let inner = self.inner.borrow();
        let surface = inner.get(&target_id)?;
        let mut rows: Vec<_> = surface
            .rows
            .iter()
            .map(|(y, row)| RenderSurfaceRowSnapshot {
                y: *y,
                runs: row
                    .runs
                    .iter()
                    .map(|run| RenderSurfaceRunSnapshot {
                        x: run.x,
                        text: run.text.clone(),
                        style: run.style,
                    })
                    .collect(),
            })
            .collect();

        for (index, command) in surface.pixel_rects.iter().enumerate() {
            if let Some(text) = encode_pixel_fill_rect_command(command) {
                rows.push(RenderSurfaceRowSnapshot {
                    y: PIXEL_RECT_ROW_BASE + index as u32,
                    runs: vec![RenderSurfaceRunSnapshot {
                        x: 0,
                        text,
                        style: TextStyleDto::plain(),
                    }],
                });
            }
        }

        Some(RenderSurfaceSnapshot {
            target_id,
            latest_seq: surface.latest_seq,
            default_background: RgbColorDto::new(0, 0, 0),
            rows,
            video_surfaces: surface.video_surfaces.clone(),
            image_surfaces: Vec::new(),
            dirty_rows: surface.latest_dirty_rows.clone(),
            cursor: None,
            ime_preedit: None,
        })
    }
}

fn video_surface_snapshot_of(command: &RenderCommandDto) -> RenderSurfaceVideoSurfaceSnapshot {
    let RenderCommandDto::VideoSurface {
        id,
        x_px,
        y_px,
        width_px,
        height_px,
    } = command
    else {
        panic!("video_surface_snapshot_of requires a VideoSurface command");
    };

    RenderSurfaceVideoSurfaceSnapshot {
        id: id.clone(),
        x_px: *x_px,
        y_px: *y_px,
        width_px: *width_px,
        height_px: *height_px,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSurface {
    pub latest_seq: Seq,
    pub latest_dirty_rows: Vec<u32>,
    rows: BTreeMap<u32, TextSurfaceRow>,
    pixel_rects: Vec<RenderCommandDto>,
    video_surfaces: Vec<RenderSurfaceVideoSurfaceSnapshot>,
}

impl Default for TextSurface {
    fn default() -> Self {
        Self {
            latest_seq: Seq::ZERO,
            latest_dirty_rows: Vec::new(),
            rows: BTreeMap::new(),
            pixel_rects: Vec::new(),
            video_surfaces: Vec::new(),
        }
    }
}

impl TextSurface {
    pub fn text_at(&self, row: u32) -> Option<String> {
        self.rows.get(&row).map(TextSurfaceRow::text)
    }

    pub fn line_texts(&self) -> Vec<String> {
        self.rows.values().map(TextSurfaceRow::text).collect()
    }

    pub fn row_runs(&self, row: u32) -> Option<&[TextSurfaceRun]> {
        self.rows.get(&row).map(|row| row.runs.as_slice())
    }

    pub fn rows(&self) -> &BTreeMap<u32, TextSurfaceRow> {
        &self.rows
    }

    fn apply_text_run(&mut self, x: u32, y: u32, text: &str, style: TextStyleDto) {
        if text.is_empty() {
            return;
        }
        let row = self.rows.entry(y).or_default();
        row.apply_run(TextSurfaceRun {
            x,
            text: text.to_string(),
            style,
        });
        if row.runs.is_empty() {
            self.rows.remove(&y);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextSurfaceRow {
    runs: Vec<TextSurfaceRun>,
}

impl TextSurfaceRow {
    pub fn runs(&self) -> &[TextSurfaceRun] {
        &self.runs
    }

    pub fn text(&self) -> String {
        let mut result = String::new();
        for run in &self.runs {
            while terminal_text_cell_width(&result) < run.x {
                result.push(' ');
            }
            replace_text_at(&mut result, run.x as usize, &run.text);
        }
        result
    }

    fn apply_run(&mut self, run: TextSurfaceRun) {
        let run_start = run.x;
        let run_end = run.x + terminal_text_cell_width(&run.text);
        self.runs.retain(|existing| {
            let existing_start = existing.x;
            let existing_end = existing.x + terminal_text_cell_width(&existing.text);
            existing_end <= run_start || existing_start >= run_end
        });
        self.runs.push(run);
        self.runs.sort_by_key(|run| run.x);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSurfaceRun {
    pub x: u32,
    pub text: String,
    pub style: TextStyleDto,
}

fn replace_text_at(target: &mut String, x: usize, text: &str) {
    let mut chars: Vec<char> = target.chars().collect();
    while terminal_text_cell_width_of_chars(&chars) < x as u32 {
        chars.push(' ');
    }
    let mut index = 0usize;
    let mut cell_x = 0u32;
    while index < chars.len() && cell_x < x as u32 {
        cell_x += terminal_char_cell_advance(chars[index]);
        index += 1;
    }
    let text_width = terminal_text_cell_width(text);
    let mut remove_count = 0usize;
    let mut removed_width = 0u32;
    while index + remove_count < chars.len() && removed_width < text_width {
        removed_width += terminal_char_cell_advance(chars[index + remove_count]);
        remove_count += 1;
    }
    chars.splice(index..index + remove_count, text.chars());
    *target = chars.into_iter().collect();
}

fn terminal_text_cell_width_of_chars(chars: &[char]) -> u32 {
    terminal_chars_cell_width(chars)
}

#[cfg(test)]
mod tests {
    use germinal_ports::rendering::frame_plan_builder::BuiltFramePlan;

    use super::*;

    fn text_frame(target_id: RenderTargetId, seq: u64, text: &str) -> BuiltFramePlan {
        BuiltFramePlan {
            target_id,
            seq: Seq::new(seq),
            commands: vec![
                RenderCommandDto::Clear,
                RenderCommandDto::TextRun {
                    x: 0,
                    y: 0,
                    text: text.to_string(),
                },
            ],
        }
    }

    #[test]
    fn rejects_duplicate_and_stale_frames_without_rolling_back_surface() {
        let presenter = TextSurfaceFramePlanPresenter::new();
        let target_id = RenderTargetId::new(7);

        assert!(presenter.present(&text_frame(target_id, 2, "new")));
        assert!(!presenter.present(&text_frame(target_id, 2, "duplicate")));
        assert!(!presenter.present(&text_frame(target_id, 1, "stale")));

        let surface = presenter
            .surface_of(target_id)
            .expect("surface should remain available");
        assert_eq!(surface.latest_seq, Seq::new(2));
        assert_eq!(surface.text_at(0).as_deref(), Some("new"));
    }

    #[test]
    fn sequence_order_is_tracked_independently_per_target() {
        let presenter = TextSurfaceFramePlanPresenter::new();
        let first_target = RenderTargetId::new(1);
        let second_target = RenderTargetId::new(2);

        assert!(presenter.present(&text_frame(first_target, 3, "first")));
        assert!(presenter.present(&text_frame(second_target, 1, "second")));

        assert_eq!(
            presenter.surface_of(first_target).unwrap().latest_seq,
            Seq::new(3)
        );
        assert_eq!(
            presenter.surface_of(second_target).unwrap().latest_seq,
            Seq::new(1)
        );
    }
}
