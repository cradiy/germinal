#[cfg(target_os = "linux")]
use std::os::fd::OwnedFd;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum WgpuVideoSurfaceFrame {
    Nv12Gpu(WgpuVideoSurfaceNv12GpuFrame),
    #[cfg(target_os = "linux")]
    Nv12DmaBuf(WgpuVideoSurfaceNv12DmaBufFrame),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuVideoSurfaceColorRange {
    Full,
    Limited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuVideoSurfaceColorMatrix {
    Bt601,
    Bt709,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuVideoSurfaceColorProfile {
    pub range: WgpuVideoSurfaceColorRange,
    pub matrix: WgpuVideoSurfaceColorMatrix,
}

impl Default for WgpuVideoSurfaceColorProfile {
    fn default() -> Self {
        Self {
            range: WgpuVideoSurfaceColorRange::Limited,
            matrix: WgpuVideoSurfaceColorMatrix::Bt709,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WgpuVideoSurfaceNv12GpuFrame {
    pub width_px: u32,
    pub height_px: u32,
    pub color_profile: WgpuVideoSurfaceColorProfile,
    pub y_texture: Arc<wgpu::Texture>,
    pub y_plane: wgpu::TextureView,
    pub uv_texture: Arc<wgpu::Texture>,
    pub uv_plane: wgpu::TextureView,
    pub plane_sampler: wgpu::Sampler,
}

impl WgpuVideoSurfaceNv12GpuFrame {
    pub fn new(
        width_px: u32,
        height_px: u32,
        color_profile: WgpuVideoSurfaceColorProfile,
        y_plane: (Arc<wgpu::Texture>, wgpu::TextureView),
        uv_plane: (Arc<wgpu::Texture>, wgpu::TextureView),
        plane_sampler: wgpu::Sampler,
    ) -> Self {
        let (y_texture, y_plane) = y_plane;
        let (uv_texture, uv_plane) = uv_plane;
        Self {
            width_px,
            height_px,
            color_profile,
            y_texture,
            y_plane,
            uv_texture,
            uv_plane,
            plane_sampler,
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct WgpuVideoSurfaceDmaBufPlane {
    pub fd: Arc<OwnedFd>,
    pub offset: u64,
    pub stride: u32,
    pub modifier: u64,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
pub struct WgpuVideoSurfaceNv12DmaBufFrame {
    pub width_px: u32,
    pub height_px: u32,
    pub color_profile: WgpuVideoSurfaceColorProfile,
    pub y_plane: WgpuVideoSurfaceDmaBufPlane,
    pub uv_plane: WgpuVideoSurfaceDmaBufPlane,
}
