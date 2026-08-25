use std::sync::Arc;

use crate::rendering::pty_surface::glyph_atlas::WgpuTerminalGlyphAtlas;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuTerminalGlyphAtlasUploadBytes {
    pub width_px: u32,
    pub height_px: u32,
    pub pixels: Arc<Vec<u8>>,
    pub format: wgpu::TextureFormat,
}

impl WgpuTerminalGlyphAtlasUploadBytes {
    pub fn is_empty(&self) -> bool {
        self.width_px == 0 || self.height_px == 0 || self.pixels.is_empty()
    }

    pub fn byte_len(&self) -> usize {
        self.pixels.len()
    }

    pub fn expected_byte_len(&self) -> usize {
        (self.width_px * self.height_px * 4) as usize
    }

    pub fn bytes_per_row(&self) -> u32 {
        self.width_px * 4
    }

    pub fn rows_per_image(&self) -> u32 {
        self.height_px
    }

    pub fn is_tightly_packed(&self) -> bool {
        self.byte_len() == self.expected_byte_len()
    }
}

fn rgba_pixels_from_atlas(atlas: &WgpuTerminalGlyphAtlas) -> Arc<Vec<u8>> {
    let alpha_len = (atlas.width_px * atlas.height_px) as usize;
    let rgba_len = alpha_len * 4;

    if atlas.pixels.len() == rgba_len {
        return Arc::new(atlas.pixels.clone());
    }

    if atlas.pixels.len() == alpha_len {
        return Arc::new(
            atlas
                .pixels
                .iter()
                .flat_map(|alpha| [0, 0, 0, *alpha])
                .collect(),
        );
    }

    let mut pixels = vec![0; rgba_len];
    let copy_len = pixels.len().min(atlas.pixels.len());
    pixels[..copy_len].copy_from_slice(&atlas.pixels[..copy_len]);
    Arc::new(pixels)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WgpuTerminalGlyphAtlasTextureFactory;

impl WgpuTerminalGlyphAtlasTextureFactory {
    pub fn new() -> Self {
        Self
    }

    pub fn build_upload_bytes(
        &self,
        atlas: &WgpuTerminalGlyphAtlas,
    ) -> Option<WgpuTerminalGlyphAtlasUploadBytes> {
        if atlas.is_empty() {
            return None;
        }

        Some(WgpuTerminalGlyphAtlasUploadBytes {
            width_px: atlas.width_px,
            height_px: atlas.height_px,
            pixels: rgba_pixels_from_atlas(atlas),
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
        })
    }

    pub fn upload(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &WgpuTerminalGlyphAtlas,
    ) -> Option<WgpuTerminalGlyphAtlasTexture> {
        let upload_bytes = self.build_upload_bytes(atlas)?;

        Some(self.upload_bytes(device, queue, &upload_bytes))
    }

    pub fn upload_bytes(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        upload_bytes: &WgpuTerminalGlyphAtlasUploadBytes,
    ) -> WgpuTerminalGlyphAtlasTexture {
        let size = wgpu::Extent3d {
            width: upload_bytes.width_px,
            height: upload_bytes.height_px,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("germinal.terminal.glyph_atlas_texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: upload_bytes.format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            texture.as_image_copy(),
            upload_bytes.pixels.as_slice(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(upload_bytes.bytes_per_row()),
                rows_per_image: Some(upload_bytes.rows_per_image()),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("germinal.terminal.glyph_atlas_texture_view"),
            ..Default::default()
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("germinal.terminal.glyph_atlas_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        WgpuTerminalGlyphAtlasTexture {
            texture,
            view,
            sampler,
            width_px: upload_bytes.width_px,
            height_px: upload_bytes.height_px,
            format: upload_bytes.format,
        }
    }

    pub fn create_fallback(&self, device: &wgpu::Device) -> WgpuTerminalGlyphAtlasTexture {
        let size = wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        };
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("germinal.terminal.fallback_glyph_atlas_texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("germinal.terminal.fallback_glyph_atlas_texture_view"),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("germinal.terminal.fallback_glyph_atlas_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        WgpuTerminalGlyphAtlasTexture {
            texture,
            view,
            sampler,
            width_px: size.width,
            height_px: size.height,
            format,
        }
    }
}

pub struct WgpuTerminalGlyphAtlasTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub width_px: u32,
    pub height_px: u32,
    pub format: wgpu::TextureFormat,
}

impl WgpuTerminalGlyphAtlasTexture {
    pub fn size(&self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width_px,
            height: self.height_px,
            depth_or_array_layers: 1,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.width_px == 0 || self.height_px == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalGlyphAtlasTextureSpec {
    pub format: wgpu::TextureFormat,
    pub dimension: wgpu::TextureDimension,
    pub mip_level_count: u32,
    pub sample_count: u32,
    pub usage: wgpu::TextureUsages,
    pub sampler_mag_filter: wgpu::FilterMode,
    pub sampler_min_filter: wgpu::FilterMode,
}

impl WgpuTerminalGlyphAtlasTextureSpec {
    pub fn new() -> Self {
        Self {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            dimension: wgpu::TextureDimension::D2,
            mip_level_count: 1,
            sample_count: 1,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            sampler_mag_filter: wgpu::FilterMode::Nearest,
            sampler_min_filter: wgpu::FilterMode::Nearest,
        }
    }
}

impl Default for WgpuTerminalGlyphAtlasTextureSpec {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rendering::pty_surface::glyph_atlas::WgpuDebugGlyphAtlasBuilder;

    #[test]
    fn builds_upload_bytes_from_debug_glyph_atlas() {
        let atlas = WgpuDebugGlyphAtlasBuilder::new().build_for_texts(["red"]);

        let factory = WgpuTerminalGlyphAtlasTextureFactory::new();

        let upload_bytes = factory
            .build_upload_bytes(&atlas)
            .expect("upload bytes should exist");

        assert_eq!(upload_bytes.width_px, atlas.width_px);
        assert_eq!(upload_bytes.height_px, atlas.height_px);
        assert_eq!(upload_bytes.format, wgpu::TextureFormat::Rgba8UnormSrgb);
        assert_eq!(
            upload_bytes.byte_len(),
            (atlas.width_px * atlas.height_px * 4) as usize
        );
        assert_eq!(
            upload_bytes.expected_byte_len(),
            (atlas.width_px * atlas.height_px * 4) as usize
        );
        assert_eq!(upload_bytes.bytes_per_row(), atlas.width_px * 4);
        assert_eq!(upload_bytes.rows_per_image(), atlas.height_px);
        assert!(upload_bytes.is_tightly_packed());
        assert!(!upload_bytes.is_empty());
    }

    #[test]
    fn empty_atlas_has_no_upload_bytes() {
        let atlas = WgpuDebugGlyphAtlasBuilder::new().build_for_texts([""]);

        let factory = WgpuTerminalGlyphAtlasTextureFactory::new();

        assert!(factory.build_upload_bytes(&atlas).is_none());
    }

    #[test]
    fn texture_spec_matches_rgba_atlas() {
        let spec = WgpuTerminalGlyphAtlasTextureSpec::new();

        assert_eq!(spec.format, wgpu::TextureFormat::Rgba8UnormSrgb);
        assert_eq!(spec.dimension, wgpu::TextureDimension::D2);
        assert_eq!(spec.mip_level_count, 1);
        assert_eq!(spec.sample_count, 1);
        assert!(spec.usage.contains(wgpu::TextureUsages::TEXTURE_BINDING));
        assert!(spec.usage.contains(wgpu::TextureUsages::COPY_DST));
        assert_eq!(spec.sampler_mag_filter, wgpu::FilterMode::Nearest);
        assert_eq!(spec.sampler_min_filter, wgpu::FilterMode::Nearest);
    }
}
