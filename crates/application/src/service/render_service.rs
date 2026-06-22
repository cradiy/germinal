use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
	mpsc::{self, Receiver, Sender, TryRecvError},
};

use germinal_domain::{
	pty_host::{size_info::TerminalSizeInfo, window_size::TerminalWindowSize},
	rendering::render_target_id::RenderTargetId,
};
use germinal_ports::{
	rendering::{
		surface_snapshot::RenderSurfaceSnapshot,
		window_runtime::{IRenderRuntimeStore, ITerminalWindowRuntime},
	},
	service::render_service::IRenderService,
};

#[derive(kudi::DepInj)]
#[target(RenderService)]
pub struct RenderServiceState {
	redraw_pending:          bool,
	window_focused:          bool,
	focused_render_target:   Option<RenderTargetId>,
	latest_surface_snapshot: Option<RenderSurfaceSnapshot>,
	surface_snapshot_tx:     Sender<RenderSurfaceSnapshot>,
	surface_snapshot_rx:     Receiver<RenderSurfaceSnapshot>,
	snapshot_wake_pending:   Arc<AtomicBool>,
}

impl RenderServiceState {
	pub fn new() -> Self {
		let (surface_snapshot_tx, surface_snapshot_rx) = mpsc::channel::<RenderSurfaceSnapshot>();

		Self {
			redraw_pending: false,
			window_focused: true,
			focused_render_target: None,
			latest_surface_snapshot: None,
			surface_snapshot_tx,
			surface_snapshot_rx,
			snapshot_wake_pending: Arc::new(AtomicBool::new(false)),
		}
	}

	fn take_latest_surface_snapshot(&self) -> Option<RenderSurfaceSnapshot> {
		self.snapshot_wake_pending.store(false, Ordering::Release);

		let mut latest_snapshot = None;

		loop {
			match self.surface_snapshot_rx.try_recv() {
				Ok(snapshot) => latest_snapshot = Some(snapshot),
				Err(TryRecvError::Empty) => break,
				Err(TryRecvError::Disconnected) => break,
			}
		}

		latest_snapshot
	}

	fn set_window_focused(&mut self, focused: bool) -> bool {
		if self.window_focused == focused {
			return false;
		}

		self.window_focused = focused;
		true
	}

	fn set_focused_render_target(&mut self, target_id: RenderTargetId) -> bool {
		if self.focused_render_target == Some(target_id) {
			return false;
		}

		self.focused_render_target = Some(target_id);
		true
	}

	fn with_cursor_focus(&self, mut snapshot: RenderSurfaceSnapshot) -> RenderSurfaceSnapshot {
		if let Some(cursor) = snapshot.cursor.as_mut() {
			cursor.focused =
				self.window_focused && self.focused_render_target == Some(snapshot.target_id);
		}

		snapshot
	}

	fn request_redraw(&mut self) { self.redraw_pending = true; }
}

impl Default for RenderServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IRenderService for RenderService<Deps>
where Deps: AsRef<RenderServiceState> + AsMut<RenderServiceState> + IRenderRuntimeStore
{
	fn prepare_render_backend(&mut self) {
		let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
		state.request_redraw();
	}

	fn surface_snapshot_sender(&self) -> Sender<RenderSurfaceSnapshot> {
		let state: &RenderServiceState = self.prj_ref().as_ref();
		state.surface_snapshot_tx.clone()
	}

	fn snapshot_wake_pending(&self) -> Arc<AtomicBool> {
		let state: &RenderServiceState = self.prj_ref().as_ref();
		Arc::clone(&state.snapshot_wake_pending)
	}

	fn consume_latest_terminal_snapshot(&mut self) {
		let snapshot = {
			let state: &RenderServiceState = self.prj_ref().as_ref();
			state.take_latest_surface_snapshot()
		};

		let Some(snapshot) = snapshot else {
			return;
		};

		let snapshot = {
			let state: &RenderServiceState = self.prj_ref().as_ref();
			state.with_cursor_focus(snapshot)
		};

		self
			.prj_ref_mut()
			.window_runtime_mut()
			.expect("window runtime must be initialized before use")
			.set_surface_snapshot(snapshot.clone());

		let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
		state.latest_surface_snapshot = Some(snapshot);
		state.redraw_pending = true;
	}

	fn current_terminal_size_info(&self) -> TerminalSizeInfo {
		self
			.prj_ref()
			.window_runtime()
			.expect("window runtime must be initialized before use")
			.terminal_size_info()
	}

	fn resize_window_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
		let size_info = self
			.prj_ref_mut()
			.window_runtime_mut()
			.expect("window runtime must be initialized before use")
			.resize_surface_size_info(window_size);

		self.prj_ref_mut().as_mut().redraw_pending = true;

		size_info
	}

	fn set_window_focused(&mut self, focused: bool) {
		let changed = {
			let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
			state.set_window_focused(focused)
		};

		if !changed {
			return;
		}

		refresh_cursor_focus(self.prj_ref_mut());
	}

	fn set_focused_render_target(&mut self, target_id: RenderTargetId) {
		let changed = {
			let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
			state.set_focused_render_target(target_id)
		};

		if !changed {
			return;
		}

		refresh_cursor_focus(self.prj_ref_mut());
	}

	fn request_redraw(&mut self) {
		let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
		state.request_redraw();
	}

	fn flush_redraw_request(&mut self) {
		let should_request = {
			let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
			let should_request = state.redraw_pending;
			state.redraw_pending = false;
			should_request
		};

		let runtime = self
			.prj_ref_mut()
			.window_runtime_mut()
			.expect("window runtime must be initialized before use");

		let should_request = should_request || runtime.take_redraw_request();
		if should_request {
			runtime.request_window_redraw();
		}
	}

	fn present_workspace(&mut self) {
		self
			.prj_ref_mut()
			.window_runtime_mut()
			.expect("window runtime must be initialized before use")
			.render();
	}
}

fn refresh_cursor_focus<Deps>(deps: &mut Deps)
where Deps: AsRef<RenderServiceState> + AsMut<RenderServiceState> + IRenderRuntimeStore {
	let Some(snapshot) = ({
		let state: &RenderServiceState = deps.as_ref();
		state.latest_surface_snapshot.clone()
	}) else {
		return;
	};

	let snapshot = {
		let state: &RenderServiceState = deps.as_ref();
		state.with_cursor_focus(snapshot)
	};

	deps
		.window_runtime_mut()
		.expect("window runtime must be initialized before use")
		.set_surface_snapshot(snapshot.clone());

	let state: &mut RenderServiceState = deps.as_mut();
	state.latest_surface_snapshot = Some(snapshot);
	state.redraw_pending = true;
}
