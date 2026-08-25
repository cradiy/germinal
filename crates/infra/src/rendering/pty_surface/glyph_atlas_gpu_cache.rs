use std::{cell::RefCell, collections::HashMap, sync::Arc};

use germinal_ports::rendering::render_target_id::RenderTargetId;

use crate::rendering::pty_surface::{
    glyph_atlas::WgpuTerminalGlyphAtlas,
    glyph_atlas_bind_group::{
        WgpuTerminalGlyphAtlasBindGroup, WgpuTerminalGlyphAtlasBindGroupFactory,
    },
    glyph_atlas_frame::{WgpuTerminalGlyphAtlasFrame, WgpuTerminalGlyphAtlasSourceKind},
    glyph_atlas_texture::{WgpuTerminalGlyphAtlasTexture, WgpuTerminalGlyphAtlasTextureFactory},
};

#[derive(Clone)]
pub struct WgpuTerminalGlyphAtlasGpuCache {
    inner: RefCell<HashMap<RenderTargetId, WgpuTerminalGlyphAtlasGpuCacheEntry>>,
}

impl WgpuTerminalGlyphAtlasGpuCache {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(HashMap::new()),
        }
    }

    pub fn remove_render_target(&self, target_id: RenderTargetId) -> bool {
        self.inner.borrow_mut().remove(&target_id).is_some()
    }

    pub fn get_or_upload(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        glyph_atlas_bind_group_layout: &wgpu::BindGroupLayout,
        glyph_atlas_frame: &WgpuTerminalGlyphAtlasFrame,
    ) -> WgpuTerminalGlyphAtlasGpuCacheResult {
        if !glyph_atlas_frame.has_upload_work() {
            return WgpuTerminalGlyphAtlasGpuCacheResult {
                texture: None,
                bind_group: None,
                cache_hit: false,
            };
        }

        {
            let cache = self.inner.borrow();

            if let Some(entry) = cache.get(&glyph_atlas_frame.target_id)
                && entry.source == glyph_atlas_frame.source
                && Arc::ptr_eq(&entry.atlas, &glyph_atlas_frame.atlas)
            {
                return WgpuTerminalGlyphAtlasGpuCacheResult {
                    texture: Some(Arc::clone(&entry.texture)),
                    bind_group: Some(Arc::clone(&entry.bind_group)),
                    cache_hit: true,
                };
            }
        }

        let Some(upload_bytes) = glyph_atlas_frame.upload_bytes.as_ref() else {
            return WgpuTerminalGlyphAtlasGpuCacheResult {
                texture: None,
                bind_group: None,
                cache_hit: false,
            };
        };
        let texture_factory = WgpuTerminalGlyphAtlasTextureFactory::new();
        let texture = texture_factory.upload_bytes(device, queue, upload_bytes);

        let texture = Arc::new(texture);

        let bind_group_factory = WgpuTerminalGlyphAtlasBindGroupFactory::new();

        let bind_group = Arc::new(bind_group_factory.create_bind_group(
            device,
            glyph_atlas_bind_group_layout,
            texture.as_ref(),
        ));

        {
            let mut cache = self.inner.borrow_mut();

            cache.insert(
                glyph_atlas_frame.target_id,
                WgpuTerminalGlyphAtlasGpuCacheEntry {
                    source: glyph_atlas_frame.source,
                    atlas: Arc::clone(&glyph_atlas_frame.atlas),
                    texture: Arc::clone(&texture),
                    bind_group: Arc::clone(&bind_group),
                },
            );
        }

        WgpuTerminalGlyphAtlasGpuCacheResult {
            texture: Some(texture),
            bind_group: Some(bind_group),
            cache_hit: false,
        }
    }
}

impl Default for WgpuTerminalGlyphAtlasGpuCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WgpuTerminalGlyphAtlasGpuCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let entry_count = self.inner.borrow().len();

        f.debug_struct("WgpuTerminalGlyphAtlasGpuCache")
            .field("entry_count", &entry_count)
            .finish()
    }
}

#[derive(Clone)]
pub struct WgpuTerminalGlyphAtlasGpuCacheResult {
    pub texture: Option<Arc<WgpuTerminalGlyphAtlasTexture>>,
    pub bind_group: Option<Arc<WgpuTerminalGlyphAtlasBindGroup>>,
    pub cache_hit: bool,
}

impl WgpuTerminalGlyphAtlasGpuCacheResult {
    pub fn has_gpu_resources(&self) -> bool {
        self.texture.is_some() && self.bind_group.is_some()
    }
}

#[derive(Clone)]
struct WgpuTerminalGlyphAtlasGpuCacheEntry {
    source: WgpuTerminalGlyphAtlasSourceKind,
    atlas: Arc<WgpuTerminalGlyphAtlas>,
    texture: Arc<WgpuTerminalGlyphAtlasTexture>,
    bind_group: Arc<WgpuTerminalGlyphAtlasBindGroup>,
}
