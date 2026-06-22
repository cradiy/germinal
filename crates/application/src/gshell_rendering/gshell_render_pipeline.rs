use germinal_domain::gshell::gshell_id::GShellId;
use germinal_ports::{
	gshell::output_event::GShellOutputEvent,
	pty_host::output_applier::TerminalOutputApplier,
	rendering::{
		frame_plan_builder::BuiltFramePlan, frame_plan_executor::FramePlanExecutor,
		render_target_id::RenderTargetId,
	},
	seq::Seq,
};

use crate::{
	rendering::render_pipeline::{FrameBuiltResult, InputUpdateResult},
	workspace_rendering::workspace_render_pipeline::WorkspaceRenderPipeline,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GShellOutputState {
	pub latest_seq:       Seq,
	pub render_target_id: RenderTargetId,
	pub total_bytes:      u64,
	pub chunk_count:      u64,
	pub changed:          bool,
}

impl Default for GShellOutputState {
	fn default() -> Self {
		Self {
			latest_seq:       Seq::ZERO,
			render_target_id: RenderTargetId::new(0),
			total_bytes:      0,
			chunk_count:      0,
			changed:          false,
		}
	}
}

#[derive(Debug, Default)]
pub struct GShellOutputTracker {
	states: std::collections::HashMap<GShellId, GShellOutputState>,
}

impl GShellOutputTracker {
	pub fn new() -> Self { Self::default() }

	pub fn record_apply_result(
		&mut self,
		result: &germinal_ports::pty_host::output_applier::TerminalApplyResult,
	) {
		let state = self.states.entry(result.gshell_id).or_default();
		state.latest_seq = result.latest_seq;
		state.render_target_id = result.render_target_id;
		state.total_bytes += result.bytes_applied as u64;
		state.chunk_count += 1;
		state.changed |= result.changed;
	}

	pub fn state_of(&self, gshell_id: GShellId) -> Option<&GShellOutputState> {
		self.states.get(&gshell_id)
	}
}

#[derive(Debug)]
pub struct GShellRenderPipeline<E, T> {
	output_tracker:     GShellOutputTracker,
	terminal_applier:   T,
	workspace_pipeline: WorkspaceRenderPipeline<E>,
}

impl<E, T> GShellRenderPipeline<E, T>
where
	E: FramePlanExecutor,
	T: TerminalOutputApplier,
{
	pub fn new(executor: E, terminal_applier: T) -> Self {
		Self {
			output_tracker: GShellOutputTracker::new(),
			terminal_applier,
			workspace_pipeline: WorkspaceRenderPipeline::new(executor),
		}
	}

	pub fn register_gshell(&mut self, gshell_id: GShellId) {
		self.workspace_pipeline.register_gshell(gshell_id)
	}

	pub fn on_gshell_output_event(&mut self, event: GShellOutputEvent) -> GShellOutputUpdateResult {
		let gshell_id = event.gshell_id;

		self.workspace_pipeline.register_gshell(gshell_id);
		let render_target_id = RenderTargetId::new(gshell_id.value());

		let apply_result = self.terminal_applier.apply(render_target_id, &event);

		let seq = apply_result.latest_seq;

		self.output_tracker.record_apply_result(&apply_result);

		let result = self.workspace_pipeline.on_gshell_output_updated(gshell_id, seq);

		GShellOutputUpdateResult::Accepted(result)
	}

	pub fn on_gshell_output_updated(
		&mut self,
		gshell_id: GShellId,
		seq: Seq,
	) -> GShellOutputUpdateResult {
		let result = self.workspace_pipeline.on_gshell_output_updated(gshell_id, seq);

		GShellOutputUpdateResult::Accepted(result)
	}

	pub fn on_frame_built(&mut self, frame: BuiltFramePlan) -> FrameBuiltResult {
		self.workspace_pipeline.on_frame_built(frame)
	}

	pub fn mark_presented(&mut self, target_id: RenderTargetId, seq: Seq) -> bool {
		self.workspace_pipeline.mark_presented(target_id, seq)
	}

	pub fn output_state_of(&self, gshell_id: GShellId) -> Option<&GShellOutputState> {
		self.output_tracker.state_of(gshell_id)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GShellOutputUpdateResult {
	UnknownGShell,
	Accepted(InputUpdateResult),
}

#[cfg(test)]
mod tests {
	use std::cell::RefCell;

	use germinal_ports::{
		pty_host::output_applier::TerminalApplyResult,
		rendering::{
			frame_plan_builder::{BuildFramePlanTask, BuiltFramePlan, RenderCommandDto},
			frame_plan_executor::FramePlanExecutor,
		},
	};

	use super::*;

	#[derive(Debug, Default)]
	struct TestFramePlanExecutor {
		submitted: RefCell<Vec<BuildFramePlanTask>>,
	}

	impl TestFramePlanExecutor {
		fn submitted(&self) -> Vec<BuildFramePlanTask> { self.submitted.borrow().clone() }
	}

	impl FramePlanExecutor for TestFramePlanExecutor {
		fn submit(&self, task: BuildFramePlanTask) { self.submitted.borrow_mut().push(task); }
	}

	#[derive(Debug, Default)]
	struct TestTerminalOutputApplier;

	impl TerminalOutputApplier for TestTerminalOutputApplier {
		fn apply(
			&mut self,
			render_target_id: RenderTargetId,
			event: &GShellOutputEvent,
		) -> TerminalApplyResult {
			TerminalApplyResult {
				gshell_id: event.gshell_id,
				render_target_id,
				latest_seq: event.seq,
				bytes_applied: event.bytes.len(),
				changed: !event.bytes.is_empty(),
			}
		}
	}

	fn frame(task: BuildFramePlanTask) -> BuiltFramePlan {
		BuiltFramePlan {
			target_id: task.target_id,
			seq:       task.seq,
			commands:  vec![RenderCommandDto::Clear],
		}
	}

	#[test]
	fn unknown_gshell_output_is_rejected() {
		let executor = TestFramePlanExecutor::default();
		let terminal_applier = TestTerminalOutputApplier;
		let mut pipeline = GShellRenderPipeline::new(executor, terminal_applier);

		assert!(matches!(
			pipeline.on_gshell_output_updated(GShellId::new(1), Seq::new(1)),
			GShellOutputUpdateResult::Accepted(_)
		));
	}

	#[test]
	fn gshell_output_submits_render_task() {
		let executor = TestFramePlanExecutor::default();
		let terminal_applier = TestTerminalOutputApplier;
		let mut pipeline = GShellRenderPipeline::new(executor, terminal_applier);

		let gshell_id = GShellId::new(1);
		pipeline.register_gshell(gshell_id);

		let result = pipeline.on_gshell_output_updated(gshell_id, Seq::new(1));

		assert!(matches!(
			result,
			GShellOutputUpdateResult::Accepted(InputUpdateResult::TaskSubmitted(_))
		));

		let submitted = pipeline.workspace_pipeline.render_pipeline.executor.submitted();

		assert_eq!(submitted.len(), 1);
		assert_eq!(submitted[0].seq, Seq::new(1));
	}

	#[test]
	fn gshell_output_event_submits_render_task_and_stores_apply_result() {
		let executor = TestFramePlanExecutor::default();
		let terminal_applier = TestTerminalOutputApplier;
		let mut pipeline = GShellRenderPipeline::new(executor, terminal_applier);

		let gshell_id = GShellId::new(1);
		pipeline.register_gshell(gshell_id);

		let event = GShellOutputEvent::new(gshell_id, Seq::new(1), b"hello\n".to_vec());

		let result = pipeline.on_gshell_output_event(event);

		assert!(matches!(
			result,
			GShellOutputUpdateResult::Accepted(InputUpdateResult::TaskSubmitted(_))
		));

		let submitted = pipeline.workspace_pipeline.render_pipeline.executor.submitted();

		assert_eq!(submitted.len(), 1);
		assert_eq!(submitted[0].seq, Seq::new(1));

		let output_state = pipeline.output_state_of(gshell_id).unwrap();

		assert_eq!(output_state.latest_seq, Seq::new(1));
		assert_eq!(output_state.total_bytes, 6);
		assert_eq!(output_state.chunk_count, 1);
		assert!(output_state.changed);
	}

	#[test]
	fn gshell_output_is_latest_wins() {
		let executor = TestFramePlanExecutor::default();
		let terminal_applier = TestTerminalOutputApplier;
		let mut pipeline = GShellRenderPipeline::new(executor, terminal_applier);

		let gshell_id = GShellId::new(1);
		pipeline.register_gshell(gshell_id);

		pipeline.on_gshell_output_updated(gshell_id, Seq::new(1));
		pipeline.on_gshell_output_updated(gshell_id, Seq::new(2));
		pipeline.on_gshell_output_updated(gshell_id, Seq::new(5));

		let submitted = pipeline.workspace_pipeline.render_pipeline.executor.submitted();

		assert_eq!(submitted.len(), 1);
		assert_eq!(submitted[0].seq, Seq::new(1));

		let built = pipeline.on_frame_built(frame(submitted[0]));

		assert_eq!(built.result.next_request.unwrap().seq, Seq::new(5));

		let submitted = pipeline.workspace_pipeline.render_pipeline.executor.submitted();

		assert_eq!(submitted.len(), 2);
		assert_eq!(submitted[0].seq, Seq::new(1));
		assert_eq!(submitted[1].seq, Seq::new(5));
	}
}
