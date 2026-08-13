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
