#[cfg(target_os = "linux")]
use std::os::fd::OwnedFd;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum WgpuVideoSurfaceFrame {
	Nv12Gpu(WgpuVideoSurfaceNv12GpuFrame),
	#[cfg(target_os = "linux")]
	Nv12DmaBuf(WgpuVideoSurfaceNv12DmaBufFrame),
}

#[derive(Debug, Clone)]
pub struct WgpuVideoSurfaceNv12GpuFrame {
	pub width_px:      u32,
	pub height_px:     u32,
	pub y_texture:     Arc<wgpu::Texture>,
	pub y_plane:       wgpu::TextureView,
	pub uv_texture:    Arc<wgpu::Texture>,
	pub uv_plane:      wgpu::TextureView,
	pub plane_sampler: wgpu::Sampler,
}

impl WgpuVideoSurfaceNv12GpuFrame {
	pub fn new(
		width_px: u32,
		height_px: u32,
		y_texture: Arc<wgpu::Texture>,
		y_plane: wgpu::TextureView,
		uv_texture: Arc<wgpu::Texture>,
		uv_plane: wgpu::TextureView,
		plane_sampler: wgpu::Sampler,
	) -> Self {
		Self { width_px, height_px, y_texture, y_plane, uv_texture, uv_plane, plane_sampler }
	}
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct WgpuVideoSurfaceDmaBufPlane {
	pub fd:       Arc<OwnedFd>,
	pub offset:   u64,
	pub stride:   u32,
	pub modifier: u64,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct WgpuVideoSurfaceNv12DmaBufFrame {
	pub width_px:  u32,
	pub height_px: u32,
	pub y_plane:   WgpuVideoSurfaceDmaBufPlane,
	pub uv_plane:  WgpuVideoSurfaceDmaBufPlane,
}
