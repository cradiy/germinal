use germinal_domain::{
	rendering::render_target_id::RenderTargetId, shared::seq::Seq, workspace::pane_id::PaneId,
};
use germinal_ports::rendering::{
	frame_plan_builder::BuiltFramePlan, frame_plan_executor::FramePlanExecutor,
};

use crate::{
	rendering::render_pipeline::{FrameBuiltResult, InputUpdateResult, RenderPipeline},
	workspace_rendering::pane_render_registry::PaneRenderRegistry,
};

#[derive(Debug)]
pub struct WorkspaceRenderPipeline<E> {
	registry:                   PaneRenderRegistry,
	pub(crate) render_pipeline: RenderPipeline<E>,
}

impl<E> WorkspaceRenderPipeline<E>
where E: FramePlanExecutor
{
	pub fn new(executor: E) -> Self {
		Self {
			registry:        PaneRenderRegistry::new(),
			render_pipeline: RenderPipeline::new(executor),
		}
	}

	pub fn register_pane(&mut self, pane_id: PaneId) {
		let target_id = self.registry.register_pane(pane_id);
		self.render_pipeline.register_target(target_id);
	}

	pub fn render_target_of(&self, pane_id: PaneId) -> Option<RenderTargetId> {
		self.registry.render_target_of(pane_id)
	}

	pub fn on_pane_output_updated(&mut self, pane_id: PaneId, seq: Seq) -> InputUpdateResult {
		let target_id = self.registry.ensure_render_target(pane_id);
		self.render_pipeline.register_target(target_id);
		self.render_pipeline.on_input_updated(target_id, seq)
	}

	pub fn on_frame_built(&mut self, frame: BuiltFramePlan) -> FrameBuiltResult {
		self.render_pipeline.on_frame_built(frame)
	}

	pub fn mark_frame_presented(&mut self, frame: &FrameBuiltResult) -> bool {
		let Some(ready) = frame.ready_frame() else {
			return false;
		};

		self.render_pipeline.mark_presented(ready.target_id, ready.seq)
	}

	pub fn mark_presented(&mut self, target_id: RenderTargetId, seq: Seq) -> bool {
		self.render_pipeline.mark_presented(target_id, seq)
	}

	pub fn registry(&self) -> &PaneRenderRegistry { &self.registry }
}
