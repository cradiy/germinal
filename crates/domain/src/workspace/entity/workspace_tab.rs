use serde::{Deserialize, Serialize};

use crate::workspace::{
    entity::pane_tree::PaneTree,
    vo::{
        pane_id::PaneId, pane_resize_direction::PaneResizeDirection,
        pane_split_direction::PaneSplitDirection, tab_id::TabId,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTab {
    tab_id: TabId,
    focused_pane: PaneId,
    next_pane_id: u64,
    pane_tree: PaneTree,
}

impl WorkspaceTab {
    pub fn new(tab_id: TabId, initial_pane: PaneId) -> Self {
        Self {
            tab_id,
            focused_pane: initial_pane,
            next_pane_id: initial_pane.value() + 1,
            pane_tree: PaneTree::single(initial_pane),
        }
    }

    pub const fn tab_id(&self) -> TabId {
        self.tab_id
    }

    pub const fn focused_pane(&self) -> PaneId {
        self.focused_pane
    }

    pub const fn next_pane_id(&self) -> u64 {
        self.next_pane_id
    }

    pub fn pane_tree(&self) -> &PaneTree {
        &self.pane_tree
    }

    pub fn pane_count(&self) -> usize {
        self.pane_tree.pane_count()
    }

    pub fn contains_pane(&self, pane_id: PaneId) -> bool {
        self.pane_tree.contains_pane(pane_id)
    }

    pub fn focus_pane(&mut self, pane_id: PaneId) -> bool {
        if !self.contains_pane(pane_id) {
            return false;
        }

        self.focused_pane = pane_id;
        true
    }

    pub fn focus_next_pane(&mut self) -> PaneId {
        let pane_ids = self.pane_tree.pane_ids();
        let current_index = pane_ids
            .iter()
            .position(|pane_id| *pane_id == self.focused_pane)
            .expect("focused pane must exist in pane tree");
        let next_pane = pane_ids[(current_index + 1) % pane_ids.len()];
        self.focused_pane = next_pane;
        next_pane
    }

    pub fn focus_previous_pane(&mut self) -> PaneId {
        let pane_ids = self.pane_tree.pane_ids();
        let current_index = pane_ids
            .iter()
            .position(|pane_id| *pane_id == self.focused_pane)
            .expect("focused pane must exist in pane tree");
        let previous_index = current_index.checked_sub(1).unwrap_or(pane_ids.len() - 1);
        let previous_pane = pane_ids[previous_index];
        self.focused_pane = previous_pane;
        previous_pane
    }

    pub fn split_focused_pane(&mut self, direction: PaneSplitDirection) -> PaneId {
        let new_pane_id = PaneId::new(self.next_pane_id);
        self.next_pane_id += 1;

        let split = self
            .pane_tree
            .split_pane(self.focused_pane, direction, new_pane_id);
        debug_assert!(split, "focused pane must exist in pane tree");

        self.focused_pane = new_pane_id;
        new_pane_id
    }

    pub fn swap_focused_pane_with(&mut self, other: PaneId) -> bool {
        self.pane_tree.swap_panes(self.focused_pane, other)
    }

    pub fn resize_focused_pane(&mut self, direction: PaneResizeDirection) -> bool {
        self.pane_tree.resize_pane(self.focused_pane, direction)
    }

    pub fn close_pane(&mut self, pane_id: PaneId) -> bool {
        let pane_ids = self.pane_tree.pane_ids();
        let Some(closed_index) = pane_ids.iter().position(|id| *id == pane_id) else {
            return false;
        };

        if !self.pane_tree.remove_pane(pane_id) {
            return false;
        }

        if self.focused_pane == pane_id {
            let remaining = self.pane_tree.pane_ids();
            self.focused_pane = remaining[closed_index.min(remaining.len() - 1)];
        }

        true
    }
}
