use germinal_domain::{rendering::render_target_id::RenderTargetId, shared::seq::Seq};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildFramePlanTask {
	pub target_id: RenderTargetId,
	pub seq:       Seq,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltFramePlan {
	pub target_id: RenderTargetId,
	pub seq:       Seq,
	pub commands:  Vec<RenderCommandDto>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderCommandDto {
	Clear,
	ClearLine { y: u32 },
	TextRun { x: u32, y: u32, text: String },
	StyledTextRun { x: u32, y: u32, text: String, style: TextStyleDto },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextStyleDto {
	pub foreground: Option<RgbColorDto>,
	pub background: Option<RgbColorDto>,
	pub bold:       bool,
	pub italic:     bool,
	pub underline:  bool,
}

impl TextStyleDto {
	pub const fn plain() -> Self {
		Self {
			foreground: None,
			background: None,
			bold:       false,
			italic:     false,
			underline:  false,
		}
	}
}

impl Default for TextStyleDto {
	fn default() -> Self { Self::plain() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColorDto {
	pub red:   u8,
	pub green: u8,
	pub blue:  u8,
}

impl RgbColorDto {
	pub const fn new(red: u8, green: u8, blue: u8) -> Self { Self { red, green, blue } }
}

pub trait FramePlanBuilder {
	fn build(&self, task: BuildFramePlanTask) -> BuiltFramePlan;
}
