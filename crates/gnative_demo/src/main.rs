use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io,
    io::stdout,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::{
    gnative::{
        frame::{GNativeFrame, GNativeFrameCursor},
        input::{
            GNativeInputElementState, GNativeInputEvent, GNativeInputKey, GNativeInputModifiers,
            GNativeInputNamedKey,
        },
    },
    rendering::frame_plan_builder::RenderCommandDto,
    seq::Seq,
};
use germinal_gnative_sdk::{
    GNativeSdkError,
    local_session::{LocalGNativeFrameWriter, LocalGNativeSession, LocalGNativeTunnelBootstrap},
};
use germinal_gnative_ui::{
    CompiledUi, Element, GridSize, IntoElementNode, UiTree,
    elements::div::{div, h_flex, v_flex},
    px, rgb, rgba,
};
use germinal_gnative_widgets::{
    checkbox::Checkbox,
    group_box::GroupBox,
    input::{Input, InputState},
    label::Label,
};
use thiserror::Error;

const MAX_TODO_EVENTS: usize = 10;

#[derive(Debug, Error)]
enum DemoError {
    #[error(transparent)]
    Sdk(#[from] GNativeSdkError),
    #[error("failed to write enter control sequence: {0}")]
    WriteEnterControlSequence(#[source] io::Error),
    #[error("host closed before initial resize")]
    HostClosedBeforeInitialResize,
}

fn main() -> Result<(), DemoError> {
    let bootstrap = LocalGNativeTunnelBootstrap::from_env()?;
    eprintln!(
        "germinal-gnative-demo connecting to germinal at {}",
        bootstrap.tunnel_env().endpoint
    );
    let mut terminal_stdout = stdout();
    bootstrap
        .write_enter_control_sequence(&mut terminal_stdout)
        .map_err(DemoError::WriteEnterControlSequence)?;

    let mut session = bootstrap.connect()?;
    let accepted = session.accepted().clone();
    let initial_size = wait_for_initial_size(&mut session)?;
    let mut emitter = DemoFrameEmitter::new(accepted.gshell_id, session.frame_writer());
    let mut app = DemoHostApp::new(initial_size);
    app.push_notice(format!(
        "connected gshell={} protocol=v{}",
        accepted.gshell_id.value(),
        accepted.protocol_version
    ));

    let (input_tx, input_rx) = mpsc::channel();
    spawn_input_reader(session, input_tx);
    draw_app(&mut app, &mut emitter)?;

    loop {
        match input_rx.recv_timeout(Duration::from_secs(60)) {
            Ok(message) => {
                if !handle_session_message(&mut app, message) {
                    app.should_quit = true;
                }
                draw_app(&mut app, &mut emitter)?;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn spawn_input_reader(mut session: LocalGNativeSession, tx: mpsc::Sender<SessionMessage>) {
    thread::spawn(move || {
        loop {
            match session.read_input() {
                Ok(Some(input)) => {
                    if tx.send(SessionMessage::Input(input)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(SessionMessage::Closed);
                    break;
                }
                Err(error) => {
                    let _ = tx.send(SessionMessage::Error(error));
                    break;
                }
            }
        }
    });
}

fn handle_session_message(app: &mut DemoHostApp, message: SessionMessage) -> bool {
    match message {
        SessionMessage::Input(input) => {
            handle_input(app, input);
            true
        }
        SessionMessage::Closed => {
            app.push_notice("host closed the GNative session".to_string());
            false
        }
        SessionMessage::Error(error) => {
            app.push_notice(format!("session error: {error}"));
            false
        }
    }
}

fn draw_app(app: &mut DemoHostApp, emitter: &mut DemoFrameEmitter) -> Result<(), DemoError> {
    let layout = app.ui_tree().layout(app.size);
    let compiled = layout.render();
    emitter.send(compiled)?;
    Ok(())
}

fn wait_for_initial_size(session: &mut LocalGNativeSession) -> Result<GridSize, DemoError> {
    while let Some(input) = session.read_input()? {
        if let GNativeInputEvent::Resize { columns, rows, .. } = input {
            return Ok(clamped_size(columns, rows));
        }
    }
    Err(DemoError::HostClosedBeforeInitialResize)
}

fn handle_input(app: &mut DemoHostApp, input: GNativeInputEvent) {
    match input {
        GNativeInputEvent::Resize { columns, rows, .. } => {
            let size = clamped_size(columns, rows);
            app.resize(size);
            app.push_notice(format!("resize {}x{}", columns, rows));
        }
        GNativeInputEvent::Paste(text) | GNativeInputEvent::Ime(text) => app.paste(&text),
        GNativeInputEvent::ImeEnabled
        | GNativeInputEvent::ImePreedit { .. }
        | GNativeInputEvent::ImeDisabled => {}
        GNativeInputEvent::Bytes(bytes) => {
            app.push_notice(format!("unsupported input: bytes {:?}", bytes));
        }
        GNativeInputEvent::ModifiersChanged(_)
        | GNativeInputEvent::FocusChanged(_)
        | GNativeInputEvent::PointerMoved { .. }
        | GNativeInputEvent::PointerLeft
        | GNativeInputEvent::PointerButton { .. }
        | GNativeInputEvent::Scroll { .. } => {}
        GNativeInputEvent::Key {
            state,
            logical_key,
            text,
            modifiers,
        } => {
            handle_key_input(app, state, logical_key, text.as_deref(), modifiers);
        }
    }
}

fn handle_key_input(
    app: &mut DemoHostApp,
    state: GNativeInputElementState,
    logical_key: GNativeInputKey,
    text: Option<&str>,
    modifiers: GNativeInputModifiers,
) {
    if state != GNativeInputElementState::Pressed {
        return;
    }

    if modifiers.control
        && character_of(&logical_key, text).is_some_and(|ch| ch.eq_ignore_ascii_case(&'c'))
    {
        app.should_quit = true;
        return;
    }

    if matches!(
        logical_key,
        GNativeInputKey::Named(GNativeInputNamedKey::F1)
    ) {
        app.toggle_switcher();
        return;
    }

    if app.switcher_open() {
        match logical_key {
            GNativeInputKey::Named(GNativeInputNamedKey::ArrowUp) => app.select_previous_demo(),
            GNativeInputKey::Named(GNativeInputNamedKey::ArrowDown) => app.select_next_demo(),
            GNativeInputKey::Named(GNativeInputNamedKey::Enter)
            | GNativeInputKey::Named(GNativeInputNamedKey::Escape) => {
                if matches!(
                    logical_key,
                    GNativeInputKey::Named(GNativeInputNamedKey::Enter)
                ) {
                    app.activate_selected_demo();
                } else {
                    app.close_switcher();
                }
            }
            _ if !modifiers.control && !modifiers.alt => {
                if let Some(ch) = character_of(&logical_key, text) {
                    match ch {
                        'j' => app.select_next_demo(),
                        'k' => app.select_previous_demo(),
                        ' ' => app.activate_selected_demo(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        return;
    }

    app.handle_demo_key(logical_key, text, modifiers);
}

fn character_of(logical_key: &GNativeInputKey, text: Option<&str>) -> Option<char> {
    text_input_of(logical_key, text).and_then(|value| {
        let mut chars = value.chars();
        let first = chars.next()?;
        if chars.next().is_some() {
            None
        } else {
            Some(first)
        }
    })
}

fn text_input_of(logical_key: &GNativeInputKey, text: Option<&str>) -> Option<String> {
    match logical_key {
        GNativeInputKey::Character(value) => Some(value.clone()),
        GNativeInputKey::Unidentified => text.map(ToOwned::to_owned),
        GNativeInputKey::Named(_) => None,
    }
}

fn clamped_size(columns: u32, rows: u32) -> GridSize {
    GridSize::new(
        u16::try_from(columns).unwrap_or(u16::MAX).max(1),
        u16::try_from(rows).unwrap_or(u16::MAX).max(1),
    )
}

#[derive(Debug)]
enum SessionMessage {
    Input(GNativeInputEvent),
    Closed,
    Error(GNativeSdkError),
}

struct DemoFrameEmitter {
    gshell_id: GShellId,
    frame_seq: u64,
    writer: LocalGNativeFrameWriter,
    last_compiled: Option<CompiledUi>,
}

impl DemoFrameEmitter {
    fn new(gshell_id: GShellId, writer: LocalGNativeFrameWriter) -> Self {
        Self {
            gshell_id,
            frame_seq: 0,
            writer,
            last_compiled: None,
        }
    }

    fn send(&mut self, compiled: CompiledUi) -> Result<bool, DemoError> {
        let Some((commands, cursor)) = frame_delta(self.last_compiled.as_ref(), &compiled) else {
            return Ok(false);
        };
        self.frame_seq += 1;
        self.writer.send_frame(GNativeFrame {
            gshell_id: self.gshell_id,
            seq: Seq::new(self.frame_seq),
            commands,
            cursor,
        })?;
        self.last_compiled = Some(compiled);
        Ok(true)
    }
}

fn frame_delta(
    previous: Option<&CompiledUi>,
    current: &CompiledUi,
) -> Option<(Vec<RenderCommandDto>, Option<GNativeFrameCursor>)> {
    let commands = diff_compiled_commands(previous, current);
    let cursor = current.cursor.map(|cursor| GNativeFrameCursor {
        x: cursor.x,
        y: cursor.y,
    });
    let previous_cursor =
        previous
            .and_then(|previous| previous.cursor)
            .map(|cursor| GNativeFrameCursor {
                x: cursor.x,
                y: cursor.y,
            });
    if commands.is_empty() && cursor == previous_cursor {
        return None;
    }
    Some((commands, cursor))
}

fn diff_compiled_commands(
    previous: Option<&CompiledUi>,
    current: &CompiledUi,
) -> Vec<RenderCommandDto> {
    let Some(previous) = previous else {
        return current.commands.clone();
    };
    let Some(previous_frame) = FullUiFrame::from_compiled(previous) else {
        return current.commands.clone();
    };
    let Some(current_frame) = FullUiFrame::from_compiled(current) else {
        return current.commands.clone();
    };

    if previous_frame.structural_commands != current_frame.structural_commands {
        return current.commands.clone();
    }

    let changed_rows = previous_frame.changed_rows_against(&current_frame);
    if changed_rows.is_empty() {
        return Vec::new();
    }

    let mut commands = Vec::new();
    for row in changed_rows {
        commands.push(RenderCommandDto::ClearLine { y: row });
        if let Some(row_commands) = current_frame.rows.get(&row) {
            commands.extend(row_commands.iter().cloned());
        }
    }

    commands
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FullUiFrame {
    structural_commands: Vec<RenderCommandDto>,
    rows: BTreeMap<u32, Vec<RenderCommandDto>>,
}

impl FullUiFrame {
    fn from_compiled(compiled: &CompiledUi) -> Option<Self> {
        let mut commands = compiled.commands.iter();
        if matches!(commands.next(), Some(RenderCommandDto::Clear)) {
        } else {
            return None;
        }

        let mut structural_commands = Vec::new();
        let mut rows = BTreeMap::<u32, Vec<RenderCommandDto>>::new();
        for command in commands {
            match command {
                RenderCommandDto::TextRun { y, .. } | RenderCommandDto::StyledTextRun { y, .. } => {
                    rows.entry(*y).or_default().push(command.clone());
                }
                RenderCommandDto::PixelFillRect { .. }
                | RenderCommandDto::PngSurface { .. }
                | RenderCommandDto::SharedRgbaSurface { .. } => {
                    structural_commands.push(command.clone());
                }
                RenderCommandDto::Clear | RenderCommandDto::ClearLine { .. } => return None,
            }
        }

        Some(Self {
            structural_commands,
            rows,
        })
    }

    fn changed_rows_against(&self, other: &Self) -> BTreeSet<u32> {
        let mut rows = BTreeSet::new();
        rows.extend(self.rows.keys().copied());
        rows.extend(other.rows.keys().copied());
        rows.into_iter()
            .filter(|row| self.rows.get(row) != other.rows.get(row))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DemoId {
    Todo,
}

impl DemoId {
    const ALL: [DemoId; 1] = [DemoId::Todo];

    fn title(self) -> &'static str {
        match self {
            DemoId::Todo => "Todo",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            DemoId::Todo => "Keyboard-first form and list interactions",
        }
    }
}

#[derive(Debug, Clone)]
struct DemoRender {
    title: &'static str,
    help: &'static str,
    content: Element,
    overlay: Option<Element>,
}

struct DemoHostApp {
    size: GridSize,
    switcher: DemoSwitcherState,
    active_demo: ActiveDemo,
    notice: Option<String>,
    should_quit: bool,
}

impl DemoHostApp {
    fn new(size: GridSize) -> Self {
        Self {
            size,
            switcher: DemoSwitcherState::new(DemoId::Todo),
            active_demo: ActiveDemo::new(DemoId::Todo),
            notice: Some("F1 opens demo list. j/k switches. space confirms.".to_string()),
            should_quit: false,
        }
    }

    fn resize(&mut self, size: GridSize) {
        self.size = size;
        self.active_demo.resize(size);
    }

    fn push_notice(&mut self, notice: String) {
        self.notice = Some(notice);
    }

    fn toggle_switcher(&mut self) {
        self.switcher.open = !self.switcher.open;
    }

    fn close_switcher(&mut self) {
        self.switcher.open = false;
    }

    fn switcher_open(&self) -> bool {
        self.switcher.open
    }

    fn select_next_demo(&mut self) {
        self.switcher.select_next();
    }

    fn select_previous_demo(&mut self) {
        self.switcher.select_previous();
    }

    fn activate_selected_demo(&mut self) {
        let selected = self.switcher.selected_demo();
        if self.active_demo.id() != selected {
            self.active_demo = ActiveDemo::new(selected);
            self.active_demo.resize(self.size);
            self.notice = Some(format!("loaded demo: {}", selected.title()));
        }
        self.switcher.open = false;
    }

    fn handle_demo_key(
        &mut self,
        logical_key: GNativeInputKey,
        text: Option<&str>,
        modifiers: GNativeInputModifiers,
    ) {
        let demo_notice = self.active_demo.handle_key(logical_key, text, modifiers);
        if let Some(notice) = demo_notice {
            self.notice = Some(notice);
        }
    }

    fn paste(&mut self, text: &str) {
        let demo_notice = self.active_demo.paste(text);
        if let Some(notice) = demo_notice {
            self.notice = Some(notice);
        }
    }

    fn ui_tree(&self) -> UiTree {
        let render = self.active_demo.render();

        let mut root = div().size_full().bg(rgba(6, 9, 20, 255)).child(
            v_flex()
                .size_full()
                .child(self.status_panel(render.title, render.help))
                .child(div().flex_1().child(render.content)),
        );

        if let Some(overlay) = render.overlay {
            root = root.child(overlay);
        }
        if self.switcher.open {
            root = root.child(self.switcher_overlay());
        }

        UiTree::new(root)
    }

    fn status_panel(&self, title: &'static str, help: &'static str) -> Element {
        let notice = self.notice.as_deref().unwrap_or(" ");
        GroupBox::new()
            .id("demo-host-status")
            .outline()
            .fill()
            .title("Demo Host")
            .child(
                Label::new(format!(
                    "demo={} viewport={}x{}",
                    title, self.size.columns, self.size.rows
                ))
                .font_semibold()
                .text_color(rgb(80, 220, 255)),
            )
            .child(Label::new(
                "F1 demos  j/k navigate  space confirm or play/pause  Ctrl+C quit",
            ))
            .child(Label::new(help).secondary(notice))
            .into_element()
    }

    fn switcher_overlay(&self) -> Element {
        let mut entries = v_flex().gap_1();
        for (index, demo_id) in DemoId::ALL.iter().copied().enumerate() {
            let selected = index == self.switcher.selected_index;
            let mut row = v_flex()
                .child(
                    Label::new(format!(
                        "{} {}",
                        if selected { ">" } else { " " },
                        demo_id.title()
                    ))
                    .font_semibold()
                    .text_color(if selected {
                        rgb(255, 214, 92)
                    } else {
                        rgb(226, 230, 238)
                    }),
                )
                .child(Label::new(demo_id.subtitle()).secondary("space loads"));
            if selected {
                row = row.bg(rgba(24, 42, 88, 220));
            }
            entries = entries.child(row);
        }

        div()
            .size_full()
            .bg(rgba(3, 5, 12, 220))
            .child(
                v_flex()
                    .size_full()
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .size_full()
                            .child(div().flex_1())
                            .child(
                                div().w(px(84.0)).child(
                                    GroupBox::new()
                                        .id("demo-switcher")
                                        .outline()
                                        .fill()
                                        .title("Switch Demo")
                                        .child(Label::new(
                                            "j/k switch selection, space confirm, F1 or Esc closes",
                                        ))
                                        .child(entries),
                                ),
                            )
                            .child(div().flex_1()),
                    )
                    .child(div().flex_1()),
            )
            .into_element()
    }
}

#[derive(Debug, Clone, Copy)]
struct DemoSwitcherState {
    open: bool,
    selected_index: usize,
}

impl DemoSwitcherState {
    fn new(active: DemoId) -> Self {
        let selected_index = DemoId::ALL
            .iter()
            .position(|candidate| *candidate == active)
            .unwrap_or_default();
        Self {
            open: false,
            selected_index,
        }
    }

    fn selected_demo(&self) -> DemoId {
        DemoId::ALL[self.selected_index]
    }

    fn select_next(&mut self) {
        if self.selected_index + 1 < DemoId::ALL.len() {
            self.selected_index += 1;
        }
    }

    fn select_previous(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }
}

enum ActiveDemo {
    Todo(TodoDemo),
}

impl ActiveDemo {
    fn new(id: DemoId) -> Self {
        match id {
            DemoId::Todo => Self::Todo(TodoDemo::new()),
        }
    }

    fn id(&self) -> DemoId {
        match self {
            Self::Todo(_) => DemoId::Todo,
        }
    }

    fn resize(&mut self, size: GridSize) {
        match self {
            Self::Todo(demo) => demo.resize(size),
        }
    }

    fn render(&self) -> DemoRender {
        match self {
            Self::Todo(demo) => demo.render(),
        }
    }

    fn handle_key(
        &mut self,
        logical_key: GNativeInputKey,
        text: Option<&str>,
        modifiers: GNativeInputModifiers,
    ) -> Option<String> {
        match self {
            Self::Todo(demo) => demo.handle_key(logical_key, text, modifiers),
        }
    }

    fn paste(&mut self, text: &str) -> Option<String> {
        match self {
            Self::Todo(demo) => demo.paste(text),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TodoFocusArea {
    List,
    Activity,
}

#[derive(Debug, Clone)]
struct TodoItem {
    title: String,
    done: bool,
}

struct TodoDemo {
    size: GridSize,
    composer: InputState,
    todos: Vec<TodoItem>,
    selected_task: usize,
    focus: TodoFocusArea,
    editing: Option<usize>,
    events: VecDeque<String>,
}

impl TodoDemo {
    fn new() -> Self {
        let mut composer = InputState::new("todo-composer")
            .placeholder("Type a task title")
            .clean_on_escape();
        composer.set_focused(false);

        let mut demo = Self {
            size: GridSize::new(1, 1),
            composer,
            todos: vec![
                TodoItem {
                    title: "Port gpui-component surface API".to_string(),
                    done: true,
                },
                TodoItem {
                    title: "Build keyboard-first todo interactions".to_string(),
                    done: false,
                },
                TodoItem {
                    title: "Split primitives and widgets crates".to_string(),
                    done: false,
                },
            ],
            selected_task: 0,
            focus: TodoFocusArea::List,
            editing: None,
            events: VecDeque::new(),
        };
        demo.push_event("1/2 switch panels, j/k move, n new, e edit, d delete".to_string());
        demo
    }

    fn resize(&mut self, size: GridSize) {
        self.size = size;
    }

    fn render(&self) -> DemoRender {
        let mut root = h_flex()
            .gap_1()
            .flex_1()
            .child(div().flex_1().child(self.todo_panel()))
            .child(div().flex_1().child(self.activity_panel()));

        if self.composer.is_focused() {
            root = root.child(div());
        }

        DemoRender {
            title: "Todo",
            help: "1/2 focus panels. n new. e edit. d delete. space toggles selected todo.",
            content: root.into_element(),
            overlay: self.composer.is_focused().then(|| self.dialog_overlay()),
        }
    }

    fn handle_key(
        &mut self,
        logical_key: GNativeInputKey,
        text: Option<&str>,
        modifiers: GNativeInputModifiers,
    ) -> Option<String> {
        if self.composer.is_focused() {
            match logical_key {
                GNativeInputKey::Named(GNativeInputNamedKey::ArrowLeft) => {
                    self.composer.move_cursor_left()
                }
                GNativeInputKey::Named(GNativeInputNamedKey::ArrowRight) => {
                    self.composer.move_cursor_right()
                }
                GNativeInputKey::Named(GNativeInputNamedKey::Backspace) => self.backspace(),
                GNativeInputKey::Named(GNativeInputNamedKey::Escape) => self.escape(),
                GNativeInputKey::Named(GNativeInputNamedKey::Enter) => self.submit_composer(),
                _ if !modifiers.control && !modifiers.alt => {
                    if let Some(value) = text_input_of(&logical_key, text) {
                        self.insert_text(&value);
                    }
                }
                _ => {}
            }
            return self.events.front().cloned();
        }

        match logical_key {
            GNativeInputKey::Named(GNativeInputNamedKey::Tab) => self.next_focus(),
            GNativeInputKey::Named(GNativeInputNamedKey::ArrowUp) => self.move_up(),
            GNativeInputKey::Named(GNativeInputNamedKey::ArrowDown) => self.move_down(),
            GNativeInputKey::Named(GNativeInputNamedKey::Enter) => self.submit_composer(),
            GNativeInputKey::Named(GNativeInputNamedKey::Escape) => self.escape(),
            _ if !modifiers.control && !modifiers.alt => {
                if let Some(ch) = character_of(&logical_key, text) {
                    match ch {
                        '1' => self.focus = TodoFocusArea::List,
                        '2' => self.focus = TodoFocusArea::Activity,
                        'j' => self.move_down(),
                        'k' => self.move_up(),
                        ' ' => self.toggle_selected(),
                        'n' => self.prepare_new(),
                        'e' => self.start_edit_selected(),
                        'd' => self.delete_selected(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        self.events.front().cloned()
    }

    fn paste(&mut self, text: &str) -> Option<String> {
        if self.composer.is_focused() {
            self.insert_text(text);
            self.push_event(format!("pasted {} chars", text.chars().count()));
        }
        self.events.front().cloned()
    }

    fn push_event(&mut self, event: String) {
        self.events.push_front(event);
        if self.events.len() > MAX_TODO_EVENTS {
            self.events.pop_back();
        }
    }

    fn next_focus(&mut self) {
        self.focus = match self.focus {
            TodoFocusArea::List => TodoFocusArea::Activity,
            TodoFocusArea::Activity => TodoFocusArea::List,
        };
    }

    fn move_up(&mut self) {
        if self.focus == TodoFocusArea::List && self.selected_task > 0 {
            self.selected_task -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.focus == TodoFocusArea::List && self.selected_task + 1 < self.todos.len() {
            self.selected_task += 1;
        }
    }

    fn insert_text(&mut self, text: &str) {
        if self.composer.is_focused() {
            self.composer.insert_text(text);
        }
    }

    fn backspace(&mut self) {
        if self.composer.is_focused() {
            self.composer.backspace();
        }
    }

    fn escape(&mut self) {
        if !self.composer.is_focused() {
            return;
        }

        if self.editing.is_some() {
            self.editing = None;
            self.composer.clear();
            self.push_event("edit dialog canceled".to_string());
        } else if self.composer.clean_on_escape_enabled() && !self.composer.value().is_empty() {
            self.composer.clear();
            self.push_event("new task dialog cleared".to_string());
        }

        self.composer.set_focused(false);
        self.focus = TodoFocusArea::List;
    }

    fn prepare_new(&mut self) {
        self.editing = None;
        self.composer.clear();
        self.composer.set_focused(true);
        self.push_event("new task dialog opened".to_string());
    }

    fn submit_composer(&mut self) {
        if !self.composer.is_focused() {
            return;
        }

        let title = self.composer.value().trim().to_string();
        if title.is_empty() {
            self.push_event("ignored empty task".to_string());
            return;
        }

        if let Some(index) = self.editing.take() {
            if let Some(item) = self.todos.get_mut(index) {
                item.title = title.clone();
                self.selected_task = index;
                self.push_event(format!("updated task {}", index + 1));
            }
        } else {
            self.todos.push(TodoItem {
                title: title.clone(),
                done: false,
            });
            self.selected_task = self.todos.len().saturating_sub(1);
            self.push_event(format!("added task: {title}"));
        }

        self.composer.clear();
        self.composer.set_focused(false);
        self.focus = TodoFocusArea::List;
    }

    fn toggle_selected(&mut self) {
        if let Some(item) = self.todos.get_mut(self.selected_task) {
            item.done = !item.done;
            let state = if item.done { "completed" } else { "reopened" };
            self.push_event(format!("task {} {state}", self.selected_task + 1));
        }
    }

    fn start_edit_selected(&mut self) {
        if let Some(item) = self.todos.get(self.selected_task) {
            self.editing = Some(self.selected_task);
            self.composer.set_value(item.title.clone());
            self.composer.set_focused(true);
            self.push_event(format!(
                "edit dialog opened for task {}",
                self.selected_task + 1
            ));
        }
    }

    fn delete_selected(&mut self) {
        if self.todos.is_empty() {
            self.push_event("no task to delete".to_string());
            return;
        }

        let removed = self.todos.remove(self.selected_task);
        self.push_event(format!("deleted task: {}", removed.title));

        if let Some(editing) = self.editing {
            if editing == self.selected_task {
                self.editing = None;
                self.composer.clear();
                self.composer.set_focused(false);
            } else if editing > self.selected_task {
                self.editing = Some(editing - 1);
            }
        }

        if self.todos.is_empty() {
            self.selected_task = 0;
        } else {
            self.selected_task = self.selected_task.min(self.todos.len() - 1);
        }
    }

    fn completed_count(&self) -> usize {
        self.todos.iter().filter(|item| item.done).count()
    }

    fn focus_name(&self) -> &'static str {
        match self.focus {
            TodoFocusArea::List => "list",
            TodoFocusArea::Activity => "activity",
        }
    }

    fn todo_panel(&self) -> Element {
        let mut panel = GroupBox::new()
            .id("todo-list")
            .outline()
            .fill()
            .title("Todo List")
            .child(
                Label::new(format!(
                    "focus={} tasks={} done={}",
                    self.focus_name(),
                    self.todos.len(),
                    self.completed_count()
                ))
                .secondary("n new  e edit  d delete"),
            );
        if self.todos.is_empty() {
            panel = panel.child(
                Label::new("No tasks yet").secondary("Press n to open the new-task dialog."),
            );
        } else {
            let mut items = v_flex().gap_1();
            for (index, item) in self.todos.iter().enumerate() {
                items = items.child(self.todo_row(index, item));
            }
            panel = panel.child(items);
        }
        panel.into_element()
    }

    fn todo_row(&self, index: usize, item: &TodoItem) -> Element {
        let mut line = h_flex()
            .gap_1()
            .child(
                Label::new(
                    if index == self.selected_task && self.focus == TodoFocusArea::List {
                        ">"
                    } else {
                        " "
                    },
                )
                .text_color(rgb(80, 220, 255)),
            )
            .child(
                Checkbox::new(format!("todo-{index}"))
                    .label(item.title.clone())
                    .checked(item.done),
            );
        if self.editing == Some(index) {
            line = line.child(Label::new("editing").secondary("Enter saves"));
        }
        if index == self.selected_task {
            line = line.bg(rgba(22, 36, 72, 220));
        }
        line.into_element()
    }

    fn activity_panel(&self) -> Element {
        GroupBox::new()
            .id("todo-activity")
            .outline()
            .fill()
            .title("Activity")
            .child(
                Label::new("Recent events")
                    .font_semibold()
                    .text_color(rgb(80, 220, 255)),
            )
            .child(div().child(self.activity_text()))
            .into_element()
    }

    fn activity_text(&self) -> String {
        if self.events.is_empty() {
            "waiting for interaction".to_string()
        } else {
            self.events.iter().cloned().collect::<Vec<_>>().join("\n")
        }
    }

    fn dialog_overlay(&self) -> Element {
        div()
			.size_full()
			.bg(rgba(3, 5, 12, 220))
			.child(
				v_flex()
					.size_full()
					.child(div().flex_1())
					.child(
						h_flex()
							.size_full()
							.child(div().flex_1())
							.child(
								div().w(px(76.0)).child(
									GroupBox::new()
										.id("todo-dialog")
										.outline()
										.fill()
										.title(if self.editing.is_some() { "Edit Task" } else { "New Task" })
										.child(Label::new(
											"Type task title, Enter saves, Esc cancels. F1 is reserved for demo \
											 switching.",
										))
										.child(Input::new(&self.composer)),
								),
							)
							.child(div().flex_1()),
					)
					.child(div().flex_1()),
			)
			.into_element()
    }
}

#[cfg(test)]
mod tests {
    use germinal_gnative_protocol::rendering::frame_plan_builder::RenderCommandDto;
    use germinal_gnative_ui::CompiledUi;

    use super::{diff_compiled_commands, frame_delta};

    #[test]
    fn compiled_ui_diff_emits_only_changed_rows_when_structure_matches() {
        let previous = compiled_ui(vec![
            RenderCommandDto::Clear,
            RenderCommandDto::PixelFillRect {
                x_px: 0,
                y_px: 0,
                width_px: 10,
                height_px: 10,
                color: germinal_gnative_ui::rgba(1, 2, 3, 4),
            },
            RenderCommandDto::TextRun {
                x: 0,
                y: 0,
                text: "alpha".to_string(),
            },
            RenderCommandDto::TextRun {
                x: 0,
                y: 1,
                text: "beta".to_string(),
            },
        ]);
        let current = compiled_ui(vec![
            RenderCommandDto::Clear,
            RenderCommandDto::PixelFillRect {
                x_px: 0,
                y_px: 0,
                width_px: 10,
                height_px: 10,
                color: germinal_gnative_ui::rgba(1, 2, 3, 4),
            },
            RenderCommandDto::TextRun {
                x: 0,
                y: 0,
                text: "alpha".to_string(),
            },
            RenderCommandDto::TextRun {
                x: 0,
                y: 1,
                text: "gamma".to_string(),
            },
        ]);

        assert_eq!(
            diff_compiled_commands(Some(&previous), &current),
            vec![
                RenderCommandDto::ClearLine { y: 1 },
                RenderCommandDto::TextRun {
                    x: 0,
                    y: 1,
                    text: "gamma".to_string()
                },
            ]
        );
    }

    #[test]
    fn compiled_ui_diff_falls_back_to_full_frame_when_structure_changes() {
        let previous = compiled_ui(vec![
            RenderCommandDto::Clear,
            RenderCommandDto::PixelFillRect {
                x_px: 0,
                y_px: 0,
                width_px: 10,
                height_px: 10,
                color: germinal_gnative_ui::rgba(1, 2, 3, 4),
            },
            RenderCommandDto::TextRun {
                x: 0,
                y: 0,
                text: "alpha".to_string(),
            },
        ]);
        let current = compiled_ui(vec![
            RenderCommandDto::Clear,
            RenderCommandDto::PixelFillRect {
                x_px: 1,
                y_px: 0,
                width_px: 10,
                height_px: 10,
                color: germinal_gnative_ui::rgba(1, 2, 3, 4),
            },
            RenderCommandDto::TextRun {
                x: 0,
                y: 0,
                text: "alpha".to_string(),
            },
        ]);

        assert_eq!(
            diff_compiled_commands(Some(&previous), &current),
            current.commands
        );
    }

    #[test]
    fn frame_delta_returns_none_when_ui_and_cursor_are_unchanged() {
        let compiled = compiled_ui(vec![
            RenderCommandDto::Clear,
            RenderCommandDto::TextRun {
                x: 0,
                y: 0,
                text: "stable".to_string(),
            },
        ]);

        assert_eq!(frame_delta(None, &compiled).unwrap().0, compiled.commands);
        assert!(frame_delta(Some(&compiled), &compiled).is_none());
    }

    fn compiled_ui(commands: Vec<RenderCommandDto>) -> CompiledUi {
        CompiledUi {
            commands,
            cursor: None,
        }
    }
}
