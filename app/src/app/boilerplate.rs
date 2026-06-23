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
	gshell::vo::gshell_id::GShellId,
	pty_host::{pty_host_id::PtyHostId, terminal_size::TerminalGridSize},
};
use germinal_ports::{
	event::{
		gshell_input::{GShellInput, GShellInputEvent},
		runtime_event_dispatcher::{IRuntimeEventDispatcher, IRuntimeEventDispatcherProvider},
	},
	pty_host::{
		pty_backend::IPtyBackendProvider, size_info::TerminalSizeInfo, terminal_size::TerminalPtySize,
		window_metrics::TerminalWindowMetrics, window_size::TerminalWindowSize,
		worker_backend::ITerminalWorkerBackendProvider, worker_input::TerminalWorkerInput,
	},
	rendering::{
		render_target_id::RenderTargetId, surface_snapshot::RenderSurfaceSnapshot,
		window_runtime::IRenderRuntimeStore,
	},
	repository::IRepository,
	service::{
		gnative_service::IGNativeService, gshell_service::IGShellService,
		layout_service::ILayoutService, pty_service::IPtyService, render_service::IRenderService,
		worker_service::IWorkerService, workspace_service::IWorkspaceService,
	},
};

use crate::app::{App, AppRuntimeEventDispatcher};

impl IRuntimeEventDispatcher for AppRuntimeEventDispatcher {
	fn dispatch(
		&self,
		event: germinal_ports::event::runtime_event::RuntimeEvent,
	) -> Result<(), String> {
		self.proxy.send_event(event).map_err(|error| error.to_string())
	}
}

impl IRuntimeEventDispatcherProvider for App {
	type RuntimeEventDispatcher = AppRuntimeEventDispatcher;

	fn runtime_event_dispatcher(&self) -> &Self::RuntimeEventDispatcher {
		&self.runtime_event_dispatcher
	}
}

impl AsRef<WorkspaceServiceState> for App {
	fn as_ref(&self) -> &WorkspaceServiceState { &self.workspace_service_state }
}

impl AsMut<WorkspaceServiceState> for App {
	fn as_mut(&mut self) -> &mut WorkspaceServiceState { &mut self.workspace_service_state }
}

impl AsRef<GShellServiceState> for App {
	fn as_ref(&self) -> &GShellServiceState { &self.gshell_service_state }
}

impl IRepository for App {
	type Aggregate = germinal_domain::workspace::entity::workspace::Workspace;
	type Id = u64;

	fn get(&self, id: Self::Id) -> Result<Option<Self::Aggregate>, String> {
		self.workspace_persistence_repository.get(id)
	}

	fn list(&self) -> Result<Vec<(Self::Id, Self::Aggregate)>, String> {
		self.workspace_persistence_repository.list()
	}

	fn insert(&self, aggregate: Self::Aggregate) -> Result<Self::Id, String> {
		self.workspace_persistence_repository.insert(aggregate)
	}

	fn update(&self, id: Self::Id, aggregate: Self::Aggregate) -> Result<(), String> {
		self.workspace_persistence_repository.update(id, aggregate)
	}

	fn delete(&self, id: Self::Id) -> Result<(), String> {
		self.workspace_persistence_repository.delete(id)
	}
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

impl IPtyBackendProvider for App {
	type PtyBackend = germinal_infra::pty::PlatformPtyBackend;

	fn pty_backend(&self) -> &Self::PtyBackend { &self.pty_backend }
}

impl ITerminalWorkerBackendProvider for App {
	type TerminalWorkerBackend =
		germinal_infra::pty_host::worker::PlatformTerminalWorkerBackend<AppRuntimeEventDispatcher>;

	fn terminal_worker_backend(&self) -> &Self::TerminalWorkerBackend {
		&self.terminal_worker_backend
	}
}

impl IRenderRuntimeStore for App {
	type WindowRuntime =
		germinal_infra::rendering::pty_surface::window_runtime::WgpuTerminalWindowRuntime;

	fn window_runtime(&self) -> Option<&Self::WindowRuntime> { self.render_runtime.as_ref() }

	fn window_runtime_mut(&mut self) -> Option<&mut Self::WindowRuntime> {
		self.render_runtime.as_mut()
	}

	fn set_window_runtime(&mut self, runtime: Self::WindowRuntime) {
		self.render_runtime = Some(runtime);
	}
}

impl IGShellService for App {
	fn ensure_gshell(
		&self,
		gshell_id: GShellId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		GShellService::inj_ref(self).ensure_gshell(
			gshell_id,
			pty_size,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		)
	}

	fn route_input_to_gshell(&self, input: GShellInput) {
		GShellService::inj_ref(self).route_input_to_gshell(input)
	}

	fn resize_gshell(
		&self,
		gshell_id: GShellId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		GShellService::inj_ref(self).resize_gshell(gshell_id, pty_size, term_size)
	}
}

impl IPtyService for App {
	fn ensure_gshell_pty(
		&self,
		gshell_id: GShellId,
		pty_host_id: PtyHostId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) {
		PtyService::inj_ref(self).ensure_gshell_pty(
			gshell_id,
			pty_host_id,
			pty_size,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		)
	}

	fn send_pty_host_input(&self, pty_host_id: PtyHostId, event: GShellInputEvent) {
		PtyService::inj_ref(self).send_pty_host_input(pty_host_id, event)
	}

	fn resize_pty_host(
		&self,
		pty_host_id: PtyHostId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	) {
		PtyService::inj_ref(self).resize_pty_host(pty_host_id, pty_size, term_size)
	}
}

impl IGNativeService for App {
	fn ensure_gshell_gnative(&self, gshell_id: GShellId) {
		GNativeService::inj_ref(self).ensure_gshell_gnative(gshell_id)
	}
}

impl IWorkerService for App {
	type TerminalWorkerSender = SyncSender<TerminalWorkerInput>;

	fn start_worker_pool(&self) { WorkerService::inj_ref(self).start_worker_pool() }

	fn spawn_terminal_worker(
		&self,
		gshell_id: GShellId,
		initial_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	) -> Option<SyncSender<TerminalWorkerInput>> {
		WorkerService::inj_ref(self).spawn_terminal_worker(
			gshell_id,
			initial_size,
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

	fn set_window_focused(&mut self, focused: bool) {
		RenderService::inj_ref_mut(self).set_window_focused(focused)
	}

	fn set_focused_render_target(&mut self, target_id: RenderTargetId) {
		RenderService::inj_ref_mut(self).set_focused_render_target(target_id)
	}

	fn request_redraw(&mut self) { RenderService::inj_ref_mut(self).request_redraw() }

	fn flush_redraw_request(&mut self) { RenderService::inj_ref_mut(self).flush_redraw_request() }

	fn present_workspace(&mut self) { RenderService::inj_ref_mut(self).present_workspace() }
}

impl IWorkspaceService for App {
	fn focused_gshell(&self) -> GShellId {
		germinal_application::service::workspace_service::WorkspaceService::inj_ref(self)
			.focused_gshell()
	}

	fn restore_workspace(&self) -> Result<(), String> {
		germinal_application::service::workspace_service::WorkspaceService::inj_ref(self)
			.restore_workspace()
	}

	fn persist_workspace(&self) -> Result<(), String> {
		germinal_application::service::workspace_service::WorkspaceService::inj_ref(self)
			.persist_workspace()
	}
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
