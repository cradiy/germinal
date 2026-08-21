use std::collections::HashMap;

use base64::{Engine as _, engine::general_purpose};
use germinal_ports::pty_host::terminal_notification::{
    TerminalNotification, TerminalNotificationOccasion,
};

const MAX_CONTROL_SEQUENCE_BYTES: usize = 16 * 1024;
const MAX_NOTIFICATION_TEXT_BYTES: usize = 16 * 1024;
const MAX_PENDING_NOTIFICATIONS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotificationProtocolEvent {
    Bytes(Vec<u8>),
    Notification(TerminalNotification),
    PtyWrite(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DecoderState {
    Ground,
    Escape,
    Osc {
        raw: Vec<u8>,
        data: Vec<u8>,
        escape: bool,
    },
    PassthroughOsc {
        escape: bool,
    },
}

#[derive(Debug, Clone)]
struct PendingNotification {
    title: String,
    body: String,
    occasion: TerminalNotificationOccasion,
    focus_on_activation: bool,
}

impl Default for PendingNotification {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            occasion: TerminalNotificationOccasion::Always,
            focus_on_activation: true,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TerminalNotificationProtocolDecoder {
    state: DecoderState,
    utf8_continuations: u8,
    pending: HashMap<String, PendingNotification>,
    pending_anonymous: Option<PendingNotification>,
}

impl Default for TerminalNotificationProtocolDecoder {
    fn default() -> Self {
        Self {
            state: DecoderState::Ground,
            utf8_continuations: 0,
            pending: HashMap::new(),
            pending_anonymous: None,
        }
    }
}

impl TerminalNotificationProtocolDecoder {
    pub(crate) fn feed(&mut self, input: &[u8]) -> Vec<NotificationProtocolEvent> {
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
                    0x9d if !utf8_continuation => DecoderState::Osc {
                        raw: vec![0x9d],
                        data: Vec::new(),
                        escape: false,
                    },
                    _ => {
                        visible.push(byte);
                        DecoderState::Ground
                    }
                },
                DecoderState::Escape => {
                    if byte == b']' {
                        DecoderState::Osc {
                            raw: vec![0x1b, b']'],
                            data: Vec::new(),
                            escape: false,
                        }
                    } else {
                        visible.extend_from_slice(&[0x1b, byte]);
                        DecoderState::Ground
                    }
                }
                DecoderState::Osc {
                    mut raw,
                    mut data,
                    escape,
                } => {
                    raw.push(byte);
                    if byte == 0x07
                        || (byte == 0x9c && !utf8_continuation)
                        || (escape && byte == b'\\')
                    {
                        flush_visible(&mut events, &mut visible);
                        if let Some(protocol_events) = self.parse_osc(&data) {
                            events.extend(protocol_events);
                        } else {
                            visible.extend_from_slice(&raw);
                        }
                        DecoderState::Ground
                    } else if raw.len() > MAX_CONTROL_SEQUENCE_BYTES {
                        visible.extend_from_slice(&raw);
                        DecoderState::PassthroughOsc {
                            escape: byte == 0x1b,
                        }
                    } else {
                        if escape {
                            data.push(0x1b);
                        }
                        if byte != 0x1b {
                            data.push(byte);
                        }
                        DecoderState::Osc {
                            raw,
                            data,
                            escape: byte == 0x1b,
                        }
                    }
                }
                DecoderState::PassthroughOsc { escape } => {
                    visible.push(byte);
                    if byte == 0x07
                        || (byte == 0x9c && !utf8_continuation)
                        || (escape && byte == b'\\')
                    {
                        DecoderState::Ground
                    } else {
                        DecoderState::PassthroughOsc {
                            escape: byte == 0x1b,
                        }
                    }
                }
            };
        }

        flush_visible(&mut events, &mut visible);
        events
    }

    fn parse_osc(&mut self, data: &[u8]) -> Option<Vec<NotificationProtocolEvent>> {
        if let Some(payload) = data.strip_prefix(b"9;") {
            return Some(self.parse_legacy_notification(payload));
        }

        let rest = data.strip_prefix(b"99;")?;
        let Some(separator) = rest.iter().position(|byte| *byte == b';') else {
            return Some(Vec::new());
        };
        let metadata = parse_metadata(&rest[..separator]);
        let payload = &rest[separator + 1..];
        Some(self.parse_kitty_notification(&metadata, payload))
    }

    fn parse_legacy_notification(&self, payload: &[u8]) -> Vec<NotificationProtocolEvent> {
        let Some(body) = decode_plain_text(payload) else {
            return Vec::new();
        };
        let body = body.trim().to_owned();
        if body.is_empty() {
            return Vec::new();
        }

        vec![NotificationProtocolEvent::Notification(
            TerminalNotification::new(None, Some(body), TerminalNotificationOccasion::Always),
        )]
    }

    fn parse_kitty_notification(
        &mut self,
        metadata: &HashMap<char, &str>,
        payload: &[u8],
    ) -> Vec<NotificationProtocolEvent> {
        let identifier = metadata.get(&'i').copied().filter(|id| !id.is_empty());
        let payload_type = metadata.get(&'p').copied().unwrap_or("title");

        if payload_type == "?" {
            return vec![NotificationProtocolEvent::PtyWrite(query_response(
                identifier.unwrap_or("0"),
            ))];
        }
        if !matches!(payload_type, "title" | "body") {
            return Vec::new();
        }

        let encoded = metadata.get(&'e').is_some_and(|value| *value == "1");
        let Some(text) = decode_payload(payload, encoded) else {
            return Vec::new();
        };
        let done = !metadata.get(&'d').is_some_and(|value| *value == "0");
        let occasion = notification_occasion(metadata.get(&'o').copied());
        let mut pending = self.take_pending(identifier);
        pending.occasion = occasion;
        if let Some(actions) = metadata.get(&'a') {
            pending.focus_on_activation = focus_action_enabled(actions);
        }
        match payload_type {
            "title" => append_bounded(&mut pending.title, &text),
            "body" => append_bounded(&mut pending.body, &text),
            _ => unreachable!(),
        }

        if !done {
            self.store_pending(identifier, pending);
            return Vec::new();
        }

        let title = non_empty(pending.title);
        let body = non_empty(pending.body);
        if title.is_none() && body.is_none() {
            return Vec::new();
        }

        let mut notification = TerminalNotification::new(title, body, pending.occasion);
        notification.focus_on_activation = pending.focus_on_activation;
        vec![NotificationProtocolEvent::Notification(notification)]
    }

    fn take_pending(&mut self, identifier: Option<&str>) -> PendingNotification {
        match identifier {
            Some(identifier) => self.pending.remove(identifier).unwrap_or_default(),
            None => self.pending_anonymous.take().unwrap_or_default(),
        }
    }

    fn store_pending(&mut self, identifier: Option<&str>, pending: PendingNotification) {
        match identifier {
            Some(identifier) => {
                if self.pending.contains_key(identifier)
                    || self.pending.len() < MAX_PENDING_NOTIFICATIONS
                {
                    self.pending.insert(identifier.to_owned(), pending);
                }
            }
            None => self.pending_anonymous = Some(pending),
        }
    }
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

fn flush_visible(events: &mut Vec<NotificationProtocolEvent>, visible: &mut Vec<u8>) {
    if !visible.is_empty() {
        events.push(NotificationProtocolEvent::Bytes(std::mem::take(visible)));
    }
}

fn parse_metadata(bytes: &[u8]) -> HashMap<char, &str> {
    let Ok(metadata) = std::str::from_utf8(bytes) else {
        return HashMap::new();
    };
    metadata
        .split(':')
        .filter_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            let mut chars = key.chars();
            let key = chars.next()?;
            (chars.next().is_none() && key.is_ascii_alphabetic()).then_some((key, value))
        })
        .collect()
}

fn decode_payload(payload: &[u8], encoded: bool) -> Option<String> {
    if !encoded {
        return decode_plain_text(payload);
    }
    let decoded = general_purpose::STANDARD
        .decode(payload)
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(payload))
        .ok()?;
    decode_plain_text(&decoded)
}

fn decode_plain_text(bytes: &[u8]) -> Option<String> {
    (bytes.len() <= MAX_NOTIFICATION_TEXT_BYTES)
        .then(|| std::str::from_utf8(bytes).ok().map(str::to_owned))
        .flatten()
}

fn notification_occasion(value: Option<&str>) -> TerminalNotificationOccasion {
    match value {
        Some("unfocused") => TerminalNotificationOccasion::Unfocused,
        Some("invisible") => TerminalNotificationOccasion::Invisible,
        _ => TerminalNotificationOccasion::Always,
    }
}

fn focus_action_enabled(actions: &str) -> bool {
    actions
        .split(',')
        .fold(true, |enabled, action| match action {
            "focus" => true,
            "-focus" => false,
            _ => enabled,
        })
}

fn append_bounded(target: &mut String, text: &str) {
    let remaining = MAX_NOTIFICATION_TEXT_BYTES.saturating_sub(target.len());
    target.extend(
        text.chars()
            .take_while(|character| character.len_utf8() <= remaining)
            .scan(0, |used, character| {
                *used += character.len_utf8();
                (*used <= remaining).then_some(character)
            }),
    );
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

fn query_response(identifier: &str) -> Vec<u8> {
    format!("\x1b]99;i={identifier}:p=?;a=focus:p=title,body:o=always,unfocused,invisible\x1b\\")
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_legacy_osc_9_and_preserves_surrounding_text() {
        let mut decoder = TerminalNotificationProtocolDecoder::default();
        let events = decoder.feed(b"before\x1b]9;build finished\x07after");

        assert_eq!(
            events,
            vec![
                NotificationProtocolEvent::Bytes(b"before".to_vec()),
                NotificationProtocolEvent::Notification(TerminalNotification::new(
                    None,
                    Some("build finished".to_owned()),
                    TerminalNotificationOccasion::Always,
                )),
                NotificationProtocolEvent::Bytes(b"after".to_vec()),
            ]
        );
    }

    #[test]
    fn assembles_chunked_kitty_title_and_base64_body() {
        let mut decoder = TerminalNotificationProtocolDecoder::default();
        assert_eq!(decoder.feed(b"\x1b]99;i=build:d=0;Cargo\x1b\\"), Vec::new());
        let events = decoder.feed(b"\x1b]99;i=build:p=body:e=1:o=unfocused;VGVzdHMgcGFzc2Vk\x1b\\");

        assert_eq!(
            events,
            vec![NotificationProtocolEvent::Notification(
                TerminalNotification::new(
                    Some("Cargo".to_owned()),
                    Some("Tests passed".to_owned()),
                    TerminalNotificationOccasion::Unfocused,
                )
            )]
        );
    }

    #[test]
    fn supports_control_sequences_split_across_input_chunks() {
        let mut decoder = TerminalNotificationProtocolDecoder::default();
        assert!(decoder.feed(b"\x1b]99;;hel").is_empty());
        assert_eq!(
            decoder.feed(b"lo\x1b\\"),
            vec![NotificationProtocolEvent::Notification(
                TerminalNotification::new(
                    Some("hello".to_owned()),
                    None,
                    TerminalNotificationOccasion::Always,
                )
            )]
        );
    }

    #[test]
    fn answers_kitty_capability_queries() {
        let mut decoder = TerminalNotificationProtocolDecoder::default();
        assert_eq!(
            decoder.feed(b"\x1b]99;i=query:p=?;\x1b\\"),
            vec![NotificationProtocolEvent::PtyWrite(
                b"\x1b]99;i=query:p=?;a=focus:p=title,body:o=always,unfocused,invisible\x1b\\"
                    .to_vec()
            )]
        );
    }

    #[test]
    fn preserves_unrelated_osc_sequences() {
        let mut decoder = TerminalNotificationProtocolDecoder::default();
        let sequence = b"\x1b]0;window title\x1b\\";
        assert_eq!(
            decoder.feed(sequence),
            vec![NotificationProtocolEvent::Bytes(sequence.to_vec())]
        );
    }

    #[test]
    fn preserves_utf8_continuation_bytes_that_match_c1_controls() {
        let mut decoder = TerminalNotificationProtocolDecoder::default();
        let text = "before ❯ hello ✔ after";

        assert_eq!(
            decoder.feed(text.as_bytes()),
            vec![NotificationProtocolEvent::Bytes(text.as_bytes().to_vec())]
        );
    }

    #[test]
    fn preserves_utf8_continuation_bytes_across_input_chunks() {
        let mut decoder = TerminalNotificationProtocolDecoder::default();
        let bytes = "❯ hello".as_bytes();

        assert_eq!(
            decoder.feed(&bytes[..1]),
            vec![NotificationProtocolEvent::Bytes(bytes[..1].to_vec())]
        );
        assert_eq!(
            decoder.feed(&bytes[1..]),
            vec![NotificationProtocolEvent::Bytes(bytes[1..].to_vec())]
        );
    }

    #[test]
    fn preserves_utf8_c1_like_bytes_in_notification_payloads() {
        let mut decoder = TerminalNotificationProtocolDecoder::default();

        assert_eq!(
            decoder.feed("\x1b]99;;build ✔ done\x1b\\".as_bytes()),
            vec![NotificationProtocolEvent::Notification(
                TerminalNotification::new(
                    Some("build ✔ done".to_owned()),
                    None,
                    TerminalNotificationOccasion::Always,
                )
            )]
        );
    }

    #[test]
    fn kitty_notifications_can_disable_source_focus() {
        let mut decoder = TerminalNotificationProtocolDecoder::default();
        let events = decoder.feed(b"\x1b]99;a=-focus;Background update\x1b\\");

        let NotificationProtocolEvent::Notification(notification) = &events[0] else {
            panic!("expected a notification event");
        };
        assert!(!notification.focus_on_activation);
    }
}
