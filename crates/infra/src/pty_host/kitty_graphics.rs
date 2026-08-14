use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Cursor, Read},
    sync::Arc,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use flate2::read::ZlibDecoder;
use germinal_ports::rendering::surface_snapshot::RenderSurfaceImageSnapshot;

const MAX_APC_BYTES: usize = 16 * 1024;
const MAX_ENCODED_IMAGE_BYTES: usize = 96 * 1024 * 1024;
const MAX_DECODED_IMAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 8_192;
const MAX_IMAGES: usize = 256;
const MAX_TOTAL_IMAGE_BYTES: usize = 128 * 1024 * 1024;

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

    fn has_only_chunk_continuation_keys(&self) -> bool {
        self.control.keys().all(|key| matches!(key, b'm' | b'q'))
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
    pub(crate) fn feed(&mut self, input: &[u8]) -> Vec<KittyStreamEvent> {
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
    generation: u64,
    width_px: u32,
    height_px: u32,
    rgba: Arc<[u8]>,
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
    source_x_px: u32,
    source_y_px: u32,
    source_width_px: u32,
    source_height_px: u32,
    z_index: i32,
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
    pub x_cell: u32,
    pub y_cell: u32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct KittyCommandResult {
    pub response: Option<Vec<u8>>,
    pub cursor_move: Option<KittyCursorMove>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct KittyGraphicsState {
    images: HashMap<u64, KittyImage>,
    image_ids: HashMap<u32, u64>,
    placements: HashMap<u64, KittyPlacement>,
    virtual_images: HashSet<u64>,
    insertion_order: VecDeque<u64>,
    pending: Option<PendingTransmission>,
    next_key: u64,
    next_generation: u64,
    total_bytes: usize,
}

impl KittyGraphicsState {
    pub(crate) fn handle(
        &mut self,
        command: KittyCommand,
        cursor: (u32, u32),
    ) -> KittyCommandResult {
        let response_command = self
            .pending
            .as_ref()
            .map(|pending| &pending.command)
            .unwrap_or(&command);
        let quiet = response_command.u32(b'q', 0).unwrap_or(0);
        let image_id = response_command.u32(b'i', 0).unwrap_or(0);
        let placement_id = response_command.u32(b'p', 0).unwrap_or(0);
        let result = self.handle_inner(command, cursor);
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
                    response_for(image_id, placement_id, error.message())
                },
                cursor_move: None,
            },
        }
    }

    fn handle_inner(
        &mut self,
        mut command: KittyCommand,
        cursor: (u32, u32),
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        if command.control.contains_key(&b'I') {
            return Err(KittyGraphicsError::UnsupportedImageNumber);
        }
        if command.u32(b'U', 0)? > 1 {
            return Err(KittyGraphicsError::InvalidControl);
        }
        if self.pending.is_some() {
            if command.char(b'a', 't') == 'd' {
                self.pending = None;
                return self.delete(command);
            }
            if !command.has_only_chunk_continuation_keys() {
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
        if matches!(action, 't' | 'T' | 'q') && command.u32(b'm', 0)? == 1 {
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
            't' | 'T' | 'q' => self.transmit(command, cursor, action),
            'p' => self.place(command, cursor),
            'd' => self.delete(command),
            _ => Err(KittyGraphicsError::UnsupportedAction),
        }
    }

    fn transmit(
        &mut self,
        command: KittyCommand,
        cursor: (u32, u32),
        action: char,
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        if command.char(b't', 'd') != 'd' {
            return Err(KittyGraphicsError::UnsupportedMedium);
        }

        let image_id = command.u32(b'i', 0)?;
        let placement_id = command.u32(b'p', 0)?;
        let encoded = STANDARD
            .decode(&command.payload)
            .map_err(|_| KittyGraphicsError::InvalidPayload)?;
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
                response: response_for(image_id, placement_id, "OK"),
                cursor_move: None,
            });
        }

        let image_key = self.insert_image(image_id, decoded);
        let cursor_move = if action == 'T' && command.u32(b'U', 0)? == 1 {
            self.virtual_images.insert(image_key);
            None
        } else if action == 'T' {
            Some(self.insert_placement(&command, cursor, image_key, image_id)?)
        } else {
            None
        };

        Ok(KittyCommandResult {
            response: response_for(image_id, placement_id, "OK"),
            cursor_move,
        })
    }

    fn place(
        &mut self,
        command: KittyCommand,
        cursor: (u32, u32),
    ) -> Result<KittyCommandResult, KittyGraphicsError> {
        let image_id = command.u32(b'i', 0)?;
        let image_key = *self
            .image_ids
            .get(&image_id)
            .ok_or(KittyGraphicsError::ImageNotFound)?;
        let placement_id = command.u32(b'p', 0)?;
        if command.u32(b'U', 0)? == 1 {
            self.virtual_images.insert(image_key);
            return Ok(KittyCommandResult {
                response: response_for(image_id, placement_id, "OK"),
                cursor_move: None,
            });
        }
        let cursor_move = self.insert_placement(&command, cursor, image_key, image_id)?;

        Ok(KittyCommandResult {
            response: response_for(image_id, placement_id, "OK"),
            cursor_move: Some(cursor_move),
        })
    }

    fn delete(&mut self, command: KittyCommand) -> Result<KittyCommandResult, KittyGraphicsError> {
        self.pending = None;
        let selector = command.char(b'd', 'a');
        let image_id = command.u32(b'i', 0)?;
        let placement_id = command.u32(b'p', 0)?;

        match selector.to_ascii_lowercase() {
            'a' => {
                self.placements.clear();
                self.virtual_images.clear();
                if selector.is_ascii_uppercase() {
                    self.images.clear();
                    self.image_ids.clear();
                    self.insertion_order.clear();
                    self.total_bytes = 0;
                }
            }
            'i' if image_id != 0 => {
                if let Some(image_key) = self.image_ids.get(&image_id).copied() {
                    self.placements.retain(|_, placement| {
                        placement.image_key != image_key
                            || (placement_id != 0 && placement.placement_id != placement_id)
                    });
                    if selector.is_ascii_uppercase() && placement_id == 0 {
                        self.remove_image(image_key);
                    }
                }
            }
            _ => return Err(KittyGraphicsError::UnsupportedDelete),
        }

        Ok(KittyCommandResult {
            response: response_for(image_id, placement_id, "OK"),
            cursor_move: None,
        })
    }

    fn insert_image(&mut self, image_id: u32, decoded: DecodedImage) -> u64 {
        if let Some(old_key) = self.image_ids.remove(&image_id).filter(|_| image_id != 0) {
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
                generation: self.next_generation,
                width_px: decoded.width_px,
                height_px: decoded.height_px,
                rgba: decoded.rgba.into(),
            },
        );
        self.insertion_order.push_back(key);
        if image_id != 0 {
            self.image_ids.insert(image_id, key);
        }
        self.evict_to_quota();
        key
    }

    fn insert_placement(
        &mut self,
        command: &KittyCommand,
        cursor: (u32, u32),
        image_key: u64,
        image_id: u32,
    ) -> Result<KittyCursorMove, KittyGraphicsError> {
        let image = self
            .images
            .get(&image_key)
            .ok_or(KittyGraphicsError::ImageNotFound)?;
        let placement_id = command.u32(b'p', 0)?;
        let columns = command.u32(b'c', 0)?;
        let rows = command.u32(b'r', 0)?;
        self.next_key = self.next_key.saturating_add(1).max(1);
        let key = if image_id != 0 && placement_id != 0 {
            self.placements
                .iter()
                .find_map(|(key, placement)| {
                    (placement.image_id == image_id && placement.placement_id == placement_id)
                        .then_some(*key)
                })
                .unwrap_or(self.next_key)
        } else {
            self.next_key
        };
        let source_x_px = command.u32(b'x', 0)?.min(image.width_px);
        let source_y_px = command.u32(b'y', 0)?.min(image.height_px);
        let max_width = image.width_px.saturating_sub(source_x_px);
        let max_height = image.height_px.saturating_sub(source_y_px);
        let source_width_px = command.u32(b'w', 0)?.min(max_width);
        let source_height_px = command.u32(b'h', 0)?.min(max_height);

        self.placements.insert(
            key,
            KittyPlacement {
                key,
                image_key,
                image_id,
                placement_id,
                x_cell: cursor.0,
                y_cell: cursor.1,
                x_offset_px: command.u32(b'X', 0)?,
                y_offset_px: command.u32(b'Y', 0)?,
                columns,
                rows,
                source_x_px,
                source_y_px,
                source_width_px: if source_width_px == 0 {
                    max_width
                } else {
                    source_width_px
                },
                source_height_px: if source_height_px == 0 {
                    max_height
                } else {
                    source_height_px
                },
                z_index: command.i32(b'z', 0)?,
            },
        );

        if command.u32(b'C', 0)? == 1 {
            Ok(KittyCursorMove {
                columns: 0,
                rows: 0,
            })
        } else {
            Ok(KittyCursorMove { columns, rows })
        }
    }

    pub(crate) fn snapshots(
        &self,
        placeholder_cells: &[KittyPlaceholderCell],
    ) -> Vec<RenderSurfaceImageSnapshot> {
        let mut snapshots: Vec<_> = self
            .placements
            .values()
            .filter_map(|placement| {
                let image = self.images.get(&placement.image_key)?;
                Some(RenderSurfaceImageSnapshot {
                    id: format!("{}:{}", image.key, placement.key),
                    image_generation: image.generation,
                    x_cell: placement.x_cell,
                    y_cell: placement.y_cell,
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
                    z_index: placement.z_index,
                    rgba: Arc::clone(&image.rgba),
                })
            })
            .collect();
        snapshots.extend(self.virtual_snapshots(placeholder_cells));
        snapshots.sort_by_key(|snapshot| snapshot.z_index);
        snapshots
    }

    fn virtual_snapshots(
        &self,
        placeholder_cells: &[KittyPlaceholderCell],
    ) -> Vec<RenderSurfaceImageSnapshot> {
        let mut positions_by_image = HashMap::<u32, HashSet<(u32, u32)>>::new();
        for cell in placeholder_cells {
            let Some(image_key) = self.image_ids.get(&cell.image_id) else {
                continue;
            };
            if self.virtual_images.contains(image_key) {
                positions_by_image
                    .entry(cell.image_id)
                    .or_default()
                    .insert((cell.x_cell, cell.y_cell));
            }
        }

        let mut snapshots = Vec::new();
        for (image_id, mut remaining) in positions_by_image {
            let Some(image_key) = self.image_ids.get(&image_id).copied() else {
                continue;
            };
            let Some(image) = self.images.get(&image_key) else {
                continue;
            };
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
                    id: format!("virtual:{image_id}:{min_x}:{min_y}"),
                    image_generation: image.generation,
                    x_cell: min_x,
                    y_cell: min_y,
                    x_offset_px: 0,
                    y_offset_px: 0,
                    columns: max_x.saturating_sub(min_x).saturating_add(1),
                    rows: max_y.saturating_sub(min_y).saturating_add(1),
                    source_x_px: 0,
                    source_y_px: 0,
                    source_width_px: image.width_px,
                    source_height_px: image.height_px,
                    image_width_px: image.width_px,
                    image_height_px: image.height_px,
                    z_index: 0,
                    rgba: Arc::clone(&image.rgba),
                });
            }
        }
        snapshots
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
        if let Some(image) = self.images.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(image.rgba.len());
        }
        self.image_ids.retain(|_, value| *value != key);
        self.placements
            .retain(|_, placement| placement.image_key != key);
        self.virtual_images.remove(&key);
    }
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

fn response_for(image_id: u32, placement_id: u32, message: &str) -> Option<Vec<u8>> {
    if image_id == 0 {
        return None;
    }
    let placement = if placement_id == 0 {
        String::new()
    } else {
        format!(",p={placement_id}")
    };
    Some(format!("\x1b_Gi={image_id}{placement};{message}\x1b\\").into_bytes())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KittyGraphicsError {
    InvalidControl,
    InvalidChunk,
    InvalidPayload,
    ImageTooLarge,
    ImageNotFound,
    UnsupportedAction,
    UnsupportedCompression,
    UnsupportedDelete,
    UnsupportedFormat,
    UnsupportedImageNumber,
    UnsupportedMedium,
}

impl KittyGraphicsError {
    fn message(self) -> &'static str {
        match self {
            Self::InvalidControl => "EINVAL:invalid control data",
            Self::InvalidChunk => "EINVAL:invalid chunk sequence",
            Self::InvalidPayload => "EINVAL:invalid image payload",
            Self::ImageTooLarge => "E2BIG:image exceeds terminal limits",
            Self::ImageNotFound => "ENOENT:image not found",
            Self::UnsupportedAction => "ENOTSUP:action is not supported",
            Self::UnsupportedCompression => "ENOTSUP:compression is not supported",
            Self::UnsupportedDelete => "ENOTSUP:delete selector is not supported",
            Self::UnsupportedFormat => "ENOTSUP:image format is not supported",
            Self::UnsupportedImageNumber => "ENOTSUP:image numbers are not supported",
            Self::UnsupportedMedium => "ENOTSUP:transmission medium is not supported",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn command(control: &str, payload: &[u8]) -> KittyCommand {
        let mut bytes = control.as_bytes().to_vec();
        bytes.push(b';');
        bytes.extend_from_slice(payload);
        KittyCommand::parse(&bytes).unwrap()
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
    fn decoder_preserves_non_kitty_apc_prefix() {
        let mut decoder = KittyGraphicsStreamDecoder::default();
        assert_eq!(
            decoder.feed(b"a\x1b_Xpayload\x1b\\b"),
            vec![KittyStreamEvent::Bytes(b"a\x1b_Xpayload\x1b\\b".to_vec())]
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
    fn query_validates_without_storing() {
        let mut graphics = KittyGraphicsState::default();
        let payload = STANDARD.encode([0, 0, 0]);
        let result = graphics.handle(command("a=q,f=24,s=1,v=1,i=31", payload.as_bytes()), (0, 0));

        assert_eq!(result.response, Some(b"\x1b_Gi=31;OK\x1b\\".to_vec()));
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
    fn rejects_unsupported_file_medium() {
        let mut graphics = KittyGraphicsState::default();
        let path = STANDARD.encode("/tmp/image.png");
        let result = graphics.handle(command("a=t,t=f,f=100,i=4", path.as_bytes()), (0, 0));

        assert_eq!(
            result.response,
            Some(b"\x1b_Gi=4;ENOTSUP:transmission medium is not supported\x1b\\".to_vec())
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
                x_cell: 4,
                y_cell: 3,
            },
            KittyPlaceholderCell {
                image_id: 7,
                x_cell: 5,
                y_cell: 3,
            },
            KittyPlaceholderCell {
                image_id: 7,
                x_cell: 4,
                y_cell: 4,
            },
            KittyPlaceholderCell {
                image_id: 7,
                x_cell: 5,
                y_cell: 4,
            },
        ];

        let snapshots = graphics.snapshots(&placeholders);

        assert_eq!(snapshots.len(), 1);
        assert_eq!((snapshots[0].x_cell, snapshots[0].y_cell), (4, 3));
        assert_eq!((snapshots[0].columns, snapshots[0].rows), (2, 2));
    }
}
