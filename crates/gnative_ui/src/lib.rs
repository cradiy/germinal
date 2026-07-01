use germinal_ports::rendering::frame_plan_builder::{
	RenderCommandDto, RgbColorDto, RgbaColorDto, TextStyleDto,
};

const CELL_WIDTH_PX: u32 = 8;
const CELL_HEIGHT_PX: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pixels(f32);

pub const fn px(value: f32) -> Pixels { Pixels(value) }

pub const fn rgb(red: u8, green: u8, blue: u8) -> RgbColorDto { RgbColorDto::new(red, green, blue) }

pub const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> RgbaColorDto {
	RgbaColorDto::new(red, green, blue, alpha)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
	pub columns: u16,
	pub rows:    u16,
}

impl GridSize {
	pub const fn new(columns: u16, rows: u16) -> Self { Self { columns, rows } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridPoint {
	pub x: u32,
	pub y: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledUi {
	pub commands: Vec<RenderCommandDto>,
	pub cursor:   Option<GridPoint>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiTree {
	pub root: Element,
}

impl UiTree {
	pub fn new(root: impl IntoElementNode) -> Self { Self { root: root.into_element() } }

	pub fn compile(&self, viewport: GridSize) -> CompiledUi {
		let mut state = RenderState { commands: vec![RenderCommandDto::Clear], cursor: None };
		render_element(&self.root, Rect::from(viewport), &mut state, TextStyleDto::plain());
		CompiledUi { commands: state.commands, cursor: state.cursor }
	}
}

pub trait IntoElementNode {
	fn into_element(self) -> Element;
}

pub trait IntoDivChild {
	fn into_child(self, inherited_style: TextStyleDto) -> Element;
}

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
	Div(Div),
	Text(Text),
	Input(Input),
}

impl IntoElementNode for Element {
	fn into_element(self) -> Element { self }
}

impl IntoElementNode for Div {
	fn into_element(self) -> Element { Element::Div(self) }
}

impl IntoDivChild for Element {
	fn into_child(self, _inherited_style: TextStyleDto) -> Element { self }
}

impl IntoDivChild for Div {
	fn into_child(self, _inherited_style: TextStyleDto) -> Element { Element::Div(self) }
}

impl IntoDivChild for String {
	fn into_child(self, inherited_style: TextStyleDto) -> Element {
		Element::Text(Text { content: self, style: inherited_style })
	}
}

impl IntoDivChild for &str {
	fn into_child(self, inherited_style: TextStyleDto) -> Element {
		self.to_string().into_child(inherited_style)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Display {
	Block,
	FlexRow,
	FlexCol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositionMode {
	Static,
	Absolute,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Div {
	children:    Vec<Element>,
	display:     Display,
	gap:         u16,
	padding:     u16,
	border:      bool,
	text_style:  TextStyleDto,
	background:  Option<RgbaColorDto>,
	width:       Option<Pixels>,
	height:      Option<Pixels>,
	fill_width:  bool,
	fill_height: bool,
	flex_grow:   u16,
	position:    PositionMode,
	left:        Option<Pixels>,
	top:         Option<Pixels>,
}

pub fn div() -> Div { Div::new() }

pub fn v_flex() -> Div { Div::new().v_flex() }

pub fn h_flex() -> Div { Div::new().h_flex() }

pub fn text_input(value: impl Into<String>, focused: bool) -> Element {
	Element::Input(Input { value: value.into(), style: TextStyleDto::plain(), focused })
}

pub fn styled_text_input(value: impl Into<String>, focused: bool, style: TextStyleDto) -> Element {
	Element::Input(Input { value: value.into(), style, focused })
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupBox {
	title: Option<String>,
	child: Option<Element>,
}

impl GroupBox {
	pub fn new() -> Self { Self { title: None, child: None } }

	pub fn title(mut self, title: impl Into<String>) -> Self {
		self.title = Some(title.into());
		self
	}

	pub fn child(mut self, child: impl IntoElementNode) -> Self {
		self.child = Some(child.into_element());
		self
	}

	fn into_div(self) -> Div {
		let mut body = v_flex();
		if let Some(title) = self.title {
			body = body.child(div().text_color(rgb(210, 210, 210)).font_bold().child(title));
		}
		if let Some(child) = self.child {
			body = body.child(child);
		}
		div().border_1().child(body)
	}
}

impl IntoElementNode for GroupBox {
	fn into_element(self) -> Element { self.into_div().into_element() }
}

impl IntoDivChild for GroupBox {
	fn into_child(self, _inherited_style: TextStyleDto) -> Element { self.into_element() }
}

impl Div {
	pub fn new() -> Self {
		Self {
			children:    Vec::new(),
			display:     Display::Block,
			gap:         0,
			padding:     0,
			border:      false,
			text_style:  TextStyleDto::plain(),
			background:  None,
			width:       None,
			height:      None,
			fill_width:  false,
			fill_height: false,
			flex_grow:   0,
			position:    PositionMode::Static,
			left:        None,
			top:         None,
		}
	}

	pub fn flex(mut self) -> Self {
		if self.display == Display::Block {
			self.display = Display::FlexRow;
		}
		self
	}

	pub fn h_flex(self) -> Self { self.flex_row() }

	pub fn v_flex(self) -> Self { self.flex_col() }

	pub fn flex_row(mut self) -> Self {
		self.display = Display::FlexRow;
		self
	}

	pub fn flex_col(mut self) -> Self {
		self.display = Display::FlexCol;
		self
	}

	pub fn flex_1(mut self) -> Self {
		self.flex_grow = 1;
		self
	}

	pub fn size_full(mut self) -> Self {
		self.fill_width = true;
		self.fill_height = true;
		self
	}

	pub fn w_full(mut self) -> Self {
		self.fill_width = true;
		self
	}

	pub fn h_full(mut self) -> Self {
		self.fill_height = true;
		self
	}

	pub fn w(mut self, width: Pixels) -> Self {
		self.width = Some(width);
		self
	}

	pub fn h(mut self, height: Pixels) -> Self {
		self.height = Some(height);
		self
	}

	pub fn gap(mut self, gap: Pixels) -> Self {
		self.gap = cells_of(gap).max(0) as u16;
		self
	}

	pub fn gap_1(mut self) -> Self {
		self.gap = 1;
		self
	}

	pub fn gap_2(mut self) -> Self {
		self.gap = 2;
		self
	}

	pub fn gap_3(mut self) -> Self {
		self.gap = 3;
		self
	}

	pub fn gap_4(mut self) -> Self {
		self.gap = 4;
		self
	}

	pub fn children<I, T>(mut self, children: I) -> Self
	where
		I: IntoIterator<Item = T>,
		T: IntoDivChild,
	{
		for child in children {
			self.children.push(child.into_child(self.text_style));
		}
		self
	}

	pub fn p_1(mut self) -> Self {
		self.padding = 1;
		self
	}

	pub fn border_1(mut self) -> Self {
		self.border = true;
		self
	}

	pub fn bg(mut self, color: RgbaColorDto) -> Self {
		self.background = Some(color);
		self
	}

	pub fn text_color(mut self, color: RgbColorDto) -> Self {
		self.text_style.foreground = Some(color);
		self
	}

	pub fn font_bold(mut self) -> Self {
		self.text_style.bold = true;
		self
	}

	pub fn items_center(self) -> Self { self }

	pub fn justify_center(self) -> Self { self }

	pub fn justify_between(self) -> Self { self }

	pub fn absolute(mut self) -> Self {
		self.position = PositionMode::Absolute;
		self
	}

	pub fn left(mut self, value: Pixels) -> Self {
		self.left = Some(value);
		self
	}

	pub fn top(mut self, value: Pixels) -> Self {
		self.top = Some(value);
		self
	}

	pub fn child(mut self, child: impl IntoDivChild) -> Self {
		self.children.push(child.into_child(self.text_style));
		self
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct Text {
	pub content: String,
	pub style:   TextStyleDto,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Input {
	pub value:   String,
	pub style:   TextStyleDto,
	pub focused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
	x:      u16,
	y:      u16,
	width:  u16,
	height: u16,
}

impl Rect {
	fn from(size: GridSize) -> Self { Self { x: 0, y: 0, width: size.columns, height: size.rows } }

	fn inset(self, amount: u16) -> Self {
		let total = amount.saturating_mul(2);
		Self {
			x:      self.x.saturating_add(amount),
			y:      self.y.saturating_add(amount),
			width:  self.width.saturating_sub(total),
			height: self.height.saturating_sub(total),
		}
	}
}

#[derive(Debug, Default)]
struct RenderState {
	commands: Vec<RenderCommandDto>,
	cursor:   Option<GridPoint>,
}

fn render_element(
	element: &Element,
	rect: Rect,
	state: &mut RenderState,
	inherited_style: TextStyleDto,
) {
	if rect.width == 0 || rect.height == 0 {
		return;
	}

	match element {
		Element::Div(div) => render_div(div, rect, state, inherited_style),
		Element::Text(text) => render_text(text, rect, state, inherited_style),
		Element::Input(input) => render_input(input, rect, state, inherited_style),
	}
}

fn render_div(div: &Div, rect: Rect, state: &mut RenderState, inherited_style: TextStyleDto) {
	if div.position == PositionMode::Absolute {
		render_absolute_div(div, rect, state);
		return;
	}

	let mut content_rect = rect;
	let current_style = merge_styles(inherited_style, div.text_style);

	if let Some(color) = div.background {
		state.commands.push(RenderCommandDto::PixelFillRect {
			x_px: u32::from(rect.x) * CELL_WIDTH_PX,
			y_px: u32::from(rect.y) * CELL_HEIGHT_PX,
			width_px: u32::from(rect.width) * CELL_WIDTH_PX,
			height_px: u32::from(rect.height) * CELL_HEIGHT_PX,
			color,
		});
	}

	if div.border {
		render_border(rect, current_style, state);
		content_rect = content_rect.inset(1);
	}

	if div.padding > 0 {
		content_rect = content_rect.inset(div.padding);
	}

	match div.display {
		Display::Block => {
			for child in &div.children {
				render_element(child, content_rect, state, current_style);
			}
		}
		Display::FlexRow => {
			render_flex_children(div, content_rect, state, current_style, Axis::Horizontal)
		}
		Display::FlexCol => {
			render_flex_children(div, content_rect, state, current_style, Axis::Vertical)
		}
	}
}

fn render_absolute_div(div: &Div, rect: Rect, state: &mut RenderState) {
	let Some(color) = div.background else {
		return;
	};
	let x_px = div.left.map(pixels_of).unwrap_or(0);
	let y_px = div.top.map(pixels_of).unwrap_or(0);
	let width_px = div.width.map(pixels_of).unwrap_or(u32::from(rect.width) * CELL_WIDTH_PX);
	let height_px = div.height.map(pixels_of).unwrap_or(u32::from(rect.height) * CELL_HEIGHT_PX);
	if width_px == 0 || height_px == 0 {
		return;
	}

	state.commands.push(RenderCommandDto::PixelFillRect { x_px, y_px, width_px, height_px, color });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
	Horizontal,
	Vertical,
}

fn render_flex_children(
	div: &Div,
	rect: Rect,
	state: &mut RenderState,
	inherited_style: TextStyleDto,
	axis: Axis,
) {
	let flow_children: Vec<_> = div.children.iter().filter(|child| !is_absolute(child)).collect();
	let absolute_children: Vec<_> = div.children.iter().filter(|child| is_absolute(child)).collect();

	if flow_children.is_empty() && absolute_children.is_empty() {
		return;
	}

	let axis_extent = match axis {
		Axis::Horizontal => rect.width,
		Axis::Vertical => rect.height,
	};
	let gap_total = div.gap.saturating_mul(flow_children.len().saturating_sub(1) as u16);
	let available = axis_extent.saturating_sub(gap_total);

	let fixed_total: u16 = flow_children.iter().map(|child| child_fixed_extent(child, axis)).sum();
	let flex_total: u32 = flow_children.iter().map(|child| u32::from(child_flex_grow(child))).sum();
	let remaining = available.saturating_sub(fixed_total);

	let mut cursor = 0u16;
	let mut flex_consumed = 0u16;
	let mut flex_seen = 0u32;
	for child in flow_children {
		let extent = if child_flex_grow(child) > 0 && flex_total > 0 {
			let grow = u32::from(child_flex_grow(child));
			if flex_seen + grow >= flex_total {
				remaining.saturating_sub(flex_consumed)
			} else {
				let share = (u32::from(remaining) * grow / flex_total) as u16;
				flex_consumed = flex_consumed.saturating_add(share);
				flex_seen += grow;
				share
			}
		} else {
			child_fixed_extent(child, axis)
		};

		let child_rect = match axis {
			Axis::Horizontal => Rect {
				x:      rect.x.saturating_add(cursor),
				y:      rect.y,
				width:  extent.min(rect.width.saturating_sub(cursor)),
				height: rect.height,
			},
			Axis::Vertical => Rect {
				x:      rect.x,
				y:      rect.y.saturating_add(cursor),
				width:  rect.width,
				height: extent.min(rect.height.saturating_sub(cursor)),
			},
		};
		render_element(child, child_rect, state, inherited_style);
		cursor = cursor.saturating_add(extent).saturating_add(div.gap);
	}

	for child in absolute_children {
		render_element(child, rect, state, inherited_style);
	}
}

fn render_border(rect: Rect, style: TextStyleDto, state: &mut RenderState) {
	if rect.width < 2 || rect.height < 2 {
		return;
	}

	state.commands.push(RenderCommandDto::StyledTextRun {
		x: u32::from(rect.x),
		y: u32::from(rect.y),
		text: border_line(rect.width),
		style,
	});

	for row in 0..rect.height.saturating_sub(2) {
		state.commands.push(RenderCommandDto::StyledTextRun {
			x: u32::from(rect.x),
			y: u32::from(rect.y + row + 1),
			text: side_border(rect.width),
			style,
		});
	}

	state.commands.push(RenderCommandDto::StyledTextRun {
		x: u32::from(rect.x),
		y: u32::from(rect.y + rect.height - 1),
		text: border_line(rect.width),
		style,
	});
}

fn render_text(text: &Text, rect: Rect, state: &mut RenderState, inherited_style: TextStyleDto) {
	let style = merge_styles(inherited_style, text.style);
	for (row, line) in text.content.lines().take(rect.height as usize).enumerate() {
		let clipped = clip_line(line, rect.width);
		if clipped.is_empty() {
			continue;
		}
		state.commands.push(RenderCommandDto::StyledTextRun {
			x: u32::from(rect.x),
			y: u32::from(rect.y + row as u16),
			text: clipped,
			style,
		});
	}
}

fn render_input(input: &Input, rect: Rect, state: &mut RenderState, inherited_style: TextStyleDto) {
	let style = merge_styles(inherited_style, input.style);
	let clipped = clip_line(&input.value, rect.width);
	if !clipped.is_empty() {
		state.commands.push(RenderCommandDto::StyledTextRun {
			x: u32::from(rect.x),
			y: u32::from(rect.y),
			text: clipped.clone(),
			style,
		});
	}

	if input.focused {
		let cursor_x = rect
			.x
			.saturating_add(clipped.chars().count() as u16)
			.min(rect.x.saturating_add(rect.width.saturating_sub(1)));
		state.cursor = Some(GridPoint { x: u32::from(cursor_x), y: u32::from(rect.y) });
	}
}

fn merge_styles(parent: TextStyleDto, child: TextStyleDto) -> TextStyleDto {
	TextStyleDto {
		foreground: child.foreground.or(parent.foreground),
		background: child.background.or(parent.background),
		bold:       parent.bold || child.bold,
		italic:     parent.italic || child.italic,
		underline:  parent.underline || child.underline,
	}
}

fn clip_line(line: &str, max_width: u16) -> String {
	line.chars().take(max_width as usize).collect()
}

fn border_line(width: u16) -> String {
	if width <= 1 {
		return "+".to_string();
	}
	format!("+{}+", "-".repeat(width.saturating_sub(2) as usize))
}

fn side_border(width: u16) -> String {
	if width <= 1 {
		return "|".to_string();
	}
	if width == 2 {
		return "||".to_string();
	}
	format!("|{}|", " ".repeat(width.saturating_sub(2) as usize))
}

fn pixels_of(length: Pixels) -> u32 { length.0.max(0.0).round() as u32 }

fn cells_of(length: Pixels) -> i32 { length.0.max(0.0).round() as i32 }

fn child_fixed_extent(child: &Element, axis: Axis) -> u16 {
	match child {
		Element::Div(div) => match axis {
			Axis::Horizontal => div.width.map(cells_of).unwrap_or(1).max(0) as u16,
			Axis::Vertical => div.height.map(cells_of).unwrap_or(1).max(0) as u16,
		},
		Element::Text(_) | Element::Input(_) => 1,
	}
}

fn child_flex_grow(child: &Element) -> u16 {
	match child {
		Element::Div(div) => div.flex_grow,
		Element::Text(_) | Element::Input(_) => 0,
	}
}

fn is_absolute(child: &Element) -> bool {
	matches!(child, Element::Div(div) if div.position == PositionMode::Absolute)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn gpui_like_flex_tree_compiles_to_text_commands() {
		let tree = UiTree::new(
			div()
				.flex()
				.flex_col()
				.child(div().h(px(1.0)).text_color(rgb(1, 2, 3)).font_bold().child("header"))
				.child(div().h(px(1.0)).child("body")),
		);

		let compiled = tree.compile(GridSize::new(16, 8));

		assert_eq!(compiled.commands[0], RenderCommandDto::Clear);
		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { text, style, .. }
				if text == "header"
					&& style.foreground == Some(rgb(1, 2, 3))
					&& style.bold
		)));
	}

	#[test]
	fn focused_text_input_sets_cursor_position() {
		let tree = UiTree::new(
			div()
				.flex()
				.flex_col()
				.child(div().h(px(1.0)).child("Current input"))
				.child(div().h(px(1.0)).child(text_input("abc", true))),
		);

		let compiled = tree.compile(GridSize::new(20, 5));

		assert_eq!(compiled.cursor, Some(GridPoint { x: 3, y: 1 }));
	}

	#[test]
	fn absolute_background_div_compiles_to_pixel_command() {
		let tree = UiTree::new(div().size_full().child(
			div().absolute().left(px(10.0)).top(px(20.0)).w(px(30.0)).h(px(40.0)).bg(rgba(1, 2, 3, 4)),
		));

		let compiled = tree.compile(GridSize::new(20, 5));

		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::PixelFillRect { x_px, y_px, width_px, height_px, color }
				if *x_px == 10
					&& *y_px == 20
					&& *width_px == 30
					&& *height_px == 40
					&& *color == rgba(1, 2, 3, 4)
		)));
	}
}
