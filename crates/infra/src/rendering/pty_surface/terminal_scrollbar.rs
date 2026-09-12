use germinal_ports::rendering::surface_snapshot::RenderSurfaceScrollbackSnapshot;

const SCROLLBAR_WIDTH_PX: u32 = 9;
const SCROLLBAR_RIGHT_MARGIN_PX: u32 = 3;
const SCROLLBAR_VERTICAL_MARGIN_PX: u32 = 4;
const SCROLLBAR_HIT_WIDTH_PX: u32 = 12;
const SCROLLBAR_MIN_THUMB_PX: u32 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WgpuTerminalScrollbarGeometry {
    pub track_x_px: u32,
    pub track_y_px: u32,
    pub track_width_px: u32,
    pub track_height_px: u32,
    pub thumb_y_px: u32,
    pub thumb_height_px: u32,
    pub display_offset: u32,
    pub history_size: u32,
}

impl WgpuTerminalScrollbarGeometry {
    pub(crate) fn new(
        scrollback: RenderSurfaceScrollbackSnapshot,
        content_width_px: u32,
        content_height_px: u32,
        visible_rows: u32,
    ) -> Option<Self> {
        if scrollback.history_size == 0
            || content_width_px < SCROLLBAR_WIDTH_PX
            || content_height_px <= SCROLLBAR_VERTICAL_MARGIN_PX.saturating_mul(2)
        {
            return None;
        }

        let history_size = scrollback.history_size;
        let display_offset = scrollback.display_offset.min(history_size);
        let track_height_px = content_height_px
            .saturating_sub(SCROLLBAR_VERTICAL_MARGIN_PX.saturating_mul(2))
            .max(1);
        let total_rows = history_size.saturating_add(visible_rows.max(1));
        let proportional_thumb = ((u64::from(track_height_px) * u64::from(visible_rows.max(1)))
            / u64::from(total_rows.max(1))) as u32;
        let thumb_height_px = proportional_thumb
            .max(SCROLLBAR_MIN_THUMB_PX.min(track_height_px))
            .min(track_height_px);
        let travel_px = track_height_px.saturating_sub(thumb_height_px);
        let rows_from_top = history_size.saturating_sub(display_offset);
        let thumb_offset_px = ((u64::from(travel_px) * u64::from(rows_from_top)
            + u64::from(history_size / 2))
            / u64::from(history_size)) as u32;

        Some(Self {
            track_x_px: content_width_px
                .saturating_sub(SCROLLBAR_RIGHT_MARGIN_PX)
                .saturating_sub(SCROLLBAR_WIDTH_PX),
            track_y_px: SCROLLBAR_VERTICAL_MARGIN_PX,
            track_width_px: SCROLLBAR_WIDTH_PX,
            track_height_px,
            thumb_y_px: SCROLLBAR_VERTICAL_MARGIN_PX.saturating_add(thumb_offset_px),
            thumb_height_px,
            display_offset,
            history_size,
        })
    }

    pub(crate) fn visible(self) -> bool {
        self.display_offset > 0
    }

    pub(crate) fn hit_test(self, x_px: f64, y_px: f64, content_width_px: u32) -> bool {
        x_px.is_finite()
            && y_px.is_finite()
            && x_px >= f64::from(content_width_px.saturating_sub(SCROLLBAR_HIT_WIDTH_PX))
            && x_px < f64::from(content_width_px)
            && y_px >= f64::from(self.track_y_px)
            && y_px < f64::from(self.track_y_px.saturating_add(self.track_height_px))
    }

    pub(crate) fn thumb_contains(self, y_px: f64) -> bool {
        y_px >= f64::from(self.thumb_y_px)
            && y_px < f64::from(self.thumb_y_px.saturating_add(self.thumb_height_px))
    }

    fn display_offset_for_pointer(self, pointer_y_px: f64, grab_offset_px: f64) -> u32 {
        let travel_px = self.track_height_px.saturating_sub(self.thumb_height_px);
        if travel_px == 0 {
            return self.display_offset;
        }

        let thumb_top = (pointer_y_px - grab_offset_px).clamp(
            f64::from(self.track_y_px),
            f64::from(self.track_y_px.saturating_add(travel_px)),
        ) - f64::from(self.track_y_px);
        let rows_from_top = (thumb_top * f64::from(self.history_size) / f64::from(travel_px))
            .round()
            .clamp(0.0, f64::from(self.history_size)) as u32;
        self.history_size.saturating_sub(rows_from_top)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WgpuTerminalScrollbarDrag {
    geometry: WgpuTerminalScrollbarGeometry,
    grab_offset_px: f64,
    predicted_display_offset: u32,
}

impl WgpuTerminalScrollbarDrag {
    pub(crate) fn begin(
        geometry: WgpuTerminalScrollbarGeometry,
        pointer_y_px: f64,
    ) -> (Self, Option<i32>) {
        let grab_offset_px = if geometry.thumb_contains(pointer_y_px) {
            pointer_y_px - f64::from(geometry.thumb_y_px)
        } else {
            f64::from(geometry.thumb_height_px) / 2.0
        };
        let mut drag = Self {
            geometry,
            grab_offset_px,
            predicted_display_offset: geometry.display_offset,
        };
        let delta = drag.update(pointer_y_px);
        (drag, delta)
    }

    pub(crate) fn update(&mut self, pointer_y_px: f64) -> Option<i32> {
        if !pointer_y_px.is_finite() {
            return None;
        }
        let next = self
            .geometry
            .display_offset_for_pointer(pointer_y_px, self.grab_offset_px);
        let delta = i64::from(next) - i64::from(self.predicted_display_offset);
        self.predicted_display_offset = next;
        (delta != 0).then_some(delta.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(display_offset: u32) -> WgpuTerminalScrollbarGeometry {
        WgpuTerminalScrollbarGeometry::new(
            RenderSurfaceScrollbackSnapshot {
                display_offset,
                history_size: 900,
            },
            800,
            600,
            100,
        )
        .unwrap()
    }

    #[test]
    fn thumb_maps_top_and_bottom_scrollback_positions_to_track_ends() {
        let top = geometry(900);
        let bottom = geometry(0);

        assert_eq!(top.thumb_y_px, top.track_y_px);
        assert_eq!(
            bottom.thumb_y_px.saturating_add(bottom.thumb_height_px),
            bottom.track_y_px.saturating_add(bottom.track_height_px)
        );
        assert!(top.visible());
        assert!(!bottom.visible());
    }

    #[test]
    fn track_click_and_drag_produce_absolute_viewport_deltas() {
        let geometry = geometry(450);
        let track_bottom = f64::from(geometry.track_y_px + geometry.track_height_px - 1);
        let (mut drag, initial_delta) = WgpuTerminalScrollbarDrag::begin(geometry, track_bottom);

        assert_eq!(initial_delta, Some(-450));
        assert_eq!(drag.update(track_bottom), None);
        assert_eq!(drag.update(f64::from(geometry.track_y_px)), Some(900));
    }

    #[test]
    fn hit_target_is_wider_than_the_drawn_thumb() {
        let geometry = geometry(450);
        let y = f64::from(geometry.thumb_y_px);

        assert!(geometry.hit_test(789.0, y, 800));
        assert!(!geometry.hit_test(787.0, y, 800));
    }
}
