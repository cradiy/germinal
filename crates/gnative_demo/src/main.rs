use std::{
	collections::VecDeque,
	io::stdout,
	sync::mpsc::{self, RecvTimeoutError},
	thread,
	time::{Duration, Instant},
};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_sdk::{
	input::{UnsupportedCrosstermEvent, try_to_crossterm_event},
	local_session::{LocalGNativeBootstrap, LocalGNativeFrameWriter, LocalGNativeSession},
};
use germinal_gnative_ui::{
	CompiledUi, Element, GridSize, GroupBox, IntoElementNode, UiTree, div, h_flex, px, rgb, rgba,
	styled_text_input, v_flex,
};
use germinal_ports::{
	gnative::{
		frame::{GNativeFrame, GNativeFrameCursor},
		input::GNativeInputEvent,
	},
	seq::Seq,
};

const COMMAND_ANIMATION_DURATION: Duration = Duration::from_secs(10);
const TARGET_FRAME_INTERVAL: Duration = Duration::from_millis(5);
const MAX_ACTIVITY_EVENTS: usize = 8;
const MAX_SUBMISSIONS: usize = 6;
const FPS_WINDOW: Duration = Duration::from_secs(1);

fn main() -> Result<(), String> {
	let bootstrap = LocalGNativeBootstrap::bind_temporary(1)?;
	eprintln!("germinal-gnative-demo waiting for host at {}", bootstrap.descriptor().endpoint);
	let mut terminal_stdout = stdout();
	bootstrap
		.write_enter_control_sequence(&mut terminal_stdout)
		.map_err(|error| error.to_string())?;

	let mut session = bootstrap.accept()?;
	let accepted = session.accepted().clone();
	let initial_size = wait_for_initial_size(&mut session)?;
	let mut emitter = DemoFrameEmitter::new(accepted.gshell_id, session.frame_writer()?);
	let mut app = DemoApp::new(initial_size);
	app.push_event(format!(
		"connected gshell={} protocol=v{}",
		accepted.gshell_id.value(),
		accepted.protocol_version
	));
	app.push_event("type 'animation' and press Enter to play the pixel demo".to_string());

	let (input_tx, input_rx) = mpsc::channel();
	spawn_input_reader(session, input_tx);
	draw_app(&mut app, &mut emitter)?;

	loop {
		let frame_started_at = Instant::now();
		let mut needs_draw = false;

		while let Ok(message) = input_rx.try_recv() {
			if !handle_session_message(&mut app, message) {
				app.should_quit = true;
			}
			needs_draw = true;
		}

		if app.should_quit {
			break;
		}

		if app.advance_animation(Instant::now()) {
			needs_draw = true;
		}

		if needs_draw {
			draw_app(&mut app, &mut emitter)?;
		}

		let wait = if app.is_animating() {
			TARGET_FRAME_INTERVAL.saturating_sub(frame_started_at.elapsed())
		} else {
			Duration::from_secs(60)
		};

		match input_rx.recv_timeout(wait) {
			Ok(message) => {
				if !handle_session_message(&mut app, message) {
					app.should_quit = true;
				}
				draw_app(&mut app, &mut emitter)?;
			}
			Err(RecvTimeoutError::Timeout) => {}
			Err(RecvTimeoutError::Disconnected) => break,
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
	let compiled = app.ui_tree().compile(app.size);
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
	match try_to_crossterm_event(input) {
		Ok(event) => handle_crossterm_event(app, event),
		Err(unsupported) => {
			app.push_event(format!("unsupported input: {}", describe_unsupported(&unsupported)));
		}
	}
}

fn handle_crossterm_event(app: &mut DemoApp, event: Event) {
	match event {
		Event::Resize(columns, rows) => {
			app.size = GridSize::new(columns, rows);
			app.push_event(format!("resize {}x{}", columns, rows));
		}
		Event::Paste(text) => {
			app.input.push_str(&text);
			app.push_event(format!("paste {:?}", text));
		}
		Event::Key(key) if key.kind == KeyEventKind::Press => {
			app.push_event(format!("key {}", describe_key_event(&key)));
			match key.code {
				KeyCode::Char('q') if key.modifiers.is_empty() => app.should_quit = true,
				KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
					app.should_quit = true
				}
				KeyCode::Char(ch)
					if !key.modifiers.contains(KeyModifiers::CONTROL)
						&& !key.modifiers.contains(KeyModifiers::ALT) =>
				{
					app.input.push(ch)
				}
				KeyCode::Backspace => {
					app.input.pop();
				}
				KeyCode::Enter => app.submit_input(),
				_ => {}
			}
		}
		_ => {}
	}
}

fn clamped_size(columns: u32, rows: u32) -> GridSize {
	GridSize::new(
		u16::try_from(columns).unwrap_or(u16::MAX).max(1),
		u16::try_from(rows).unwrap_or(u16::MAX).max(1),
	)
}

fn describe_unsupported(event: &UnsupportedCrosstermEvent) -> String {
	match event {
		UnsupportedCrosstermEvent::Bytes(bytes) => format!("bytes {:?}", bytes),
		UnsupportedCrosstermEvent::Ime(text) => format!("ime {:?}", text),
		UnsupportedCrosstermEvent::Character(text) => format!("character {:?}", text),
	}
}

fn describe_key_event(key: &crossterm::event::KeyEvent) -> String {
	match key.code {
		KeyCode::Char(ch) => {
			let prefix = modifier_prefix(key.modifiers);
			if prefix.is_empty() { format!("{ch}") } else { format!("{prefix}{ch}") }
		}
		_ => format!("{:?}", key.code),
	}
}

fn modifier_prefix(modifiers: KeyModifiers) -> String {
	let mut parts = Vec::new();
	if modifiers.contains(KeyModifiers::CONTROL) {
		parts.push("CTRL");
	}
	if modifiers.contains(KeyModifiers::ALT) {
		parts.push("ALT");
	}
	if modifiers.contains(KeyModifiers::SHIFT) {
		parts.push("SHIFT");
	}
	if parts.is_empty() { String::new() } else { format!("{}+", parts.join("+")) }
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

struct DemoApp {
	size:                 GridSize,
	input:                String,
	submissions:          VecDeque<String>,
	events:               VecDeque<String>,
	should_quit:          bool,
	frame_count:          u64,
	fps:                  f32,
	presented_frames:     VecDeque<Instant>,
	animation_started_at: Option<Instant>,
	animation_elapsed:    Duration,
}

impl DemoApp {
	fn new(size: GridSize) -> Self {
		Self {
			size,
			input: String::new(),
			submissions: VecDeque::new(),
			events: VecDeque::new(),
			should_quit: false,
			frame_count: 0,
			fps: 0.0,
			presented_frames: VecDeque::new(),
			animation_started_at: None,
			animation_elapsed: Duration::ZERO,
		}
	}

	fn submit_input(&mut self) {
		let submitted = self.input.clone();
		let command = submitted.trim().to_ascii_lowercase();
		self.submissions.push_front(submitted);
		if self.submissions.len() > MAX_SUBMISSIONS {
			self.submissions.pop_back();
		}
		self.input.clear();
		if matches!(command.as_str(), "animation" | "animate" | "play") {
			self.animation_started_at = Some(Instant::now());
			self.animation_elapsed = Duration::ZERO;
			self.push_event("playing PixelFillRect animation".to_string());
		} else if !command.is_empty() {
			self.push_event("submit 'animation' to trigger the pixel demo".to_string());
		}
	}

	fn push_event(&mut self, event: String) {
		self.events.push_front(event);
		if self.events.len() > MAX_ACTIVITY_EVENTS {
			self.events.pop_back();
		}
	}

	fn is_animating(&self) -> bool { self.animation_started_at.is_some() }

	fn advance_animation(&mut self, now: Instant) -> bool {
		let Some(started_at) = self.animation_started_at else {
			return false;
		};
		self.animation_elapsed = now.saturating_duration_since(started_at);
		if self.animation_elapsed >= COMMAND_ANIMATION_DURATION {
			self.animation_started_at = None;
			self.animation_elapsed = COMMAND_ANIMATION_DURATION;
			self.push_event("animation complete".to_string());
		}
		true
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

	fn animation_t(&self) -> f32 { self.animation_elapsed.as_secs_f32() }

	fn ui_tree(&self) -> UiTree {
		UiTree::new(
			div().size_full().child(self.pixel_scene()).child(
				v_flex()
					.size_full()
					.child(
						div().h(px(8.0)).child(
							GroupBox::new().title("Status").child(
								v_flex()
									.h(px(6.0))
									.gap_1()
									.child(div().text_color(rgb(80, 220, 255)).font_bold().child(self.status_title()))
									.child("Input is read on a dedicated thread; rendering owns frame emission.")
									.child(
										"Type 'animation' and press Enter to play the full-GShell pixel animation.",
									),
							),
						),
					)
					.child(
						div().flex_1().child(GroupBox::new().title("Workspace").child(self.workspace_body())),
					)
					.child(
						div().h(px(8.0)).child(
							GroupBox::new().title("Composer").child(
								v_flex()
									.h(px(6.0))
									.gap_1()
									.child("Current input")
									.child(styled_text_input(self.input.clone(), true, yellow_style()))
									.child("Enter submits. Type animation to play. q exits."),
							),
						),
					),
			),
		)
	}

	fn workspace_body(&self) -> Element {
		h_flex()
			.flex_1()
			.child(
				v_flex()
					.flex_1()
					.gap_1()
					.child(div().text_color(rgb(80, 220, 255)).font_bold().child("Activity"))
					.child(div().flex_1().child(self.activity_text())),
			)
			.child(div().w(px(1.0)).child(self.main_separator_text()))
			.child(
				v_flex()
					.flex_1()
					.gap_1()
					.child(div().text_color(rgb(80, 220, 255)).font_bold().child("Pixel Layer"))
					.child(div().flex_1().child(self.pixel_text())),
			)
			.into_element()
	}

	fn status_title(&self) -> String {
		format!(
			"GNative UI Tree Demo  viewport={}x{}  frame={} fps={:.1}",
			self.size.columns, self.size.rows, self.frame_count, self.fps
		)
	}

	fn activity_text(&self) -> String {
		let mut lines = vec!["Submitted lines".to_string()];
		if self.submissions.is_empty() {
			lines.push("  <type animation + Enter>".to_string());
		} else {
			lines.extend(self.submissions.iter().map(|line| format!("  {line}")));
		}
		lines.push(String::new());
		lines.push("Recent events".to_string());
		if self.events.is_empty() {
			lines.push("  <waiting>".to_string());
		} else {
			lines.extend(self.events.iter().map(|event| format!("  {event}")));
		}
		lines.join("\n")
	}

	fn pixel_text(&self) -> String {
		let status = if self.is_animating() { "playing" } else { "idle" };
		format!(
			"Pixel animation status: {status}\nThe card, glow, and beam are expressed as gpui-like \
			 absolute divs.\nThe visible DSL is now aligned toward gpui-component style."
		)
	}

	fn main_separator_text(&self) -> String {
		let main_height = self.size.rows.saturating_sub(14).max(1);
		std::iter::repeat_n("|", main_height as usize).collect::<Vec<_>>().join("\n")
	}

	fn pixel_scene(&self) -> Element {
		let width_px = u32::from(self.size.columns).max(1) * 8;
		let height_px = u32::from(self.size.rows).max(1) * 16;
		let t = self.animation_t();
		let card_w = width_px.clamp(96, 240).min(width_px.max(1));
		let card_h = height_px.clamp(48, 120).min(height_px.max(1));
		let x = ((width_px.saturating_sub(card_w) as f32) * (t.sin() * 0.5 + 0.5)).round() as u32;
		let y =
			((height_px.saturating_sub(card_h) as f32) * ((t * 1.37).cos() * 0.5 + 0.5)).round() as u32;
		let pulse = if self.is_animating() { (((t * 4.0).sin() * 0.5 + 0.5) * 80.0) as u8 } else { 28 };
		let beam_x = if self.is_animating() {
			((width_px + 160) as f32 * ((t * 0.75).sin() * 0.5 + 0.5)).round() as u32
		} else {
			0
		};
		div()
			.size_full()
			.child(
				div()
					.absolute()
					.left(px(0.0))
					.top(px(0.0))
					.w(px(width_px as f32))
					.h(px(height_px as f32))
					.bg(rgba(6, 9, 20, 255)),
			)
			.child(
				div()
					.absolute()
					.left(px(beam_x.saturating_sub(160) as f32))
					.top(px(0.0))
					.w(px(96.0))
					.h(px(height_px as f32))
					.bg(rgba(54, 94, 210, 42)),
			)
			.child(
				div()
					.absolute()
					.left(px(x.saturating_sub(18) as f32))
					.top(px(y.saturating_sub(18) as f32))
					.w(px((card_w + 36) as f32))
					.h(px((card_h + 36) as f32))
					.bg(rgba(82, 142, 255, pulse)),
			)
			.child(
				div()
					.absolute()
					.left(px(x as f32))
					.top(px(y as f32))
					.w(px(card_w as f32))
					.h(px(card_h as f32))
					.bg(rgba(20, 32, 72, 230)),
			)
			.child(
				div()
					.absolute()
					.left(px((x + 8) as f32))
					.top(px((y + 8) as f32))
					.w(px(card_w.saturating_sub(16) as f32))
					.h(px(6.0))
					.bg(rgba(80, 220, 255, 210)),
			)
			.child(
				div()
					.absolute()
					.left(px(width_px.saturating_sub(180) as f32))
					.top(px(20.0))
					.w(px(150.0))
					.h(px(18.0))
					.bg(rgba(255, 86, 170, 190)),
			)
			.into_element()
	}
}

fn yellow_style() -> germinal_ports::rendering::frame_plan_builder::TextStyleDto {
	germinal_ports::rendering::frame_plan_builder::TextStyleDto {
		foreground: Some(rgb(255, 214, 92)),
		background: None,
		bold:       false,
		italic:     false,
		underline:  false,
	}
}
