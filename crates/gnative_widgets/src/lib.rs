use germinal_gnative_ui::{
	Element, IntoDivChild, IntoElementNode,
	elements::div::{Div, div, h_flex, v_flex},
	rgb, rgba, text_input,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Label {
	text:      String,
	secondary: Option<String>,
	color:     Option<germinal_gnative_ui::RgbColorDto>,
	bold:      bool,
}

impl Label {
	pub fn new(text: impl Into<String>) -> Self {
		Self { text: text.into(), secondary: None, color: None, bold: false }
	}

	pub fn secondary(mut self, text: impl Into<String>) -> Self {
		self.secondary = Some(text.into());
		self
	}

	pub fn text_color(mut self, color: germinal_gnative_ui::RgbColorDto) -> Self {
		self.color = Some(color);
		self
	}

	pub fn font_semibold(mut self) -> Self {
		self.bold = true;
		self
	}

	pub fn font_bold(self) -> Self { self.font_semibold() }

	pub fn text_xs(self) -> Self { self }

	pub fn text_sm(self) -> Self { self }

	pub fn text_base(self) -> Self { self }

	pub fn text_lg(self) -> Self { self }

	fn into_div(self) -> Div {
		let mut primary = div();
		if let Some(color) = self.color {
			primary = primary.text_color(color);
		}
		if self.bold {
			primary = primary.font_bold();
		}
		primary = primary.child(self.text);

		match self.secondary {
			Some(secondary) => {
				h_flex().gap_1().child(primary).child(div().text_color(rgb(130, 140, 158)).child(secondary))
			}
			None => primary,
		}
	}
}

impl IntoElementNode for Label {
	fn into_element(self) -> Element { self.into_div().into_element() }
}

impl IntoDivChild for Label {
	fn into_child(self, _inherited_style: germinal_gnative_ui::TextStyleDto) -> Element {
		self.into_element()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ButtonVariant {
	Secondary,
	Primary,
	Danger,
	Ghost,
	Text,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Button {
	id:       String,
	label:    Option<String>,
	variant:  ButtonVariant,
	outline:  bool,
	selected: bool,
	disabled: bool,
	compact:  bool,
}

impl Button {
	pub fn new(id: impl Into<String>) -> Self {
		Self {
			id:       id.into(),
			label:    None,
			variant:  ButtonVariant::Secondary,
			outline:  false,
			selected: false,
			disabled: false,
			compact:  false,
		}
	}

	pub fn label(mut self, label: impl Into<String>) -> Self {
		self.label = Some(label.into());
		self
	}

	pub fn primary(mut self) -> Self {
		self.variant = ButtonVariant::Primary;
		self
	}

	pub fn danger(mut self) -> Self {
		self.variant = ButtonVariant::Danger;
		self
	}

	pub fn ghost(mut self) -> Self {
		self.variant = ButtonVariant::Ghost;
		self
	}

	pub fn text(mut self) -> Self {
		self.variant = ButtonVariant::Text;
		self
	}

	pub fn outline(mut self) -> Self {
		self.outline = true;
		self
	}

	pub fn selected(mut self, selected: bool) -> Self {
		self.selected = selected;
		self
	}

	pub fn disabled(mut self, disabled: bool) -> Self {
		self.disabled = disabled;
		self
	}

	pub fn compact(mut self) -> Self {
		self.compact = true;
		self
	}

	fn into_div(self) -> Div {
		let label = self.label.unwrap_or(self.id);
		let (fg, bg) = if self.disabled {
			(rgb(112, 118, 132), None)
		} else {
			match self.variant {
				ButtonVariant::Secondary => (rgb(222, 226, 236), None),
				ButtonVariant::Primary => (rgb(255, 255, 255), Some(rgba(52, 96, 220, 255))),
				ButtonVariant::Danger => (rgb(255, 255, 255), Some(rgba(184, 58, 58, 255))),
				ButtonVariant::Ghost => (rgb(184, 192, 208), None),
				ButtonVariant::Text => (rgb(110, 170, 255), None),
			}
		};

		let mut container = div().text_color(fg).child(label);
		if self.variant != ButtonVariant::Ghost && self.variant != ButtonVariant::Text {
			container = container.border_1();
		}
		if let Some(color) = bg {
			container = if self.outline {
				container.bg(rgba(color.red, color.green, color.blue, 36))
			} else {
				container.bg(color)
			};
		} else if self.selected {
			container = container.bg(rgba(54, 70, 110, 180));
		}
		if self.selected
			&& (self.variant == ButtonVariant::Ghost || self.variant == ButtonVariant::Text)
		{
			container = container.border_1();
		}
		if !self.compact {
			container = container.p_1();
		}
		container
	}
}

impl IntoElementNode for Button {
	fn into_element(self) -> Element { self.into_div().into_element() }
}

impl IntoDivChild for Button {
	fn into_child(self, _inherited_style: germinal_gnative_ui::TextStyleDto) -> Element {
		self.into_element()
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct ButtonGroup {
	id:       String,
	children: Vec<Element>,
}

impl ButtonGroup {
	pub fn new(id: impl Into<String>) -> Self { Self { id: id.into(), children: Vec::new() } }

	pub fn child(mut self, child: impl IntoElementNode) -> Self {
		let _ = &self.id;
		self.children.push(child.into_element());
		self
	}

	fn into_div(self) -> Div { h_flex().gap_1().children(self.children) }
}

impl IntoElementNode for ButtonGroup {
	fn into_element(self) -> Element { self.into_div().into_element() }
}

impl IntoDivChild for ButtonGroup {
	fn into_child(self, _inherited_style: germinal_gnative_ui::TextStyleDto) -> Element {
		self.into_element()
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct Checkbox {
	id:       String,
	label:    Option<String>,
	checked:  bool,
	disabled: bool,
}

impl Checkbox {
	pub fn new(id: impl Into<String>) -> Self {
		Self { id: id.into(), label: None, checked: false, disabled: false }
	}

	pub fn label(mut self, label: impl Into<String>) -> Self {
		self.label = Some(label.into());
		self
	}

	pub fn checked(mut self, checked: bool) -> Self {
		self.checked = checked;
		self
	}

	pub fn disabled(mut self, disabled: bool) -> Self {
		self.disabled = disabled;
		self
	}

	pub fn text_xs(self) -> Self { self }

	pub fn text_sm(self) -> Self { self }

	pub fn text_base(self) -> Self { self }

	pub fn text_lg(self) -> Self { self }
}

impl IntoElementNode for Checkbox {
	fn into_element(self) -> Element {
		let mark = if self.checked { 'x' } else { ' ' };
		let label = self.label.unwrap_or(self.id);
		let color = if self.disabled { rgb(112, 118, 132) } else { rgb(224, 228, 236) };
		div().text_color(color).child(format!("[{mark}] {label}")).into_element()
	}
}

impl IntoDivChild for Checkbox {
	fn into_child(self, _inherited_style: germinal_gnative_ui::TextStyleDto) -> Element {
		self.into_element()
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupBox {
	id:       Option<String>,
	title:    Option<String>,
	children: Vec<Element>,
	outline:  bool,
	fill:     bool,
}

impl GroupBox {
	pub fn new() -> Self {
		Self { id: None, title: None, children: Vec::new(), outline: false, fill: false }
	}

	pub fn id(mut self, id: impl Into<String>) -> Self {
		self.id = Some(id.into());
		self
	}

	pub fn title(mut self, title: impl Into<String>) -> Self {
		self.title = Some(title.into());
		self
	}

	pub fn child(mut self, child: impl IntoElementNode) -> Self {
		self.children.push(child.into_element());
		self
	}

	pub fn outline(mut self) -> Self {
		self.outline = true;
		self
	}

	pub fn fill(mut self) -> Self {
		self.fill = true;
		self
	}

	fn into_div(self) -> Div {
		let _ = &self.id;
		let mut body = v_flex().gap_1();
		if let Some(title) = self.title {
			body = body.child(
				h_flex()
					.child(" ")
					.child(div().flex_1().text_color(rgb(210, 210, 210)).font_bold().child(title)),
			);
		}
		for child in self.children {
			body = body.child(h_flex().child(" ").child(div().flex_1().child(child)));
		}

		let mut container = div().child(body);
		if self.outline {
			container = container.border_1();
		}
		if self.fill {
			container = container.bg(rgba(18, 22, 40, 220));
		}
		container
	}
}

impl IntoElementNode for GroupBox {
	fn into_element(self) -> Element { self.into_div().into_element() }
}

impl IntoDivChild for GroupBox {
	fn into_child(self, _inherited_style: germinal_gnative_ui::TextStyleDto) -> Element {
		self.into_element()
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputState {
	id:              String,
	value:           String,
	placeholder:     Option<String>,
	clean_on_escape: bool,
	focused:         bool,
}

impl InputState {
	pub fn new(id: impl Into<String>) -> Self {
		Self {
			id:              id.into(),
			value:           String::new(),
			placeholder:     None,
			clean_on_escape: false,
			focused:         false,
		}
	}

	pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
		self.placeholder = Some(placeholder.into());
		self
	}

	pub fn default_value(mut self, value: impl Into<String>) -> Self {
		self.value = value.into();
		self
	}

	pub fn clean_on_escape(mut self) -> Self {
		self.clean_on_escape = true;
		self
	}

	pub fn value(&self) -> &str { &self.value }

	pub fn set_value(&mut self, value: impl Into<String>) { self.value = value.into(); }

	pub fn clear(&mut self) { self.value.clear(); }

	pub fn placeholder_text(&self) -> Option<&str> { self.placeholder.as_deref() }

	pub fn set_focused(&mut self, focused: bool) { self.focused = focused; }

	pub fn is_focused(&self) -> bool { self.focused }

	pub fn clean_on_escape_enabled(&self) -> bool { self.clean_on_escape }

	pub fn id(&self) -> &str { &self.id }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Input {
	state:      InputState,
	appearance: bool,
	disabled:   bool,
}

impl Input {
	pub fn new(state: &InputState) -> Self {
		Self { state: state.clone(), appearance: true, disabled: false }
	}

	pub fn appearance(mut self, appearance: bool) -> Self {
		self.appearance = appearance;
		self
	}

	pub fn disabled(mut self, disabled: bool) -> Self {
		self.disabled = disabled;
		self
	}

	pub fn small(self) -> Self { self }

	pub fn large(self) -> Self { self }
}

impl IntoElementNode for Input {
	fn into_element(self) -> Element {
		let color = if self.disabled { rgb(112, 118, 132) } else { rgb(255, 214, 92) };
		let mut container = div().text_color(color);
		if self.appearance {
			container = container.border_1();
		}

		if self.state.value().is_empty() && !self.state.is_focused() {
			if let Some(placeholder) = self.state.placeholder_text() {
				return container
					.text_color(rgb(112, 118, 132))
					.child(placeholder.to_string())
					.into_element();
			}
		}

		container
			.child(text_input(self.state.value().to_string(), self.state.is_focused()))
			.into_element()
	}
}

impl IntoDivChild for Input {
	fn into_child(self, _inherited_style: germinal_gnative_ui::TextStyleDto) -> Element {
		self.into_element()
	}
}

pub mod button {
	pub use super::{Button, ButtonGroup};
}

pub mod checkbox {
	pub use super::Checkbox;
}

pub mod group_box {
	pub use super::GroupBox;
}

pub mod input {
	pub use super::{Input, InputState};
}

pub mod label {
	pub use super::Label;
}

#[cfg(test)]
mod tests {
	use germinal_gnative_ui::{RenderCommandDto, UiTree};

	use super::*;

	#[test]
	fn checkbox_renders_checked_label() {
		let tree = UiTree::new(Checkbox::new("todo-1").label("Ship widgets").checked(true));
		let compiled = tree.compile(germinal_gnative_ui::GridSize::new(20, 4));

		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { text, .. } if text == "[x] Ship widgets"
		)));
	}

	#[test]
	fn input_state_renders_cursor_when_focused() {
		let mut state = InputState::new("composer").default_value("alpha");
		state.set_focused(true);
		let tree = UiTree::new(Input::new(&state));
		let compiled = tree.compile(germinal_gnative_ui::GridSize::new(20, 4));

		assert_eq!(compiled.cursor, Some(germinal_gnative_ui::GridPoint { x: 5, y: 0 }));
		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { text, .. } if text == "alpha"
		)));
	}

	#[test]
	fn group_box_supports_multiple_children() {
		let tree = UiTree::new(
			GroupBox::new().outline().title("Tasks").child(Label::new("One")).child(Label::new("Two")),
		);
		let compiled = tree.compile(germinal_gnative_ui::GridSize::new(20, 7));

		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { text, .. } if text == "Tasks"
		)));
		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { text, .. } if text == "One"
		)));
		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { text, .. } if text == "Two"
		)));
	}

	#[test]
	fn group_box_indents_title_and_content_inside_border() {
		let tree = UiTree::new(GroupBox::new().outline().title("Tasks").child(Label::new("One")));
		let compiled = tree.compile(germinal_gnative_ui::GridSize::new(20, 5));

		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { x, text, .. } if *x == 1 && text == "Tasks"
		)));
		assert!(compiled.commands.iter().any(|command| matches!(
			command,
			RenderCommandDto::StyledTextRun { x, text, .. } if *x == 1 && text == "One"
		)));
	}
}
