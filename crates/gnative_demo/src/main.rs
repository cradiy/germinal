use std::{
	collections::VecDeque,
	io::{self, stdout},
	sync::mpsc::{self, RecvTimeoutError},
	thread,
	time::{Duration, Instant},
};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use germinal_gnative_ratatui_backend::GerminalBackend;
use germinal_gnative_sdk::{
	input::{UnsupportedCrosstermEvent, try_to_crossterm_event},
	local_session::{LocalGNativeBootstrap, LocalGNativeSession},
};
use germinal_ports::{
	gnative::{frame::GNativeFrame, input::GNativeInputEvent},
	rendering::frame_plan_builder::{RenderCommandDto, RgbaColorDto},
};
use ratatui::{
	Frame, Terminal,
	layout::{Constraint, Layout, Rect, Size},
	style::{Color, Modifier, Style, Stylize},
	text::{Line, Span},
	widgets::{Block, Borders, Paragraph, Wrap},
};

const COMMAND_ANIMATION_DURATION: Duration = Duration::from_secs(10);
const TARGET_FRAME_INTERVAL: Duration = Duration::from_millis(5);
const MAX_ACTIVITY_EVENTS: usize = 8;
const MAX_SUBMISSIONS: usize = 6;
const FPS_WINDOW: Duration = Duration::from_secs(1);

type FrameEmitter = Box<dyn FnMut(GNativeFrame) -> io::Result<()>>;
type DemoTerminal = Terminal<GerminalBackend<FrameEmitter>>;

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let bootstrap = LocalGNativeBootstrap::bind_temporary(1)?;
	eprintln!("germinal-gnative-demo waiting for host at {}", bootstrap.descriptor().endpoint);
	let mut terminal_stdout = stdout();
	bootstrap.write_enter_control_sequence(&mut terminal_stdout)?;

	let mut session = bootstrap.accept()?;
	let accepted = session.accepted().clone();
	let initial_size = wait_for_initial_size(&mut session)?;
	let frame_writer = session.frame_writer()?;
	let emitter: FrameEmitter = Box::new({
		let mut frame_writer = frame_writer;
		move |frame| frame_writer.send_frame(frame).map_err(io::Error::other)
	});

	let backend = GerminalBackend::new(accepted.gshell_id, initial_size, emitter);
	let mut terminal = Terminal::new(backend)?;
	let mut app = DemoApp::new(initial_size);
	app.push_event(format!(
		"connected gshell={} protocol=v{}",
		accepted.gshell_id.value(),
		accepted.protocol_version
	));
	app.push_event("type 'animation' and press Enter to play the pixel demo".to_string());

	let (input_tx, input_rx) = mpsc::channel();
	spawn_input_reader(session, input_tx);
	draw_app(&mut app, &mut terminal)?;

	loop {
		let frame_started_at = Instant::now();
		let mut needs_draw = false;

		while let Ok(message) = input_rx.try_recv() {
			if !handle_session_message(&mut app, &mut terminal, message)? {
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
			draw_app(&mut app, &mut terminal)?;
		}

		let wait = if app.is_animating() {
			TARGET_FRAME_INTERVAL.saturating_sub(frame_started_at.elapsed())
		} else {
			Duration::from_secs(60)
		};

		match input_rx.recv_timeout(wait) {
			Ok(message) => {
				if !handle_session_message(&mut app, &mut terminal, message)? {
					app.should_quit = true;
				}
				draw_app(&mut app, &mut terminal)?;
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

fn handle_session_message(
	app: &mut DemoApp,
	terminal: &mut DemoTerminal,
	message: SessionMessage,
) -> io::Result<bool> {
	match message {
		SessionMessage::Input(input) => {
			handle_input(app, terminal, input)?;
			Ok(true)
		}
		SessionMessage::Closed => {
			app.push_event("host closed the GNative session".to_string());
			Ok(false)
		}
		SessionMessage::Error(error) => {
			app.push_event(format!("session error: {error}"));
			Ok(false)
		}
	}
}

fn draw_app(app: &mut DemoApp, terminal: &mut DemoTerminal) -> io::Result<()> {
	terminal.backend_mut().set_pixel_commands(app.pixel_commands());
	terminal.draw(|frame| app.render(frame))?;
	app.record_presented_frame(Instant::now());
	Ok(())
}

fn wait_for_initial_size(session: &mut LocalGNativeSession) -> Result<Size, String> {
	while let Some(input) = session.read_input()? {
		if let GNativeInputEvent::Resize { columns, rows } = input {
			return Ok(clamped_size(columns, rows));
		}
	}
	Err("host closed before initial resize".to_string())
}

fn handle_input(
	app: &mut DemoApp,
	terminal: &mut DemoTerminal,
	input: GNativeInputEvent,
) -> io::Result<()> {
	match try_to_crossterm_event(input) {
		Ok(event) => handle_crossterm_event(app, terminal, event),
		Err(unsupported) => {
			app.push_event(format!("unsupported input: {}", describe_unsupported(&unsupported)));
			Ok(())
		}
	}
}

fn handle_crossterm_event(
	app: &mut DemoApp,
	terminal: &mut DemoTerminal,
	event: Event,
) -> io::Result<()> {
	match event {
		Event::Resize(columns, rows) => {
			let size = Size::new(columns, rows);
			terminal.backend_mut().resize(size);
			terminal.autoresize()?;
			app.size = size;
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
	Ok(())
}

fn clamped_size(columns: u32, rows: u32) -> Size {
	Size::new(
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

struct DemoApp {
	size:                 Size,
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
	fn new(size: Size) -> Self {
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

	fn pixel_commands(&self) -> Vec<RenderCommandDto> {
		let width_px = u32::from(self.size.width).max(1) * 8;
		let height_px = u32::from(self.size.height).max(1) * 16;
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
		vec![
			fill(0, 0, width_px, height_px, rgba(6, 9, 20, 255)),
			fill(beam_x.saturating_sub(160), 0, 96, height_px, rgba(54, 94, 210, 42)),
			fill(
				x.saturating_sub(18),
				y.saturating_sub(18),
				card_w + 36,
				card_h + 36,
				rgba(82, 142, 255, pulse),
			),
			fill(x, y, card_w, card_h, rgba(20, 32, 72, 230)),
			fill(x + 8, y + 8, card_w.saturating_sub(16), 6, rgba(80, 220, 255, 210)),
			fill(width_px.saturating_sub(180), 20, 150, 18, rgba(255, 86, 170, 190)),
		]
	}

	fn render(&self, frame: &mut Frame) {
		let area = frame.area();
		let vertical =
			Layout::vertical([Constraint::Length(4), Constraint::Min(10), Constraint::Length(5)])
				.split(area);
		let main = Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)])
			.split(vertical[1]);
		frame.render_widget(self.header(), vertical[0]);
		frame.render_widget(self.activity_panel(), main[0]);
		frame.render_widget(self.pixel_panel(), main[1]);
		frame.render_widget(self.footer(), vertical[2]);
		let cursor_area = input_area(vertical[2]);
		if cursor_area.width > 0 {
			let cursor_x = cursor_area.x.saturating_add(self.input.chars().count() as u16);
			frame
				.set_cursor_position((cursor_x.min(cursor_area.right().saturating_sub(1)), cursor_area.y));
		}
	}

	fn header(&self) -> Paragraph<'static> {
		Paragraph::new(vec![
			Line::from(vec![
				Span::styled(
					"GNative Pixel Demo",
					Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
				),
				Span::raw("  "),
				Span::raw(format!("viewport={}x{}", self.size.width, self.size.height)),
				Span::raw("  "),
				Span::styled(
					format!("frame={} fps={:.1}", self.frame_count, self.fps),
					Style::default().fg(Color::Green),
				),
			]),
			Line::from("Input is read on a dedicated thread; rendering keeps its own frame loop."),
			Line::from("Type 'animation' and press Enter to play the full-GShell pixel animation."),
		])
		.block(Block::default().borders(Borders::ALL).title("Status"))
	}

	fn activity_panel(&self) -> Paragraph<'static> {
		let mut lines = Vec::new();
		lines.push(Line::from(Span::styled(
			"Submitted lines",
			Style::default().add_modifier(Modifier::BOLD),
		)));
		if self.submissions.is_empty() {
			lines.push(Line::from("  <type animation + Enter>"));
		} else {
			for line in &self.submissions {
				lines.push(Line::from(format!("  {line}")));
			}
		}
		lines.push(Line::default());
		lines.push(Line::from(Span::styled(
			"Recent events",
			Style::default().add_modifier(Modifier::BOLD),
		)));
		if self.events.is_empty() {
			lines.push(Line::from("  <waiting>"));
		} else {
			for event in &self.events {
				lines.push(Line::from(format!("  {event}")));
			}
		}
		Paragraph::new(lines)
			.block(Block::default().borders(Borders::ALL).title("Activity"))
			.wrap(Wrap { trim: false })
	}

	fn pixel_panel(&self) -> Paragraph<'static> {
		let status = if self.is_animating() { "playing" } else { "idle" };
		Paragraph::new(vec![
			Line::from(format!("Pixel animation status: {status}")),
			Line::from("The card/glow/beam are RenderCommandDto::PixelFillRect commands."),
			Line::from("The pixel layer is scaled to the full GShell viewport."),
		])
		.block(Block::default().borders(Borders::ALL).title("Pixel Layer"))
	}

	fn footer(&self) -> Paragraph<'static> {
		Paragraph::new(vec![
			Line::from("Current input"),
			Line::from(self.input.clone().yellow()),
			Line::from("Enter submits. Type animation to play. q exits."),
		])
		.block(Block::default().borders(Borders::ALL).title("Composer"))
	}
}

fn fill(
	x_px: u32,
	y_px: u32,
	width_px: u32,
	height_px: u32,
	color: RgbaColorDto,
) -> RenderCommandDto {
	RenderCommandDto::PixelFillRect { x_px, y_px, width_px, height_px, color }
}
fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> RgbaColorDto {
	RgbaColorDto::new(red, green, blue, alpha)
}
fn input_area(area: Rect) -> Rect {
	Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), 1)
}
