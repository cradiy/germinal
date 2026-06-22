use germinal_ports::{
	pty_host::snapshot::TerminalSnapshotProvider,
	rendering::{
		frame_plan_builder::BuiltFramePlan,
		frame_plan_presenter::FramePlanPresenter,
		render_target_id::RenderTargetId,
		renderer_backend::RendererBackend,
		surface_snapshot::{RenderSurfaceSnapshot, RenderSurfaceSnapshotProvider},
	},
	seq::Seq,
};

#[derive(Debug)]
pub struct SurfacePresentationPipeline<P, S, R, T> {
	presenter:                  P,
	surface_snapshot_provider:  S,
	renderer_backend:           R,
	terminal_snapshot_provider: T,
}

impl<P, S, R, T> SurfacePresentationPipeline<P, S, R, T>
where
	P: FramePlanPresenter,
	S: RenderSurfaceSnapshotProvider,
	R: RendererBackend,
	T: TerminalSnapshotProvider,
{
	pub fn new(
		presenter: P,
		surface_snapshot_provider: S,
		renderer_backend: R,
		terminal_snapshot_provider: T,
	) -> Self {
		Self { presenter, surface_snapshot_provider, renderer_backend, terminal_snapshot_provider }
	}

	pub fn present_frame(&self, frame: &BuiltFramePlan) -> SurfacePresentationResult {
		self.presenter.present(frame);

		self.terminal_snapshot_provider.clear_damage_up_to(frame.target_id, frame.seq);

		let surface_snapshot = self.surface_snapshot_provider.surface_snapshot_of(frame.target_id);

		if let Some(snapshot) = &surface_snapshot {
			self.renderer_backend.render_surface(snapshot);
		}

		SurfacePresentationResult {
			target_id: frame.target_id,
			seq: frame.seq,
			rendered: surface_snapshot.is_some(),
			surface_snapshot,
		}
	}

	pub fn presenter(&self) -> &P { &self.presenter }

	pub fn renderer_backend(&self) -> &R { &self.renderer_backend }

	pub fn surface_snapshot_provider(&self) -> &S { &self.surface_snapshot_provider }

	pub fn terminal_snapshot_provider(&self) -> &T { &self.terminal_snapshot_provider }
}

#[derive(Debug, Clone)]
pub struct SurfacePresentationResult {
	pub target_id:        RenderTargetId,
	pub seq:              Seq,
	pub rendered:         bool,
	pub surface_snapshot: Option<RenderSurfaceSnapshot>,
}

impl SurfacePresentationResult {
	pub fn rendered(&self) -> bool { self.rendered }
}

#[cfg(test)]
mod tests {
	use std::{cell::RefCell, collections::HashMap};

	use germinal_ports::{
		pty_host::snapshot::{TerminalSnapshot, TerminalSnapshotProvider},
		rendering::{
			frame_plan_builder::{RenderCommandDto, TextStyleDto},
			surface_snapshot::{RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot},
		},
	};

	use super::*;

	#[derive(Debug, Default)]
	struct TestPresenter {
		presented: RefCell<Vec<BuiltFramePlan>>,
	}

	impl FramePlanPresenter for TestPresenter {
		fn present(&self, frame: &BuiltFramePlan) { self.presented.borrow_mut().push(frame.clone()); }
	}

	#[derive(Debug, Default)]
	struct TestSurfaceSnapshotProvider {
		snapshots: RefCell<HashMap<RenderTargetId, RenderSurfaceSnapshot>>,
	}

	impl TestSurfaceSnapshotProvider {
		fn insert(&self, snapshot: RenderSurfaceSnapshot) {
			self.snapshots.borrow_mut().insert(snapshot.target_id, snapshot);
		}
	}

	impl RenderSurfaceSnapshotProvider for TestSurfaceSnapshotProvider {
		fn surface_snapshot_of(&self, target_id: RenderTargetId) -> Option<RenderSurfaceSnapshot> {
			self.snapshots.borrow().get(&target_id).cloned()
		}
	}

	#[derive(Debug, Default)]
	struct TestRendererBackend {
		rendered: RefCell<Vec<RenderSurfaceSnapshot>>,
	}

	impl RendererBackend for TestRendererBackend {
		fn render_surface(&self, snapshot: &RenderSurfaceSnapshot) {
			self.rendered.borrow_mut().push(snapshot.clone());
		}
	}

	#[derive(Debug, Default)]
	struct TestTerminalSnapshotProvider {
		cleared: RefCell<Vec<(RenderTargetId, Seq)>>,
	}

	impl TerminalSnapshotProvider for TestTerminalSnapshotProvider {
		fn snapshot_of(&self, _render_target_id: RenderTargetId) -> Option<TerminalSnapshot> { None }

		fn clear_damage_up_to(&self, render_target_id: RenderTargetId, presented_seq: Seq) {
			self.cleared.borrow_mut().push((render_target_id, presented_seq));
		}
	}

	#[test]
	fn presents_frame_clears_damage_and_renders_surface_snapshot() {
		let target_id = RenderTargetId::new(1);
		let seq = Seq::new(9);

		let presenter = TestPresenter::default();
		let surface_provider = TestSurfaceSnapshotProvider::default();
		let renderer = TestRendererBackend::default();
		let terminal_provider = TestTerminalSnapshotProvider::default();

		surface_provider.insert(RenderSurfaceSnapshot {
			target_id,
			latest_seq: seq,
			rows: vec![RenderSurfaceRowSnapshot {
				y:    0,
				runs: vec![RenderSurfaceRunSnapshot {
					x:     0,
					text:  "hello".to_string(),
					style: TextStyleDto::plain(),
				}],
			}],
			dirty_rows: vec![0],
			cursor: None,
		});

		let pipeline =
			SurfacePresentationPipeline::new(presenter, surface_provider, renderer, terminal_provider);

		let frame = BuiltFramePlan {
			target_id,
			seq,
			commands: vec![RenderCommandDto::TextRun { x: 0, y: 0, text: "hello".to_string() }],
		};

		let result = pipeline.present_frame(&frame);

		assert!(result.rendered());
		assert_eq!(result.target_id, target_id);
		assert_eq!(result.seq, seq);

		assert_eq!(pipeline.presenter.presented.borrow().len(), 1);
		assert_eq!(pipeline.terminal_snapshot_provider.cleared.borrow().as_slice(), &[(
			target_id, seq
		)]);

		let rendered = pipeline.renderer_backend.rendered.borrow();

		assert_eq!(rendered.len(), 1);
		assert_eq!(rendered[0].target_id, target_id);
		assert_eq!(rendered[0].latest_seq, seq);
		assert_eq!(rendered[0].rows[0].runs[0].text, "hello");
	}
}
