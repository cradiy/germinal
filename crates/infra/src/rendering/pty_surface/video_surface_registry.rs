use std::{
	cell::RefCell,
	collections::{HashMap, HashSet},
	rc::Rc,
};

use germinal_ports::{
	rendering::{
		render_target_id::RenderTargetId,
		surface_snapshot::{RenderSurfaceSnapshot, RenderSurfaceVideoSurfaceSnapshot},
	},
	seq::Seq,
};

#[cfg(target_os = "linux")]
use crate::rendering::pty_surface::video_surface_frame::WgpuVideoSurfaceNv12DmaBufFrame;
use crate::rendering::pty_surface::video_surface_frame::{
	WgpuVideoSurfaceFrame, WgpuVideoSurfaceNv12GpuFrame,
};

#[derive(Debug, Clone, Default)]
pub struct WgpuVideoSurfaceRegistry {
	inner: Rc<RefCell<HashMap<WgpuVideoSurfaceKey, WgpuVideoSurfaceState>>>,
}

impl WgpuVideoSurfaceRegistry {
	pub fn remove_render_target(&self, target_id: RenderTargetId) -> bool {
		let mut inner = self.inner.borrow_mut();
		let old_len = inner.len();
		inner.retain(|key, _| key.target_id != target_id);
		inner.len() != old_len
	}

	pub fn sync_snapshot(&self, snapshot: &RenderSurfaceSnapshot) {
		let mut inner = self.inner.borrow_mut();
		let mut live_keys = HashSet::new();

		for surface in &snapshot.video_surfaces {
			let key =
				WgpuVideoSurfaceKey { target_id: snapshot.target_id, id: surface.id.clone() };
			live_keys.insert(key.clone());
			let registration = WgpuVideoSurfaceRegistration {
				key:        key.clone(),
				latest_seq: snapshot.latest_seq,
				surface:    surface.clone(),
			};

			match inner.get_mut(&key) {
				Some(state) => state.registration = registration,
				None => {
					inner.insert(key, WgpuVideoSurfaceState { registration, frame: None });
				}
			}
		}

		inner.retain(|key, _| key.target_id != snapshot.target_id || live_keys.contains(key));
	}

	pub fn registration(
		&self,
		target_id: RenderTargetId,
		id: &str,
	) -> Option<WgpuVideoSurfaceRegistration> {
		self
			.inner
			.borrow()
			.get(&WgpuVideoSurfaceKey { target_id, id: id.to_string() })
			.map(|state| state.registration.clone())
	}

	pub fn registrations_for_target(
		&self,
		target_id: RenderTargetId,
	) -> Vec<WgpuVideoSurfaceRegistration> {
		self
			.inner
			.borrow()
			.values()
			.filter(|state| state.registration.key.target_id == target_id)
			.map(|state| state.registration.clone())
			.collect()
	}

	pub fn replace_nv12_frame(
		&self,
		target_id: RenderTargetId,
		id: &str,
		frame: WgpuVideoSurfaceNv12GpuFrame,
	) -> bool {
		self.replace_frame(target_id, id, WgpuVideoSurfaceFrame::Nv12Gpu(frame))
	}

	#[cfg(target_os = "linux")]
	pub fn replace_nv12_dma_buf_frame(
		&self,
		target_id: RenderTargetId,
		id: &str,
		frame: WgpuVideoSurfaceNv12DmaBufFrame,
	) -> bool {
		self.replace_frame(target_id, id, WgpuVideoSurfaceFrame::Nv12DmaBuf(frame))
	}

	pub fn replace_frame(
		&self,
		target_id: RenderTargetId,
		id: &str,
		frame: WgpuVideoSurfaceFrame,
	) -> bool {
		let mut inner = self.inner.borrow_mut();
		let Some(state) = inner.get_mut(&WgpuVideoSurfaceKey { target_id, id: id.to_string() }) else {
			return false;
		};
		state.frame = Some(frame);
		true
	}

	pub fn clear_frame(&self, target_id: RenderTargetId, id: &str) -> bool {
		let mut inner = self.inner.borrow_mut();
		let Some(state) = inner.get_mut(&WgpuVideoSurfaceKey { target_id, id: id.to_string() }) else {
			return false;
		};
		let had_frame = state.frame.is_some();
		state.frame = None;
		had_frame
	}

	pub fn attached_frame(
		&self,
		target_id: RenderTargetId,
		id: &str,
	) -> Option<WgpuVideoSurfaceFrame> {
		self
			.inner
			.borrow()
			.get(&WgpuVideoSurfaceKey { target_id, id: id.to_string() })
			.and_then(|state| state.frame.clone())
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WgpuVideoSurfaceKey {
	pub target_id: RenderTargetId,
	pub id:        String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgpuVideoSurfaceRegistration {
	pub key:        WgpuVideoSurfaceKey,
	pub latest_seq: Seq,
	pub surface:    RenderSurfaceVideoSurfaceSnapshot,
}

#[derive(Debug, Clone)]
struct WgpuVideoSurfaceState {
	registration: WgpuVideoSurfaceRegistration,
	frame:        Option<WgpuVideoSurfaceFrame>,
}

#[cfg(test)]
mod tests {
	use germinal_ports::rendering::surface_snapshot::RenderSurfaceSnapshot;

	use super::*;

	#[test]
	fn sync_snapshot_tracks_and_replaces_target_video_surfaces() {
		let registry = WgpuVideoSurfaceRegistry::default();
		let target_id = RenderTargetId::new(7);

		registry.sync_snapshot(&RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![],
			video_surfaces: vec![
				RenderSurfaceVideoSurfaceSnapshot {
					id:        "left".to_string(),
					x_px:      10,
					y_px:      20,
					width_px:  30,
					height_px: 40,
				},
				RenderSurfaceVideoSurfaceSnapshot {
					id:        "right".to_string(),
					x_px:      50,
					y_px:      60,
					width_px:  70,
					height_px: 80,
				},
			],
			dirty_rows: vec![],
			cursor: None,
		});

		assert_eq!(registry.registrations_for_target(target_id).len(), 2);
		assert_eq!(
			registry.registration(target_id, "left").map(|registration| registration.surface.x_px),
			Some(10)
		);

		registry.sync_snapshot(&RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(2),
			rows: vec![],
			video_surfaces: vec![RenderSurfaceVideoSurfaceSnapshot {
				id:        "right".to_string(),
				x_px:      5,
				y_px:      6,
				width_px:  7,
				height_px: 8,
			}],
			dirty_rows: vec![],
			cursor: None,
		});

		assert!(registry.registration(target_id, "left").is_none());
		assert_eq!(
			registry.registration(target_id, "right").map(|registration| registration.latest_seq),
			Some(Seq::new(2))
		);
		assert_eq!(registry.registrations_for_target(target_id).len(), 1);
	}

	#[test]
	fn removing_a_target_drops_all_of_its_video_surfaces() {
		let registry = WgpuVideoSurfaceRegistry::default();
		let target_id = RenderTargetId::new(7);
		registry.sync_snapshot(&RenderSurfaceSnapshot {
			target_id,
			latest_seq: Seq::new(1),
			rows: vec![],
			video_surfaces: vec![RenderSurfaceVideoSurfaceSnapshot {
				id:        "video".to_string(),
				x_px:      0,
				y_px:      0,
				width_px:  10,
				height_px: 10,
			}],
			dirty_rows: vec![],
			cursor: None,
		});

		assert!(registry.remove_render_target(target_id));
		assert!(registry.registrations_for_target(target_id).is_empty());
		assert!(!registry.remove_render_target(target_id));
	}
}
