use germinal_domain::{rendering::render_target_id::RenderTargetId, shared::seq::Seq};

use crate::rendering::pty_surface::{
	frame_upload_plan::{WgpuTerminalFrameUploadPlan, WgpuTerminalUploadedFrame},
	pipeline_factory::WgpuTerminalPipeline,
	render_pass_adapter::WgpuTerminalRenderPassAdapter,
	render_pass_encoder::{WgpuTerminalRenderPassEncoder, WgpuTerminalRenderPassPlanEncoder},
	render_pass_plan::{WgpuRenderPassCommand, WgpuTerminalRenderPassPlan},
};

#[derive(Debug, Clone, Copy, Default)]
pub struct WgpuTerminalFrameEncoder {
	plan_encoder: WgpuTerminalRenderPassPlanEncoder,
}

impl WgpuTerminalFrameEncoder {
	pub fn new() -> Self { Self { plan_encoder: WgpuTerminalRenderPassPlanEncoder::new() } }

	pub fn encode_upload_plan<E>(
		&self,
		upload_plan: &WgpuTerminalFrameUploadPlan,
		encoder: &mut E,
	) -> WgpuTerminalFrameEncodeResult
	where
		E: WgpuTerminalRenderPassEncoder,
	{
		self.encode_optional_plan(
			upload_plan.target_id,
			upload_plan.seq,
			upload_plan.render_pass_plan.as_ref(),
			encoder,
		)
	}

	pub fn encode_uploaded_frame<E>(
		&self,
		uploaded_frame: &WgpuTerminalUploadedFrame,
		encoder: &mut E,
	) -> WgpuTerminalFrameEncodeResult
	where
		E: WgpuTerminalRenderPassEncoder,
	{
		self.encode_optional_plan(
			uploaded_frame.target_id,
			uploaded_frame.seq,
			uploaded_frame.render_pass_plan.as_ref(),
			encoder,
		)
	}

	pub fn encode_render_pass<'resource>(
		&self,
		render_pass: &mut wgpu::RenderPass<'resource>,
		pipeline: &'resource WgpuTerminalPipeline,
		uploaded_frame: &'resource WgpuTerminalUploadedFrame,
	) -> WgpuTerminalFrameEncodeResult {
		let mut adapter = WgpuTerminalRenderPassAdapter::new(
			render_pass,
			pipeline,
			&uploaded_frame.viewport_bind_group,
			uploaded_frame.glyph_atlas_bind_group.as_deref(),
			&uploaded_frame.uploaded_buffers,
		);

		self.encode_uploaded_frame(uploaded_frame, &mut adapter)
	}

	fn encode_optional_plan<E>(
		&self,
		target_id: RenderTargetId,
		seq: Seq,
		render_pass_plan: Option<&WgpuTerminalRenderPassPlan>,
		encoder: &mut E,
	) -> WgpuTerminalFrameEncodeResult
	where
		E: WgpuTerminalRenderPassEncoder,
	{
		let Some(render_pass_plan) = render_pass_plan else {
			return WgpuTerminalFrameEncodeResult {
				target_id,
				seq,
				encoded: false,
				command_count: 0,
				draw_count: 0,
				index_count: 0,
			};
		};

		if render_pass_plan.is_empty() {
			return WgpuTerminalFrameEncodeResult {
				target_id,
				seq,
				encoded: false,
				command_count: 0,
				draw_count: 0,
				index_count: 0,
			};
		}

		self.plan_encoder.encode(render_pass_plan, encoder);

		WgpuTerminalFrameEncodeResult {
			target_id,
			seq,
			encoded: true,
			command_count: render_pass_plan.commands.len(),
			draw_count: draw_count_of(render_pass_plan),
			index_count: index_count_of(render_pass_plan),
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalFrameEncodeResult {
	pub target_id:     RenderTargetId,
	pub seq:           Seq,
	pub encoded:       bool,
	pub command_count: usize,
	pub draw_count:    usize,
	pub index_count:   u32,
}

impl WgpuTerminalFrameEncodeResult {
	pub fn encoded(&self) -> bool { self.encoded }
}

fn draw_count_of(render_pass_plan: &WgpuTerminalRenderPassPlan) -> usize {
	render_pass_plan
		.commands
		.iter()
		.filter(|command| matches!(command, WgpuRenderPassCommand::DrawIndexed { .. }))
		.count()
}

fn index_count_of(render_pass_plan: &WgpuTerminalRenderPassPlan) -> u32 {
	render_pass_plan
		.commands
		.iter()
		.filter_map(|command| match command {
			WgpuRenderPassCommand::DrawIndexed { indices, .. } => Some(indices.end - indices.start),
			_ => None,
		})
		.sum()
}
