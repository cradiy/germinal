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
    workspace_service::{WorkspaceService, WorkspaceServiceState},
};
use germinal_domain::{
    gshell::vo::gshell_id::GShellId,
    pty_host::{pty_host_id::PtyHostId, terminal_size::TerminalGridSize},
};
use germinal_gnative_protocol::gnative::session::GNativeSessionAccepted;
use germinal_ports::{
    event::{
        gshell_input::{GShellInput, GShellInputEvent},
        runtime_event_dispatcher::{
            IRuntimeEventDispatcher, IRuntimeEventDispatcherProvider, RuntimeEventDispatchError,
        },
    },
    pty_host::{
        pty_backend::IPtyBackendProvider, size_info::TerminalSizeInfo,
        terminal_size::TerminalPtySize, window_metrics::TerminalWindowMetrics,
        window_size::TerminalWindowSize, worker_backend::ITerminalWorkerBackendProvider,
        worker_input::TerminalWorkerInput,
    },
    rendering::{
        render_target_id::RenderTargetId, surface_snapshot::RenderSurfaceSnapshot,
        tab_bar::TabBarSnapshot, window_runtime::IRenderRuntimeStore,
        workspace_layout::RenderSurfacePlacement,
    },
    repository::{IRepository, RepositoryError},
    service::{
        gnative_service::{GNativeServiceError, IGNativeService},
        gnative_tunnel::IGNativeTunnelProvider,
        gshell_service::IGShellService,
        layout_service::ILayoutService,
        pty_service::IPtyService,
        render_service::IRenderService,
        worker_service::IWorkerService,
        workspace_service::{
            IWorkspaceService, WorkspaceGShellCloseOutcome, WorkspaceServiceError,
        },
    },
};

use crate::app::{App, AppRuntimeEventDispatcher};

impl IRuntimeEventDispatcher for AppRuntimeEventDispatcher {
    fn dispatch(
        &self,
        event: germinal_ports::event::runtime_event::RuntimeEvent,
    ) -> Result<(), RuntimeEventDispatchError> {
        self.proxy
            .send_event(event)
            .map_err(|_| RuntimeEventDispatchError::Closed)?;
        Ok(())
    }
}

impl IRuntimeEventDispatcherProvider for App {
    type RuntimeEventDispatcher = AppRuntimeEventDispatcher;

    fn runtime_event_dispatcher(&self) -> &Self::RuntimeEventDispatcher {
        &self.runtime_event_dispatcher
    }
}

impl AsRef<WorkspaceServiceState> for App {
    fn as_ref(&self) -> &WorkspaceServiceState {
        &self.workspace_service_state
    }
}

impl AsMut<WorkspaceServiceState> for App {
    fn as_mut(&mut self) -> &mut WorkspaceServiceState {
        &mut self.workspace_service_state
    }
}

impl AsRef<GShellServiceState> for App {
    fn as_ref(&self) -> &GShellServiceState {
        &self.gshell_service_state
    }
}

impl IRepository for App {
    type Aggregate = germinal_domain::workspace::entity::workspace::Workspace;
    type Id = u64;

    fn get(&self, id: Self::Id) -> Result<Option<Self::Aggregate>, RepositoryError> {
        Ok(self
            .workspace_repository
            .borrow()
            .clone()
            .filter(|_| id == 1))
    }

    fn list(&self) -> Result<Vec<(Self::Id, Self::Aggregate)>, RepositoryError> {
        Ok(self
            .workspace_repository
            .borrow()
            .clone()
            .into_iter()
            .map(|workspace| (1, workspace))
            .collect())
    }

    fn insert(&self, aggregate: Self::Aggregate) -> Result<Self::Id, RepositoryError> {
        *self.workspace_repository.borrow_mut() = Some(aggregate);
        Ok(1)
    }

    fn update(&self, id: Self::Id, aggregate: Self::Aggregate) -> Result<(), RepositoryError> {
        if id == 1 {
            *self.workspace_repository.borrow_mut() = Some(aggregate);
        }
        Ok(())
    }

    fn delete(&self, id: Self::Id) -> Result<(), RepositoryError> {
        if id == 1 {
            self.workspace_repository.borrow_mut().take();
        }
        Ok(())
    }
}

impl AsRef<PtyServiceState> for App {
    fn as_ref(&self) -> &PtyServiceState {
        self.gshell_service_state.pty_service_state()
    }
}

impl AsRef<GNativeServiceState> for App {
    fn as_ref(&self) -> &GNativeServiceState {
        self.gshell_service_state.gnative_service_state()
    }
}

impl AsRef<WorkerServiceState> for App {
    fn as_ref(&self) -> &WorkerServiceState {
        &self.worker_service_state
    }
}

impl AsRef<RenderServiceState> for App {
    fn as_ref(&self) -> &RenderServiceState {
        &self.render_service_state
    }
}

impl AsMut<RenderServiceState> for App {
    fn as_mut(&mut self) -> &mut RenderServiceState {
        &mut self.render_service_state
    }
}

impl AsRef<LayoutServiceState> for App {
    fn as_ref(&self) -> &LayoutServiceState {
        &self.layout_service_state
    }
}

impl IPtyBackendProvider for App {
    type PtyBackend = germinal_infra::pty::PlatformPtyBackend;

    fn pty_backend(&self) -> &Self::PtyBackend {
        &self.pty_backend
    }
}

impl ITerminalWorkerBackendProvider for App {
    type TerminalWorkerBackend =
        germinal_infra::pty_host::worker::PlatformTerminalWorkerBackend<AppRuntimeEventDispatcher>;

    fn terminal_worker_backend(&self) -> &Self::TerminalWorkerBackend {
        &self.terminal_worker_backend
    }
}

impl IGNativeTunnelProvider for App {
    type GNativeTunnel = germinal_infra::gnative::tunnel::GNativeTunnel<AppRuntimeEventDispatcher>;

    fn gnative_tunnel(&self) -> &Self::GNativeTunnel {
        &self.gnative_tunnel
    }
}

impl IRenderRuntimeStore for App {
    type WindowRuntime =
        germinal_infra::rendering::pty_surface::window_runtime::WgpuTerminalWindowRuntime;

    fn window_runtime(&self) -> Option<&Self::WindowRuntime> {
        self.render_runtime.as_ref()
    }

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

    fn begin_gnative_mode(&self, gshell_id: GShellId) {
        GShellService::inj_ref(self).begin_gnative_mode(gshell_id)
    }

    fn enter_gnative_mode(&self, gshell_id: GShellId) {
        GShellService::inj_ref(self).enter_gnative_mode(gshell_id)
    }

    fn exit_gnative_mode(&self, gshell_id: GShellId) {
        GShellService::inj_ref(self).exit_gnative_mode(gshell_id)
    }

    fn remove_gshell(&self, gshell_id: GShellId) {
        GShellService::inj_ref(self).remove_gshell(gshell_id)
    }

    fn route_input_to_gshell(&self, input: GShellInput) {
        GShellService::inj_ref(self).route_input_to_gshell(input)
    }

    fn resize_gshell(&self, gshell_id: GShellId, size_info: TerminalSizeInfo) {
        GShellService::inj_ref(self).resize_gshell(gshell_id, size_info)
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

    fn remove_pty_host(&self, pty_host_id: PtyHostId) {
        PtyService::inj_ref(self).remove_pty_host(pty_host_id)
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

    fn begin_gnative_session(&self, gshell_id: GShellId) -> Result<(), GNativeServiceError> {
        GNativeService::inj_ref(self).begin_gnative_session(gshell_id)
    }

    fn activate_gnative_session(&self, accepted: GNativeSessionAccepted) {
        GNativeService::inj_ref(self).activate_gnative_session(accepted)
    }

    fn fail_gnative_session(&self, gshell_id: GShellId) {
        GNativeService::inj_ref(self).fail_gnative_session(gshell_id)
    }

    fn exit_gnative_session(&self, gshell_id: GShellId) {
        GNativeService::inj_ref(self).exit_gnative_session(gshell_id)
    }

    fn route_gnative_input(&self, input: GShellInput) {
        GNativeService::inj_ref(self).route_gnative_input(input)
    }

    fn resize_gnative_session(&self, gshell_id: GShellId, size_info: TerminalSizeInfo) {
        GNativeService::inj_ref(self).resize_gnative_session(gshell_id, size_info)
    }
}

impl IWorkerService for App {
    type TerminalWorkerSender = SyncSender<TerminalWorkerInput>;

    fn start_worker_pool(&self) {
        WorkerService::inj_ref(self).start_worker_pool()
    }

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
    fn prepare_render_backend(&mut self) {
        RenderService::inj_ref_mut(self).prepare_render_backend()
    }

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

    fn terminal_size_info_for_surface(
        &self,
        placement: RenderSurfacePlacement,
    ) -> TerminalSizeInfo {
        RenderService::inj_ref(self).terminal_size_info_for_surface(placement)
    }

    fn set_workspace_render_layout(&mut self, placements: Vec<RenderSurfacePlacement>) {
        RenderService::inj_ref_mut(self).set_workspace_render_layout(placements)
    }

    fn set_tab_bar(&mut self, tab_bar: Option<TabBarSnapshot>) {
        RenderService::inj_ref_mut(self).set_tab_bar(tab_bar)
    }

    fn set_window_title(&mut self, title: &str) {
        RenderService::inj_ref_mut(self).set_window_title(title)
    }

    fn ring_bell(&mut self, visual_duration: std::time::Duration, request_attention: bool) {
        RenderService::inj_ref_mut(self).ring_bell(visual_duration, request_attention)
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

    fn set_ime_preedit(
        &mut self,
        target_id: RenderTargetId,
        preedit: Option<
            germinal_ports::rendering::surface_snapshot::RenderSurfaceImePreeditSnapshot,
        >,
    ) {
        RenderService::inj_ref_mut(self).set_ime_preedit(target_id, preedit)
    }

    fn remove_render_target(&mut self, target_id: RenderTargetId) {
        RenderService::inj_ref_mut(self).remove_render_target(target_id)
    }

    fn request_redraw(&mut self) {
        RenderService::inj_ref_mut(self).request_redraw()
    }

    fn flush_redraw_request(&mut self) {
        RenderService::inj_ref_mut(self).flush_redraw_request()
    }

    fn present_workspace(&mut self) {
        RenderService::inj_ref_mut(self).present_workspace()
    }
}

impl IWorkspaceService for App {
    fn focused_gshell(&self) -> GShellId {
        WorkspaceService::inj_ref(self).focused_gshell()
    }

    fn focus_gshell(&self, gshell_id: GShellId) -> bool {
        WorkspaceService::inj_ref(self).focus_gshell(gshell_id)
    }

    fn focus_next_gshell(&self) -> GShellId {
        WorkspaceService::inj_ref(self).focus_next_gshell()
    }

    fn focus_previous_gshell(&self) -> GShellId {
        WorkspaceService::inj_ref(self).focus_previous_gshell()
    }

    fn create_tab_gshell(&self) -> GShellId {
        WorkspaceService::inj_ref(self).create_tab_gshell()
    }

    fn activate_next_tab(&self) -> GShellId {
        WorkspaceService::inj_ref(self).activate_next_tab()
    }

    fn activate_previous_tab(&self) -> GShellId {
        WorkspaceService::inj_ref(self).activate_previous_tab()
    }

    fn tab_count(&self) -> usize {
        WorkspaceService::inj_ref(self).tab_count()
    }

    fn active_tab_index(&self) -> usize {
        WorkspaceService::inj_ref(self).active_tab_index()
    }

    fn tab_titles(&self) -> Vec<String> {
        WorkspaceService::inj_ref(self).tab_titles()
    }

    fn tab_gshells(&self) -> Vec<GShellId> {
        WorkspaceService::inj_ref(self).tab_gshells()
    }

    fn update_gshell_title(&self, gshell_id: GShellId, title: Option<String>) {
        WorkspaceService::inj_ref(self).update_gshell_title(gshell_id, title)
    }

    fn split_focused_gshell(
        &self,
        direction: germinal_domain::workspace::vo::pane_split_direction::PaneSplitDirection,
    ) -> GShellId {
        WorkspaceService::inj_ref(self).split_focused_gshell(direction)
    }

    fn swap_focused_gshell_with(&self, other: GShellId) -> bool {
        WorkspaceService::inj_ref(self).swap_focused_gshell_with(other)
    }

    fn close_gshell(&self, gshell_id: GShellId) -> Option<WorkspaceGShellCloseOutcome> {
        WorkspaceService::inj_ref(self).close_gshell(gshell_id)
    }

    fn visible_gshells(&self) -> Vec<GShellId> {
        WorkspaceService::inj_ref(self).visible_gshells()
    }

    fn workspace_render_layout(
        &self,
        window_size: TerminalWindowSize,
    ) -> Vec<RenderSurfacePlacement> {
        WorkspaceService::inj_ref(self).workspace_render_layout(window_size)
    }

    fn restore_workspace(&self) -> Result<(), WorkspaceServiceError> {
        WorkspaceService::inj_ref(self).restore_workspace()
    }

    fn persist_workspace(&self) -> Result<(), WorkspaceServiceError> {
        WorkspaceService::inj_ref(self).persist_workspace()
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
