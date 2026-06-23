use std::{
	collections::VecDeque,
	io::{self, stdout},
};

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use germinal_gnative_ratatui_backend::GerminalBackend;
use germinal_gnative_sdk::{
	input::{UnsupportedCrosstermEvent, try_to_crossterm_event},
	local_session::LocalGNativeBootstrap,
};
use germinal_ports::gnative::{frame::GNativeFrame, input::GNativeInputEvent};
use ratatui::{
	Frame, Terminal,
	layout::{Constraint, Layout, Rect, Size},
	style::{Color, Modifier, Style, Stylize},
	text::{Line, Span},
	widgets::{Block, Borders, Paragraph, Wrap},
};

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
	let mut pending_inputs = VecDeque::new();

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
	terminal.draw(|frame| app.render(frame))?;

	while let Some(input) = session.read_input()? {
		pending_inputs.push_back(input);
		while let Some(input) = pending_inputs.pop_front() {
			handle_input(&mut app, &mut terminal, input)?;
			terminal.draw(|frame| app.render(frame))?;
			if app.should_quit {
				session.send_exit()?;
				return Ok(());
			}
		}
	}

	Ok(())
}

fn wait_for_initial_size(
	session: &mut germinal_gnative_sdk::local_session::LocalGNativeSession,
) -> Result<Size, String> {
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
					app.should_quit = true;
				}
				KeyCode::Char(ch)
					if !key.modifiers.contains(KeyModifiers::CONTROL)
						&& !key.modifiers.contains(KeyModifiers::ALT) =>
				{
					app.input.push(ch);
				}
				KeyCode::Backspace => {
					app.input.pop();
				}
				KeyCode::Enter => {
					app.submissions.push_front(app.input.clone());
					if app.submissions.len() > 6 {
						app.submissions.pop_back();
					}
					app.input.clear();
				}
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

struct DemoApp {
	size:        Size,
	input:       String,
	submissions: VecDeque<String>,
	events:      VecDeque<String>,
	should_quit: bool,
}

impl DemoApp {
	fn new(size: Size) -> Self {
		Self {
			size,
			input: String::new(),
			submissions: VecDeque::new(),
			events: VecDeque::new(),
			should_quit: false,
		}
	}

	fn push_event(&mut self, event: String) {
		self.events.push_front(event);
		if self.events.len() > 8 {
			self.events.pop_back();
		}
	}

	fn render(&self, frame: &mut Frame) {
		let area = frame.area();
		let vertical =
			Layout::vertical([Constraint::Length(4), Constraint::Min(8), Constraint::Length(5)])
				.split(area);

		frame.render_widget(self.header(), vertical[0]);
		frame.render_widget(self.main_panel(), vertical[1]);
		frame.render_widget(self.footer(), vertical[2]);

		let cursor_area = input_area(vertical[2]);
		let cursor_x = cursor_area.x.saturating_add(self.input.chars().count() as u16);
		frame.set_cursor_position((cursor_x.min(cursor_area.right().saturating_sub(1)), cursor_area.y));
	}

	fn header(&self) -> Paragraph<'static> {
		let lines = vec![
			Line::from(vec![
				Span::styled("GNative Demo", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
				Span::raw("  "),
				Span::raw(format!("viewport={}x{}", self.size.width, self.size.height)),
			]),
			Line::from("typed input is rendered through ratatui -> GNativeFrame -> Germinal"),
			Line::from("press q or Ctrl+C to exit the demo process"),
		];

		Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Status"))
	}

	fn main_panel(&self) -> Paragraph<'static> {
		let mut lines = Vec::new();
		lines.push(Line::from(Span::styled(
			"Submitted lines",
			Style::default().add_modifier(Modifier::BOLD),
		)));
		if self.submissions.is_empty() {
			lines.push(Line::from("  <empty>"));
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

	fn footer(&self) -> Paragraph<'static> {
		let lines = vec![
			Line::from("Current input"),
			Line::from(self.input.clone().yellow()),
			Line::from("Enter submits the line back into the activity panel."),
		];

		Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Composer"))
	}
}

fn input_area(area: Rect) -> Rect {
	Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), 1)
}

#[cfg(test)]
mod tests {
	use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
	use ratatui::layout::Size;

	use super::{DemoApp, describe_key_event};

	#[test]
	fn push_event_keeps_recent_entries() {
		let mut app = DemoApp::new(Size::new(80, 24));
		for index in 0..10 {
			app.push_event(format!("event-{index}"));
		}

		assert_eq!(app.events.len(), 8);
		assert_eq!(app.events.front().map(String::as_str), Some("event-9"));
		assert_eq!(app.events.back().map(String::as_str), Some("event-2"));
	}

	#[test]
	fn describe_key_event_includes_modifiers() {
		let key =
			KeyEvent::new_with_kind(KeyCode::Char('c'), KeyModifiers::CONTROL, KeyEventKind::Press);

		assert_eq!(describe_key_event(&key), "CTRL+c");
	}

	#[test]
	fn event_type_is_available_for_demo_state_tests() {
		assert!(matches!(Event::Resize(100, 40), Event::Resize(100, 40)));
	}
}
