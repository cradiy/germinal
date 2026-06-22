use serde::{Deserialize, Serialize};

use crate::{
	gshell::vo::gshell_id::GShellId, workspace::vo::pane_split_direction::PaneSplitDirection,
};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneTree {
	Pane(GShellId),
	Split { direction: PaneSplitDirection, first: Box<PaneTree>, second: Box<PaneTree> },
}

impl PaneTree {
	pub const fn single(gshell_id: GShellId) -> Self { Self::Pane(gshell_id) }

	pub fn contains_gshell(&self, gshell_id: GShellId) -> bool {
		match self {
			Self::Pane(id) => *id == gshell_id,
			Self::Split { first, second, .. } => {
				first.contains_gshell(gshell_id) || second.contains_gshell(gshell_id)
			}
		}
	}

	pub fn pane_count(&self) -> usize {
		match self {
			Self::Pane(_) => 1,
			Self::Split { first, second, .. } => first.pane_count() + second.pane_count(),
		}
	}

	pub fn split_pane(
		&mut self,
		target: GShellId,
		direction: PaneSplitDirection,
		new_gshell_id: GShellId,
	) -> bool {
		match self {
			Self::Pane(existing) if *existing == target => {
				*self = Self::Split {
					direction,
					first: Box::new(Self::Pane(target)),
					second: Box::new(Self::Pane(new_gshell_id)),
				};
				true
			}
			Self::Pane(_) => false,
			Self::Split { first, second, .. } => {
				first.split_pane(target, direction, new_gshell_id)
					|| second.split_pane(target, direction, new_gshell_id)
			}
		}
	}
}
