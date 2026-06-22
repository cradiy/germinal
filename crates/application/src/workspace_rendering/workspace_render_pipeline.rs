use germinal_domain::gshell::gshell_id::GShellId;
use germinal_ports::{
	rendering::{
		frame_plan_builder::BuiltFramePlan, frame_plan_executor::FramePlanExecutor,
		render_target_id::RenderTargetId,
	},
	seq::Seq,
};

use crate::rendering::render_pipeline::{FrameBuiltResult, InputUpdateResult, RenderPipeline};

#[derive(Debug)]
pub struct WorkspaceRenderPipeline<E> {
	pub(crate) render_pipeline: RenderPipeline<E>,
}

impl<E> WorkspaceRenderPipeline<E>
where E: FramePlanExecutor
{
	pub fn new(executor: E) -> Self { Self { render_pipeline: RenderPipeline::new(executor) } }

	pub fn register_gshell(&mut self, gshell_id: GShellId) {
		self.render_pipeline.register_target(target_of(gshell_id));
	}

	pub fn on_gshell_output_updated(&mut self, gshell_id: GShellId, seq: Seq) -> InputUpdateResult {
		let target_id = target_of(gshell_id);
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
}

fn target_of(gshell_id: GShellId) -> RenderTargetId { RenderTargetId::new(gshell_id.value()) }
