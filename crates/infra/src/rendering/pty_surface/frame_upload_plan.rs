use std::sync::Arc;

use germinal_domain::{rendering::render_target_id::RenderTargetId, shared::seq::Seq};

use crate::rendering::pty_surface::{
	buffer_uploader::{WgpuBufferUploadBytes, WgpuBufferUploader, WgpuUploadedBuffers},
	frame_builder::WgpuTerminalPreparedFrame,
	glyph_atlas_bind_group::{
		WgpuTerminalGlyphAtlasBindGroup, WgpuTerminalGlyphAtlasBindGroupFactory,
	},
	glyph_atlas_gpu_cache::WgpuTerminalGlyphAtlasGpuCache,
	glyph_atlas_texture::{WgpuTerminalGlyphAtlasTexture, WgpuTerminalGlyphAtlasTextureFactory},
	render_pass_plan::WgpuTerminalRenderPassPlan,
	viewport_bind_group::{
		WgpuViewportBindGroup, WgpuViewportBindGroupFactory, WgpuViewportUploadBytes,
	},
};

#[derive(Debug, Clone, PartialEq)]
pub struct WgpuTerminalFrameUploadPlan {
	pub target_id:        RenderTargetId,
	pub seq:              Seq,
	pub vertex_upload:    WgpuBufferUploadBytes,
	pub viewport_upload:  WgpuViewportUploadBytes,
	pub render_pass_plan: Option<WgpuTerminalRenderPassPlan>,
}

impl WgpuTerminalFrameUploadPlan {
	pub fn from_prepared_frame(prepared: &WgpuTerminalPreparedFrame) -> Self {
		Self {
			target_id:        prepared.target_id,
			seq:              prepared.seq,
			vertex_upload:    prepared.upload_bytes.clone(),
			viewport_upload:  prepared.viewport_upload_bytes.clone(),
			render_pass_plan: prepared.render_pass_plan.clone(),
		}
	}

	pub fn has_draw_work(&self) -> bool {
		!self.vertex_upload.is_empty() && self.render_pass_plan.is_some()
	}

	pub fn vertex_count(&self) -> u32 { self.vertex_upload.vertex_count }

	pub fn index_count(&self) -> u32 { self.vertex_upload.index_count }

	pub fn viewport_byte_len(&self) -> usize { self.viewport_upload.byte_len() }

	pub fn upload(
		&self,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		viewport_bind_group_layout: &wgpu::BindGroupLayout,
		viewport_binding: u32,
		glyph_atlas_bind_group_layout: &wgpu::BindGroupLayout,
		prepared: &WgpuTerminalPreparedFrame,
	) -> WgpuTerminalUploadedFrame {
		self.upload_inner(
			device,
			queue,
			viewport_bind_group_layout,
			viewport_binding,
			glyph_atlas_bind_group_layout,
			prepared,
			None,
		)
	}

	pub fn upload_with_glyph_atlas_gpu_cache(
		&self,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		viewport_bind_group_layout: &wgpu::BindGroupLayout,
		viewport_binding: u32,
		glyph_atlas_bind_group_layout: &wgpu::BindGroupLayout,
		prepared: &WgpuTerminalPreparedFrame,
		glyph_atlas_gpu_cache: &WgpuTerminalGlyphAtlasGpuCache,
	) -> WgpuTerminalUploadedFrame {
		self.upload_inner(
			device,
			queue,
			viewport_bind_group_layout,
			viewport_binding,
			glyph_atlas_bind_group_layout,
			prepared,
			Some(glyph_atlas_gpu_cache),
		)
	}

	#[allow(clippy::too_many_arguments)]
	fn upload_inner(
		&self,
		device: &wgpu::Device,
		queue: &wgpu::Queue,
		viewport_bind_group_layout: &wgpu::BindGroupLayout,
		viewport_binding: u32,
		glyph_atlas_bind_group_layout: &wgpu::BindGroupLayout,
		prepared: &WgpuTerminalPreparedFrame,
		glyph_atlas_gpu_cache: Option<&WgpuTerminalGlyphAtlasGpuCache>,
	) -> WgpuTerminalUploadedFrame {
		let buffer_uploader = WgpuBufferUploader::new();

		let uploaded_buffers = buffer_uploader.upload_bytes(device, &self.vertex_upload);

		let viewport_factory = WgpuViewportBindGroupFactory::new();

		let viewport_bind_group = viewport_factory.create(
			device,
			viewport_bind_group_layout,
			viewport_binding,
			prepared.viewport,
		);

		let glyph_atlas_gpu = if let Some(cache) = glyph_atlas_gpu_cache {
			cache.get_or_upload(device, queue, glyph_atlas_bind_group_layout, &prepared.glyph_atlas_frame)
		} else {
			upload_glyph_atlas_without_cache(device, queue, glyph_atlas_bind_group_layout, prepared)
		};

		WgpuTerminalUploadedFrame {
			target_id: self.target_id,
			seq: self.seq,
			uploaded_buffers,
			viewport_bind_group,
			glyph_atlas_texture: glyph_atlas_gpu.texture,
			glyph_atlas_bind_group: glyph_atlas_gpu.bind_group,
			glyph_atlas_gpu_cache_hit: glyph_atlas_gpu.cache_hit,
			render_pass_plan: self.render_pass_plan.clone(),
		}
	}
}

pub struct WgpuTerminalUploadedFrame {
	pub target_id:                 RenderTargetId,
	pub seq:                       Seq,
	pub uploaded_buffers:          WgpuUploadedBuffers,
	pub viewport_bind_group:       WgpuViewportBindGroup,
	pub glyph_atlas_texture:       Option<Arc<WgpuTerminalGlyphAtlasTexture>>,
	pub glyph_atlas_bind_group:    Option<Arc<WgpuTerminalGlyphAtlasBindGroup>>,
	pub glyph_atlas_gpu_cache_hit: bool,
	pub render_pass_plan:          Option<WgpuTerminalRenderPassPlan>,
}

impl WgpuTerminalUploadedFrame {
	pub fn has_draw_work(&self) -> bool {
		self.uploaded_buffers.vertex_count > 0
			&& self.uploaded_buffers.index_count > 0
			&& self.render_pass_plan.is_some()
	}

	pub fn has_glyph_atlas_bind_group(&self) -> bool {
		self.glyph_atlas_texture.is_some() && self.glyph_atlas_bind_group.is_some()
	}
}

fn upload_glyph_atlas_without_cache(
	device: &wgpu::Device,
	queue: &wgpu::Queue,
	glyph_atlas_bind_group_layout: &wgpu::BindGroupLayout,
	prepared: &WgpuTerminalPreparedFrame,
) -> crate::rendering::pty_surface::glyph_atlas_gpu_cache::WgpuTerminalGlyphAtlasGpuCacheResult {
	let glyph_atlas_texture_factory = WgpuTerminalGlyphAtlasTextureFactory::new();

	let glyph_atlas_texture = glyph_atlas_texture_factory
		.upload(device, queue, &prepared.glyph_atlas_frame.atlas)
		.map(Arc::new);

	let glyph_atlas_bind_group_factory = WgpuTerminalGlyphAtlasBindGroupFactory::new();

	let glyph_atlas_bind_group = glyph_atlas_texture.as_ref().map(|texture| {
		Arc::new(glyph_atlas_bind_group_factory.create_bind_group(
			device,
			glyph_atlas_bind_group_layout,
			texture.as_ref(),
		))
	});

	crate::rendering::pty_surface::glyph_atlas_gpu_cache::WgpuTerminalGlyphAtlasGpuCacheResult {
		texture:    glyph_atlas_texture,
		bind_group: glyph_atlas_bind_group,
		cache_hit:  false,
	}
}
