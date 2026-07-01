pub use germinal_gnative_protocol::rendering::frame_plan_builder::{
	RenderCommandDto, RgbColorDto, RgbaColorDto, TextStyleDto,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
pub struct LayoutTree {
	pub viewport: GridSize,
	pub root:     LayoutNode,
	pub cursor:   Option<GridPoint>,
}

impl LayoutTree {
	pub fn render(&self) -> CompiledUi {
		let mut commands = vec![RenderCommandDto::Clear];
		render_layout_node(&self.root, &mut commands);
		CompiledUi { commands, cursor: self.cursor }
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutNode {
	pub grid_rect: GridRect,
	pub paints:    Vec<LayoutPaint>,
	pub children:  Vec<LayoutNode>,
}

impl LayoutNode {
	fn new(rect: Rect) -> Self {
		Self { grid_rect: rect.into(), paints: Vec::new(), children: Vec::new() }
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridRect {
	pub x:      u16,
	pub y:      u16,
	pub width:  u16,
	pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
	pub x_px:      u32,
	pub y_px:      u32,
	pub width_px:  u32,
	pub height_px: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LayoutPaint {
	FillRect { rect: PixelRect, color: RgbaColorDto },
	TextRun { origin: GridPoint, text: String, style: TextStyleDto },
}

#[derive(Debug, Clone, PartialEq)]
pub struct UiTree {
	pub root: Element,
}

impl UiTree {
	pub fn new(root: impl IntoElementNode) -> Self { Self { root: root.into_element() } }

	pub fn layout(&self, viewport: GridSize) -> LayoutTree {
		let mut state = LayoutState { cursor: None };
		let root = layout_element(&self.root, Rect::from(viewport), &mut state, TextStyleDto::plain());
		LayoutTree { viewport, root, cursor: state.cursor }
	}

	pub fn compile(&self, viewport: GridSize) -> CompiledUi { self.layout(viewport).render() }
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
	Input(InputElement),
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

pub type Anchored = Div;
pub type Animation = Div;
pub type Canvas = Div;
pub type Deferred = Div;
pub type Img = Div;
pub type List = Div;
pub type Surface = Div;
pub type Svg = Div;
pub type UniformList = Div;

pub fn anchored() -> Anchored { div() }

pub fn animation() -> Animation { div() }

pub fn canvas() -> Canvas { div() }

pub fn deferred() -> Deferred { div() }

pub fn img() -> Img { div() }

pub fn list() -> List { div() }

pub fn surface() -> Surface { div() }

pub fn svg() -> Svg { div() }

pub fn uniform_list() -> UniformList { div() }

pub fn v_flex() -> Div { Div::new().v_flex() }

pub fn h_flex() -> Div { Div::new().h_flex() }

pub fn text_input(value: impl Into<String>, focused: bool) -> Element {
	Element::Input(InputElement { value: value.into(), style: TextStyleDto::plain(), focused })
}

pub fn styled_text_input(value: impl Into<String>, focused: bool, style: TextStyleDto) -> Element {
	Element::Input(InputElement { value: value.into(), style, focused })
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

impl Text {
	pub fn new(content: impl Into<String>) -> Self {
		Self { content: content.into(), style: TextStyleDto::plain() }
	}

	pub fn text_color(mut self, color: RgbColorDto) -> Self {
		self.style.foreground = Some(color);
		self
	}

	pub fn font_bold(mut self) -> Self {
		self.style.bold = true;
		self
	}
}

pub fn text(content: impl Into<String>) -> Text { Text::new(content) }

impl IntoElementNode for Text {
	fn into_element(self) -> Element { Element::Text(self) }
}

impl IntoDivChild for Text {
	fn into_child(self, inherited_style: TextStyleDto) -> Element {
		let mut element = self.into_element();
		if let Element::Text(text) = &mut element {
			text.style = merge_styles(inherited_style, text.style);
		}
		element
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputElement {
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

	fn pixel_rect(self) -> PixelRect {
		PixelRect {
			x_px:      u32::from(self.x) * CELL_WIDTH_PX,
			y_px:      u32::from(self.y) * CELL_HEIGHT_PX,
			width_px:  u32::from(self.width) * CELL_WIDTH_PX,
			height_px: u32::from(self.height) * CELL_HEIGHT_PX,
		}
	}

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

impl From<Rect> for GridRect {
	fn from(value: Rect) -> Self {
		Self { x: value.x, y: value.y, width: value.width, height: value.height }
	}
}

#[derive(Debug, Default)]
struct LayoutState {
	cursor: Option<GridPoint>,
}

fn render_layout_node(node: &LayoutNode, commands: &mut Vec<RenderCommandDto>) {
	for paint in &node.paints {
		match paint {
			LayoutPaint::FillRect { rect, color } => commands.push(RenderCommandDto::PixelFillRect {
				x_px:      rect.x_px,
				y_px:      rect.y_px,
				width_px:  rect.width_px,
				height_px: rect.height_px,
				color:     *color,
			}),
			LayoutPaint::TextRun { origin, text, style } => {
				commands.push(RenderCommandDto::StyledTextRun {
					x:     origin.x,
					y:     origin.y,
					text:  text.clone(),
					style: *style,
				})
			}
		}
	}

	for child in &node.children {
		render_layout_node(child, commands);
	}
}

fn layout_element(
	element: &Element,
	rect: Rect,
	state: &mut LayoutState,
	inherited_style: TextStyleDto,
) -> LayoutNode {
	let node = LayoutNode::new(rect);
	if rect.width == 0 || rect.height == 0 {
		return node;
	}

	match element {
		Element::Div(div) => layout_div(div, rect, state, inherited_style),
		Element::Text(text) => layout_text(text, rect, inherited_style),
		Element::Input(input) => layout_input(input, rect, state, inherited_style),
	}
}

fn layout_div(
	div: &Div,
	rect: Rect,
	state: &mut LayoutState,
	inherited_style: TextStyleDto,
) -> LayoutNode {
	if div.position == PositionMode::Absolute {
		return layout_absolute_div(div, rect);
	}

	let mut node = LayoutNode::new(rect);
	let mut content_rect = rect;
	let current_style = merge_styles(inherited_style, div.text_style);

	if let Some(color) = div.background {
		node.paints.push(LayoutPaint::FillRect { rect: rect.pixel_rect(), color });
	}

	if div.border {
		layout_border(rect, current_style, &mut node.paints);
	}

	if div.padding > 0 {
		content_rect = content_rect.inset(div.padding);
	}

	match div.display {
		Display::Block => {
			for child in &div.children {
				node.children.push(layout_element(child, content_rect, state, current_style));
			}
		}
		Display::FlexRow => layout_flex_children(
			div,
			content_rect,
			&mut node.children,
			state,
			current_style,
			Axis::Horizontal,
		),
		Display::FlexCol => layout_flex_children(
			div,
			content_rect,
			&mut node.children,
			state,
			current_style,
			Axis::Vertical,
		),
	}

	node
}

fn layout_absolute_div(div: &Div, rect: Rect) -> LayoutNode {
	let mut node = LayoutNode::new(rect);
	let Some(color) = div.background else {
		return node;
	};
	let x_px = div.left.map(pixels_of).unwrap_or(0);
	let y_px = div.top.map(pixels_of).unwrap_or(0);
	let width_px = div.width.map(pixels_of).unwrap_or(u32::from(rect.width) * CELL_WIDTH_PX);
	let height_px = div.height.map(pixels_of).unwrap_or(u32::from(rect.height) * CELL_HEIGHT_PX);
	if width_px == 0 || height_px == 0 {
		return node;
	}

	node
		.paints
		.push(LayoutPaint::FillRect { rect: PixelRect { x_px, y_px, width_px, height_px }, color });
	node
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
	Horizontal,
	Vertical,
}

fn layout_flex_children(
	div: &Div,
	rect: Rect,
	children: &mut Vec<LayoutNode>,
	state: &mut LayoutState,
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

	let fixed_total: u16 = flow_children
		.iter()
		.filter(|child| child_flex_grow(child) == 0)
		.map(|child| child_fixed_extent(child, axis))
		.sum();
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
		children.push(layout_element(child, child_rect, state, inherited_style));
		cursor = cursor.saturating_add(extent).saturating_add(div.gap);
	}

	for child in absolute_children {
		children.push(layout_element(child, rect, state, inherited_style));
	}
}

fn layout_border(rect: Rect, style: TextStyleDto, paints: &mut Vec<LayoutPaint>) {
	if rect.width < 2 || rect.height < 2 {
		return;
	}
	let color = style
		.foreground
		.map(|color| RgbaColorDto::opaque(color.red, color.green, color.blue))
		.unwrap_or_else(|| rgba(210, 210, 210, 255));
	let rect_px = rect.pixel_rect();

	paints.push(LayoutPaint::FillRect {
		rect: PixelRect {
			x_px:      rect_px.x_px,
			y_px:      rect_px.y_px,
			width_px:  rect_px.width_px,
			height_px: 1,
		},
		color,
	});
	paints.push(LayoutPaint::FillRect {
		rect: PixelRect {
			x_px:      rect_px.x_px,
			y_px:      rect_px.y_px + rect_px.height_px.saturating_sub(1),
			width_px:  rect_px.width_px,
			height_px: 1,
		},
		color,
	});
	paints.push(LayoutPaint::FillRect {
		rect: PixelRect {
			x_px:      rect_px.x_px,
			y_px:      rect_px.y_px,
			width_px:  1,
			height_px: rect_px.height_px,
		},
		color,
	});
	paints.push(LayoutPaint::FillRect {
		rect: PixelRect {
			x_px:      rect_px.x_px + rect_px.width_px.saturating_sub(1),
			y_px:      rect_px.y_px,
			width_px:  1,
			height_px: rect_px.height_px,
		},
		color,
	});
}

fn layout_text(text: &Text, rect: Rect, inherited_style: TextStyleDto) -> LayoutNode {
	let mut node = LayoutNode::new(rect);
	let style = merge_styles(inherited_style, text.style);
	for (row, line) in text.content.lines().take(rect.height as usize).enumerate() {
		let clipped = clip_line(line, rect.width);
		if clipped.is_empty() {
			continue;
		}
		node.paints.push(LayoutPaint::TextRun {
			origin: GridPoint { x: u32::from(rect.x), y: u32::from(rect.y + row as u16) },
			text: clipped,
			style,
		});
	}
	node
}

fn layout_input(
	input: &InputElement,
	rect: Rect,
	state: &mut LayoutState,
	inherited_style: TextStyleDto,
) -> LayoutNode {
	let mut node = LayoutNode::new(rect);
	let style = merge_styles(inherited_style, input.style);
	let clipped = clip_line(&input.value, rect.width);
	if !clipped.is_empty() {
		node.paints.push(LayoutPaint::TextRun {
			origin: GridPoint { x: u32::from(rect.x), y: u32::from(rect.y) },
			text: clipped.clone(),
			style,
		});
	}

	if input.focused {
		let cursor_x = rect
			.x
			.saturating_add(display_width(&clipped))
			.min(rect.x.saturating_add(rect.width.saturating_sub(1)));
		state.cursor = Some(GridPoint { x: u32::from(cursor_x), y: u32::from(rect.y) });
	}
	node
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
	let mut width = 0u16;
	let mut clipped = String::new();
	for ch in line.chars() {
		let ch_width = char_display_width(ch);
		if width.saturating_add(ch_width) > max_width {
			break;
		}
		width = width.saturating_add(ch_width);
		clipped.push(ch);
	}
	clipped
}

fn pixels_of(length: Pixels) -> u32 { length.0.max(0.0).round() as u32 }

fn cells_of(length: Pixels) -> i32 { length.0.max(0.0).round() as i32 }

fn child_fixed_extent(child: &Element, axis: Axis) -> u16 { intrinsic_extent(child, axis) }

fn child_flex_grow(child: &Element) -> u16 {
	match child {
		Element::Div(div) => div.flex_grow,
		Element::Text(_) | Element::Input(_) => 0,
	}
}

fn is_absolute(child: &Element) -> bool {
	matches!(child, Element::Div(div) if div.position == PositionMode::Absolute)
}

fn intrinsic_extent(element: &Element, axis: Axis) -> u16 {
	match element {
		Element::Text(text) => intrinsic_text_extent(text, axis),
		Element::Input(input) => intrinsic_input_extent(input, axis),
		Element::Div(div) => intrinsic_div_extent(div, axis),
	}
}

fn intrinsic_text_extent(text: &Text, axis: Axis) -> u16 {
	match axis {
		Axis::Horizontal => text.content.lines().map(display_width).max().unwrap_or(0).max(1),
		Axis::Vertical => (text.content.lines().count() as u16).max(1),
	}
}

fn intrinsic_input_extent(input: &InputElement, axis: Axis) -> u16 {
	match axis {
		Axis::Horizontal => display_width(&input.value).max(1),
		Axis::Vertical => 1,
	}
}

fn display_width(text: &str) -> u16 {
	u16::try_from(UnicodeWidthStr::width(text)).unwrap_or(u16::MAX)
}

fn char_display_width(ch: char) -> u16 {
	u16::try_from(UnicodeWidthChar::width(ch).unwrap_or(0)).unwrap_or(0)
}

fn intrinsic_div_extent(div: &Div, axis: Axis) -> u16 {
	let explicit = match axis {
		Axis::Horizontal => div.width.map(cells_of),
		Axis::Vertical => div.height.map(cells_of),
	};
	if let Some(explicit) = explicit {
		return explicit.max(0) as u16;
	}

	let flow_children: Vec<_> = div.children.iter().filter(|child| !is_absolute(child)).collect();
	if flow_children.is_empty() {
		return div.padding.saturating_mul(2);
	}

	let gap_total = div.gap.saturating_mul(flow_children.len().saturating_sub(1) as u16);
	let content_extent = match (div.display, axis) {
		(Display::Block, Axis::Horizontal) | (Display::FlexCol, Axis::Horizontal) => {
			flow_children.iter().map(|child| intrinsic_extent(child, axis)).max().unwrap_or(0)
		}
		(Display::Block, Axis::Vertical) | (Display::FlexCol, Axis::Vertical) => flow_children
			.iter()
			.map(|child| intrinsic_extent(child, axis))
			.sum::<u16>()
			.saturating_add(gap_total),
		(Display::FlexRow, Axis::Horizontal) => flow_children
			.iter()
			.map(|child| intrinsic_extent(child, axis))
			.sum::<u16>()
			.saturating_add(gap_total),
		(Display::FlexRow, Axis::Vertical) => {
			flow_children.iter().map(|child| intrinsic_extent(child, axis)).max().unwrap_or(0)
		}
	};

	content_extent.saturating_add(div.padding.saturating_mul(2))
}

pub mod elements {
	pub mod anchored {
		pub use crate::{Anchored, anchored};
	}

	pub mod animation {
		pub use crate::{Animation, animation};
	}

	pub mod canvas {
		pub use crate::{Canvas, canvas};
	}

	pub mod deferred {
		pub use crate::{Deferred, deferred};
	}

	pub mod div {
		pub use crate::{Div, div, h_flex, v_flex};
	}

	pub mod img {
		pub use crate::{Img, img};
	}

	pub mod list {
		pub use crate::{List, list};
	}

	pub mod surface {
		pub use crate::{Surface, surface};
	}

	pub mod svg {
		pub use crate::{Svg, svg};
	}

	pub mod text {
		pub use crate::{Text, text};
	}

	pub mod uniform_list {
		pub use crate::{UniformList, uniform_list};
	}
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
	fn flex_row_measures_text_children_by_intrinsic_width() {
		let tree = UiTree::new(h_flex().gap_1().child(div().child("alpha")).child(div().child("beta")));

		let compiled = tree.compile(GridSize::new(20, 4));

		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { x, text, .. } if *x == 0 && text == "alpha"
		)));
		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { x, text, .. } if *x == 6 && text == "beta"
		)));
	}

	#[test]
	fn full_width_input_positions_cursor_by_display_cells() {
		let tree = UiTree::new(div().child(text_input("你好a", true)));

		let compiled = tree.compile(GridSize::new(10, 2));

		assert_eq!(compiled.cursor, Some(GridPoint { x: 5, y: 0 }));
	}

	#[test]
	fn clip_line_respects_full_width_characters() {
		assert_eq!(clip_line("你好ab", 5), "你好a");
	}

	#[test]
	fn flex_column_measures_nested_container_height() {
		let tree = UiTree::new(
			v_flex().child("header").child(v_flex().gap_1().child("task one").child("task two")),
		);

		let compiled = tree.compile(GridSize::new(20, 6));

		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { y, text, .. } if *y == 0 && text == "header"
		)));
		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { y, text, .. } if *y == 1 && text == "task one"
		)));
		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { y, text, .. } if *y == 3 && text == "task two"
		)));
	}

	#[test]
	fn flex_children_do_not_shrink_remaining_space_by_intrinsic_size() {
		let layout = UiTree::new(
			h_flex()
				.gap_1()
				.child(div().flex_1().child("left content grows"))
				.child(div().flex_1().child("right")),
		)
		.layout(GridSize::new(20, 4));

		assert_eq!(layout.root.children.len(), 2);
		assert_eq!(layout.root.children[0].grid_rect.width, 9);
		assert_eq!(layout.root.children[1].grid_rect.x, 10);
		assert_eq!(layout.root.children[1].grid_rect.width, 10);
	}

	#[test]
	fn layout_tree_preserves_structured_fill_node() {
		let tree = UiTree::new(
			h_flex().child("left").child(div().w(px(1.0)).bg(rgba(1, 2, 3, 255))).child("right"),
		);

		let layout = tree.layout(GridSize::new(10, 4));

		assert_eq!(layout.root.children.len(), 3);
		assert!(matches!(
			&layout.root.children[1].paints[..],
			[LayoutPaint::FillRect { rect, color }]
				if *rect
					== PixelRect {
						x_px: 32,
						y_px: 0,
						width_px: CELL_WIDTH_PX,
						height_px: 4 * CELL_HEIGHT_PX,
					} && *color == rgba(1, 2, 3, 255)
		));
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
	fn styled_text_input_preserves_cursor_and_color() {
		let tree = UiTree::new(div().child(styled_text_input("xyz", true, TextStyleDto {
			foreground: Some(rgb(9, 8, 7)),
			background: None,
			bold:       false,
			italic:     false,
			underline:  false,
		})));

		let compiled = tree.compile(GridSize::new(20, 5));

		assert_eq!(compiled.cursor, Some(GridPoint { x: 3, y: 0 }));
		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { text, style, .. }
				if text == "xyz" && style.foreground == Some(rgb(9, 8, 7))
		)));
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
