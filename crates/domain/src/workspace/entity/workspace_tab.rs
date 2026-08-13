use serde::{Deserialize, Serialize};

use crate::workspace::{
	entity::pane_tree::PaneTree,
	vo::{pane_id::PaneId, pane_split_direction::PaneSplitDirection},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTab {
	focused_pane: PaneId,
	next_pane_id: u64,
	pane_tree:    PaneTree,
}

impl WorkspaceTab {
	pub fn new(initial_pane: PaneId) -> Self {
		Self {
			focused_pane: initial_pane,
			next_pane_id: initial_pane.value() + 1,
			pane_tree:    PaneTree::single(initial_pane),
		}
	}

	pub const fn focused_pane(&self) -> PaneId { self.focused_pane }

	pub const fn next_pane_id(&self) -> u64 { self.next_pane_id }

	pub fn pane_tree(&self) -> &PaneTree { &self.pane_tree }

	pub fn pane_count(&self) -> usize { self.pane_tree.pane_count() }

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

	pub fn split_focused_pane(&mut self, direction: PaneSplitDirection) -> PaneId {
		let new_pane_id = PaneId::new(self.next_pane_id);
		self.next_pane_id += 1;

		let split = self.pane_tree.split_pane(self.focused_pane, direction, new_pane_id);
		debug_assert!(split, "focused pane must exist in pane tree");

		self.focused_pane = new_pane_id;
		new_pane_id
	}
}
