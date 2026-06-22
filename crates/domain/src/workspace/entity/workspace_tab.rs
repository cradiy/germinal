use serde::{Deserialize, Serialize};

use crate::{
	gshell::vo::gshell_id::GShellId,
	workspace::{entity::pane_tree::PaneTree, vo::pane_split_direction::PaneSplitDirection},
};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceTab {
	focused_gshell: GShellId,
	next_gshell_id: u64,
	pane_tree:      PaneTree,
}

impl WorkspaceTab {
	pub fn new(initial_gshell: GShellId) -> Self {
		Self {
			focused_gshell: initial_gshell,
			next_gshell_id: initial_gshell.value() + 1,
			pane_tree:      PaneTree::single(initial_gshell),
		}
	}

	pub const fn focused_gshell(&self) -> GShellId { self.focused_gshell }

	pub const fn next_gshell_id(&self) -> u64 { self.next_gshell_id }

	pub fn pane_tree(&self) -> &PaneTree { &self.pane_tree }

	pub fn pane_count(&self) -> usize { self.pane_tree.pane_count() }

	pub fn contains_gshell(&self, gshell_id: GShellId) -> bool {
		self.pane_tree.contains_gshell(gshell_id)
	}

	pub fn focus_gshell(&mut self, gshell_id: GShellId) -> bool {
		if !self.contains_gshell(gshell_id) {
			return false;
		}

		self.focused_gshell = gshell_id;
		true
	}

	pub fn split_focused_pane(&mut self, direction: PaneSplitDirection) -> GShellId {
		let new_gshell_id = GShellId::new(self.next_gshell_id);
		self.next_gshell_id += 1;

		let split = self.pane_tree.split_pane(self.focused_gshell, direction, new_gshell_id);
		debug_assert!(split, "focused pane must exist in pane tree");

		self.focused_gshell = new_gshell_id;
		new_gshell_id
	}
}
