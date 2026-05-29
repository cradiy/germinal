use unicode_width::UnicodeWidthChar;

/// Returns the terminal cell width for a Unicode scalar value.
///
/// This mirrors the width decision used by terminal grid layout: Unicode width
/// decides the cell advance, and values are capped at double-width. It must not
/// be replaced by font glyph advance or bitmap size; those belong only to glyph
/// placement.
pub fn terminal_char_cell_width(c: char) -> u32 { c.width().unwrap_or(0).min(2) as u32 }

pub fn terminal_char_cell_advance(c: char) -> u32 { terminal_char_cell_width(c).max(1) }

pub fn terminal_text_cell_width(text: &str) -> u32 {
	text.chars().map(terminal_char_cell_advance).sum()
}

pub fn terminal_chars_cell_width(chars: &[char]) -> u32 {
	chars.iter().copied().map(terminal_char_cell_advance).sum()
}
