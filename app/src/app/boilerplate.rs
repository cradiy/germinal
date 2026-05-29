use std::sync::{
	Arc,
	atomic::AtomicBool,
	mpsc::{Sender, SyncSender},
};

use germinal_application::service::{
	gnative_service::{GNativeService, GNativeServiceState},
	gshell_service::{GShellService, GShellServiceState},
	layout_service::{LayoutService, LayoutServiceState},
	pty_service::{PtyService, PtyServiceState},
	render_service::{RenderService, RenderServiceState},
	worker_service::{WorkerService, WorkerServiceState},
	workspace_service::WorkspaceServiceState,
};
use germinal_domain::{
	pty_host::{
		size_info::TerminalSizeInfo,
		terminal_size::{TerminalGridSize, TerminalPtySize},
		window_metrics::TerminalWindowMetrics,
		window_size::TerminalWindowSize,
	},
	workspace::pane_id::PaneId,
};
use germinal_infra::pty_host::worker::TerminalWorkerInput;
use germinal_ports::{
	event::{gshell_input::GShellInput, runtime_event_dispatcher::RuntimeEventDispatcher},
	rendering::surface_snapshot::RenderSurfaceSnapshot,
	service::{
		gnative_service::IGNativeService, gshell_service::IGShellService,
		layout_service::ILayoutService, pty_service::IPtyService, render_service::IRenderService,
		worker_service::IWorkerService,
	},
};

use crate::app::App;

impl AsRef<WorkspaceServiceState> for App {
	fn as_ref(&self) -> &WorkspaceServiceState { &self.workspace_service_state }
}

impl AsMut<WorkspaceServiceState> for App {
	fn as_mut(&mut self) -> &mut WorkspaceServiceState { &mut self.workspace_service_state }
}

impl AsRef<GShellServiceState> for App {
	fn as_ref(&self) -> &GShellServiceState { &self.gshell_service_state }
}

impl AsRef<PtyServiceState> for App {
	fn as_ref(&self) -> &PtyServiceState { self.gshell_service_state.pty_service_state() }
}

impl AsRef<GNativeServiceState> for App {
	fn as_ref(&self) -> &GNativeServiceState { self.gshell_service_state.gnative_service_state() }
}

impl AsRef<WorkerServiceState> for App {
	fn as_ref(&self) -> &WorkerServiceState { &self.worker_service_state }
}

impl AsRef<RenderServiceState> for App {
	fn as_ref(&self) -> &RenderServiceState { &self.render_service_state }
}

impl AsMut<RenderServiceState> for App {
	fn as_mut(&mut self) -> &mut RenderServiceState { &mut self.render_service_state }
}

impl AsRef<LayoutServiceState> for App {
	fn as_ref(&self) -> &LayoutServiceState { &self.layout_service_state }
}

impl IGShellService for App {
	fn ensure_pane_gshell(
		&self,
		pane_id: PaneId,
		proxy: RuntimeEventDispatcher,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		GShellService::inj_ref(self).ensure_pane_gshell(
			pane_id,
			proxy,
			pty_size,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		)
	}

	fn route_input_to_gshell(&self, input: GShellInput) {
		GShellService::inj_ref(self).route_input_to_gshell(input)
	}

	fn resize_pane_gshell(
		&self,
		pane_id: PaneId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		GShellService::inj_ref(self).resize_pane_gshell(pane_id, pty_size, term_size)
	}
}

impl IPtyService for App {
	fn ensure_pane_pty(
		&self,
		pane_id: PaneId,
		proxy: RuntimeEventDispatcher,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		PtyService::inj_ref(self).ensure_pane_pty(
			pane_id,
			proxy,
			pty_size,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		)
	}

	fn send_pane_pty_input(&self, input: GShellInput) {
		PtyService::inj_ref(self).send_pane_pty_input(input)
	}

	fn resize_pane_pty(
		&self,
		pane_id: PaneId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		PtyService::inj_ref(self).resize_pane_pty(pane_id, pty_size, term_size)
	}
}

impl IGNativeService for App {
	fn ensure_pane_gnative(&self, pane_id: PaneId) {
		GNativeService::inj_ref(self).ensure_pane_gnative(pane_id)
	}
}

impl IWorkerService for App {
	type TerminalWorkerSender = SyncSender<TerminalWorkerInput>;

	fn start_worker_pool(&self) { WorkerService::inj_ref(self).start_worker_pool() }

	fn spawn_terminal_worker(
		&self,
		pane_id: PaneId,
		initial_size: TerminalGridSize,
		proxy: RuntimeEventDispatcher,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> Option<SyncSender<TerminalWorkerInput>> {
		WorkerService::inj_ref(self).spawn_terminal_worker(
			pane_id,
			initial_size,
			proxy,
			surface_snapshot_tx,
			snapshot_wake_pending,
		)
	}
}

impl IRenderService for App {
	fn prepare_render_backend(&mut self) { RenderService::inj_ref_mut(self).prepare_render_backend() }

	fn surface_snapshot_sender(&self) -> Sender<RenderSurfaceSnapshot> {
		RenderService::inj_ref(self).surface_snapshot_sender()
	}

	fn snapshot_wake_pending(&self) -> Arc<AtomicBool> {
		RenderService::inj_ref(self).snapshot_wake_pending()
	}

	fn consume_latest_terminal_snapshot(&mut self) {
		RenderService::inj_ref_mut(self).consume_latest_terminal_snapshot()
	}

	fn current_terminal_size_info(&self) -> TerminalSizeInfo {
		RenderService::inj_ref(self).current_terminal_size_info()
	}

	fn resize_window_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
		RenderService::inj_ref_mut(self).resize_window_size_info(window_size)
	}

	fn request_redraw(&mut self) { RenderService::inj_ref_mut(self).request_redraw() }

	fn flush_redraw_request(&mut self) { RenderService::inj_ref_mut(self).flush_redraw_request() }

	fn present_workspace(&mut self) { RenderService::inj_ref_mut(self).present_workspace() }
}

impl ILayoutService for App {
	fn terminal_size_info_for_window(&self, window_size: TerminalWindowSize) -> TerminalSizeInfo {
		LayoutService::inj_ref(self).terminal_size_info_for_window(window_size)
	}

	fn terminal_size_info_for_window_metrics(
		&self,
		metrics: TerminalWindowMetrics,
	) -> TerminalSizeInfo {
		LayoutService::inj_ref(self).terminal_size_info_for_window_metrics(metrics)
	}
}
