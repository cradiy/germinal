use serde::{Deserialize, Serialize};

use crate::workspace::vo::{pane_id::PaneId, pane_split_direction::PaneSplitDirection};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneTree {
	Pane(PaneId),
	Split { direction: PaneSplitDirection, first: Box<PaneTree>, second: Box<PaneTree> },
}

impl PaneTree {
	pub const fn single(pane_id: PaneId) -> Self { Self::Pane(pane_id) }

	pub fn contains_pane(&self, pane_id: PaneId) -> bool {
		match self {
			Self::Pane(id) => *id == pane_id,
			Self::Split { first, second, .. } => {
				first.contains_pane(pane_id) || second.contains_pane(pane_id)
			}
		}
	}

	pub fn pane_ids(&self) -> Vec<PaneId> {
		let mut pane_ids = Vec::with_capacity(self.pane_count());
		self.collect_pane_ids(&mut pane_ids);
		pane_ids
	}

	pub fn pane_count(&self) -> usize {
		match self {
			Self::Pane(_) => 1,
			Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
		}
	}

	pub fn split_pane(
		&mut self,
		target: PaneId,
		direction: PaneSplitDirection,
		new_pane_id: PaneId,
	) -> bool {
		match self {
			Self::Pane(existing) if *existing == target => {
				*self = Self::Split {
					direction,
					first: Box::new(Self::Pane(target)),
					second: Box::new(Self::Pane(new_pane_id)),
				};
				true
			}
			Self::Pane(_) => false,
			Self::Split { first, second, .. } => {
				first.split_pane(target, direction, new_pane_id)
					|| second.split_pane(target, direction, new_pane_id)
			}
		}
	}

	pub fn remove_pane(&mut self, target: PaneId) -> bool {
		if self.pane_count() == 1 || !self.contains_pane(target) {
			return false;
		}

		*self = remove_pane_from(self.clone(), target)
			.expect("removing one pane from a multi-pane tree must leave a pane tree");
		true
	}

	fn collect_pane_ids(&self, pane_ids: &mut Vec<PaneId>) {
		match self {
			Self::Pane(pane_id) => pane_ids.push(*pane_id),
			Self::Split { first, second, .. } => {
				first.collect_pane_ids(pane_ids);
				second.collect_pane_ids(pane_ids);
			}
		}
	}
}

fn remove_pane_from(tree: PaneTree, target: PaneId) -> Option<PaneTree> {
	match tree {
		PaneTree::Pane(pane_id) => (pane_id != target).then_some(PaneTree::Pane(pane_id)),
		PaneTree::Split { direction, first, second } => {
			let first = remove_pane_from(*first, target);
			let second = remove_pane_from(*second, target);

			match (first, second) {
				(Some(first), Some(second)) => Some(PaneTree::Split {
					direction,
					first: Box::new(first),
					second: Box::new(second),
				}),
				(Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
				(None, None) => None,
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn removing_a_nested_pane_collapses_its_parent_split() {
		let first = PaneId::new(0);
		let second = PaneId::new(1);
		let third = PaneId::new(2);
		let mut tree = PaneTree::single(first);
		assert!(tree.split_pane(first, PaneSplitDirection::Horizontal, second));
		assert!(tree.split_pane(second, PaneSplitDirection::Vertical, third));

		assert!(tree.remove_pane(second));

		assert_eq!(tree.pane_ids(), vec![first, third]);
		assert_eq!(tree.pane_count(), 2);
		assert!(!tree.contains_pane(second));
	}

	#[test]
	fn removing_the_only_or_an_unknown_pane_is_rejected() {
		let only = PaneId::new(0);
		let mut tree = PaneTree::single(only);

		assert!(!tree.remove_pane(only));
		assert!(!tree.remove_pane(PaneId::new(7)));
		assert_eq!(tree, PaneTree::single(only));
	}
}
