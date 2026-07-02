use serde::{Deserialize, Serialize};

pub const PIXEL_FILL_RECT_MARKER: &str = "\u{E000}germinal.pixel_fill_rect:";
pub const VIDEO_SURFACE_MARKER: &str = "\u{E001}germinal.video_surface:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderCommandDto {
	Clear,
	ClearLine {
		y: u32,
	},
	TextRun {
		x:    u32,
		y:    u32,
		text: String,
	},
	StyledTextRun {
		x:     u32,
		y:     u32,
		text:  String,
		style: TextStyleDto,
	},
	PixelFillRect {
		x_px:      u32,
		y_px:      u32,
		width_px:  u32,
		height_px: u32,
		color:     RgbaColorDto,
	},
	VideoSurface {
		id:        String,
		x_px:      u32,
		y_px:      u32,
		width_px:  u32,
		height_px: u32,
	},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbColorDto {
	pub red:   u8,
	pub green: u8,
	pub blue:  u8,
}

impl RgbColorDto {
	pub const fn new(red: u8, green: u8, blue: u8) -> Self { Self { red, green, blue } }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbaColorDto {
	pub red:   u8,
	pub green: u8,
	pub blue:  u8,
	pub alpha: u8,
}

impl RgbaColorDto {
	pub const fn new(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
		Self { red, green, blue, alpha }
	}

	pub const fn opaque(red: u8, green: u8, blue: u8) -> Self { Self::new(red, green, blue, 255) }
}

pub fn encode_pixel_fill_rect_command(command: &RenderCommandDto) -> Option<String> {
	let RenderCommandDto::PixelFillRect { x_px, y_px, width_px, height_px, color } = command else {
		return None;
	};

	Some(format!(
		"{PIXEL_FILL_RECT_MARKER}{x_px},{y_px},{width_px},{height_px},{},{},{},{}",
		color.red, color.green, color.blue, color.alpha
	))
}

pub fn decode_pixel_fill_rect_command(text: &str) -> Option<RenderCommandDto> {
	let payload = text.strip_prefix(PIXEL_FILL_RECT_MARKER)?;
	let mut parts = payload.split(',');
	let x_px = parts.next()?.parse().ok()?;
	let y_px = parts.next()?.parse().ok()?;
	let width_px = parts.next()?.parse().ok()?;
	let height_px = parts.next()?.parse().ok()?;
	let red = parts.next()?.parse().ok()?;
	let green = parts.next()?.parse().ok()?;
	let blue = parts.next()?.parse().ok()?;
	let alpha = parts.next()?.parse().ok()?;
	if parts.next().is_some() {
		return None;
	}

	Some(RenderCommandDto::PixelFillRect {
		x_px,
		y_px,
		width_px,
		height_px,
		color: RgbaColorDto::new(red, green, blue, alpha),
	})
}

pub fn encode_video_surface_command(command: &RenderCommandDto) -> Option<String> {
	let RenderCommandDto::VideoSurface { id, x_px, y_px, width_px, height_px } = command else {
		return None;
	};

	Some(format!("{VIDEO_SURFACE_MARKER}{}:{id},{x_px},{y_px},{width_px},{height_px}", id.len()))
}

pub fn decode_video_surface_command(text: &str) -> Option<RenderCommandDto> {
	let payload = text.strip_prefix(VIDEO_SURFACE_MARKER)?;
	let (id_len_text, remainder) = payload.split_once(':')?;
	let id_len: usize = id_len_text.parse().ok()?;
	if remainder.len() < id_len + 1 {
		return None;
	}
	let id = remainder.get(..id_len)?.to_string();
	let numbers = remainder.get(id_len..)?;
	let numbers = numbers.strip_prefix(',')?;
	let mut parts = numbers.split(',');
	let x_px = parts.next()?.parse().ok()?;
	let y_px = parts.next()?.parse().ok()?;
	let width_px = parts.next()?.parse().ok()?;
	let height_px = parts.next()?.parse().ok()?;
	if parts.next().is_some() {
		return None;
	}

	Some(RenderCommandDto::VideoSurface { id, x_px, y_px, width_px, height_px })
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pixel_fill_rect_round_trips_through_marker_text() {
		let command = RenderCommandDto::PixelFillRect {
			x_px:      12,
			y_px:      34,
			width_px:  56,
			height_px: 78,
			color:     RgbaColorDto::new(1, 2, 3, 4),
		};

		let encoded = encode_pixel_fill_rect_command(&command).expect("pixel command should encode");
		assert_eq!(decode_pixel_fill_rect_command(&encoded), Some(command));
	}

	#[test]
	fn video_surface_round_trips_through_marker_text() {
		let command = RenderCommandDto::VideoSurface {
			id:        "demo.player".to_string(),
			x_px:      12,
			y_px:      34,
			width_px:  56,
			height_px: 78,
		};

		let encoded = encode_video_surface_command(&command).expect("video command should encode");
		assert_eq!(decode_video_surface_command(&encoded), Some(command));
	}
}
