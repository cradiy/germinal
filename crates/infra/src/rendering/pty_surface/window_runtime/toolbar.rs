use super::*;
use crate::rendering::pty_surface::svg_icon::SvgIconCache;
use germinal_ports::rendering::toolbar::{
    TOOLBAR_TARGET_ID, ToolbarAction, ToolbarConfig, toolbar_buttons, toolbar_height,
};

#[derive(Default)]
pub(super) struct Toolbar {
    pub config: Option<ToolbarConfig>,
    pub hovered: Option<ToolbarAction>,
    icons: SvgIconCache,
    generation: u64,
}

impl Toolbar {
    pub(super) fn build(
        &mut self,
        size: TerminalSizeInfo,
        theme: TerminalColorTheme,
        scale: TerminalScaleFactor,
        maximized: bool,
    ) -> Option<WgpuTabBarSurface> {
        let config = self.config?;
        let height = toolbar_height(scale, size.window_size().height_px());
        if height == 0 {
            return None;
        }
        let width = size.window_size().width_px();
        let background = theme.background;
        let sidebar_width = if config.sidebar {
            germinal_ports::rendering::tab_bar::sidebar_width_px(
                width,
                size.cell_size().height_px(),
            )
        } else {
            0
        };
        let sidebar_background = mix_rgb(background, theme.foreground, 3, 100);
        let mut images = Vec::new();
        let mut runs = Vec::new();
        let mut sidebar_hover = None;
        for button in toolbar_buttons(config, width, height, size.cell_size().height_px(), scale) {
            let button_background = if button.x < sidebar_width {
                sidebar_background
            } else {
                background
            };
            let muted = mix_rgb(button_background, theme.foreground, 65, 100);
            let (id, svg) = match button.action {
                ToolbarAction::CommandPalette => {
                    ("commands", include_str!("../icons/terminal.svg"))
                }
                ToolbarAction::NewTab => ("new-tab", include_str!("../icons/plus.svg")),
                ToolbarAction::ToggleSidebar => ("sidebar", include_str!("../icons/sidebar.svg")),
                ToolbarAction::Minimize => ("minimize", include_str!("../icons/minimize.svg")),
                ToolbarAction::Maximize if maximized => {
                    ("restore", include_str!("../icons/restore.svg"))
                }
                ToolbarAction::Maximize => ("maximize", include_str!("../icons/maximize.svg")),
                ToolbarAction::Close => ("close", include_str!("../icons/close.svg")),
            };
            let hovered = self.hovered == Some(button.action);
            let linux_control = cfg!(target_os = "linux")
                && matches!(
                    button.action,
                    ToolbarAction::Minimize | ToolbarAction::Maximize | ToolbarAction::Close
                );
            if linux_control {
                let fill = mix_rgb(
                    button_background,
                    RgbColorDto::new(255, 255, 255),
                    if hovered { 34 } else { 14 },
                    255,
                );
                if let Ok(circle) = self.icons.rasterize(
                    include_str!("../icons/circle.svg"),
                    button.width.clamp(1, 512),
                    Some(fill),
                ) {
                    images.push(circle.placement(&format!("{id}-background"), button.x, button.y));
                }
            } else if hovered {
                if button.x < sidebar_width {
                    sidebar_hover = Some((button.x, (button.x + button.width).min(sidebar_width)));
                }
                runs.push(RenderSurfaceRunSnapshot {
                    x: button.x,
                    text: " ".repeat(button.width as usize),
                    style: TextStyleDto {
                        background: Some(if button.action == ToolbarAction::Close {
                            RgbColorDto::new(150, 45, 60)
                        } else {
                            mix_rgb(button_background, theme.foreground, 8, 100)
                        }),
                        ..TextStyleDto::plain()
                    },
                    decoration: Default::default(),
                });
            }
            let icon_size = scale
                .physical_px_from_logical_px(if linux_control { 12 } else { 16 })
                .clamp(1, 512)
                .min(button.width)
                .min(button.height);
            let color = if hovered || linux_control {
                theme.foreground
            } else {
                muted
            };
            if let Ok(icon) = self.icons.rasterize(svg, icon_size, Some(color)) {
                images.push(icon.placement(
                    id,
                    button.x + (button.width - icon_size) / 2,
                    button.y + (button.height - icon_size) / 2,
                ));
            }
        }
        let (hover_start, hover_end) = sidebar_hover.unwrap_or((sidebar_width, sidebar_width));
        for (start, end) in [(0, hover_start), (hover_end, sidebar_width)] {
            if start < end {
                runs.push(RenderSurfaceRunSnapshot {
                    x: start,
                    text: " ".repeat((end - start) as usize),
                    style: TextStyleDto {
                        background: Some(sidebar_background),
                        ..TextStyleDto::plain()
                    },
                    decoration: Default::default(),
                });
            }
        }
        runs.sort_by_key(|run| run.x);
        let rows = if runs.is_empty() {
            Vec::new()
        } else {
            vec![RenderSurfaceRowSnapshot { y: 0, runs }]
        };
        self.generation = self.generation.wrapping_add(1);
        Some(WgpuTabBarSurface {
            placement: RenderSurfacePlacement::new(TOOLBAR_TARGET_ID, 0, 0, width, height),
            snapshot: RenderSurfaceSnapshot {
                target_id: TOOLBAR_TARGET_ID,
                latest_seq: Seq::new(self.generation),
                default_background: background,
                rows,
                image_surfaces: images,
                dirty_rows: Vec::new(),
                scrollback: None,
                cursor: None,
                ime_preedit: None,
            },
            cell_width_px: 1,
            hit_regions: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_changes_preserve_toolbar_geometry_and_dpi_rescales_icons() {
        let mut toolbar = Toolbar {
            config: Some(ToolbarConfig {
                sidebar: false,
                window_controls: true,
            }),
            ..Default::default()
        };
        let mut reference = None;
        for cell_height in [12, 24, 48] {
            let size = TerminalSizeInfo::zero_padding(
                TerminalWindowSize::new(1000, 600),
                TerminalCellSize::new(cell_height / 2, cell_height),
            );
            let surface = toolbar
                .build(
                    size,
                    TerminalColorTheme::default(),
                    TerminalScaleFactor::DEFAULT,
                    false,
                )
                .unwrap();
            let geometry = (
                surface.placement.height_px,
                surface
                    .snapshot
                    .image_surfaces
                    .iter()
                    .map(|image| {
                        (
                            image.x_offset_px,
                            image.y_offset_px,
                            image.image_width_px,
                            image.image_height_px,
                        )
                    })
                    .collect::<Vec<_>>(),
            );
            if let Some(reference) = &reference {
                assert_eq!(reference, &geometry);
            } else {
                reference = Some(geometry);
            }
            let scaled = toolbar
                .build(
                    size,
                    TerminalColorTheme::default(),
                    TerminalScaleFactor::new(2.0),
                    false,
                )
                .unwrap();
            assert_eq!(scaled.placement.height_px, surface.placement.height_px * 2);
            for (normal, scaled) in surface
                .snapshot
                .image_surfaces
                .iter()
                .zip(&scaled.snapshot.image_surfaces)
            {
                assert_eq!(scaled.image_width_px, normal.image_width_px * 2);
                assert_eq!(scaled.image_height_px, normal.image_height_px * 2);
            }
        }
    }
}
