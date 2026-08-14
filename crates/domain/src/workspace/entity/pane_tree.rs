use serde::{Deserialize, Serialize};

use crate::workspace::vo::{
    pane_id::PaneId, pane_resize_direction::PaneResizeDirection,
    pane_split_direction::PaneSplitDirection,
};

pub const SPLIT_RATIO_SCALE: u16 = 1_000;
pub const DEFAULT_SPLIT_RATIO: u16 = SPLIT_RATIO_SCALE / 2;
pub const MIN_SPLIT_RATIO: u16 = SPLIT_RATIO_SCALE / 10;
pub const MAX_SPLIT_RATIO: u16 = SPLIT_RATIO_SCALE - MIN_SPLIT_RATIO;
pub const SPLIT_RESIZE_STEP: u16 = SPLIT_RATIO_SCALE / 20;

const fn default_split_ratio() -> u16 {
    DEFAULT_SPLIT_RATIO
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneTree {
    Pane(PaneId),
    Split {
        direction: PaneSplitDirection,
        #[serde(default = "default_split_ratio")]
        ratio: u16,
        first: Box<PaneTree>,
        second: Box<PaneTree>,
    },
}

impl PaneTree {
    pub const fn single(pane_id: PaneId) -> Self {
        Self::Pane(pane_id)
    }

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
                    ratio: DEFAULT_SPLIT_RATIO,
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

    pub fn swap_panes(&mut self, first: PaneId, second: PaneId) -> bool {
        if first == second || !self.contains_pane(first) || !self.contains_pane(second) {
            return false;
        }

        self.swap_pane_ids(first, second);
        true
    }

    pub fn resize_pane(&mut self, target: PaneId, direction: PaneResizeDirection) -> bool {
        let Self::Split {
            direction: split_direction,
            ratio,
            first,
            second,
        } = self
        else {
            return false;
        };

        let target_branch = if first.contains_pane(target) {
            first
        } else if second.contains_pane(target) {
            second
        } else {
            return false;
        };

        if target_branch.resize_pane(target, direction) {
            return true;
        }

        if *split_direction != direction.split_direction() {
            return false;
        }

        let resized = if direction.grows_first() {
            ratio.saturating_add(SPLIT_RESIZE_STEP)
        } else {
            ratio.saturating_sub(SPLIT_RESIZE_STEP)
        }
        .clamp(MIN_SPLIT_RATIO, MAX_SPLIT_RATIO);
        if resized == *ratio {
            return false;
        }

        *ratio = resized;
        true
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

    fn swap_pane_ids(&mut self, first: PaneId, second: PaneId) {
        match self {
            Self::Pane(pane_id) if *pane_id == first => *pane_id = second,
            Self::Pane(pane_id) if *pane_id == second => *pane_id = first,
            Self::Pane(_) => {}
            Self::Split {
                first: first_tree,
                second: second_tree,
                ..
            } => {
                first_tree.swap_pane_ids(first, second);
                second_tree.swap_pane_ids(first, second);
            }
        }
    }
}

fn remove_pane_from(tree: PaneTree, target: PaneId) -> Option<PaneTree> {
    match tree {
        PaneTree::Pane(pane_id) => (pane_id != target).then_some(PaneTree::Pane(pane_id)),
        PaneTree::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let first = remove_pane_from(*first, target);
            let second = remove_pane_from(*second, target);

            match (first, second) {
                (Some(first), Some(second)) => Some(PaneTree::Split {
                    direction,
                    ratio,
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

    fn root_ratio(tree: &PaneTree) -> u16 {
        match tree {
            PaneTree::Split { ratio, .. } => *ratio,
            PaneTree::Pane(_) => panic!("expected a split pane tree"),
        }
    }

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

    #[test]
    fn swapping_panes_changes_leaf_positions_without_changing_the_split_tree() {
        let first = PaneId::new(0);
        let second = PaneId::new(1);
        let third = PaneId::new(2);
        let mut tree = PaneTree::single(first);
        assert!(tree.split_pane(first, PaneSplitDirection::Horizontal, second));
        assert!(tree.split_pane(second, PaneSplitDirection::Vertical, third));

        assert!(tree.swap_panes(first, third));
        assert_eq!(tree.pane_ids(), vec![third, second, first]);
        assert!(!tree.swap_panes(first, PaneId::new(99)));
        assert!(!tree.swap_panes(first, first));
    }

    #[test]
    fn resizing_moves_the_nearest_matching_split_and_clamps_its_ratio() {
        let first = PaneId::new(0);
        let second = PaneId::new(1);
        let third = PaneId::new(2);
        let mut tree = PaneTree::single(first);
        assert!(tree.split_pane(first, PaneSplitDirection::Horizontal, second));
        assert!(tree.split_pane(second, PaneSplitDirection::Horizontal, third));

        assert!(tree.resize_pane(third, PaneResizeDirection::Right));
        assert_eq!(root_ratio(&tree), DEFAULT_SPLIT_RATIO);
        let PaneTree::Split { second, .. } = &tree else {
            unreachable!();
        };
        assert_eq!(root_ratio(second), DEFAULT_SPLIT_RATIO + SPLIT_RESIZE_STEP);

        for _ in 0..SPLIT_RATIO_SCALE / SPLIT_RESIZE_STEP {
            tree.resize_pane(third, PaneResizeDirection::Left);
        }
        let PaneTree::Split { second, .. } = &tree else {
            unreachable!();
        };
        assert_eq!(root_ratio(second), MIN_SPLIT_RATIO);
        let mut nested = (**second).clone();
        assert!(!nested.resize_pane(third, PaneResizeDirection::Left));
    }

    #[test]
    fn resizing_uses_an_outer_split_when_the_inner_axis_does_not_match() {
        let first = PaneId::new(0);
        let second = PaneId::new(1);
        let third = PaneId::new(2);
        let mut tree = PaneTree::single(first);
        assert!(tree.split_pane(first, PaneSplitDirection::Horizontal, second));
        assert!(tree.split_pane(second, PaneSplitDirection::Vertical, third));

        assert!(tree.resize_pane(third, PaneResizeDirection::Left));
        assert_eq!(root_ratio(&tree), DEFAULT_SPLIT_RATIO - SPLIT_RESIZE_STEP);
    }
}
