use serde::{Deserialize, Serialize};

use super::pane_split_direction::PaneSplitDirection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneResizeDirection {
    Left,
    Right,
    Up,
    Down,
}

impl PaneResizeDirection {
    pub const fn split_direction(self) -> PaneSplitDirection {
        match self {
            Self::Left | Self::Right => PaneSplitDirection::Horizontal,
            Self::Up | Self::Down => PaneSplitDirection::Vertical,
        }
    }

    pub const fn grows_first(self) -> bool {
        matches!(self, Self::Right | Self::Down)
    }
}
