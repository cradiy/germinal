use germinal_ports::{rendering::render_target_id::RenderTargetId, seq::Seq};

use crate::rendering::pty_surface::{
    frame_upload_plan::{WgpuTerminalFrameUploadPlan, WgpuTerminalUploadedFrame},
    render_pass_encoder::{WgpuTerminalRenderPassEncoder, WgpuTerminalRenderPassPlanEncoder},
    render_pass_plan::{WgpuRenderPassCommand, WgpuTerminalRenderPassPlan},
};

#[derive(Debug, Clone, Copy, Default)]
pub struct WgpuTerminalFrameEncoder {
    plan_encoder: WgpuTerminalRenderPassPlanEncoder,
}

impl WgpuTerminalFrameEncoder {
    pub fn new() -> Self {
        Self {
            plan_encoder: WgpuTerminalRenderPassPlanEncoder::new(),
        }
    }

    pub fn encode_upload_plan<E>(
        &self,
        upload_plan: &WgpuTerminalFrameUploadPlan<'_>,
        encoder: &mut E,
    ) -> WgpuTerminalFrameEncodeResult
    where
        E: WgpuTerminalRenderPassEncoder,
    {
        self.encode_optional_plan(
            upload_plan.target_id,
            upload_plan.seq,
            upload_plan.render_pass_plan,
            encoder,
        )
    }

    pub fn encode_uploaded_frame<E>(
        &self,
        uploaded_frame: &WgpuTerminalUploadedFrame<'_>,
        encoder: &mut E,
    ) -> WgpuTerminalFrameEncodeResult
    where
        E: WgpuTerminalRenderPassEncoder,
    {
        self.encode_optional_plan(
            uploaded_frame.target_id,
            uploaded_frame.seq,
            uploaded_frame.render_pass_plan,
            encoder,
        )
    }

    pub fn encode_plan<E>(
        &self,
        target_id: RenderTargetId,
        seq: Seq,
        render_pass_plan: &WgpuTerminalRenderPassPlan,
        encoder: &mut E,
    ) -> WgpuTerminalFrameEncodeResult
    where
        E: WgpuTerminalRenderPassEncoder,
    {
        self.encode_optional_plan(target_id, seq, Some(render_pass_plan), encoder)
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
    pub target_id: RenderTargetId,
    pub seq: Seq,
    pub encoded: bool,
    pub command_count: usize,
    pub draw_count: usize,
    pub index_count: u32,
}

impl WgpuTerminalFrameEncodeResult {
    pub fn encoded(&self) -> bool {
        self.encoded
    }
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
