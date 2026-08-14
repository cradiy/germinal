use germinal_ports::{rendering::render_target_id::RenderTargetId, seq::Seq};

use crate::rendering::pty_surface::{
    frame_encoder::{WgpuTerminalFrameEncodeResult, WgpuTerminalFrameEncoder},
    frame_upload_plan::WgpuTerminalUploadedFrame,
    pipeline_factory::WgpuTerminalPipeline,
    render_target_plan::WgpuTerminalRenderTargetPlan,
};

#[derive(Debug, Clone, Copy, Default)]
pub struct WgpuTerminalCommandEncoderAdapter;

impl WgpuTerminalCommandEncoderAdapter {
    pub fn new() -> Self {
        Self
    }

    pub fn encode_frame(
        &self,
        command_encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        render_target_plan: WgpuTerminalRenderTargetPlan,
        pipeline: &WgpuTerminalPipeline,
        uploaded_frame: &WgpuTerminalUploadedFrame<'_>,
    ) -> WgpuTerminalCommandEncoderResult {
        if render_target_plan.is_empty() {
            return WgpuTerminalCommandEncoderResult {
                target_id: uploaded_frame.target_id,
                seq: uploaded_frame.seq,
                began_render_pass: false,
                encoded_frame: false,
                command_count: 0,
                draw_count: 0,
                index_count: 0,
            };
        }

        let color_attachment = Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: render_target_plan.wgpu_load_op(),
                store: store_op_of(render_target_plan.store),
            },
        });

        let color_attachments = [color_attachment];

        let mut render_pass = command_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("germinal.terminal.render_pass"),
            color_attachments: &color_attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        let frame_encoder = WgpuTerminalFrameEncoder::new();
        render_target_plan.apply_viewport(&mut render_pass);

        let encode_result =
            frame_encoder.encode_render_pass(&mut render_pass, pipeline, uploaded_frame);

        drop(render_pass);

        WgpuTerminalCommandEncoderResult {
            target_id: uploaded_frame.target_id,
            seq: uploaded_frame.seq,
            began_render_pass: true,
            encoded_frame: encode_result.encoded,
            command_count: encode_result.command_count,
            draw_count: encode_result.draw_count,
            index_count: encode_result.index_count,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalCommandEncoderResult {
    pub target_id: RenderTargetId,
    pub seq: Seq,
    pub began_render_pass: bool,
    pub encoded_frame: bool,
    pub command_count: usize,
    pub draw_count: usize,
    pub index_count: u32,
}

impl WgpuTerminalCommandEncoderResult {
    pub fn encoded_frame(&self) -> bool {
        self.encoded_frame
    }

    pub fn began_render_pass(&self) -> bool {
        self.began_render_pass
    }
}

impl From<WgpuTerminalFrameEncodeResult> for WgpuTerminalCommandEncoderResult {
    fn from(result: WgpuTerminalFrameEncodeResult) -> Self {
        Self {
            target_id: result.target_id,
            seq: result.seq,
            began_render_pass: result.encoded,
            encoded_frame: result.encoded,
            command_count: result.command_count,
            draw_count: result.draw_count,
            index_count: result.index_count,
        }
    }
}

fn store_op_of(store: bool) -> wgpu::StoreOp {
    if store {
        wgpu::StoreOp::Store
    } else {
        wgpu::StoreOp::Discard
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalCommandEncoderAdapterSpec {
    pub color_attachment_count: usize,
    pub has_depth_stencil_attachment: bool,
    pub has_timestamp_writes: bool,
    pub has_occlusion_query_set: bool,
    pub has_multiview_mask: bool,
}

impl WgpuTerminalCommandEncoderAdapterSpec {
    pub fn new() -> Self {
        Self {
            color_attachment_count: 1,
            has_depth_stencil_attachment: false,
            has_timestamp_writes: false,
            has_occlusion_query_set: false,
            has_multiview_mask: false,
        }
    }
}

impl Default for WgpuTerminalCommandEncoderAdapterSpec {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_spec_matches_terminal_pass_layout() {
        let spec = WgpuTerminalCommandEncoderAdapterSpec::new();

        assert_eq!(spec.color_attachment_count, 1);
        assert!(!spec.has_depth_stencil_attachment);
        assert!(!spec.has_timestamp_writes);
        assert!(!spec.has_occlusion_query_set);
        assert!(!spec.has_multiview_mask);
    }

    #[test]
    fn maps_store_flag_to_wgpu_store_op() {
        assert_eq!(store_op_of(true), wgpu::StoreOp::Store);
        assert_eq!(store_op_of(false), wgpu::StoreOp::Discard);
    }
}
