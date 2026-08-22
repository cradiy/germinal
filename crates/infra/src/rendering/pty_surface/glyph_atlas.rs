use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WgpuTerminalGlyphKey {
    identity: WgpuTerminalGlyphIdentity,
    bold: bool,
    italic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum WgpuTerminalGlyphIdentity {
    Character(char),
    Cluster(u64),
}

impl WgpuTerminalGlyphKey {
    const BOLD_BIT: u32 = 1 << 21;
    const ITALIC_BIT: u32 = 1 << 22;
    const CODEPOINT_MASK: u32 = Self::BOLD_BIT - 1;

    pub const fn new(c: char, bold: bool) -> Self {
        Self::styled(c, bold, false)
    }

    pub const fn styled(c: char, bold: bool, italic: bool) -> Self {
        Self {
            identity: WgpuTerminalGlyphIdentity::Character(c),
            bold,
            italic,
        }
    }

    pub const fn cluster(id: u64, bold: bool, italic: bool) -> Self {
        Self {
            identity: WgpuTerminalGlyphIdentity::Cluster(id),
            bold,
            italic,
        }
    }

    pub const fn plain(c: char) -> Self {
        Self::new(c, false)
    }

    pub const fn character(self) -> Option<char> {
        match self.identity {
            WgpuTerminalGlyphIdentity::Character(c) => Some(c),
            WgpuTerminalGlyphIdentity::Cluster(_) => None,
        }
    }

    pub const fn is_cluster(self) -> bool {
        matches!(self.identity, WgpuTerminalGlyphIdentity::Cluster(_))
    }

    pub const fn bold(self) -> bool {
        self.bold
    }

    pub const fn italic(self) -> bool {
        self.italic
    }

    pub const fn packed_id(self) -> u32 {
        match self.identity {
            WgpuTerminalGlyphIdentity::Character(c) => {
                (c as u32 & Self::CODEPOINT_MASK)
                    | if self.bold { Self::BOLD_BIT } else { 0 }
                    | if self.italic { Self::ITALIC_BIT } else { 0 }
            }
            WgpuTerminalGlyphIdentity::Cluster(id) => 0x8000_0000 | id as u32 & 0x7fff_ffff,
        }
    }

    pub fn from_packed_id(packed_id: u32) -> Option<Self> {
        if packed_id & 0x8000_0000 != 0 {
            return None;
        }
        let c = char::from_u32(packed_id & Self::CODEPOINT_MASK)?;
        let bold = (packed_id & Self::BOLD_BIT) != 0;
        let italic = (packed_id & Self::ITALIC_BIT) != 0;

        Some(Self::styled(c, bold, italic))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WgpuTerminalGlyphAtlas {
    pub width_px: u32,
    pub height_px: u32,
    pub pixels: Vec<u8>,
    pub entries: HashMap<WgpuTerminalGlyphKey, WgpuTerminalGlyphAtlasEntry>,
}

impl WgpuTerminalGlyphAtlas {
    pub fn empty() -> Self {
        Self {
            width_px: 0,
            height_px: 0,
            pixels: Vec::new(),
            entries: HashMap::new(),
        }
    }

    pub fn entry(&self, c: char) -> Option<&WgpuTerminalGlyphAtlasEntry> {
        self.entry_for_key(WgpuTerminalGlyphKey::plain(c))
    }

    pub fn entry_for_key(&self, key: WgpuTerminalGlyphKey) -> Option<&WgpuTerminalGlyphAtlasEntry> {
        self.entries.get(&key)
    }

    pub fn entry_for_packed_id(&self, packed_id: u32) -> Option<&WgpuTerminalGlyphAtlasEntry> {
        WgpuTerminalGlyphKey::from_packed_id(packed_id)
            .and_then(|key| self.entry_for_key(key))
            .or_else(|| {
                self.entries
                    .values()
                    .find(|entry| entry.codepoint == packed_id)
            })
    }

    pub fn has_glyph(&self, c: char) -> bool {
        self.has_glyph_key(WgpuTerminalGlyphKey::plain(c))
    }

    pub fn has_glyph_key(&self, key: WgpuTerminalGlyphKey) -> bool {
        self.entries.contains_key(&key)
    }

    pub fn pixel_count(&self) -> usize {
        self.pixels.len()
    }

    pub fn non_zero_pixel_count(&self) -> usize {
        self.pixels.iter().filter(|alpha| **alpha != 0).count()
    }

    pub fn is_empty(&self) -> bool {
        self.width_px == 0 || self.height_px == 0 || self.pixels.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuTerminalGlyphAtlasEntry {
    pub codepoint: u32,
    pub x_px: u32,
    pub y_px: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub advance_px: f32,
    pub uv: WgpuTerminalGlyphUvRect,
    pub draw_offset_x_px: i32,
    pub draw_offset_y_px: i32,
    pub draw_width_px: u32,
    pub draw_height_px: u32,
    pub is_color: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuTerminalGlyphUvRect {
    pub min_u: f32,
    pub min_v: f32,
    pub max_u: f32,
    pub max_v: f32,
}

impl WgpuTerminalGlyphUvRect {
    pub fn is_normalized(&self) -> bool {
        self.min_u >= 0.0
            && self.min_v >= 0.0
            && self.max_u <= 1.0
            && self.max_v <= 1.0
            && self.min_u <= self.max_u
            && self.min_v <= self.max_v
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WgpuDebugGlyphAtlasBuilder {
    glyph_width_px: u32,
    glyph_height_px: u32,
    padding_px: u32,
    columns: u32,
}

impl WgpuDebugGlyphAtlasBuilder {
    pub fn new() -> Self {
        Self {
            glyph_width_px: 5,
            glyph_height_px: 7,
            padding_px: 1,
            columns: 16,
        }
    }

    pub fn build_for_texts<I, S>(&self, texts: I) -> WgpuTerminalGlyphAtlas
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut glyphs = BTreeSet::new();

        for text in texts {
            for c in text.as_ref().chars() {
                glyphs.insert(WgpuTerminalGlyphKey::plain(c));
            }
        }

        self.build_for_glyphs(glyphs)
    }

    pub fn build_for_chars<I>(&self, chars: I) -> WgpuTerminalGlyphAtlas
    where
        I: IntoIterator<Item = char>,
    {
        self.build_for_glyphs(chars.into_iter().map(WgpuTerminalGlyphKey::plain))
    }

    pub fn build_for_glyphs<I>(&self, glyphs: I) -> WgpuTerminalGlyphAtlas
    where
        I: IntoIterator<Item = WgpuTerminalGlyphKey>,
    {
        let glyphs: Vec<WgpuTerminalGlyphKey> = glyphs.into_iter().collect();

        if glyphs.is_empty() {
            return WgpuTerminalGlyphAtlas {
                width_px: 0,
                height_px: 0,
                pixels: Vec::new(),
                entries: HashMap::new(),
            };
        }

        let cell_width = self.glyph_width_px + self.padding_px;
        let cell_height = self.glyph_height_px + self.padding_px;

        let row_count = (glyphs.len() as u32).div_ceil(self.columns);

        let atlas_width = self.padding_px + self.columns * cell_width;
        let atlas_height = self.padding_px + row_count * cell_height;

        let mut pixels = vec![0u8; (atlas_width * atlas_height) as usize];
        let mut entries = HashMap::new();

        for (index, glyph) in glyphs.into_iter().enumerate() {
            let index = index as u32;
            let col = index % self.columns;
            let row = index / self.columns;

            let x = self.padding_px + col * cell_width;
            let y = self.padding_px + row * cell_height;

            let Some(character) = glyph.character() else {
                continue;
            };
            self.write_glyph_pixels(character, x, y, atlas_width, &mut pixels);

            let uv = WgpuTerminalGlyphUvRect {
                min_u: x as f32 / atlas_width as f32,
                min_v: y as f32 / atlas_height as f32,
                max_u: (x + self.glyph_width_px) as f32 / atlas_width as f32,
                max_v: (y + self.glyph_height_px) as f32 / atlas_height as f32,
            };

            entries.insert(
                glyph,
                WgpuTerminalGlyphAtlasEntry {
                    codepoint: glyph.packed_id(),
                    x_px: x,
                    y_px: y,
                    width_px: self.glyph_width_px,
                    height_px: self.glyph_height_px,
                    advance_px: self.glyph_width_px as f32 + 1.0,
                    uv,
                    draw_offset_x_px: 0,
                    draw_offset_y_px: 0,
                    draw_width_px: self.glyph_width_px,
                    draw_height_px: self.glyph_height_px,
                    is_color: false,
                },
            );
        }

        WgpuTerminalGlyphAtlas {
            width_px: atlas_width,
            height_px: atlas_height,
            pixels,
            entries,
        }
    }

    fn write_glyph_pixels(
        &self,
        c: char,
        x_px: u32,
        y_px: u32,
        atlas_width_px: u32,
        pixels: &mut [u8],
    ) {
        let bitmap = debug_5x7_bitmap(c);

        for row in 0..self.glyph_height_px {
            let bits = bitmap[row as usize];

            for col in 0..self.glyph_width_px {
                let bit_index = self.glyph_width_px - 1 - col;
                let enabled = ((bits >> bit_index) & 1) != 0;

                let px = x_px + col;
                let py = y_px + row;
                let offset = (py * atlas_width_px + px) as usize;

                pixels[offset] = if enabled { 255 } else { 0 };
            }
        }
    }
}

impl Default for WgpuDebugGlyphAtlasBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn debug_5x7_bitmap(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],

        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],

        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        _ => [
            0b11111, 0b10001, 0b00101, 0b00010, 0b00101, 0b10001, 0b11111,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_glyph_key_round_trips_bold_and_italic_bits() {
        for key in [
            WgpuTerminalGlyphKey::styled('A', false, false),
            WgpuTerminalGlyphKey::styled('A', true, false),
            WgpuTerminalGlyphKey::styled('A', false, true),
            WgpuTerminalGlyphKey::styled('界', true, true),
        ] {
            assert_eq!(
                WgpuTerminalGlyphKey::from_packed_id(key.packed_id()),
                Some(key)
            );
        }
    }

    #[test]
    fn builds_debug_glyph_atlas_for_terminal_text() {
        let atlas = WgpuDebugGlyphAtlasBuilder::new()
            .build_for_texts(["red green under", "Germinal wgpu terminal smoke 123"]);

        assert!(!atlas.is_empty());
        assert!(atlas.width_px > 0);
        assert!(atlas.height_px > 0);
        assert_eq!(
            atlas.pixel_count(),
            (atlas.width_px * atlas.height_px) as usize
        );
        assert!(atlas.non_zero_pixel_count() > 0);

        assert!(atlas.has_glyph('r'));
        assert!(atlas.has_glyph('e'));
        assert!(atlas.has_glyph('d'));
        assert!(atlas.has_glyph('G'));
        assert!(atlas.has_glyph('1'));
        assert!(atlas.has_glyph(' '));
    }

    #[test]
    fn glyph_entries_have_normalized_uvs() {
        let atlas = WgpuDebugGlyphAtlasBuilder::new().build_for_texts(["red"]);

        for entry in atlas.entries.values() {
            assert!(entry.uv.is_normalized());
            assert_eq!(entry.width_px, 5);
            assert_eq!(entry.height_px, 7);
            assert!(entry.advance_px > 0.0);
        }
    }

    #[test]
    fn empty_input_builds_empty_atlas() {
        let atlas = WgpuDebugGlyphAtlasBuilder::new().build_for_texts([""]);

        assert!(atlas.is_empty());
        assert_eq!(atlas.pixel_count(), 0);
        assert_eq!(atlas.non_zero_pixel_count(), 0);
    }

    #[test]
    fn unknown_glyph_gets_fallback_bitmap() {
        let atlas = WgpuDebugGlyphAtlasBuilder::new().build_for_texts(["你"]);

        assert!(atlas.has_glyph('你'));
        assert!(atlas.non_zero_pixel_count() > 0);
    }
}
