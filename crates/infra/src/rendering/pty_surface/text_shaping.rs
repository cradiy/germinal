use germinal_ports::rendering::frame_plan_builder::TextStyleDto;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::rendering::pty_surface::glyph_atlas::WgpuTerminalGlyphKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTextSegment {
    pub text: String,
    pub cell_width: u32,
    pub glyph_key: WgpuTerminalGlyphKey,
    pub shaped: bool,
}

pub fn terminal_text_segments(text: &str, style: TextStyleDto) -> Vec<TerminalTextSegment> {
    let normalized: String = text.nfc().collect();
    let graphemes: Vec<&str> = normalized.graphemes(true).collect();
    let mut segments = Vec::new();
    let mut index = 0;

    while index < graphemes.len() {
        let grapheme = graphemes[index];
        if is_contextual_script_grapheme(grapheme) {
            let start = index;
            index += 1;
            while index < graphemes.len() && is_contextual_script_grapheme(graphemes[index]) {
                index += 1;
            }
            push_shaped_segment(&mut segments, &graphemes[start..index], style);
            continue;
        }

        if grapheme.chars().count() > 1 {
            push_shaped_segment(&mut segments, &graphemes[index..=index], style);
        } else if let Some(character) = grapheme.chars().next() {
            segments.push(TerminalTextSegment {
                text: grapheme.to_owned(),
                cell_width: terminal_grapheme_cell_width(grapheme),
                glyph_key: WgpuTerminalGlyphKey::styled(character, style.bold, style.italic),
                shaped: false,
            });
        }
        index += 1;
    }

    segments
}

fn push_shaped_segment(
    segments: &mut Vec<TerminalTextSegment>,
    graphemes: &[&str],
    style: TextStyleDto,
) {
    let text = graphemes.concat();
    let cell_width = graphemes
        .iter()
        .map(|grapheme| terminal_grapheme_cell_width(grapheme))
        .sum();
    segments.push(TerminalTextSegment {
        cell_width,
        glyph_key: WgpuTerminalGlyphKey::cluster(stable_text_hash(&text), style.bold, style.italic),
        text,
        shaped: true,
    });
}

pub fn terminal_grapheme_cell_width(grapheme: &str) -> u32 {
    UnicodeWidthStr::width(grapheme).min(2) as u32
}

pub fn cursor_fallback_segments(segment: &TerminalTextSegment) -> Vec<TerminalTextSegment> {
    if !segment.shaped {
        return Vec::new();
    }

    segment
        .text
        .graphemes(true)
        .filter_map(|grapheme| {
            let character = grapheme.chars().next()?;
            let cell_width = terminal_grapheme_cell_width(grapheme);
            (cell_width > 0).then(|| TerminalTextSegment {
                text: character.to_string(),
                cell_width,
                glyph_key: WgpuTerminalGlyphKey::styled(
                    character,
                    segment.glyph_key.bold(),
                    segment.glyph_key.italic(),
                ),
                shaped: false,
            })
        })
        .collect()
}

fn stable_text_hash(text: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    text.as_bytes().iter().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

fn is_contextual_script_grapheme(grapheme: &str) -> bool {
    grapheme.chars().any(is_contextual_script_character)
}

fn is_contextual_script_character(character: char) -> bool {
    matches!(
        character as u32,
        0x0600..=0x06ff
            | 0x0700..=0x074f
            | 0x0750..=0x077f
            | 0x0780..=0x07bf
            | 0x0840..=0x085f
            | 0x08a0..=0x08ff
            | 0x0900..=0x0dff
            | 0x0e00..=0x0fff
            | 0x1000..=0x109f
            | 0x1780..=0x17ff
            | 0xa980..=0xa9df
            | 0xaa60..=0xaa7f
            | 0xfb50..=0xfdff
            | 0xfe70..=0xfeff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_plain_terminal_characters_independent() {
        let segments = terminal_text_segments("ab中", TextStyleDto::plain());

        assert_eq!(segments.len(), 3);
        assert!(segments.iter().all(|segment| !segment.shaped));
        assert_eq!(segments[2].cell_width, 2);
    }

    #[test]
    fn groups_joined_emoji_and_normalizes_composable_marks() {
        let segments = terminal_text_segments("👩\u{200d}💻 e\u{301}", TextStyleDto::plain());

        assert_eq!(segments[0].text, "👩\u{200d}💻");
        assert!(segments[0].shaped);
        assert_eq!(segments[0].cell_width, 2);
        assert_eq!(segments[2].text, "é");
        assert!(!segments[2].shaped);
    }

    #[test]
    fn groups_joining_script_words_for_contextual_shaping() {
        let segments = terminal_text_segments("سلام عالم", TextStyleDto::plain());

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].text, "سلام");
        assert!(segments[0].shaped);
        assert_eq!(segments[2].text, "عالم");
        assert!(segments[2].shaped);
    }

    #[test]
    fn groups_indic_words_for_contextual_shaping() {
        let segments = terminal_text_segments("नमस्ते दुनिया", TextStyleDto::plain());

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].text, "नमस्ते");
        assert!(segments[0].shaped);
        assert_eq!(segments[2].text, "दुनिया");
        assert!(segments[2].shaped);
    }
}
