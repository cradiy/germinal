use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
	mpsc::{self, Receiver, Sender, TryRecvError},
};

use germinal_domain::pty_host::{size_info::TerminalSizeInfo, window_size::TerminalWindowSize};
use germinal_infra::rendering::pty_surface::window_runtime::WgpuTerminalWindowRuntime;
use germinal_ports::{
	rendering::surface_snapshot::RenderSurfaceSnapshot, service::render_service::IRenderService,
};
use winit::{
	dpi::LogicalSize,
	event_loop::ActiveEventLoop,
	window::{Window, WindowId},
};

struct WindowRuntimeState {
	runtime: WgpuTerminalWindowRuntime,
}

#[derive(kudi::DepInj)]
#[target(RenderService)]
pub struct RenderServiceState {
	runtime:               Option<WindowRuntimeState>,
	window_size:           TerminalWindowSize,
	redraw_pending:        bool,
	surface_snapshot_tx:   Sender<RenderSurfaceSnapshot>,
	surface_snapshot_rx:   Receiver<RenderSurfaceSnapshot>,
	snapshot_wake_pending: Arc<AtomicBool>,
}

impl RenderServiceState {
	pub fn new() -> Self {
		let (surface_snapshot_tx, surface_snapshot_rx) = mpsc::channel::<RenderSurfaceSnapshot>();

		Self {
			runtime: None,
			window_size: TerminalWindowSize::new(960, 540),
			redraw_pending: false,
			surface_snapshot_tx,
			surface_snapshot_rx,
			snapshot_wake_pending: Arc::new(AtomicBool::new(false)),
		}
	}

	pub fn ensure_window_runtime(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
		if self.runtime.is_some() {
			return Ok(());
		}

		let window = Arc::new(
			event_loop
				.create_window(
					Window::default_attributes()
						.with_title("Germinal")
						.with_inner_size(LogicalSize::new(960.0, 540.0)),
				)
				.map_err(|error| error.to_string())?,
		);

		window.set_ime_allowed(true);

		let runtime = pollster::block_on(WgpuTerminalWindowRuntime::new(window))?;
		let size = runtime.window_size();
		self.window_size = TerminalWindowSize::new(size.width.max(1), size.height.max(1));
		self.runtime = Some(WindowRuntimeState { runtime });

		Ok(())
	}

	pub fn current_window_id(&self) -> WindowId {
		self
			.runtime
			.as_ref()
			.expect("window runtime must be initialized before use")
			.runtime
			.window_id()
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

	fn current_terminal_size_info(&self) -> TerminalSizeInfo {
		let runtime = self.runtime.as_ref().expect("window runtime must be initialized before use");
		runtime.runtime.terminal_size_info()
	}

	fn resize_window_surface_size_info(
		&mut self,
		window_size: TerminalWindowSize,
	) -> TerminalSizeInfo {
		let runtime = self.runtime.as_mut().expect("window runtime must be initialized before use");

		let size_info = runtime.runtime.resize_surface_size_info(window_size);
		self.window_size = size_info.window_size();
		self.redraw_pending = true;

		size_info
	}

	fn set_surface_snapshot(&mut self, snapshot: RenderSurfaceSnapshot) {
		let runtime = self.runtime.as_mut().expect("window runtime must be initialized before use");

		runtime.runtime.set_surface_snapshot(snapshot);
		self.redraw_pending = true;
	}

	fn request_redraw(&mut self) { self.redraw_pending = true; }

	fn flush_redraw_request(&mut self) {
		let should_request = self.redraw_pending;
		self.redraw_pending = false;

		let runtime = self.runtime.as_mut().expect("window runtime must be initialized before use");

		let should_request = should_request || runtime.runtime.take_redraw_request();
		if should_request {
			runtime.runtime.request_window_redraw();
		}
	}

	fn render_window(&mut self) {
		let runtime = self.runtime.as_mut().expect("window runtime must be initialized before use");
		runtime.runtime.render();
	}
}

impl Default for RenderServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IRenderService for RenderService<Deps>
where Deps: AsRef<RenderServiceState> + AsMut<RenderServiceState>
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

		let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
		state.set_surface_snapshot(snapshot);
	}

	fn current_terminal_size_info(&self) -> TerminalSizeInfo {
		let state: &RenderServiceState = self.prj_ref().as_ref();
		state.current_terminal_size_info()
	}

	fn resize_window_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
		let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
		state.resize_window_surface_size_info(window_size)
	}

	fn request_redraw(&mut self) {
		let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
		state.request_redraw();
	}

	fn flush_redraw_request(&mut self) {
		let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
		state.flush_redraw_request();
	}

	fn present_workspace(&mut self) {
		let state: &mut RenderServiceState = self.prj_ref_mut().as_mut();
		state.render_window();
	}
}
