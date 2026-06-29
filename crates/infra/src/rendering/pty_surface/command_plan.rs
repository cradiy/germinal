use germinal_ports::{rendering::render_target_id::RenderTargetId, seq::Seq};

use crate::rendering::pty_surface::{
	frame_encoder::WgpuTerminalFrameEncodeResult, frame_upload_plan::WgpuTerminalFrameUploadPlan,
	render_target_plan::WgpuTerminalRenderTargetPlan,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuTerminalCommandPlan {
	pub target_id: RenderTargetId,
	pub seq:       Seq,
	pub commands:  Vec<WgpuTerminalCommand>,
}

impl WgpuTerminalCommandPlan {
	pub fn new(
		render_target_plan: WgpuTerminalRenderTargetPlan,
		upload_plan: &WgpuTerminalFrameUploadPlan<'_>,
	) -> Self {
		let mut commands = Vec::new();

		if render_target_plan.is_empty() {
			return Self { target_id: upload_plan.target_id, seq: upload_plan.seq, commands };
		}

		commands.push(WgpuTerminalCommand::BeginRenderPass {
			width_px:  render_target_plan.width_px,
			height_px: render_target_plan.height_px,
			store:     render_target_plan.store,
		});

		if upload_plan.has_draw_work() {
			commands.extend([
				WgpuTerminalCommand::SetPipeline,
				WgpuTerminalCommand::SetBindGroup { index: 0, dynamic_offsets_len: 0 },
				WgpuTerminalCommand::SetVertexBuffer { slot: 0 },
				WgpuTerminalCommand::SetIndexBuffer { format: wgpu::IndexFormat::Uint32 },
				WgpuTerminalCommand::DrawIndexed {
					index_count:    upload_plan.index_count(),
					instance_count: 1,
				},
			]);
		}

		commands.push(WgpuTerminalCommand::EndRenderPass);
		commands.push(WgpuTerminalCommand::Submit);

		Self { target_id: upload_plan.target_id, seq: upload_plan.seq, commands }
	}

	pub fn is_empty(&self) -> bool { self.commands.is_empty() }

	pub fn len(&self) -> usize { self.commands.len() }

	pub fn has_draw_work(&self) -> bool {
		self.commands.iter().any(|command| matches!(command, WgpuTerminalCommand::DrawIndexed { .. }))
	}

	pub fn begin_render_pass_count(&self) -> usize {
		self
			.commands
			.iter()
			.filter(|command| matches!(command, WgpuTerminalCommand::BeginRenderPass { .. }))
			.count()
	}

	pub fn submit_count(&self) -> usize {
		self.commands.iter().filter(|command| matches!(command, WgpuTerminalCommand::Submit)).count()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuTerminalCommand {
	BeginRenderPass { width_px: u32, height_px: u32, store: bool },
	SetPipeline,
	SetBindGroup { index: u32, dynamic_offsets_len: usize },
	SetVertexBuffer { slot: u32 },
	SetIndexBuffer { format: wgpu::IndexFormat },
	DrawIndexed { index_count: u32, instance_count: u32 },
	EndRenderPass,
	Submit,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WgpuTerminalCommandPlanRecorder {
	pub recorded: Vec<WgpuTerminalCommand>,
}

impl WgpuTerminalCommandPlanRecorder {
	pub fn new() -> Self { Self::default() }

	pub fn record_plan(&mut self, plan: &WgpuTerminalCommandPlan) {
		self.recorded.extend(plan.commands.iter().copied());
	}

	pub fn command_count(&self) -> usize { self.recorded.len() }

	pub fn draw_count(&self) -> usize {
		self
			.recorded
			.iter()
			.filter(|command| matches!(command, WgpuTerminalCommand::DrawIndexed { .. }))
			.count()
	}

	pub fn submit_count(&self) -> usize {
		self.recorded.iter().filter(|command| matches!(command, WgpuTerminalCommand::Submit)).count()
	}
}

impl From<WgpuTerminalFrameEncodeResult> for WgpuTerminalCommandEncodeSummary {
	fn from(result: WgpuTerminalFrameEncodeResult) -> Self {
		Self {
			target_id:                 result.target_id,
			seq:                       result.seq,
			encoded:                   result.encoded,
			render_pass_command_count: result.command_count,
			draw_count:                result.draw_count,
			index_count:               result.index_count,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalCommandEncodeSummary {
	pub target_id:                 RenderTargetId,
	pub seq:                       Seq,
	pub encoded:                   bool,
	pub render_pass_command_count: usize,
	pub draw_count:                usize,
	pub index_count:               u32,
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use super::*;
	use crate::rendering::pty_surface::render_target_plan::WgpuTerminalRenderTargetPlan;

	#[test]
	fn builds_command_plan_for_draw_frame() {
		let target_id = RenderTargetId::new(1);
		let seq = Seq::new(9);
		let vertex_upload = crate::rendering::pty_surface::buffer_uploader::WgpuBufferUploadBytes {
			vertex_data:  Arc::from(vec![
				crate::rendering::pty_surface::buffer_uploader::WgpuGpuVertex::default(),
			]),
			index_data:   Arc::from(vec![0_u32]),
			vertex_count: 4,
			index_count:  42,
		};
		let viewport_upload =
			crate::rendering::pty_surface::viewport_bind_group::WgpuViewportUploadBytes {
				bytes:     vec![0; 8],
				width_px:  1280.0,
				height_px: 720.0,
			};
		let render_pass_plan =
			crate::rendering::pty_surface::render_pass_plan::WgpuTerminalRenderPassPlan::new(
				crate::rendering::pty_surface::draw_indexed::WgpuDrawIndexedPlan {
					vertex_slot:    0,
					index_format:   wgpu::IndexFormat::Uint32,
					index_count:    42,
					base_vertex:    0,
					first_instance: 0,
					instance_count: 1,
				},
			);

		let upload_plan = WgpuTerminalFrameUploadPlan {
			target_id,
			seq,
			vertex_upload: &vertex_upload,
			viewport_upload: &viewport_upload,
			render_pass_plan: Some(&render_pass_plan),
		};

		let command_plan =
			WgpuTerminalCommandPlan::new(WgpuTerminalRenderTargetPlan::new(1280, 720), &upload_plan);

		assert_eq!(command_plan.target_id, target_id);
		assert_eq!(command_plan.seq, seq);

		assert_eq!(command_plan.begin_render_pass_count(), 1);
		assert_eq!(command_plan.submit_count(), 1);
		assert!(command_plan.has_draw_work());

		assert_eq!(command_plan.commands, vec![
			WgpuTerminalCommand::BeginRenderPass { width_px: 1280, height_px: 720, store: true },
			WgpuTerminalCommand::SetPipeline,
			WgpuTerminalCommand::SetBindGroup { index: 0, dynamic_offsets_len: 0 },
			WgpuTerminalCommand::SetVertexBuffer { slot: 0 },
			WgpuTerminalCommand::SetIndexBuffer { format: wgpu::IndexFormat::Uint32 },
			WgpuTerminalCommand::DrawIndexed { index_count: 42, instance_count: 1 },
			WgpuTerminalCommand::EndRenderPass,
			WgpuTerminalCommand::Submit,
		]);
	}

	#[test]
	fn empty_render_target_builds_empty_command_plan() {
		let target_id = RenderTargetId::new(1);
		let seq = Seq::new(9);
		let vertex_upload = crate::rendering::pty_surface::buffer_uploader::WgpuBufferUploadBytes {
			vertex_data:  Arc::from(
				Vec::<crate::rendering::pty_surface::buffer_uploader::WgpuGpuVertex>::new(),
			),
			index_data:   Arc::from(Vec::<u32>::new()),
			vertex_count: 0,
			index_count:  0,
		};
		let viewport_upload =
			crate::rendering::pty_surface::viewport_bind_group::WgpuViewportUploadBytes {
				bytes:     vec![0; 8],
				width_px:  0.0,
				height_px: 0.0,
			};

		let upload_plan = WgpuTerminalFrameUploadPlan {
			target_id,
			seq,
			vertex_upload: &vertex_upload,
			viewport_upload: &viewport_upload,
			render_pass_plan: None,
		};

		let command_plan =
			WgpuTerminalCommandPlan::new(WgpuTerminalRenderTargetPlan::new(0, 720), &upload_plan);

		assert!(command_plan.is_empty());
		assert!(!command_plan.has_draw_work());
		assert_eq!(command_plan.begin_render_pass_count(), 0);
		assert_eq!(command_plan.submit_count(), 0);
	}

	#[test]
	fn recorder_records_command_plan() {
		let target_id = RenderTargetId::new(1);
		let seq = Seq::new(9);
		let vertex_upload = crate::rendering::pty_surface::buffer_uploader::WgpuBufferUploadBytes {
			vertex_data:  Arc::from(vec![
				crate::rendering::pty_surface::buffer_uploader::WgpuGpuVertex::default(),
			]),
			index_data:   Arc::from(vec![0_u32]),
			vertex_count: 4,
			index_count:  6,
		};
		let viewport_upload =
			crate::rendering::pty_surface::viewport_bind_group::WgpuViewportUploadBytes {
				bytes:     vec![0; 8],
				width_px:  1280.0,
				height_px: 720.0,
			};
		let render_pass_plan =
			crate::rendering::pty_surface::render_pass_plan::WgpuTerminalRenderPassPlan::new(
				crate::rendering::pty_surface::draw_indexed::WgpuDrawIndexedPlan {
					vertex_slot:    0,
					index_format:   wgpu::IndexFormat::Uint32,
					index_count:    6,
					base_vertex:    0,
					first_instance: 0,
					instance_count: 1,
				},
			);

		let upload_plan = WgpuTerminalFrameUploadPlan {
			target_id,
			seq,
			vertex_upload: &vertex_upload,
			viewport_upload: &viewport_upload,
			render_pass_plan: Some(&render_pass_plan),
		};

		let command_plan =
			WgpuTerminalCommandPlan::new(WgpuTerminalRenderTargetPlan::new(1280, 720), &upload_plan);

		let mut recorder = WgpuTerminalCommandPlanRecorder::new();

		recorder.record_plan(&command_plan);

		assert_eq!(recorder.command_count(), command_plan.len());
		assert_eq!(recorder.draw_count(), 1);
		assert_eq!(recorder.submit_count(), 1);
		assert_eq!(recorder.recorded, command_plan.commands);
	}
}
