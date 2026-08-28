use germinal_ports::rendering::frame_plan_builder::TextStyleDto;
use smol_str::SmolStr;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::rendering::pty_surface::glyph_atlas::WgpuTerminalGlyphKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalTextSegment {
    pub text: SmolStr,
    pub cell_width: u32,
    pub glyph_key: WgpuTerminalGlyphKey,
    pub shaped: bool,
    pub ligature: bool,
}

#[cfg(test)]
pub fn terminal_text_segments(text: &str, style: TextStyleDto) -> Vec<TerminalTextSegment> {
    terminal_text_segments_with_ligatures(text, style, true)
}

pub fn terminal_text_segments_with_ligatures(
    text: &str,
    style: TextStyleDto,
    ligatures: bool,
) -> Vec<TerminalTextSegment> {
    if text
        .bytes()
        .all(|byte| byte.is_ascii_graphic() || byte == b' ')
    {
        return ascii_text_segments(text, style, ligatures);
    }

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
            push_shaped_segment(&mut segments, &graphemes[start..index], style, false);
            continue;
        }

        if ligatures && is_code_operator_grapheme(grapheme) {
            let start = index;
            index += 1;
            while index < graphemes.len() && is_code_operator_grapheme(graphemes[index]) {
                index += 1;
            }
            if should_shape_operator_run(&graphemes[start..index]) {
                push_shaped_segment(&mut segments, &graphemes[start..index], style, true);
                continue;
            }
            if index - start > 4 {
                for grapheme in &graphemes[start..index] {
                    push_plain_ascii_segment(&mut segments, grapheme.as_bytes()[0], style);
                }
                continue;
            }
            index = start;
        }

        if ligatures && let Some(ligature_len) = standard_ligature_len(&graphemes[index..]) {
            push_shaped_segment(
                &mut segments,
                &graphemes[index..index + ligature_len],
                style,
                true,
            );
            index += ligature_len;
            continue;
        }

        if grapheme.chars().count() > 1 {
            push_shaped_segment(&mut segments, &graphemes[index..=index], style, false);
        } else if let Some(character) = grapheme.chars().next() {
            segments.push(TerminalTextSegment {
                text: SmolStr::new(grapheme),
                cell_width: terminal_grapheme_cell_width(grapheme),
                glyph_key: WgpuTerminalGlyphKey::styled(character, style.bold, style.italic),
                shaped: false,
                ligature: false,
            });
        }
        index += 1;
    }

    segments
}

fn ascii_text_segments(
    text: &str,
    style: TextStyleDto,
    ligatures: bool,
) -> Vec<TerminalTextSegment> {
    let bytes = text.as_bytes();
    let mut segments = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if ligatures && is_code_operator_byte(bytes[index]) {
            let start = index;
            index += 1;
            while index < bytes.len() && is_code_operator_byte(bytes[index]) {
                index += 1;
            }
            if should_shape_ascii_operator_run(&bytes[start..index]) {
                push_ascii_shaped_segment(&mut segments, &text[start..index], style, true);
                continue;
            }
            if index - start > 4 {
                for byte in &bytes[start..index] {
                    push_plain_ascii_segment(&mut segments, *byte, style);
                }
                continue;
            }
            index = start;
        }

        if ligatures && let Some(ligature_len) = standard_ascii_ligature_len(&bytes[index..]) {
            let end = index + ligature_len;
            push_ascii_shaped_segment(&mut segments, &text[index..end], style, true);
            index = end;
            continue;
        }

        push_plain_ascii_segment(&mut segments, bytes[index], style);
        index += 1;
    }

    segments
}

fn push_plain_ascii_segment(
    segments: &mut Vec<TerminalTextSegment>,
    byte: u8,
    style: TextStyleDto,
) {
    let character = char::from(byte);
    segments.push(TerminalTextSegment {
        text: std::iter::once(character).collect(),
        cell_width: 1,
        glyph_key: WgpuTerminalGlyphKey::styled(character, style.bold, style.italic),
        shaped: false,
        ligature: false,
    });
}

fn push_ascii_shaped_segment(
    segments: &mut Vec<TerminalTextSegment>,
    text: &str,
    style: TextStyleDto,
    ligature: bool,
) {
    segments.push(TerminalTextSegment {
        text: SmolStr::new(text),
        cell_width: text.len() as u32,
        glyph_key: WgpuTerminalGlyphKey::cluster(stable_text_hash(text), style.bold, style.italic),
        shaped: true,
        ligature,
    });
}

fn push_shaped_segment(
    segments: &mut Vec<TerminalTextSegment>,
    graphemes: &[&str],
    style: TextStyleDto,
    ligature: bool,
) {
    let text: SmolStr = graphemes.iter().copied().collect();
    let cell_width = graphemes
        .iter()
        .map(|grapheme| terminal_grapheme_cell_width(grapheme))
        .sum();
    segments.push(TerminalTextSegment {
        cell_width,
        glyph_key: WgpuTerminalGlyphKey::cluster(
            stable_text_hash(text.as_str()),
            style.bold,
            style.italic,
        ),
        text,
        shaped: true,
        ligature,
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
                text: std::iter::once(character).collect(),
                cell_width,
                glyph_key: WgpuTerminalGlyphKey::styled(
                    character,
                    segment.glyph_key.bold(),
                    segment.glyph_key.italic(),
                ),
                shaped: false,
                ligature: false,
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

fn is_code_operator_grapheme(grapheme: &str) -> bool {
    grapheme
        .as_bytes()
        .first()
        .is_some_and(|byte| grapheme.len() == 1 && is_code_operator_byte(*byte))
}

fn is_code_operator_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'!' | b'#'
            | b'$'
            | b'%'
            | b'&'
            | b'*'
            | b'+'
            | b'-'
            | b'.'
            | b'/'
            | b':'
            | b'<'
            | b'='
            | b'>'
            | b'?'
            | b'@'
            | b'\\'
            | b'^'
            | b'|'
            | b'~'
    )
}

fn should_shape_ascii_operator_run(bytes: &[u8]) -> bool {
    bytes.len() > 1 && (bytes.len() <= 4 || bytes.windows(2).any(|pair| pair[0] != pair[1]))
}

fn should_shape_operator_run(graphemes: &[&str]) -> bool {
    graphemes.len() > 1
        && (graphemes.len() <= 4
            || graphemes
                .windows(2)
                .any(|pair| pair[0].as_bytes() != pair[1].as_bytes()))
}

fn standard_ascii_ligature_len(bytes: &[u8]) -> Option<usize> {
    const STANDARD_LIGATURES: [&[u8]; 6] = [b"ffi", b"ffl", b"www", b"ff", b"fi", b"fl"];

    STANDARD_LIGATURES
        .iter()
        .find_map(|ligature| bytes.starts_with(ligature).then_some(ligature.len()))
}

fn standard_ligature_len(graphemes: &[&str]) -> Option<usize> {
    const STANDARD_LIGATURES: [&str; 6] = ["ffi", "ffl", "www", "ff", "fi", "fl"];

    STANDARD_LIGATURES.iter().find_map(|ligature| {
        let len = ligature.len();
        (graphemes.len() >= len
            && graphemes[..len]
                .iter()
                .zip(ligature.bytes())
                .all(|(grapheme, byte)| grapheme.as_bytes() == [byte]))
        .then_some(len)
    })
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
    fn common_terminal_segments_do_not_allocate_text_on_the_heap() {
        let segments = terminal_text_segments("ascii != 中文", TextStyleDto::plain());

        assert!(
            segments
                .iter()
                .all(|segment| !segment.text.is_heap_allocated())
        );
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

    #[test]
    fn groups_programming_operators_for_ligature_shaping() {
        let segments = terminal_text_segments("a != b -> c => d::e", TextStyleDto::plain());
        let shaped = segments
            .iter()
            .filter(|segment| segment.shaped)
            .map(|segment| (segment.text.as_str(), segment.cell_width))
            .collect::<Vec<_>>();

        assert_eq!(shaped, vec![("!=", 2), ("->", 2), ("=>", 2), ("::", 2)]);
        assert!(
            segments
                .iter()
                .filter(|segment| segment.shaped)
                .all(|segment| segment.ligature)
        );
    }

    #[test]
    fn groups_standard_alphabetic_ligatures_without_grouping_identifiers() {
        let text = "office float www name";
        let segments = terminal_text_segments(text, TextStyleDto::plain());
        let shaped = segments
            .iter()
            .filter(|segment| segment.shaped)
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>();

        assert_eq!(shaped, vec!["ffi", "fl", "www"]);
        assert!(
            segments
                .iter()
                .filter(|segment| segment.shaped)
                .all(|segment| segment.ligature)
        );
        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<String>(),
            text
        );
    }

    #[test]
    fn disabling_ligatures_keeps_code_operators_separate_but_preserves_required_shaping() {
        let segments =
            terminal_text_segments_with_ligatures("a != b سلام", TextStyleDto::plain(), false);

        assert!(
            segments
                .iter()
                .filter(|segment| segment.shaped)
                .all(|segment| segment.text == "سلام" && !segment.ligature)
        );
        assert!(
            segments
                .iter()
                .any(|segment| segment.text == "!" && !segment.shaped)
        );
        assert!(
            segments
                .iter()
                .any(|segment| segment.text == "=" && !segment.shaped)
        );
    }

    #[test]
    fn keeps_long_homogeneous_operator_fill_out_of_the_ligature_atlas() {
        let segments = terminal_text_segments("..........", TextStyleDto::plain());

        assert_eq!(segments.len(), 10);
        assert!(segments.iter().all(|segment| !segment.shaped));
    }
}
