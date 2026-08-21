use germinal_ports::pty_host::{color_theme::TerminalColorTheme, terminal_size::TerminalPtySize};

const MAX_CSI_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompatibilityProtocolEvent {
    Bytes(Vec<u8>),
    PtyWrite(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DecoderState {
    Ground,
    Escape,
    Csi(Vec<u8>),
    Osc { escape: bool },
    StringControl { escape: bool },
}

#[derive(Debug, Clone)]
pub(crate) struct TerminalCompatibilityProtocolDecoder {
    state: DecoderState,
    utf8_continuations: u8,
    size: TerminalPtySize,
    dark_color_scheme: bool,
    color_scheme_updates_enabled: bool,
}

impl TerminalCompatibilityProtocolDecoder {
    pub(crate) fn new(size: TerminalPtySize, color_theme: TerminalColorTheme) -> Self {
        Self {
            state: DecoderState::Ground,
            utf8_continuations: 0,
            size,
            dark_color_scheme: is_dark_color_scheme(color_theme),
            color_scheme_updates_enabled: false,
        }
    }

    pub(crate) fn resize(&mut self, size: TerminalPtySize) {
        self.size = size;
    }

    pub(crate) fn feed(&mut self, input: &[u8]) -> Vec<CompatibilityProtocolEvent> {
        let mut events = Vec::new();
        let mut visible = Vec::new();

        for &byte in input {
            let utf8_continuation = self.utf8_continuations > 0 && is_utf8_continuation(byte);
            if utf8_continuation {
                self.utf8_continuations -= 1;
            } else {
                self.utf8_continuations = utf8_continuations_after_lead(byte);
            }

            let state = std::mem::replace(&mut self.state, DecoderState::Ground);
            self.state = match state {
                DecoderState::Ground => match byte {
                    0x1b => DecoderState::Escape,
                    0x9b if !utf8_continuation => DecoderState::Csi(vec![0x9b]),
                    0x9d if !utf8_continuation => {
                        visible.push(byte);
                        DecoderState::Osc { escape: false }
                    }
                    0x90 | 0x98 | 0x9e | 0x9f if !utf8_continuation => {
                        visible.push(byte);
                        DecoderState::StringControl { escape: false }
                    }
                    _ => {
                        visible.push(byte);
                        DecoderState::Ground
                    }
                },
                DecoderState::Escape => {
                    if byte == b'[' {
                        DecoderState::Csi(vec![0x1b, b'['])
                    } else if byte == b']' {
                        visible.extend_from_slice(&[0x1b, b']']);
                        DecoderState::Osc { escape: false }
                    } else if matches!(byte, b'P' | b'X' | b'^' | b'_') {
                        visible.extend_from_slice(&[0x1b, byte]);
                        DecoderState::StringControl { escape: false }
                    } else {
                        visible.extend_from_slice(&[0x1b, byte]);
                        DecoderState::Ground
                    }
                }
                DecoderState::Csi(mut raw) => {
                    raw.push(byte);
                    if is_csi_final(byte) {
                        self.finish_csi(raw, &mut events, &mut visible);
                        DecoderState::Ground
                    } else if raw.len() > MAX_CSI_BYTES {
                        visible.extend_from_slice(&raw);
                        DecoderState::Ground
                    } else {
                        DecoderState::Csi(raw)
                    }
                }
                DecoderState::Osc { escape } => {
                    visible.push(byte);
                    if byte == 0x07
                        || (byte == 0x9c && !utf8_continuation)
                        || (escape && byte == b'\\')
                    {
                        DecoderState::Ground
                    } else {
                        DecoderState::Osc {
                            escape: byte == 0x1b,
                        }
                    }
                }
                DecoderState::StringControl { escape } => {
                    visible.push(byte);
                    if (byte == 0x9c && !utf8_continuation) || (escape && byte == b'\\') {
                        DecoderState::Ground
                    } else {
                        DecoderState::StringControl {
                            escape: byte == 0x1b,
                        }
                    }
                }
            };
        }

        flush_visible(&mut events, &mut visible);
        events
    }

    fn finish_csi(
        &mut self,
        raw: Vec<u8>,
        events: &mut Vec<CompatibilityProtocolEvent>,
        visible: &mut Vec<u8>,
    ) {
        let sequence = if raw.starts_with(b"\x1b[") {
            &raw[2..]
        } else {
            &raw[1..]
        };

        let response = match sequence {
            b"?996n" => Some(color_scheme_response(self.dark_color_scheme)),
            b"14t" => Some(text_area_size_response(self.size)),
            b"16t" => Some(cell_size_response(self.size)),
            b"?2031h" => {
                self.color_scheme_updates_enabled = true;
                None
            }
            b"?2031l" => {
                self.color_scheme_updates_enabled = false;
                None
            }
            _ => {
                visible.extend_from_slice(&raw);
                return;
            }
        };

        if let Some(response) = response {
            flush_visible(events, visible);
            events.push(CompatibilityProtocolEvent::PtyWrite(response));
        }
    }
}

fn color_scheme_response(dark: bool) -> Vec<u8> {
    if dark {
        b"\x1b[?997;1n".to_vec()
    } else {
        b"\x1b[?997;2n".to_vec()
    }
}

fn text_area_size_response(size: TerminalPtySize) -> Vec<u8> {
    format!("\x1b[4;{};{}t", size.pixel_height(), size.pixel_width()).into_bytes()
}

fn cell_size_response(size: TerminalPtySize) -> Vec<u8> {
    let cell_height = u32::from(size.pixel_height()) / u32::from(size.rows().max(1));
    let cell_width = u32::from(size.pixel_width()) / u32::from(size.columns().max(1));
    format!("\x1b[6;{cell_height};{cell_width}t").into_bytes()
}

fn is_dark_color_scheme(theme: TerminalColorTheme) -> bool {
    let background = theme.background;
    let luminance = u32::from(background.red) * 299
        + u32::from(background.green) * 587
        + u32::from(background.blue) * 114;
    luminance < 128_000
}

fn is_csi_final(byte: u8) -> bool {
    matches!(byte, 0x40..=0x7e)
}

fn is_utf8_continuation(byte: u8) -> bool {
    matches!(byte, 0x80..=0xbf)
}

fn utf8_continuations_after_lead(byte: u8) -> u8 {
    match byte {
        0xc2..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf4 => 3,
        _ => 0,
    }
}

fn flush_visible(events: &mut Vec<CompatibilityProtocolEvent>, visible: &mut Vec<u8>) {
    if !visible.is_empty() {
        events.push(CompatibilityProtocolEvent::Bytes(std::mem::take(visible)));
    }
}

#[cfg(test)]
mod tests {
    use germinal_ports::{
        pty_host::{color_theme::TerminalColorTheme, terminal_size::TerminalPtySize},
        rendering::frame_plan_builder::RgbColorDto,
    };

    use super::*;

    #[test]
    fn answers_color_scheme_queries_and_consumes_update_mode() {
        let size = TerminalPtySize::new(24, 80, 960, 576);
        let mut decoder =
            TerminalCompatibilityProtocolDecoder::new(size, TerminalColorTheme::default());

        assert_eq!(
            decoder.feed(b"before\x1b[?996n\x1b[?2031hafter"),
            vec![
                CompatibilityProtocolEvent::Bytes(b"before".to_vec()),
                CompatibilityProtocolEvent::PtyWrite(b"\x1b[?997;1n".to_vec()),
                CompatibilityProtocolEvent::Bytes(b"after".to_vec()),
            ]
        );
        assert!(decoder.color_scheme_updates_enabled);
        assert!(decoder.feed(b"\x1b[?2031l").is_empty());
        assert!(!decoder.color_scheme_updates_enabled);
    }

    #[test]
    fn reports_light_color_scheme() {
        let theme = TerminalColorTheme {
            background: RgbColorDto::new(245, 245, 245),
            ..TerminalColorTheme::default()
        };
        let mut decoder = TerminalCompatibilityProtocolDecoder::new(
            TerminalPtySize::new(24, 80, 960, 576),
            theme,
        );

        assert_eq!(
            decoder.feed(b"\x1b[?996n"),
            vec![CompatibilityProtocolEvent::PtyWrite(
                b"\x1b[?997;2n".to_vec()
            )]
        );
    }

    #[test]
    fn reports_text_area_and_cell_pixel_sizes_after_resize() {
        let mut decoder = TerminalCompatibilityProtocolDecoder::new(
            TerminalPtySize::new(24, 80, 960, 576),
            TerminalColorTheme::default(),
        );

        assert_eq!(
            decoder.feed(b"\x1b[14t\x1b[16t"),
            vec![
                CompatibilityProtocolEvent::PtyWrite(b"\x1b[4;576;960t".to_vec()),
                CompatibilityProtocolEvent::PtyWrite(b"\x1b[6;24;12t".to_vec()),
            ]
        );

        decoder.resize(TerminalPtySize::new(20, 100, 1000, 400));
        assert_eq!(
            decoder.feed(b"\x9b14t\x9b16t"),
            vec![
                CompatibilityProtocolEvent::PtyWrite(b"\x1b[4;400;1000t".to_vec()),
                CompatibilityProtocolEvent::PtyWrite(b"\x1b[6;20;10t".to_vec()),
            ]
        );
    }

    #[test]
    fn preserves_unknown_csi_and_utf8_bytes_across_chunks() {
        let mut decoder = TerminalCompatibilityProtocolDecoder::new(
            TerminalPtySize::new(24, 80, 960, 576),
            TerminalColorTheme::default(),
        );

        let mut events = decoder.feed(&[0xe4, 0xbd]);
        events.extend(decoder.feed(&[0xa0, 0x1b, b'[', b'3']));
        events.extend(decoder.feed(b"1mtext"));

        assert_eq!(
            events,
            vec![
                CompatibilityProtocolEvent::Bytes(vec![0xe4, 0xbd]),
                CompatibilityProtocolEvent::Bytes(vec![0xa0]),
                CompatibilityProtocolEvent::Bytes(b"\x1b[31mtext".to_vec()),
            ]
        );
    }

    #[test]
    fn does_not_interpret_csi_queries_inside_osc_payloads() {
        let mut decoder = TerminalCompatibilityProtocolDecoder::new(
            TerminalPtySize::new(24, 80, 960, 576),
            TerminalColorTheme::default(),
        );
        let sequence = b"\x1b]0;literal \x1b[?996n title\x1b\\";

        assert_eq!(
            decoder.feed(sequence),
            vec![CompatibilityProtocolEvent::Bytes(sequence.to_vec())]
        );
    }

    #[test]
    fn does_not_interpret_csi_queries_inside_string_controls() {
        let mut decoder = TerminalCompatibilityProtocolDecoder::new(
            TerminalPtySize::new(24, 80, 960, 576),
            TerminalColorTheme::default(),
        );
        let sequence = b"\x1bPpayload \x1b[?996n\x1b\\";

        assert_eq!(
            decoder.feed(sequence),
            vec![CompatibilityProtocolEvent::Bytes(sequence.to_vec())]
        );
    }
}
