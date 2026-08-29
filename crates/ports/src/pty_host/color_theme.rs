use crate::rendering::frame_plan_builder::RgbColorDto;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalColorTheme {
    pub palette: [RgbColorDto; 256],
    pub foreground: RgbColorDto,
    pub background: RgbColorDto,
    pub cursor: RgbColorDto,
    pub cursor_text: RgbColorDto,
    pub selection_foreground: Option<RgbColorDto>,
    pub selection_background: Option<RgbColorDto>,
    pub url: RgbColorDto,
    pub active_border: RgbColorDto,
    pub inactive_border: RgbColorDto,
    pub bell_border: RgbColorDto,
    pub active_tab_foreground: RgbColorDto,
    pub active_tab_background: RgbColorDto,
    pub inactive_tab_foreground: RgbColorDto,
    pub inactive_tab_background: RgbColorDto,
}

impl TerminalColorTheme {
    pub fn with_derived_host_colors(mut self) -> Self {
        let contrast = contrasting_color(self.background);
        self.inactive_border = mix_rgb(self.background, contrast, 42, 255);
        self.active_border = mix_rgb(self.background, contrast, 155, 255);
        self.bell_border = self.palette[11];
        self.url = self.palette[12];
        self.inactive_tab_background = mix_rgb(self.background, contrast, 24, 255);
        self.inactive_tab_foreground = mix_rgb(self.background, contrast, 132, 255);
        self.active_tab_background = mix_rgb(self.background, contrast, 48, 255);
        self.active_tab_foreground = mix_rgb(
            self.active_tab_background,
            contrasting_color(self.active_tab_background),
            220,
            255,
        );
        self
    }
}

impl Default for TerminalColorTheme {
    fn default() -> Self {
        let palette = default_palette();
        Self {
            palette,
            foreground: RgbColorDto::new(229, 229, 229),
            background: RgbColorDto::new(0, 0, 0),
            cursor: RgbColorDto::new(235, 235, 235),
            cursor_text: RgbColorDto::new(0, 0, 0),
            selection_foreground: None,
            selection_background: None,
            url: palette[12],
            active_border: RgbColorDto::new(167, 178, 201),
            inactive_border: RgbColorDto::new(60, 69, 87),
            bell_border: RgbColorDto::new(255, 158, 46),
            active_tab_foreground: RgbColorDto::new(225, 228, 236),
            active_tab_background: RgbColorDto::new(72, 75, 88),
            inactive_tab_foreground: RgbColorDto::new(132, 136, 153),
            inactive_tab_background: RgbColorDto::new(42, 44, 56),
        }
    }
}

fn default_palette() -> [RgbColorDto; 256] {
    let mut palette = [RgbColorDto::new(0, 0, 0); 256];
    palette[..16].copy_from_slice(&[
        RgbColorDto::new(0, 0, 0),
        RgbColorDto::new(205, 49, 49),
        RgbColorDto::new(13, 188, 121),
        RgbColorDto::new(229, 229, 16),
        RgbColorDto::new(36, 114, 200),
        RgbColorDto::new(188, 63, 188),
        RgbColorDto::new(17, 168, 205),
        RgbColorDto::new(229, 229, 229),
        RgbColorDto::new(102, 102, 102),
        RgbColorDto::new(241, 76, 76),
        RgbColorDto::new(35, 209, 139),
        RgbColorDto::new(245, 245, 67),
        RgbColorDto::new(59, 142, 234),
        RgbColorDto::new(214, 112, 214),
        RgbColorDto::new(41, 184, 219),
        RgbColorDto::new(255, 255, 255),
    ]);

    for index in 16u16..=231 {
        let cube_index = index - 16;
        let red = cube_component((cube_index / 36) as u8);
        let green = cube_component(((cube_index % 36) / 6) as u8);
        let blue = cube_component((cube_index % 6) as u8);
        palette[index as usize] = RgbColorDto::new(red, green, blue);
    }
    for index in 232u16..=255 {
        let level = 8 + 10 * (index as u8 - 232);
        palette[index as usize] = RgbColorDto::new(level, level, level);
    }
    palette
}

fn cube_component(level: u8) -> u8 {
    if level == 0 { 0 } else { 55 + level * 40 }
}

fn contrasting_color(color: RgbColorDto) -> RgbColorDto {
    let luminance =
        u32::from(color.red) * 299 + u32::from(color.green) * 587 + u32::from(color.blue) * 114;
    if luminance < 140_000 {
        RgbColorDto::new(255, 255, 255)
    } else {
        RgbColorDto::new(0, 0, 0)
    }
}

fn mix_rgb(from: RgbColorDto, to: RgbColorDto, amount: u16, total: u16) -> RgbColorDto {
    fn channel(from: u8, to: u8, amount: u16, total: u16) -> u8 {
        let inverse = total.saturating_sub(amount);
        ((u32::from(from) * u32::from(inverse) + u32::from(to) * u32::from(amount))
            / u32::from(total.max(1))) as u8
    }

    RgbColorDto::new(
        channel(from.red, to.red, amount, total),
        channel(from.green, to.green, amount, total),
        channel(from.blue, to.blue, amount, total),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_palette_contains_ansi_cube_and_grayscale_colors() {
        let theme = TerminalColorTheme::default();
        assert_eq!(theme.palette[1], RgbColorDto::new(205, 49, 49));
        assert_eq!(theme.palette[16], RgbColorDto::new(0, 0, 0));
        assert_eq!(theme.palette[21], RgbColorDto::new(0, 0, 255));
        assert_eq!(theme.palette[232], RgbColorDto::new(8, 8, 8));
        assert_eq!(theme.palette[255], RgbColorDto::new(238, 238, 238));
    }
}
