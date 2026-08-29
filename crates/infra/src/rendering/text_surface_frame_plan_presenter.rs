use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    fs::OpenOptions,
    io::Cursor,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt as _;

use germinal_gnative_protocol::shared_rgba::{
    SHARED_RGBA_HEADER_BYTES, SHARED_RGBA_MAGIC, SHARED_RGBA_SLOT_FREE, SHARED_RGBA_SLOT_READING,
    SHARED_RGBA_SLOT_READY, SharedRgbaLayout,
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
            RenderSurfaceImageSnapshot, RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot,
            RenderSurfaceSnapshot, RenderSurfaceSnapshotProvider,
        },
    },
    seq::Seq,
};
use memmap2::{MmapMut, MmapOptions};

const PIXEL_RECT_ROW_BASE: u32 = u32::MAX - 4096;

#[derive(Clone, Default)]
pub struct TextSurfaceFramePlanPresenter {
    inner: Rc<RefCell<HashMap<RenderTargetId, TextSurface>>>,
    shared_frames: Rc<RefCell<HashMap<PathBuf, SharedFrameMapping>>>,
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

        present_incremental(surface, frame, &mut self.shared_frames.borrow_mut());
        surface.latest_seq = frame.seq;
        true
    }
}

fn present_incremental(
    surface: &mut TextSurface,
    frame: &BuiltFramePlan,
    shared_frames: &mut HashMap<PathBuf, SharedFrameMapping>,
) {
    let mut dirty_rows = BTreeSet::new();
    let mut full_damage = false;

    for command in &frame.commands {
        match command {
            RenderCommandDto::Clear => {
                surface.rows.clear();
                surface.pixel_rects.clear();
                surface.image_surfaces.clear();
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
            RenderCommandDto::PngSurface { .. } => {
                if let Some(image) = image_surface_snapshot_of(command) {
                    surface
                        .image_surfaces
                        .retain(|current| current.id != image.id);
                    surface.image_surfaces.push(image);
                }
                full_damage = true;
            }
            RenderCommandDto::SharedRgbaSurface { .. } => {
                if let Some(image) = shared_image_surface_snapshot_of(command, shared_frames) {
                    surface
                        .image_surfaces
                        .retain(|current| current.id != image.id);
                    surface.image_surfaces.push(image);
                }
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
    let mut cleared_rows = BTreeSet::new();
    let mut full_damage = false;

    for command in &frame.commands {
        match command {
            RenderCommandDto::Clear => {
                surface.rows.clear();
                surface.pixel_rects.clear();
                surface.image_surfaces.clear();
                staged_rows.clear();
                staged_pixel_rects.clear();
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
            RenderCommandDto::PngSurface { .. } | RenderCommandDto::SharedRgbaSurface { .. } => {
                return None;
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
                        decoration: Default::default(),
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
                        decoration: Default::default(),
                    }],
                });
            }
        }

        Some(RenderSurfaceSnapshot {
            target_id,
            latest_seq: surface.latest_seq,
            default_background: RgbColorDto::new(0, 0, 0),
            rows,
            image_surfaces: surface.image_surfaces.clone(),
            dirty_rows: surface.latest_dirty_rows.clone(),
            cursor: None,
            ime_preedit: None,
        })
    }
}

fn image_surface_snapshot_of(command: &RenderCommandDto) -> Option<RenderSurfaceImageSnapshot> {
    let RenderCommandDto::PngSurface {
        id,
        generation,
        x_px,
        y_px,
        width_px,
        height_px,
        png,
    } = command
    else {
        return None;
    };
    let mut decoder = png::Decoder::new(Cursor::new(png));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let image_width_px = reader.info().width;
    let image_height_px = reader.info().height;
    if image_width_px != *width_px || image_height_px != *height_px {
        return None;
    }
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut pixels).ok()?;
    pixels.truncate(info.buffer_size());
    let pixel_count = usize::try_from(image_width_px)
        .ok()?
        .checked_mul(usize::try_from(image_height_px).ok()?)?;
    let mut rgba = Vec::with_capacity(pixel_count.checked_mul(4)?);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(&pixels),
        png::ColorType::Rgb => {
            for rgb in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for gray in pixels {
                rgba.extend_from_slice(&[gray, gray, gray, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for gray_alpha in pixels.chunks_exact(2) {
                rgba.extend_from_slice(&[
                    gray_alpha[0],
                    gray_alpha[0],
                    gray_alpha[0],
                    gray_alpha[1],
                ]);
            }
        }
        png::ColorType::Indexed => return None,
    }
    (rgba.len() == pixel_count.checked_mul(4)?).then_some(RenderSurfaceImageSnapshot {
        id: id.clone(),
        image_id: 0,
        image_generation: *generation,
        x_cell: 0,
        y_cell: 0,
        x_offset_px: *x_px,
        y_offset_px: *y_px,
        columns: 0,
        rows: 0,
        source_x_px: 0,
        source_y_px: 0,
        source_width_px: image_width_px,
        source_height_px: image_height_px,
        image_width_px,
        image_height_px,
        clip_top_cell: None,
        clip_bottom_cell: None,
        z_index: 0,
        rgba: Arc::from(rgba),
    })
}

fn shared_image_surface_snapshot_of(
    command: &RenderCommandDto,
    mappings: &mut HashMap<PathBuf, SharedFrameMapping>,
) -> Option<RenderSurfaceImageSnapshot> {
    let RenderCommandDto::SharedRgbaSurface {
        id,
        generation,
        path,
        slot,
        x_px,
        y_px,
        width_px,
        height_px,
        stride_bytes,
    } = command
    else {
        return None;
    };
    let layout = SharedRgbaLayout::new(*width_px, *height_px)?;
    if layout.stride_bytes != *stride_bytes {
        return None;
    }
    let path = PathBuf::from(path);
    if !mappings.contains_key(&path) {
        mappings.insert(path.clone(), SharedFrameMapping::open(&path, layout)?);
    }
    let rgba = mappings.get_mut(&path)?.take_frame(*slot, *generation)?;
    Some(RenderSurfaceImageSnapshot {
        id: id.clone(),
        image_id: 0,
        image_generation: *generation,
        x_cell: 0,
        y_cell: 0,
        x_offset_px: *x_px,
        y_offset_px: *y_px,
        columns: 0,
        rows: 0,
        source_x_px: 0,
        source_y_px: 0,
        source_width_px: *width_px,
        source_height_px: *height_px,
        image_width_px: *width_px,
        image_height_px: *height_px,
        clip_top_cell: None,
        clip_bottom_cell: None,
        z_index: 0,
        rgba,
    })
}

struct SharedFrameMapping {
    mmap: MmapMut,
    layout: SharedRgbaLayout,
}

impl SharedFrameMapping {
    fn open(path: &Path, layout: SharedRgbaLayout) -> Option<Self> {
        let runtime_dir = PathBuf::from(env::var_os("XDG_RUNTIME_DIR")?)
            .canonicalize()
            .ok()?;
        let path = path.canonicalize().ok()?;
        if !path.starts_with(&runtime_dir) {
            return None;
        }
        let metadata = path.metadata().ok()?;
        if !metadata.is_file() || metadata.len() != u64::try_from(layout.file_len()?).ok()? {
            return None;
        }
        #[cfg(unix)]
        if metadata.uid() != unsafe { nix::libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return None;
        }
        let file = OpenOptions::new().read(true).write(true).open(path).ok()?;
        let mmap = unsafe { MmapOptions::new().map_mut(&file) }.ok()?;
        if mmap.len() < SHARED_RGBA_HEADER_BYTES
            || mmap[..SHARED_RGBA_MAGIC.len()] != SHARED_RGBA_MAGIC
            || read_u32(&mmap, 8)? != layout.width_px
            || read_u32(&mmap, 12)? != layout.height_px
            || read_u32(&mmap, 16)? != layout.stride_bytes
            || read_u32(&mmap, 20)? != layout.slot_count
        {
            return None;
        }
        Some(Self { mmap, layout })
    }

    fn take_frame(&mut self, slot: u32, generation: u64) -> Option<Arc<[u8]>> {
        let header_offset = self.layout.slot_header_offset(slot)?;
        let state = unsafe { &*self.mmap.as_ptr().add(header_offset).cast::<AtomicU32>() };
        state
            .compare_exchange(
                SHARED_RGBA_SLOT_READY,
                SHARED_RGBA_SLOT_READING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()?;
        let generation_value = unsafe {
            &*self
                .mmap
                .as_ptr()
                .add(header_offset + 8)
                .cast::<AtomicU64>()
        }
        .load(Ordering::Acquire);
        if generation_value != generation {
            state.store(SHARED_RGBA_SLOT_FREE, Ordering::Release);
            return None;
        }
        let data_offset = self.layout.slot_data_offset(slot)?;
        let frame_bytes = self.layout.frame_bytes()?;
        let rgba = Arc::<[u8]>::from(&self.mmap[data_offset..data_offset + frame_bytes]);
        state.store(SHARED_RGBA_SLOT_FREE, Ordering::Release);
        Some(rgba)
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSurface {
    pub latest_seq: Seq,
    pub latest_dirty_rows: Vec<u32>,
    rows: BTreeMap<u32, TextSurfaceRow>,
    pixel_rects: Vec<RenderCommandDto>,
    image_surfaces: Vec<RenderSurfaceImageSnapshot>,
}

impl Default for TextSurface {
    fn default() -> Self {
        Self {
            latest_seq: Seq::ZERO,
            latest_dirty_rows: Vec::new(),
            rows: BTreeMap::new(),
            pixel_rects: Vec::new(),
            image_surfaces: Vec::new(),
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
