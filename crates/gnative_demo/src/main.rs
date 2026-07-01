use std::{
	collections::VecDeque,
	io::stdout,
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
	},
	seq::Seq,
};
use germinal_gnative_sdk::local_session::{
	LocalGNativeFrameWriter, LocalGNativeSession, LocalGNativeTunnelBootstrap,
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

const MAX_ACTIVITY_EVENTS: usize = 10;
const FPS_WINDOW: Duration = Duration::from_secs(1);

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
	let mut app = DemoApp::new(initial_size);
	app.push_event(format!(
		"connected gshell={} protocol=v{}",
		accepted.gshell_id.value(),
		accepted.protocol_version
	));
	app.push_event(
		"keyboard todo demo ready: 1/2 switch panels, j/k move, n/e dialogs, space toggles".to_string(),
	);

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

fn handle_session_message(app: &mut DemoApp, message: SessionMessage) -> bool {
	match message {
		SessionMessage::Input(input) => {
			handle_input(app, input);
			true
		}
		SessionMessage::Closed => {
			app.push_event("host closed the GNative session".to_string());
			false
		}
		SessionMessage::Error(error) => {
			app.push_event(format!("session error: {error}"));
			false
		}
	}
}

fn draw_app(app: &mut DemoApp, emitter: &mut DemoFrameEmitter) -> Result<(), String> {
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

fn handle_input(app: &mut DemoApp, input: GNativeInputEvent) {
	match input {
		GNativeInputEvent::Resize { columns, rows } => {
			app.size = GridSize::new(
				u16::try_from(columns).unwrap_or(u16::MAX).max(1),
				u16::try_from(rows).unwrap_or(u16::MAX).max(1),
			);
			app.push_event(format!("resize {}x{}", columns, rows));
		}
		GNativeInputEvent::Paste(text) | GNativeInputEvent::Ime(text) => app.paste(&text),
		GNativeInputEvent::Bytes(bytes) => {
			app.push_event(format!("unsupported input: bytes {:?}", bytes));
		}
		GNativeInputEvent::Key { state, logical_key, text, modifiers } => {
			handle_key_input(app, state, logical_key, text.as_deref(), modifiers);
		}
	}
}

fn handle_key_input(
	app: &mut DemoApp,
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

	if app.composer.is_focused() {
		match logical_key {
			GNativeInputKey::Named(GNativeInputNamedKey::Backspace) => app.backspace(),
			GNativeInputKey::Named(GNativeInputNamedKey::Escape) => app.escape(),
			GNativeInputKey::Named(GNativeInputNamedKey::Enter) => app.activate_focused(),
			_ if !modifiers.control && !modifiers.alt => {
				if let Some(text) = text_input_of(&logical_key, text) {
					app.insert_text(&text);
				}
			}
			_ => {}
		}
		return;
	}

	match logical_key {
		GNativeInputKey::Named(GNativeInputNamedKey::Tab) => app.next_focus(),
		GNativeInputKey::Named(GNativeInputNamedKey::ArrowUp) => app.move_up(),
		GNativeInputKey::Named(GNativeInputNamedKey::ArrowDown) => app.move_down(),
		GNativeInputKey::Named(GNativeInputNamedKey::Enter) => app.activate_focused(),
		GNativeInputKey::Named(GNativeInputNamedKey::Escape) => app.escape(),
		_ if !modifiers.control && !modifiers.alt => {
			if let Some(ch) = character_of(&logical_key, text) {
				match ch {
					'1' => app.focus_list(),
					'2' => app.focus_activity(),
					'j' => app.move_down(),
					'k' => app.move_up(),
					' ' => app.toggle_from_space(),
					'n' => app.prepare_new(),
					'e' => app.start_edit_selected(),
					'd' => app.delete_selected(),
					_ => {}
				}
			}
		}
		_ => {}
	}
}

fn character_of(logical_key: &GNativeInputKey, text: Option<&str>) -> Option<char> {
	text_input_of(logical_key, text).and_then(|text| {
		let mut chars = text.chars();
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusArea {
	List,
	Activity,
}

#[derive(Debug, Clone)]
struct TodoItem {
	title: String,
	done:  bool,
}

struct DemoApp {
	size:             GridSize,
	composer:         InputState,
	todos:            Vec<TodoItem>,
	selected_task:    usize,
	focus:            FocusArea,
	editing:          Option<usize>,
	events:           VecDeque<String>,
	should_quit:      bool,
	frame_count:      u64,
	fps:              f32,
	presented_frames: VecDeque<Instant>,
}

impl DemoApp {
	fn new(size: GridSize) -> Self {
		let mut composer =
			InputState::new("todo-composer").placeholder("Type a task title").clean_on_escape();
		composer.set_focused(false);
		Self {
			size,
			composer,
			todos: vec![
				TodoItem { title: "Port gpui-component surface API".to_string(), done: true },
				TodoItem { title: "Build keyboard-first todo interactions".to_string(), done: false },
				TodoItem { title: "Split primitives and widgets crates".to_string(), done: false },
			],
			selected_task: 0,
			focus: FocusArea::List,
			editing: None,
			events: VecDeque::new(),
			should_quit: false,
			frame_count: 0,
			fps: 0.0,
			presented_frames: VecDeque::new(),
		}
	}

	fn push_event(&mut self, event: String) {
		self.events.push_front(event);
		if self.events.len() > MAX_ACTIVITY_EVENTS {
			self.events.pop_back();
		}
	}

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

	fn set_focus(&mut self, focus: FocusArea) {
		self.focus = focus;
		self.composer.set_focused(false);
	}

	fn focus_list(&mut self) { self.set_focus(FocusArea::List); }

	fn focus_activity(&mut self) { self.set_focus(FocusArea::Activity); }

	fn next_focus(&mut self) {
		match self.focus {
			FocusArea::List => self.set_focus(FocusArea::Activity),
			FocusArea::Activity => self.set_focus(FocusArea::List),
		}
	}

	fn move_up(&mut self) {
		if self.focus == FocusArea::List && self.selected_task > 0 {
			self.selected_task -= 1;
		}
	}

	fn move_down(&mut self) {
		if self.focus == FocusArea::List && self.selected_task + 1 < self.todos.len() {
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

	fn paste(&mut self, text: &str) {
		if self.composer.is_focused() {
			self.insert_text(text);
			self.push_event(format!("pasted {} chars", text.chars().count()));
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
		if self.composer.is_focused() {
			if self.editing.is_some() {
				self.editing = None;
				self.composer.clear();
				self.push_event("edit dialog canceled".to_string());
			} else if self.composer.clean_on_escape_enabled() && !self.composer.value().is_empty() {
				self.composer.clear();
				self.push_event("new task dialog cleared".to_string());
			}
			self.set_focus(FocusArea::List);
		}
	}

	fn activate_focused(&mut self) {
		if self.composer.is_focused() {
			self.submit_composer();
		}
	}

	fn toggle_from_space(&mut self) {
		if self.focus == FocusArea::List {
			self.toggle_selected();
		}
	}

	fn prepare_new(&mut self) {
		self.editing = None;
		self.composer.clear();
		self.composer.set_focused(true);
		self.push_event("new task dialog opened".to_string());
	}

	fn submit_composer(&mut self) {
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
		self.set_focus(FocusArea::List);
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
			self.set_focus(FocusArea::List);
		} else {
			self.selected_task = self.selected_task.min(self.todos.len() - 1);
		}
	}

	fn completed_count(&self) -> usize { self.todos.iter().filter(|item| item.done).count() }

	fn mode_label(&self) -> &'static str {
		if self.composer.is_focused() {
			if self.editing.is_some() { "Editing" } else { "Creating" }
		} else {
			"Browsing"
		}
	}

	fn status_title(&self) -> String {
		format!(
			"GNative Todo Demo  viewport={}x{}  frame={} fps={:.1}",
			self.size.columns, self.size.rows, self.frame_count, self.fps
		)
	}

	fn ui_tree(&self) -> UiTree {
		let mut root = div().size_full().bg(rgba(6, 9, 20, 255)).child(
			v_flex()
				.size_full()
				.child(
					div().h(px(8.0)).child(
						GroupBox::new().id("status").outline().title("Status").child(
							v_flex()
								.h(px(6.0))
								.gap_1()
								.child(
									Label::new(self.status_title()).font_semibold().text_color(rgb(80, 220, 255)),
								)
								.child(Label::new(format!(
									"mode={} focus={} tasks={} done={}",
									self.mode_label(),
									self.focus_name(),
									self.todos.len(),
									self.completed_count()
								)))
								.child(Label::new(
									"Goal: gpui-component style widgets over gnative-ui primitives.",
								)),
						),
					),
				)
				.child(
					div().flex_1().child(
						h_flex()
							.gap_1()
							.flex_1()
							.child(div().flex_1().child(self.todo_panel()))
							.child(div().flex_1().child(self.activity_panel())),
					),
				),
		);

		if self.composer.is_focused() {
			root = root.child(self.dialog_overlay());
		}

		UiTree::new(root)
	}

	fn todo_panel(&self) -> Element {
		let mut panel = GroupBox::new().id("todo-list").outline().title("Todo List");
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
				Label::new(if index == self.selected_task && self.focus == FocusArea::List {
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
			.id("activity")
			.outline()
			.fill()
			.title("Activity")
			.child(Label::new("Keyboard").font_semibold().text_color(rgb(80, 220, 255)))
			.child(div().child(
				"1 todo list  2 activity\nj/k move selection\nspace toggle selected todo\nn new dialog  e \
				 edit dialog  d delete\nEnter confirm dialog  Esc close dialog\nCtrl+C quits",
			))
			.child(Label::new("Recent events").font_semibold().text_color(rgb(80, 220, 255)))
			.child(div().child(self.activity_text()))
			.into_element()
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
										.title(self.dialog_title())
										.child(Label::new(self.dialog_hint()).text_color(rgb(80, 220, 255)))
										.child(Input::new(&self.composer))
										.child(Label::new("Enter confirm  Esc cancel").secondary("Dialog mode")),
								),
							)
							.child(div().flex_1()),
					)
					.child(div().flex_1()),
			)
			.into_element()
	}

	fn focus_name(&self) -> &'static str {
		match self.focus {
			FocusArea::List => "list",
			FocusArea::Activity => "activity",
		}
	}

	fn dialog_title(&self) -> &'static str {
		if self.editing.is_some() { "Edit Todo" } else { "New Todo" }
	}

	fn dialog_hint(&self) -> &'static str {
		if self.editing.is_some() { "Update the selected task title." } else { "Create a new task." }
	}

	fn activity_text(&self) -> String {
		if self.events.is_empty() {
			return "<waiting>".to_string();
		}
		self.events.iter().map(|event| format!("  {event}")).collect::<Vec<_>>().join("\n")
	}
}
