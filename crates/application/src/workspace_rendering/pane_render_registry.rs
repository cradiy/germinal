use std::collections::HashMap;

use germinal_domain::{rendering::render_target_id::RenderTargetId, workspace::pane_id::PaneId};

#[derive(Debug, Default)]
pub struct PaneRenderRegistry {
	pane_to_target:           HashMap<PaneId, RenderTargetId>,
	next_render_target_value: u64,
}

impl PaneRenderRegistry {
	pub fn new() -> Self { Self::default() }

	pub fn register_pane(&mut self, pane_id: PaneId) -> RenderTargetId {
		if let Some(target_id) = self.pane_to_target.get(&pane_id) {
			return *target_id;
		}

		let target_id = RenderTargetId::new(self.next_render_target_value);
		self.next_render_target_value += 1;

		self.pane_to_target.insert(pane_id, target_id);

		target_id
	}

	pub fn render_target_of(&self, pane_id: PaneId) -> Option<RenderTargetId> {
		self.pane_to_target.get(&pane_id).copied()
	}

	pub fn ensure_render_target(&mut self, pane_id: PaneId) -> RenderTargetId {
		self.register_pane(pane_id)
	}

	pub fn len(&self) -> usize { self.pane_to_target.len() }

	pub fn is_empty(&self) -> bool { self.pane_to_target.is_empty() }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn register_pane_returns_stable_render_target() {
		let mut registry = PaneRenderRegistry::new();

		let pane_id = PaneId::new(10);

		let first = registry.register_pane(pane_id);
		let second = registry.register_pane(pane_id);

		assert_eq!(first, second);
		assert_eq!(registry.len(), 1);
	}

	#[test]
	fn different_panes_get_different_render_targets() {
		let mut registry = PaneRenderRegistry::new();

		let a = registry.register_pane(PaneId::new(1));
		let b = registry.register_pane(PaneId::new(2));

		assert_ne!(a, b);
		assert_eq!(registry.len(), 2);
	}

	#[test]
	fn unknown_pane_has_no_render_target() {
		let registry = PaneRenderRegistry::new();

		assert_eq!(registry.render_target_of(PaneId::new(1)), None);
	}
}
