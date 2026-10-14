use germinal_ports::rendering::{
    frame_plan_builder::RgbColorDto, surface_snapshot::RenderSurfaceImageSnapshot,
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, thiserror::Error)]
pub enum SvgIconError {
    #[error("SVG icon size must be between 1 and 512 pixels")]
    InvalidSize,
    #[error("invalid SVG icon: {0}")]
    Parse(#[from] resvg::usvg::Error),
    #[error("failed to allocate SVG icon pixels")]
    Allocation,
}

#[derive(Clone)]
pub struct SvgIcon {
    size: u32,
    generation: u64,
    rgba: Arc<[u8]>,
}

impl SvgIcon {
    pub fn placement(&self, id: &str, x_px: u32, y_px: u32) -> RenderSurfaceImageSnapshot {
        RenderSurfaceImageSnapshot {
            id: id.into(),
            image_id: 0,
            image_generation: self.generation,
            x_cell: 0,
            y_cell: 0,
            x_offset_px: x_px,
            y_offset_px: y_px,
            columns: 0,
            rows: 0,
            source_x_px: 0,
            source_y_px: 0,
            source_width_px: self.size,
            source_height_px: self.size,
            image_width_px: self.size,
            image_height_px: self.size,
            clip_top_cell: None,
            clip_bottom_cell: None,
            z_index: 0,
            rgba: Arc::clone(&self.rgba),
        }
    }
}

#[derive(Default)]
pub struct SvgIconCache {
    trees: VecDeque<(&'static str, resvg::usvg::Tree)>,
    rasters: VecDeque<(&'static str, u32, Option<RgbColorDto>, SvgIcon)>,
}

impl SvgIconCache {
    pub fn rasterize(
        &mut self,
        source: &'static str,
        size: u32,
        tint: Option<RgbColorDto>,
    ) -> Result<SvgIcon, SvgIconError> {
        if !(1..=512).contains(&size) {
            return Err(SvgIconError::InvalidSize);
        }
        if let Some(index) = self
            .rasters
            .iter()
            .position(|(svg, pixels, color, _)| *svg == source && *pixels == size && *color == tint)
        {
            let entry = self.rasters.remove(index).expect("cache entry exists");
            let icon = entry.3.clone();
            self.rasters.push_back(entry);
            return Ok(icon);
        }
        if !self.trees.iter().any(|(svg, _)| *svg == source) {
            let options = resvg::usvg::Options {
                image_href_resolver: resvg::usvg::ImageHrefResolver {
                    resolve_data: Box::new(|_, _, _| None),
                    resolve_string: Box::new(|_, _| None),
                },
                ..Default::default()
            };
            let tree = resvg::usvg::Tree::from_str(source, &options)?;
            if self.trees.len() == 32 {
                self.trees.pop_front();
            }
            self.trees.push_back((source, tree));
        }
        let tree = &self
            .trees
            .iter()
            .find(|(svg, _)| *svg == source)
            .expect("parsed SVG exists")
            .1;
        let mut pixmap =
            resvg::tiny_skia::Pixmap::new(size, size).ok_or(SvgIconError::Allocation)?;
        let scale = (size as f32 / tree.size().width()).min(size as f32 / tree.size().height());
        let transform = resvg::tiny_skia::Transform::from_row(
            scale,
            0.0,
            0.0,
            scale,
            (size as f32 - tree.size().width() * scale) / 2.0,
            (size as f32 - tree.size().height() * scale) / 2.0,
        );
        resvg::render(tree, transform, &mut pixmap.as_mut());
        let mut bytes = pixmap.take();
        // tiny-skia emits premultiplied pixels; the shared image renderer expects straight RGBA.
        for pixel in bytes.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            if alpha == 0 {
                continue;
            }
            if let Some(color) = tint {
                pixel[..3].copy_from_slice(&[color.red, color.green, color.blue]);
            } else {
                for channel in &mut pixel[..3] {
                    *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
        let icon = SvgIcon {
            size,
            generation: GENERATION.fetch_add(1, Ordering::Relaxed),
            rgba: bytes.into(),
        };
        if self.rasters.len() == 64 {
            self.rasters.pop_front();
        }
        self.rasters.push_back((source, size, tint, icon.clone()));
        Ok(icon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><path fill="#ff0000" opacity="0.5" d="M0 0H20V10H0Z"/></svg>"##;
    #[test]
    fn preserves_color_alpha_aspect_ratio_and_reuses_size_and_tint_cache() {
        let mut cache = SvgIconCache::default();
        let icon = cache.rasterize(SVG, 20, None).unwrap();
        assert_eq!(&icon.rgba[..4], &[0, 0, 0, 0]);
        let center = (10 * 20 + 10) * 4;
        assert_eq!(icon.rgba[center], 255);
        assert!((127..=128).contains(&icon.rgba[center + 3]));
        let cached = cache.rasterize(SVG, 20, None).unwrap();
        assert!(Arc::ptr_eq(&icon.rgba, &cached.rgba));
        assert_eq!(icon.generation, cached.generation);
        let tinted = cache
            .rasterize(SVG, 20, Some(RgbColorDto::new(10, 20, 30)))
            .unwrap();
        assert_eq!(&tinted.rgba[center..center + 3], &[10, 20, 30]);
        assert_ne!(icon.generation, tinted.generation);
        assert_ne!(
            icon.generation,
            cache.rasterize(SVG, 40, None).unwrap().generation
        );
        assert_eq!(cache.trees.len(), 1);
        assert!(cache.rasterize(SVG, 513, None).is_err());
        assert!(cache.rasterize("invalid", 20, None).is_err());
    }
}
