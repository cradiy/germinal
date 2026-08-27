use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File},
    io::{Cursor, Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::ZlibDecoder;
use germinal_ports::rendering::surface_snapshot::RenderSurfaceImageSnapshot;

const MAX_APC_BYTES: usize = 256 * 1024;
const MAX_ENCODED_IMAGE_BYTES: usize = 96 * 1024 * 1024;
const MAX_DECODED_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 8_192;
const MAX_IMAGES: usize = 256;
const MAX_TOTAL_IMAGE_BYTES: usize = 128 * 1024 * 1024;
const MAX_RELATIVE_PLACEMENT_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KittyStreamEvent {
    Bytes(Vec<u8>),
    Command(KittyCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KittyCommand {
    control: HashMap<u8, String>,
    payload: Vec<u8>,
}

impl KittyCommand {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let separator = bytes.iter().position(|byte| *byte == b';');
        let (control_bytes, payload) = separator
            .map(|index| (&bytes[..index], bytes[index + 1..].to_vec()))
            .unwrap_or((bytes, Vec::new()));
        let mut control = HashMap::new();

        for pair in control_bytes
            .split(|byte| *byte == b',')
            .filter(|pair| !pair.is_empty())
        {
            let equals = pair.iter().position(|byte| *byte == b'=')?;
            if equals != 1 {
                return None;
            }
            let value = std::str::from_utf8(&pair[equals + 1..]).ok()?.to_owned();
            control.insert(pair[0], value);
        }

        Some(Self { control, payload })
    }

    fn char(&self, key: u8, default: char) -> char {
        self.control
            .get(&key)
            .and_then(|value| value.as_bytes().first().copied())
            .map(char::from)
            .unwrap_or(default)
    }

    fn u32(&self, key: u8, default: u32) -> Result<u32, KittyGraphicsError> {
        match self.control.get(&key) {
            Some(value) => value
                .parse()
                .map_err(|_| KittyGraphicsError::InvalidControl),
            None => Ok(default),
        }
    }

    fn i32(&self, key: u8, default: i32) -> Result<i32, KittyGraphicsError> {
        match self.control.get(&key) {
            Some(value) => value
                .parse()
                .map_err(|_| KittyGraphicsError::InvalidControl),
            None => Ok(default),
        }
    }

    fn has_only_chunk_continuation_keys(&self, action: char) -> bool {
        self.control
            .keys()
            .all(|key| matches!(key, b'm' | b'q' | b'a'))
            && match self.control.get(&b'a') {
                Some(value) => value.as_bytes() == [action as u8],
                None => action != 'f',
            }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DecoderState {
    Ground,
    Escape,
    ApcPrefix,
    KittyApc {
        bytes: Vec<u8>,
        escape: bool,
        oversized: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KittyGraphicsStreamDecoder {
    state: DecoderState,
}

impl Default for KittyGraphicsStreamDecoder {
    fn default() -> Self {
        Self {
            state: DecoderState::Ground,
        }
    }
}

impl KittyGraphicsStreamDecoder {
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.state, DecoderState::Ground)
    }

    pub(crate) fn can_passthrough(&self, input: &[u8]) -> bool {
        self.is_idle() && contains_no_kitty_apc(input)
    }

    pub(crate) fn feed(&mut self, input: &[u8]) -> Vec<KittyStreamEvent> {
        if self.can_passthrough(input) {
            return vec![KittyStreamEvent::Bytes(input.to_vec())];
        }

        let mut events = Vec::new();
        let mut visible = Vec::new();

        for &byte in input {
            match &mut self.state {
                DecoderState::Ground => match byte {
                    0x1b => self.state = DecoderState::Escape,
                    0x9f => visible.push(byte),
                    _ => visible.push(byte),
                },
                DecoderState::Escape => {
                    if byte == b'_' {
                        self.state = DecoderState::ApcPrefix;
                    } else {
                        visible.extend_from_slice(&[0x1b, byte]);
                        self.state = DecoderState::Ground;
                    }
                }
                DecoderState::ApcPrefix => {
                    if byte == b'G' {
                        flush_visible(&mut events, &mut visible);
                        self.state = DecoderState::KittyApc {
                            bytes: Vec::new(),
                            escape: false,
                            oversized: false,
                        };
                    } else {
                        visible.extend_from_slice(&[0x1b, b'_', byte]);
                        self.state = DecoderState::Ground;
                    }
                }
                DecoderState::KittyApc {
                    bytes,
                    escape,
                    oversized,
                } => {
                    if byte == 0x9c || (*escape && byte == b'\\') {
                        if !*oversized && let Some(command) = KittyCommand::parse(bytes) {
                            events.push(KittyStreamEvent::Command(command));
                        }
                        self.state = DecoderState::Ground;
                        continue;
                    }

                    if *escape {
                        push_apc_byte(bytes, oversized, 0x1b);
                        *escape = false;
                    }
                    if byte == 0x1b {
                        *escape = true;
                    } else {
                        push_apc_byte(bytes, oversized, byte);
                    }
                }
            }
        }

        flush_visible(&mut events, &mut visible);
        events
    }
}

fn contains_no_kitty_apc(input: &[u8]) -> bool {
    if input.is_empty() || std::str::from_utf8(input).is_err() {
        return false;
    }

    let mut cursor = 0;
    while let Some(relative_escape) = input[cursor..].iter().position(|byte| *byte == 0x1b) {
        let escape = cursor + relative_escape;
        let Some(next) = input.get(escape + 1) else {
            return false;
        };
        if *next == b'_' {
            let Some(prefix) = input.get(escape + 2) else {
                return false;
            };
            if *prefix == b'G' {
                return false;
            }
        }
        cursor = escape + 1;
    }
    true
}

fn push_apc_byte(bytes: &mut Vec<u8>, oversized: &mut bool, byte: u8) {
    if bytes.len() < MAX_APC_BYTES {
        bytes.push(byte);
    } else {
        *oversized = true;
    }
}

fn flush_visible(events: &mut Vec<KittyStreamEvent>, visible: &mut Vec<u8>) {
    if !visible.is_empty() {
        events.push(KittyStreamEvent::Bytes(std::mem::take(visible)));
    }
}

#[derive(Debug, Clone)]
struct KittyImage {
    key: u64,
    image_id: u32,
    width_px: u32,
    height_px: u32,
    frames: Vec<KittyAnimationFrame>,
    animation: KittyAnimation,
}

#[derive(Debug, Clone)]
struct KittyAnimationFrame {
    generation: u64,
    rgba: Arc<[u8]>,
    gap_ms: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KittyAnimationState {
    Stopped,
    Loading,
    Running,
}

#[derive(Debug, Clone)]
struct KittyAnimation {
    state: KittyAnimationState,
    current_frame: usize,
    loop_limit: Option<u32>,
    completed_loops: u32,
    next_frame_at: Option<Instant>,
}

impl Default for KittyAnimation {
    fn default() -> Self {
        Self {
            state: KittyAnimationState::Stopped,
            current_frame: 0,
            loop_limit: None,
            completed_loops: 0,
            next_frame_at: None,
        }
    }
}

impl KittyImage {
    fn current_frame(&self) -> &KittyAnimationFrame {
        &self.frames[self.animation.current_frame.min(self.frames.len() - 1)]
    }

    fn byte_len(&self) -> usize {
        self.frames.iter().map(|frame| frame.rgba.len()).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KittyPlacement {
    key: u64,
    image_key: u64,
    image_id: u32,
    placement_id: u32,
    x_cell: u32,
    y_cell: u32,
    x_offset_px: u32,
    y_offset_px: u32,
    columns: u32,
    rows: u32,
    occupied_columns: u32,
    occupied_rows: u32,
    source_x_px: u32,
    source_y_px: u32,
    source_width_px: u32,
    source_height_px: u32,
    z_index: i32,
    relative_to: Option<u64>,
    horizontal_offset: i32,
    vertical_offset: i32,
    scroll_offset_y: i64,
    clip_top_cell: Option<i64>,
    clip_bottom_cell: Option<i64>,
}

#[derive(Debug, Clone, Default)]
struct KittyScreenState {
    placements: HashMap<u64, KittyPlacement>,
    virtual_placements: HashMap<u64, KittyPlacement>,
    resolved_positions: HashMap<u64, (i64, i64)>,
}

impl KittyScreenState {
    fn image_is_referenced(&self, image_key: u64) -> bool {
        self.placements
            .values()
            .chain(self.virtual_placements.values())
            .any(|placement| placement.image_key == image_key)
    }

    fn remove_image_placements(&mut self, image_key: u64) -> HashSet<u64> {
        let roots = self
            .placements
            .values()
            .chain(self.virtual_placements.values())
            .filter(|placement| placement.image_key == image_key)
            .map(|placement| placement.key)
            .collect::<HashSet<_>>();
        let mut removed = roots.clone();

        loop {
            let children = self
                .placements
                .values()
                .chain(self.virtual_placements.values())
                .filter_map(|placement| {
                    placement
                        .relative_to
                        .filter(|parent| removed.contains(parent))
                        .map(|_| placement.key)
                })
                .collect::<Vec<_>>();
            let old_len = removed.len();
            removed.extend(children);
            if removed.len() == old_len {
                break;
            }
        }

        let descendant_images = removed
            .difference(&roots)
            .filter_map(|key| {
                self.placements
                    .get(key)
                    .or_else(|| self.virtual_placements.get(key))
            })
            .map(|placement| placement.image_key)
            .filter(|key| *key != image_key)
            .collect();
        self.placements.retain(|key, _| !removed.contains(key));
        self.virtual_placements
            .retain(|key, _| !removed.contains(key));
        self.resolved_positions
            .retain(|key, _| !removed.contains(key));
        descendant_images
    }
}

#[derive(Debug, Clone)]
struct PendingTransmission {
    command: KittyCommand,
    encoded_payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KittyCursorMove {
    pub columns: u32,
    pub rows: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct KittyPlaceholderCell {
    pub image_id: u32,
    pub placement_id: u32,
    pub x_cell: u32,
    pub y_cell: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct KittyCommandResult {
    pub response: Option<Vec<u8>>,
    pub cursor_move: Option<KittyCursorMove>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyImageReference {
    image_key: u64,
    image_id: u32,
    image_number: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct KittyGraphicsState {
    images: HashMap<u64, KittyImage>,
    image_ids: HashMap<u32, u64>,
    image_numbers: HashMap<u32, Vec<u64>>,
    placements: HashMap<u64, KittyPlacement>,
    virtual_placements: HashMap<u64, KittyPlacement>,
    primary_screen: Option<KittyScreenState>,
    insertion_order: VecDeque<u64>,
    pending: Option<PendingTransmission>,
    next_key: u64,
    next_generation: u64,
    next_image_id: u32,
    total_bytes: usize,
    resolved_positions: HashMap<u64, (i64, i64)>,
}

impl KittyGraphicsState {
    pub(crate) fn has_any_placements(&self) -> bool {
        !self.placements.is_empty()
            || !self.virtual_placements.is_empty()
            || self.primary_screen.as_ref().is_some_and(|screen| {
                !screen.placements.is_empty() || !screen.virtual_placements.is_empty()
            })
    }

    pub(crate) fn advance_animations(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for image in self.images.values_mut() {
            changed |= advance_image_animation(image, now);
        }
        changed
    }

    pub(crate) fn enter_alternate_screen(&mut self) {
        if self.primary_screen.is_some() {
            return;
        }

        self.primary_screen = Some(KittyScreenState {
            placements: std::mem::take(&mut self.placements),
            virtual_placements: std::mem::take(&mut self.virtual_placements),
            resolved_positions: std::mem::take(&mut self.resolved_positions),
        });
    }

    pub(crate) fn leave_alternate_screen(&mut self) {
        let Some(primary) = self.primary_screen.take() else {
            return;
        };

        self.placements = primary.placements;
        self.virtual_placements = primary.virtual_placements;
        self.resolved_positions = primary.resolved_positions;
    }

    pub(crate) fn reset_terminal(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn clear_screen_all(&mut self) {
        let roots = self.placements.keys().copied().collect();
        self.remove_placement_trees(roots, false);
    }

    pub(crate) fn scroll_up(&mut self, start_row: u32, end_row: u32, lines: u32) {
        self.scroll_screen_region(start_row, end_row, -i64::from(lines));
    }

    pub(crate) fn scroll_down(&mut self, start_row: u32, end_row: u32, lines: u32) {
        self.scroll_screen_region(start_row, end_row, i64::from(lines));
    }

    fn scroll_screen_region(&mut self, start_row: u32, end_row: u32, delta: i64) {
        if delta == 0 || start_row >= end_row {
            return;
        }

        let start_row = i64::from(start_row);
        let end_row = i64::from(end_row);
        let roots = self
            .placements
            .values()
            .filter(|placement| placement.relative_to.is_none())
            .filter(|placement| {
                let origin = i64::from(placement.y_cell).saturating_add(placement.scroll_offset_y);
                let visible_top = placement.clip_top_cell.unwrap_or(origin);
                let visible_bottom = placement
                    .clip_bottom_cell
                    .unwrap_or_else(|| origin.saturating_add(i64::from(placement.occupied_rows)));
                visible_top >= start_row && visible_bottom <= end_row
            })
            .map(|placement| placement.key)
            .collect::<Vec<_>>();

        let mut fully_clipped = Vec::new();
        for key in roots {
            if let Some(placement) = self.placements.get_mut(&key) {
                let origin = i64::from(placement.y_cell).saturating_add(placement.scroll_offset_y);
                let visible_top = placement.clip_top_cell.unwrap_or(origin);
                let visible_bottom = placement
                    .clip_bottom_cell
                    .unwrap_or_else(|| origin.saturating_add(i64::from(placement.occupied_rows)));
                placement.scroll_offset_y = placement.scroll_offset_y.saturating_add(delta);
                let clipped_top = visible_top.saturating_add(delta).max(start_row);
                let clipped_bottom = visible_bottom.saturating_add(delta).min(end_row);
                if clipped_top >= clipped_bottom {
                    fully_clipped.push(key);
                } else {
                    placement.clip_top_cell = Some(clipped_top);
                    placement.clip_bottom_cell = Some(clipped_bottom);
                }
            }
        }
        self.remove_placement_trees(fully_clipped.into_iter().collect(), false);
        self.resolved_positions.clear();
    }

    pub(crate) fn command_uses_terminal_cursor(&self, command: &KittyCommand) -> bool {
        let effective_command = if let Some(pending) = &self.pending {
            if command.u32(b'm', 0).unwrap_or(0) == 1 {
                return false;
            }
            &pending.command
        } else {
            command
        };

        matches!(effective_command.char(b'a', 't'), 'T' | 'p' | 'd')
    }

    #[cfg(test)]
    pub(crate) fn handle(
        &mut self,
        command: KittyCommand,
        cursor: (u32, u32),
    ) -> KittyCommandResult {
        self.handle_with_cell_size(command, cursor, (1, 1))
    }

    pub(crate) fn handle_with_cell_size(
        &mut self,
        command: KittyCommand,
        cursor: (u32, u32),
        cell_size_px: (u32, u32),
    ) -> KittyCommandResult {
        let response_command = self
            .pending
            .as_ref()
            .map(|pending| &pending.command)
            .unwrap_or(&command);
        let quiet = response_command.u32(b'q', 0).unwrap_or(0);
        let image_id = response_command.u32(b'i', 0).unwrap_or(0);
        let image_number = response_command.u32(b'I', 0).unwrap_or(0);
        let placement_id = response_command.u32(b'p', 0).unwrap_or(0);
        let result = self.handle_inner(command, cursor, cell_size_px);
        match result {
            Ok(mut command_result) => {
                if quiet != 0 {
                    command_result.response = None;
                }
                command_result
            }
            Err(error) => KittyCommandResult {
                response: if quiet == 2 {
                    None
                } else {
                    response_for(image_id, image_number, placement_id, error.message())
                },
                cursor_move: None,
            },
        }
    }

    fn handle_inner(
        &mut self,
        mut command: KittyCommand,
        cursor: (u32, u32),
        cell_size_px: (u32, u32),
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        validate_image_reference(&command)?;
        if command.u32(b'U', 0)? > 1 {
            return Err(KittyGraphicsError::InvalidControl);
        }
        if self.pending.is_some() {
            if command.char(b'a', 't') == 'd' {
                self.pending = None;
                return self.delete(command, cursor);
            }
            let pending_action = self
                .pending
                .as_ref()
                .map(|pending| pending.command.char(b'a', 't'))
                .unwrap_or('t');
            if !command.has_only_chunk_continuation_keys(pending_action) {
                self.pending = None;
                return Err(KittyGraphicsError::InvalidChunk);
            }
            let pending = self
                .pending
                .as_mut()
                .expect("pending transmission must exist");
            append_encoded_payload(&mut pending.encoded_payload, &command.payload)?;
            if command.u32(b'm', 0)? == 1 {
                return Ok(KittyCommandResult::default());
            }
            let pending = self
                .pending
                .take()
                .expect("pending transmission must exist");
            command = pending.command;
            command.payload = pending.encoded_payload;
            command.control.insert(b'm', "0".to_owned());
        }

        let action = command.char(b'a', 't');
        if matches!(action, 't' | 'T' | 'q' | 'f') && command.u32(b'm', 0)? == 1 {
            let mut encoded_payload = Vec::new();
            append_encoded_payload(&mut encoded_payload, &command.payload)?;
            command.payload.clear();
            self.pending = Some(PendingTransmission {
                command,
                encoded_payload,
            });
            return Ok(KittyCommandResult::default());
        }

        match action {
            't' | 'T' | 'q' => self.transmit(command, cursor, cell_size_px, action),
            'f' => self.transmit_animation_frame(command),
            'a' => self.control_animation(command, Instant::now()),
            'c' => self.compose_animation_frames(command),
            'p' => self.place(command, cursor, cell_size_px),
            'd' => self.delete(command, cursor),
            _ => Err(KittyGraphicsError::UnsupportedAction),
        }
    }

    fn transmit(
        &mut self,
        command: KittyCommand,
        cursor: (u32, u32),
        cell_size_px: (u32, u32),
        action: char,
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        let requested_image_id = command.u32(b'i', 0)?;
        let image_number = command.u32(b'I', 0)?;
        let placement_id = command.u32(b'p', 0)?;
        let encoded = transmission_bytes(&command)?;
        let bytes = if command.char(b'o', '\0') == 'z' {
            decompress_zlib(&encoded)?
        } else if command.control.contains_key(&b'o') {
            return Err(KittyGraphicsError::UnsupportedCompression);
        } else {
            encoded
        };
        let decoded = decode_image(&command, &bytes)?;

        if action == 'q' {
            return Ok(KittyCommandResult {
                response: response_for(requested_image_id, image_number, placement_id, "OK"),
                cursor_move: None,
            });
        }

        let image_id = if image_number == 0 {
            requested_image_id
        } else {
            self.allocate_image_id()?
        };
        let image_key = self.insert_image(image_id, image_number, decoded);
        let cursor_move = if action == 'T' && command.u32(b'U', 0)? == 1 {
            self.insert_virtual_placement(&command, image_key, image_id, cell_size_px)?;
            None
        } else if action == 'T' {
            Some(self.insert_placement(&command, cursor, image_key, image_id, cell_size_px)?)
        } else {
            None
        };

        Ok(KittyCommandResult {
            response: response_for(image_id, image_number, placement_id, "OK"),
            cursor_move,
        })
    }

    fn transmit_animation_frame(
        &mut self,
        command: KittyCommand,
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        let image = self.resolve_image(&command)?;
        let encoded = transmission_bytes(&command)?;
        let bytes = if command.char(b'o', '\0') == 'z' {
            decompress_zlib(&encoded)?
        } else if command.control.contains_key(&b'o') {
            return Err(KittyGraphicsError::UnsupportedCompression);
        } else {
            encoded
        };
        let decoded = decode_image(&command, &bytes)?;
        let destination_x = command.u32(b'x', 0)?;
        let destination_y = command.u32(b'y', 0)?;
        let edit_frame = command.u32(b'r', 0)?;
        let base_frame = command.u32(b'c', 0)?;
        let composition = command.u32(b'X', 0)?;
        if composition > 1 {
            return Err(KittyGraphicsError::InvalidControl);
        }
        let frame_gap = nonzero_frame_gap(&command)?;

        let (width_px, height_px, existing_frames) = {
            let image = self
                .images
                .get(&image.image_key)
                .ok_or(KittyGraphicsError::ImageNotFound)?;
            (image.width_px, image.height_px, image.frames.len())
        };
        validate_rect(
            destination_x,
            destination_y,
            decoded.width_px,
            decoded.height_px,
            width_px,
            height_px,
        )?;
        let edit_index = optional_frame_index(edit_frame, existing_frames)?;
        let base_index = optional_frame_index(base_frame, existing_frames)?;
        let mut frame = if let Some(index) = edit_index {
            self.images[&image.image_key].frames[index].rgba.to_vec()
        } else if let Some(index) = base_index {
            self.images[&image.image_key].frames[index].rgba.to_vec()
        } else {
            solid_frame(width_px, height_px, command.u32(b'Y', 0)?)?
        };
        composite_rgba_rect(
            &mut frame,
            width_px,
            (destination_x, destination_y),
            &decoded.rgba,
            (decoded.width_px, decoded.height_px),
            composition == 1,
        )?;

        let generation = self.allocate_generation();
        let now = Instant::now();
        let image_state = self
            .images
            .get_mut(&image.image_key)
            .ok_or(KittyGraphicsError::ImageNotFound)?;
        if let Some(index) = edit_index {
            let existing_gap = image_state.frames[index].gap_ms;
            image_state.frames[index] = KittyAnimationFrame {
                generation,
                rgba: frame.into(),
                gap_ms: frame_gap.or(existing_gap),
            };
            if index == image_state.animation.current_frame && frame_gap.is_some() {
                schedule_next_frame(image_state, now);
            }
        } else {
            let was_loading_at_end = image_state.animation.state == KittyAnimationState::Loading
                && image_state.animation.current_frame + 1 == image_state.frames.len()
                && image_state.animation.next_frame_at.is_none();
            image_state.frames.push(KittyAnimationFrame {
                generation,
                rgba: frame.into(),
                gap_ms: frame_gap,
            });
            self.total_bytes = self
                .total_bytes
                .saturating_add(frame_byte_len(width_px, height_px)?);
            if was_loading_at_end {
                image_state.animation.next_frame_at = Some(now);
            }
        }
        self.evict_to_quota();

        Ok(KittyCommandResult {
            response: response_for(image.image_id, image.image_number, 0, "OK"),
            cursor_move: None,
        })
    }

    fn control_animation(
        &mut self,
        command: KittyCommand,
        now: Instant,
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        let image = self.resolve_image(&command)?;
        let current_frame = command.u32(b'c', 0)?;
        let affected_frame = command.u32(b'r', 0)?;
        let state = command.u32(b's', 0)?;
        let loops = command.u32(b'v', 0)?;
        if state > 3 {
            return Err(KittyGraphicsError::InvalidControl);
        }

        let image_state = self
            .images
            .get_mut(&image.image_key)
            .ok_or(KittyGraphicsError::ImageNotFound)?;
        if current_frame != 0 {
            image_state.animation.current_frame =
                required_frame_index(current_frame, image_state.frames.len())?;
        }
        if affected_frame != 0 {
            let index = required_frame_index(affected_frame, image_state.frames.len())?;
            if let Some(gap) = nonzero_frame_gap(&command)? {
                image_state.frames[index].gap_ms = Some(gap);
            }
        }
        if loops != 0 {
            image_state.animation.loop_limit = (loops != 1).then_some(loops - 1);
            image_state.animation.completed_loops = 0;
        }
        match state {
            0 => {}
            1 => {
                image_state.animation.state = KittyAnimationState::Stopped;
                image_state.animation.completed_loops = 0;
                image_state.animation.next_frame_at = None;
            }
            2 => {
                image_state.animation.state = KittyAnimationState::Loading;
                image_state.animation.completed_loops = 0;
                schedule_next_frame(image_state, now);
            }
            3 => {
                image_state.animation.state = KittyAnimationState::Running;
                image_state.animation.completed_loops = 0;
                schedule_next_frame(image_state, now);
            }
            _ => unreachable!(),
        }
        let affected_current_frame = affected_frame != 0
            && required_frame_index(affected_frame, image_state.frames.len())?
                == image_state.animation.current_frame;
        if state == 0 && (current_frame != 0 || affected_current_frame) {
            schedule_next_frame(image_state, now);
        }

        Ok(KittyCommandResult {
            response: response_for(image.image_id, image.image_number, 0, "OK"),
            cursor_move: None,
        })
    }

    fn compose_animation_frames(
        &mut self,
        command: KittyCommand,
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        let image = self.resolve_image(&command)?;
        let source_frame = command.u32(b'r', 0)?;
        let destination_frame = command.u32(b'c', 0)?;
        if source_frame == 0 || destination_frame == 0 {
            return Err(KittyGraphicsError::InvalidControl);
        }
        let source_x = command.u32(b'X', 0)?;
        let source_y = command.u32(b'Y', 0)?;
        let destination_x = command.u32(b'x', 0)?;
        let destination_y = command.u32(b'y', 0)?;
        let replace = match command.u32(b'C', 0)? {
            0 => false,
            1 => true,
            _ => return Err(KittyGraphicsError::InvalidControl),
        };

        let (width_px, height_px, source_index, destination_index) = {
            let image_state = self
                .images
                .get(&image.image_key)
                .ok_or(KittyGraphicsError::ImageNotFound)?;
            (
                image_state.width_px,
                image_state.height_px,
                required_frame_index(source_frame, image_state.frames.len())?,
                required_frame_index(destination_frame, image_state.frames.len())?,
            )
        };
        let rect_width = command.u32(b'w', width_px)?;
        let rect_height = command.u32(b'h', height_px)?;
        validate_rect(
            source_x,
            source_y,
            rect_width,
            rect_height,
            width_px,
            height_px,
        )?;
        validate_rect(
            destination_x,
            destination_y,
            rect_width,
            rect_height,
            width_px,
            height_px,
        )?;
        if source_index == destination_index
            && rects_overlap(
                (source_x, source_y, rect_width, rect_height),
                (destination_x, destination_y, rect_width, rect_height),
            )
        {
            return Err(KittyGraphicsError::InvalidControl);
        }

        let source = extract_rgba_rect(
            &self.images[&image.image_key].frames[source_index].rgba,
            width_px,
            source_x,
            source_y,
            rect_width,
            rect_height,
        )?;
        let mut destination = self.images[&image.image_key].frames[destination_index]
            .rgba
            .to_vec();
        composite_rgba_rect(
            &mut destination,
            width_px,
            (destination_x, destination_y),
            &source,
            (rect_width, rect_height),
            replace,
        )?;
        let generation = self.allocate_generation();
        let image_state = self
            .images
            .get_mut(&image.image_key)
            .ok_or(KittyGraphicsError::ImageNotFound)?;
        image_state.frames[destination_index].generation = generation;
        image_state.frames[destination_index].rgba = destination.into();

        Ok(KittyCommandResult {
            response: response_for(image.image_id, image.image_number, 0, "OK"),
            cursor_move: None,
        })
    }

    fn place(
        &mut self,
        command: KittyCommand,
        cursor: (u32, u32),
        cell_size_px: (u32, u32),
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        let image = self.resolve_image(&command)?;
        let placement_id = command.u32(b'p', 0)?;
        if command.u32(b'U', 0)? == 1 {
            self.insert_virtual_placement(&command, image.image_key, image.image_id, cell_size_px)?;
            return Ok(KittyCommandResult {
                response: response_for(image.image_id, image.image_number, placement_id, "OK"),
                cursor_move: None,
            });
        }
        let cursor_move = self.insert_placement(
            &command,
            cursor,
            image.image_key,
            image.image_id,
            cell_size_px,
        )?;

        Ok(KittyCommandResult {
            response: response_for(image.image_id, image.image_number, placement_id, "OK"),
            cursor_move: Some(cursor_move),
        })
    }

    fn delete(
        &mut self,
        command: KittyCommand,
        cursor: (u32, u32),
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        self.pending = None;
        let selector = command.char(b'd', 'a');
        let requested_image_id = command.u32(b'i', 0)?;
        let requested_image_number = command.u32(b'I', 0)?;
        let placement_id = command.u32(b'p', 0)?;
        let mut response_image_id = requested_image_id;

        match selector.to_ascii_lowercase() {
            'a' => {
                let roots = self.placements.keys().copied().collect();
                self.remove_placement_trees(roots, selector.is_ascii_uppercase());
            }
            'i' if requested_image_id != 0 && requested_image_number == 0 => {
                if let Some(image_key) = self.image_ids.get(&requested_image_id).copied() {
                    self.delete_image_placements(image_key, placement_id);
                    if selector.is_ascii_uppercase() {
                        self.remove_image(image_key);
                    }
                }
            }
            'n' if requested_image_number != 0 && requested_image_id == 0 => {
                if let Some(image_key) = self.newest_image_with_number(requested_image_number) {
                    response_image_id = self
                        .images
                        .get(&image_key)
                        .map_or(0, |image| image.image_id);
                    self.delete_image_placements(image_key, placement_id);
                    if selector.is_ascii_uppercase() {
                        self.remove_image(image_key);
                    }
                }
            }
            'f' => {
                let image = self.resolve_image(&command)?;
                let image_state = self
                    .images
                    .get_mut(&image.image_key)
                    .ok_or(KittyGraphicsError::ImageNotFound)?;
                let removed_bytes = image_state
                    .frames
                    .iter()
                    .skip(1)
                    .map(|frame| frame.rgba.len())
                    .sum::<usize>();
                image_state.frames.truncate(1);
                image_state.animation = KittyAnimation::default();
                self.total_bytes = self.total_bytes.saturating_sub(removed_bytes);
                response_image_id = image.image_id;
            }
            'c' => {
                self.delete_physical_placements(
                    |placement, origin| {
                        placement_intersects_cell(placement, origin, cursor.0, cursor.1)
                    },
                    selector.is_ascii_uppercase(),
                );
            }
            'p' => {
                let point = command_cell_point(&command)?;
                self.delete_physical_placements(
                    |placement, origin| {
                        placement_intersects_cell(placement, origin, point.0, point.1)
                    },
                    selector.is_ascii_uppercase(),
                );
            }
            'q' => {
                let point = command_cell_point(&command)?;
                let z_index = command.i32(b'z', 0)?;
                self.delete_physical_placements(
                    |placement, origin| {
                        placement.z_index == z_index
                            && placement_intersects_cell(placement, origin, point.0, point.1)
                    },
                    selector.is_ascii_uppercase(),
                );
            }
            'r' => {
                let start = command.u32(b'x', 0)?;
                let end = command.u32(b'y', u32::MAX)?;
                if start > end {
                    return Err(KittyGraphicsError::InvalidControl);
                }
                let keys: Vec<_> = self
                    .images
                    .values()
                    .filter(|image| (start..=end).contains(&image.image_id))
                    .map(|image| image.key)
                    .collect();
                for key in keys {
                    self.delete_image_placements(key, 0);
                    if selector.is_ascii_uppercase() {
                        self.remove_image(key);
                    }
                }
            }
            'x' => {
                let column = command_cell_coordinate(&command, b'x')?;
                self.delete_physical_placements(
                    |placement, origin| placement_intersects_column(placement, origin.0, column),
                    selector.is_ascii_uppercase(),
                );
            }
            'y' => {
                let row = command_cell_coordinate(&command, b'y')?;
                self.delete_physical_placements(
                    |placement, origin| placement_intersects_row(placement, origin.1, row),
                    selector.is_ascii_uppercase(),
                );
            }
            'z' => {
                let z_index = command.i32(b'z', 0)?;
                let roots = self
                    .placements
                    .values()
                    .filter(|placement| placement.z_index == z_index)
                    .map(|placement| placement.key)
                    .collect();
                self.remove_placement_trees(roots, selector.is_ascii_uppercase());
            }
            _ => return Err(KittyGraphicsError::UnsupportedDelete),
        }

        Ok(KittyCommandResult {
            response: response_for(
                response_image_id,
                requested_image_number,
                placement_id,
                "OK",
            ),
            cursor_move: None,
        })
    }

    fn insert_image(&mut self, image_id: u32, image_number: u32, decoded: DecodedImage) -> u64 {
        if image_number == 0
            && let Some(old_key) = self.image_ids.remove(&image_id).filter(|_| image_id != 0)
        {
            self.remove_image(old_key);
        }

        self.next_key = self.next_key.saturating_add(1).max(1);
        self.next_generation = self.next_generation.saturating_add(1).max(1);
        let key = self.next_key;
        let byte_len = decoded.rgba.len();
        self.total_bytes = self.total_bytes.saturating_add(byte_len);
        self.images.insert(
            key,
            KittyImage {
                key,
                image_id,
                width_px: decoded.width_px,
                height_px: decoded.height_px,
                frames: vec![KittyAnimationFrame {
                    generation: self.next_generation,
                    rgba: decoded.rgba.into(),
                    gap_ms: None,
                }],
                animation: KittyAnimation::default(),
            },
        );
        self.insertion_order.push_back(key);
        if image_id != 0 {
            self.image_ids.insert(image_id, key);
        }
        if image_number != 0 {
            self.image_numbers
                .entry(image_number)
                .or_default()
                .push(key);
        }
        self.evict_to_quota();
        key
    }

    fn allocate_image_id(&mut self) -> Result<u32, KittyGraphicsError> {
        for _ in 0..=MAX_IMAGES {
            self.next_image_id = self.next_image_id.wrapping_add(1).max(1);
            if !self.image_ids.contains_key(&self.next_image_id) {
                return Ok(self.next_image_id);
            }
        }
        Err(KittyGraphicsError::StorageFull)
    }

    fn allocate_generation(&mut self) -> u64 {
        self.next_generation = self.next_generation.saturating_add(1).max(1);
        self.next_generation
    }

    fn resolve_image(
        &self,
        command: &KittyCommand,
    ) -> Result<KittyImageReference, KittyGraphicsError> {
        let image_id = command.u32(b'i', 0)?;
        let image_number = command.u32(b'I', 0)?;
        let image_key = if image_id != 0 {
            self.image_ids.get(&image_id).copied()
        } else if image_number != 0 {
            self.newest_image_with_number(image_number)
        } else {
            None
        }
        .ok_or(KittyGraphicsError::ImageNotFound)?;
        let image = self
            .images
            .get(&image_key)
            .ok_or(KittyGraphicsError::ImageNotFound)?;

        Ok(KittyImageReference {
            image_key,
            image_id: image.image_id,
            image_number,
        })
    }

    fn newest_image_with_number(&self, image_number: u32) -> Option<u64> {
        self.image_numbers
            .get(&image_number)?
            .iter()
            .rev()
            .copied()
            .find(|key| self.images.contains_key(key))
    }

    fn delete_image_placements(&mut self, image_key: u64, placement_id: u32) {
        let matches = |placement: &KittyPlacement| {
            placement.image_key == image_key
                && (placement_id == 0 || placement.placement_id == placement_id)
        };
        let roots = self
            .placements
            .values()
            .chain(self.virtual_placements.values())
            .filter(|placement| matches(placement))
            .map(|placement| placement.key)
            .collect::<HashSet<_>>();
        self.remove_placement_trees(roots, false);
    }

    fn image_is_referenced(&self, image_key: u64) -> bool {
        let active = self
            .placements
            .values()
            .chain(self.virtual_placements.values())
            .any(|placement| placement.image_key == image_key);
        active
            || self
                .primary_screen
                .as_ref()
                .is_some_and(|screen| screen.image_is_referenced(image_key))
    }

    fn remove_unreferenced_images(&mut self, keys: HashSet<u64>) {
        for key in keys {
            if !self.image_is_referenced(key) {
                self.remove_image(key);
            }
        }
    }

    fn delete_physical_placements(
        &mut self,
        predicate: impl Fn(&KittyPlacement, (i64, i64)) -> bool,
        delete_data: bool,
    ) {
        let roots = self
            .placements
            .values()
            .filter(|placement| predicate(placement, self.placement_origin(placement)))
            .map(|placement| placement.key)
            .collect::<HashSet<_>>();
        self.remove_placement_trees(roots, delete_data);
    }

    fn placement_origin(&self, placement: &KittyPlacement) -> (i64, i64) {
        self.resolved_positions
            .get(&placement.key)
            .copied()
            .unwrap_or((
                i64::from(placement.x_cell),
                i64::from(placement.y_cell).saturating_add(placement.scroll_offset_y),
            ))
    }

    fn remove_placement_trees(&mut self, roots: HashSet<u64>, delete_root_data: bool) {
        if roots.is_empty() {
            return;
        }
        let mut removed = roots.clone();
        loop {
            let children = self
                .placements
                .values()
                .chain(self.virtual_placements.values())
                .filter_map(|placement| {
                    placement
                        .relative_to
                        .filter(|parent| removed.contains(parent))
                        .map(|_| placement.key)
                })
                .collect::<Vec<_>>();
            let previous_len = removed.len();
            removed.extend(children);
            if removed.len() == previous_len {
                break;
            }
        }

        let root_images = roots
            .iter()
            .filter_map(|key| self.placement_by_key(*key))
            .map(|placement| placement.image_key)
            .collect::<HashSet<_>>();
        let descendant_images = removed
            .difference(&roots)
            .filter_map(|key| self.placement_by_key(*key))
            .map(|placement| placement.image_key)
            .filter(|image_key| !root_images.contains(image_key))
            .collect::<HashSet<_>>();

        self.placements.retain(|key, _| !removed.contains(key));
        self.virtual_placements
            .retain(|key, _| !removed.contains(key));
        self.resolved_positions
            .retain(|key, _| !removed.contains(key));
        self.remove_unreferenced_images(descendant_images);
        if delete_root_data {
            for image_key in root_images {
                self.remove_image(image_key);
            }
        }
    }

    fn insert_placement(
        &mut self,
        command: &KittyCommand,
        cursor: (u32, u32),
        image_key: u64,
        image_id: u32,
        cell_size_px: (u32, u32),
    ) -> Result<KittyCursorMove, KittyGraphicsError> {
        let replacement_key = matching_placement_key(&self.placements, command, image_id)?;
        let (placement, cursor_move) = self.create_placement(
            command,
            cursor,
            image_key,
            image_id,
            replacement_key,
            cell_size_px,
        )?;
        self.placements.insert(placement.key, placement);
        Ok(cursor_move)
    }

    fn insert_virtual_placement(
        &mut self,
        command: &KittyCommand,
        image_key: u64,
        image_id: u32,
        cell_size_px: (u32, u32),
    ) -> Result<(), KittyGraphicsError> {
        if has_relative_placement_controls(command) {
            return Err(KittyGraphicsError::InvalidControl);
        }
        let replacement_key = matching_placement_key(&self.virtual_placements, command, image_id)?;
        let (placement, _) = self.create_placement(
            command,
            (0, 0),
            image_key,
            image_id,
            replacement_key,
            cell_size_px,
        )?;
        self.virtual_placements.insert(placement.key, placement);
        Ok(())
    }

    fn create_placement(
        &mut self,
        command: &KittyCommand,
        cursor: (u32, u32),
        image_key: u64,
        image_id: u32,
        replacement_key: Option<u64>,
        cell_size_px: (u32, u32),
    ) -> Result<(KittyPlacement, KittyCursorMove), KittyGraphicsError> {
        let image = self
            .images
            .get(&image_key)
            .ok_or(KittyGraphicsError::ImageNotFound)?;
        let image_width_px = image.width_px;
        let image_height_px = image.height_px;
        let placement_id = command.u32(b'p', 0)?;
        let requested_columns = command.u32(b'c', 0)?;
        let requested_rows = command.u32(b'r', 0)?;
        self.next_key = self.next_key.saturating_add(1).max(1);
        let key = replacement_key.unwrap_or(self.next_key);
        let relative_to = self.relative_parent(command, key)?;
        let source_x_px = command.u32(b'x', 0)?.min(image_width_px);
        let source_y_px = command.u32(b'y', 0)?.min(image_height_px);
        let max_width = image_width_px.saturating_sub(source_x_px);
        let max_height = image_height_px.saturating_sub(source_y_px);
        let source_width_px = command.u32(b'w', 0)?.min(max_width);
        let source_height_px = command.u32(b'h', 0)?.min(max_height);
        let source_width_px = if source_width_px == 0 {
            max_width
        } else {
            source_width_px
        };
        let source_height_px = if source_height_px == 0 {
            max_height
        } else {
            source_height_px
        };
        let x_offset_px = command.u32(b'X', 0)?;
        let y_offset_px = command.u32(b'Y', 0)?;
        let layout = placement_layout(
            requested_columns,
            requested_rows,
            source_width_px,
            source_height_px,
            x_offset_px,
            y_offset_px,
            cell_size_px,
        )?;

        let placement = KittyPlacement {
            key,
            image_key,
            image_id,
            placement_id,
            x_cell: cursor.0,
            y_cell: cursor.1,
            x_offset_px,
            y_offset_px,
            columns: layout.render_columns,
            rows: layout.render_rows,
            occupied_columns: layout.occupied_columns,
            occupied_rows: layout.occupied_rows,
            source_x_px,
            source_y_px,
            source_width_px,
            source_height_px,
            z_index: command.i32(b'z', 0)?,
            relative_to,
            horizontal_offset: command.i32(b'H', 0)?,
            vertical_offset: command.i32(b'V', 0)?,
            scroll_offset_y: 0,
            clip_top_cell: None,
            clip_bottom_cell: None,
        };

        let cursor_move = if relative_to.is_some() || command.u32(b'C', 0)? == 1 {
            KittyCursorMove {
                columns: 0,
                rows: 0,
            }
        } else {
            KittyCursorMove {
                columns: layout.cursor_columns,
                rows: layout.cursor_rows,
            }
        };
        Ok((placement, cursor_move))
    }

    fn relative_parent(
        &self,
        command: &KittyCommand,
        child_key: u64,
    ) -> Result<Option<u64>, KittyGraphicsError> {
        let parent_image_id = command.u32(b'P', 0)?;
        let parent_placement_id = command.u32(b'Q', 0)?;
        if parent_image_id == 0 && parent_placement_id == 0 {
            if command.control.contains_key(&b'H') || command.control.contains_key(&b'V') {
                return Err(KittyGraphicsError::InvalidControl);
            }
            return Ok(None);
        }
        if parent_image_id == 0 || parent_placement_id == 0 {
            return Err(KittyGraphicsError::InvalidControl);
        }

        let parent_key = self
            .placements
            .values()
            .chain(self.virtual_placements.values())
            .filter(|placement| {
                placement.image_id == parent_image_id
                    && placement.placement_id == parent_placement_id
            })
            .max_by_key(|placement| placement.key)
            .map(|placement| placement.key)
            .ok_or(KittyGraphicsError::ParentNotFound)?;
        self.validate_relative_chain(child_key, parent_key)?;
        Ok(Some(parent_key))
    }

    fn validate_relative_chain(
        &self,
        child_key: u64,
        mut parent_key: u64,
    ) -> Result<(), KittyGraphicsError> {
        let mut visited = HashSet::from([child_key]);
        for _ in 0..MAX_RELATIVE_PLACEMENT_DEPTH {
            if !visited.insert(parent_key) {
                return Err(KittyGraphicsError::RelativeCycle);
            }
            let Some(parent) = self.placement_by_key(parent_key) else {
                return Err(KittyGraphicsError::ParentNotFound);
            };
            let Some(next_parent) = parent.relative_to else {
                return Ok(());
            };
            parent_key = next_parent;
        }
        Err(KittyGraphicsError::RelativeTooDeep)
    }

    fn placement_by_key(&self, key: u64) -> Option<&KittyPlacement> {
        self.placements
            .get(&key)
            .or_else(|| self.virtual_placements.get(&key))
    }

    pub(crate) fn snapshots(
        &mut self,
        placeholder_cells: &[KittyPlaceholderCell],
    ) -> Vec<RenderSurfaceImageSnapshot> {
        let (virtual_snapshots, virtual_positions) = self.virtual_snapshots(placeholder_cells);
        let mut positions = HashMap::new();
        let mut visiting = HashSet::new();
        for key in self
            .placements
            .keys()
            .chain(self.virtual_placements.keys())
            .copied()
            .collect::<Vec<_>>()
        {
            self.resolve_placement_position(
                key,
                &virtual_positions,
                &mut positions,
                &mut visiting,
                0,
            );
        }
        self.resolved_positions.clone_from(&positions);

        let mut snapshots: Vec<_> = self
            .placements
            .values()
            .filter_map(|placement| {
                let image = self.images.get(&placement.image_key)?;
                let frame = image.current_frame();
                let &(x_cell, y_cell) = positions.get(&placement.key)?;
                let (clip_top_cell, clip_bottom_cell) = self.placement_clip_cells(placement.key);
                Some(RenderSurfaceImageSnapshot {
                    id: format!("{}:{}", image.key, placement.key),
                    image_id: placement.image_id,
                    image_generation: frame.generation,
                    x_cell: signed_cell_to_i32(x_cell),
                    y_cell: signed_cell_to_i32(y_cell),
                    x_offset_px: placement.x_offset_px,
                    y_offset_px: placement.y_offset_px,
                    columns: placement.columns,
                    rows: placement.rows,
                    source_x_px: placement.source_x_px,
                    source_y_px: placement.source_y_px,
                    source_width_px: placement.source_width_px,
                    source_height_px: placement.source_height_px,
                    image_width_px: image.width_px,
                    image_height_px: image.height_px,
                    clip_top_cell: clip_top_cell.map(signed_cell_to_i32),
                    clip_bottom_cell: clip_bottom_cell.map(signed_cell_to_i32),
                    z_index: placement.z_index,
                    rgba: Arc::clone(&frame.rgba),
                })
            })
            .collect();
        snapshots.extend(virtual_snapshots);
        snapshots.sort_by_key(|snapshot| snapshot.z_index);
        snapshots
    }

    fn virtual_snapshots(
        &self,
        placeholder_cells: &[KittyPlaceholderCell],
    ) -> (Vec<RenderSurfaceImageSnapshot>, HashMap<u64, (i64, i64)>) {
        let mut positions_by_placement = HashMap::<u64, HashSet<(u32, u32)>>::new();
        for cell in placeholder_cells {
            let Some(image_key) = self.image_ids.get(&cell.image_id) else {
                continue;
            };
            let placement = self
                .virtual_placements
                .values()
                .filter(|placement| {
                    placement.image_key == *image_key
                        && (cell.placement_id == 0 || placement.placement_id == cell.placement_id)
                })
                .max_by_key(|placement| placement.key);
            let Some(placement) = placement else {
                continue;
            };
            positions_by_placement
                .entry(placement.key)
                .or_default()
                .insert((cell.x_cell, cell.y_cell));
        }

        let virtual_positions = positions_by_placement
            .iter()
            .filter_map(|(placement_key, positions)| {
                let min_x = positions.iter().map(|position| position.0).min()?;
                let min_y = positions.iter().map(|position| position.1).min()?;
                Some((*placement_key, (i64::from(min_x), i64::from(min_y))))
            })
            .collect();
        let mut snapshots = Vec::new();
        for (placement_key, mut remaining) in positions_by_placement {
            let Some(placement) = self.virtual_placements.get(&placement_key) else {
                continue;
            };
            let Some(image) = self.images.get(&placement.image_key) else {
                continue;
            };
            let frame = image.current_frame();
            while let Some(start) = remaining.iter().next().copied() {
                let mut stack = vec![start];
                remaining.remove(&start);
                let (mut min_x, mut min_y) = start;
                let (mut max_x, mut max_y) = start;

                while let Some((x, y)) = stack.pop() {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                    for neighbor in [
                        x.checked_sub(1).map(|nx| (nx, y)),
                        x.checked_add(1).map(|nx| (nx, y)),
                        y.checked_sub(1).map(|ny| (x, ny)),
                        y.checked_add(1).map(|ny| (x, ny)),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if remaining.remove(&neighbor) {
                            stack.push(neighbor);
                        }
                    }
                }

                snapshots.push(RenderSurfaceImageSnapshot {
                    id: format!("virtual:{placement_key}:{min_x}:{min_y}"),
                    image_id: placement.image_id,
                    image_generation: frame.generation,
                    x_cell: i32::try_from(min_x).unwrap_or(i32::MAX),
                    y_cell: i32::try_from(min_y).unwrap_or(i32::MAX),
                    x_offset_px: 0,
                    y_offset_px: 0,
                    columns: max_x.saturating_sub(min_x).saturating_add(1),
                    rows: max_y.saturating_sub(min_y).saturating_add(1),
                    source_x_px: placement.source_x_px,
                    source_y_px: placement.source_y_px,
                    source_width_px: placement.source_width_px,
                    source_height_px: placement.source_height_px,
                    image_width_px: image.width_px,
                    image_height_px: image.height_px,
                    clip_top_cell: None,
                    clip_bottom_cell: None,
                    z_index: placement.z_index,
                    rgba: Arc::clone(&frame.rgba),
                });
            }
        }
        (snapshots, virtual_positions)
    }

    fn placement_clip_cells(&self, mut key: u64) -> (Option<i64>, Option<i64>) {
        let mut top = None::<i64>;
        let mut bottom = None::<i64>;
        for _ in 0..=MAX_RELATIVE_PLACEMENT_DEPTH {
            let Some(placement) = self.placement_by_key(key) else {
                break;
            };
            if let Some(placement_top) = placement.clip_top_cell {
                top = Some(top.map_or(placement_top, |current| current.max(placement_top)));
            }
            if let Some(placement_bottom) = placement.clip_bottom_cell {
                bottom =
                    Some(bottom.map_or(placement_bottom, |current| current.min(placement_bottom)));
            }
            let Some(parent) = placement.relative_to else {
                break;
            };
            key = parent;
        }
        (top, bottom)
    }

    fn resolve_placement_position(
        &self,
        key: u64,
        virtual_positions: &HashMap<u64, (i64, i64)>,
        resolved: &mut HashMap<u64, (i64, i64)>,
        visiting: &mut HashSet<u64>,
        depth: usize,
    ) -> Option<(i64, i64)> {
        if let Some(position) = resolved.get(&key).copied() {
            return Some(position);
        }
        if depth > MAX_RELATIVE_PLACEMENT_DEPTH || !visiting.insert(key) {
            return None;
        }
        let placement = self.placement_by_key(key)?;
        let position = if let Some(parent_key) = placement.relative_to {
            let parent = self.resolve_placement_position(
                parent_key,
                virtual_positions,
                resolved,
                visiting,
                depth + 1,
            )?;
            (
                parent
                    .0
                    .saturating_add(i64::from(placement.horizontal_offset)),
                parent
                    .1
                    .saturating_add(i64::from(placement.vertical_offset)),
            )
        } else if self.virtual_placements.contains_key(&key) {
            *virtual_positions.get(&key)?
        } else {
            (
                i64::from(placement.x_cell),
                i64::from(placement.y_cell).saturating_add(placement.scroll_offset_y),
            )
        };
        visiting.remove(&key);
        resolved.insert(key, position);
        Some(position)
    }

    fn evict_to_quota(&mut self) {
        while self.images.len() > MAX_IMAGES || self.total_bytes > MAX_TOTAL_IMAGE_BYTES {
            let Some(key) = self.insertion_order.pop_front() else {
                break;
            };
            self.remove_image(key);
        }
    }

    fn remove_image(&mut self, key: u64) {
        let inactive_descendants = self
            .primary_screen
            .as_mut()
            .map(|screen| screen.remove_image_placements(key))
            .unwrap_or_default();
        let roots = self
            .placements
            .values()
            .chain(self.virtual_placements.values())
            .filter(|placement| placement.image_key == key)
            .map(|placement| placement.key)
            .collect();
        self.remove_placement_trees(roots, false);
        if let Some(image) = self.images.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(image.byte_len());
        }
        self.image_ids.retain(|_, value| *value != key);
        self.image_numbers.retain(|_, keys| {
            keys.retain(|image_key| *image_key != key);
            !keys.is_empty()
        });
        self.remove_unreferenced_images(inactive_descendants);
    }
}

fn schedule_next_frame(image: &mut KittyImage, now: Instant) {
    if image.animation.state == KittyAnimationState::Stopped {
        image.animation.next_frame_at = None;
        return;
    }
    image.animation.next_frame_at = match image.current_frame().gap_ms {
        Some(gap) if gap > 0 => Some(now + Duration::from_millis(gap as u64)),
        Some(gap) if gap < 0 => Some(now),
        _ => None,
    };
}

fn advance_image_animation(image: &mut KittyImage, now: Instant) -> bool {
    let mut changed = false;
    let max_transitions = image.frames.len().saturating_add(1);
    for _ in 0..max_transitions {
        let Some(deadline) = image.animation.next_frame_at else {
            break;
        };
        if deadline > now {
            break;
        }

        if image.animation.current_frame + 1 < image.frames.len() {
            image.animation.current_frame += 1;
            changed = true;
        } else {
            match image.animation.state {
                KittyAnimationState::Stopped => {
                    image.animation.next_frame_at = None;
                    break;
                }
                KittyAnimationState::Loading => {
                    image.animation.next_frame_at = None;
                    break;
                }
                KittyAnimationState::Running => {
                    if image
                        .animation
                        .loop_limit
                        .is_some_and(|limit| image.animation.completed_loops >= limit)
                    {
                        image.animation.state = KittyAnimationState::Stopped;
                        image.animation.next_frame_at = None;
                        break;
                    }
                    image.animation.completed_loops =
                        image.animation.completed_loops.saturating_add(1);
                    changed |= image.animation.current_frame != 0;
                    image.animation.current_frame = 0;
                }
            }
        }
        schedule_next_frame(image, now);
        if image.animation.next_frame_at != Some(now) {
            break;
        }
    }
    changed
}

fn nonzero_frame_gap(command: &KittyCommand) -> Result<Option<i32>, KittyGraphicsError> {
    command.i32(b'z', 0).map(|gap| (gap != 0).then_some(gap))
}

fn required_frame_index(number: u32, frame_count: usize) -> Result<usize, KittyGraphicsError> {
    let index =
        usize::try_from(number.saturating_sub(1)).map_err(|_| KittyGraphicsError::ImageNotFound)?;
    if number == 0 || index >= frame_count {
        return Err(KittyGraphicsError::ImageNotFound);
    }
    Ok(index)
}

fn optional_frame_index(
    number: u32,
    frame_count: usize,
) -> Result<Option<usize>, KittyGraphicsError> {
    (number != 0)
        .then(|| required_frame_index(number, frame_count))
        .transpose()
}

fn frame_byte_len(width: u32, height: u32) -> Result<usize, KittyGraphicsError> {
    usize::try_from(u64::from(width) * u64::from(height) * 4)
        .map_err(|_| KittyGraphicsError::ImageTooLarge)
}

fn solid_frame(width: u32, height: u32, rgba: u32) -> Result<Vec<u8>, KittyGraphicsError> {
    let pixel = rgba.to_be_bytes();
    let mut frame = Vec::with_capacity(frame_byte_len(width, height)?);
    for _ in 0..u64::from(width) * u64::from(height) {
        frame.extend_from_slice(&pixel);
    }
    Ok(frame)
}

fn validate_rect(
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    image_width: u32,
    image_height: u32,
) -> Result<(), KittyGraphicsError> {
    if width == 0
        || height == 0
        || x.checked_add(width).is_none_or(|right| right > image_width)
        || y.checked_add(height)
            .is_none_or(|bottom| bottom > image_height)
    {
        return Err(KittyGraphicsError::InvalidControl);
    }
    Ok(())
}

fn composite_rgba_rect(
    destination: &mut [u8],
    destination_width: u32,
    destination_origin: (u32, u32),
    source: &[u8],
    source_size: (u32, u32),
    replace: bool,
) -> Result<(), KittyGraphicsError> {
    let (destination_x, destination_y) = destination_origin;
    let (source_width, source_height) = source_size;
    let expected_destination = frame_byte_len(
        destination_width,
        u32::try_from(destination.len() / 4)
            .ok()
            .and_then(|pixels| pixels.checked_div(destination_width))
            .ok_or(KittyGraphicsError::InvalidPayload)?,
    )?;
    if destination.len() != expected_destination
        || source.len() != frame_byte_len(source_width, source_height)?
    {
        return Err(KittyGraphicsError::InvalidPayload);
    }
    let destination_width =
        usize::try_from(destination_width).map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let destination_x =
        usize::try_from(destination_x).map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let destination_y =
        usize::try_from(destination_y).map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let source_width =
        usize::try_from(source_width).map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let source_height =
        usize::try_from(source_height).map_err(|_| KittyGraphicsError::ImageTooLarge)?;

    for row in 0..source_height {
        for column in 0..source_width {
            let source_offset = (row * source_width + column) * 4;
            let destination_offset =
                ((destination_y + row) * destination_width + destination_x + column) * 4;
            if replace {
                destination[destination_offset..destination_offset + 4]
                    .copy_from_slice(&source[source_offset..source_offset + 4]);
            } else {
                alpha_blend_pixel(
                    &mut destination[destination_offset..destination_offset + 4],
                    &source[source_offset..source_offset + 4],
                );
            }
        }
    }
    Ok(())
}

fn alpha_blend_pixel(destination: &mut [u8], source: &[u8]) {
    let source_alpha = u32::from(source[3]);
    let destination_alpha = u32::from(destination[3]);
    let inverse_source_alpha = 255 - source_alpha;
    let output_alpha = source_alpha + (destination_alpha * inverse_source_alpha + 127) / 255;
    if output_alpha == 0 {
        destination.copy_from_slice(&[0, 0, 0, 0]);
        return;
    }
    for channel in 0..3 {
        let source_premultiplied = u32::from(source[channel]) * source_alpha;
        let destination_premultiplied = u32::from(destination[channel]) * destination_alpha;
        let output_premultiplied =
            source_premultiplied + (destination_premultiplied * inverse_source_alpha + 127) / 255;
        destination[channel] =
            u8::try_from((output_premultiplied + output_alpha / 2) / output_alpha)
                .unwrap_or(u8::MAX);
    }
    destination[3] = u8::try_from(output_alpha).unwrap_or(u8::MAX);
}

fn extract_rgba_rect(
    source: &[u8],
    source_width: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, KittyGraphicsError> {
    let source_width =
        usize::try_from(source_width).map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let x = usize::try_from(x).map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let y = usize::try_from(y).map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let width = usize::try_from(width).map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let height = usize::try_from(height).map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let mut extracted = Vec::with_capacity(width.saturating_mul(height).saturating_mul(4));
    for row in 0..height {
        let start = ((y + row) * source_width + x) * 4;
        let end = start + width * 4;
        let pixels = source
            .get(start..end)
            .ok_or(KittyGraphicsError::InvalidControl)?;
        extracted.extend_from_slice(pixels);
    }
    Ok(extracted)
}

fn rects_overlap(first: (u32, u32, u32, u32), second: (u32, u32, u32, u32)) -> bool {
    let first_right = first.0.saturating_add(first.2);
    let first_bottom = first.1.saturating_add(first.3);
    let second_right = second.0.saturating_add(second.2);
    let second_bottom = second.1.saturating_add(second.3);
    first.0 < second_right
        && second.0 < first_right
        && first.1 < second_bottom
        && second.1 < first_bottom
}

fn matching_placement_key(
    placements: &HashMap<u64, KittyPlacement>,
    command: &KittyCommand,
    image_id: u32,
) -> Result<Option<u64>, KittyGraphicsError> {
    let placement_id = command.u32(b'p', 0)?;
    if image_id == 0 || placement_id == 0 {
        return Ok(None);
    }
    Ok(placements.iter().find_map(|(key, placement)| {
        (placement.image_id == image_id && placement.placement_id == placement_id).then_some(*key)
    }))
}

fn has_relative_placement_controls(command: &KittyCommand) -> bool {
    b"PQHV".iter().any(|key| command.control.contains_key(key))
}

fn signed_cell_to_i32(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value.is_negative() {
        i32::MIN
    } else {
        i32::MAX
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyPlacementLayout {
    render_columns: u32,
    render_rows: u32,
    occupied_columns: u32,
    occupied_rows: u32,
    cursor_columns: u32,
    cursor_rows: u32,
}

fn placement_layout(
    requested_columns: u32,
    requested_rows: u32,
    source_width_px: u32,
    source_height_px: u32,
    x_offset_px: u32,
    y_offset_px: u32,
    cell_size_px: (u32, u32),
) -> Result<KittyPlacementLayout, KittyGraphicsError> {
    let (cell_width_px, cell_height_px) = (cell_size_px.0.max(1), cell_size_px.1.max(1));
    if source_width_px == 0
        || source_height_px == 0
        || x_offset_px >= cell_width_px
        || y_offset_px >= cell_height_px
    {
        return Err(KittyGraphicsError::InvalidControl);
    }

    let mut render_columns = requested_columns;
    let mut render_rows = requested_rows;
    if requested_columns != 0 && requested_rows == 0 {
        let scaled_height = u64::from(source_height_px)
            .saturating_mul(u64::from(requested_columns))
            .saturating_mul(u64::from(cell_width_px));
        let denominator = u64::from(source_width_px).saturating_mul(u64::from(cell_height_px));
        render_rows = u32::try_from(ceil_div(scaled_height, denominator).max(1))
            .map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    } else if requested_columns == 0 && requested_rows != 0 {
        let scaled_width = u64::from(source_width_px)
            .saturating_mul(u64::from(requested_rows))
            .saturating_mul(u64::from(cell_height_px));
        let denominator = u64::from(source_height_px).saturating_mul(u64::from(cell_width_px));
        render_columns = u32::try_from(ceil_div(scaled_width, denominator).max(1))
            .map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    }

    let display_width_px = if render_columns == 0 {
        u64::from(source_width_px)
    } else {
        u64::from(render_columns).saturating_mul(u64::from(cell_width_px))
    };
    let display_height_px = if render_rows == 0 {
        u64::from(source_height_px)
    } else {
        u64::from(render_rows).saturating_mul(u64::from(cell_height_px))
    };
    let occupied_columns = u32::try_from(ceil_div(
        u64::from(x_offset_px).saturating_add(display_width_px),
        u64::from(cell_width_px),
    ))
    .map_err(|_| KittyGraphicsError::ImageTooLarge)?;
    let occupied_rows = u32::try_from(ceil_div(
        u64::from(y_offset_px).saturating_add(display_height_px),
        u64::from(cell_height_px),
    ))
    .map_err(|_| KittyGraphicsError::ImageTooLarge)?;

    Ok(KittyPlacementLayout {
        render_columns,
        render_rows,
        occupied_columns,
        occupied_rows,
        cursor_columns: if render_columns == 0 {
            u32::try_from(ceil_div(
                u64::from(source_width_px),
                u64::from(cell_width_px),
            ))
            .map_err(|_| KittyGraphicsError::ImageTooLarge)?
        } else {
            render_columns
        },
        cursor_rows: if render_rows == 0 {
            u32::try_from(ceil_div(
                u64::from(source_height_px),
                u64::from(cell_height_px),
            ))
            .map_err(|_| KittyGraphicsError::ImageTooLarge)?
        } else {
            render_rows
        },
    })
}

fn ceil_div(numerator: u64, denominator: u64) -> u64 {
    numerator / denominator + u64::from(!numerator.is_multiple_of(denominator))
}

fn command_cell_point(command: &KittyCommand) -> Result<(u32, u32), KittyGraphicsError> {
    Ok((
        command_cell_coordinate(command, b'x')?,
        command_cell_coordinate(command, b'y')?,
    ))
}

fn command_cell_coordinate(command: &KittyCommand, key: u8) -> Result<u32, KittyGraphicsError> {
    command
        .u32(key, 0)?
        .checked_sub(1)
        .ok_or(KittyGraphicsError::InvalidControl)
}

fn placement_intersects_cell(
    placement: &KittyPlacement,
    origin: (i64, i64),
    x_cell: u32,
    y_cell: u32,
) -> bool {
    placement_intersects_column(placement, origin.0, x_cell)
        && placement_intersects_row(placement, origin.1, y_cell)
}

fn placement_intersects_column(placement: &KittyPlacement, origin_x: i64, x_cell: u32) -> bool {
    let x_cell = i64::from(x_cell);
    x_cell >= origin_x && x_cell < origin_x.saturating_add(i64::from(placement.occupied_columns))
}

fn placement_intersects_row(placement: &KittyPlacement, origin_y: i64, y_cell: u32) -> bool {
    let y_cell = i64::from(y_cell);
    y_cell >= origin_y && y_cell < origin_y.saturating_add(i64::from(placement.occupied_rows))
}

fn transmission_bytes(command: &KittyCommand) -> Result<Vec<u8>, KittyGraphicsError> {
    let decoded = STANDARD
        .decode(&command.payload)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(&command.payload))
        .map_err(|_| KittyGraphicsError::InvalidPayload)?;

    match command.char(b't', 'd') {
        'd' => Ok(decoded),
        'f' => read_regular_file(command, &decoded),
        't' => read_temporary_file(command, &decoded),
        's' => read_shared_memory(command, &decoded),
        _ => Err(KittyGraphicsError::UnsupportedMedium),
    }
}

fn read_regular_file(
    command: &KittyCommand,
    encoded_path: &[u8],
) -> Result<Vec<u8>, KittyGraphicsError> {
    let path = decoded_path(encoded_path)?;
    let mut file = File::open(path).map_err(|_| KittyGraphicsError::FileReadFailed)?;
    let metadata = file
        .metadata()
        .map_err(|_| KittyGraphicsError::FileReadFailed)?;
    if !metadata.file_type().is_file() {
        return Err(KittyGraphicsError::UnsupportedFileType);
    }

    read_file_range(command, &mut file, metadata.len())
}

fn read_temporary_file(
    command: &KittyCommand,
    encoded_path: &[u8],
) -> Result<Vec<u8>, KittyGraphicsError> {
    let path = decoded_path(encoded_path)?;
    let result = read_regular_file(command, encoded_path);
    if is_safe_temporary_protocol_file(path) {
        let _ = fs::remove_file(path);
    }
    result
}

fn decoded_path(encoded_path: &[u8]) -> Result<&Path, KittyGraphicsError> {
    let path = std::str::from_utf8(encoded_path).map_err(|_| KittyGraphicsError::InvalidPayload)?;
    Ok(Path::new(path))
}

fn is_safe_temporary_protocol_file(path: &Path) -> bool {
    if !path.is_absolute() || !path.to_string_lossy().contains("tty-graphics-protocol") {
        return false;
    }

    let Ok(canonical_path) = path.canonicalize() else {
        return false;
    };
    [
        Path::new("/tmp"),
        Path::new("/dev/shm"),
        &std::env::temp_dir(),
    ]
    .into_iter()
    .filter_map(|directory| directory.canonicalize().ok())
    .any(|directory| canonical_path.starts_with(directory))
}

#[cfg(all(unix, not(target_os = "android")))]
fn read_shared_memory(
    command: &KittyCommand,
    encoded_name: &[u8],
) -> Result<Vec<u8>, KittyGraphicsError> {
    use nix::{
        fcntl::OFlag,
        sys::{mman, stat::Mode},
    };

    let name = std::str::from_utf8(encoded_name).map_err(|_| KittyGraphicsError::InvalidPayload)?;
    let descriptor = mman::shm_open(name, OFlag::O_RDONLY | OFlag::O_CLOEXEC, Mode::empty())
        .map_err(|_| KittyGraphicsError::FileReadFailed)?;
    let mut file = File::from(descriptor);
    let result = file
        .metadata()
        .map_err(|_| KittyGraphicsError::FileReadFailed)
        .and_then(|metadata| read_file_range(command, &mut file, metadata.len()));
    let unlink_result = mman::shm_unlink(name).map_err(|_| KittyGraphicsError::FileReadFailed);

    match (result, unlink_result) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(bytes), Ok(())) => Ok(bytes),
    }
}

#[cfg(not(all(unix, not(target_os = "android"))))]
fn read_shared_memory(
    _command: &KittyCommand,
    _encoded_name: &[u8],
) -> Result<Vec<u8>, KittyGraphicsError> {
    Err(KittyGraphicsError::UnsupportedMedium)
}

fn read_file_range(
    command: &KittyCommand,
    file: &mut File,
    file_len: u64,
) -> Result<Vec<u8>, KittyGraphicsError> {
    let offset = u64::from(command.u32(b'O', 0)?);
    let available = file_len
        .checked_sub(offset)
        .ok_or(KittyGraphicsError::InvalidPayload)?;
    let requested = match command.control.get(&b'S') {
        Some(_) => u64::from(command.u32(b'S', 0)?),
        None => available,
    };
    let requested = if requested > available {
        kitten_query_raw_data_size(command)?
            .filter(|expected| *expected <= available)
            .ok_or(KittyGraphicsError::InvalidPayload)?
    } else {
        requested
    };
    if requested > MAX_DECODED_IMAGE_BYTES as u64 {
        return Err(KittyGraphicsError::ImageTooLarge);
    }

    file.seek(SeekFrom::Start(offset))
        .map_err(|_| KittyGraphicsError::FileReadFailed)?;
    let mut bytes = Vec::with_capacity(requested as usize);
    file.take(requested)
        .read_to_end(&mut bytes)
        .map_err(|_| KittyGraphicsError::FileReadFailed)?;
    if bytes.len() != requested as usize {
        return Err(KittyGraphicsError::InvalidPayload);
    }
    Ok(bytes)
}

fn kitten_query_raw_data_size(command: &KittyCommand) -> Result<Option<u64>, KittyGraphicsError> {
    if command.char(b'a', 't') != 'q' || command.control.contains_key(&b'o') {
        return Ok(None);
    }

    let channels = match command.u32(b'f', 32)? {
        24 => 3,
        32 => 4,
        _ => return Ok(None),
    };
    let size = u64::from(command.u32(b's', 0)?)
        .checked_mul(u64::from(command.u32(b'v', 0)?))
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or(KittyGraphicsError::ImageTooLarge)?;
    Ok(Some(size))
}

fn append_encoded_payload(target: &mut Vec<u8>, payload: &[u8]) -> Result<(), KittyGraphicsError> {
    if target.len().saturating_add(payload.len()) > MAX_ENCODED_IMAGE_BYTES {
        return Err(KittyGraphicsError::ImageTooLarge);
    }
    target.extend_from_slice(payload);
    Ok(())
}

fn decompress_zlib(bytes: &[u8]) -> Result<Vec<u8>, KittyGraphicsError> {
    let mut decoded = Vec::new();
    ZlibDecoder::new(bytes)
        .take(MAX_DECODED_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut decoded)
        .map_err(|_| KittyGraphicsError::InvalidPayload)?;
    if decoded.len() > MAX_DECODED_IMAGE_BYTES {
        return Err(KittyGraphicsError::ImageTooLarge);
    }
    Ok(decoded)
}

struct DecodedImage {
    width_px: u32,
    height_px: u32,
    rgba: Vec<u8>,
}

fn decode_image(command: &KittyCommand, bytes: &[u8]) -> Result<DecodedImage, KittyGraphicsError> {
    match command.u32(b'f', 32)? {
        24 => decode_raw(command, bytes, 3),
        32 => decode_raw(command, bytes, 4),
        100 => decode_png(bytes),
        _ => Err(KittyGraphicsError::UnsupportedFormat),
    }
}

fn decode_raw(
    command: &KittyCommand,
    bytes: &[u8],
    channels: usize,
) -> Result<DecodedImage, KittyGraphicsError> {
    let width_px = command.u32(b's', 0)?;
    let height_px = command.u32(b'v', 0)?;
    validate_dimensions(width_px, height_px)?;
    let pixels = usize::try_from(width_px)
        .ok()
        .and_then(|width| {
            usize::try_from(height_px)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(KittyGraphicsError::ImageTooLarge)?;
    let expected = pixels
        .checked_mul(channels)
        .ok_or(KittyGraphicsError::ImageTooLarge)?;
    if bytes.len() != expected || expected > MAX_DECODED_IMAGE_BYTES {
        return Err(KittyGraphicsError::InvalidPayload);
    }

    let rgba = if channels == 4 {
        bytes.to_vec()
    } else {
        let mut rgba = Vec::with_capacity(pixels * 4);
        for rgb in bytes.chunks_exact(3) {
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        rgba
    };

    Ok(DecodedImage {
        width_px,
        height_px,
        rgba,
    })
}

fn decode_png(bytes: &[u8]) -> Result<DecodedImage, KittyGraphicsError> {
    if bytes.len() > MAX_DECODED_IMAGE_BYTES {
        return Err(KittyGraphicsError::ImageTooLarge);
    }
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|_| KittyGraphicsError::InvalidPayload)?;
    let width_px = reader.info().width;
    let height_px = reader.info().height;
    validate_dimensions(width_px, height_px)?;
    let output_size = reader
        .output_buffer_size()
        .ok_or(KittyGraphicsError::ImageTooLarge)?;
    if output_size > MAX_DECODED_IMAGE_BYTES {
        return Err(KittyGraphicsError::ImageTooLarge);
    }
    let mut pixels = vec![0; output_size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|_| KittyGraphicsError::InvalidPayload)?;
    pixels.truncate(info.buffer_size());
    if info.bit_depth != png::BitDepth::Eight {
        return Err(KittyGraphicsError::InvalidPayload);
    }
    let pixel_count = usize::try_from(width_px)
        .ok()
        .and_then(|width| {
            usize::try_from(height_px)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .ok_or(KittyGraphicsError::ImageTooLarge)?;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(&pixels),
        png::ColorType::Rgb => {
            for rgb in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for gray in pixels {
                rgba.extend_from_slice(&[gray, gray, gray, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for gray_alpha in pixels.chunks_exact(2) {
                rgba.extend_from_slice(&[
                    gray_alpha[0],
                    gray_alpha[0],
                    gray_alpha[0],
                    gray_alpha[1],
                ]);
            }
        }
        png::ColorType::Indexed => return Err(KittyGraphicsError::InvalidPayload),
    }
    if rgba.len() != pixel_count * 4 {
        return Err(KittyGraphicsError::InvalidPayload);
    }
    Ok(DecodedImage {
        width_px,
        height_px,
        rgba,
    })
}

fn validate_dimensions(width_px: u32, height_px: u32) -> Result<(), KittyGraphicsError> {
    if width_px == 0
        || height_px == 0
        || width_px > MAX_IMAGE_DIMENSION
        || height_px > MAX_IMAGE_DIMENSION
        || u64::from(width_px) * u64::from(height_px) * 4 > MAX_DECODED_IMAGE_BYTES as u64
    {
        return Err(KittyGraphicsError::ImageTooLarge);
    }
    Ok(())
}

fn validate_image_reference(command: &KittyCommand) -> Result<(), KittyGraphicsError> {
    let has_image_id = command.control.contains_key(&b'i');
    let has_image_number = command.control.contains_key(&b'I');
    if has_image_id && has_image_number {
        return Err(KittyGraphicsError::InvalidControl);
    }
    if has_image_number && command.u32(b'I', 0)? == 0 {
        return Err(KittyGraphicsError::InvalidControl);
    }
    Ok(())
}

fn response_for(
    image_id: u32,
    image_number: u32,
    placement_id: u32,
    message: &str,
) -> Option<Vec<u8>> {
    if image_id == 0 && image_number == 0 {
        return None;
    }
    let mut controls = Vec::with_capacity(3);
    if image_id != 0 {
        controls.push(format!("i={image_id}"));
    }
    if image_number != 0 {
        controls.push(format!("I={image_number}"));
    }
    if placement_id != 0 {
        controls.push(format!("p={placement_id}"));
    }
    Some(format!("\x1b_G{};{message}\x1b\\", controls.join(",")).into_bytes())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KittyGraphicsError {
    InvalidControl,
    InvalidChunk,
    InvalidPayload,
    ImageTooLarge,
    ImageNotFound,
    ParentNotFound,
    RelativeCycle,
    RelativeTooDeep,
    UnsupportedAction,
    UnsupportedCompression,
    UnsupportedDelete,
    UnsupportedFormat,
    UnsupportedMedium,
    UnsupportedFileType,
    FileReadFailed,
    StorageFull,
}

impl KittyGraphicsError {
    fn message(self) -> &'static str {
        match self {
            Self::InvalidControl => "EINVAL:invalid control data",
            Self::InvalidChunk => "EINVAL:invalid chunk sequence",
            Self::InvalidPayload => "EINVAL:invalid image payload",
            Self::ImageTooLarge => "E2BIG:image exceeds terminal limits",
            Self::ImageNotFound => "ENOENT:image not found",
            Self::ParentNotFound => "ENOPARENT:parent placement not found",
            Self::RelativeCycle => "ECYCLE:relative placement cycle",
            Self::RelativeTooDeep => "ETOODEEP:relative placement chain is too deep",
            Self::UnsupportedAction => "ENOTSUP:action is not supported",
            Self::UnsupportedCompression => "ENOTSUP:compression is not supported",
            Self::UnsupportedDelete => "ENOTSUP:delete selector is not supported",
            Self::UnsupportedFormat => "ENOTSUP:image format is not supported",
            Self::UnsupportedMedium => "ENOTSUP:transmission medium is not supported",
            Self::UnsupportedFileType => "ENOTSUP:file is not a regular file",
            Self::FileReadFailed => "EIO:failed to read image file",
            Self::StorageFull => "ENOSPC:no image id is available",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Write as _,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn command(control: &str, payload: &[u8]) -> KittyCommand {
        let mut bytes = control.as_bytes().to_vec();
        bytes.push(b';');
        bytes.extend_from_slice(payload);
        KittyCommand::parse(&bytes).unwrap()
    }

    fn temp_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "germinal-kitty-graphics-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn decoder_preserves_text_and_extracts_split_kitty_apc() {
        let mut decoder = KittyGraphicsStreamDecoder::default();

        assert_eq!(
            decoder.feed(b"before\x1b_"),
            vec![KittyStreamEvent::Bytes(b"before".to_vec())]
        );
        assert!(decoder.feed(b"Gf=32,s=1,").is_empty());
        assert_eq!(
            decoder.feed(b"v=1;AAAAAA==\x1b\\after"),
            vec![
                KittyStreamEvent::Command(command("f=32,s=1,v=1", b"AAAAAA==")),
                KittyStreamEvent::Bytes(b"after".to_vec()),
            ]
        );
    }

    #[test]
    fn decoder_fast_path_preserves_plain_bytes() {
        let mut decoder = KittyGraphicsStreamDecoder::default();

        assert_eq!(
            decoder.feed(b"plain text\r\n"),
            vec![KittyStreamEvent::Bytes(b"plain text\r\n".to_vec())]
        );
        assert!(decoder.can_passthrough("终端性能".as_bytes()));
        assert!(decoder.can_passthrough(b"\x1b[31mred\x1b[0m"));
        assert!(decoder.feed(b"\x1b_").is_empty());
        assert_eq!(
            decoder.feed(b"Xnot-kitty"),
            vec![KittyStreamEvent::Bytes(b"\x1b_Xnot-kitty".to_vec())]
        );
    }

    #[test]
    fn decoder_preserves_non_kitty_apc_prefix() {
        let mut decoder = KittyGraphicsStreamDecoder::default();
        assert_eq!(
            decoder.feed(b"a\x1b_Xpayload\x1b\\b"),
            vec![KittyStreamEvent::Bytes(b"a\x1b_Xpayload\x1b\\b".to_vec())]
        );
    }

    #[test]
    fn decoder_accepts_modern_kitten_128_kib_apc_payloads() {
        let payload = vec![b'A'; 128 * 1024];
        let mut bytes = b"\x1b_Ga=T,q=2,m=1;".to_vec();
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(b"\x1b\\");
        let mut decoder = KittyGraphicsStreamDecoder::default();

        assert_eq!(
            decoder.feed(&bytes),
            vec![KittyStreamEvent::Command(command("a=T,q=2,m=1", &payload))]
        );
    }

    #[test]
    fn transmits_and_places_raw_rgba() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([255, 0, 0, 128]);
        let result = graphics.handle(
            command("a=T,f=32,s=1,v=1,i=7,p=2,c=3,r=4", payload.as_bytes()),
            (5, 6),
        );

        assert_eq!(result.response, Some(b"\x1b_Gi=7,p=2;OK\x1b\\".to_vec()));
        assert_eq!(
            result.cursor_move,
            Some(KittyCursorMove {
                columns: 3,
                rows: 4
            })
        );
        let snapshots = graphics.snapshots(&[]);
        assert_eq!(snapshots.len(), 1);
        assert_eq!((snapshots[0].x_cell, snapshots[0].y_cell), (5, 6));
        assert_eq!(&*snapshots[0].rgba, &[255, 0, 0, 128]);
    }

    #[test]
    fn page_margin_scroll_moves_only_fully_contained_images_and_clips_exits() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([255; 12]);
        graphics.handle(
            command("a=T,f=32,s=1,v=3,i=7,c=1,r=3,C=1", payload.as_bytes()),
            (0, 2),
        );
        graphics.handle(
            command("a=T,f=32,s=1,v=3,i=8,c=1,r=3,C=1", payload.as_bytes()),
            (1, 1),
        );

        graphics.scroll_up(2, 6, 1);

        let snapshots = graphics.snapshots(&[]);
        let moved = snapshots
            .iter()
            .find(|snapshot| snapshot.x_cell == 0)
            .unwrap();
        let crossing_margin = snapshots
            .iter()
            .find(|snapshot| snapshot.x_cell == 1)
            .unwrap();
        assert_eq!(moved.y_cell, 1);
        assert_eq!(
            (moved.clip_top_cell, moved.clip_bottom_cell),
            (Some(2), Some(4))
        );
        assert_eq!(crossing_margin.y_cell, 1);
        assert_eq!(crossing_margin.clip_top_cell, None);
        assert_eq!(crossing_margin.clip_bottom_cell, None);

        graphics.scroll_down(2, 6, 1);
        let snapshots = graphics.snapshots(&[]);
        let moved = snapshots
            .iter()
            .find(|snapshot| snapshot.x_cell == 0)
            .unwrap();
        assert_eq!(moved.y_cell, 2);
        assert_eq!(
            (moved.clip_top_cell, moved.clip_bottom_cell),
            (Some(3), Some(5))
        );
    }

    #[test]
    fn placement_layout_resolves_aspect_ratio_and_native_pixel_footprint() {
        let mut graphics = KittyGraphicsState::default();
        let wide = STANDARD.encode(vec![255; 40 * 20 * 4]);
        let scaled = graphics.handle_with_cell_size(
            command("a=T,f=32,s=40,v=20,i=1,c=2", wide.as_bytes()),
            (0, 0),
            (10, 10),
        );

        assert_eq!(
            scaled.cursor_move,
            Some(KittyCursorMove {
                columns: 2,
                rows: 1,
            })
        );
        assert_eq!(
            (
                graphics.snapshots(&[])[0].columns,
                graphics.snapshots(&[])[0].rows,
            ),
            (2, 1)
        );

        let native = STANDARD.encode(vec![128; 15 * 21 * 4]);
        let native = graphics.handle_with_cell_size(
            command("a=T,f=32,s=15,v=21,i=2", native.as_bytes()),
            (5, 5),
            (10, 10),
        );
        assert_eq!(
            native.cursor_move,
            Some(KittyCursorMove {
                columns: 2,
                rows: 3,
            })
        );
        let native_snapshot = graphics
            .snapshots(&[])
            .into_iter()
            .find(|snapshot| snapshot.x_cell == 5)
            .unwrap();
        assert_eq!((native_snapshot.columns, native_snapshot.rows), (0, 0));
    }

    #[test]
    fn delete_at_cursor_and_explicit_cell_use_one_based_protocol_coordinates() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([255, 0, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=7,p=1,c=2,r=2", payload.as_bytes()),
            (2, 2),
        );
        graphics.handle(command("a=p,i=7,p=2,c=2,r=2", b""), (8, 8));

        graphics.handle(command("a=d,d=c", b""), (3, 3));
        let remaining = graphics.snapshots(&[]);
        assert_eq!(remaining.len(), 1);
        assert_eq!((remaining[0].x_cell, remaining[0].y_cell), (8, 8));

        graphics.handle(command("a=d,d=p,x=9,y=9", b""), (0, 0));
        assert!(graphics.snapshots(&[]).is_empty());
        assert!(
            graphics
                .handle(command("a=p,i=7,c=1,r=1", b""), (0, 0))
                .response
                .is_some(),
            "lowercase deletion must retain image data"
        );
    }

    #[test]
    fn delete_at_cell_with_z_index_keeps_other_overlapping_placements() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([255, 0, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=7,p=1,c=2,r=2,z=-1", payload.as_bytes()),
            (1, 1),
        );
        graphics.handle(command("a=p,i=7,p=2,c=2,r=2,z=2", b""), (1, 1));

        graphics.handle(command("a=d,d=q,x=2,y=2,z=-1", b""), (0, 0));

        let snapshots = graphics.snapshots(&[]);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].z_index, 2);
    }

    #[test]
    fn delete_by_column_accounts_for_pixel_offset_beyond_the_nominal_cell_count() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode(vec![255; 10 * 10 * 4]);
        graphics.handle_with_cell_size(
            command("a=T,f=32,s=10,v=10,i=7,p=1,X=3,C=1", payload.as_bytes()),
            (0, 0),
            (10, 10),
        );

        graphics.handle(command("a=d,d=x,x=2", b""), (0, 0));

        assert!(graphics.snapshots(&[]).is_empty());
    }

    #[test]
    fn uppercase_row_deletion_releases_unreferenced_image_data() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([255, 0, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=7,c=2,r=2", payload.as_bytes()),
            (4, 5),
        );

        graphics.handle(command("a=d,d=Y,y=6", b""), (0, 0));

        let placement = graphics.handle(command("a=p,i=7,c=1,r=1", b""), (0, 0));
        assert_eq!(
            placement.response,
            Some(b"\x1b_Gi=7;ENOENT:image not found\x1b\\".to_vec())
        );
    }

    #[test]
    fn relative_placement_follows_parent_replacement_and_does_not_move_cursor() {
        let mut graphics = KittyGraphicsState::default();
        let parent = STANDARD.encode([255, 0, 0, 255]);
        let child = STANDARD.encode([0, 0, 255, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=1,p=1,c=2,r=2,C=1", parent.as_bytes()),
            (4, 5),
        );

        let result = graphics.handle(
            command(
                "a=T,f=32,s=1,v=1,i=2,p=1,c=1,r=1,P=1,Q=1,H=-2,V=3",
                child.as_bytes(),
            ),
            (40, 50),
        );
        assert_eq!(
            result.cursor_move,
            Some(KittyCursorMove {
                columns: 0,
                rows: 0,
            })
        );
        let child_snapshot = graphics
            .snapshots(&[])
            .into_iter()
            .find(|snapshot| *snapshot.rgba == [0, 0, 255, 255])
            .unwrap();
        assert_eq!((child_snapshot.x_cell, child_snapshot.y_cell), (2, 8));

        graphics.handle(command("a=p,i=1,p=1,c=2,r=2,C=1", b""), (9, 10));
        let child_snapshot = graphics
            .snapshots(&[])
            .into_iter()
            .find(|snapshot| *snapshot.rgba == [0, 0, 255, 255])
            .unwrap();
        assert_eq!((child_snapshot.x_cell, child_snapshot.y_cell), (7, 13));
    }

    #[test]
    fn relative_placement_uses_minimum_virtual_parent_placeholder_position() {
        let mut graphics = KittyGraphicsState::default();
        let parent = STANDARD.encode([255, 0, 0, 255]);
        let child = STANDARD.encode([0, 255, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=1,p=1,c=1,r=1,U=1", parent.as_bytes()),
            (0, 0),
        );
        graphics.handle(
            command(
                "a=T,f=32,s=1,v=1,i=2,p=1,c=1,r=1,P=1,Q=1,H=2,V=-1",
                child.as_bytes(),
            ),
            (40, 50),
        );

        let child_snapshot = graphics
            .snapshots(&[
                KittyPlaceholderCell {
                    image_id: 1,
                    placement_id: 1,
                    x_cell: 10,
                    y_cell: 10,
                },
                KittyPlaceholderCell {
                    image_id: 1,
                    placement_id: 1,
                    x_cell: 3,
                    y_cell: 7,
                },
            ])
            .into_iter()
            .find(|snapshot| *snapshot.rgba == [0, 255, 0, 255])
            .unwrap();
        assert_eq!((child_snapshot.x_cell, child_snapshot.y_cell), (5, 6));
    }

    #[test]
    fn relative_placement_rejects_missing_parent_cycles_and_virtual_children() {
        let mut graphics = KittyGraphicsState::default();
        let pixel = STANDARD.encode([255, 0, 0, 255]);
        let missing = graphics.handle(
            command("a=T,f=32,s=1,v=1,i=1,p=1,P=99,Q=1", pixel.as_bytes()),
            (0, 0),
        );
        assert_eq!(
            missing.response,
            Some(b"\x1b_Gi=1,p=1;ENOPARENT:parent placement not found\x1b\\".to_vec())
        );

        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=2,p=1,C=1", pixel.as_bytes()),
            (0, 0),
        );
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=3,p=1,P=2,Q=1", pixel.as_bytes()),
            (0, 0),
        );
        let cycle = graphics.handle(command("a=p,i=2,p=1,P=3,Q=1", b""), (0, 0));
        assert_eq!(
            cycle.response,
            Some(b"\x1b_Gi=2,p=1;ECYCLE:relative placement cycle\x1b\\".to_vec())
        );

        let virtual_child = graphics.handle(command("a=p,i=2,p=2,U=1,P=3,Q=1", b""), (0, 0));
        assert_eq!(
            virtual_child.response,
            Some(b"\x1b_Gi=2,p=2;EINVAL:invalid control data\x1b\\".to_vec())
        );
    }

    #[test]
    fn deleting_parent_cascades_and_releases_unreferenced_child_images() {
        let mut graphics = KittyGraphicsState::default();
        for (image_id, parent) in [(1, None), (2, Some(1)), (3, Some(2))] {
            let payload = STANDARD.encode([image_id as u8, 0, 0, 255]);
            let control = parent.map_or_else(
                || format!("a=T,f=32,s=1,v=1,i={image_id},p=1,C=1"),
                |parent_id| format!("a=T,f=32,s=1,v=1,i={image_id},p=1,P={parent_id},Q=1"),
            );
            graphics.handle(command(&control, payload.as_bytes()), (0, 0));
        }
        assert_eq!(graphics.snapshots(&[]).len(), 3);

        graphics.handle(command("a=d,d=i,i=1,p=1", b""), (0, 0));
        assert!(graphics.snapshots(&[]).is_empty());
        for image_id in [2, 3] {
            let result = graphics.handle(command(&format!("a=p,i={image_id},p=2"), b""), (0, 0));
            assert_eq!(
                result.response,
                Some(format!("\x1b_Gi={image_id},p=2;ENOENT:image not found\x1b\\").into_bytes())
            );
        }
        let parent = graphics.handle(command("a=p,i=1,p=2,C=1", b""), (0, 0));
        assert_eq!(parent.response, Some(b"\x1b_Gi=1,p=2;OK\x1b\\".to_vec()));
    }

    #[test]
    fn joins_chunked_base64_before_decoding() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([1, 2, 3, 255]);
        let split = 4;

        let first = graphics.handle(
            command("a=T,f=32,s=1,v=1,i=9,m=1", &payload.as_bytes()[..split]),
            (0, 0),
        );
        assert!(first.response.is_none());
        assert!(graphics.snapshots(&[]).is_empty());

        let second = graphics.handle(command("m=0", &payload.as_bytes()[split..]), (2, 3));
        assert_eq!(second.response, Some(b"\x1b_Gi=9;OK\x1b\\".to_vec()));
        assert_eq!(
            (
                graphics.snapshots(&[])[0].x_cell,
                graphics.snapshots(&[])[0].y_cell
            ),
            (2, 3)
        );
    }

    #[test]
    fn joins_chunked_base64_when_transfer_action_is_repeated() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([1, 2, 3, 255]);
        let split = 4;

        let first = graphics.handle(
            command("a=T,f=32,s=1,v=1,i=9,m=1", &payload.as_bytes()[..split]),
            (0, 0),
        );
        assert!(first.response.is_none());

        let second = graphics.handle(command("a=T,m=0", &payload.as_bytes()[split..]), (2, 3));
        assert_eq!(second.response, Some(b"\x1b_Gi=9;OK\x1b\\".to_vec()));
        assert_eq!(graphics.snapshots(&[]).len(), 1);
    }

    #[test]
    fn query_validates_without_storing() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([0, 0, 0]);
        let result = graphics.handle(command("a=q,f=24,s=1,v=1,i=31", payload.as_bytes()), (0, 0));

        assert_eq!(result.response, Some(b"\x1b_Gi=31;OK\x1b\\".to_vec()));
        assert!(graphics.snapshots(&[]).is_empty());
    }

    #[test]
    fn query_accepts_unpadded_base64_payload() {
        let mut graphics = KittyGraphicsState::default();
        let payload = base64::engine::general_purpose::STANDARD_NO_PAD.encode([0, 0, 0, 0]);

        let result = graphics.handle(command("a=q,f=32,s=1,v=1,i=32", payload.as_bytes()), (0, 0));

        assert_eq!(result.response, Some(b"\x1b_Gi=32;OK\x1b\\".to_vec()));
        assert!(graphics.snapshots(&[]).is_empty());
    }

    #[test]
    fn yazi_quiet_upload_does_not_inject_a_response_into_input() {
        let payload = STANDARD.encode([255, 0, 0, 255]);
        let mut graphics = KittyGraphicsState::default();

        let result = graphics.handle(
            command("q=2,a=T,C=1,U=1,f=32,s=1,v=1,i=7", payload.as_bytes()),
            (0, 0),
        );

        assert!(result.response.is_none());
    }

    #[test]
    fn snacks_file_upload_creates_a_virtual_image() {
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[10, 20, 30]).unwrap();
        }
        let path = temp_path("snacks.png");
        fs::write(&path, png_bytes).unwrap();
        let encoded_path = STANDARD.encode(path.to_string_lossy().as_bytes());
        let mut graphics = KittyGraphicsState::default();

        let upload = graphics.handle(
            command("q=2,t=f,f=100,i=4", encoded_path.as_bytes()),
            (0, 0),
        );
        let placement = graphics.handle(command("q=2,a=p,U=1,i=4,p=1,c=1,r=1", b""), (0, 0));
        let snapshots = graphics.snapshots(&[KittyPlaceholderCell {
            image_id: 4,
            placement_id: 0,
            x_cell: 2,
            y_cell: 3,
        }]);

        fs::remove_file(path).unwrap();
        assert!(upload.response.is_none());
        assert!(placement.response.is_none());
        assert_eq!(snapshots.len(), 1);
        assert_eq!(&*snapshots[0].rgba, &[10, 20, 30, 255]);
    }

    #[test]
    fn image_numbers_allocate_ids_and_resolve_the_newest_image() {
        let mut graphics = KittyGraphicsState::default();
        let red = STANDARD.encode([255, 0, 0, 255]);
        let blue = STANDARD.encode([0, 0, 255, 255]);

        let first = graphics.handle(command("a=t,f=32,s=1,v=1,I=13", red.as_bytes()), (0, 0));
        let second = graphics.handle(command("a=t,f=32,s=1,v=1,I=13", blue.as_bytes()), (0, 0));
        let placement = graphics.handle(command("a=p,I=13,p=4,c=1,r=1", b""), (2, 3));

        assert_eq!(first.response, Some(b"\x1b_Gi=1,I=13;OK\x1b\\".to_vec()));
        assert_eq!(second.response, Some(b"\x1b_Gi=2,I=13;OK\x1b\\".to_vec()));
        assert_eq!(
            placement.response,
            Some(b"\x1b_Gi=2,I=13,p=4;OK\x1b\\".to_vec())
        );
        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[0, 0, 255, 255]);
    }

    #[test]
    fn image_id_and_number_cannot_be_combined() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([255, 0, 0, 255]);

        let result = graphics.handle(
            command("a=t,f=32,s=1,v=1,i=7,I=13", payload.as_bytes()),
            (0, 0),
        );

        assert_eq!(
            result.response,
            Some(b"\x1b_Gi=7,I=13;EINVAL:invalid control data\x1b\\".to_vec())
        );
        assert!(graphics.snapshots(&[]).is_empty());
    }

    #[test]
    fn delete_by_image_number_targets_only_the_newest_image() {
        let mut graphics = KittyGraphicsState::default();
        let red = STANDARD.encode([255, 0, 0, 255]);
        let blue = STANDARD.encode([0, 0, 255, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,I=13,p=1,c=1,r=1", red.as_bytes()),
            (0, 0),
        );
        graphics.handle(
            command("a=T,f=32,s=1,v=1,I=13,p=2,c=1,r=1", blue.as_bytes()),
            (1, 0),
        );

        let result = graphics.handle(command("a=d,d=N,I=13", b""), (0, 0));

        assert_eq!(result.response, Some(b"\x1b_Gi=2,I=13;OK\x1b\\".to_vec()));
        let snapshots = graphics.snapshots(&[]);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(&*snapshots[0].rgba, &[255, 0, 0, 255]);
    }

    #[test]
    fn virtual_placeholders_select_and_delete_by_placement_id() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([1, 2, 3, 255]);
        graphics.handle(command("a=t,f=32,s=1,v=1,i=7", payload.as_bytes()), (0, 0));
        graphics.handle(command("a=p,U=1,i=7,p=1,c=1,r=1", b""), (0, 0));
        graphics.handle(command("a=p,U=1,i=7,p=2,c=1,r=1", b""), (0, 0));
        let placeholder = [KittyPlaceholderCell {
            image_id: 7,
            placement_id: 1,
            x_cell: 2,
            y_cell: 3,
        }];

        assert_eq!(graphics.snapshots(&placeholder).len(), 1);
        graphics.handle(command("a=d,d=i,i=7,p=1", b""), (0, 0));
        assert!(graphics.snapshots(&placeholder).is_empty());
        assert_eq!(
            graphics
                .snapshots(&[KittyPlaceholderCell {
                    placement_id: 2,
                    ..placeholder[0]
                }])
                .len(),
            1
        );
    }

    #[test]
    fn temporary_file_medium_reads_and_removes_safe_protocol_file() {
        let path = temp_path("tty-graphics-protocol-rgba");
        fs::write(&path, [4, 3, 2, 1]).unwrap();
        let encoded_path = STANDARD.encode(path.to_string_lossy().as_bytes());
        let mut graphics = KittyGraphicsState::default();

        let result = graphics.handle(
            command("a=T,t=t,f=32,s=1,v=1,i=5", encoded_path.as_bytes()),
            (0, 0),
        );

        assert_eq!(result.response, Some(b"\x1b_Gi=5;OK\x1b\\".to_vec()));
        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[4, 3, 2, 1]);
        assert!(!path.exists());
    }

    #[test]
    fn current_kitten_temporary_file_probe_uses_the_path_length_as_data_size() {
        let path = temp_path("tty-graphics-protocol-query");
        fs::write(&path, [1, 2, 3]).unwrap();
        let path_bytes = path.to_string_lossy();
        let encoded_path = STANDARD.encode(path_bytes.as_bytes());
        let control = format!("a=q,t=t,f=24,s=1,v=1,S={},i=2", path_bytes.len());
        let mut graphics = KittyGraphicsState::default();

        let result = graphics.handle(command(&control, encoded_path.as_bytes()), (0, 0));

        assert_eq!(result.response, Some(b"\x1b_Gi=2;OK\x1b\\".to_vec()));
        assert!(graphics.snapshots(&[]).is_empty());
        assert!(!path.exists());
    }

    #[cfg(all(unix, not(target_os = "android")))]
    #[test]
    fn shared_memory_medium_reads_range_and_unlinks_object() {
        use nix::{
            fcntl::OFlag,
            sys::{mman, stat::Mode},
        };

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!(
            "/germinal-tty-graphics-protocol-{}-{nonce}",
            std::process::id()
        );
        let descriptor = mman::shm_open(
            name.as_str(),
            OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_RDWR,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .unwrap();
        let mut shared = File::from(descriptor);
        shared.write_all(&[9, 8, 7, 6, 5, 4]).unwrap();
        let encoded_name = STANDARD.encode(name.as_bytes());
        let mut graphics = KittyGraphicsState::default();

        let result = graphics.handle(
            command("a=T,t=s,f=32,s=1,v=1,O=1,S=4,i=6", encoded_name.as_bytes()),
            (0, 0),
        );

        assert_eq!(result.response, Some(b"\x1b_Gi=6;OK\x1b\\".to_vec()));
        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[8, 7, 6, 5]);
        assert!(
            mman::shm_open(name.as_str(), OFlag::O_RDONLY, Mode::empty()).is_err(),
            "the terminal must unlink POSIX shared memory after reading it"
        );
    }

    #[test]
    fn decodes_png_payload_to_rgba() {
        let mut png_bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[10, 20, 30]).unwrap();
        }
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode(png_bytes);

        graphics.handle(command("a=T,f=100,i=5,C=1", payload.as_bytes()), (0, 0));

        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[10, 20, 30, 255]);
    }

    #[test]
    fn inflates_zlib_payload_before_decoding() {
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&[1, 2, 3, 4]).unwrap();
        let payload = STANDARD.encode(encoder.finish().unwrap());
        let mut graphics = KittyGraphicsState::default();

        graphics.handle(
            command("a=T,f=32,s=1,v=1,o=z,i=8,C=1", payload.as_bytes()),
            (0, 0),
        );

        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[1, 2, 3, 4]);
    }

    #[test]
    fn virtual_placement_uses_connected_placeholder_bounds() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([1, 2, 3, 255]);
        graphics.handle(
            command("a=T,U=1,f=32,s=1,v=1,i=7,C=1", payload.as_bytes()),
            (0, 0),
        );
        let placeholders = [
            KittyPlaceholderCell {
                image_id: 7,
                placement_id: 0,
                x_cell: 4,
                y_cell: 3,
            },
            KittyPlaceholderCell {
                image_id: 7,
                placement_id: 0,
                x_cell: 5,
                y_cell: 3,
            },
            KittyPlaceholderCell {
                image_id: 7,
                placement_id: 0,
                x_cell: 4,
                y_cell: 4,
            },
            KittyPlaceholderCell {
                image_id: 7,
                placement_id: 0,
                x_cell: 5,
                y_cell: 4,
            },
        ];

        let snapshots = graphics.snapshots(&placeholders);

        assert_eq!(snapshots.len(), 1);
        assert_eq!((snapshots[0].x_cell, snapshots[0].y_cell), (4, 3));
        assert_eq!((snapshots[0].columns, snapshots[0].rows), (2, 2));
    }

    #[test]
    fn animation_frame_transmission_composes_partial_pixels_on_a_base_frame() {
        let mut graphics = KittyGraphicsState::default();
        let root = STANDARD.encode([255, 0, 0, 255, 255, 0, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=2,v=1,i=17,C=1", root.as_bytes()),
            (0, 0),
        );
        let patch = STANDARD.encode([0, 255, 0, 255]);

        let result = graphics.handle(
            command("a=f,f=32,s=1,v=1,i=17,c=1,x=1,y=0,z=25", patch.as_bytes()),
            (0, 0),
        );
        assert_eq!(result.response, Some(b"\x1b_Gi=17;OK\x1b\\".to_vec()));

        graphics.handle(command("a=a,i=17,c=2", b""), (0, 0));
        assert_eq!(
            &*graphics.snapshots(&[])[0].rgba,
            &[255, 0, 0, 255, 0, 255, 0, 255]
        );
    }

    #[test]
    fn chunked_animation_frame_requires_and_accepts_repeated_frame_action() {
        let mut graphics = KittyGraphicsState::default();
        let root = STANDARD.encode([255, 0, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=17,C=1", root.as_bytes()),
            (0, 0),
        );
        let frame = STANDARD.encode([0, 255, 0, 255]);
        let split = 4;

        assert!(
            graphics
                .handle(
                    command("a=f,f=32,s=1,v=1,i=17,m=1", &frame.as_bytes()[..split]),
                    (0, 0),
                )
                .response
                .is_none()
        );
        let invalid = graphics.handle(command("m=0", &frame.as_bytes()[split..]), (0, 0));
        assert_eq!(
            invalid.response,
            Some(b"\x1b_Gi=17;EINVAL:invalid chunk sequence\x1b\\".to_vec())
        );
        assert!(
            graphics
                .handle(
                    command("a=f,f=32,s=1,v=1,i=17,m=1", &frame.as_bytes()[..split]),
                    (0, 0),
                )
                .response
                .is_none()
        );
        let result = graphics.handle(command("a=f,m=0", &frame.as_bytes()[split..]), (0, 0));
        assert_eq!(result.response, Some(b"\x1b_Gi=17;OK\x1b\\".to_vec()));
        graphics.handle(command("a=a,i=17,c=2", b""), (0, 0));
        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[0, 255, 0, 255]);
    }

    #[test]
    fn terminal_driven_animation_advances_on_frame_deadlines_and_loops() {
        let mut graphics = KittyGraphicsState::default();
        let root = STANDARD.encode([255, 0, 0, 255]);
        let second = STANDARD.encode([0, 255, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=17,C=1", root.as_bytes()),
            (0, 0),
        );
        graphics.handle(
            command("a=f,f=32,s=1,v=1,i=17,z=10", second.as_bytes()),
            (0, 0),
        );
        let started_at = Instant::now();
        graphics
            .control_animation(command("a=a,i=17,r=1,z=10,s=3", b""), started_at)
            .unwrap();

        assert!(!graphics.advance_animations(started_at + Duration::from_millis(9)));
        assert!(graphics.advance_animations(started_at + Duration::from_millis(10)));
        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[0, 255, 0, 255]);
        assert!(graphics.advance_animations(started_at + Duration::from_millis(20)));
        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[255, 0, 0, 255]);
    }

    #[test]
    fn finite_animation_loop_count_stops_on_the_last_frame() {
        let mut graphics = KittyGraphicsState::default();
        let root = STANDARD.encode([255, 0, 0, 255]);
        let second = STANDARD.encode([0, 255, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=17,C=1", root.as_bytes()),
            (0, 0),
        );
        graphics.handle(
            command("a=f,f=32,s=1,v=1,i=17,z=10", second.as_bytes()),
            (0, 0),
        );
        let started_at = Instant::now();
        graphics
            .control_animation(command("a=a,i=17,r=1,z=10,s=3,v=2", b""), started_at)
            .unwrap();

        for elapsed_ms in [10, 20, 30] {
            assert!(graphics.advance_animations(started_at + Duration::from_millis(elapsed_ms)));
        }
        assert!(!graphics.advance_animations(started_at + Duration::from_millis(40)));
        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[0, 255, 0, 255]);
    }

    #[test]
    fn animation_composition_updates_the_destination_frame() {
        let mut graphics = KittyGraphicsState::default();
        let root = STANDARD.encode([255, 0, 0, 255, 255, 0, 0, 255]);
        let patch = STANDARD.encode([0, 255, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=2,v=1,i=17,C=1", root.as_bytes()),
            (0, 0),
        );
        graphics.handle(
            command("a=f,f=32,s=1,v=1,i=17,c=1,x=1", patch.as_bytes()),
            (0, 0),
        );

        let result = graphics.handle(
            command("a=c,i=17,r=2,c=1,X=1,Y=0,x=0,y=0,w=1,h=1,C=1", b""),
            (0, 0),
        );
        assert_eq!(result.response, Some(b"\x1b_Gi=17;OK\x1b\\".to_vec()));
        assert_eq!(
            &*graphics.snapshots(&[])[0].rgba,
            &[0, 255, 0, 255, 255, 0, 0, 255]
        );
    }

    #[test]
    fn deleting_animation_frames_keeps_the_root_image() {
        let mut graphics = KittyGraphicsState::default();
        let root = STANDARD.encode([255, 0, 0, 255]);
        let second = STANDARD.encode([0, 255, 0, 255]);
        graphics.handle(
            command("a=T,f=32,s=1,v=1,i=17,C=1", root.as_bytes()),
            (0, 0),
        );
        graphics.handle(command("a=f,f=32,s=1,v=1,i=17", second.as_bytes()), (0, 0));
        graphics.handle(command("a=a,i=17,c=2", b""), (0, 0));

        let result = graphics.handle(command("a=d,d=f,i=17", b""), (0, 0));
        assert_eq!(result.response, Some(b"\x1b_Gi=17;OK\x1b\\".to_vec()));
        assert_eq!(&*graphics.snapshots(&[])[0].rgba, &[255, 0, 0, 255]);
    }
}
