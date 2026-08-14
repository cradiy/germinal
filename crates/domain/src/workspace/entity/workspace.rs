use serde::{Deserialize, Serialize};

use crate::{
    aggregate_root::AggregateRoot,
    workspace::{
        entity::workspace_tab::WorkspaceTab,
        vo::{
            pane_id::PaneId, pane_resize_direction::PaneResizeDirection,
            pane_split_direction::PaneSplitDirection, tab_id::TabId,
        },
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    active_tab_index: usize,
    tabs: Vec<WorkspaceTab>,
}

impl Workspace {
    pub fn new(focused_pane: PaneId) -> Self {
        Self {
            active_tab_index: 0,
            tabs: vec![WorkspaceTab::new(TabId::new(0), focused_pane)],
        }
    }

    pub fn from_tabs(tabs: Vec<WorkspaceTab>, active_tab_index: usize) -> Self {
        assert!(!tabs.is_empty(), "workspace must contain at least one tab");
        assert!(
            active_tab_index < tabs.len(),
            "active tab index must be in range"
        );
        assert!(
            tabs.iter().enumerate().all(|(index, tab)| tabs
                .iter()
                .skip(index + 1)
                .all(|other| other.tab_id() != tab.tab_id())),
            "workspace tab ids must be unique"
        );

        Self {
            active_tab_index,
            tabs,
        }
    }

    pub fn main() -> Self {
        Self::new(PaneId::new(0))
    }

    pub fn two_pane() -> Self {
        let mut workspace = Self::main();
        workspace.split_focused_pane(PaneSplitDirection::Horizontal);
        workspace
    }

    pub fn focused_pane(&self) -> PaneId {
        self.active_tab().focused_pane()
    }

    pub fn active_tab_id(&self) -> TabId {
        self.active_tab().tab_id()
    }

    pub fn create_tab(&mut self) -> TabId {
        let tab_id = TabId::new(
            self.tabs
                .iter()
                .map(|tab| tab.tab_id().value())
                .max()
                .unwrap_or(0)
                + 1,
        );
        self.tabs.push(WorkspaceTab::new(tab_id, PaneId::new(0)));
        self.active_tab_index = self.tabs.len() - 1;
        tab_id
    }

    pub fn activate_next_tab(&mut self) -> TabId {
        self.active_tab_index = (self.active_tab_index + 1) % self.tabs.len();
        self.active_tab_id()
    }

    pub fn activate_previous_tab(&mut self) -> TabId {
        self.active_tab_index = self
            .active_tab_index
            .checked_sub(1)
            .unwrap_or(self.tabs.len() - 1);
        self.active_tab_id()
    }

    pub fn activate_tab(&mut self, tab_id: TabId) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.tab_id() == tab_id) else {
            return false;
        };
        self.active_tab_index = index;
        true
    }

    pub fn tab(&self, tab_id: TabId) -> Option<&WorkspaceTab> {
        self.tabs.iter().find(|tab| tab.tab_id() == tab_id)
    }

    pub fn close_pane_in_tab(&mut self, tab_id: TabId, pane_id: PaneId) -> bool {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.tab_id() == tab_id) else {
            return false;
        };
        tab.close_pane(pane_id)
    }

    pub fn close_tab(&mut self, tab_id: TabId) -> Option<WorkspaceTab> {
        if self.tabs.len() == 1 {
            return None;
        }
        let index = self.tabs.iter().position(|tab| tab.tab_id() == tab_id)?;
        let removed = self.tabs.remove(index);

        if index < self.active_tab_index {
            self.active_tab_index -= 1;
        } else if index == self.active_tab_index {
            self.active_tab_index = index.min(self.tabs.len() - 1);
        }

        Some(removed)
    }

    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn set_focused_pane(&mut self, pane_id: PaneId) -> bool {
        self.active_tab_mut().focus_pane(pane_id)
    }

    pub fn focus_next_pane(&mut self) -> PaneId {
        self.active_tab_mut().focus_next_pane()
    }

    pub fn focus_previous_pane(&mut self) -> PaneId {
        self.active_tab_mut().focus_previous_pane()
    }

    pub fn split_focused_pane(&mut self, direction: PaneSplitDirection) -> PaneId {
        self.active_tab_mut().split_focused_pane(direction)
    }

    pub fn swap_focused_pane_with(&mut self, other: PaneId) -> bool {
        self.active_tab_mut().swap_focused_pane_with(other)
    }

    pub fn resize_focused_pane(&mut self, direction: PaneResizeDirection) -> bool {
        self.active_tab_mut().resize_focused_pane(direction)
    }

    pub fn close_pane(&mut self, pane_id: PaneId) -> bool {
        self.active_tab_mut().close_pane(pane_id)
    }

    pub const fn active_tab_index(&self) -> usize {
        self.active_tab_index
    }

    pub fn active_tab(&self) -> &WorkspaceTab {
        &self.tabs[self.active_tab_index]
    }

    pub fn tabs(&self) -> &[WorkspaceTab] {
        &self.tabs
    }

    fn active_tab_mut(&mut self) -> &mut WorkspaceTab {
        &mut self.tabs[self.active_tab_index]
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::main()
    }
}

impl AggregateRoot for Workspace {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::vo::{
        pane_id::PaneId, pane_split_direction::PaneSplitDirection, tab_id::TabId,
    };

    #[test]
    fn main_workspace_starts_with_single_focused_pane() {
        let workspace = Workspace::main();

        assert_eq!(workspace.focused_pane(), PaneId::new(0));
        assert_eq!(workspace.active_tab().pane_count(), 1);
    }

    #[test]
    fn split_focused_pane_creates_new_pane_and_focuses_it() {
        let mut workspace = Workspace::main();

        let new_pane = workspace.split_focused_pane(PaneSplitDirection::Horizontal);

        assert_eq!(new_pane, PaneId::new(1));
        assert_eq!(workspace.focused_pane(), new_pane);
        assert_eq!(workspace.active_tab().pane_count(), 2);
        assert!(workspace.active_tab().contains_pane(PaneId::new(0)));
        assert!(workspace.active_tab().contains_pane(PaneId::new(1)));
    }

    #[test]
    fn set_focused_pane_rejects_unknown_pane() {
        let mut workspace = Workspace::main();

        assert!(!workspace.set_focused_pane(PaneId::new(7)));
        assert_eq!(workspace.focused_pane(), PaneId::new(0));
    }

    #[test]
    fn focus_next_pane_cycles_in_tree_order() {
        let mut workspace = Workspace::two_pane();

        assert_eq!(workspace.focused_pane(), PaneId::new(1));
        assert_eq!(workspace.focus_next_pane(), PaneId::new(0));
        assert_eq!(workspace.focus_next_pane(), PaneId::new(1));
    }

    #[test]
    fn focus_previous_pane_cycles_in_reverse_tree_order() {
        let mut workspace = Workspace::two_pane();

        assert_eq!(workspace.focus_previous_pane(), PaneId::new(0));
        assert_eq!(workspace.focus_previous_pane(), PaneId::new(1));
    }

    #[test]
    fn swapping_focused_pane_keeps_focus_on_the_moved_pane() {
        let mut workspace = Workspace::two_pane();

        assert!(workspace.swap_focused_pane_with(PaneId::new(0)));
        assert_eq!(
            workspace.active_tab().pane_tree().pane_ids(),
            vec![PaneId::new(1), PaneId::new(0),]
        );
        assert_eq!(workspace.focused_pane(), PaneId::new(1));
    }

    #[test]
    fn closing_the_focused_pane_focuses_its_remaining_neighbor() {
        let mut workspace = Workspace::two_pane();

        assert!(workspace.close_pane(PaneId::new(1)));
        assert_eq!(workspace.focused_pane(), PaneId::new(0));
        assert_eq!(workspace.active_tab().pane_count(), 1);
    }

    #[test]
    fn closing_an_unfocused_pane_preserves_focus() {
        let mut workspace = Workspace::two_pane();

        assert!(workspace.close_pane(PaneId::new(0)));
        assert_eq!(workspace.focused_pane(), PaneId::new(1));
        assert_eq!(workspace.active_tab().pane_count(), 1);
    }

    #[test]
    fn closing_the_last_pane_is_rejected() {
        let mut workspace = Workspace::main();

        assert!(!workspace.close_pane(PaneId::new(0)));
        assert_eq!(workspace.focused_pane(), PaneId::new(0));
    }

    #[test]
    fn tabs_cycle_and_restore_their_own_focused_pane() {
        let mut workspace = Workspace::two_pane();
        let first_tab = workspace.active_tab_id();
        let first_focus = workspace.focused_pane();

        let second_tab = workspace.create_tab();
        assert_ne!(second_tab, first_tab);
        assert_eq!(workspace.tab_count(), 2);
        assert_eq!(workspace.focused_pane(), PaneId::new(0));

        assert_eq!(workspace.activate_previous_tab(), first_tab);
        assert_eq!(workspace.focused_pane(), first_focus);
        assert_eq!(workspace.activate_next_tab(), second_tab);
    }

    #[test]
    fn closing_an_active_tab_focuses_the_tab_that_takes_its_position() {
        let mut workspace = Workspace::main();
        let first = workspace.active_tab_id();
        let second = workspace.create_tab();
        let third = workspace.create_tab();

        assert!(workspace.activate_tab(second));
        assert_eq!(
            workspace.close_tab(second).map(|tab| tab.tab_id()),
            Some(second)
        );
        assert_eq!(workspace.active_tab_id(), third);
        assert_eq!(workspace.tab_count(), 2);

        assert_eq!(
            workspace.close_tab(first).map(|tab| tab.tab_id()),
            Some(first)
        );
        assert_eq!(workspace.active_tab_id(), third);
        assert!(workspace.close_tab(third).is_none());
    }

    #[test]
    #[should_panic(expected = "workspace tab ids must be unique")]
    fn restored_workspace_rejects_duplicate_tab_ids() {
        Workspace::from_tabs(
            vec![
                WorkspaceTab::new(TabId::new(7), PaneId::new(0)),
                WorkspaceTab::new(TabId::new(7), PaneId::new(0)),
            ],
            0,
        );
    }
}
