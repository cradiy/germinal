use super::render_target_id::RenderTargetId;
use crate::pty_host::scale_factor::TerminalScaleFactor;

pub const TOOLBAR_TARGET_ID: RenderTargetId = RenderTargetId::new(u64::MAX - 3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolbarConfig {
    pub modern: bool,
    pub inline_tabs: bool,
    pub sidebar: bool,
    pub window_controls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarAction {
    CommandPalette,
    NewTab,
    ToggleSidebar,
    Minimize,
    Maximize,
    Close,
}

#[derive(Debug, Clone, Copy)]
pub struct ToolbarButton {
    pub action: ToolbarAction,
    pub x: u32,
    pub width: u32,
    pub y: u32,
    pub height: u32,
}

pub fn toolbar_height(scale: TerminalScaleFactor, window_height: u32) -> u32 {
    scale
        .physical_px_from_logical_px(40)
        .min(window_height.saturating_sub(1))
}

pub fn toolbar_buttons(
    config: ToolbarConfig,
    width: u32,
    height: u32,
    sidebar: u32,
    scale: TerminalScaleFactor,
) -> Vec<ToolbarButton> {
    if height == 0 {
        return Vec::new();
    }
    let mut buttons = Vec::new();
    let linux = cfg!(target_os = "linux");
    let control_size = if linux {
        scale.physical_px_from_logical_px(24).min(height)
    } else {
        height
    };
    let gap = if linux {
        scale.physical_px_from_logical_px(6)
    } else {
        0
    };
    let padding = if linux {
        scale.physical_px_from_logical_px(10)
    } else {
        0
    };
    let right = if config.window_controls {
        width.saturating_sub(control_size.saturating_mul(3) + gap * 2 + padding * 2)
    } else {
        width
    };
    if !config.inline_tabs && height <= right {
        buttons.push(ToolbarButton {
            action: ToolbarAction::CommandPalette,
            x: 0,
            width: height,
            y: 0,
            height,
        });
    }
    if config.modern {
        for (index, action) in [ToolbarAction::NewTab, ToolbarAction::ToggleSidebar]
            .into_iter()
            .enumerate()
        {
            let x = if config.sidebar {
                sidebar.saturating_sub(height.saturating_mul(2 - index as u32))
            } else {
                height.saturating_mul(index as u32 + 1)
            };
            if x.saturating_add(height) <= right
                && buttons
                    .iter()
                    .all(|button| x >= button.x + button.width || x + height <= button.x)
            {
                buttons.push(ToolbarButton {
                    action,
                    x,
                    width: height,
                    y: 0,
                    height,
                });
            }
        }
    }
    if config.window_controls {
        for (index, action) in [
            ToolbarAction::Close,
            ToolbarAction::Maximize,
            ToolbarAction::Minimize,
        ]
        .into_iter()
        .enumerate()
        {
            let used = padding + control_size.saturating_mul(index as u32 + 1) + gap * index as u32;
            if used <= width {
                buttons.push(ToolbarButton {
                    action,
                    x: width - used,
                    width: control_size,
                    y: (height - control_size) / 2,
                    height: control_size,
                });
            }
        }
    }
    buttons
}

/// Tab viewport within the caption, with inset edges and a permanent drag gap.
pub fn inline_tab_bounds(
    config: ToolbarConfig,
    width: u32,
    height: u32,
    scale: TerminalScaleFactor,
) -> (u32, u32, u32, u32) {
    let inset = scale.physical_px_from_logical_px(8);
    let vertical = scale.physical_px_from_logical_px(4).min(height / 2);
    let right = toolbar_buttons(config, width, height, 0, scale)
        .iter()
        .filter(|button| {
            matches!(
                button.action,
                ToolbarAction::Minimize | ToolbarAction::Maximize | ToolbarAction::Close
            )
        })
        .map(|button| button.x)
        .min()
        .unwrap_or(width);
    let end = right.saturating_sub(scale.physical_px_from_logical_px(48));
    (
        inset.min(width),
        vertical,
        end.saturating_sub(inset),
        height.saturating_sub(vertical * 2),
    )
}

pub fn toolbar_hit(
    buttons: &[ToolbarButton],
    height: u32,
    x: f64,
    y: f64,
) -> Option<ToolbarAction> {
    if !x.is_finite() || !y.is_finite() || y < 0.0 || y >= f64::from(height) {
        return None;
    }
    buttons
        .iter()
        .find(|b| {
            x >= f64::from(b.x)
                && x < f64::from(b.x + b.width)
                && y >= f64::from(b.y)
                && y < f64::from(b.y + b.height)
        })
        .map(|b| b.action)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hidden_controls_have_no_hit_targets_and_narrow_windows_never_overlap() {
        for width in [1, 30, 80, 300, 1200] {
            for (modern, sidebar, inline_tabs) in [
                (false, false, false),
                (false, false, true),
                (true, true, false),
                (true, false, false),
            ] {
                for window_controls in [false, true] {
                    let buttons = toolbar_buttons(
                        ToolbarConfig {
                            modern,
                            inline_tabs,
                            sidebar,
                            window_controls,
                        },
                        width,
                        30,
                        super::super::tab_bar::sidebar_width_px(width, 24),
                        TerminalScaleFactor::DEFAULT,
                    );
                    if width == 1200 {
                        for action in [ToolbarAction::NewTab, ToolbarAction::ToggleSidebar] {
                            assert_eq!(
                                buttons.iter().any(|button| button.action == action),
                                modern
                            );
                        }
                    }
                    if inline_tabs {
                        let (x, y, w, h) = inline_tab_bounds(
                            ToolbarConfig {
                                modern,
                                inline_tabs,
                                sidebar,
                                window_controls,
                            },
                            width,
                            30,
                            TerminalScaleFactor::DEFAULT,
                        );
                        assert!(x + w <= width && y + h <= 30);
                        for button in &buttons {
                            assert!(w == 0 || x + w < button.x);
                        }
                    }
                    for (i, b) in buttons.iter().enumerate() {
                        assert!(b.x + b.width <= width);
                        assert_eq!(
                            toolbar_hit(&buttons, 30, f64::from(b.x), 15.0),
                            Some(b.action)
                        );
                        assert_eq!(toolbar_hit(&buttons, 30, f64::from(b.x), 30.0), None);
                        for other in &buttons[i + 1..] {
                            assert!(b.x + b.width <= other.x || other.x + other.width <= b.x);
                        }
                        if !window_controls {
                            assert!(!matches!(
                                b.action,
                                ToolbarAction::Minimize
                                    | ToolbarAction::Maximize
                                    | ToolbarAction::Close
                            ));
                        }
                    }
                }
            }
        }
    }
}
