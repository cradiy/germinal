use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Instant,
};

use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    term::{Config, Term, TermDamage, TermMode, cell::Flags, color::Colors, point_to_viewport},
    vte::ansi::{Color, CursorShape, NamedColor, Processor, Rgb, StdSyncHandler},
};
use germinal_ports::{
    pty_host::{
        snapshot::{
            TerminalLineSnapshot, TerminalSnapshot, TerminalSnapshotProvider,
            TerminalTextRunSnapshot,
        },
        terminal_input_mode::TerminalInputModes,
        width::terminal_char_cell_width,
    },
    rendering::{
        frame_plan_builder::{RgbColorDto, TextStyleDto},
        render_target_id::RenderTargetId,
        surface_snapshot::{
            RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot, RenderSurfaceRowSnapshot,
            RenderSurfaceRunSnapshot, RenderSurfaceSnapshot,
        },
    },
    seq::Seq,
};

use super::kitty_graphics::{
    KittyGraphicsState, KittyGraphicsStreamDecoder, KittyPlaceholderCell, KittyStreamEvent,
};

const KITTY_IMAGE_PLACEHOLDER: char = '\u{10EEEE}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlacrittyTermSize {
    columns: usize,
    screen_lines: usize,
}

impl AlacrittyTermSize {
    pub const fn new(columns: usize, screen_lines: usize) -> Self {
        Self {
            columns,
            screen_lines,
        }
    }

    pub const fn columns(self) -> usize {
        self.columns
    }

    pub const fn screen_lines(self) -> usize {
        self.screen_lines
    }
}

impl Default for AlacrittyTermSize {
    fn default() -> Self {
        Self {
            columns: 80,
            screen_lines: 24,
        }
    }
}

impl Dimensions for AlacrittyTermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

#[derive(Clone)]
struct PtyWriteEventListener {
    pending_writes: Sender<Vec<u8>>,
}

impl PtyWriteEventListener {
    fn new(pending_writes: Sender<Vec<u8>>) -> Self {
        Self { pending_writes }
    }
}

impl EventListener for PtyWriteEventListener {
    fn send_event(&self, event: Event) {
        let Event::PtyWrite(text) = event else {
            return;
        };

        let _ = self.pending_writes.send(text.into_bytes());
    }
}

#[derive(Clone)]
pub struct AlacrittyTerminalStore {
    inner: Rc<RefCell<HashMap<RenderTargetId, AlacrittyTermState>>>,
    size: AlacrittyTermSize,
    scrollback_history: usize,
}

impl AlacrittyTerminalStore {
    pub fn new() -> Self {
        Self::with_size(AlacrittyTermSize::default())
    }

    pub fn with_size(size: AlacrittyTermSize) -> Self {
        Self::with_size_and_scrollback_history(size, Config::default().scrolling_history)
    }

    pub fn with_size_and_scrollback_history(
        size: AlacrittyTermSize,
        scrollback_history: usize,
    ) -> Self {
        Self {
            inner: Rc::new(RefCell::new(HashMap::new())),
            size,
            scrollback_history,
        }
    }

    pub fn size(&self) -> AlacrittyTermSize {
        self.size
    }

    pub fn apply_bytes(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        bytes: &[u8],
    ) -> AlacrittyTermApplyStats {
        let mut inner = self.inner.borrow_mut();

        let state = inner
            .entry(render_target_id)
            .or_insert_with(|| AlacrittyTermState::new(self.size, self.scrollback_history));

        for event in state.graphics_decoder.feed(bytes) {
            match event {
                KittyStreamEvent::Bytes(visible) => {
                    state.processor.advance(&mut state.term, &visible);
                }
                KittyStreamEvent::Command(command) => {
                    let point = state.term.grid().cursor.point;
                    let cursor = (
                        u32::try_from(point.column.0).unwrap_or(0),
                        u32::try_from(point.line.0).unwrap_or(0),
                    );
                    let result = state.graphics.handle(command, cursor);
                    if let Some(response) = result.response {
                        let _ = state.pending_write_tx.send(response);
                    }
                    if let Some(cursor_move) = result.cursor_move {
                        if cursor_move.columns > 0 {
                            let bytes = format!("\x1b[{}C", cursor_move.columns);
                            state.processor.advance(&mut state.term, bytes.as_bytes());
                        }
                        if cursor_move.rows > 0 {
                            let bytes = format!("\x1b[{}B", cursor_move.rows);
                            state.processor.advance(&mut state.term, bytes.as_bytes());
                        }
                    }
                }
            }
        }

        state.latest_seq = seq;
        state.total_bytes += bytes.len() as u64;
        state.chunk_count += 1;

        AlacrittyTermApplyStats {
            latest_seq: state.latest_seq,
            total_bytes: state.total_bytes,
            chunk_count: state.chunk_count,
        }
    }

    pub fn resize(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        size: AlacrittyTermSize,
    ) -> AlacrittyTermApplyStats {
        let mut inner = self.inner.borrow_mut();

        let state = inner
            .entry(render_target_id)
            .or_insert_with(|| AlacrittyTermState::new(size, self.scrollback_history));

        state.resize(size);
        state.latest_seq = seq;

        AlacrittyTermApplyStats {
            latest_seq: state.latest_seq,
            total_bytes: state.total_bytes,
            chunk_count: state.chunk_count,
        }
    }

    pub fn scroll_display(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        scroll: Scroll,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };

        let previous_offset = state.term.grid().display_offset();
        state.term.scroll_display(scroll);
        if state.term.grid().display_offset() == previous_offset {
            return false;
        }

        state.latest_seq = seq;
        true
    }

    fn snapshot_from_state(
        render_target_id: RenderTargetId,
        state: &mut AlacrittyTermState,
    ) -> TerminalSnapshot {
        let (lines, text_runs) = visible_lines_and_runs(&state.term);
        let dirty_rows = dirty_rows_of(state.term.damage(), state.size.screen_lines());

        TerminalSnapshot {
            render_target_id,
            latest_seq: state.latest_seq,
            lines,
            text_runs,
            dirty_rows,
        }
    }

    fn render_surface_snapshot_from_state(
        render_target_id: RenderTargetId,
        state: &mut AlacrittyTermState,
    ) -> RenderSurfaceSnapshot {
        let rows = visible_surface_rows(&state.term);
        let dirty_rows = dirty_rows_of(state.term.damage(), state.size.screen_lines());

        let placeholder_cells = kitty_placeholder_cells(&state.term);
        let renderable = state.term.renderable_content();
        let default_background = renderable.colors[NamedColor::Background]
            .map(rgb_to_dto)
            .or_else(|| dominant_background_of(&rows))
            .unwrap_or(RgbColorDto::new(0, 0, 0));

        RenderSurfaceSnapshot {
            target_id: render_target_id,
            latest_seq: state.latest_seq,
            default_background,
            rows,
            video_surfaces: Vec::new(),
            image_surfaces: state.graphics.snapshots(&placeholder_cells),
            dirty_rows,
            cursor: None,
        }
    }

    pub fn stats_of(&self, render_target_id: RenderTargetId) -> Option<AlacrittyTermApplyStats> {
        let inner = self.inner.borrow();

        let state = inner.get(&render_target_id)?;

        Some(AlacrittyTermApplyStats {
            latest_seq: state.latest_seq,
            total_bytes: state.total_bytes,
            chunk_count: state.chunk_count,
        })
    }

    pub fn take_pending_pty_writes(&self, render_target_id: RenderTargetId) -> Vec<Vec<u8>> {
        let mut inner = self.inner.borrow_mut();

        let Some(state) = inner.get_mut(&render_target_id) else {
            return Vec::new();
        };

        state.take_pending_writes()
    }

    pub fn input_modes(&self, render_target_id: RenderTargetId) -> TerminalInputModes {
        let inner = self.inner.borrow();
        let Some(state) = inner.get(&render_target_id) else {
            return TerminalInputModes::default();
        };
        let mode = state.term.mode();

        TerminalInputModes::new(
            mode.contains(TermMode::APP_CURSOR),
            mode.contains(TermMode::BRACKETED_PASTE),
            mode.contains(TermMode::FOCUS_IN_OUT),
            mode.contains(TermMode::SGR_MOUSE),
            mode.contains(TermMode::MOUSE_REPORT_CLICK),
            mode.contains(TermMode::MOUSE_DRAG),
            mode.contains(TermMode::MOUSE_MOTION),
        )
    }

    pub fn synchronized_update_pending(&self, render_target_id: RenderTargetId) -> bool {
        let inner = self.inner.borrow();
        inner
            .get(&render_target_id)
            .is_some_and(AlacrittyTermState::synchronized_update_pending)
    }

    pub fn synchronized_update_deadline(
        &self,
        render_target_id: RenderTargetId,
    ) -> Option<Instant> {
        let inner = self.inner.borrow();
        inner.get(&render_target_id)?.synchronized_update_deadline()
    }

    pub fn finish_expired_synchronized_update(
        &self,
        render_target_id: RenderTargetId,
        now: Instant,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };

        state.finish_expired_synchronized_update(now)
    }

    pub fn cursor_position_1_based(
        &self,
        render_target_id: RenderTargetId,
    ) -> Option<(usize, usize)> {
        let inner = self.inner.borrow();

        let state = inner.get(&render_target_id)?;
        if !state.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }

        let point = state.term.grid().cursor.point;

        Some((point.line.0 as usize + 1, point.column.0 + 1))
    }

    pub fn cursor_position_0_based(&self, render_target_id: RenderTargetId) -> Option<(u32, u32)> {
        let inner = self.inner.borrow();

        let state = inner.get(&render_target_id)?;
        if !state.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        let point = state.term.grid().cursor.point;

        let row = u32::try_from(point.line.0).ok()?;
        let col = u32::try_from(point.column.0).ok()?;

        Some((col, row))
    }

    pub fn cursor_snapshot(
        &self,
        render_target_id: RenderTargetId,
    ) -> Option<RenderSurfaceCursorSnapshot> {
        let inner = self.inner.borrow();
        let state = inner.get(&render_target_id)?;
        if state.term.grid().display_offset() != 0
            || !state.term.mode().contains(TermMode::SHOW_CURSOR)
        {
            return None;
        }

        let shape = match state.term.cursor_style().shape {
            CursorShape::Block => RenderSurfaceCursorShape::Block,
            CursorShape::Underline => RenderSurfaceCursorShape::Underline,
            CursorShape::Beam => RenderSurfaceCursorShape::Beam,
            CursorShape::HollowBlock => RenderSurfaceCursorShape::HollowBlock,
            CursorShape::Hidden => RenderSurfaceCursorShape::Hidden,
        };
        let point = state.term.grid().cursor.point;
        Some(RenderSurfaceCursorSnapshot {
            x: u32::try_from(point.column.0).ok()?,
            y: u32::try_from(point.line.0).ok()?,
            focused: true,
            shape,
        })
    }
}

impl Default for AlacrittyTerminalStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalSnapshotProvider for AlacrittyTerminalStore {
    fn snapshot_of(&self, render_target_id: RenderTargetId) -> Option<TerminalSnapshot> {
        let mut inner = self.inner.borrow_mut();

        let state = inner.get_mut(&render_target_id)?;

        Some(Self::snapshot_from_state(render_target_id, state))
    }

    fn render_surface_snapshot_of(
        &self,
        render_target_id: RenderTargetId,
    ) -> Option<RenderSurfaceSnapshot> {
        let mut inner = self.inner.borrow_mut();

        let state = inner.get_mut(&render_target_id)?;

        Some(Self::render_surface_snapshot_from_state(
            render_target_id,
            state,
        ))
    }

    fn snapshot_for_build(
        &self,
        render_target_id: RenderTargetId,
        build_seq: Seq,
    ) -> Option<TerminalSnapshot> {
        let _ = build_seq;

        let mut inner = self.inner.borrow_mut();

        let state = inner.get_mut(&render_target_id)?;

        Some(Self::snapshot_from_state(render_target_id, state))
    }

    fn clear_damage_up_to(&self, render_target_id: RenderTargetId, presented_seq: Seq) {
        let mut inner = self.inner.borrow_mut();

        let Some(state) = inner.get_mut(&render_target_id) else {
            return;
        };

        if state.latest_seq <= presented_seq {
            state.term.reset_damage();
        }
    }
}

pub struct AlacrittyTermState {
    term: Term<PtyWriteEventListener>,
    pending_write_tx: Sender<Vec<u8>>,
    pending_write_rx: Receiver<Vec<u8>>,
    processor: Processor<StdSyncHandler>,
    graphics_decoder: KittyGraphicsStreamDecoder,
    graphics: KittyGraphicsState,
    size: AlacrittyTermSize,
    latest_seq: Seq,
    total_bytes: u64,
    chunk_count: u64,
}

impl AlacrittyTermState {
    fn new(size: AlacrittyTermSize, scrollback_history: usize) -> Self {
        let (pending_write_tx, pending_write_rx) = mpsc::channel();
        let event_listener = PtyWriteEventListener::new(pending_write_tx.clone());
        let config = Config {
            scrolling_history: scrollback_history,
            ..Config::default()
        };
        let mut term = Term::new(config, &size, event_listener.clone());

        term.reset_damage();

        Self {
            term,
            pending_write_tx,
            pending_write_rx,
            processor: Processor::<StdSyncHandler>::new(),
            graphics_decoder: KittyGraphicsStreamDecoder::default(),
            graphics: KittyGraphicsState::default(),
            size,
            latest_seq: Seq::ZERO,
            total_bytes: 0,
            chunk_count: 0,
        }
    }

    fn resize(&mut self, size: AlacrittyTermSize) {
        if self.size == size {
            return;
        }

        self.size = size;
        self.term.resize(self.size);
    }

    fn take_pending_writes(&mut self) -> Vec<Vec<u8>> {
        let mut writes = Vec::new();

        while let Ok(bytes) = self.pending_write_rx.try_recv() {
            writes.push(bytes);
        }

        writes
    }

    fn synchronized_update_pending(&self) -> bool {
        self.processor.sync_timeout().sync_timeout().is_some()
    }

    fn synchronized_update_deadline(&self) -> Option<Instant> {
        self.processor.sync_timeout().sync_timeout()
    }

    fn finish_expired_synchronized_update(&mut self, now: Instant) -> bool {
        let Some(deadline) = self.synchronized_update_deadline() else {
            return false;
        };
        if now < deadline {
            return false;
        }

        self.processor.stop_sync(&mut self.term);
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlacrittyTermApplyStats {
    pub latest_seq: Seq,
    pub total_bytes: u64,
    pub chunk_count: u64,
}

#[derive(Debug, Clone)]
struct StyledCell {
    col: u32,
    c: char,
    style: TextStyleDto,
}

fn visible_lines_and_runs(
    term: &Term<PtyWriteEventListener>,
) -> (Vec<TerminalLineSnapshot>, Vec<TerminalTextRunSnapshot>) {
    let renderable = term.renderable_content();
    let display_offset = renderable.display_offset;
    let mut cells_by_row: BTreeMap<u32, Vec<StyledCell>> = BTreeMap::new();

    for indexed in renderable.display_iter {
        let Some(point) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let cell = indexed.cell;
        let row = point.line as u32;
        let col = point.column.0 as u32;

        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.c == KITTY_IMAGE_PLACEHOLDER {
            continue;
        }

        cells_by_row.entry(row).or_default().push(StyledCell {
            col,
            c: cell.c,
            style: style_of_cell(cell.fg, cell.bg, cell.flags, renderable.colors),
        });
    }

    let mut lines = Vec::new();
    let mut text_runs = Vec::new();

    for (row, mut cells) in cells_by_row {
        cells.sort_by_key(|cell| cell.col);

        let line_text = line_text_from_cells(&cells);

        if !line_text.is_empty() {
            lines.push(TerminalLineSnapshot {
                row,
                text: line_text,
            });
        }

        text_runs.extend(styled_runs_from_cells(row, &cells));
    }

    (lines, text_runs)
}

fn visible_surface_rows(term: &Term<PtyWriteEventListener>) -> Vec<RenderSurfaceRowSnapshot> {
    let renderable = term.renderable_content();
    let display_offset = renderable.display_offset;
    let mut rows = Vec::new();
    let mut current_row = None::<u32>;
    let mut current_runs = Vec::new();
    let mut current_x = 0_u32;
    let mut current_next_x = 0_u32;
    let mut current_text = String::new();
    let mut current_style = None::<TextStyleDto>;

    for indexed in renderable.display_iter {
        let Some(point) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let cell = indexed.cell;
        let row = point.line as u32;
        let col = point.column.0 as u32;

        if current_row != Some(row) {
            if let Some(style) = current_style.take() {
                push_surface_run_if_not_blank(&mut current_runs, current_x, &current_text, style);
                current_text.clear();
            }

            if let Some(previous_row) = current_row.replace(row) {
                if !current_runs.is_empty() {
                    rows.push(RenderSurfaceRowSnapshot {
                        y: previous_row,
                        runs: std::mem::take(&mut current_runs),
                    });
                }
            }
        }

        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        if cell.c == KITTY_IMAGE_PLACEHOLDER {
            continue;
        }

        let style = style_of_cell(cell.fg, cell.bg, cell.flags, renderable.colors);
        if cell.c == ' ' && !style_has_visible_content(style) {
            continue;
        }

        let cell_width = terminal_char_cell_width(cell.c).max(1);
        let is_contiguous = current_style.is_some() && col == current_next_x;

        match current_style {
            None => {
                current_x = col;
                current_next_x = col + cell_width;
                current_text.push(cell.c);
                current_style = Some(style);
            }
            Some(existing_style) if existing_style == style && is_contiguous => {
                current_text.push(cell.c);
                current_next_x = col + cell_width;
            }
            Some(existing_style) => {
                push_surface_run_if_not_blank(
                    &mut current_runs,
                    current_x,
                    &current_text,
                    existing_style,
                );

                current_x = col;
                current_next_x = col + cell_width;
                current_text.clear();
                current_text.push(cell.c);
                current_style = Some(style);
            }
        }
    }

    if let Some(style) = current_style.take() {
        push_surface_run_if_not_blank(&mut current_runs, current_x, &current_text, style);
    }

    if let Some(row) = current_row {
        if !current_runs.is_empty() {
            rows.push(RenderSurfaceRowSnapshot {
                y: row,
                runs: current_runs,
            });
        }
    }

    rows
}

fn kitty_placeholder_cells(term: &Term<PtyWriteEventListener>) -> Vec<KittyPlaceholderCell> {
    let renderable = term.renderable_content();
    let display_offset = renderable.display_offset;

    renderable
        .display_iter
        .filter_map(|indexed| {
            let point = point_to_viewport(display_offset, indexed.point)?;
            let cell = indexed.cell;
            if cell.c != KITTY_IMAGE_PLACEHOLDER {
                return None;
            }
            let Color::Spec(rgb) = cell.fg else {
                return None;
            };
            Some(KittyPlaceholderCell {
                image_id: u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b),
                x_cell: u32::try_from(point.column.0).ok()?,
                y_cell: u32::try_from(point.line).ok()?,
            })
        })
        .collect()
}

fn line_text_from_cells(cells: &[StyledCell]) -> String {
    let mut text = String::new();
    let mut next_col = 0_u32;

    for cell in cells {
        while next_col < cell.col {
            text.push(' ');
            next_col += 1;
        }

        text.push(cell.c);
        next_col = cell.col + terminal_char_cell_width(cell.c).max(1);
    }

    text.trim_end().to_string()
}

fn styled_runs_from_cells(row: u32, cells: &[StyledCell]) -> Vec<TerminalTextRunSnapshot> {
    let mut runs = Vec::new();

    let mut current_x = 0_u32;
    let mut current_next_x = 0_u32;
    let mut current_text = String::new();
    let mut current_style = None::<TextStyleDto>;

    for cell in cells {
        let style = cell.style;

        if cell.c == ' ' && !style_has_visible_content(style) {
            continue;
        }

        let cell_width = terminal_char_cell_width(cell.c).max(1);
        let is_contiguous = current_style.is_some() && cell.col == current_next_x;

        match current_style {
            None => {
                current_x = cell.col;
                current_next_x = cell.col + cell_width;
                current_text.push(cell.c);
                current_style = Some(style);
            }
            Some(existing_style) if existing_style == style && is_contiguous => {
                current_text.push(cell.c);
                current_next_x = cell.col + cell_width;
            }
            Some(existing_style) => {
                push_run_if_not_blank(&mut runs, current_x, row, &current_text, existing_style);

                current_x = cell.col;
                current_next_x = cell.col + cell_width;
                current_text.clear();
                current_text.push(cell.c);
                current_style = Some(style);
            }
        }
    }

    if let Some(style) = current_style {
        push_run_if_not_blank(&mut runs, current_x, row, &current_text, style);
    }

    runs
}

fn push_run_if_not_blank(
    runs: &mut Vec<TerminalTextRunSnapshot>,
    x: u32,
    y: u32,
    text: &str,
    style: TextStyleDto,
) {
    if text.is_empty() {
        return;
    }

    if text.trim().is_empty() && !style_has_visible_content(style) {
        return;
    }

    runs.push(TerminalTextRunSnapshot {
        x,
        y,
        text: text.to_string(),
        style,
    });
}

fn push_surface_run_if_not_blank(
    runs: &mut Vec<RenderSurfaceRunSnapshot>,
    x: u32,
    text: &str,
    style: TextStyleDto,
) {
    if text.is_empty() {
        return;
    }

    if text.trim().is_empty() && !style_has_visible_content(style) {
        return;
    }

    runs.push(RenderSurfaceRunSnapshot {
        x,
        text: text.to_string(),
        style,
    });
}

fn style_has_visible_content(style: TextStyleDto) -> bool {
    style.background.is_some() || style.underline || style.bold || style.italic
}

fn dominant_background_of(rows: &[RenderSurfaceRowSnapshot]) -> Option<RgbColorDto> {
    let mut weights = Vec::<(RgbColorDto, u64)>::new();
    for run in rows.iter().flat_map(|row| &row.runs) {
        let Some(background) = run.style.background else {
            continue;
        };
        let cell_width = run
            .text
            .chars()
            .map(terminal_char_cell_width)
            .map(u64::from)
            .sum::<u64>();
        if let Some((_, weight)) = weights.iter_mut().find(|(color, _)| *color == background) {
            *weight += cell_width;
        } else {
            weights.push((background, cell_width));
        }
    }
    weights
        .into_iter()
        .max_by_key(|(_, weight)| *weight)
        .map(|(color, _)| color)
}

fn style_of_cell(fg: Color, bg: Color, flags: Flags, colors: &Colors) -> TextStyleDto {
    let mut foreground = color_to_rgb(fg, colors);
    let mut background = color_to_rgb(bg, colors);

    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut foreground, &mut background);
    }

    TextStyleDto {
        foreground,
        background,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.contains(Flags::UNDERLINE),
    }
}

fn color_to_rgb(color: Color, colors: &Colors) -> Option<RgbColorDto> {
    match color {
        Color::Spec(rgb) => Some(rgb_to_dto(rgb)),
        Color::Named(named) => named_color_to_rgb(named, colors),
        Color::Indexed(index) => indexed_color_to_rgb(index, colors),
    }
}

fn named_color_to_rgb(color: NamedColor, colors: &Colors) -> Option<RgbColorDto> {
    if let Some(rgb) = colors[color] {
        return Some(rgb_to_dto(rgb));
    }

    match color {
        NamedColor::Black => Some(RgbColorDto::new(0, 0, 0)),
        NamedColor::Red => Some(RgbColorDto::new(205, 49, 49)),
        NamedColor::Green => Some(RgbColorDto::new(13, 188, 121)),
        NamedColor::Yellow => Some(RgbColorDto::new(229, 229, 16)),
        NamedColor::Blue => Some(RgbColorDto::new(36, 114, 200)),
        NamedColor::Magenta => Some(RgbColorDto::new(188, 63, 188)),
        NamedColor::Cyan => Some(RgbColorDto::new(17, 168, 205)),
        NamedColor::White => Some(RgbColorDto::new(229, 229, 229)),
        NamedColor::BrightBlack => Some(RgbColorDto::new(102, 102, 102)),
        NamedColor::BrightRed => Some(RgbColorDto::new(241, 76, 76)),
        NamedColor::BrightGreen => Some(RgbColorDto::new(35, 209, 139)),
        NamedColor::BrightYellow => Some(RgbColorDto::new(245, 245, 67)),
        NamedColor::BrightBlue => Some(RgbColorDto::new(59, 142, 234)),
        NamedColor::BrightMagenta => Some(RgbColorDto::new(214, 112, 214)),
        NamedColor::BrightCyan => Some(RgbColorDto::new(41, 184, 219)),
        NamedColor::BrightWhite => Some(RgbColorDto::new(255, 255, 255)),
        NamedColor::Foreground => Some(RgbColorDto::new(229, 229, 229)),
        NamedColor::Background => Some(RgbColorDto::new(0, 0, 0)),
        _ => None,
    }
}

fn indexed_color_to_rgb(index: u8, colors: &Colors) -> Option<RgbColorDto> {
    if let Some(rgb) = colors[index as usize] {
        return Some(rgb_to_dto(rgb));
    }

    match index {
        0 => Some(RgbColorDto::new(0, 0, 0)),
        1 => Some(RgbColorDto::new(205, 49, 49)),
        2 => Some(RgbColorDto::new(13, 188, 121)),
        3 => Some(RgbColorDto::new(229, 229, 16)),
        4 => Some(RgbColorDto::new(36, 114, 200)),
        5 => Some(RgbColorDto::new(188, 63, 188)),
        6 => Some(RgbColorDto::new(17, 168, 205)),
        7 => Some(RgbColorDto::new(229, 229, 229)),
        8 => Some(RgbColorDto::new(102, 102, 102)),
        9 => Some(RgbColorDto::new(241, 76, 76)),
        10 => Some(RgbColorDto::new(35, 209, 139)),
        11 => Some(RgbColorDto::new(245, 245, 67)),
        12 => Some(RgbColorDto::new(59, 142, 234)),
        13 => Some(RgbColorDto::new(214, 112, 214)),
        14 => Some(RgbColorDto::new(41, 184, 219)),
        15 => Some(RgbColorDto::new(255, 255, 255)),
        16..=231 => {
            let cube_index = index - 16;

            let red_level = cube_index / 36;
            let green_level = (cube_index % 36) / 6;
            let blue_level = cube_index % 6;

            Some(RgbColorDto::new(
                ansi_256_cube_component(red_level),
                ansi_256_cube_component(green_level),
                ansi_256_cube_component(blue_level),
            ))
        }
        232..=255 => {
            let level = 8 + 10 * (index - 232);

            Some(RgbColorDto::new(level, level, level))
        }
    }
}

fn rgb_to_dto(rgb: Rgb) -> RgbColorDto {
    RgbColorDto::new(rgb.r, rgb.g, rgb.b)
}

fn ansi_256_cube_component(level: u8) -> u8 {
    match level {
        0 => 0,
        1..=5 => 55 + 40 * level,
        _ => unreachable!("ANSI 256-color cube level must be 0..=5"),
    }
}

fn dirty_rows_of(damage: TermDamage<'_>, screen_lines: usize) -> Vec<u32> {
    match damage {
        TermDamage::Full => (0..screen_lines as u32).collect(),
        TermDamage::Partial(lines) => {
            let mut rows = BTreeSet::new();

            for line in lines {
                let row = line.line as u32;
                rows.insert(row);

                if row > 0 {
                    rows.insert(row - 1);
                }

                if row + 1 < screen_lines as u32 {
                    rows.insert(row + 1);
                }
            }

            rows.into_iter().collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    use super::*;

    #[test]
    fn extracts_kitty_rgba_image_without_leaking_apc_into_text() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);
        let payload = STANDARD.encode([255, 0, 0, 255]);
        let bytes =
            format!("before\x1b_Ga=T,f=32,s=1,v=1,i=7,p=2,c=2,r=1,C=1;{payload}\x1b\\after");

        store.apply_bytes(target_id, Seq::new(1), bytes.as_bytes());

        let snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        let text: String = snapshot
            .rows
            .iter()
            .flat_map(|row| row.runs.iter())
            .map(|run| run.text.as_str())
            .collect();
        assert!(text.contains("beforeafter"));
        assert_eq!(snapshot.image_surfaces.len(), 1);
        assert_eq!(&*snapshot.image_surfaces[0].rgba, &[255, 0, 0, 255]);
        assert_eq!(snapshot.image_surfaces[0].columns, 2);
        assert_eq!(
            store.take_pending_pty_writes(target_id),
            vec![b"\x1b_Gi=7,p=2;OK\x1b\\".to_vec()]
        );
    }

    #[test]
    fn kitty_query_responds_without_adding_an_image() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);
        let payload = STANDARD.encode([0, 0, 0]);
        let bytes = format!("\x1b_Ga=q,f=24,s=1,v=1,i=31;{payload}\x1b\\");

        store.apply_bytes(target_id, Seq::new(1), bytes.as_bytes());

        assert!(
            store
                .render_surface_snapshot_of(target_id)
                .unwrap()
                .image_surfaces
                .is_empty()
        );
        assert_eq!(
            store.take_pending_pty_writes(target_id),
            vec![b"\x1b_Gi=31;OK\x1b\\".to_vec()]
        );
    }

    #[test]
    fn exports_terminal_input_modes_from_private_mode_sequences() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 10));
        let target_id = RenderTargetId::new(44);
        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"\x1b[?1h\x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1006h",
        );

        let modes = store.input_modes(target_id);
        assert!(modes.app_cursor());
        assert!(modes.bracketed_paste());
        assert!(modes.focus_in_out());
        assert!(modes.sgr_mouse());
        assert!(modes.mouse_drag());
        assert!(modes.mouse_tracking());

        store.apply_bytes(
            target_id,
            Seq::new(2),
            b"\x1b[?1l\x1b[?2004l\x1b[?1004l\x1b[?1002l\x1b[?1006l",
        );
        assert_eq!(store.input_modes(target_id), TerminalInputModes::default());
    }

    #[test]
    fn exports_decscusr_cursor_shape() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(46);
        store.apply_bytes(target_id, Seq::new(1), b"\x1b[6 q");

        assert_eq!(
            store.cursor_snapshot(target_id).unwrap().shape,
            RenderSurfaceCursorShape::Beam,
        );

        store.apply_bytes(target_id, Seq::new(2), b"\x1b[4 q");
        assert_eq!(
            store.cursor_snapshot(target_id).unwrap().shape,
            RenderSurfaceCursorShape::Underline,
        );
    }

    #[test]
    fn scroll_display_reveals_history_and_hides_the_live_cursor() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 2));
        let target_id = RenderTargetId::new(47);
        store.apply_bytes(target_id, Seq::new(1), b"one\r\ntwo\r\nthree");

        let live_text: String = store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .rows
            .iter()
            .flat_map(|row| &row.runs)
            .map(|run| run.text.as_str())
            .collect();
        assert!(!live_text.contains("one"));
        assert!(live_text.contains("three"));

        assert!(store.scroll_display(target_id, Seq::new(2), Scroll::Delta(1)));
        let history_snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        let history_text: String = history_snapshot
            .rows
            .iter()
            .flat_map(|row| &row.runs)
            .map(|run| run.text.as_str())
            .collect();
        assert!(history_text.contains("one"));
        assert!(!history_text.contains("three"));
        assert_eq!(history_snapshot.latest_seq, Seq::new(2));
        assert!(store.cursor_snapshot(target_id).is_none());

        assert!(store.scroll_display(target_id, Seq::new(3), Scroll::Bottom));
        assert!(store.cursor_snapshot(target_id).is_some());
        assert!(!store.scroll_display(target_id, Seq::new(4), Scroll::Bottom));
    }

    #[test]
    fn limits_scrollback_to_configured_history() {
        let store = AlacrittyTerminalStore::with_size_and_scrollback_history(
            AlacrittyTermSize::new(8, 2),
            1,
        );
        let target_id = RenderTargetId::new(49);
        store.apply_bytes(target_id, Seq::new(1), b"one\r\ntwo\r\nthree\r\nfour");

        assert!(store.scroll_display(target_id, Seq::new(2), Scroll::Top));
        let history_text: String = store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .rows
            .iter()
            .flat_map(|row| &row.runs)
            .map(|run| run.text.as_str())
            .collect();

        assert!(!history_text.contains("one"));
        assert!(history_text.contains("two"));
        assert!(history_text.contains("three"));
        assert!(!history_text.contains("four"));
    }

    #[test]
    fn alternate_screen_does_not_scroll_into_primary_history() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 2));
        let target_id = RenderTargetId::new(48);
        store.apply_bytes(target_id, Seq::new(1), b"one\r\ntwo\r\nthree");
        store.apply_bytes(target_id, Seq::new(2), b"\x1b[?1049halt");

        assert!(!store.scroll_display(target_id, Seq::new(3), Scroll::Delta(1)));
        let alternate_text: String = store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .rows
            .iter()
            .flat_map(|row| &row.runs)
            .map(|run| run.text.as_str())
            .collect();
        assert!(alternate_text.contains("alt"));
        assert!(!alternate_text.contains("one"));

        store.apply_bytes(target_id, Seq::new(4), b"\x1b[?1049l");
        assert!(store.scroll_display(target_id, Seq::new(5), Scroll::Delta(1)));
    }

    #[test]
    fn buffers_synchronized_update_until_end_or_timeout() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 4));
        let target_id = RenderTargetId::new(45);
        store.apply_bytes(target_id, Seq::new(1), b"old");
        store.apply_bytes(
            target_id,
            Seq::new(2),
            b"\x1b[?2026h\x1b[2J\x1b[Hreplacement",
        );

        assert!(store.synchronized_update_pending(target_id));
        let pending_text: String = store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .rows
            .iter()
            .flat_map(|row| &row.runs)
            .map(|run| run.text.as_str())
            .collect();
        assert!(pending_text.contains("old"));
        assert!(!pending_text.contains("replacement"));

        let deadline = store.synchronized_update_deadline(target_id).unwrap();
        assert!(!store.finish_expired_synchronized_update(
            target_id,
            deadline - std::time::Duration::from_nanos(1),
        ));
        assert!(store.finish_expired_synchronized_update(target_id, deadline));
        assert!(!store.synchronized_update_pending(target_id));

        let completed_text: String = store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .rows
            .iter()
            .flat_map(|row| &row.runs)
            .map(|run| run.text.as_str())
            .collect();
        assert!(completed_text.contains("replacement"));
    }

    #[test]
    fn resolves_yazi_style_unicode_placeholders_into_an_image_surface() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);
        let payload = STANDARD.encode([255, 0, 0, 255]);
        let transfer = format!("\x1b_Gq=2,a=T,C=1,U=1,f=32,s=1,v=1,i=7;{payload}\x1b\\");
        let placeholders = concat!(
            "\x1b[38;2;0;0;7m",
            "\x1b[2;3H\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{0305}\u{030D}",
            "\x1b[3;3H\u{10EEEE}\u{030D}\u{0305}\u{10EEEE}\u{030D}\u{030D}",
            "\x1b[0m",
        );

        store.apply_bytes(target_id, Seq::new(1), transfer.as_bytes());
        store.apply_bytes(target_id, Seq::new(2), placeholders.as_bytes());

        let snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        assert_eq!(snapshot.image_surfaces.len(), 1);
        assert_eq!(
            (
                snapshot.image_surfaces[0].x_cell,
                snapshot.image_surfaces[0].y_cell
            ),
            (2, 1)
        );
        assert_eq!(
            (
                snapshot.image_surfaces[0].columns,
                snapshot.image_surfaces[0].rows
            ),
            (2, 2)
        );
        assert!(
            snapshot
                .rows
                .iter()
                .flat_map(|row| &row.runs)
                .all(|run| !run.text.contains(KITTY_IMAGE_PLACEHOLDER))
        );
    }

    #[test]
    fn applies_bytes_and_exports_snapshot() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"\x1b[31mred\x1b[0m\r\nhello\r\nworld",
        );

        let snapshot = store.snapshot_of(target_id).unwrap();

        let texts: Vec<String> = snapshot
            .lines
            .iter()
            .map(|line| line.text.clone())
            .collect();

        assert_eq!(snapshot.latest_seq, Seq::new(1));
        assert!(texts.iter().any(|line| line == "red"));
        assert!(texts.iter().any(|line| line == "hello"));
        assert!(texts.iter().any(|line| line == "world"));
        assert!(!snapshot.dirty_rows.is_empty());
        assert!(!snapshot.text_runs.is_empty());
    }

    #[test]
    fn exports_red_bold_text_run() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b[31;1mred\x1b[0m");

        let snapshot = store.snapshot_of(target_id).unwrap();

        let red_run = snapshot
            .text_runs
            .iter()
            .find(|run| run.text == "red")
            .expect("red run should exist");

        assert_eq!(red_run.x, 0);
        assert_eq!(red_run.y, 0);
        assert!(red_run.style.foreground.is_some());
        assert!(red_run.style.bold);
    }

    #[test]
    fn exports_eza_bold_blue_as_blue() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b[1;34mAndroid\x1b[0m");

        let snapshot = store.snapshot_of(target_id).unwrap();
        let run = snapshot
            .text_runs
            .iter()
            .find(|run| run.text == "Android")
            .expect("eza-style directory run should exist");

        assert_eq!(run.style.foreground, Some(RgbColorDto::new(36, 114, 200)));
        assert!(run.style.bold);
    }

    #[test]
    fn clear_damage_up_to_resets_full_damage() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(target_id, Seq::new(1), b"hello");

        let before = store.snapshot_of(target_id).unwrap();
        assert!(!before.dirty_rows.is_empty());

        store.clear_damage_up_to(target_id, Seq::new(1));

        let after = store.snapshot_of(target_id).unwrap();

        assert!(
            after
                .dirty_rows
                .iter()
                .all(|row| *row < store.size().screen_lines() as u32),
            "dirty rows should stay inside screen bounds: {:?}",
            after.dirty_rows,
        );
    }

    #[test]
    fn clear_damage_up_to_does_not_clear_newer_seq() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(target_id, Seq::new(2), b"hello");
        store.clear_damage_up_to(target_id, Seq::new(1));

        let snapshot = store.snapshot_of(target_id).unwrap();

        assert_eq!(snapshot.latest_seq, Seq::new(2));
        assert!(!snapshot.dirty_rows.is_empty());
    }

    #[test]
    fn resize_updates_snapshot_bounds() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(target_id, Seq::new(1), b"hello");
        store.resize(target_id, Seq::new(2), AlacrittyTermSize::new(100, 40));

        let snapshot = store.snapshot_of(target_id).unwrap();

        assert_eq!(snapshot.latest_seq, Seq::new(2));
        assert!(
            snapshot.dirty_rows.iter().all(|row| *row < 40),
            "dirty rows should stay inside resized screen bounds: {:?}",
            snapshot.dirty_rows,
        );
    }

    #[test]
    fn resolves_default_background_to_black_run() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b[40m \x1b[0m");

        let snapshot = store.snapshot_of(target_id).expect("snapshot should exist");
        let run = snapshot
            .text_runs
            .iter()
            .find(|run| run.y == 0 && run.x == 0)
            .expect("background run should exist");

        assert!(!run.text.is_empty());
        assert!(run.text.chars().all(|c| c == ' '));
        assert_eq!(run.style.background, Some(RgbColorDto::new(0, 0, 0)));
    }

    #[test]
    fn uses_dominant_application_background_for_surface_padding() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(4, 2));
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b[48;2;30;30;47m\x1b[2J");
        let snapshot = store.render_surface_snapshot_of(target_id).unwrap();

        assert_eq!(snapshot.default_background, RgbColorDto::new(30, 30, 47));
    }

    #[test]
    fn expands_partial_damage_to_neighbor_rows() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(4, 4));
        let target_id = RenderTargetId::new(1);

        let _ = store.snapshot_of(target_id);

        store.apply_bytes(target_id, Seq::new(1), b"a");

        let snapshot = store.snapshot_of(target_id).expect("snapshot should exist");

        assert!(snapshot.dirty_rows.contains(&0));
        assert!(snapshot.dirty_rows.contains(&1));
    }

    #[test]
    fn inverse_text_swaps_default_foreground_and_background() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b[7mfoo\x1b[0m");

        let snapshot = store.snapshot_of(target_id).expect("snapshot should exist");
        let run = snapshot
            .text_runs
            .iter()
            .find(|run| run.text == "foo")
            .expect("inverse run should exist");

        assert_eq!(run.style.foreground, Some(RgbColorDto::new(0, 0, 0)));
        assert_eq!(run.style.background, Some(RgbColorDto::new(229, 229, 229)));
    }
}
