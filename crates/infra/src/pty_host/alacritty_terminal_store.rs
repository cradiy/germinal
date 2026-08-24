use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
    rc::Rc,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    time::Instant,
};

use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Boundary, Column, Direction, Point, Side},
    selection::{Selection, SelectionType},
    term::{
        ClipboardType, Config, Osc52, Term, TermDamage, TermMode,
        cell::{Cell, Flags},
        color::Colors,
        point_to_viewport,
        search::RegexSearch,
        viewport_to_point,
    },
    vte::ansi::{
        ClearMode, Color, CursorShape, CursorStyle, NamedColor, Processor, Rgb, StdSyncHandler,
    },
};
use germinal_ports::{
    pty_host::{
        color_theme::TerminalColorTheme,
        cursor_style::{TerminalCursorShape, TerminalCursorStyle},
        hyperlink::TerminalHyperlink,
        snapshot::{
            TerminalLineSnapshot, TerminalSnapshot, TerminalSnapshotProvider,
            TerminalTextRunSnapshot,
        },
        terminal_clipboard::{TerminalClipboard, TerminalOsc52Mode},
        terminal_input_mode::TerminalInputModes,
        width::terminal_char_cell_width,
        worker_input::{
            TerminalSelectionKind, TerminalSelectionPoint, TerminalSelectionSide, TerminalViMotion,
            TerminalViSearchDirection, TerminalViSearchPrompt, TerminalViSelectionKind,
            TerminalViTextObject,
        },
    },
    rendering::{
        frame_plan_builder::{RgbColorDto, TextStyleDto},
        render_target_id::RenderTargetId,
        surface_snapshot::{
            RenderSurfaceCursorShape, RenderSurfaceCursorSnapshot, RenderSurfaceRowSnapshot,
            RenderSurfaceRunSnapshot, RenderSurfaceSnapshot, RenderSurfaceTextDecoration,
            RenderSurfaceUnderlineStyle,
        },
    },
    seq::Seq,
};

use super::kitty_graphics::{
    KittyGraphicsState, KittyGraphicsStreamDecoder, KittyPlaceholderCell, KittyStreamEvent,
};
use super::kitty_terminal_lifecycle::{
    KittyTerminalLifecycleEvent, KittyTerminalLifecycleObserver,
};

const KITTY_IMAGE_PLACEHOLDER: char = '\u{10EEEE}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlacrittyTermSize {
    columns: usize,
    screen_lines: usize,
    pixel_width: u32,
    pixel_height: u32,
}

impl AlacrittyTermSize {
    pub const fn new(columns: usize, screen_lines: usize) -> Self {
        Self {
            columns,
            screen_lines,
            pixel_width: columns as u32,
            pixel_height: screen_lines as u32,
        }
    }

    pub const fn with_pixels(
        columns: usize,
        screen_lines: usize,
        pixel_width: u32,
        pixel_height: u32,
    ) -> Self {
        Self {
            columns,
            screen_lines,
            pixel_width,
            pixel_height,
        }
    }

    pub const fn columns(self) -> usize {
        self.columns
    }

    pub const fn screen_lines(self) -> usize {
        self.screen_lines
    }

    fn cell_size_px(self) -> (u32, u32) {
        (
            self.pixel_width
                .saturating_div(self.columns.max(1) as u32)
                .max(1),
            self.pixel_height
                .saturating_div(self.screen_lines.max(1) as u32)
                .max(1),
        )
    }
}

impl Default for AlacrittyTermSize {
    fn default() -> Self {
        Self {
            columns: 80,
            screen_lines: 24,
            pixel_width: 80,
            pixel_height: 24,
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
    pending_writes: Sender<PendingPtyWrite>,
    pending_titles: Sender<Option<String>>,
    pending_bells: Sender<()>,
    pending_clipboard_stores: Sender<(TerminalClipboard, String)>,
    pending_clipboard_loads: Sender<TerminalClipboardLoad>,
}

pub(crate) type TerminalClipboardFormatter = Arc<dyn Fn(&str) -> String + Sync + Send + 'static>;
type TerminalColorFormatter = Arc<dyn Fn(Rgb) -> String + Sync + Send + 'static>;

enum PendingPtyWrite {
    Bytes(Vec<u8>),
    ColorRequest(usize, TerminalColorFormatter),
}

pub(crate) struct TerminalClipboardLoad {
    pub clipboard: TerminalClipboard,
    pub formatter: TerminalClipboardFormatter,
}

impl PtyWriteEventListener {
    fn new(
        pending_writes: Sender<PendingPtyWrite>,
        pending_titles: Sender<Option<String>>,
        pending_bells: Sender<()>,
        pending_clipboard_stores: Sender<(TerminalClipboard, String)>,
        pending_clipboard_loads: Sender<TerminalClipboardLoad>,
    ) -> Self {
        Self {
            pending_writes,
            pending_titles,
            pending_bells,
            pending_clipboard_stores,
            pending_clipboard_loads,
        }
    }
}

impl EventListener for PtyWriteEventListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                let _ = self
                    .pending_writes
                    .send(PendingPtyWrite::Bytes(text.into_bytes()));
            }
            Event::ColorRequest(index, formatter) => {
                let _ = self
                    .pending_writes
                    .send(PendingPtyWrite::ColorRequest(index, formatter));
            }
            Event::Title(title) => {
                let _ = self.pending_titles.send(Some(title));
            }
            Event::ResetTitle => {
                let _ = self.pending_titles.send(None);
            }
            Event::Bell => {
                let _ = self.pending_bells.send(());
            }
            Event::ClipboardStore(clipboard, text) => {
                let _ = self
                    .pending_clipboard_stores
                    .send((terminal_clipboard(clipboard), text));
            }
            Event::ClipboardLoad(clipboard, formatter) => {
                let _ = self.pending_clipboard_loads.send(TerminalClipboardLoad {
                    clipboard: terminal_clipboard(clipboard),
                    formatter,
                });
            }
            _ => {}
        }
    }
}

#[derive(Clone)]
pub struct AlacrittyTerminalStore {
    inner: Rc<RefCell<HashMap<RenderTargetId, AlacrittyTermState>>>,
    size: AlacrittyTermSize,
    scrollback_history: usize,
    cursor_style: TerminalCursorStyle,
    osc52_mode: TerminalOsc52Mode,
    color_theme: TerminalColorTheme,
}

impl AlacrittyTerminalStore {
    pub fn new() -> Self {
        Self::with_size(AlacrittyTermSize::default())
    }

    pub fn with_size(size: AlacrittyTermSize) -> Self {
        Self::with_size_scrollback_and_cursor_style(
            size,
            Config::default().scrolling_history,
            TerminalCursorStyle::default(),
        )
    }

    pub fn with_size_and_scrollback_history(
        size: AlacrittyTermSize,
        scrollback_history: usize,
    ) -> Self {
        Self::with_size_scrollback_and_cursor_style(
            size,
            scrollback_history,
            TerminalCursorStyle::default(),
        )
    }

    pub fn with_size_scrollback_and_cursor_style(
        size: AlacrittyTermSize,
        scrollback_history: usize,
        cursor_style: TerminalCursorStyle,
    ) -> Self {
        Self::with_size_scrollback_cursor_style_and_osc52(
            size,
            scrollback_history,
            cursor_style,
            TerminalOsc52Mode::default(),
        )
    }

    pub fn with_size_scrollback_cursor_style_and_osc52(
        size: AlacrittyTermSize,
        scrollback_history: usize,
        cursor_style: TerminalCursorStyle,
        osc52_mode: TerminalOsc52Mode,
    ) -> Self {
        Self::with_size_scrollback_cursor_style_osc52_and_colors(
            size,
            scrollback_history,
            cursor_style,
            osc52_mode,
            TerminalColorTheme::default(),
        )
    }

    pub fn with_size_scrollback_cursor_style_osc52_and_colors(
        size: AlacrittyTermSize,
        scrollback_history: usize,
        cursor_style: TerminalCursorStyle,
        osc52_mode: TerminalOsc52Mode,
        color_theme: TerminalColorTheme,
    ) -> Self {
        Self {
            inner: Rc::new(RefCell::new(HashMap::new())),
            size,
            scrollback_history,
            cursor_style,
            osc52_mode,
            color_theme,
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

        let state = inner.entry(render_target_id).or_insert_with(|| {
            AlacrittyTermState::new(
                self.size,
                self.scrollback_history,
                self.cursor_style,
                self.osc52_mode,
            )
        });
        let previous_selection = state.term.selection.clone();

        for event in state.graphics_decoder.feed(bytes) {
            match event {
                KittyStreamEvent::Bytes(visible) => {
                    state.advance_terminal_bytes(&visible);
                }
                KittyStreamEvent::Command(command) => {
                    let point = state.term.grid().cursor.point;
                    let cursor = (
                        u32::try_from(point.column.0).unwrap_or(0),
                        u32::try_from(point.line.0).unwrap_or(0),
                    );
                    let result = state.graphics.handle_with_cell_size(
                        command,
                        cursor,
                        state.size.cell_size_px(),
                    );
                    if let Some(response) = result.response {
                        let _ = state
                            .pending_write_tx
                            .send(PendingPtyWrite::Bytes(response));
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
        state.apply_pending_title_changes();
        state.mark_selection_damage_if_changed(previous_selection);

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

        let state = inner.entry(render_target_id).or_insert_with(|| {
            AlacrittyTermState::new(
                size,
                self.scrollback_history,
                self.cursor_style,
                self.osc52_mode,
            )
        });

        let previous_selection = state.term.selection.clone();
        state.resize(size);
        state.mark_selection_damage_if_changed(previous_selection);
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

    pub fn start_selection(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        kind: TerminalSelectionKind,
        point: TerminalSelectionPoint,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };
        let point = selection_point(&state.term, point);
        let side = selection_side(point.1);
        let kind = match kind {
            TerminalSelectionKind::Character => SelectionType::Simple,
            TerminalSelectionKind::Word => SelectionType::Semantic,
            TerminalSelectionKind::Line => SelectionType::Lines,
        };

        state.term.selection = Some(Selection::new(kind, point.0, side));
        state.latest_seq = seq;
        state.selection_damage = true;
        true
    }

    pub fn update_selection(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        point: TerminalSelectionPoint,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };
        let point = selection_point(&state.term, point);
        let side = selection_side(point.1);
        let Some(selection) = state.term.selection.as_mut() else {
            return false;
        };
        let previous = selection.clone();
        selection.update(point.0, side);
        if *selection == previous {
            return false;
        }

        state.latest_seq = seq;
        state.selection_damage = true;
        true
    }

    pub fn selection_text(&self, render_target_id: RenderTargetId) -> Option<String> {
        let inner = self.inner.borrow();
        inner.get(&render_target_id)?.term.selection_to_string()
    }

    pub fn set_vi_mode(&self, render_target_id: RenderTargetId, seq: Seq, enabled: bool) -> bool {
        let mut inner = self.inner.borrow_mut();
        let state = inner.entry(render_target_id).or_insert_with(|| {
            AlacrittyTermState::new(
                self.size,
                self.scrollback_history,
                self.cursor_style,
                self.osc52_mode,
            )
        });
        if state.host_search_mode || state.term.mode().contains(TermMode::VI) == enabled {
            return false;
        }

        if !enabled {
            state.term.scroll_display(Scroll::Bottom);
            state.vi_search_prompt = None;
        }
        state.term.toggle_vi_mode();
        state.latest_seq = seq;
        state.selection_damage = true;
        true
    }

    pub fn set_search_mode(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        enabled: bool,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let state = inner.entry(render_target_id).or_insert_with(|| {
            AlacrittyTermState::new(
                self.size,
                self.scrollback_history,
                self.cursor_style,
                self.osc52_mode,
            )
        });
        if state.host_search_mode == enabled {
            return false;
        }
        if enabled && state.term.mode().contains(TermMode::VI) {
            return false;
        }

        if !enabled {
            state.term.scroll_display(Scroll::Bottom);
            state.vi_search_prompt = None;
        }
        if state.term.mode().contains(TermMode::VI) != enabled {
            state.term.toggle_vi_mode();
        }
        state.host_search_mode = enabled;
        state.latest_seq = seq;
        state.selection_damage = true;
        true
    }

    pub fn vi_motion(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        motion: TerminalViMotion,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };
        if !state.term.mode().contains(TermMode::VI) {
            return false;
        }

        let motion = match motion {
            TerminalViMotion::Up => alacritty_terminal::vi_mode::ViMotion::Up,
            TerminalViMotion::Down => alacritty_terminal::vi_mode::ViMotion::Down,
            TerminalViMotion::Left => alacritty_terminal::vi_mode::ViMotion::Left,
            TerminalViMotion::Right => alacritty_terminal::vi_mode::ViMotion::Right,
            TerminalViMotion::First => alacritty_terminal::vi_mode::ViMotion::First,
            TerminalViMotion::FirstOccupied => alacritty_terminal::vi_mode::ViMotion::FirstOccupied,
            TerminalViMotion::Last => alacritty_terminal::vi_mode::ViMotion::Last,
            TerminalViMotion::WordLeft => {
                vim_word_left(&mut state.term);
                state.latest_seq = seq;
                state.selection_damage = true;
                return true;
            }
            TerminalViMotion::WordRight => {
                vim_word_right(&mut state.term);
                state.latest_seq = seq;
                state.selection_damage = true;
                return true;
            }
            TerminalViMotion::WordRightEnd => {
                vim_word_right_end(&mut state.term);
                state.latest_seq = seq;
                state.selection_damage = true;
                return true;
            }
            TerminalViMotion::High => alacritty_terminal::vi_mode::ViMotion::High,
            TerminalViMotion::Middle => alacritty_terminal::vi_mode::ViMotion::Middle,
            TerminalViMotion::Low => alacritty_terminal::vi_mode::ViMotion::Low,
            TerminalViMotion::HalfPageUp => {
                let lines = i32::try_from(state.term.screen_lines() / 2)
                    .unwrap_or(i32::MAX)
                    .max(1);
                state.term.scroll_display(Scroll::Delta(lines));
                state.latest_seq = seq;
                state.selection_damage = true;
                return true;
            }
            TerminalViMotion::HalfPageDown => {
                let lines = i32::try_from(state.term.screen_lines() / 2)
                    .unwrap_or(i32::MAX)
                    .max(1);
                state.term.scroll_display(Scroll::Delta(-lines));
                state.latest_seq = seq;
                state.selection_damage = true;
                return true;
            }
            TerminalViMotion::PageUp => {
                state.term.scroll_display(Scroll::PageUp);
                state.latest_seq = seq;
                state.selection_damage = true;
                return true;
            }
            TerminalViMotion::PageDown => {
                state.term.scroll_display(Scroll::PageDown);
                state.latest_seq = seq;
                state.selection_damage = true;
                return true;
            }
            TerminalViMotion::Top => {
                state.term.scroll_display(Scroll::Top);
                alacritty_terminal::vi_mode::ViMotion::High
            }
            TerminalViMotion::Bottom => {
                state.term.scroll_display(Scroll::Bottom);
                alacritty_terminal::vi_mode::ViMotion::Low
            }
        };
        state.term.vi_motion(motion);
        state.latest_seq = seq;
        state.selection_damage = true;
        true
    }

    pub fn set_vi_selection(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        kind: Option<TerminalViSelectionKind>,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };
        if !state.term.mode().contains(TermMode::VI) {
            return false;
        }

        if let Some(kind) = kind {
            let selection_type = match kind {
                TerminalViSelectionKind::Character => SelectionType::Simple,
                TerminalViSelectionKind::Line => SelectionType::Lines,
            };
            let mut selection =
                Selection::new(selection_type, state.term.vi_mode_cursor.point, Side::Left);
            selection.include_all();
            state.term.selection = Some(selection);
        } else {
            state.term.selection = None;
        }

        state.latest_seq = seq;
        state.selection_damage = true;
        true
    }

    pub fn select_vi_text_object(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        text_object: TerminalViTextObject,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };
        if !state.term.mode().contains(TermMode::VI) {
            return false;
        }

        match text_object {
            TerminalViTextObject::InnerWord => {
                let _ = select_inner_vim_word(&mut state.term);
            }
            TerminalViTextObject::AroundWord => select_around_vim_word(&mut state.term),
        }
        state.latest_seq = seq;
        state.selection_damage = true;
        true
    }

    pub fn set_vi_search_prompt(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        prompt: Option<TerminalViSearchPrompt>,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };
        if !state.term.mode().contains(TermMode::VI) || state.vi_search_prompt == prompt {
            return false;
        }

        state.vi_search_prompt = prompt;
        state.latest_seq = seq;
        state.selection_damage = true;
        true
    }

    pub fn vi_search(
        &self,
        render_target_id: RenderTargetId,
        seq: Seq,
        pattern: &str,
        direction: TerminalViSearchDirection,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };
        if !state.term.mode().contains(TermMode::VI) || pattern.is_empty() {
            return false;
        }

        let Ok(mut regex) = RegexSearch::new(pattern) else {
            return false;
        };
        let cursor = state.term.vi_mode_cursor.point;
        let (origin, direction) = match direction {
            TerminalViSearchDirection::Forward => {
                (cursor.add(&state.term, Boundary::None, 1), Direction::Right)
            }
            TerminalViSearchDirection::Backward => {
                (cursor.sub(&state.term, Boundary::None, 1), Direction::Left)
            }
        };
        let Some(regex_match) =
            state
                .term
                .search_next(&mut regex, origin, direction, Side::Left, None)
        else {
            return false;
        };

        state.term.vi_goto_point(*regex_match.start());
        state.latest_seq = seq;
        state.selection_damage = true;
        true
    }

    pub fn vi_mode_enabled(&self, render_target_id: RenderTargetId) -> bool {
        let inner = self.inner.borrow();
        inner
            .get(&render_target_id)
            .is_some_and(|state| state.term.mode().contains(TermMode::VI))
    }

    fn snapshot_from_state(
        render_target_id: RenderTargetId,
        state: &mut AlacrittyTermState,
        color_theme: &TerminalColorTheme,
    ) -> TerminalSnapshot {
        let (lines, text_runs) = visible_lines_and_runs(&state.term, color_theme);
        let dirty_rows = dirty_rows_from_state(state);

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
        color_theme: &TerminalColorTheme,
    ) -> RenderSurfaceSnapshot {
        let mut rows = visible_surface_rows(&state.term, color_theme);
        if state.term.mode().contains(TermMode::VI) {
            if !state.host_search_mode {
                append_vi_mode_indicator(&mut rows, state.size.columns());
            }
            if let Some(prompt) = state.vi_search_prompt.as_ref() {
                append_vi_search_prompt(
                    &mut rows,
                    state.size.columns(),
                    state.size.screen_lines(),
                    prompt,
                );
            }
        }
        let dirty_rows = dirty_rows_from_state(state);

        let placeholder_cells = kitty_placeholder_cells(&state.term);
        let renderable = state.term.renderable_content();
        let default_background = renderable.colors[NamedColor::Background]
            .map(rgb_to_dto)
            .or_else(|| dominant_background_of_term(&state.term, color_theme))
            .unwrap_or(color_theme.background);

        RenderSurfaceSnapshot {
            target_id: render_target_id,
            latest_seq: state.latest_seq,
            default_background,
            rows,
            video_surfaces: Vec::new(),
            image_surfaces: state.graphics.snapshots(&placeholder_cells),
            dirty_rows,
            cursor: None,
            ime_preedit: None,
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

    pub fn visible_hyperlinks(&self, render_target_id: RenderTargetId) -> Vec<TerminalHyperlink> {
        let inner = self.inner.borrow();
        inner
            .get(&render_target_id)
            .map(|state| visible_hyperlinks(&state.term))
            .unwrap_or_default()
    }

    pub fn take_pending_pty_writes(&self, render_target_id: RenderTargetId) -> Vec<Vec<u8>> {
        let mut inner = self.inner.borrow_mut();

        let Some(state) = inner.get_mut(&render_target_id) else {
            return Vec::new();
        };

        state.take_pending_writes(&self.color_theme)
    }

    pub fn take_title_change(&self, render_target_id: RenderTargetId) -> Option<Option<String>> {
        let mut inner = self.inner.borrow_mut();
        let state = inner.get_mut(&render_target_id)?;
        if !state.title_changed {
            return None;
        }

        state.title_changed = false;
        Some(state.title.clone())
    }

    pub fn take_bell(&self, render_target_id: RenderTargetId) -> bool {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return false;
        };

        let mut rang = false;
        while state.pending_bell_rx.try_recv().is_ok() {
            rang = true;
        }
        rang
    }

    pub(crate) fn take_clipboard_stores(
        &self,
        render_target_id: RenderTargetId,
    ) -> Vec<(TerminalClipboard, String)> {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return Vec::new();
        };

        state.pending_clipboard_store_rx.try_iter().collect()
    }

    pub(crate) fn take_clipboard_loads(
        &self,
        render_target_id: RenderTargetId,
    ) -> Vec<TerminalClipboardLoad> {
        let mut inner = self.inner.borrow_mut();
        let Some(state) = inner.get_mut(&render_target_id) else {
            return Vec::new();
        };

        state.pending_clipboard_load_rx.try_iter().collect()
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
        .with_kitty_keyboard(
            mode.contains(TermMode::DISAMBIGUATE_ESC_CODES),
            mode.contains(TermMode::REPORT_EVENT_TYPES),
            mode.contains(TermMode::REPORT_ALTERNATE_KEYS),
            mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
            mode.contains(TermMode::REPORT_ASSOCIATED_TEXT),
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
        let vi_mode = state.term.mode().contains(TermMode::VI);
        if !vi_mode
            && (state.term.grid().display_offset() != 0
                || !state.term.mode().contains(TermMode::SHOW_CURSOR))
        {
            return None;
        }
        let renderable = state.term.renderable_content();
        let cursor = renderable.cursor;
        let shape = match cursor.shape {
            CursorShape::Block => RenderSurfaceCursorShape::Block,
            CursorShape::Underline => RenderSurfaceCursorShape::Underline,
            CursorShape::Beam => RenderSurfaceCursorShape::Beam,
            CursorShape::HollowBlock => RenderSurfaceCursorShape::HollowBlock,
            CursorShape::Hidden => RenderSurfaceCursorShape::Hidden,
        };
        let point = point_to_viewport(renderable.display_offset, cursor.point)?;
        Some(RenderSurfaceCursorSnapshot {
            x: u32::try_from(point.column.0).ok()?,
            y: u32::try_from(point.line).ok()?,
            focused: true,
            shape,
            blinking: state.term.cursor_style().blinking,
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

        Some(Self::snapshot_from_state(
            render_target_id,
            state,
            &self.color_theme,
        ))
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
            &self.color_theme,
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

        Some(Self::snapshot_from_state(
            render_target_id,
            state,
            &self.color_theme,
        ))
    }

    fn clear_damage_up_to(&self, render_target_id: RenderTargetId, presented_seq: Seq) {
        let mut inner = self.inner.borrow_mut();

        let Some(state) = inner.get_mut(&render_target_id) else {
            return;
        };

        if state.latest_seq <= presented_seq {
            state.term.reset_damage();
            state.selection_damage = false;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VimWordClass {
    Whitespace,
    Keyword,
    Punctuation,
}

fn vim_word_class(c: char) -> VimWordClass {
    if c.is_whitespace() {
        VimWordClass::Whitespace
    } else if c == '_' || c.is_alphanumeric() {
        VimWordClass::Keyword
    } else {
        VimWordClass::Punctuation
    }
}

fn current_vim_word_class<T>(term: &Term<T>) -> VimWordClass {
    vim_word_class(term.grid()[term.vi_mode_cursor.point].c)
}

fn set_vi_cursor_point<T: EventListener>(term: &mut Term<T>, point: Point) {
    term.vi_mode_cursor.point = point;
    term.scroll_to_point(point);
    if let Some(selection) = term
        .selection
        .as_mut()
        .filter(|selection| !selection.is_empty())
    {
        selection.update(point, Side::Left);
        selection.include_all();
    }
}

fn vim_step_right<T: EventListener>(term: &mut Term<T>) -> bool {
    let previous = term.vi_mode_cursor.point;
    term.vi_motion(alacritty_terminal::vi_mode::ViMotion::Right);
    if term.vi_mode_cursor.point != previous {
        return true;
    }

    let bottommost_line = term.grid().bottommost_line();
    if previous.line >= bottommost_line {
        return false;
    }
    set_vi_cursor_point(term, Point::new(previous.line + 1, Column(0)));
    true
}

fn vim_step_left<T: EventListener>(term: &mut Term<T>) -> bool {
    let previous = term.vi_mode_cursor.point;
    term.vi_motion(alacritty_terminal::vi_mode::ViMotion::Left);
    if term.vi_mode_cursor.point != previous {
        return true;
    }

    let topmost_line = term.grid().topmost_line();
    if previous.line <= topmost_line {
        return false;
    }
    set_vi_cursor_point(
        term,
        Point::new(previous.line - 1, term.grid().last_column()),
    );
    true
}

fn vim_step_right_in_logical_line<T: EventListener>(term: &mut Term<T>) -> bool {
    let previous = term.vi_mode_cursor.point;
    term.vi_motion(alacritty_terminal::vi_mode::ViMotion::Right);
    term.vi_mode_cursor.point != previous
}

fn vim_step_left_in_logical_line<T: EventListener>(term: &mut Term<T>) -> bool {
    let previous = term.vi_mode_cursor.point;
    term.vi_motion(alacritty_terminal::vi_mode::ViMotion::Left);
    term.vi_mode_cursor.point != previous
}

fn vim_word_right<T: EventListener>(term: &mut Term<T>) {
    let initial_class = current_vim_word_class(term);
    if !vim_step_right(term) {
        return;
    }

    if initial_class != VimWordClass::Whitespace {
        while current_vim_word_class(term) == initial_class {
            if !vim_step_right(term) {
                return;
            }
        }
    }
    while current_vim_word_class(term) == VimWordClass::Whitespace {
        if !vim_step_right(term) {
            return;
        }
    }
}

fn vim_word_left<T: EventListener>(term: &mut Term<T>) {
    if !vim_step_left(term) {
        return;
    }
    while current_vim_word_class(term) == VimWordClass::Whitespace {
        if !vim_step_left(term) {
            return;
        }
    }

    let target_class = current_vim_word_class(term);
    while vim_step_left(term) {
        if current_vim_word_class(term) != target_class {
            let _ = vim_step_right(term);
            return;
        }
    }
}

fn vim_word_right_end<T: EventListener>(term: &mut Term<T>) {
    let initial_class = current_vim_word_class(term);
    if !vim_step_right(term) {
        return;
    }

    if initial_class == VimWordClass::Whitespace || current_vim_word_class(term) != initial_class {
        while current_vim_word_class(term) == VimWordClass::Whitespace {
            if !vim_step_right(term) {
                return;
            }
        }
    }

    let target_class = current_vim_word_class(term);
    while vim_step_right(term) {
        if current_vim_word_class(term) != target_class {
            let _ = vim_step_left(term);
            return;
        }
    }
}

fn select_inner_vim_word<T: EventListener>(term: &mut Term<T>) -> (Point, Point) {
    term.selection = None;
    let target_class = current_vim_word_class(term);
    while vim_step_left(term) {
        if current_vim_word_class(term) != target_class {
            let _ = vim_step_right(term);
            break;
        }
    }

    let selection_start = term.vi_mode_cursor.point;
    let mut selection = Selection::new(SelectionType::Simple, selection_start, Side::Left);
    selection.include_all();
    term.selection = Some(selection);

    while vim_step_right(term) {
        if current_vim_word_class(term) != target_class {
            let _ = vim_step_left(term);
            break;
        }
    }

    (selection_start, term.vi_mode_cursor.point)
}

fn select_around_vim_word<T: EventListener>(term: &mut Term<T>) {
    let (word_start, word_end) = select_inner_vim_word(term);
    let mut trailing_whitespace = false;

    while vim_step_right_in_logical_line(term) {
        if current_vim_word_class(term) == VimWordClass::Whitespace {
            trailing_whitespace = true;
            continue;
        }

        let _ = vim_step_left_in_logical_line(term);
        return;
    }

    if !trailing_whitespace {
        return;
    }

    term.selection = None;
    set_vi_cursor_point(term, word_start);
    let mut preceding_whitespace = false;
    while vim_step_left_in_logical_line(term) {
        if current_vim_word_class(term) == VimWordClass::Whitespace {
            preceding_whitespace = true;
        } else {
            let _ = vim_step_right_in_logical_line(term);
            break;
        }
    }

    let selection_start = if preceding_whitespace {
        term.vi_mode_cursor.point
    } else {
        word_start
    };
    let mut selection = Selection::new(SelectionType::Simple, word_end, Side::Right);
    selection.update(selection_start, Side::Left);
    selection.include_all();
    term.selection = Some(selection);
}

#[derive(Debug, Clone, Copy)]
struct TerminalCursorObservation {
    column: u32,
    line: u32,
    input_needs_wrap: bool,
}

impl TerminalCursorObservation {
    const fn point(self) -> (u32, u32) {
        (self.column, self.line)
    }
}

fn terminal_cursor_observation<T: EventListener>(term: &Term<T>) -> TerminalCursorObservation {
    let cursor = &term.grid().cursor;
    TerminalCursorObservation {
        column: u32::try_from(cursor.point.column.0).unwrap_or(0),
        line: u32::try_from(cursor.point.line.0).unwrap_or(0),
        input_needs_wrap: cursor.input_needs_wrap,
    }
}

pub struct AlacrittyTermState {
    term: Term<PtyWriteEventListener>,
    pending_write_tx: Sender<PendingPtyWrite>,
    pending_write_rx: Receiver<PendingPtyWrite>,
    pending_title_rx: Receiver<Option<String>>,
    pending_bell_rx: Receiver<()>,
    pending_clipboard_store_rx: Receiver<(TerminalClipboard, String)>,
    pending_clipboard_load_rx: Receiver<TerminalClipboardLoad>,
    title: Option<String>,
    title_changed: bool,
    processor: Processor<StdSyncHandler>,
    graphics_lifecycle_processor: Processor<StdSyncHandler>,
    graphics_lifecycle_observer: KittyTerminalLifecycleObserver,
    graphics_decoder: KittyGraphicsStreamDecoder,
    graphics: KittyGraphicsState,
    size: AlacrittyTermSize,
    latest_seq: Seq,
    total_bytes: u64,
    chunk_count: u64,
    selection_damage: bool,
    host_search_mode: bool,
    vi_search_prompt: Option<TerminalViSearchPrompt>,
}

impl AlacrittyTermState {
    fn new(
        size: AlacrittyTermSize,
        scrollback_history: usize,
        cursor_style: TerminalCursorStyle,
        osc52_mode: TerminalOsc52Mode,
    ) -> Self {
        let (pending_write_tx, pending_write_rx) = mpsc::channel();
        let (pending_title_tx, pending_title_rx) = mpsc::channel();
        let (pending_bell_tx, pending_bell_rx) = mpsc::channel();
        let (pending_clipboard_store_tx, pending_clipboard_store_rx) = mpsc::channel();
        let (pending_clipboard_load_tx, pending_clipboard_load_rx) = mpsc::channel();
        let event_listener = PtyWriteEventListener::new(
            pending_write_tx.clone(),
            pending_title_tx,
            pending_bell_tx,
            pending_clipboard_store_tx,
            pending_clipboard_load_tx,
        );
        let config = Config {
            scrolling_history: scrollback_history,
            kitty_keyboard: true,
            default_cursor_style: CursorStyle {
                shape: match cursor_style.shape {
                    TerminalCursorShape::Block => CursorShape::Block,
                    TerminalCursorShape::Underline => CursorShape::Underline,
                    TerminalCursorShape::Beam => CursorShape::Beam,
                },
                blinking: cursor_style.blinking,
            },
            osc52: alacritty_osc52_mode(osc52_mode),
            ..Config::default()
        };
        let mut term = Term::new(config, &size, event_listener.clone());

        term.reset_damage();

        Self {
            term,
            pending_write_tx,
            pending_write_rx,
            pending_title_rx,
            pending_bell_rx,
            pending_clipboard_store_rx,
            pending_clipboard_load_rx,
            title: None,
            title_changed: false,
            processor: Processor::<StdSyncHandler>::new(),
            graphics_lifecycle_processor: Processor::<StdSyncHandler>::new(),
            graphics_lifecycle_observer: KittyTerminalLifecycleObserver::new(size.screen_lines()),
            graphics_decoder: KittyGraphicsStreamDecoder::default(),
            graphics: KittyGraphicsState::default(),
            size,
            latest_seq: Seq::ZERO,
            total_bytes: 0,
            chunk_count: 0,
            selection_damage: false,
            host_search_mode: false,
            vi_search_prompt: None,
        }
    }

    fn advance_terminal_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            let before = terminal_cursor_observation(&self.term);
            self.graphics_lifecycle_processor.advance(
                &mut self.graphics_lifecycle_observer,
                std::slice::from_ref(byte),
            );
            self.processor
                .advance(&mut self.term, std::slice::from_ref(byte));
            let after = terminal_cursor_observation(&self.term);
            let events = self.graphics_lifecycle_observer.take_events();

            for event in events {
                self.apply_graphics_lifecycle_event(event, before, after);
            }
        }
    }

    fn apply_graphics_lifecycle_event(
        &mut self,
        event: KittyTerminalLifecycleEvent,
        before: TerminalCursorObservation,
        after: TerminalCursorObservation,
    ) {
        match event {
            KittyTerminalLifecycleEvent::ClearScreen(ClearMode::All) => {
                self.graphics.clear_screen_all();
            }
            KittyTerminalLifecycleEvent::ClearScreen(ClearMode::Above) => {
                self.graphics.clear_screen_above(before.point());
            }
            KittyTerminalLifecycleEvent::ClearScreen(ClearMode::Below) => {
                self.graphics.clear_screen_below(before.point());
            }
            KittyTerminalLifecycleEvent::ClearScreen(ClearMode::Saved) => {}
            KittyTerminalLifecycleEvent::Reset => self.graphics.reset_terminal(),
            KittyTerminalLifecycleEvent::EnterAlternateScreen => {
                self.graphics.enter_alternate_screen();
            }
            KittyTerminalLifecycleEvent::LeaveAlternateScreen => {
                self.graphics.leave_alternate_screen();
            }
            KittyTerminalLifecycleEvent::ScrollUp { start, end, lines } => {
                self.graphics.scroll_up(start, end, lines);
            }
            KittyTerminalLifecycleEvent::ScrollDown { start, end, lines } => {
                self.graphics.scroll_down(start, end, lines);
            }
            KittyTerminalLifecycleEvent::DeleteLines { end, lines } => {
                if before.line < end {
                    self.graphics.scroll_up(before.line, end, lines);
                }
            }
            KittyTerminalLifecycleEvent::InsertBlankLines { end, lines } => {
                if before.line < end {
                    self.graphics.scroll_down(before.line, end, lines);
                }
            }
            KittyTerminalLifecycleEvent::Linefeed { start, end } => {
                if before.line == end.saturating_sub(1) && after.line == before.line {
                    self.graphics.scroll_up(start, end, 1);
                }
            }
            KittyTerminalLifecycleEvent::ReverseIndex { start, end } => {
                if before.line == start && after.line == before.line {
                    self.graphics.scroll_down(start, end, 1);
                }
            }
            KittyTerminalLifecycleEvent::Input { start, end } => {
                let wrapped = before.line == end.saturating_sub(1)
                    && after.line == before.line
                    && (before.input_needs_wrap || after.column < before.column);
                if wrapped {
                    self.graphics.scroll_up(start, end, 1);
                }
            }
        }
    }

    fn resize(&mut self, size: AlacrittyTermSize) {
        if self.size == size {
            return;
        }

        self.size = size;
        self.term.resize(self.size);
        self.graphics_lifecycle_observer
            .resize(self.size.screen_lines());
    }

    fn take_pending_writes(&mut self, color_theme: &TerminalColorTheme) -> Vec<Vec<u8>> {
        let mut writes = Vec::new();

        while let Ok(write) = self.pending_write_rx.try_recv() {
            match write {
                PendingPtyWrite::Bytes(bytes) => writes.push(bytes),
                PendingPtyWrite::ColorRequest(index, formatter) => {
                    if let Some(color) = requested_color(index, self.term.colors(), color_theme) {
                        writes.push(formatter(color).into_bytes());
                    }
                }
            }
        }

        writes
    }

    fn apply_pending_title_changes(&mut self) {
        while let Ok(title) = self.pending_title_rx.try_recv() {
            let title = title.and_then(normalize_terminal_title);
            if self.title != title {
                self.title = title;
                self.title_changed = true;
            }
        }
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

        let before = terminal_cursor_observation(&self.term);
        self.graphics_lifecycle_processor
            .stop_sync(&mut self.graphics_lifecycle_observer);
        self.processor.stop_sync(&mut self.term);
        let after = terminal_cursor_observation(&self.term);
        let events = self.graphics_lifecycle_observer.take_events();
        for event in events {
            self.apply_graphics_lifecycle_event(event, before, after);
        }
        true
    }

    fn mark_selection_damage_if_changed(&mut self, previous: Option<Selection>) {
        if self.term.selection != previous {
            self.selection_damage = true;
        }
    }
}

fn normalize_terminal_title(title: String) -> Option<String> {
    let title = title
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let title = title.trim();
    (!title.is_empty()).then(|| title.chars().take(256).collect())
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
    zerowidth: Vec<char>,
    style: TextStyleDto,
}

fn visible_lines_and_runs(
    term: &Term<PtyWriteEventListener>,
    color_theme: &TerminalColorTheme,
) -> (Vec<TerminalLineSnapshot>, Vec<TerminalTextRunSnapshot>) {
    let renderable = term.renderable_content();
    let display_offset = renderable.display_offset;
    let selection = renderable.selection;
    let cursor = renderable.cursor;
    let colors = renderable.colors;
    let mut cells_by_row: BTreeMap<u32, Vec<StyledCell>> = BTreeMap::new();

    for indexed in renderable.display_iter {
        let Some(point) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let selected = selection
            .is_some_and(|selection| selection.contains_cell(&indexed, cursor.point, cursor.shape));
        let cell = indexed.cell;
        let row = point.line as u32;
        let col = point.column.0 as u32;

        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.c == KITTY_IMAGE_PLACEHOLDER {
            continue;
        }

        let style = style_of_cell(cell.fg, cell.bg, cell.flags, colors, color_theme);
        cells_by_row.entry(row).or_default().push(StyledCell {
            col,
            c: cell.c,
            zerowidth: cell.zerowidth().unwrap_or_default().to_vec(),
            style: if selected {
                selected_style(style, color_theme)
            } else {
                style
            },
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

fn append_vi_mode_indicator(rows: &mut Vec<RenderSurfaceRowSnapshot>, columns: usize) {
    if columns == 0 {
        return;
    }

    let text = match columns {
        1 => "V",
        2..=3 => "VI",
        _ => " VI ",
    };
    let x = columns.saturating_sub(text.chars().count()) as u32;
    let indicator = RenderSurfaceRunSnapshot {
        x,
        text: text.to_string(),
        style: TextStyleDto {
            foreground: Some(RgbColorDto::new(255, 255, 255)),
            background: Some(RgbColorDto::new(38, 92, 168)),
            bold: true,
            italic: false,
            underline: false,
        },
        decoration: Default::default(),
    };

    if let Some(row) = rows.iter_mut().find(|row| row.y == 0) {
        row.runs.push(indicator);
    } else {
        rows.push(RenderSurfaceRowSnapshot {
            y: 0,
            runs: vec![indicator],
        });
        rows.sort_by_key(|row| row.y);
    }
}

fn append_vi_search_prompt(
    rows: &mut Vec<RenderSurfaceRowSnapshot>,
    columns: usize,
    screen_lines: usize,
    prompt: &TerminalViSearchPrompt,
) {
    if columns == 0 || screen_lines == 0 {
        return;
    }

    let marker = match prompt.direction {
        TerminalViSearchDirection::Forward => '/',
        TerminalViSearchDirection::Backward => '?',
    };
    let query_columns = columns.saturating_sub(2);
    let mut query = prompt
        .query
        .chars()
        .rev()
        .take(query_columns)
        .collect::<Vec<_>>();
    query.reverse();

    let mut text = String::with_capacity(columns);
    text.push(marker);
    text.extend(query);
    if text.chars().count() < columns {
        text.push(' ');
    }

    let prompt_run = RenderSurfaceRunSnapshot {
        x: 0,
        text,
        style: TextStyleDto {
            foreground: Some(RgbColorDto::new(255, 255, 255)),
            background: Some(RgbColorDto::new(38, 92, 168)),
            bold: false,
            italic: false,
            underline: false,
        },
        decoration: Default::default(),
    };
    let row_index = screen_lines.saturating_sub(1) as u32;
    if let Some(row) = rows.iter_mut().find(|row| row.y == row_index) {
        row.runs.push(prompt_run);
    } else {
        rows.push(RenderSurfaceRowSnapshot {
            y: row_index,
            runs: vec![prompt_run],
        });
        rows.sort_by_key(|row| row.y);
    }
}

fn visible_surface_rows(
    term: &Term<PtyWriteEventListener>,
    color_theme: &TerminalColorTheme,
) -> Vec<RenderSurfaceRowSnapshot> {
    let renderable = term.renderable_content();
    let display_offset = renderable.display_offset;
    let selection = renderable.selection;
    let cursor = renderable.cursor;
    let colors = renderable.colors;
    let mut rows = Vec::new();
    let mut current_row = None::<u32>;
    let mut current_runs = Vec::new();
    let mut current_x = 0_u32;
    let mut current_next_x = 0_u32;
    let mut current_text = String::new();
    let mut current_style = None::<(TextStyleDto, RenderSurfaceTextDecoration)>;

    for indexed in renderable.display_iter {
        let Some(point) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let selected = selection
            .is_some_and(|selection| selection.contains_cell(&indexed, cursor.point, cursor.shape));
        let cell = indexed.cell;
        let row = point.line as u32;
        let col = point.column.0 as u32;

        if current_row != Some(row) {
            if let Some((style, decoration)) = current_style.take() {
                push_surface_run_if_not_blank(
                    &mut current_runs,
                    current_x,
                    &current_text,
                    style,
                    decoration,
                );
                current_text.clear();
            }

            if let Some(previous_row) = current_row.replace(row)
                && !current_runs.is_empty()
            {
                rows.push(RenderSurfaceRowSnapshot {
                    y: previous_row,
                    runs: std::mem::take(&mut current_runs),
                });
            }
        }

        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        if cell.c == KITTY_IMAGE_PLACEHOLDER {
            continue;
        }

        let mut style = style_of_cell(cell.fg, cell.bg, cell.flags, colors, color_theme);
        let mut decoration = decoration_of_cell(cell, colors, color_theme);
        if cell.hyperlink().is_some() {
            style.underline = true;
            if decoration.underline == RenderSurfaceUnderlineStyle::None {
                decoration.underline = RenderSurfaceUnderlineStyle::Single;
            }
        }
        let style = if selected {
            selected_style(style, color_theme)
        } else {
            style
        };
        if cell.c == ' '
            && cell.zerowidth().is_none_or(<[char]>::is_empty)
            && !surface_style_has_visible_content(style, decoration)
        {
            continue;
        }

        let cell_width = terminal_char_cell_width(cell.c).max(1);
        let is_contiguous = current_style.is_some() && col == current_next_x;

        match current_style {
            None => {
                current_x = col;
                current_next_x = col + cell_width;
                push_cell_characters(
                    &mut current_text,
                    cell.c,
                    cell.zerowidth().unwrap_or_default(),
                );
                current_style = Some((style, decoration));
            }
            Some((existing_style, existing_decoration))
                if existing_style == style
                    && existing_decoration == decoration
                    && is_contiguous =>
            {
                push_cell_characters(
                    &mut current_text,
                    cell.c,
                    cell.zerowidth().unwrap_or_default(),
                );
                current_next_x = col + cell_width;
            }
            Some((existing_style, existing_decoration)) => {
                push_surface_run_if_not_blank(
                    &mut current_runs,
                    current_x,
                    &current_text,
                    existing_style,
                    existing_decoration,
                );

                current_x = col;
                current_next_x = col + cell_width;
                current_text.clear();
                push_cell_characters(
                    &mut current_text,
                    cell.c,
                    cell.zerowidth().unwrap_or_default(),
                );
                current_style = Some((style, decoration));
            }
        }
    }

    if let Some((style, decoration)) = current_style.take() {
        push_surface_run_if_not_blank(
            &mut current_runs,
            current_x,
            &current_text,
            style,
            decoration,
        );
    }

    if let Some(row) = current_row
        && !current_runs.is_empty()
    {
        rows.push(RenderSurfaceRowSnapshot {
            y: row,
            runs: current_runs,
        });
    }

    rows
}

fn visible_hyperlinks(term: &Term<PtyWriteEventListener>) -> Vec<TerminalHyperlink> {
    let renderable = term.renderable_content();
    let display_offset = renderable.display_offset;
    let mut hyperlinks = Vec::<TerminalHyperlink>::new();

    for indexed in renderable.display_iter {
        let Some(point) = point_to_viewport(display_offset, indexed.point) else {
            continue;
        };
        let cell = indexed.cell;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let Some(hyperlink) = cell.hyperlink() else {
            continue;
        };

        let x = point.column.0 as u32;
        let y = point.line as u32;
        let columns = terminal_char_cell_width(cell.c).max(1);
        if let Some(previous) = hyperlinks.last_mut()
            && previous.uri == hyperlink.uri()
            && previous.y == y
            && previous.x.saturating_add(previous.columns) == x
        {
            previous.columns = previous.columns.saturating_add(columns);
            continue;
        }

        hyperlinks.push(TerminalHyperlink {
            uri: hyperlink.uri().to_string(),
            x,
            y,
            columns,
        });
    }

    hyperlinks
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
                placement_id: match cell.underline_color() {
                    Some(Color::Spec(rgb)) => {
                        u32::from(rgb.r) << 16 | u32::from(rgb.g) << 8 | u32::from(rgb.b)
                    }
                    _ => 0,
                },
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

        push_cell_characters(&mut text, cell.c, &cell.zerowidth);
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

        if cell.c == ' ' && cell.zerowidth.is_empty() && !style_has_visible_content(style) {
            continue;
        }

        let cell_width = terminal_char_cell_width(cell.c).max(1);
        let is_contiguous = current_style.is_some() && cell.col == current_next_x;

        match current_style {
            None => {
                current_x = cell.col;
                current_next_x = cell.col + cell_width;
                push_cell_characters(&mut current_text, cell.c, &cell.zerowidth);
                current_style = Some(style);
            }
            Some(existing_style) if existing_style == style && is_contiguous => {
                push_cell_characters(&mut current_text, cell.c, &cell.zerowidth);
                current_next_x = cell.col + cell_width;
            }
            Some(existing_style) => {
                push_run_if_not_blank(&mut runs, current_x, row, &current_text, existing_style);

                current_x = cell.col;
                current_next_x = cell.col + cell_width;
                current_text.clear();
                push_cell_characters(&mut current_text, cell.c, &cell.zerowidth);
                current_style = Some(style);
            }
        }
    }

    if let Some(style) = current_style {
        push_run_if_not_blank(&mut runs, current_x, row, &current_text, style);
    }

    runs
}

fn push_cell_characters(text: &mut String, c: char, zerowidth: &[char]) {
    text.push(c);
    text.extend(zerowidth.iter().copied());
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
    decoration: RenderSurfaceTextDecoration,
) {
    if text.is_empty() {
        return;
    }

    if text.trim().is_empty() && !surface_style_has_visible_content(style, decoration) {
        return;
    }

    runs.push(RenderSurfaceRunSnapshot {
        x,
        text: text.to_string(),
        style,
        decoration,
    });
}

fn surface_style_has_visible_content(
    style: TextStyleDto,
    decoration: RenderSurfaceTextDecoration,
) -> bool {
    style_has_visible_content(style)
        || decoration.underline != RenderSurfaceUnderlineStyle::None
        || decoration.strikeout
}

fn style_has_visible_content(style: TextStyleDto) -> bool {
    style.background.is_some() || style.underline || style.bold || style.italic
}

fn dominant_background_of_term(
    term: &Term<PtyWriteEventListener>,
    color_theme: &TerminalColorTheme,
) -> Option<RgbColorDto> {
    let mut weights = Vec::<(RgbColorDto, u64)>::new();
    let renderable = term.renderable_content();
    for indexed in renderable.display_iter {
        let cell = indexed.cell;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.c == KITTY_IMAGE_PLACEHOLDER {
            continue;
        }
        let style = style_of_cell(cell.fg, cell.bg, cell.flags, renderable.colors, color_theme);
        let background = style.background.unwrap_or(color_theme.background);
        let cell_width = u64::from(terminal_char_cell_width(cell.c).max(1));
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

fn style_of_cell(
    mut foreground_color: Color,
    mut background_color: Color,
    flags: Flags,
    colors: &Colors,
    color_theme: &TerminalColorTheme,
) -> TextStyleDto {
    if flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut foreground_color, &mut background_color);
    }
    let foreground = color_to_rgb(foreground_color, colors, color_theme);
    let background = if background_color == Color::Named(NamedColor::Background) {
        None
    } else {
        color_to_rgb(background_color, colors, color_theme)
    };

    TextStyleDto {
        foreground,
        background,
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.contains(Flags::UNDERLINE),
    }
}

fn decoration_of_cell(
    cell: &Cell,
    colors: &Colors,
    color_theme: &TerminalColorTheme,
) -> RenderSurfaceTextDecoration {
    let underline = if cell.flags.contains(Flags::UNDERCURL) {
        RenderSurfaceUnderlineStyle::Curly
    } else if cell.flags.contains(Flags::DOUBLE_UNDERLINE) {
        RenderSurfaceUnderlineStyle::Double
    } else if cell.flags.contains(Flags::DOTTED_UNDERLINE) {
        RenderSurfaceUnderlineStyle::Dotted
    } else if cell.flags.contains(Flags::DASHED_UNDERLINE) {
        RenderSurfaceUnderlineStyle::Dashed
    } else if cell.flags.contains(Flags::UNDERLINE) {
        RenderSurfaceUnderlineStyle::Single
    } else {
        RenderSurfaceUnderlineStyle::None
    };

    RenderSurfaceTextDecoration {
        underline,
        underline_color: cell
            .underline_color()
            .and_then(|color| color_to_rgb(color, colors, color_theme)),
        strikeout: cell.flags.contains(Flags::STRIKEOUT),
        dim: cell.flags.contains(Flags::DIM),
        hidden: cell.flags.contains(Flags::HIDDEN),
    }
}

fn selected_style(mut style: TextStyleDto, color_theme: &TerminalColorTheme) -> TextStyleDto {
    if color_theme.selection_foreground.is_none() && color_theme.selection_background.is_none() {
        let foreground = style.foreground.unwrap_or(color_theme.foreground);
        let background = style.background.unwrap_or(color_theme.background);
        style.foreground = Some(background);
        style.background = Some(foreground);
        return style;
    }

    if let Some(foreground) = color_theme.selection_foreground {
        style.foreground = Some(foreground);
    }
    if let Some(background) = color_theme.selection_background {
        style.background = Some(background);
    }
    style
}

fn color_to_rgb(
    color: Color,
    colors: &Colors,
    color_theme: &TerminalColorTheme,
) -> Option<RgbColorDto> {
    match color {
        Color::Spec(rgb) => Some(rgb_to_dto(rgb)),
        Color::Named(named) => named_color_to_rgb(named, colors, color_theme),
        Color::Indexed(index) => indexed_color_to_rgb(index, colors, color_theme),
    }
}

fn named_color_to_rgb(
    color: NamedColor,
    colors: &Colors,
    color_theme: &TerminalColorTheme,
) -> Option<RgbColorDto> {
    if let Some(rgb) = colors[color] {
        return Some(rgb_to_dto(rgb));
    }

    match color {
        NamedColor::Black => Some(color_theme.palette[0]),
        NamedColor::Red => Some(color_theme.palette[1]),
        NamedColor::Green => Some(color_theme.palette[2]),
        NamedColor::Yellow => Some(color_theme.palette[3]),
        NamedColor::Blue => Some(color_theme.palette[4]),
        NamedColor::Magenta => Some(color_theme.palette[5]),
        NamedColor::Cyan => Some(color_theme.palette[6]),
        NamedColor::White => Some(color_theme.palette[7]),
        NamedColor::BrightBlack => Some(color_theme.palette[8]),
        NamedColor::BrightRed => Some(color_theme.palette[9]),
        NamedColor::BrightGreen => Some(color_theme.palette[10]),
        NamedColor::BrightYellow => Some(color_theme.palette[11]),
        NamedColor::BrightBlue => Some(color_theme.palette[12]),
        NamedColor::BrightMagenta => Some(color_theme.palette[13]),
        NamedColor::BrightCyan => Some(color_theme.palette[14]),
        NamedColor::BrightWhite => Some(color_theme.palette[15]),
        NamedColor::Foreground => Some(color_theme.foreground),
        NamedColor::Background => Some(color_theme.background),
        NamedColor::Cursor => Some(color_theme.cursor),
        NamedColor::DimBlack => Some(dim_color(color_theme.palette[0])),
        NamedColor::DimRed => Some(dim_color(color_theme.palette[1])),
        NamedColor::DimGreen => Some(dim_color(color_theme.palette[2])),
        NamedColor::DimYellow => Some(dim_color(color_theme.palette[3])),
        NamedColor::DimBlue => Some(dim_color(color_theme.palette[4])),
        NamedColor::DimMagenta => Some(dim_color(color_theme.palette[5])),
        NamedColor::DimCyan => Some(dim_color(color_theme.palette[6])),
        NamedColor::DimWhite => Some(dim_color(color_theme.palette[7])),
        NamedColor::BrightForeground => Some(color_theme.palette[15]),
        NamedColor::DimForeground => Some(dim_color(color_theme.foreground)),
    }
}

fn indexed_color_to_rgb(
    index: u8,
    colors: &Colors,
    color_theme: &TerminalColorTheme,
) -> Option<RgbColorDto> {
    if let Some(rgb) = colors[index as usize] {
        return Some(rgb_to_dto(rgb));
    }

    Some(color_theme.palette[index as usize])
}

fn requested_color(index: usize, colors: &Colors, color_theme: &TerminalColorTheme) -> Option<Rgb> {
    let color = if index <= NamedColor::DimForeground as usize
        && let Some(color) = colors[index]
    {
        rgb_to_dto(color)
    } else {
        match index {
            0..=255 => color_theme.palette[index],
            index if index == NamedColor::Foreground as usize => color_theme.foreground,
            index if index == NamedColor::Background as usize => color_theme.background,
            index if index == NamedColor::Cursor as usize => color_theme.cursor,
            index if index == NamedColor::DimBlack as usize => dim_color(color_theme.palette[0]),
            index if index == NamedColor::DimRed as usize => dim_color(color_theme.palette[1]),
            index if index == NamedColor::DimGreen as usize => dim_color(color_theme.palette[2]),
            index if index == NamedColor::DimYellow as usize => dim_color(color_theme.palette[3]),
            index if index == NamedColor::DimBlue as usize => dim_color(color_theme.palette[4]),
            index if index == NamedColor::DimMagenta as usize => dim_color(color_theme.palette[5]),
            index if index == NamedColor::DimCyan as usize => dim_color(color_theme.palette[6]),
            index if index == NamedColor::DimWhite as usize => dim_color(color_theme.palette[7]),
            index if index == NamedColor::BrightForeground as usize => color_theme.palette[15],
            index if index == NamedColor::DimForeground as usize => {
                dim_color(color_theme.foreground)
            }
            _ => return None,
        }
    };

    Some(Rgb {
        r: color.red,
        g: color.green,
        b: color.blue,
    })
}

fn rgb_to_dto(rgb: Rgb) -> RgbColorDto {
    RgbColorDto::new(rgb.r, rgb.g, rgb.b)
}

fn dim_color(color: RgbColorDto) -> RgbColorDto {
    let dim = |channel: u8| (u16::from(channel) * 2 / 3) as u8;
    RgbColorDto::new(dim(color.red), dim(color.green), dim(color.blue))
}

fn terminal_clipboard(clipboard: ClipboardType) -> TerminalClipboard {
    match clipboard {
        ClipboardType::Clipboard => TerminalClipboard::Clipboard,
        ClipboardType::Selection => TerminalClipboard::Selection,
    }
}

fn alacritty_osc52_mode(mode: TerminalOsc52Mode) -> Osc52 {
    match mode {
        TerminalOsc52Mode::Disabled => Osc52::Disabled,
        TerminalOsc52Mode::OnlyCopy => Osc52::OnlyCopy,
        TerminalOsc52Mode::OnlyPaste => Osc52::OnlyPaste,
        TerminalOsc52Mode::CopyPaste => Osc52::CopyPaste,
    }
}

fn selection_point(
    term: &Term<PtyWriteEventListener>,
    point: TerminalSelectionPoint,
) -> (Point, TerminalSelectionSide) {
    let side = point.side;
    let column = usize::from(point.column).min(term.columns().saturating_sub(1));
    let row = usize::from(point.row).min(term.screen_lines().saturating_sub(1));
    let point = viewport_to_point(
        term.grid().display_offset(),
        Point::new(row, Column(column)),
    );
    (point, side)
}

fn selection_side(side: TerminalSelectionSide) -> Side {
    match side {
        TerminalSelectionSide::Left => Side::Left,
        TerminalSelectionSide::Right => Side::Right,
    }
}

fn dirty_rows_from_state(state: &mut AlacrittyTermState) -> Vec<u32> {
    if state.selection_damage {
        (0..state.size.screen_lines() as u32).collect()
    } else {
        dirty_rows_of(state.term.damage(), state.size.screen_lines())
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

    fn image_count(store: &AlacrittyTerminalStore, target_id: RenderTargetId) -> usize {
        store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .image_surfaces
            .len()
    }

    fn first_image_y(store: &AlacrittyTerminalStore, target_id: RenderTargetId) -> i32 {
        store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .image_surfaces[0]
            .y_cell
    }

    #[test]
    fn exports_osc_title_changes_and_reset() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(81);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b]2;nvim - germinal\x1b\\");
        assert_eq!(
            store.take_title_change(target_id),
            Some(Some("nvim - germinal".to_string()))
        );
        assert_eq!(store.take_title_change(target_id), None);

        store.apply_bytes(target_id, Seq::new(2), b"\x1b]2;\x1b\\");
        assert_eq!(store.take_title_change(target_id), Some(None));
    }

    #[test]
    fn captures_terminal_bell_events() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(83);

        store.apply_bytes(target_id, Seq::new(1), b"before\x07after");

        assert!(store.take_bell(target_id));
        assert!(!store.take_bell(target_id));
    }

    #[test]
    fn osc52_only_copy_exports_clipboard_store_and_denies_load() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(86);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(
            store.take_clipboard_stores(target_id),
            vec![(TerminalClipboard::Clipboard, "hello".to_string())]
        );

        store.apply_bytes(target_id, Seq::new(2), b"\x1b]52;c;?\x07");
        assert!(store.take_clipboard_loads(target_id).is_empty());
    }

    #[test]
    fn osc52_copy_paste_formats_clipboard_load_response() {
        let store = AlacrittyTerminalStore::with_size_scrollback_cursor_style_and_osc52(
            AlacrittyTermSize::default(),
            Config::default().scrolling_history,
            TerminalCursorStyle::default(),
            TerminalOsc52Mode::CopyPaste,
        );
        let target_id = RenderTargetId::new(87);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b]52;p;?\x1b\\");
        let loads = store.take_clipboard_loads(target_id);
        assert_eq!(loads.len(), 1);
        assert_eq!(loads[0].clipboard, TerminalClipboard::Selection);
        assert_eq!((loads[0].formatter)("secret"), "\x1b]52;p;c2VjcmV0\x1b\\");
    }

    #[test]
    fn osc52_disabled_rejects_clipboard_store() {
        let store = AlacrittyTerminalStore::with_size_scrollback_cursor_style_and_osc52(
            AlacrittyTermSize::default(),
            Config::default().scrolling_history,
            TerminalCursorStyle::default(),
            TerminalOsc52Mode::Disabled,
        );
        let target_id = RenderTargetId::new(88);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b]52;c;aGVsbG8=\x07");

        assert!(store.take_clipboard_stores(target_id).is_empty());
    }

    #[test]
    fn exports_and_underlines_osc_8_hyperlinks() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(82);
        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"\x1b]8;;https://example.com/docs\x1b\\docs\x1b]8;;\x1b\\",
        );

        assert_eq!(
            store.visible_hyperlinks(target_id),
            vec![TerminalHyperlink {
                uri: "https://example.com/docs".to_string(),
                x: 0,
                y: 0,
                columns: 4,
            }]
        );
        let snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        assert_eq!(snapshot.rows[0].runs[0].text, "docs");
        assert!(snapshot.rows[0].runs[0].style.underline);
    }

    #[test]
    fn exports_modern_sgr_text_decorations() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(84);
        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"\x1b[2mD\x1b[0m\x1b[8mH\x1b[0m\x1b[9mS\x1b[0m\x1b[4:2m2\x1b[0m\x1b[4:3mC\x1b[0m\x1b[4:4mO\x1b[0m\x1b[4:5mA\x1b[0m\x1b[4;58;2;12;34;56mU",
        );

        let snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        let runs = &snapshot.rows[0].runs;
        let run = |text: &str| runs.iter().find(|run| run.text == text).unwrap();

        assert!(run("D").decoration.dim);
        assert!(run("H").decoration.hidden);
        assert!(run("S").decoration.strikeout);
        assert_eq!(
            run("2").decoration.underline,
            RenderSurfaceUnderlineStyle::Double
        );
        assert_eq!(
            run("C").decoration.underline,
            RenderSurfaceUnderlineStyle::Curly
        );
        assert_eq!(
            run("O").decoration.underline,
            RenderSurfaceUnderlineStyle::Dotted
        );
        assert_eq!(
            run("A").decoration.underline,
            RenderSurfaceUnderlineStyle::Dashed
        );
        assert_eq!(
            run("U").decoration.underline_color,
            Some(RgbColorDto::new(12, 34, 56))
        );
    }

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
    fn kitty_images_follow_clear_reset_and_alternate_screen_lifecycle() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 4));
        let target_id = RenderTargetId::new(101);
        let payload = STANDARD.encode([255, 0, 0, 255]);
        let primary = format!("\x1b_Ga=T,f=32,s=1,v=1,i=7,p=1,C=1;{payload}\x1b\\");
        let alternate = format!("\x1b_Ga=T,f=32,s=1,v=1,i=8,p=1,C=1;{payload}\x1b\\");

        store.apply_bytes(target_id, Seq::new(1), primary.as_bytes());
        assert_eq!(image_count(&store, target_id), 1);

        store.apply_bytes(target_id, Seq::new(2), b"\x1b[?1049h");
        assert_eq!(image_count(&store, target_id), 0);
        store.apply_bytes(target_id, Seq::new(3), alternate.as_bytes());
        assert_eq!(image_count(&store, target_id), 1);

        store.apply_bytes(target_id, Seq::new(4), b"\x1b[?1049l");
        assert_eq!(image_count(&store, target_id), 1);
        store.apply_bytes(target_id, Seq::new(5), b"\x1b[2J");
        assert_eq!(image_count(&store, target_id), 0);

        store.apply_bytes(target_id, Seq::new(6), primary.as_bytes());
        store.apply_bytes(target_id, Seq::new(7), b"\x1bc");
        assert_eq!(image_count(&store, target_id), 0);
    }

    #[test]
    fn kitty_physical_images_scroll_with_terminal_content() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 3));
        let target_id = RenderTargetId::new(102);
        let payload = STANDARD.encode([255, 0, 0, 255]);
        let transfer = format!("\x1b[3;1H\x1b_Ga=T,f=32,s=1,v=1,i=7,p=1,C=1;{payload}\x1b\\");

        store.apply_bytes(target_id, Seq::new(1), transfer.as_bytes());
        assert_eq!(first_image_y(&store, target_id), 2);
        store.apply_bytes(target_id, Seq::new(2), b"\n");
        assert_eq!(first_image_y(&store, target_id), 1);
        store.apply_bytes(target_id, Seq::new(3), b"\x1b[1S");
        assert_eq!(first_image_y(&store, target_id), 0);
        store.apply_bytes(target_id, Seq::new(4), b"\x1b[1S");
        assert_eq!(first_image_y(&store, target_id), -1);
    }

    #[test]
    fn kitty_uppercase_deletion_reaches_hidden_primary_screen_data() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(103);
        let payload = STANDARD.encode([255, 0, 0, 255]);
        let transfer = format!("\x1b_Ga=T,f=32,s=1,v=1,i=7,p=1,C=1;{payload}\x1b\\");

        store.apply_bytes(target_id, Seq::new(1), transfer.as_bytes());
        store.apply_bytes(
            target_id,
            Seq::new(2),
            b"\x1b[?1049h\x1b_Ga=d,d=i,i=7\x1b\\",
        );
        store.apply_bytes(target_id, Seq::new(3), b"\x1b[?1049l");
        assert_eq!(image_count(&store, target_id), 1);

        store.apply_bytes(
            target_id,
            Seq::new(4),
            b"\x1b[?1049h\x1b_Ga=d,d=I,i=7\x1b\\",
        );
        store.apply_bytes(target_id, Seq::new(5), b"\x1b[?1049l");
        assert_eq!(image_count(&store, target_id), 0);
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
    fn parses_reports_and_pops_kitty_keyboard_modes() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(46);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b[>31u\x1b[?u");
        let modes = store.input_modes(target_id);
        assert!(modes.kitty_keyboard());
        assert!(modes.kitty_disambiguate_esc_codes());
        assert!(modes.kitty_report_event_types());
        assert!(modes.kitty_report_alternate_keys());
        assert!(modes.kitty_report_all_keys_as_escape_codes());
        assert!(modes.kitty_report_associated_text());
        assert_eq!(
            store.take_pending_pty_writes(target_id),
            vec![b"\x1b[?31u".to_vec()]
        );

        store.apply_bytes(target_id, Seq::new(2), b"\x1b[<u");
        assert_eq!(store.input_modes(target_id), TerminalInputModes::default());
    }

    #[test]
    fn responds_to_codex_terminal_queries_in_order() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(200, 60));
        let target_id = RenderTargetId::new(47);

        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"\x1b[6n\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[?u\x1b[c",
        );

        assert_eq!(
            store.take_pending_pty_writes(target_id),
            vec![
                b"\x1b[1;1R".to_vec(),
                b"\x1b]10;rgb:e5e5/e5e5/e5e5\x1b\\".to_vec(),
                b"\x1b]11;rgb:0000/0000/0000\x1b\\".to_vec(),
                b"\x1b[?0u".to_vec(),
                b"\x1b[?6c".to_vec(),
            ]
        );
    }

    #[test]
    fn reports_pushed_kitty_keyboard_flags_in_codex_startup_probe() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(200, 60));
        let target_id = RenderTargetId::new(49);

        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"\x1b[?2004h\x1b[>4;0m\x1b[>7u\x1b[?1004h\x1b[6n\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[?u\x1b[c",
        );

        assert_eq!(
            store.take_pending_pty_writes(target_id),
            vec![
                b"\x1b[1;1R".to_vec(),
                b"\x1b]10;rgb:e5e5/e5e5/e5e5\x1b\\".to_vec(),
                b"\x1b]11;rgb:0000/0000/0000\x1b\\".to_vec(),
                b"\x1b[?7u".to_vec(),
                b"\x1b[?6c".to_vec(),
            ]
        );
    }

    #[test]
    fn color_queries_report_dynamic_overrides() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(48);

        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"\x1b]10;#123456\x1b\\\x1b]10;?\x1b\\",
        );

        assert_eq!(
            store.take_pending_pty_writes(target_id),
            vec![b"\x1b]10;rgb:1212/3434/5656\x1b\\".to_vec()]
        );
    }

    #[test]
    fn toggles_alacritty_vi_mode_without_writing_to_the_pty() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(45);

        assert!(!store.vi_mode_enabled(target_id));
        assert!(store.set_vi_mode(target_id, Seq::new(1), true));
        assert!(store.vi_mode_enabled(target_id));
        assert!(store.take_pending_pty_writes(target_id).is_empty());
        let vi_snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        assert_eq!(vi_snapshot.latest_seq, Seq::new(1));
        assert!(vi_snapshot.rows.iter().any(|row| {
            row.runs
                .iter()
                .any(|run| run.text.trim() == "VI" && run.style.background.is_some())
        }));

        assert!(store.set_vi_mode(target_id, Seq::new(2), false));
        assert!(!store.vi_mode_enabled(target_id));
        assert!(store.take_pending_pty_writes(target_id).is_empty());
        assert!(
            !store
                .render_surface_snapshot_of(target_id)
                .unwrap()
                .rows
                .iter()
                .any(|row| row.runs.iter().any(|run| run.text.trim() == "VI"))
        );
    }

    #[test]
    fn vi_cursor_remains_visible_while_viewing_scrollback() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 2));
        let target_id = RenderTargetId::new(54);
        store.apply_bytes(target_id, Seq::new(1), b"one\r\ntwo\r\nthree");
        assert!(store.scroll_display(target_id, Seq::new(2), Scroll::Top));
        assert!(store.cursor_snapshot(target_id).is_none());

        assert!(store.set_vi_mode(target_id, Seq::new(3), true));

        let cursor = store.cursor_snapshot(target_id).unwrap();
        assert_eq!((cursor.x, cursor.y), (0, 0));
        assert_eq!(cursor.shape, RenderSurfaceCursorShape::Block);
    }

    #[test]
    fn vi_motions_move_the_host_cursor_without_pty_writes() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 3));
        let target_id = RenderTargetId::new(55);
        store.apply_bytes(target_id, Seq::new(1), b"alpha beta");
        assert!(store.set_vi_mode(target_id, Seq::new(2), true));

        assert!(store.vi_motion(target_id, Seq::new(3), TerminalViMotion::First));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().x, 0);
        assert!(store.vi_motion(target_id, Seq::new(4), TerminalViMotion::WordRight,));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().x, 6);
        assert!(store.vi_motion(target_id, Seq::new(5), TerminalViMotion::Last));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().x, 9);
        assert!(store.take_pending_pty_writes(target_id).is_empty());
    }

    #[test]
    fn vi_viewport_motions_scroll_history_and_position_the_cursor() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 4));
        let target_id = RenderTargetId::new(61);
        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"00\r\n01\r\n02\r\n03\r\n04\r\n05\r\n06\r\n07",
        );
        assert!(store.set_vi_mode(target_id, Seq::new(2), true));

        assert!(store.vi_motion(target_id, Seq::new(3), TerminalViMotion::PageUp));
        let page_offset = store
            .inner
            .borrow()
            .get(&target_id)
            .unwrap()
            .term
            .grid()
            .display_offset();
        assert!(page_offset >= 4);

        assert!(store.vi_motion(target_id, Seq::new(4), TerminalViMotion::High));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().y, 0);
        assert!(store.vi_motion(target_id, Seq::new(5), TerminalViMotion::Middle));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().y, 1);
        assert!(store.vi_motion(target_id, Seq::new(6), TerminalViMotion::Low));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().y, 3);

        assert!(store.vi_motion(target_id, Seq::new(7), TerminalViMotion::HalfPageDown,));
        let half_page_down_offset = store
            .inner
            .borrow()
            .get(&target_id)
            .unwrap()
            .term
            .grid()
            .display_offset();
        assert_eq!(half_page_down_offset, page_offset - 2);

        assert!(store.vi_motion(target_id, Seq::new(8), TerminalViMotion::HalfPageUp,));
        assert_eq!(
            store
                .inner
                .borrow()
                .get(&target_id)
                .unwrap()
                .term
                .grid()
                .display_offset(),
            page_offset
        );

        assert!(store.vi_motion(target_id, Seq::new(9), TerminalViMotion::PageDown));
        assert_eq!(
            store
                .inner
                .borrow()
                .get(&target_id)
                .unwrap()
                .term
                .grid()
                .display_offset(),
            0
        );
        assert!(store.take_pending_pty_writes(target_id).is_empty());
    }

    #[test]
    fn vi_search_moves_between_wrapped_results_and_renders_the_prompt() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(12, 4));
        let target_id = RenderTargetId::new(62);
        store.apply_bytes(target_id, Seq::new(1), b"alpha\r\nbeta\r\nalpha");
        assert!(store.set_vi_mode(target_id, Seq::new(2), true));

        assert!(store.set_vi_search_prompt(
            target_id,
            Seq::new(3),
            Some(TerminalViSearchPrompt {
                direction: TerminalViSearchDirection::Forward,
                query: "alp".into(),
            }),
        ));
        let snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        assert!(snapshot.rows.iter().any(|row| {
            row.y == 3 && row.runs.iter().any(|run| run.x == 0 && run.text == "/alp ")
        }));

        assert!(store.set_vi_search_prompt(target_id, Seq::new(4), None));
        assert!(store.vi_search(
            target_id,
            Seq::new(5),
            "alpha",
            TerminalViSearchDirection::Forward,
        ));
        let cursor = store.cursor_snapshot(target_id).unwrap();
        assert_eq!((cursor.x, cursor.y), (0, 0));

        assert!(store.vi_search(
            target_id,
            Seq::new(6),
            "alpha",
            TerminalViSearchDirection::Backward,
        ));
        let cursor = store.cursor_snapshot(target_id).unwrap();
        assert_eq!((cursor.x, cursor.y), (0, 2));
        assert!(!store.vi_search(
            target_id,
            Seq::new(7),
            "[",
            TerminalViSearchDirection::Forward,
        ));
        assert!(store.take_pending_pty_writes(target_id).is_empty());
    }

    #[test]
    fn host_search_uses_vi_navigation_without_rendering_the_vi_indicator() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(12, 4));
        let target_id = RenderTargetId::new(63);
        store.apply_bytes(target_id, Seq::new(1), b"alpha\r\nbeta\r\nalpha");

        assert!(store.set_search_mode(target_id, Seq::new(2), true));
        assert!(store.set_vi_search_prompt(
            target_id,
            Seq::new(3),
            Some(TerminalViSearchPrompt {
                direction: TerminalViSearchDirection::Forward,
                query: "alpha".into(),
            }),
        ));
        let snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        assert!(snapshot.rows.iter().any(|row| {
            row.y == 3
                && row
                    .runs
                    .iter()
                    .any(|run| run.x == 0 && run.text == "/alpha ")
        }));
        assert!(!snapshot.rows.iter().any(|row| {
            row.runs
                .iter()
                .any(|run| run.text == " VI " || run.text == "VI" || run.text == "V")
        }));

        assert!(store.vi_search(
            target_id,
            Seq::new(4),
            "alpha",
            TerminalViSearchDirection::Forward,
        ));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().y, 0);

        assert!(store.set_search_mode(target_id, Seq::new(5), false));
        let state = store.inner.borrow();
        let state = state.get(&target_id).unwrap();
        assert!(!state.host_search_mode);
        assert!(!state.term.mode().contains(TermMode::VI));
        assert_eq!(state.term.grid().display_offset(), 0);
    }

    #[test]
    fn vi_mode_keeps_older_history_stable_during_output_and_returns_live_on_exit() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 3));
        let target_id = RenderTargetId::new(63);
        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"one\r\ntwo\r\nthree\r\nfour\r\nfive",
        );
        assert!(store.set_vi_mode(target_id, Seq::new(2), true));
        assert!(store.vi_motion(target_id, Seq::new(3), TerminalViMotion::Top));

        let text_before_output: String = store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .rows
            .iter()
            .flat_map(|row| row.runs.iter())
            .map(|run| run.text.as_str())
            .collect();
        let offset_before_output = store
            .inner
            .borrow()
            .get(&target_id)
            .unwrap()
            .term
            .grid()
            .display_offset();
        assert!(offset_before_output > 0);
        assert!(text_before_output.contains("one"));

        store.apply_bytes(target_id, Seq::new(4), b"\r\nsix");
        let offset_after_output = store
            .inner
            .borrow()
            .get(&target_id)
            .unwrap()
            .term
            .grid()
            .display_offset();
        let text_after_output: String = store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .rows
            .iter()
            .flat_map(|row| row.runs.iter())
            .map(|run| run.text.as_str())
            .collect();
        assert_eq!(offset_after_output, offset_before_output + 1);
        assert!(text_after_output.contains("one"));
        assert!(!text_after_output.contains("six"));

        assert!(store.set_vi_mode(target_id, Seq::new(5), false));
        assert_eq!(
            store
                .inner
                .borrow()
                .get(&target_id)
                .unwrap()
                .term
                .grid()
                .display_offset(),
            0
        );
        let live_text: String = store
            .render_surface_snapshot_of(target_id)
            .unwrap()
            .rows
            .iter()
            .flat_map(|row| row.runs.iter())
            .map(|run| run.text.as_str())
            .collect();
        assert!(live_text.contains("six"));
        assert!(store.take_pending_pty_writes(target_id).is_empty());
    }

    #[test]
    fn vi_word_motion_treats_punctuation_as_vim_word_boundaries() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(60, 3));
        let target_id = RenderTargetId::new(58);
        store.apply_bytes(
            target_id,
            Seq::new(1),
            b"focus_border: rgba(hex: 0xfa2d55cc).into()",
        );
        assert!(store.set_vi_mode(target_id, Seq::new(2), true));
        assert!(store.vi_motion(target_id, Seq::new(3), TerminalViMotion::First));

        for (seq, expected_column) in [(4, 12), (5, 14), (6, 18), (7, 19), (8, 22), (9, 24)] {
            assert!(store.vi_motion(target_id, Seq::new(seq), TerminalViMotion::WordRight,));
            assert_eq!(store.cursor_snapshot(target_id).unwrap().x, expected_column);
        }
        assert!(store.take_pending_pty_writes(target_id).is_empty());
    }

    #[test]
    fn vi_word_motions_group_adjacent_punctuation_like_vim() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(30, 3));
        let target_id = RenderTargetId::new(59);
        store.apply_bytes(target_id, Seq::new(1), b"name().next value");
        assert!(store.set_vi_mode(target_id, Seq::new(2), true));
        assert!(store.vi_motion(target_id, Seq::new(3), TerminalViMotion::First));

        assert!(store.vi_motion(target_id, Seq::new(4), TerminalViMotion::WordRight));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().x, 4);
        assert!(store.vi_motion(target_id, Seq::new(5), TerminalViMotion::WordRight));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().x, 7);
        assert!(store.vi_motion(target_id, Seq::new(6), TerminalViMotion::WordRightEnd));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().x, 10);
        assert!(store.vi_motion(target_id, Seq::new(7), TerminalViMotion::WordLeft));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().x, 7);
        assert!(store.vi_motion(target_id, Seq::new(8), TerminalViMotion::WordLeft));
        assert_eq!(store.cursor_snapshot(target_id).unwrap().x, 4);
    }

    #[test]
    fn vi_visual_selection_toggles_and_follows_the_host_cursor() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 3));
        let target_id = RenderTargetId::new(56);
        store.apply_bytes(target_id, Seq::new(1), b"alpha beta");
        assert!(store.set_vi_mode(target_id, Seq::new(2), true));
        assert!(store.vi_motion(target_id, Seq::new(3), TerminalViMotion::First));

        assert!(store.set_vi_selection(
            target_id,
            Seq::new(4),
            Some(TerminalViSelectionKind::Character),
        ));
        assert_eq!(store.selection_text(target_id).as_deref(), Some("a"));
        assert!(store.vi_motion(target_id, Seq::new(5), TerminalViMotion::WordRight));
        assert_eq!(store.selection_text(target_id).as_deref(), Some("alpha b"));

        assert!(store.set_vi_selection(
            target_id,
            Seq::new(6),
            Some(TerminalViSelectionKind::Line),
        ));
        assert_eq!(
            store.selection_text(target_id).as_deref(),
            Some("alpha beta\n")
        );
        assert!(store.set_vi_selection(target_id, Seq::new(7), None));
        assert_eq!(store.selection_text(target_id), None);
        assert!(store.take_pending_pty_writes(target_id).is_empty());
    }

    #[test]
    fn vi_inner_word_uses_the_same_vim_word_boundaries_as_motion() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 3));
        let target_id = RenderTargetId::new(57);
        store.apply_bytes(target_id, Seq::new(1), b"name().next");
        assert!(store.set_vi_mode(target_id, Seq::new(2), true));
        assert!(store.vi_motion(target_id, Seq::new(3), TerminalViMotion::First));

        assert!(store.select_vi_text_object(
            target_id,
            Seq::new(4),
            TerminalViTextObject::InnerWord,
        ));
        assert_eq!(store.selection_text(target_id).as_deref(), Some("name"));

        assert!(store.vi_motion(target_id, Seq::new(5), TerminalViMotion::WordRight));
        assert!(store.select_vi_text_object(
            target_id,
            Seq::new(6),
            TerminalViTextObject::InnerWord,
        ));
        assert_eq!(store.selection_text(target_id).as_deref(), Some("()."));
        assert!(store.take_pending_pty_writes(target_id).is_empty());
    }

    #[test]
    fn vi_around_word_prefers_following_whitespace_then_preceding_whitespace() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 3));
        let target_id = RenderTargetId::new(60);
        store.apply_bytes(target_id, Seq::new(1), b"one two");
        assert!(store.set_vi_mode(target_id, Seq::new(2), true));
        assert!(store.vi_motion(target_id, Seq::new(3), TerminalViMotion::First));

        assert!(store.select_vi_text_object(
            target_id,
            Seq::new(4),
            TerminalViTextObject::AroundWord,
        ));
        assert_eq!(store.selection_text(target_id).as_deref(), Some("one "));

        assert!(store.set_vi_selection(target_id, Seq::new(5), None));
        assert!(store.vi_motion(target_id, Seq::new(6), TerminalViMotion::WordRight));
        assert!(store.select_vi_text_object(
            target_id,
            Seq::new(7),
            TerminalViTextObject::AroundWord,
        ));
        assert_eq!(store.selection_text(target_id).as_deref(), Some(" two"));
        assert!(store.take_pending_pty_writes(target_id).is_empty());
    }

    #[test]
    fn exports_decscusr_cursor_shape() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(46);
        store.apply_bytes(target_id, Seq::new(1), b"\x1b[5 q");

        let cursor = store.cursor_snapshot(target_id).unwrap();
        assert_eq!(cursor.shape, RenderSurfaceCursorShape::Beam);
        assert!(cursor.blinking);

        store.apply_bytes(target_id, Seq::new(2), b"\x1b[4 q");
        let cursor = store.cursor_snapshot(target_id).unwrap();
        assert_eq!(cursor.shape, RenderSurfaceCursorShape::Underline);
        assert!(!cursor.blinking);
    }

    #[test]
    fn configured_default_cursor_style_is_used_until_decscusr_overrides_it() {
        let store = AlacrittyTerminalStore::with_size_scrollback_and_cursor_style(
            AlacrittyTermSize::default(),
            Config::default().scrolling_history,
            TerminalCursorStyle::new(TerminalCursorShape::Beam, true),
        );
        let target_id = RenderTargetId::new(84);
        store.apply_bytes(target_id, Seq::new(1), b"");

        let cursor = store.cursor_snapshot(target_id).unwrap();
        assert_eq!(cursor.shape, RenderSurfaceCursorShape::Beam);
        assert!(cursor.blinking);

        store.apply_bytes(target_id, Seq::new(2), b"\x1b[2 q");
        let cursor = store.cursor_snapshot(target_id).unwrap();
        assert_eq!(cursor.shape, RenderSurfaceCursorShape::Block);
        assert!(!cursor.blinking);
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
    fn character_selection_tracks_dragged_cell_sides() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(16, 2));
        let target_id = RenderTargetId::new(50);
        store.apply_bytes(target_id, Seq::new(1), b"hello world");

        assert!(store.start_selection(
            target_id,
            Seq::new(2),
            TerminalSelectionKind::Character,
            TerminalSelectionPoint::new(0, 0, TerminalSelectionSide::Left),
        ));
        assert!(store.update_selection(
            target_id,
            Seq::new(3),
            TerminalSelectionPoint::new(4, 0, TerminalSelectionSide::Right),
        ));

        assert_eq!(store.selection_text(target_id).as_deref(), Some("hello"));
        let snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        assert_eq!(snapshot.dirty_rows, vec![0, 1]);
        let selected = snapshot.rows[0]
            .runs
            .iter()
            .find(|run| run.text == "hello")
            .expect("selected text should have its own styled run");
        assert_eq!(selected.style.foreground, Some(RgbColorDto::new(0, 0, 0)));
        assert_eq!(
            selected.style.background,
            Some(RgbColorDto::new(229, 229, 229))
        );
    }

    #[test]
    fn word_and_line_selection_expand_with_alacritty_semantics() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 3));
        let target_id = RenderTargetId::new(51);
        store.apply_bytes(target_id, Seq::new(1), b"hello world\r\nsecond line");

        assert!(store.start_selection(
            target_id,
            Seq::new(2),
            TerminalSelectionKind::Word,
            TerminalSelectionPoint::new(7, 0, TerminalSelectionSide::Left),
        ));
        assert_eq!(store.selection_text(target_id).as_deref(), Some("world"));

        assert!(store.start_selection(
            target_id,
            Seq::new(3),
            TerminalSelectionKind::Line,
            TerminalSelectionPoint::new(3, 1, TerminalSelectionSide::Left),
        ));
        assert_eq!(
            store.selection_text(target_id).as_deref(),
            Some("second line\n")
        );
    }

    #[test]
    fn selection_uses_the_scrollback_viewport_offset() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 2));
        let target_id = RenderTargetId::new(52);
        store.apply_bytes(target_id, Seq::new(1), b"one\r\ntwo\r\nthree");
        assert!(store.scroll_display(target_id, Seq::new(2), Scroll::Top));

        assert!(store.start_selection(
            target_id,
            Seq::new(3),
            TerminalSelectionKind::Word,
            TerminalSelectionPoint::new(1, 0, TerminalSelectionSide::Left),
        ));

        assert_eq!(store.selection_text(target_id).as_deref(), Some("one"));
    }

    #[test]
    fn unrelated_output_preserves_the_active_selection() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 4));
        let target_id = RenderTargetId::new(53);
        store.apply_bytes(target_id, Seq::new(1), b"hello");
        store.start_selection(
            target_id,
            Seq::new(2),
            TerminalSelectionKind::Word,
            TerminalSelectionPoint::new(2, 0, TerminalSelectionSide::Left),
        );

        store.apply_bytes(target_id, Seq::new(3), b"\x1b[2;1Hworld");

        assert_eq!(store.selection_text(target_id).as_deref(), Some("hello"));
    }

    #[test]
    fn scrolling_output_moves_selection_with_its_content() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 2));
        let target_id = RenderTargetId::new(57);
        store.apply_bytes(target_id, Seq::new(1), b"one\r\ntwo");
        store.start_selection(
            target_id,
            Seq::new(2),
            TerminalSelectionKind::Word,
            TerminalSelectionPoint::new(1, 0, TerminalSelectionSide::Left),
        );

        store.apply_bytes(target_id, Seq::new(3), b"\r\nthree");

        assert_eq!(store.selection_text(target_id).as_deref(), Some("one"));
        store.scroll_display(target_id, Seq::new(4), Scroll::Top);
        assert_eq!(store.selection_text(target_id).as_deref(), Some("one"));
    }

    #[test]
    fn destructive_output_clears_selection_and_redraws_the_old_highlight() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 5));
        let target_id = RenderTargetId::new(54);
        store.apply_bytes(target_id, Seq::new(1), b"\x1b[3;1Hselected");
        store.start_selection(
            target_id,
            Seq::new(2),
            TerminalSelectionKind::Line,
            TerminalSelectionPoint::new(2, 2, TerminalSelectionSide::Left),
        );
        store.clear_damage_up_to(target_id, Seq::new(2));

        store.apply_bytes(target_id, Seq::new(3), b"\x1b[3;1H\x1b[2K");

        assert_eq!(store.selection_text(target_id), None);
        assert_eq!(
            store
                .render_surface_snapshot_of(target_id)
                .unwrap()
                .dirty_rows,
            vec![0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn resize_preserves_selection_by_height_and_clears_it_by_width() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(20, 3));
        let target_id = RenderTargetId::new(55);
        store.apply_bytes(target_id, Seq::new(1), b"hello");
        store.start_selection(
            target_id,
            Seq::new(2),
            TerminalSelectionKind::Word,
            TerminalSelectionPoint::new(2, 0, TerminalSelectionSide::Left),
        );

        store.resize(target_id, Seq::new(3), AlacrittyTermSize::new(20, 5));
        assert_eq!(store.selection_text(target_id).as_deref(), Some("hello"));

        store.resize(target_id, Seq::new(4), AlacrittyTermSize::new(10, 5));
        assert_eq!(store.selection_text(target_id), None);
    }

    #[test]
    fn viewport_scrolling_preserves_selection_anchors() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 2));
        let target_id = RenderTargetId::new(56);
        store.apply_bytes(target_id, Seq::new(1), b"one\r\ntwo\r\nthree");
        store.scroll_display(target_id, Seq::new(2), Scroll::Top);
        store.start_selection(
            target_id,
            Seq::new(3),
            TerminalSelectionKind::Word,
            TerminalSelectionPoint::new(1, 0, TerminalSelectionSide::Left),
        );

        store.scroll_display(target_id, Seq::new(4), Scroll::Bottom);
        assert_eq!(store.selection_text(target_id).as_deref(), Some("one"));

        store.scroll_display(target_id, Seq::new(5), Scroll::Top);
        assert_eq!(store.selection_text(target_id).as_deref(), Some("one"));
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
    fn preserves_combining_marks_variation_selectors_and_zwj_sequences() {
        let store = AlacrittyTerminalStore::new();
        let target_id = RenderTargetId::new(1);
        let text = "e\u{301}❤\u{fe0f}👩\u{200d}💻";

        store.apply_bytes(target_id, Seq::new(1), text.as_bytes());

        let snapshot = store.snapshot_of(target_id).unwrap();
        assert_eq!(snapshot.lines[0].text, text);
        assert_eq!(snapshot.text_runs[0].text, text);

        let surface = store.render_surface_snapshot_of(target_id).unwrap();
        assert_eq!(surface.rows[0].runs[0].text, text);
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
    fn applies_configured_kitty_palette_and_default_colors() {
        let mut color_theme = TerminalColorTheme {
            foreground: RgbColorDto::new(210, 211, 212),
            background: RgbColorDto::new(20, 21, 22),
            ..TerminalColorTheme::default()
        };
        color_theme.palette[4] = RgbColorDto::new(40, 80, 160);
        let store = AlacrittyTerminalStore::with_size_scrollback_cursor_style_osc52_and_colors(
            AlacrittyTermSize::default(),
            Config::default().scrolling_history,
            TerminalCursorStyle::default(),
            TerminalOsc52Mode::default(),
            color_theme,
        );
        let target_id = RenderTargetId::new(91);

        store.apply_bytes(target_id, Seq::new(1), b"default \x1b[34mblue\x1b[0m");
        let snapshot = store.snapshot_of(target_id).expect("snapshot should exist");
        let default_run = snapshot
            .text_runs
            .iter()
            .find(|run| run.text == "default")
            .expect("default run should exist");
        let blue_run = snapshot
            .text_runs
            .iter()
            .find(|run| run.text == "blue")
            .expect("blue run should exist");

        assert_eq!(default_run.style.foreground, Some(color_theme.foreground));
        assert_eq!(default_run.style.background, None);
        assert_eq!(blue_run.style.foreground, Some(color_theme.palette[4]));
        assert_eq!(blue_run.style.background, None);
        assert_eq!(
            store
                .render_surface_snapshot_of(target_id)
                .expect("surface snapshot should exist")
                .default_background,
            color_theme.background
        );
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

        store.start_selection(
            target_id,
            Seq::new(2),
            TerminalSelectionKind::Line,
            TerminalSelectionPoint::new(0, 0, TerminalSelectionSide::Left),
        );
        let selected_snapshot = store.render_surface_snapshot_of(target_id).unwrap();
        assert_eq!(
            selected_snapshot.default_background,
            RgbColorDto::new(30, 30, 47)
        );
    }

    #[test]
    fn sparse_explicit_background_does_not_replace_the_terminal_background() {
        let store = AlacrittyTerminalStore::with_size(AlacrittyTermSize::new(8, 2));
        let target_id = RenderTargetId::new(1);

        store.apply_bytes(target_id, Seq::new(1), b"\x1b[48;2;122;162;247m \x1b[0m");
        let snapshot = store.render_surface_snapshot_of(target_id).unwrap();

        assert_eq!(
            snapshot.default_background,
            TerminalColorTheme::default().background
        );
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
