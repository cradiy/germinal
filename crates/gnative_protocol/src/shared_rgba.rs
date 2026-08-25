pub const SHARED_RGBA_MAGIC: [u8; 8] = *b"GRGBA001";
pub const SHARED_RGBA_HEADER_BYTES: usize = 64;
pub const SHARED_RGBA_SLOT_HEADER_BYTES: usize = 16;
pub const SHARED_RGBA_SLOT_COUNT: u32 = 3;

pub const SHARED_RGBA_SLOT_FREE: u32 = 0;
pub const SHARED_RGBA_SLOT_WRITING: u32 = 1;
pub const SHARED_RGBA_SLOT_READY: u32 = 2;
pub const SHARED_RGBA_SLOT_READING: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharedRgbaLayout {
    pub width_px: u32,
    pub height_px: u32,
    pub stride_bytes: u32,
    pub slot_count: u32,
}

impl SharedRgbaLayout {
    pub fn new(width_px: u32, height_px: u32) -> Option<Self> {
        let stride_bytes = width_px.checked_mul(4)?;
        let layout = Self {
            width_px,
            height_px,
            stride_bytes,
            slot_count: SHARED_RGBA_SLOT_COUNT,
        };
        layout.file_len()?;
        Some(layout)
    }

    pub fn frame_bytes(self) -> Option<usize> {
        usize::try_from(self.stride_bytes)
            .ok()?
            .checked_mul(usize::try_from(self.height_px).ok()?)
    }

    pub fn slot_bytes(self) -> Option<usize> {
        let unaligned = SHARED_RGBA_SLOT_HEADER_BYTES.checked_add(self.frame_bytes()?)?;
        unaligned.checked_add(15).map(|value| value & !15)
    }

    pub fn file_len(self) -> Option<usize> {
        SHARED_RGBA_HEADER_BYTES.checked_add(
            self.slot_bytes()?
                .checked_mul(usize::try_from(self.slot_count).ok()?)?,
        )
    }

    pub fn slot_header_offset(self, slot: u32) -> Option<usize> {
        if slot >= self.slot_count {
            return None;
        }
        SHARED_RGBA_HEADER_BYTES.checked_add(
            self.slot_bytes()?
                .checked_mul(usize::try_from(slot).ok()?)?,
        )
    }

    pub fn slot_data_offset(self, slot: u32) -> Option<usize> {
        self.slot_header_offset(slot)?
            .checked_add(SHARED_RGBA_SLOT_HEADER_BYTES)
    }
}
