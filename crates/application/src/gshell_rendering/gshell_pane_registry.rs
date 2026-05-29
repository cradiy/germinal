use std::collections::HashMap;

use germinal_domain::{gshell::gshell_id::GShellId, workspace::pane_id::PaneId};

#[derive(Debug, Default)]
pub struct GShellPaneRegistry {
	gshell_to_pane: HashMap<GShellId, PaneId>,
}

impl GShellPaneRegistry {
	pub fn new() -> Self { Self::default() }

	pub fn bind(&mut self, gshell_id: GShellId, pane_id: PaneId) {
		self.gshell_to_pane.insert(gshell_id, pane_id);
	}

	pub fn pane_of(&self, gshell_id: GShellId) -> Option<PaneId> {
		self.gshell_to_pane.get(&gshell_id).copied()
	}

	pub fn unbind(&mut self, gshell_id: GShellId) -> Option<PaneId> {
		self.gshell_to_pane.remove(&gshell_id)
	}

	pub fn len(&self) -> usize { self.gshell_to_pane.len() }

	pub fn is_empty(&self) -> bool { self.gshell_to_pane.is_empty() }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn bind_gshell_to_pane() {
		let mut registry = GShellPaneRegistry::new();

		let gshell_id = GShellId::new(1);
		let pane_id = PaneId::new(10);

		registry.bind(gshell_id, pane_id);

		assert_eq!(registry.pane_of(gshell_id), Some(pane_id));
		assert_eq!(registry.len(), 1);
	}

	#[test]
	fn rebind_gshell_to_new_pane() {
		let mut registry = GShellPaneRegistry::new();

		let gshell_id = GShellId::new(1);
		let pane_a = PaneId::new(10);
		let pane_b = PaneId::new(20);

		registry.bind(gshell_id, pane_a);
		registry.bind(gshell_id, pane_b);

		assert_eq!(registry.pane_of(gshell_id), Some(pane_b));
		assert_eq!(registry.len(), 1);
	}

	#[test]
	fn unbind_gshell() {
		let mut registry = GShellPaneRegistry::new();

		let gshell_id = GShellId::new(1);
		let pane_id = PaneId::new(10);

		registry.bind(gshell_id, pane_id);

		assert_eq!(registry.unbind(gshell_id), Some(pane_id));
		assert_eq!(registry.pane_of(gshell_id), None);
	}
}
