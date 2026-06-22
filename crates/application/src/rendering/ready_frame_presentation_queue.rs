use std::collections::HashMap;

use germinal_ports::{
	pty_host::snapshot::TerminalSnapshotProvider,
	rendering::{
		frame_plan_builder::BuiltFramePlan, frame_plan_presenter::FramePlanPresenter,
		render_target_id::RenderTargetId, renderer_backend::RendererBackend,
		surface_snapshot::RenderSurfaceSnapshotProvider,
	},
	seq::Seq,
};

use crate::rendering::surface_presentation_pipeline::SurfacePresentationPipeline;

#[derive(Debug, Default)]
pub struct ReadyFramePresentationQueue {
	frames: HashMap<RenderTargetId, BuiltFramePlan>,
}

impl ReadyFramePresentationQueue {
	pub fn new() -> Self { Self::default() }

	pub fn enqueue(&mut self, frame: BuiltFramePlan) -> ReadyFrameEnqueueResult {
		match self.frames.get(&frame.target_id) {
			Some(existing) if existing.seq >= frame.seq => ReadyFrameEnqueueResult {
				accepted:     false,
				replaced_seq: None,
				queued_seq:   existing.seq,
			},
			Some(existing) => {
				let replaced_seq = existing.seq;

				self.frames.insert(frame.target_id, frame.clone());

				ReadyFrameEnqueueResult {
					accepted:     true,
					replaced_seq: Some(replaced_seq),
					queued_seq:   frame.seq,
				}
			}
			None => {
				let queued_seq = frame.seq;

				self.frames.insert(frame.target_id, frame);

				ReadyFrameEnqueueResult { accepted: true, replaced_seq: None, queued_seq }
			}
		}
	}

	pub fn present_latest<P, S, R, T, M>(
		&mut self,
		mut mark_presented: M,
		presentation_pipeline: &SurfacePresentationPipeline<P, S, R, T>,
	) -> ReadyFramePresentationSummary
	where
		P: FramePlanPresenter,
		S: RenderSurfaceSnapshotProvider,
		R: RendererBackend,
		T: TerminalSnapshotProvider,
		M: FnMut(RenderTargetId, Seq) -> bool,
	{
		let frames = std::mem::take(&mut self.frames);
		let mut summary = ReadyFramePresentationSummary {
			queued_before: frames.len() as u64,
			..ReadyFramePresentationSummary::default()
		};

		for (_, frame) in frames {
			if !mark_presented(frame.target_id, frame.seq) {
				summary.skipped += 1;
				continue;
			}

			let result = presentation_pipeline.present_frame(&frame);

			summary.presented += 1;

			if result.rendered() {
				summary.rendered += 1;
			}
		}

		summary
	}

	pub fn queued_seq_of(&self, target_id: RenderTargetId) -> Option<Seq> {
		self.frames.get(&target_id).map(|frame| frame.seq)
	}

	pub fn len(&self) -> usize { self.frames.len() }

	pub fn is_empty(&self) -> bool { self.frames.is_empty() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadyFrameEnqueueResult {
	pub accepted:     bool,
	pub replaced_seq: Option<Seq>,
	pub queued_seq:   Seq,
}

impl ReadyFrameEnqueueResult {
	pub fn accepted(&self) -> bool { self.accepted }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadyFramePresentationSummary {
	pub queued_before: u64,
	pub presented:     u64,
	pub rendered:      u64,
	pub skipped:       u64,
}

#[cfg(test)]
mod tests {
	use std::{cell::RefCell, collections::HashMap};

	use germinal_ports::{
		pty_host::snapshot::TerminalSnapshot,
		rendering::{
			frame_plan_builder::{RenderCommandDto, TextStyleDto},
			surface_snapshot::{
				RenderSurfaceRowSnapshot, RenderSurfaceRunSnapshot, RenderSurfaceSnapshot,
			},
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
	fn enqueue_keeps_latest_frame_per_target() {
		let target_id = RenderTargetId::new(1);
		let mut queue = ReadyFramePresentationQueue::new();

		let first = queue.enqueue(test_frame(target_id, Seq::new(1), "old"));
		assert!(first.accepted());
		assert_eq!(first.replaced_seq, None);
		assert_eq!(first.queued_seq, Seq::new(1));

		let newer = queue.enqueue(test_frame(target_id, Seq::new(3), "new"));
		assert!(newer.accepted());
		assert_eq!(newer.replaced_seq, Some(Seq::new(1)));
		assert_eq!(newer.queued_seq, Seq::new(3));

		let stale = queue.enqueue(test_frame(target_id, Seq::new(2), "stale"));
		assert!(!stale.accepted());
		assert_eq!(stale.replaced_seq, None);
		assert_eq!(stale.queued_seq, Seq::new(3));

		assert_eq!(queue.len(), 1);
		assert_eq!(queue.queued_seq_of(target_id), Some(Seq::new(3)));
	}

	#[test]
	fn present_latest_marks_presents_clears_damage_and_renders() {
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
			cursor: None,
		});

		let presentation_pipeline =
			SurfacePresentationPipeline::new(presenter, surface_provider, renderer, terminal_provider);

		let mut queue = ReadyFramePresentationQueue::new();
		queue.enqueue(test_frame(target_id, seq, "hello"));

		let mut marked = Vec::new();

		let summary = queue.present_latest(
			|target_id, seq| {
				marked.push((target_id, seq));
				true
			},
			&presentation_pipeline,
		);

		assert_eq!(summary, ReadyFramePresentationSummary {
			queued_before: 1,
			presented:     1,
			rendered:      1,
			skipped:       0,
		});

		assert!(queue.is_empty());
		assert_eq!(marked, vec![(target_id, seq)]);

		assert_eq!(presentation_pipeline.presenter().presented.borrow().len(), 1);

		assert_eq!(presentation_pipeline.terminal_snapshot_provider().cleared.borrow().as_slice(), &[
			(target_id, seq)
		]);

		let rendered = presentation_pipeline.renderer_backend().rendered.borrow();

		assert_eq!(rendered.len(), 1);
		assert_eq!(rendered[0].target_id, target_id);
		assert_eq!(rendered[0].latest_seq, seq);
		assert_eq!(rendered[0].rows[0].runs[0].text, "hello");
	}

	#[test]
	fn present_latest_skips_when_mark_presented_rejects_frame() {
		let target_id = RenderTargetId::new(1);
		let seq = Seq::new(9);

		let presenter = TestPresenter::default();
		let surface_provider = TestSurfaceSnapshotProvider::default();
		let renderer = TestRendererBackend::default();
		let terminal_provider = TestTerminalSnapshotProvider::default();

		let presentation_pipeline =
			SurfacePresentationPipeline::new(presenter, surface_provider, renderer, terminal_provider);

		let mut queue = ReadyFramePresentationQueue::new();
		queue.enqueue(test_frame(target_id, seq, "hello"));

		let summary = queue.present_latest(|_target_id, _seq| false, &presentation_pipeline);

		assert_eq!(summary, ReadyFramePresentationSummary {
			queued_before: 1,
			presented:     0,
			rendered:      0,
			skipped:       1,
		});

		assert!(queue.is_empty());

		assert_eq!(presentation_pipeline.presenter().presented.borrow().len(), 0);

		assert_eq!(presentation_pipeline.renderer_backend().rendered.borrow().len(), 0);

		assert_eq!(presentation_pipeline.terminal_snapshot_provider().cleared.borrow().len(), 0);
	}

	fn test_frame(target_id: RenderTargetId, seq: Seq, text: &str) -> BuiltFramePlan {
		BuiltFramePlan {
			target_id,
			seq,
			commands: vec![RenderCommandDto::TextRun { x: 0, y: 0, text: text.to_string() }],
		}
	}
}
