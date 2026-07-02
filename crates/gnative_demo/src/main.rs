use std::{
	collections::VecDeque,
	fs,
	io::stdout,
	path::{Path, PathBuf},
	sync::mpsc::{self, RecvTimeoutError},
	thread,
	time::{Duration, Instant},
};

use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::{
	gnative::{
		frame::{GNativeFrame, GNativeFrameCursor},
		input::{
			GNativeInputElementState, GNativeInputEvent, GNativeInputKey, GNativeInputModifiers,
			GNativeInputNamedKey,
		},
		media::GNativeMediaControlCommand,
	},
	seq::Seq,
};
use germinal_gnative_sdk::local_session::{
	LocalGNativeFrameWriter, LocalGNativeSession, LocalGNativeTunnelBootstrap,
};
use germinal_gnative_ui::{
	CompiledUi, Element, GridSize, IntoElementNode, UiTree,
	elements::div::{div, h_flex, v_flex},
	px, rgb, rgba, video,
};
use germinal_gnative_widgets::{
	checkbox::Checkbox,
	group_box::GroupBox,
	input::{Input, InputState},
	label::Label,
};

const FPS_WINDOW: Duration = Duration::from_secs(1);
const MAX_TODO_EVENTS: usize = 10;
const VIDEO_SURFACE_ID: &str = "video-player-surface";
const VIDEO_SEEK_STEP_US: u64 = 1_000_000;
const VIDEO_EXTENSIONS: &[&str] =
	&["mp4", "mkv", "mov", "webm", "avi", "m4v", "ts", "mpeg", "mpg", "flv", "wmv"];

fn main() -> Result<(), String> {
	let bootstrap = LocalGNativeTunnelBootstrap::from_env()?;
	eprintln!("germinal-gnative-demo connecting to germinal at {}", bootstrap.tunnel_env().endpoint);
	let mut terminal_stdout = stdout();
	bootstrap
		.write_enter_control_sequence(&mut terminal_stdout)
		.map_err(|error| error.to_string())?;

	let mut session = bootstrap.connect()?;
	let accepted = session.accepted().clone();
	let initial_size = wait_for_initial_size(&mut session)?;
	let mut emitter = DemoFrameEmitter::new(accepted.gshell_id, session.frame_writer()?);
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
				if !handle_session_message(&mut app, &mut emitter, message) {
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

fn handle_session_message(
	app: &mut DemoHostApp,
	emitter: &mut DemoFrameEmitter,
	message: SessionMessage,
) -> bool {
	match message {
		SessionMessage::Input(input) => {
			handle_input(app, input);
			flush_media_commands(app, emitter);
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

fn flush_media_commands(app: &mut DemoHostApp, emitter: &mut DemoFrameEmitter) {
	for command in app.drain_media_commands() {
		if let Err(error) = emitter.send_control(command) {
			app.push_notice(format!("failed to send media control: {error}"));
			app.should_quit = true;
			return;
		}
	}
}

fn draw_app(app: &mut DemoHostApp, emitter: &mut DemoFrameEmitter) -> Result<(), String> {
	let layout = app.ui_tree().layout(app.size);
	let compiled = layout.render();
	emitter.send(compiled)?;
	app.record_presented_frame(Instant::now());
	Ok(())
}

fn wait_for_initial_size(session: &mut LocalGNativeSession) -> Result<GridSize, String> {
	while let Some(input) = session.read_input()? {
		if let GNativeInputEvent::Resize { columns, rows } = input {
			return Ok(clamped_size(columns, rows));
		}
	}
	Err("host closed before initial resize".to_string())
}

fn handle_input(app: &mut DemoHostApp, input: GNativeInputEvent) {
	match input {
		GNativeInputEvent::Resize { columns, rows } => {
			let size = clamped_size(columns, rows);
			app.resize(size);
			app.push_notice(format!("resize {}x{}", columns, rows));
		}
		GNativeInputEvent::Paste(text) | GNativeInputEvent::Ime(text) => app.paste(&text),
		GNativeInputEvent::Bytes(bytes) => {
			app.push_notice(format!("unsupported input: bytes {:?}", bytes));
		}
		GNativeInputEvent::Key { state, logical_key, text, modifiers } => {
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

	if matches!(logical_key, GNativeInputKey::Named(GNativeInputNamedKey::F1)) {
		app.toggle_switcher();
		return;
	}

	if app.switcher_open() {
		match logical_key {
			GNativeInputKey::Named(GNativeInputNamedKey::ArrowUp) => app.select_previous_demo(),
			GNativeInputKey::Named(GNativeInputNamedKey::ArrowDown) => app.select_next_demo(),
			GNativeInputKey::Named(GNativeInputNamedKey::Enter)
			| GNativeInputKey::Named(GNativeInputNamedKey::Escape) => {
				if matches!(logical_key, GNativeInputKey::Named(GNativeInputNamedKey::Enter)) {
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
		if chars.next().is_some() { None } else { Some(first) }
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
	Error(String),
}

struct DemoFrameEmitter {
	gshell_id: GShellId,
	frame_seq: u64,
	writer:    LocalGNativeFrameWriter,
}

impl DemoFrameEmitter {
	fn new(gshell_id: GShellId, writer: LocalGNativeFrameWriter) -> Self {
		Self { gshell_id, frame_seq: 0, writer }
	}

	fn send(&mut self, compiled: CompiledUi) -> Result<(), String> {
		self.frame_seq += 1;
		self.writer.send_frame(GNativeFrame {
			gshell_id: self.gshell_id,
			seq:       Seq::new(self.frame_seq),
			commands:  compiled.commands,
			cursor:    compiled.cursor.map(|cursor| GNativeFrameCursor { x: cursor.x, y: cursor.y }),
		})
	}

	fn send_control(&mut self, command: GNativeMediaControlCommand) -> Result<(), String> {
		self.writer.send_control(command)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DemoId {
	Todo,
	VideoPlayer,
}

impl DemoId {
	const ALL: [DemoId; 2] = [DemoId::Todo, DemoId::VideoPlayer];

	fn title(self) -> &'static str {
		match self {
			DemoId::Todo => "Todo",
			DemoId::VideoPlayer => "Video Player",
		}
	}

	fn subtitle(self) -> &'static str {
		match self {
			DemoId::Todo => "Keyboard-first form and list interactions",
			DemoId::VideoPlayer => "Directory-backed video browser with dedicated Video element",
		}
	}
}

#[derive(Debug, Clone)]
struct DemoRender {
	title:   &'static str,
	help:    &'static str,
	content: Element,
	overlay: Option<Element>,
}

struct DemoHostApp {
	size:             GridSize,
	switcher:         DemoSwitcherState,
	active_demo:      ActiveDemo,
	pending_controls: Vec<GNativeMediaControlCommand>,
	notice:           Option<String>,
	should_quit:      bool,
	frame_count:      u64,
	fps:              f32,
	presented_frames: VecDeque<Instant>,
}

impl DemoHostApp {
	fn new(size: GridSize) -> Self {
		Self {
			size,
			switcher: DemoSwitcherState::new(DemoId::Todo),
			active_demo: ActiveDemo::new(DemoId::Todo),
			pending_controls: Vec::new(),
			notice: Some("F1 opens demo list. j/k switches. space confirms.".to_string()),
			should_quit: false,
			frame_count: 0,
			fps: 0.0,
			presented_frames: VecDeque::new(),
		}
	}

	fn resize(&mut self, size: GridSize) {
		self.size = size;
		self.active_demo.resize(size);
	}

	fn push_notice(&mut self, notice: String) { self.notice = Some(notice); }

	fn record_presented_frame(&mut self, now: Instant) {
		self.frame_count += 1;
		self.presented_frames.push_back(now);
		while let Some(oldest) = self.presented_frames.front().copied() {
			if now.saturating_duration_since(oldest) <= FPS_WINDOW {
				break;
			}
			self.presented_frames.pop_front();
		}
		self.fps = self.presented_frames.len() as f32 / FPS_WINDOW.as_secs_f32();
	}

	fn toggle_switcher(&mut self) { self.switcher.open = !self.switcher.open; }

	fn close_switcher(&mut self) { self.switcher.open = false; }

	fn switcher_open(&self) -> bool { self.switcher.open }

	fn select_next_demo(&mut self) { self.switcher.select_next(); }

	fn select_previous_demo(&mut self) { self.switcher.select_previous(); }

	fn activate_selected_demo(&mut self) {
		let selected = self.switcher.selected_demo();
		if self.active_demo.id() != selected {
			self.pending_controls.extend(self.active_demo.shutdown_controls());
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
		self.pending_controls.extend(self.active_demo.drain_controls());
		if let Some(notice) = demo_notice {
			self.notice = Some(notice);
		}
	}

	fn paste(&mut self, text: &str) {
		let demo_notice = self.active_demo.paste(text);
		self.pending_controls.extend(self.active_demo.drain_controls());
		if let Some(notice) = demo_notice {
			self.notice = Some(notice);
		}
	}

	fn drain_media_commands(&mut self) -> Vec<GNativeMediaControlCommand> {
		std::mem::take(&mut self.pending_controls)
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
					"demo={} viewport={}x{} frame={} fps={:.1}",
					title, self.size.columns, self.size.rows, self.frame_count, self.fps
				))
				.font_semibold()
				.text_color(rgb(80, 220, 255)),
			)
			.child(Label::new("F1 demos  j/k navigate  space confirm or play/pause  Ctrl+C quit"))
			.child(Label::new(help).secondary(notice))
			.into_element()
	}

	fn switcher_overlay(&self) -> Element {
		let mut entries = v_flex().gap_1();
		for (index, demo_id) in DemoId::ALL.iter().copied().enumerate() {
			let selected = index == self.switcher.selected_index;
			let mut row = v_flex()
				.child(
					Label::new(format!("{} {}", if selected { ">" } else { " " }, demo_id.title()))
						.font_semibold()
						.text_color(if selected { rgb(255, 214, 92) } else { rgb(226, 230, 238) }),
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
										.child(Label::new("j/k switch selection, space confirm, F1 or Esc closes"))
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
	open:           bool,
	selected_index: usize,
}

impl DemoSwitcherState {
	fn new(active: DemoId) -> Self {
		let selected_index =
			DemoId::ALL.iter().position(|candidate| *candidate == active).unwrap_or_default();
		Self { open: false, selected_index }
	}

	fn selected_demo(&self) -> DemoId { DemoId::ALL[self.selected_index] }

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
	VideoPlayer(VideoPlayerDemo),
}

impl ActiveDemo {
	fn new(id: DemoId) -> Self {
		match id {
			DemoId::Todo => Self::Todo(TodoDemo::new()),
			DemoId::VideoPlayer => Self::VideoPlayer(VideoPlayerDemo::new()),
		}
	}

	fn id(&self) -> DemoId {
		match self {
			Self::Todo(_) => DemoId::Todo,
			Self::VideoPlayer(_) => DemoId::VideoPlayer,
		}
	}

	fn resize(&mut self, size: GridSize) {
		match self {
			Self::Todo(demo) => demo.resize(size),
			Self::VideoPlayer(demo) => demo.resize(size),
		}
	}

	fn render(&self) -> DemoRender {
		match self {
			Self::Todo(demo) => demo.render(),
			Self::VideoPlayer(demo) => demo.render(),
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
			Self::VideoPlayer(demo) => demo.handle_key(logical_key, text, modifiers),
		}
	}

	fn paste(&mut self, text: &str) -> Option<String> {
		match self {
			Self::Todo(demo) => demo.paste(text),
			Self::VideoPlayer(demo) => demo.paste(text),
		}
	}

	fn drain_controls(&mut self) -> Vec<GNativeMediaControlCommand> {
		match self {
			Self::Todo(_) => Vec::new(),
			Self::VideoPlayer(demo) => demo.drain_controls(),
		}
	}

	fn shutdown_controls(&mut self) -> Vec<GNativeMediaControlCommand> {
		match self {
			Self::Todo(_) => Vec::new(),
			Self::VideoPlayer(demo) => demo.shutdown_controls(),
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
	done:  bool,
}

struct TodoDemo {
	size:          GridSize,
	composer:      InputState,
	todos:         Vec<TodoItem>,
	selected_task: usize,
	focus:         TodoFocusArea,
	editing:       Option<usize>,
	events:        VecDeque<String>,
}

impl TodoDemo {
	fn new() -> Self {
		let mut composer =
			InputState::new("todo-composer").placeholder("Type a task title").clean_on_escape();
		composer.set_focused(false);

		let mut demo = Self {
			size: GridSize::new(1, 1),
			composer,
			todos: vec![
				TodoItem { title: "Port gpui-component surface API".to_string(), done: true },
				TodoItem { title: "Build keyboard-first todo interactions".to_string(), done: false },
				TodoItem { title: "Split primitives and widgets crates".to_string(), done: false },
			],
			selected_task: 0,
			focus: TodoFocusArea::List,
			editing: None,
			events: VecDeque::new(),
		};
		demo.push_event("1/2 switch panels, j/k move, n new, e edit, d delete".to_string());
		demo
	}

	fn resize(&mut self, size: GridSize) { self.size = size; }

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
			title:   "Todo",
			help:    "1/2 focus panels. n new. e edit. d delete. space toggles selected todo.",
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
			let mut value = self.composer.value().to_string();
			value.push_str(text);
			self.composer.set_value(value);
		}
	}

	fn backspace(&mut self) {
		if self.composer.is_focused() {
			let mut value = self.composer.value().to_string();
			value.pop();
			self.composer.set_value(value);
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
			self.todos.push(TodoItem { title: title.clone(), done: false });
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
			self.push_event(format!("edit dialog opened for task {}", self.selected_task + 1));
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

	fn completed_count(&self) -> usize { self.todos.iter().filter(|item| item.done).count() }

	fn focus_name(&self) -> &'static str {
		match self.focus {
			TodoFocusArea::List => "list",
			TodoFocusArea::Activity => "activity",
		}
	}

	fn todo_panel(&self) -> Element {
		let mut panel = GroupBox::new().id("todo-list").outline().fill().title("Todo List").child(
			Label::new(format!(
				"focus={} tasks={} done={}",
				self.focus_name(),
				self.todos.len(),
				self.completed_count()
			))
			.secondary("n new  e edit  d delete"),
		);
		if self.todos.is_empty() {
			panel =
				panel.child(Label::new("No tasks yet").secondary("Press n to open the new-task dialog."));
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
				Label::new(if index == self.selected_task && self.focus == TodoFocusArea::List {
					">"
				} else {
					" "
				})
				.text_color(rgb(80, 220, 255)),
			)
			.child(Checkbox::new(format!("todo-{index}")).label(item.title.clone()).checked(item.done));
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
			.child(Label::new("Recent events").font_semibold().text_color(rgb(80, 220, 255)))
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct VideoEntry {
	name: String,
	path: PathBuf,
}

#[derive(Debug, Clone)]
struct VideoLibrary {
	root_path:      PathBuf,
	videos:         Vec<VideoEntry>,
	selected_index: usize,
	loaded_index:   Option<usize>,
	playback:       PlaybackState,
	position_us:    u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaybackState {
	Stopped,
	Playing,
	Paused,
}

struct VideoPlayerDemo {
	size:             GridSize,
	path_input:       InputState,
	library:          Option<VideoLibrary>,
	error_message:    Option<String>,
	info_message:     Option<String>,
	pending_controls: Vec<GNativeMediaControlCommand>,
}

impl VideoPlayerDemo {
	fn new() -> Self {
		let mut path_input = InputState::new("video-root-path")
			.placeholder("Enter a directory path that contains videos")
			.clean_on_escape();
		path_input.set_focused(true);
		Self {
			size: GridSize::new(1, 1),
			path_input,
			library: None,
			error_message: None,
			info_message: Some("Enter confirms the directory. F1 opens the demo switcher.".to_string()),
			pending_controls: Vec::new(),
		}
	}

	fn resize(&mut self, size: GridSize) { self.size = size; }

	fn render(&self) -> DemoRender {
		let content =
			if self.library.is_some() { self.browser_view() } else { self.path_prompt_view() };

		DemoRender {
			title: "Video Player",
			help: "Path prompt uses Enter. Browser uses j/k, space, h/l. Selection does not auto-play.",
			content,
			overlay: None,
		}
	}

	fn handle_key(
		&mut self,
		logical_key: GNativeInputKey,
		text: Option<&str>,
		modifiers: GNativeInputModifiers,
	) -> Option<String> {
		if self.library.is_none() {
			match logical_key {
				GNativeInputKey::Named(GNativeInputNamedKey::Backspace) => self.backspace_path(),
				GNativeInputKey::Named(GNativeInputNamedKey::Escape) => self.clear_prompt_error(),
				GNativeInputKey::Named(GNativeInputNamedKey::Enter) => self.submit_path(),
				_ if !modifiers.control && !modifiers.alt => {
					if let Some(value) = text_input_of(&logical_key, text) {
						self.insert_path_text(&value);
					}
				}
				_ => {}
			}
			return self.current_notice();
		}

		let Some(library) = self.library.as_mut() else {
			return self.current_notice();
		};

		match logical_key {
			GNativeInputKey::Named(GNativeInputNamedKey::ArrowUp) => {
				if library.selected_index > 0 {
					library.selected_index -= 1;
				}
			}
			GNativeInputKey::Named(GNativeInputNamedKey::ArrowDown) => {
				if library.selected_index + 1 < library.videos.len() {
					library.selected_index += 1;
				}
			}
			_ if !modifiers.control && !modifiers.alt => {
				if let Some(ch) = character_of(&logical_key, text) {
					match ch {
						'j' => {
							if library.selected_index + 1 < library.videos.len() {
								library.selected_index += 1;
							}
						}
						'k' => {
							if library.selected_index > 0 {
								library.selected_index -= 1;
							}
						}
						' ' => self.toggle_playback(),
						'h' => self.seek_by(-1),
						'l' => self.seek_by(1),
						_ => {}
					}
				}
			}
			_ => {}
		}

		self.current_notice()
	}

	fn paste(&mut self, text: &str) -> Option<String> {
		if self.library.is_none() {
			self.insert_path_text(text);
		}
		self.current_notice()
	}

	fn drain_controls(&mut self) -> Vec<GNativeMediaControlCommand> {
		std::mem::take(&mut self.pending_controls)
	}

	fn shutdown_controls(&mut self) -> Vec<GNativeMediaControlCommand> {
		if self.library.as_ref().is_some_and(|library| library.loaded_index.is_some()) {
			vec![GNativeMediaControlCommand::Stop]
		} else {
			Vec::new()
		}
	}

	fn current_notice(&self) -> Option<String> {
		self.error_message.clone().or_else(|| self.info_message.clone())
	}

	fn insert_path_text(&mut self, text: &str) {
		let mut value = self.path_input.value().to_string();
		value.push_str(text);
		self.path_input.set_value(value);
		self.error_message = None;
	}

	fn backspace_path(&mut self) {
		let mut value = self.path_input.value().to_string();
		value.pop();
		self.path_input.set_value(value);
		self.error_message = None;
	}

	fn clear_prompt_error(&mut self) {
		self.error_message = None;
		if self.path_input.clean_on_escape_enabled() && !self.path_input.value().is_empty() {
			self.path_input.clear();
		}
	}

	fn submit_path(&mut self) {
		let value = self.path_input.value().trim();
		match scan_video_entries(value) {
			Ok(videos) => {
				let root_path = PathBuf::from(value);
				let video_count = videos.len();
				self.library = Some(VideoLibrary {
					root_path,
					videos,
					selected_index: 0,
					loaded_index: None,
					playback: PlaybackState::Stopped,
					position_us: 0,
				});
				self.path_input.set_focused(false);
				self.error_message = None;
				self.info_message = Some(format!(
					"loaded directory. videos={} selected file does not auto-play; press space to start.",
					video_count
				));
			}
			Err(error) => {
				self.error_message = Some(error);
				self.info_message = None;
			}
		}
	}

	fn toggle_playback(&mut self) {
		let Some(library) = self.library.as_mut() else {
			return;
		};
		if library.videos.is_empty() {
			self.error_message = Some("the selected directory has no supported video files".to_string());
			self.info_message = None;
			return;
		}

		let selected_index = library.selected_index;
		if library.loaded_index != Some(selected_index) {
			if library.loaded_index.is_some() {
				self.pending_controls.push(GNativeMediaControlCommand::Stop);
			}
			library.loaded_index = Some(selected_index);
			library.playback = PlaybackState::Playing;
			library.position_us = 0;
			self.pending_controls.push(GNativeMediaControlCommand::Seek { position_us: 0 });
			self.pending_controls.push(GNativeMediaControlCommand::Play);
			self.info_message = Some(format!("queued play: {}", library.videos[selected_index].name));
			self.error_message = None;
			return;
		}

		match library.playback {
			PlaybackState::Playing => {
				library.playback = PlaybackState::Paused;
				self.pending_controls.push(GNativeMediaControlCommand::Pause);
				self.info_message = Some("queued pause".to_string());
			}
			PlaybackState::Paused | PlaybackState::Stopped => {
				library.playback = PlaybackState::Playing;
				self.pending_controls.push(GNativeMediaControlCommand::Play);
				self.info_message = Some("queued play".to_string());
			}
		}
		self.error_message = None;
	}

	fn seek_by(&mut self, seconds: i64) {
		let Some(library) = self.library.as_mut() else {
			return;
		};
		if library.loaded_index.is_none() {
			self.error_message = Some("select a video and press space before seeking".to_string());
			self.info_message = None;
			return;
		}

		if seconds < 0 {
			library.position_us =
				library.position_us.saturating_sub(seconds.unsigned_abs() * VIDEO_SEEK_STEP_US);
		} else {
			library.position_us =
				library.position_us.saturating_add((seconds as u64).saturating_mul(VIDEO_SEEK_STEP_US));
		}
		self
			.pending_controls
			.push(GNativeMediaControlCommand::Seek { position_us: library.position_us });
		self.info_message = Some(format!("queued seek to {}", format_duration_us(library.position_us)));
		self.error_message = None;
	}

	fn path_prompt_view(&self) -> Element {
		let status = self
			.error_message
			.as_deref()
			.unwrap_or("Enter an existing directory path. Invalid paths keep the prompt open.");
		GroupBox::new()
			.id("video-path-prompt")
			.outline()
			.fill()
			.title("Open Video Directory")
			.child(Label::new(
				"The browser loads the directory on the left and reserves a dedicated Video surface on \
				 the right.",
			))
			.child(Label::new(status).text_color(if self.error_message.is_some() {
				rgb(255, 120, 120)
			} else {
				rgb(80, 220, 255)
			}))
			.child(Input::new(&self.path_input))
			.child(Label::new("Supported extensions").secondary(VIDEO_EXTENSIONS.join(", ")))
			.into_element()
	}

	fn browser_view(&self) -> Element {
		let Some(library) = self.library.as_ref() else {
			return div().into_element();
		};

		h_flex()
			.gap_1()
			.size_full()
			.child(div().w(px(42.0)).child(self.video_list_panel(library)))
			.child(div().flex_1().child(self.video_player_panel(library)))
			.into_element()
	}

	fn video_list_panel(&self, library: &VideoLibrary) -> Element {
		let mut panel = GroupBox::new()
			.id("video-list")
			.outline()
			.fill()
			.title("Videos")
			.child(Label::new(library.root_path.display().to_string()).secondary("j/k move selection"));

		if library.videos.is_empty() {
			panel = panel.child(
				Label::new("No supported videos found")
					.secondary("Use F1 to switch demos or restart and enter another directory."),
			);
			return panel.into_element();
		}

		let mut rows = v_flex().gap_1();
		for (index, entry) in library.videos.iter().enumerate() {
			let selected = index == library.selected_index;
			let active = library.loaded_index == Some(index);
			let state_label = if active {
				match library.playback {
					PlaybackState::Playing => "playing",
					PlaybackState::Paused => "paused",
					PlaybackState::Stopped => "stopped",
				}
			} else {
				"queued"
			};

			let mut row = v_flex()
				.child(
					Label::new(format!("{} {}", if selected { ">" } else { " " }, entry.name))
						.font_semibold()
						.text_color(if selected { rgb(255, 214, 92) } else { rgb(226, 230, 238) }),
				)
				.child(Label::new(entry.path.display().to_string()).secondary(state_label));
			if selected {
				row = row.bg(rgba(22, 36, 72, 220));
			}
			rows = rows.child(row);
		}

		panel.child(rows).into_element()
	}

	fn video_player_panel(&self, library: &VideoLibrary) -> Element {
		let current_file = library
			.loaded_index
			.and_then(|index| library.videos.get(index))
			.map(|entry| entry.name.as_str())
			.unwrap_or("not playing");
		let playback = match library.playback {
			PlaybackState::Stopped => "stopped",
			PlaybackState::Playing => "playing",
			PlaybackState::Paused => "paused",
		};
		let notice = self.error_message.as_deref().or(self.info_message.as_deref()).unwrap_or(" ");

		GroupBox::new()
			.id("video-player-panel")
			.outline()
			.fill()
			.title("Player")
			.child(Label::new(format!(
				"current={}  state={}  position={}",
				current_file,
				playback,
				format_duration_us(library.position_us)
			)))
			.child(Label::new("space play/pause  h -1s  l +1s").secondary(notice))
			.child(div().h(px(22.0)).bg(rgba(8, 12, 24, 255)).child(video(VIDEO_SURFACE_ID)))
			.child(Label::new(
				"The dedicated Video element reserves a GPU-backed surface here; it is no longer faked \
				 through text rows.",
			))
			.into_element()
	}
}

fn scan_video_entries(path: &str) -> Result<Vec<VideoEntry>, String> {
	let trimmed = path.trim();
	if trimmed.is_empty() {
		return Err("path is empty; enter an existing directory".to_string());
	}

	let root = Path::new(trimmed);
	if !root.exists() {
		return Err(format!("path does not exist: {trimmed}"));
	}
	if !root.is_dir() {
		return Err(format!("path is not a directory: {trimmed}"));
	}

	let mut videos = Vec::new();
	for entry in fs::read_dir(root).map_err(|error| error.to_string())? {
		let entry = entry.map_err(|error| error.to_string())?;
		let path = entry.path();
		if !path.is_file() || !is_supported_video_path(&path) {
			continue;
		}

		let name = path
			.file_name()
			.and_then(|name| name.to_str())
			.map(ToOwned::to_owned)
			.unwrap_or_else(|| path.display().to_string());
		videos.push(VideoEntry { name, path });
	}

	videos.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
	Ok(videos)
}

fn is_supported_video_path(path: &Path) -> bool {
	path.extension().and_then(|extension| extension.to_str()).is_some_and(|extension| {
		VIDEO_EXTENSIONS.iter().any(|candidate| candidate.eq_ignore_ascii_case(extension))
	})
}

fn format_duration_us(position_us: u64) -> String {
	let total_seconds = position_us / 1_000_000;
	let minutes = total_seconds / 60;
	let seconds = total_seconds % 60;
	format!("{minutes:02}:{seconds:02}")
}

#[cfg(test)]
mod tests {
	use std::{
		fs,
		path::PathBuf,
		time::{SystemTime, UNIX_EPOCH},
	};

	use super::{
		ActiveDemo, DemoHostApp, DemoId, VIDEO_EXTENSIONS, format_duration_us, scan_video_entries,
	};

	#[test]
	fn scan_video_entries_filters_and_sorts_supported_files() {
		let temp_dir = unique_temp_dir("gnative-video-demo-scan");
		fs::create_dir_all(&temp_dir).unwrap();
		fs::write(temp_dir.join("zeta.mkv"), b"").unwrap();
		fs::write(temp_dir.join("alpha.MP4"), b"").unwrap();
		fs::write(temp_dir.join("notes.txt"), b"").unwrap();

		let videos = scan_video_entries(temp_dir.to_str().unwrap()).unwrap();
		assert_eq!(videos.len(), 2);
		assert_eq!(videos[0].name, "alpha.MP4");
		assert_eq!(videos[1].name, "zeta.mkv");
		assert!(VIDEO_EXTENSIONS.iter().any(|extension| extension.eq_ignore_ascii_case("mp4")));

		fs::remove_dir_all(&temp_dir).unwrap();
	}

	#[test]
	fn demo_host_switch_confirmation_replaces_active_demo() {
		let mut app = DemoHostApp::new(germinal_gnative_ui::GridSize::new(80, 24));
		app.select_next_demo();
		app.activate_selected_demo();

		assert_eq!(app.active_demo.id(), DemoId::VideoPlayer);
		app.active_demo = ActiveDemo::new(DemoId::Todo);
		assert_eq!(app.active_demo.id(), DemoId::Todo);
	}

	#[test]
	fn formats_seek_position_as_minutes_and_seconds() {
		assert_eq!(format_duration_us(0), "00:00");
		assert_eq!(format_duration_us(61_000_000), "01:01");
	}

	fn unique_temp_dir(prefix: &str) -> PathBuf {
		let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
		std::env::temp_dir().join(format!("{prefix}-{unique}"))
	}
}
