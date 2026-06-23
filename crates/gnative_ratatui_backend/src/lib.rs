use std::io;

use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_ports::{
	gnative::frame::{GNativeFrame, GNativeFrameCursor},
	rendering::frame_plan_builder::{RenderCommandDto, RgbColorDto, TextStyleDto},
	seq::Seq,
};
use ratatui_core::{
	backend::{Backend, ClearType, WindowSize},
	buffer::Cell,
	layout::{Position, Size},
	style::{Color, Modifier},
};

pub struct GerminalBackend<F>
where F: FnMut(GNativeFrame) -> io::Result<()>
{
	gshell_id:       GShellId,
	size:            Size,
	cursor_position: Position,
	cursor_visible:  bool,
	frame_seq:       u64,
	cells:           Vec<Cell>,
	pixel_commands:  Vec<RenderCommandDto>,
	emit_frame:      F,
}

impl<F> GerminalBackend<F>
where F: FnMut(GNativeFrame) -> io::Result<()>
{
	pub fn new(gshell_id: GShellId, size: Size, emit_frame: F) -> Self {
		let cell_count = usize::from(size.width) * usize::from(size.height);
		Self {
			gshell_id,
			size,
			cursor_position: Position::ORIGIN,
			cursor_visible: false,
			frame_seq: 0,
			cells: vec![Cell::default(); cell_count],
			pixel_commands: Vec::new(),
			emit_frame,
		}
	}

	pub fn resize(&mut self, size: Size) {
		if self.size == size {
			return;
		}
		self.size = size;
		self.cursor_position = Position::ORIGIN;
		self.cells = vec![Cell::default(); usize::from(size.width) * usize::from(size.height)];
	}

	pub fn set_pixel_commands(&mut self, commands: Vec<RenderCommandDto>) {
		self.pixel_commands = commands;
	}

	fn index_of(&self, x: u16, y: u16) -> Option<usize> {
		if x >= self.size.width || y >= self.size.height {
			return None;
		}
		Some(usize::from(y) * usize::from(self.size.width) + usize::from(x))
	}

	fn emit_current_frame(&mut self) -> io::Result<()> {
		self.frame_seq += 1;
		let mut commands = build_frame_commands(self.size, &self.cells);
		commands.extend(self.pixel_commands.iter().cloned());
		let frame = GNativeFrame {
			gshell_id: self.gshell_id,
			seq: Seq::new(self.frame_seq),
			commands,
			cursor: self.cursor_visible.then(|| GNativeFrameCursor {
				x: u32::from(self.cursor_position.x),
				y: u32::from(self.cursor_position.y),
			}),
		};
		(self.emit_frame)(frame)
	}
}

impl<F> Backend for GerminalBackend<F>
where F: FnMut(GNativeFrame) -> io::Result<()>
{
	type Error = io::Error;

	fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
	where I: Iterator<Item = (u16, u16, &'a Cell)> {
		for (x, y, cell) in content {
			let Some(index) = self.index_of(x, y) else {
				continue;
			};
			self.cells[index] = cell.clone();
		}
		Ok(())
	}

	fn hide_cursor(&mut self) -> Result<(), Self::Error> {
		self.cursor_visible = false;
		Ok(())
	}

	fn show_cursor(&mut self) -> Result<(), Self::Error> {
		self.cursor_visible = true;
		Ok(())
	}

	fn get_cursor_position(&mut self) -> Result<Position, Self::Error> { Ok(self.cursor_position) }

	fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
		self.cursor_position = position.into();
		Ok(())
	}

	fn clear(&mut self) -> Result<(), Self::Error> {
		for cell in &mut self.cells {
			cell.reset();
		}
		Ok(())
	}

	fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
		match clear_type {
			ClearType::All => self.clear(),
			ClearType::CurrentLine => {
				for x in 0..self.size.width {
					if let Some(index) = self.index_of(x, self.cursor_position.y) {
						self.cells[index].reset();
					}
				}
				Ok(())
			}
			ClearType::UntilNewLine => {
				for x in self.cursor_position.x..self.size.width {
					if let Some(index) = self.index_of(x, self.cursor_position.y) {
						self.cells[index].reset();
					}
				}
				Ok(())
			}
			ClearType::AfterCursor => {
				for y in self.cursor_position.y..self.size.height {
					let start_x = if y == self.cursor_position.y { self.cursor_position.x } else { 0 };
					for x in start_x..self.size.width {
						if let Some(index) = self.index_of(x, y) {
							self.cells[index].reset();
						}
					}
				}
				Ok(())
			}
			ClearType::BeforeCursor => {
				for y in 0..=self.cursor_position.y {
					let end_x = if y == self.cursor_position.y {
						self.cursor_position.x
					} else {
						self.size.width.saturating_sub(1)
					};
					for x in 0..=end_x {
						if let Some(index) = self.index_of(x, y) {
							self.cells[index].reset();
						}
					}
				}
				Ok(())
			}
		}
	}

	fn size(&self) -> Result<Size, Self::Error> { Ok(self.size) }

	fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
		Ok(WindowSize { columns_rows: self.size, pixels: Size::new(0, 0) })
	}

	fn flush(&mut self) -> Result<(), Self::Error> { self.emit_current_frame() }
}

fn build_frame_commands(size: Size, cells: &[Cell]) -> Vec<RenderCommandDto> {
	let mut commands = vec![RenderCommandDto::Clear];
	for y in 0..size.height {
		let mut last_relevant_x = None;
		for x in 0..size.width {
			let index = usize::from(y) * usize::from(size.width) + usize::from(x);
			if is_relevant_cell(&cells[index]) {
				last_relevant_x = Some(x);
			}
		}
		let Some(last_relevant_x) = last_relevant_x else {
			continue;
		};
		let mut run_start_x = 0u16;
		let mut run_text = String::new();
		let mut run_style = text_style_of(&cells[usize::from(y) * usize::from(size.width)]);
		for x in 0..=last_relevant_x {
			let index = usize::from(y) * usize::from(size.width) + usize::from(x);
			let cell = &cells[index];
			let cell_style = text_style_of(cell);
			let cell_symbol = cell.symbol();
			if run_text.is_empty() {
				run_start_x = x;
				run_style = cell_style;
			} else if cell_style != run_style {
				commands.push(RenderCommandDto::StyledTextRun {
					x:     u32::from(run_start_x),
					y:     u32::from(y),
					text:  std::mem::take(&mut run_text),
					style: run_style,
				});
				run_start_x = x;
				run_style = cell_style;
			}
			run_text.push_str(cell_symbol);
		}
		if !run_text.is_empty() {
			commands.push(RenderCommandDto::StyledTextRun {
				x:     u32::from(run_start_x),
				y:     u32::from(y),
				text:  run_text,
				style: run_style,
			});
		}
	}
	commands
}

fn is_relevant_cell(cell: &Cell) -> bool {
	cell.symbol() != " " || text_style_of(cell) != TextStyleDto::plain()
}

fn text_style_of(cell: &Cell) -> TextStyleDto {
	TextStyleDto {
		foreground: color_of(cell.fg),
		background: color_of(cell.bg),
		bold:       cell.modifier.contains(Modifier::BOLD),
		italic:     cell.modifier.contains(Modifier::ITALIC),
		underline:  cell.modifier.contains(Modifier::UNDERLINED),
	}
}

fn color_of(color: Color) -> Option<RgbColorDto> {
	match color {
		Color::Reset => None,
		Color::Black => Some(RgbColorDto::new(0, 0, 0)),
		Color::Red => Some(RgbColorDto::new(0xCD, 0, 0)),
		Color::Green => Some(RgbColorDto::new(0, 0xCD, 0)),
		Color::Yellow => Some(RgbColorDto::new(0xCD, 0xCD, 0)),
		Color::Blue => Some(RgbColorDto::new(0, 0, 0xEE)),
		Color::Magenta => Some(RgbColorDto::new(0xCD, 0, 0xCD)),
		Color::Cyan => Some(RgbColorDto::new(0, 0xCD, 0xCD)),
		Color::Gray => Some(RgbColorDto::new(0xE5, 0xE5, 0xE5)),
		Color::DarkGray => Some(RgbColorDto::new(0x7F, 0x7F, 0x7F)),
		Color::LightRed => Some(RgbColorDto::new(255, 0, 0)),
		Color::LightGreen => Some(RgbColorDto::new(0, 255, 0)),
		Color::LightYellow => Some(RgbColorDto::new(255, 255, 0)),
		Color::LightBlue => Some(RgbColorDto::new(0x5C, 0x5C, 255)),
		Color::LightMagenta => Some(RgbColorDto::new(255, 0, 255)),
		Color::LightCyan => Some(RgbColorDto::new(0, 255, 255)),
		Color::White => Some(RgbColorDto::new(255, 255, 255)),
		Color::Rgb(red, green, blue) => Some(RgbColorDto::new(red, green, blue)),
		Color::Indexed(index) => Some(indexed_rgb(index)),
	}
}

fn indexed_rgb(index: u8) -> RgbColorDto {
	if index < 16 {
		return color_of(match index {
			0 => Color::Black,
			1 => Color::Red,
			2 => Color::Green,
			3 => Color::Yellow,
			4 => Color::Blue,
			5 => Color::Magenta,
			6 => Color::Cyan,
			7 => Color::Gray,
			8 => Color::DarkGray,
			9 => Color::LightRed,
			10 => Color::LightGreen,
			11 => Color::LightYellow,
			12 => Color::LightBlue,
			13 => Color::LightMagenta,
			14 => Color::LightCyan,
			_ => Color::White,
		})
		.expect("indexed ANSI color should map");
	}
	if index >= 232 {
		let level = 8 + (index - 232) * 10;
		return RgbColorDto::new(level, level, level);
	}
	let color = index - 16;
	let red = color / 36;
	let green = (color % 36) / 6;
	let blue = color % 6;
	let channel = |value: u8| if value == 0 { 0 } else { value * 40 + 55 };
	RgbColorDto::new(channel(red), channel(green), channel(blue))
}
