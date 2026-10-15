use super::*;
use crate::rendering::pty_surface::svg_icon::SvgIconCache;
use germinal_ports::rendering::tab_bar::{SIDEBAR_RENDER_TARGET_ID, sidebar_width_px};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub(super) struct TabSidebar {
    pub(super) width: Option<u32>,
    first: usize,
    active: Option<usize>,
    visible: usize,
    icons: SvgIconCache,
    pub(super) pointer: Option<(f64, f64)>,
}

impl TabSidebar {
    pub(super) fn scroll(&mut self, down: bool) {
        self.first = if down {
            self.first.saturating_add(1)
        } else {
            self.first.saturating_sub(1)
        };
    }

    pub(super) fn build(
        &mut self,
        bar: &TabBarSnapshot,
        size: TerminalSizeInfo,
        cell: TerminalCellSize,
        toolbar_height: u32,
        theme: TerminalColorTheme,
    ) -> Option<WgpuTabBarSurface> {
        if bar.titles.is_empty() {
            return None;
        }
        let cw = cell.width_px().max(1);
        let ch = cell.height_px().max(1);
        let width = self.width.unwrap_or_else(|| {
            sidebar_width_px(size.window_size().width_px(), size.cell_size().height_px())
        });
        let height = size.window_size().height_px();
        if width == 0 || height == 0 {
            return None;
        }
        let columns = width / cw;
        let row_count = height / ch;
        let first_row = toolbar_height.saturating_add(ch / 4).div_ceil(ch);
        let visible = (row_count.saturating_sub(first_row) / 2).max(1) as usize;
        self.first = self.first.min(bar.titles.len().saturating_sub(visible));
        if self.active != Some(bar.active_tab_index) || self.visible != visible {
            if bar.active_tab_index < self.first {
                self.first = bar.active_tab_index;
            } else if bar.active_tab_index >= self.first + visible {
                self.first = bar.active_tab_index.saturating_sub(visible - 1);
            }
        }
        self.active = Some(bar.active_tab_index);
        self.visible = visible;
        let background = mix_rgb(theme.background, theme.foreground, 3, 100);
        let muted = mix_rgb(background, theme.foreground, 78, 100);
        let subtle = mix_rgb(background, theme.foreground, 55, 100);
        let run = |x, text: String, color, bold| RenderSurfaceRunSnapshot {
            x,
            text,
            style: TextStyleDto {
                foreground: Some(color),
                bold,
                ..TextStyleDto::plain()
            },
            decoration: Default::default(),
        };
        let mut rows = Vec::new();
        let mut images = Vec::new();
        let mut hits = Vec::new();
        if columns >= 8 && row_count >= first_row + 2 {
            let icon_size = (ch * 3 / 4).min(cw * 2).clamp(1, 512);
            let icon = self
                .icons
                .rasterize(
                    include_str!("../icons/terminal.svg"),
                    icon_size,
                    Some(subtle),
                )
                .ok();
            for (index, title) in bar.titles.iter().enumerate().skip(self.first).take(visible) {
                let y = first_row + (index - self.first) as u32 * 2;
                let active = index == bar.active_tab_index;
                let hovered = self.pointer.is_some_and(|(x, pointer_y)| {
                    x >= f64::from(cw)
                        && x < f64::from(columns.saturating_sub(1) * cw)
                        && pointer_y >= f64::from(y * ch - ch / 4)
                        && pointer_y < f64::from(y * ch + ch * 5 / 4)
                });
                let label = bar
                    .progresses
                    .get(index)
                    .copied()
                    .flatten()
                    .map(tab_progress_label)
                    .unwrap_or_default();
                let label = if hovered { String::new() } else { label };
                let label = truncate_title_to_cells(&label, columns.saturating_sub(10));
                let label_width = terminal_text_cell_width(&label);
                let title = title_tail(title, columns.saturating_sub(9 + label_width));
                let mut marker = run(0, String::new(), muted, active);
                marker.style.italic = hovered;
                rows.push(RenderSurfaceRowSnapshot {
                    y,
                    runs: vec![
                        marker,
                        run(
                            5,
                            title,
                            if active { theme.foreground } else { muted },
                            false,
                        ),
                        run(
                            columns.saturating_sub(label_width + 1),
                            label,
                            theme.foreground,
                            false,
                        ),
                    ],
                });
                if let Some(icon) = &icon {
                    images.push(icon.placement(
                        &format!("tab-{index}"),
                        cw * 2,
                        y * ch + (ch - icon_size) / 2,
                    ));
                }
                if hovered {
                    let close_hovered = self
                        .pointer
                        .is_some_and(|(x, _)| x >= f64::from(columns.saturating_sub(3) * cw));
                    if let Ok(close) = self.icons.rasterize(
                        include_str!("../icons/close.svg"),
                        icon_size,
                        Some(if close_hovered {
                            theme.foreground
                        } else {
                            subtle
                        }),
                    ) {
                        images.push(close.placement(
                            &format!("tab-close-{index}"),
                            columns.saturating_sub(3) * cw + (cw * 2 - icon_size) / 2,
                            y * ch + (ch - icon_size) / 2,
                        ));
                    }
                }
                hits.push(TabBarHitRegion {
                    tab_index: index,
                    start_column: 1,
                    end_column: columns.saturating_sub(1),
                    top_px: y * ch - ch / 4,
                    bottom_px: y * ch + ch * 5 / 4,
                });
            }
        }
        Some(WgpuTabBarSurface {
            placement: RenderSurfacePlacement::new(SIDEBAR_RENDER_TARGET_ID, 0, 0, width, height),
            snapshot: RenderSurfaceSnapshot {
                target_id: SIDEBAR_RENDER_TARGET_ID,
                latest_seq: Seq::new(0),
                default_background: background,
                rows,
                image_surfaces: images,
                dirty_rows: Vec::new(),
                scrollback: None,
                cursor: None,
                ime_preedit: None,
            },
            cell_width_px: cw,
            hit_regions: hits,
        })
    }
}

fn title_tail(title: &str, max_width: u32) -> String {
    if terminal_text_cell_width(title) <= max_width {
        return title.to_owned();
    }
    if max_width == 0 {
        return String::new();
    }
    let mut width = 1;
    let mut start = title.len();
    for (offset, grapheme) in title.grapheme_indices(true).rev() {
        let next = width + terminal_text_cell_width(grapheme);
        if next > max_width {
            break;
        }
        width = next;
        start = offset;
    }
    format!("…{}", &title[start..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_hover_close_and_tail_share_the_visible_row() {
        let size = TerminalSizeInfo::zero_padding(
            TerminalWindowSize::new(900, 480),
            TerminalCellSize::new(12, 24),
        );
        let bar = TabBarSnapshot {
            titles: vec!["~/long/directory/中文-e\u{301}".into(), "other".into()],
            progresses: vec![],
            render_target_ids: vec![],
            active_tab_index: 1,
            position: TabBarPosition::Left,
            style: TabBarStyle::Fade,
        };
        let mut state = TabSidebar {
            pointer: Some((210.0, 48.0)),
            ..Default::default()
        };
        let build = |state: &mut TabSidebar| {
            state
                .build(
                    &bar,
                    size,
                    size.cell_size(),
                    40,
                    TerminalColorTheme::default(),
                )
                .unwrap()
        };
        let surface = build(&mut state);
        assert!(surface.snapshot.rows[0].runs[0].style.italic);
        assert!(!surface.snapshot.rows[1].runs[0].style.italic);
        let title = &surface.snapshot.rows[0].runs[1].text;
        assert!(title.starts_with('…') && title.ends_with("中文-e\u{301}"));
        assert!(terminal_text_cell_width(title) <= 11);
        assert_eq!(surface.sidebar_close_at_position(210.0, 48.0), Some(0));
        assert_eq!(surface.sidebar_close_at_position(60.0, 48.0), None);
        assert_eq!(surface.sidebar_close_at_position(210.0, 20.0), None);
        let image_count = surface.snapshot.image_surfaces.len();
        state.pointer = None;
        let plain = build(&mut state);
        assert_eq!(image_count, plain.snapshot.image_surfaces.len() + 1);
        assert_eq!(plain.snapshot.rows[0].runs[1].text, *title);
        assert!(!plain.snapshot.rows[0].runs[0].style.italic);
        state.width = Some(336);
        state.pointer = Some((306.0, 48.0));
        let resized = build(&mut state);
        assert_eq!(resized.sidebar_close_at_position(210.0, 48.0), None);
        assert_eq!(resized.sidebar_close_at_position(306.0, 48.0), Some(0));
        assert_eq!(resized.tab_index_at_position(337.0, 48.0), None);
    }

    #[test]
    fn sidebar_hit_testing_scrolling_and_active_reveal_share_layout() {
        let size = TerminalSizeInfo::zero_padding(
            TerminalWindowSize::new(900, 480),
            TerminalCellSize::new(12, 24),
        );
        let mut bar = TabBarSnapshot {
            titles: (0..30).map(|i| format!("Terminal {i} 中文")).collect(),
            progresses: vec![],
            render_target_ids: vec![],
            active_tab_index: 0,
            position: TabBarPosition::Left,
            style: TabBarStyle::Fade,
        };
        let mut state = TabSidebar::default();
        let mut surface = state
            .build(
                &bar,
                size,
                size.cell_size(),
                40,
                TerminalColorTheme::default(),
            )
            .unwrap();
        assert_eq!(surface.tab_index_at_position(60.0, 48.0), Some(0));
        assert_eq!(surface.tab_index_at_position(60.0, 96.0), Some(1));
        assert_eq!(surface.tab_index_at_position(60.0, 20.0), None);
        for _ in 0..3 {
            state.scroll(true);
        }
        surface = state
            .build(
                &bar,
                size,
                size.cell_size(),
                40,
                TerminalColorTheme::default(),
            )
            .unwrap();
        assert_eq!(surface.tab_index_at_position(60.0, 48.0), Some(3));
        bar.active_tab_index = 29;
        surface = state
            .build(
                &bar,
                size,
                size.cell_size(),
                40,
                TerminalColorTheme::default(),
            )
            .unwrap();
        assert!(surface.hit_regions.iter().any(|hit| hit.tab_index == 29));
        for hit in &surface.hit_regions {
            assert!(hit.bottom_px <= surface.placement.height_px);
            assert_eq!(
                surface.tab_index_at_position(60.0, f64::from(hit.top_px + 1)),
                Some(hit.tab_index)
            );
        }
        bar.titles.truncate(1);
        bar.active_tab_index = 0;
        surface = state
            .build(
                &bar,
                size,
                size.cell_size(),
                40,
                TerminalColorTheme::default(),
            )
            .unwrap();
        assert_eq!(surface.tab_index_at_position(60.0, 48.0), Some(0));
    }
}
