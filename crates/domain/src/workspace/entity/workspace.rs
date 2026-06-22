use serde::{Deserialize, Serialize};

use crate::{
	aggregate_root::AggregateRoot,
	gshell::vo::gshell_id::GShellId,
	workspace::{entity::workspace_tab::WorkspaceTab, vo::pane_split_direction::PaneSplitDirection},
};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
	active_tab_index: usize,
	tabs:             Vec<WorkspaceTab>,
}

impl Workspace {
	pub fn new(focused_gshell: GShellId) -> Self {
		Self { active_tab_index: 0, tabs: vec![WorkspaceTab::new(focused_gshell)] }
	}

	pub fn from_tabs(tabs: Vec<WorkspaceTab>, active_tab_index: usize) -> Self {
		assert!(!tabs.is_empty(), "workspace must contain at least one tab");
		assert!(active_tab_index < tabs.len(), "active tab index must be in range");

		Self { active_tab_index, tabs }
	}

	pub fn main() -> Self { Self::new(GShellId::new(0)) }

	pub fn focused_gshell(&self) -> GShellId { self.active_tab().focused_gshell() }

	pub fn set_focused_gshell(&mut self, gshell_id: GShellId) -> bool {
		self.active_tab_mut().focus_gshell(gshell_id)
	}

	pub fn split_focused_pane(&mut self, direction: PaneSplitDirection) -> GShellId {
		self.active_tab_mut().split_focused_pane(direction)
	}

	pub const fn active_tab_index(&self) -> usize { self.active_tab_index }

	pub fn active_tab(&self) -> &WorkspaceTab { &self.tabs[self.active_tab_index] }

	pub fn tabs(&self) -> &[WorkspaceTab] { &self.tabs }

	fn active_tab_mut(&mut self) -> &mut WorkspaceTab { &mut self.tabs[self.active_tab_index] }
}

impl Default for Workspace {
	fn default() -> Self { Self::main() }
}

impl AggregateRoot for Workspace {}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		gshell::vo::gshell_id::GShellId, workspace::vo::pane_split_direction::PaneSplitDirection,
	};

	#[test]
	fn main_workspace_starts_with_single_focused_pane() {
		let workspace = Workspace::main();

		assert_eq!(workspace.focused_gshell(), GShellId::new(0));
		assert_eq!(workspace.active_tab().pane_count(), 1);
	}

	#[test]
	fn split_focused_pane_creates_new_pane_and_focuses_it() {
		let mut workspace = Workspace::main();

		let new_pane = workspace.split_focused_pane(PaneSplitDirection::Horizontal);

		assert_eq!(new_pane, GShellId::new(1));
		assert_eq!(workspace.focused_gshell(), new_pane);
		assert_eq!(workspace.active_tab().pane_count(), 2);
		assert!(workspace.active_tab().contains_gshell(GShellId::new(0)));
		assert!(workspace.active_tab().contains_gshell(GShellId::new(1)));
	}

	#[test]
	fn set_focused_pane_rejects_unknown_pane() {
		let mut workspace = Workspace::main();

		assert!(!workspace.set_focused_gshell(GShellId::new(7)));
		assert_eq!(workspace.focused_gshell(), GShellId::new(0));
	}
}
