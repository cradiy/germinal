use std::collections::HashMap;

use germinal_ports::{
	rendering::{frame_plan_builder::BuildFramePlanTask, render_target_id::RenderTargetId},
	seq::Seq,
};

use crate::rendering::render_generation::{BuildCompletion, RenderGenerationState};

#[derive(Debug, Default)]
pub struct RenderScheduler {
	targets: HashMap<RenderTargetId, RenderGenerationState>,
}

impl RenderScheduler {
	pub fn new() -> Self { Self { targets: HashMap::new() } }

	pub fn register_target(&mut self, target_id: RenderTargetId) {
		self.targets.entry(target_id).or_insert_with(|| RenderGenerationState::new(Seq::ZERO));
	}

	pub fn mark_input_updated(
		&mut self,
		target_id: RenderTargetId,
		seq: Seq,
	) -> Option<BuildRequest> {
		let state =
			self.targets.entry(target_id).or_insert_with(|| RenderGenerationState::new(Seq::ZERO));

		state.mark_input_updated(seq);

		state.start_build().map(|build_seq| BuildRequest { target_id, seq: build_seq })
	}

	pub fn complete_build(&mut self, target_id: RenderTargetId, seq: Seq) -> BuildResult {
		let Some(state) = self.targets.get_mut(&target_id) else {
			return BuildResult { accepted: false, ready: None, next_request: None };
		};

		let completion = state.complete_build(seq);

		match completion {
			BuildCompletion::Ready => BuildResult {
				accepted:     true,
				ready:        Some(ReadyFrame { target_id, seq }),
				next_request: None,
			},

			BuildCompletion::ReadyAndNeedsRebuild => {
				let next_request =
					state.start_build().map(|next_seq| BuildRequest { target_id, seq: next_seq });

				BuildResult { accepted: true, ready: Some(ReadyFrame { target_id, seq }), next_request }
			}

			BuildCompletion::Stale => {
				BuildResult { accepted: false, ready: None, next_request: None }
			}
		}
	}

	pub fn mark_presented(&mut self, target_id: RenderTargetId, seq: Seq) -> bool {
		let Some(state) = self.targets.get_mut(&target_id) else {
			return false;
		};

		state.mark_presented(seq)
	}

	pub fn state(&self, target_id: RenderTargetId) -> Option<&RenderGenerationState> {
		self.targets.get(&target_id)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildRequest {
	pub target_id: RenderTargetId,
	pub seq:       Seq,
}

impl BuildRequest {
	pub fn into_task(self) -> BuildFramePlanTask {
		BuildFramePlanTask { target_id: self.target_id, seq: self.seq }
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadyFrame {
	pub target_id: RenderTargetId,
	pub seq:       Seq,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildResult {
	pub accepted:     bool,
	pub ready:        Option<ReadyFrame>,
	pub next_request: Option<BuildRequest>,
}

#[cfg(test)]
mod tests {
	use super::*;

	fn target(value: u64) -> RenderTargetId { RenderTargetId::new(value) }

	#[test]
	fn first_input_creates_build_request() {
		let mut scheduler = RenderScheduler::new();
		let target_id = target(1);

		let request = scheduler.mark_input_updated(target_id, Seq::new(1));

		assert_eq!(request, Some(BuildRequest { target_id, seq: Seq::new(1) }));
	}

	#[test]
	fn input_during_in_flight_does_not_create_extra_request() {
		let mut scheduler = RenderScheduler::new();
		let target_id = target(1);

		assert_eq!(
			scheduler.mark_input_updated(target_id, Seq::new(1)),
			Some(BuildRequest { target_id, seq: Seq::new(1) })
		);

		assert_eq!(scheduler.mark_input_updated(target_id, Seq::new(2)), None);
		assert_eq!(scheduler.mark_input_updated(target_id, Seq::new(3)), None);
		assert_eq!(scheduler.mark_input_updated(target_id, Seq::new(5)), None);
	}

	#[test]
	fn complete_build_creates_next_request_for_latest_seq() {
		let mut scheduler = RenderScheduler::new();
		let target_id = target(1);

		assert_eq!(
			scheduler.mark_input_updated(target_id, Seq::new(1)),
			Some(BuildRequest { target_id, seq: Seq::new(1) })
		);

		assert_eq!(scheduler.mark_input_updated(target_id, Seq::new(5)), None);

		let result = scheduler.complete_build(target_id, Seq::new(1));

		assert_eq!(result, BuildResult {
			accepted:     true,
			ready:        Some(ReadyFrame { target_id, seq: Seq::new(1) }),
			next_request: Some(BuildRequest { target_id, seq: Seq::new(5) }),
		});
	}

	#[test]
	fn stale_completion_is_rejected() {
		let mut scheduler = RenderScheduler::new();
		let target_id = target(1);

		scheduler.mark_input_updated(target_id, Seq::new(5));

		let result = scheduler.complete_build(target_id, Seq::new(3));

		assert_eq!(result, BuildResult { accepted: false, ready: None, next_request: None });
	}

	#[test]
	fn different_targets_schedule_independently() {
		let mut scheduler = RenderScheduler::new();

		let a = target(1);
		let b = target(2);

		assert_eq!(
			scheduler.mark_input_updated(a, Seq::new(1)),
			Some(BuildRequest { target_id: a, seq: Seq::new(1) })
		);

		assert_eq!(
			scheduler.mark_input_updated(b, Seq::new(1)),
			Some(BuildRequest { target_id: b, seq: Seq::new(1) })
		);

		assert_eq!(scheduler.mark_input_updated(a, Seq::new(2)), None);

		let result = scheduler.complete_build(a, Seq::new(1));

		assert_eq!(result.next_request, Some(BuildRequest { target_id: a, seq: Seq::new(2) }));
	}

	#[test]
	fn ready_frame_can_be_marked_presented() {
		let mut scheduler = RenderScheduler::new();
		let target_id = target(1);

		scheduler.mark_input_updated(target_id, Seq::new(1));
		scheduler.complete_build(target_id, Seq::new(1));

		assert!(scheduler.mark_presented(target_id, Seq::new(1)));
		assert!(!scheduler.mark_presented(target_id, Seq::new(1)));
		assert!(!scheduler.mark_presented(target_id, Seq::new(0)));
	}
}
