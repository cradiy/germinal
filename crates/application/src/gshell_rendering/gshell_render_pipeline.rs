use germinal_domain::{
	gshell::gshell_id::GShellId, rendering::render_target_id::RenderTargetId, shared::seq::Seq,
	workspace::pane_id::PaneId,
};
use germinal_ports::{
	gshell::output_event::GShellOutputEvent,
	pty_host::output_applier::TerminalOutputApplier,
	rendering::{frame_plan_builder::BuiltFramePlan, frame_plan_executor::FramePlanExecutor},
};

use super::{
	gshell_output_store::{GShellOutputState, GShellOutputStore},
	gshell_pane_registry::GShellPaneRegistry,
};
use crate::{
	rendering::render_pipeline::{FrameBuiltResult, InputUpdateResult},
	workspace_rendering::workspace_render_pipeline::WorkspaceRenderPipeline,
};

#[derive(Debug)]
pub struct GShellRenderPipeline<E, T> {
	registry:           GShellPaneRegistry,
	output_store:       GShellOutputStore,
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
			registry: GShellPaneRegistry::new(),
			output_store: GShellOutputStore::new(),
			terminal_applier,
			workspace_pipeline: WorkspaceRenderPipeline::new(executor),
		}
	}

	pub fn bind_gshell_to_pane(&mut self, gshell_id: GShellId, pane_id: PaneId) {
		self.registry.bind(gshell_id, pane_id);
		self.workspace_pipeline.register_pane(pane_id);
	}

	pub fn on_gshell_output_event(&mut self, event: GShellOutputEvent) -> GShellOutputUpdateResult {
		let gshell_id = event.gshell_id;

		let Some(pane_id) = self.registry.pane_of(gshell_id) else {
			return GShellOutputUpdateResult::UnknownGShell;
		};

		self.workspace_pipeline.register_pane(pane_id);

		let Some(render_target_id) = self.workspace_pipeline.render_target_of(pane_id) else {
			return GShellOutputUpdateResult::UnknownGShell;
		};

		let apply_result = self.terminal_applier.apply(render_target_id, &event);

		let seq = apply_result.latest_seq;

		self.output_store.record_apply_result(&apply_result);

		let result = self.workspace_pipeline.on_pane_output_updated(pane_id, seq);

		GShellOutputUpdateResult::Accepted(result)
	}

	pub fn on_gshell_output_updated(
		&mut self,
		gshell_id: GShellId,
		seq: Seq,
	) -> GShellOutputUpdateResult {
		let Some(pane_id) = self.registry.pane_of(gshell_id) else {
			return GShellOutputUpdateResult::UnknownGShell;
		};

		let result = self.workspace_pipeline.on_pane_output_updated(pane_id, seq);

		GShellOutputUpdateResult::Accepted(result)
	}

	pub fn on_frame_built(&mut self, frame: BuiltFramePlan) -> FrameBuiltResult {
		self.workspace_pipeline.on_frame_built(frame)
	}

	pub fn mark_presented(&mut self, target_id: RenderTargetId, seq: Seq) -> bool {
		self.workspace_pipeline.mark_presented(target_id, seq)
	}

	pub fn registry(&self) -> &GShellPaneRegistry { &self.registry }

	pub fn output_state_of(&self, gshell_id: GShellId) -> Option<&GShellOutputState> {
		self.output_store.state_of(gshell_id)
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

		assert_eq!(
			pipeline.on_gshell_output_updated(GShellId::new(1), Seq::new(1)),
			GShellOutputUpdateResult::UnknownGShell
		);
	}

	#[test]
	fn gshell_output_submits_render_task() {
		let executor = TestFramePlanExecutor::default();
		let terminal_applier = TestTerminalOutputApplier;
		let mut pipeline = GShellRenderPipeline::new(executor, terminal_applier);

		let gshell_id = GShellId::new(1);
		let pane_id = PaneId::new(10);

		pipeline.bind_gshell_to_pane(gshell_id, pane_id);

		let result = pipeline.on_gshell_output_updated(gshell_id, Seq::new(1));

		assert!(matches!(
			result,
			GShellOutputUpdateResult::Accepted(InputUpdateResult::TaskSubmitted(_))
		));

		assert_eq!(pipeline.registry().pane_of(gshell_id), Some(pane_id));

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
		let pane_id = PaneId::new(10);

		pipeline.bind_gshell_to_pane(gshell_id, pane_id);

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
		let pane_id = PaneId::new(10);

		pipeline.bind_gshell_to_pane(gshell_id, pane_id);

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
