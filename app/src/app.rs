use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

mod audible_bell;
mod boilerplate;
mod config;
mod error;
mod logging;
mod paste;

use audible_bell::AudibleBell;
pub use config::{GerminalConfig, load_or_create_config};
use config::{KeyboardAction, KeyboardBinding};
pub use error::{AppError, AppResult};
use germinal_application::service::{
    gshell_service::GShellServiceState, layout_service::LayoutServiceState,
    render_service::RenderServiceState, worker_service::WorkerServiceState,
    workspace_service::WorkspaceServiceState,
};
use germinal_domain::{
    gshell::vo::gshell_id::GShellId,
    workspace::{
        entity::workspace::Workspace,
        vo::{
            pane_resize_direction::PaneResizeDirection, pane_split_direction::PaneSplitDirection,
        },
    },
};
#[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
use germinal_infra::gnative::gst_video_player_bridge::GstVideoPlayerBridge;
use germinal_infra::rendering::pty_surface::render_plugin::WgpuPaneRenderPlugin;
use germinal_infra::{
    pty::PlatformPtyBackend,
    pty_host::worker::PlatformTerminalWorkerBackend,
    rendering::pty_surface::window_runtime::{
        WgpuTerminalWindowRuntime, WgpuTerminalWindowRuntimeFactory,
    },
    system_notification::SystemNotifier,
};
use germinal_ports::{
    event::{
        gshell_input::{GShellInput, GShellInputEvent},
        runtime_event::{GShellRuntimeEvent, RuntimeEvent, WorkspaceRuntimeEvent},
        runtime_event_dispatcher::IRuntimeEventDispatcher,
        window_input_event::{
            WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
            WindowInputNamedKey, WindowPointerButton, WindowPointerPosition, WindowScrollDelta,
        },
    },
    pty_host::{
        hyperlink::TerminalHyperlink,
        size_info::TerminalSizeInfo,
        spawn_config::PtySpawnConfig,
        terminal_notification::{TerminalNotification, TerminalNotificationOccasion},
        window_size::TerminalWindowSize,
    },
    rendering::{
        render_target_id::RenderTargetId,
        surface_snapshot::RenderSurfaceImePreeditSnapshot,
        tab_bar::{TabBarPosition, TabBarSnapshot},
        workspace_layout::RenderSurfacePlacement,
    },
    service::{
        gnative_service::IGNativeService,
        gshell_service::IGShellService,
        render_service::IRenderService,
        workspace_service::{IWorkspaceService, WorkspaceGShellCloseOutcome},
    },
};
pub use logging::init_logging;
use paste::{HostPasteController, HostPasteModifiers};
use tracing::{debug, error, warn};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalPosition,
    event::{ElementState, Ime, MouseButton, MouseScrollDelta, StartCause, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{Key, NamedKey},
    window::WindowId,
};

#[cfg(target_os = "linux")]
const DESKTOP_APP_ID: &str = "io.github.cradiy.Germinal";

#[derive(Clone)]
pub struct AppRuntimeEventDispatcher {
    proxy: EventLoopProxy<RuntimeEvent>,
}

pub struct App {
    workspace_service_state: WorkspaceServiceState,
    gshell_service_state: GShellServiceState,
    worker_service_state: WorkerServiceState,
    render_service_state: RenderServiceState,
    layout_service_state: LayoutServiceState,
    workspace_repository: RefCell<Option<Workspace>>,
    runtime_event_dispatcher: AppRuntimeEventDispatcher,
    pty_backend: PlatformPtyBackend,
    gnative_tunnel: germinal_infra::gnative::tunnel::GNativeTunnel<AppRuntimeEventDispatcher>,
    #[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
    media_bridge: std::sync::Arc<GstVideoPlayerBridge>,
    terminal_worker_backend: PlatformTerminalWorkerBackend<AppRuntimeEventDispatcher>,
    render_runtime_factory: WgpuTerminalWindowRuntimeFactory,
    render_runtime: Option<WgpuTerminalWindowRuntime>,
    pending_wgpu_pane_plugins: Vec<WgpuPaneRenderPlugin>,
    wgpu_pane_targets: HashSet<RenderTargetId>,
    render_window_id: Option<WindowId>,
    audible_bell: AudibleBell,
    system_notifier: SystemNotifier,
    paste_controller: HostPasteController,
    window_input_modifiers: WindowInputModifiers,
    routed_input_modifiers: WindowInputModifiers,
    window_focused: bool,
    ime_enabled: bool,
    cursor_position: Option<PhysicalPosition<f64>>,
    pointer_gshell: Option<GShellId>,
    pointer_capture: Option<PointerCapture>,
    pending_working_directories: RefCell<HashMap<GShellId, PathBuf>>,
    terminal_hyperlinks: HashMap<GShellId, Vec<TerminalHyperlink>>,
    hyperlink_pointer_consumed: bool,
    pane_navigation_enabled: bool,
    config: GerminalConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PointerCapture {
    gshell_id: GShellId,
    position: WindowPointerPosition,
}

impl App {
    pub fn new(
        runtime_event_proxy: EventLoopProxy<RuntimeEvent>,
        config: GerminalConfig,
    ) -> AppResult<Self> {
        Self::new_with_workspace(runtime_event_proxy, config, Workspace::main())
    }

    pub fn new_with_workspace(
        runtime_event_proxy: EventLoopProxy<RuntimeEvent>,
        config: GerminalConfig,
        workspace: Workspace,
    ) -> AppResult<Self> {
        Self::new_with_workspace_and_wgpu_panes(runtime_event_proxy, config, workspace, Vec::new())
    }

    pub fn new_with_workspace_and_wgpu_panes(
        runtime_event_proxy: EventLoopProxy<RuntimeEvent>,
        config: GerminalConfig,
        workspace: Workspace,
        wgpu_pane_plugins: Vec<WgpuPaneRenderPlugin>,
    ) -> AppResult<Self> {
        let pane_navigation_enabled = workspace.active_tab().pane_count() > 1;
        let wgpu_pane_targets = wgpu_pane_plugins
            .iter()
            .map(WgpuPaneRenderPlugin::target_id)
            .collect();
        let runtime_event_dispatcher = AppRuntimeEventDispatcher {
            proxy: runtime_event_proxy,
        };
        let notification_dispatcher = runtime_event_dispatcher.clone();
        let system_notifier = SystemNotifier::new(move |gshell_id| {
            let _ = notification_dispatcher.dispatch(RuntimeEvent::GShell(
                GShellRuntimeEvent::SystemNotificationActivated { gshell_id },
            ));
        });
        #[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
        let media_dispatcher = {
            let runtime_event_dispatcher = runtime_event_dispatcher.clone();
            std::sync::Arc::new(move |event: RuntimeEvent| runtime_event_dispatcher.dispatch(event))
        };
        #[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
        let media_bridge = std::sync::Arc::new(
            GstVideoPlayerBridge::new(media_dispatcher).map_err(AppError::MediaBridge)?,
        );
        let terminal_profile = config.terminal_profile();
        let scrollback_history = config.scrolling.history;
        let terminal_cursor_style = config.terminal_cursor_style();
        let terminal_color_theme = config.terminal_color_theme();
        let terminal_osc52_mode = config.terminal_osc52_mode();
        let cursor_blink_interval = Duration::from_millis(config.cursor.blink_interval_ms.max(1));
        let cursor_motion_duration = Duration::from_millis(config.cursor.motion_duration_ms);
        let window_title = config.window.title.clone();
        let window_opacity = config.window.opacity;
        let background_shader = config.background_shader();
        let audible_bell = AudibleBell::new(config.bell.command.clone());
        let render_runtime_factory = WgpuTerminalWindowRuntimeFactory::new(
            terminal_profile.clone(),
            window_title,
            cursor_blink_interval,
            cursor_motion_duration,
            terminal_color_theme,
            window_opacity,
        )
        .with_cursor_motion_modes(config.cursor.motion_on_input, config.cursor.motion_on_enter);
        let render_runtime_factory = match background_shader {
            Some(shader) => render_runtime_factory.with_background_shader(shader),
            None => render_runtime_factory,
        };

        let app = Self {
            workspace_service_state: WorkspaceServiceState::with_workspace(workspace),
            gshell_service_state: GShellServiceState::new(),
            worker_service_state: WorkerServiceState::new(),
            render_service_state: RenderServiceState::new(),
            layout_service_state: LayoutServiceState::new(terminal_profile.clone()),
            workspace_repository: RefCell::new(None),
            runtime_event_dispatcher: runtime_event_dispatcher.clone(),
            pty_backend: PlatformPtyBackend::new(),
            gnative_tunnel: germinal_infra::gnative::tunnel::GNativeTunnel::new()
                .map_err(AppError::CreateGNativeTunnel)?,
            #[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
            media_bridge: std::sync::Arc::clone(&media_bridge),
            terminal_worker_backend: PlatformTerminalWorkerBackend::new(
                runtime_event_dispatcher,
                scrollback_history,
                terminal_cursor_style,
                terminal_color_theme,
                terminal_osc52_mode,
            ),
            render_runtime_factory,
            render_runtime: None,
            pending_wgpu_pane_plugins: wgpu_pane_plugins,
            wgpu_pane_targets,
            render_window_id: None,
            audible_bell,
            system_notifier,
            paste_controller: HostPasteController::default(),
            window_input_modifiers: WindowInputModifiers::new(false, false, false, false),
            routed_input_modifiers: WindowInputModifiers::new(false, false, false, false),
            window_focused: true,
            ime_enabled: false,
            cursor_position: None,
            pointer_gshell: None,
            pointer_capture: None,
            pending_working_directories: RefCell::new(HashMap::new()),
            terminal_hyperlinks: HashMap::new(),
            hyperlink_pointer_consumed: false,
            pane_navigation_enabled,
            config,
        };

        app.gnative_tunnel.configure(
            app.runtime_event_dispatcher.clone(),
            app.snapshot_wake_pending(),
            app.surface_snapshot_sender(),
        );
        #[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
        app.gnative_tunnel.configure_media_bridge(media_bridge);

        app.restore_workspace()
            .map_err(AppError::RestoreWorkspace)?;

        Ok(app)
    }

    pub fn run(&mut self, event_loop: EventLoop<RuntimeEvent>) -> AppResult<()> {
        event_loop.run_app(self).map_err(AppError::RunEventLoop)
    }

    fn ensure_window_runtime(&mut self, event_loop: &ActiveEventLoop) -> AppResult<()> {
        if self.render_runtime.is_some() {
            return Ok(());
        }

        let window_attributes = winit::window::Window::default_attributes()
            .with_title(self.config.window.title.as_str())
            .with_transparent(self.config.window.opacity < 1.0)
            .with_decorations(self.config.window.decorations)
            .with_maximized(self.config.window.maximized)
            .with_inner_size(winit::dpi::LogicalSize::new(
                f64::from(self.config.window.width_px),
                f64::from(self.config.window.height_px),
            ));
        #[cfg(target_os = "linux")]
        let window_attributes = {
            use winit::platform::{
                wayland::WindowAttributesExtWayland, x11::WindowAttributesExtX11,
            };

            let window_attributes = WindowAttributesExtWayland::with_name(
                window_attributes,
                DESKTOP_APP_ID,
                "germinal",
            );
            WindowAttributesExtX11::with_name(window_attributes, DESKTOP_APP_ID, "germinal")
        };
        let window = std::sync::Arc::new(
            event_loop
                .create_window(window_attributes)
                .map_err(AppError::CreateWindow)?,
        );
        let window_id = window.id();
        window.set_ime_allowed(true);

        let runtime = self
            .render_runtime_factory
            .create_window_runtime_with_plugins(
                window,
                std::mem::take(&mut self.pending_wgpu_pane_plugins),
            )
            .map_err(AppError::CreateWindowRuntime)?;
        self.render_runtime = Some(runtime);
        self.render_window_id = Some(window_id);
        Ok(())
    }

    fn current_window_id(&self) -> Option<WindowId> {
        self.render_window_id
    }

    fn current_tab_bar_snapshot(&self) -> Option<TabBarSnapshot> {
        (self.tab_count() > 1).then(|| TabBarSnapshot {
            titles: self.tab_titles(),
            progresses: self.tab_progresses(),
            render_target_ids: self
                .tab_gshells()
                .into_iter()
                .map(|gshell_id| RenderTargetId::new(gshell_id.value()))
                .collect(),
            active_tab_index: self.active_tab_index(),
            position: self.config.tabs.position,
        })
    }

    fn current_terminal_window_title(&self) -> String {
        self.tab_titles()
            .get(self.active_tab_index())
            .cloned()
            .unwrap_or_else(|| self.config.window.title.clone())
    }

    fn sync_current_terminal_window_title(&mut self) {
        let title = self.current_terminal_window_title();
        self.set_window_title(&title);
    }

    fn current_workspace_render_layout(
        &self,
        window_size: TerminalWindowSize,
    ) -> Vec<RenderSurfacePlacement> {
        let layout = workspace_content_layout(
            window_size,
            self.current_terminal_size_info().cell_size().height_px(),
            self.tab_count() > 1,
            self.config.tabs.position,
        );
        self.workspace_render_layout(layout.content_size)
            .into_iter()
            .map(|placement| {
                RenderSurfacePlacement::new(
                    placement.target_id,
                    placement.x_px,
                    placement.y_px.saturating_add(layout.y_px),
                    placement.width_px,
                    placement.height_px,
                )
            })
            .collect()
    }

    fn set_current_workspace_render_layout(&mut self, placements: Vec<RenderSurfacePlacement>) {
        self.sync_current_terminal_window_title();
        self.set_tab_bar(self.current_tab_bar_snapshot());
        self.set_workspace_render_layout(placements);
    }

    fn ensure_workspace_gshells(&mut self) {
        let window_size = self.current_terminal_size_info().window_size();
        let placements = self.current_workspace_render_layout(window_size);
        self.set_current_workspace_render_layout(placements.clone());

        let surface_snapshot_tx = self.surface_snapshot_sender();
        let snapshot_wake_pending = self.snapshot_wake_pending();
        for placement in placements {
            if self.wgpu_pane_targets.contains(&placement.target_id) {
                continue;
            }
            let size_info = self.terminal_size_info_for_surface(placement);
            let spawn_config = self.pty_spawn_config_for_gshell(
                GShellId::new(placement.target_id.value()),
                size_info.pty_size(),
            );
            self.ensure_gshell(
                GShellId::new(placement.target_id.value()),
                spawn_config,
                size_info.grid_size(),
                surface_snapshot_tx.clone(),
                std::sync::Arc::clone(&snapshot_wake_pending),
            );
        }
    }

    fn pty_spawn_config_for_gshell(
        &self,
        gshell_id: GShellId,
        initial_size: germinal_ports::pty_host::terminal_size::TerminalPtySize,
    ) -> PtySpawnConfig {
        let working_directory = self
            .pending_working_directories
            .borrow_mut()
            .remove(&gshell_id)
            .or_else(|| self.config.configured_working_directory());
        PtySpawnConfig {
            shell: self.config.pty_shell_command(),
            working_directory,
            initial_size,
        }
    }

    fn inherit_working_directory(&self, source: GShellId, target: GShellId) {
        let working_directory = self
            .gshell_working_directory(source)
            .or_else(|| self.config.configured_working_directory());
        if let Some(working_directory) = working_directory {
            self.pending_working_directories
                .borrow_mut()
                .insert(target, working_directory);
        }
    }

    fn resize_workspace_gshells(&mut self, window_size: TerminalWindowSize) {
        let placements = self.current_workspace_render_layout(window_size);
        self.set_current_workspace_render_layout(placements.clone());

        for placement in placements {
            if self.wgpu_pane_targets.contains(&placement.target_id) {
                continue;
            }
            let size_info = self.terminal_size_info_for_surface(placement);
            self.resize_gshell(GShellId::new(placement.target_id.value()), size_info);
        }
    }

    fn split_focused_workspace_pane(&mut self, direction: PaneSplitDirection) {
        let previous_gshell = self.focused_gshell();
        let focused_gshell = self.split_focused_gshell(direction);
        self.inherit_working_directory(previous_gshell, focused_gshell);
        self.pane_navigation_enabled = true;

        if self.render_runtime.is_none() {
            return;
        }

        self.clear_ime_preedit(previous_gshell);
        self.ensure_workspace_gshells();
        let window_size = self.current_terminal_size_info().window_size();
        self.resize_workspace_gshells(window_size);
        self.apply_gshell_focus_change(previous_gshell, focused_gshell);

        if let Some(position) = self.cursor_position {
            self.route_pointer_moved(position);
        }
        self.request_redraw();
    }

    fn create_workspace_tab(&mut self) {
        let previous_gshell = self.focused_gshell();
        let focused_gshell = self.create_tab_gshell();
        self.inherit_working_directory(previous_gshell, focused_gshell);
        self.activate_workspace_tab(previous_gshell, focused_gshell);
    }

    fn create_window(&self) {
        let executable = match std::env::current_exe() {
            Ok(executable) => executable,
            Err(error) => {
                warn!(error = %error, "failed to locate the Germinal executable for a new window");
                return;
            }
        };
        let working_directory = self
            .gshell_working_directory(self.focused_gshell())
            .or_else(|| self.config.configured_working_directory());
        let mut command = Command::new(executable);
        if let Some(working_directory) = working_directory.filter(|path| path.is_dir()) {
            command.current_dir(working_directory);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;

            command.process_group(0);
        }

        match command.spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    if let Err(error) = child.wait() {
                        warn!(error = %error, "failed to wait for a Germinal child window");
                    }
                });
            }
            Err(error) => {
                warn!(error = %error, "failed to create a Germinal window");
            }
        }
    }

    fn activate_next_workspace_tab(&mut self) {
        if self.tab_count() < 2 {
            return;
        }
        let previous_gshell = self.focused_gshell();
        let focused_gshell = self.activate_next_tab();
        self.activate_workspace_tab(previous_gshell, focused_gshell);
    }

    fn activate_previous_workspace_tab(&mut self) {
        if self.tab_count() < 2 {
            return;
        }
        let previous_gshell = self.focused_gshell();
        let focused_gshell = self.activate_previous_tab();
        self.activate_workspace_tab(previous_gshell, focused_gshell);
    }

    fn move_active_workspace_tab_left(&mut self) {
        if self.move_active_tab_left() {
            self.set_tab_bar(self.current_tab_bar_snapshot());
            self.request_redraw();
        }
    }

    fn move_active_workspace_tab_right(&mut self) {
        if self.move_active_tab_right() {
            self.set_tab_bar(self.current_tab_bar_snapshot());
            self.request_redraw();
        }
    }

    fn activate_workspace_tab(&mut self, previous_gshell: GShellId, focused_gshell: GShellId) {
        self.pane_navigation_enabled = self.visible_gshells().len() > 1;
        if self.render_runtime.is_none() {
            return;
        }

        self.clear_ime_preedit(previous_gshell);
        self.ensure_workspace_gshells();
        let window_size = self.current_terminal_size_info().window_size();
        self.resize_workspace_gshells(window_size);
        self.apply_gshell_focus_change(previous_gshell, focused_gshell);
        if let Some(position) = self.cursor_position {
            self.route_pointer_moved(position);
        }
        self.request_redraw();
    }

    fn focus_workspace_pane(&mut self, direction: PaneDirection) {
        if self.render_runtime.is_none() || !self.pane_navigation_enabled {
            return;
        }

        let previous_gshell = self.focused_gshell();
        let window_size = self.current_terminal_size_info().window_size();
        let placements = self.current_workspace_render_layout(window_size);
        let Some(target) = directional_neighbor_target(
            &placements,
            RenderTargetId::new(previous_gshell.value()),
            direction,
        ) else {
            return;
        };
        let focused_gshell = GShellId::new(target.value());
        if self.focus_gshell(focused_gshell) {
            self.apply_gshell_focus_change(previous_gshell, focused_gshell);
        }
    }

    fn swap_focused_workspace_pane(&mut self, direction: PaneDirection) {
        if self.render_runtime.is_none() {
            return;
        }

        let focused_gshell = self.focused_gshell();
        let window_size = self.current_terminal_size_info().window_size();
        let placements = self.current_workspace_render_layout(window_size);
        let Some(other_target) = directional_neighbor_target(
            &placements,
            RenderTargetId::new(focused_gshell.value()),
            direction,
        ) else {
            return;
        };

        self.clear_ime_preedit(focused_gshell);
        if !self.swap_focused_gshell_with(GShellId::new(other_target.value())) {
            return;
        }

        self.resize_workspace_gshells(window_size);
        self.update_ime_cursor_area();
        if let Some(position) = self.cursor_position {
            self.route_pointer_moved(position);
        }
        self.request_redraw();
    }

    fn resize_focused_workspace_pane(&mut self, direction: PaneResizeDirection) {
        if self.render_runtime.is_none() || !self.pane_navigation_enabled {
            return;
        }

        if !self.resize_focused_gshell(direction) {
            return;
        }

        let window_size = self.current_terminal_size_info().window_size();
        self.resize_workspace_gshells(window_size);
        self.update_ime_cursor_area();
        if let Some(position) = self.cursor_position {
            self.route_pointer_moved(position);
        }
        self.request_redraw();
    }

    fn close_workspace_gshell(&mut self, event_loop: &ActiveEventLoop, gshell_id: GShellId) {
        let previous_gshell = self.focused_gshell();
        let Some(outcome) = self.close_gshell(gshell_id) else {
            debug!(
                gshell_id = gshell_id.value(),
                "ignored close request for an unknown gshell"
            );
            return;
        };
        let WorkspaceGShellCloseOutcome::Closed {
            closed_gshells,
            focused_gshell,
        } = outcome
        else {
            self.exit_and_persist(event_loop);
            return;
        };

        self.apply_gshell_focus_change(previous_gshell, focused_gshell);
        for closed_gshell in closed_gshells {
            self.pending_working_directories
                .borrow_mut()
                .remove(&closed_gshell);
            self.terminal_hyperlinks.remove(&closed_gshell);
            self.remove_gshell(closed_gshell);
            self.remove_render_target(RenderTargetId::new(closed_gshell.value()));
        }
        self.pane_navigation_enabled = self.visible_gshells().len() > 1;
        let window_size = self.current_terminal_size_info().window_size();
        self.resize_workspace_gshells(window_size);
        if let Some(position) = self.cursor_position {
            self.route_pointer_moved(position);
        }
        self.request_redraw();
    }

    fn current_gshell_size_info(&self, gshell_id: GShellId) -> Option<TerminalSizeInfo> {
        let window_size = self.current_terminal_size_info().window_size();
        self.current_workspace_render_layout(window_size)
            .into_iter()
            .find(|placement| placement.target_id.value() == gshell_id.value())
            .map(|placement| self.terminal_size_info_for_surface(placement))
    }

    fn try_handle_keyboard_binding(
        &mut self,
        event_loop: &ActiveEventLoop,
        state: WindowInputElementState,
        logical_key: &WindowInputKey,
        physical_key: winit::keyboard::PhysicalKey,
    ) -> bool {
        let effective_modifiers = self.effective_input_modifiers();
        let action = self
            .config
            .keyboard
            .bindings
            .iter()
            .find(|binding| {
                matches_keyboard_binding(
                    binding,
                    effective_modifiers,
                    state,
                    logical_key,
                    physical_key,
                )
            })
            .map(|binding| binding.action);

        let Some(action) = action else {
            return false;
        };
        if matches!(
            action,
            KeyboardAction::FocusNextPane
                | KeyboardAction::FocusPreviousPane
                | KeyboardAction::FocusPaneLeft
                | KeyboardAction::FocusPaneRight
                | KeyboardAction::FocusPaneUp
                | KeyboardAction::FocusPaneDown
                | KeyboardAction::SwapPaneLeft
                | KeyboardAction::SwapPaneRight
                | KeyboardAction::SwapPaneUp
                | KeyboardAction::SwapPaneDown
                | KeyboardAction::ResizePaneLeft
                | KeyboardAction::ResizePaneRight
                | KeyboardAction::ResizePaneUp
                | KeyboardAction::ResizePaneDown
        ) && !self.pane_navigation_enabled
        {
            return false;
        }
        if matches!(
            action,
            KeyboardAction::MoveTabLeft | KeyboardAction::MoveTabRight
        ) && self.tab_count() < 2
        {
            return false;
        }

        if state == WindowInputElementState::Pressed {
            match action {
                KeyboardAction::NewWindow => self.create_window(),
                KeyboardAction::ToggleViMode => {
                    let gshell_id = self.focused_gshell();
                    self.clear_ime_preedit(gshell_id);
                    self.route_input_to_gshell(GShellInput {
                        gshell_id,
                        event: GShellInputEvent::ToggleViMode,
                    });
                    // Consume the shortcut chord immediately. On some Wayland
                    // compositor/input-stack combinations, modifier releases
                    // arrive late or not at all; carrying Ctrl into the first
                    // Vi key turns `d` into terminal Ctrl+D (EOF) and can close
                    // the last shell before Vi visibly updates.
                    self.reset_input_modifiers();
                }
                KeyboardAction::ToggleSearch => {
                    let gshell_id = self.focused_gshell();
                    self.clear_ime_preedit(gshell_id);
                    self.route_input_to_gshell(GShellInput {
                        gshell_id,
                        event: GShellInputEvent::ToggleSearch,
                    });
                }
                KeyboardAction::Copy => {
                    self.route_input_to_gshell(GShellInput {
                        gshell_id: self.focused_gshell(),
                        event: GShellInputEvent::CopySelection,
                    });
                }
                KeyboardAction::Paste => {
                    match self
                        .paste_controller
                        .clipboard_paste_input(self.focused_gshell())
                    {
                        Ok(Some(input)) => self.route_input_to_gshell(input),
                        Ok(None) => {
                            debug!("paste shortcut matched but clipboard text was empty");
                        }
                        Err(error) => warn!(error = %error, "failed to paste from clipboard"),
                    }
                }
                KeyboardAction::NewTab => self.create_workspace_tab(),
                KeyboardAction::NextTab => self.activate_next_workspace_tab(),
                KeyboardAction::PreviousTab => self.activate_previous_workspace_tab(),
                KeyboardAction::MoveTabLeft => self.move_active_workspace_tab_left(),
                KeyboardAction::MoveTabRight => self.move_active_workspace_tab_right(),
                KeyboardAction::SplitHorizontal => {
                    self.split_focused_workspace_pane(PaneSplitDirection::Horizontal);
                }
                KeyboardAction::SplitVertical => {
                    self.split_focused_workspace_pane(PaneSplitDirection::Vertical);
                }
                KeyboardAction::FocusNextPane => {
                    let previous_gshell = self.focused_gshell();
                    let focused_gshell = self.focus_next_gshell();
                    self.apply_gshell_focus_change(previous_gshell, focused_gshell);
                }
                KeyboardAction::FocusPreviousPane => {
                    let previous_gshell = self.focused_gshell();
                    let focused_gshell = self.focus_previous_gshell();
                    self.apply_gshell_focus_change(previous_gshell, focused_gshell);
                }
                KeyboardAction::FocusPaneLeft => self.focus_workspace_pane(PaneDirection::Left),
                KeyboardAction::FocusPaneRight => self.focus_workspace_pane(PaneDirection::Right),
                KeyboardAction::FocusPaneUp => self.focus_workspace_pane(PaneDirection::Up),
                KeyboardAction::FocusPaneDown => self.focus_workspace_pane(PaneDirection::Down),
                KeyboardAction::ClosePane => {
                    self.close_workspace_gshell(event_loop, self.focused_gshell());
                }
                KeyboardAction::SwapPaneLeft => {
                    self.swap_focused_workspace_pane(PaneDirection::Left);
                }
                KeyboardAction::SwapPaneRight => {
                    self.swap_focused_workspace_pane(PaneDirection::Right);
                }
                KeyboardAction::SwapPaneUp => {
                    self.swap_focused_workspace_pane(PaneDirection::Up);
                }
                KeyboardAction::SwapPaneDown => {
                    self.swap_focused_workspace_pane(PaneDirection::Down);
                }
                KeyboardAction::ResizePaneLeft => {
                    self.resize_focused_workspace_pane(PaneResizeDirection::Left);
                }
                KeyboardAction::ResizePaneRight => {
                    self.resize_focused_workspace_pane(PaneResizeDirection::Right);
                }
                KeyboardAction::ResizePaneUp => {
                    self.resize_focused_workspace_pane(PaneResizeDirection::Up);
                }
                KeyboardAction::ResizePaneDown => {
                    self.resize_focused_workspace_pane(PaneResizeDirection::Down);
                }
            }
        }

        true
    }

    fn effective_input_modifiers(&self) -> WindowInputModifiers {
        let physical_modifiers = self.paste_controller.effective_modifiers();
        WindowInputModifiers::new(
            self.window_input_modifiers.control_key() || physical_modifiers.control,
            self.window_input_modifiers.alt_key(),
            self.window_input_modifiers.shift_key() || physical_modifiers.shift,
            self.window_input_modifiers.super_key(),
        )
    }

    fn route_effective_input_modifiers(&mut self) {
        let modifiers = self.effective_input_modifiers();
        if modifiers == self.routed_input_modifiers {
            return;
        }

        self.routed_input_modifiers = modifiers;
        self.route_window_input(
            self.focused_gshell(),
            WindowInputEvent::ModifiersChanged(modifiers),
        );
    }

    fn reset_input_modifiers(&mut self) {
        self.window_input_modifiers = WindowInputModifiers::new(false, false, false, false);
        self.paste_controller.reset_modifiers();
        self.route_effective_input_modifiers();
    }

    fn write_selection_to_clipboard(&mut self, gshell_id: GShellId, text: Option<String>) {
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            debug!(
                gshell_id = gshell_id.value(),
                "copy shortcut matched without an active selection"
            );
            return;
        };

        if let Err(error) = self.paste_controller.write_clipboard_text(text) {
            warn!(gshell_id = gshell_id.value(), error = %error, "failed to copy terminal selection");
        }
    }

    fn osc52_clipboard_access_allowed(&self, gshell_id: GShellId) -> bool {
        let allowed = self.window_focused && self.focused_gshell() == gshell_id;
        if !allowed {
            debug!(
                gshell_id = gshell_id.value(),
                "ignored OSC 52 clipboard access from an unfocused terminal"
            );
        }
        allowed
    }

    fn show_terminal_notification(&self, gshell_id: GShellId, notification: TerminalNotification) {
        let allowed = match notification.occasion {
            TerminalNotificationOccasion::Always => true,
            TerminalNotificationOccasion::Unfocused => {
                !self.window_focused || self.focused_gshell() != gshell_id
            }
            TerminalNotificationOccasion::Invisible => {
                !self.window_focused || !self.visible_gshells().contains(&gshell_id)
            }
        };
        if allowed {
            self.system_notifier.show(gshell_id, notification);
        } else {
            debug!(
                gshell_id = gshell_id.value(),
                "ignored terminal notification because its visibility condition was not met"
            );
        }
    }

    fn try_focus_pane_at_cursor(&mut self) -> bool {
        let Some(cursor_position) = self.cursor_position else {
            return false;
        };
        let window_size = self.current_terminal_size_info().window_size();
        let placements = self.current_workspace_render_layout(window_size);
        let Some(target_id) = render_target_at_position(&placements, cursor_position) else {
            debug!(
                x = cursor_position.x,
                y = cursor_position.y,
                "pane focus click missed workspace"
            );
            return false;
        };
        let gshell_id = GShellId::new(target_id.value());
        let previous_gshell = self.focused_gshell();

        if !self.focus_gshell(gshell_id) {
            debug!(
                target_id = target_id.value(),
                "pane focus click resolved to an unknown target"
            );
            return false;
        }

        debug!(
            x = cursor_position.x,
            y = cursor_position.y,
            target_id = target_id.value(),
            "focused pane from pointer"
        );
        self.apply_gshell_focus_change(previous_gshell, gshell_id);
        true
    }

    fn apply_gshell_focus_change(&mut self, previous: GShellId, focused: GShellId) {
        if previous != focused {
            self.clear_ime_preedit(previous);
        }
        if previous != focused && self.window_focused {
            self.route_focus_changed(previous, false);
            self.route_focus_changed(focused, true);
        }
        self.set_focused_render_target(RenderTargetId::new(focused.value()));
        self.sync_current_terminal_window_title();
        self.set_tab_bar(self.current_tab_bar_snapshot());
        self.update_ime_cursor_area();
    }

    fn clear_ime_preedit(&mut self, gshell_id: GShellId) {
        self.set_ime_preedit(RenderTargetId::new(gshell_id.value()), None);
    }

    fn update_ime_cursor_area(&self) {
        if !self.ime_enabled {
            return;
        }
        let Some(runtime) = self.render_runtime.as_ref() else {
            return;
        };
        let _ = runtime.update_ime_cursor_area(RenderTargetId::new(self.focused_gshell().value()));
    }

    fn route_focus_changed(&mut self, gshell_id: GShellId, focused: bool) {
        self.route_window_input(gshell_id, WindowInputEvent::FocusChanged(focused));
    }

    fn route_window_input(&mut self, gshell_id: GShellId, event: WindowInputEvent) {
        let target_id = RenderTargetId::new(gshell_id.value());
        if self
            .render_runtime
            .as_mut()
            .is_some_and(|runtime| runtime.route_wgpu_pane_input(target_id, &event))
        {
            return;
        }

        self.route_input_to_gshell(GShellInput {
            gshell_id,
            event: GShellInputEvent::Window(event),
        });
    }

    fn pointer_input_at(
        &self,
        position: PhysicalPosition<f64>,
    ) -> Option<(GShellId, WindowPointerPosition)> {
        let window_size = self.current_terminal_size_info().window_size();
        let placements = self.current_workspace_render_layout(window_size);
        let placement = *render_surface_at_position(&placements, position)?;
        let size_info = self.terminal_size_info_for_surface(placement);
        let viewport = size_info.render_viewport();
        Some((
            GShellId::new(placement.target_id.value()),
            surface_local_pointer_position(
                placement,
                viewport.origin_x_px(),
                viewport.origin_y_px(),
                position,
            ),
        ))
    }

    fn captured_pointer_input_at(
        &self,
        gshell_id: GShellId,
        position: PhysicalPosition<f64>,
    ) -> Option<WindowPointerPosition> {
        let window_size = self.current_terminal_size_info().window_size();
        let placements = self.current_workspace_render_layout(window_size);
        let placement = placements
            .into_iter()
            .find(|placement| placement.target_id.value() == gshell_id.value())?;
        let size_info = self.terminal_size_info_for_surface(placement);
        let viewport = size_info.render_viewport();
        let local_position = surface_local_pointer_position(
            placement,
            viewport.origin_x_px(),
            viewport.origin_y_px(),
            position,
        );
        clamp_pointer_position_to_content(
            local_position,
            size_info.content_width_px(),
            size_info.content_height_px(),
        )
    }

    fn try_open_hyperlink_at_cursor(&self) -> bool {
        let Some(position) = self.cursor_position else {
            return false;
        };
        let Some((gshell_id, local_position)) = self.pointer_input_at(position) else {
            return false;
        };
        if local_position.x_px < 0.0 || local_position.y_px < 0.0 {
            return false;
        }
        let Some(size_info) = self.current_gshell_size_info(gshell_id) else {
            return false;
        };
        let cell_size = size_info.cell_size();
        let x = (local_position.x_px / f64::from(cell_size.width_px().max(1))) as u32;
        let y = (local_position.y_px / f64::from(cell_size.height_px().max(1))) as u32;
        let Some(uri) = self
            .terminal_hyperlinks
            .get(&gshell_id)
            .and_then(|hyperlinks| hyperlinks.iter().find(|link| link.contains(x, y)))
            .map(|link| link.uri.clone())
        else {
            return false;
        };

        match open_terminal_hyperlink(&uri) {
            Ok(()) => true,
            Err(error) => {
                warn!(%uri, %error, "failed to open terminal hyperlink");
                false
            }
        }
    }

    fn route_pointer_moved(&mut self, position: PhysicalPosition<f64>) {
        if let Some(capture) = self.pointer_capture {
            if let Some(local_position) =
                self.captured_pointer_input_at(capture.gshell_id, position)
            {
                self.pointer_capture = Some(PointerCapture {
                    position: local_position,
                    ..capture
                });
                self.pointer_gshell = Some(capture.gshell_id);
                self.route_window_input(
                    capture.gshell_id,
                    WindowInputEvent::PointerMoved {
                        position: local_position,
                        modifiers: self.window_input_modifiers,
                    },
                );
                return;
            }
            self.pointer_capture = None;
        }

        let Some((gshell_id, local_position)) = self.pointer_input_at(position) else {
            self.route_pointer_left();
            return;
        };

        if self.pointer_gshell != Some(gshell_id) {
            self.route_pointer_left();
            self.pointer_gshell = Some(gshell_id);
        }
        self.route_window_input(
            gshell_id,
            WindowInputEvent::PointerMoved {
                position: local_position,
                modifiers: self.window_input_modifiers,
            },
        );
    }

    fn route_pointer_button(&mut self, state: ElementState, button: MouseButton) {
        let captured = if button == MouseButton::Left && state == ElementState::Released {
            self.pointer_capture
        } else {
            None
        };
        let input = captured
            .map(|capture| {
                let position = self
                    .cursor_position
                    .and_then(|position| {
                        self.captured_pointer_input_at(capture.gshell_id, position)
                    })
                    .unwrap_or(capture.position);
                (capture.gshell_id, position)
            })
            .or_else(|| {
                self.cursor_position
                    .and_then(|position| self.pointer_input_at(position))
            });

        if button == MouseButton::Left && state == ElementState::Pressed {
            self.pointer_capture = input.map(|(gshell_id, position)| PointerCapture {
                gshell_id,
                position,
            });
        }

        if let Some((gshell_id, position)) = input {
            self.route_window_input(
                gshell_id,
                WindowInputEvent::PointerButton {
                    state: winit_element_state_to_port(state),
                    button: winit_mouse_button_to_port(button),
                    position,
                    modifiers: self.window_input_modifiers,
                },
            );
        }

        if button == MouseButton::Left && state == ElementState::Released {
            self.pointer_capture = None;
            if let Some(position) = self.cursor_position {
                self.route_pointer_moved(position);
            } else {
                self.route_pointer_left();
            }
        }
    }

    fn route_pointer_left(&mut self) {
        let Some(gshell_id) = self.pointer_gshell.take() else {
            return;
        };
        self.route_window_input(gshell_id, WindowInputEvent::PointerLeft);
    }

    #[cfg(all(feature = "media-gstreamer", target_os = "linux"))]
    fn drain_media_bridge_frames(&self) {
        let Some(render_runtime) = self.render_runtime.as_ref() else {
            return;
        };

        for pending in self.media_bridge.drain_pending_video_surface_frames() {
            let import_result = render_runtime
                .import_video_surface_dma_buf_frame(&pending.surface_id, &pending.frame);
            if let Err(error) = import_result {
                warn!(surface_id = %pending.surface_id, error = %error, "failed to import video surface frame");
            }
        }
    }

    #[cfg(not(all(feature = "media-gstreamer", target_os = "linux")))]
    fn drain_media_bridge_frames(&self) {}

    fn exit_and_persist(&self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.persist_workspace() {
            error!(error = %error, "failed to persist workspace");
        }

        event_loop.exit();
    }
}

impl ApplicationHandler<RuntimeEvent> for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, _cause: StartCause) {
        let now = Instant::now();
        if self
            .render_runtime
            .as_mut()
            .is_some_and(|runtime| runtime.take_due_render_deadline(now))
        {
            self.request_redraw();
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.ensure_window_runtime(event_loop) {
            error!(error = %error, "failed to initialize Germinal window runtime");
            self.exit_and_persist(event_loop);
            return;
        }

        self.ensure_workspace_gshells();
        let focused_gshell = self.focused_gshell();
        self.set_focused_render_target(RenderTargetId::new(focused_gshell.value()));
        if self.window_focused {
            self.route_focus_changed(focused_gshell, true);
        }
        self.prepare_render_backend();
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: RuntimeEvent) {
        match event {
            RuntimeEvent::GShell(GShellRuntimeEvent::EnterGNative { gshell_id }) => {
                self.clear_ime_preedit(gshell_id);
                self.begin_gnative_mode(gshell_id);
                if let Err(error) = self.begin_gnative_session(gshell_id) {
                    self.exit_gnative_mode(gshell_id);
                    error!(gshell_id = gshell_id.value(), error = %error, "failed to enter gnative session");
                }
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::GNativeConnected { accepted }) => {
                let gshell_id = accepted.gshell_id;
                self.reset_surface_sequence(RenderTargetId::new(gshell_id.value()));
                self.activate_gnative_session(accepted);
                self.enter_gnative_mode(gshell_id);
                if let Some(size_info) = self.current_gshell_size_info(gshell_id) {
                    self.resize_gshell(gshell_id, size_info);
                }
                self.route_focus_changed(
                    gshell_id,
                    self.window_focused && self.focused_gshell() == gshell_id,
                );
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::GNativeConnectionFailed {
                gshell_id,
                reason,
            }) => {
                self.fail_gnative_session(gshell_id);
                self.exit_gnative_mode(gshell_id);
                self.consume_latest_terminal_snapshot();
                self.request_redraw();
                error!(gshell_id = gshell_id.value(), %reason, "failed to connect gnative session");
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::ExitGNative { gshell_id }) => {
                self.clear_ime_preedit(gshell_id);
                self.exit_gnative_session(gshell_id);
                self.exit_gnative_mode(gshell_id);
                self.reset_surface_sequence(RenderTargetId::new(gshell_id.value()));
                self.consume_latest_terminal_snapshot();
                self.request_redraw();
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady { .. }) => {
                self.consume_latest_terminal_snapshot();
                self.update_ime_cursor_area();
                self.request_redraw();
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::Bell { .. }) => {
                self.audible_bell.ring();
                self.ring_bell(
                    Duration::from_millis(self.config.bell.duration_ms),
                    self.config.bell.urgent_on_unfocused && !self.window_focused,
                );
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::SystemNotificationRequested {
                gshell_id,
                notification,
            }) => {
                self.show_terminal_notification(gshell_id, notification);
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::SystemNotificationActivated { gshell_id }) => {
                let previous_gshell = self.focused_gshell();
                if self.focus_gshell(gshell_id) {
                    self.activate_workspace_tab(previous_gshell, gshell_id);
                }
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::Osc52ClipboardStore {
                gshell_id,
                clipboard,
                text,
            }) => {
                if self.osc52_clipboard_access_allowed(gshell_id)
                    && let Err(error) = self
                        .paste_controller
                        .write_terminal_clipboard_text(clipboard, text)
                {
                    warn!(gshell_id = gshell_id.value(), error = %error, "failed to handle OSC 52 clipboard store");
                }
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::Osc52ClipboardLoad {
                gshell_id,
                clipboard,
                request_id,
            }) => {
                let text = if self.osc52_clipboard_access_allowed(gshell_id) {
                    match self
                        .paste_controller
                        .read_terminal_clipboard_text(clipboard)
                    {
                        Ok(text) => Some(text),
                        Err(error) => {
                            warn!(gshell_id = gshell_id.value(), error = %error, "failed to handle OSC 52 clipboard load");
                            None
                        }
                    }
                } else {
                    None
                };
                self.route_input_to_gshell(GShellInput {
                    gshell_id,
                    event: GShellInputEvent::Osc52ClipboardLoadResponse {
                        clipboard,
                        request_id,
                        text,
                    },
                });
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::TitleChanged { gshell_id, title }) => {
                self.update_gshell_title(gshell_id, title);
                self.sync_current_terminal_window_title();
                self.set_tab_bar(self.current_tab_bar_snapshot());
                self.request_redraw();
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::WorkingDirectoryChanged {
                gshell_id,
                working_directory,
            }) => {
                self.report_gshell_working_directory(gshell_id, working_directory.clone());
                self.update_gshell_working_directory(gshell_id, working_directory);
                self.sync_current_terminal_window_title();
                self.set_tab_bar(self.current_tab_bar_snapshot());
                self.request_redraw();
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::CommandChanged { gshell_id, command }) => {
                self.update_gshell_command(gshell_id, command);
                self.sync_current_terminal_window_title();
                self.set_tab_bar(self.current_tab_bar_snapshot());
                self.request_redraw();
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::ProgressChanged {
                gshell_id,
                progress,
            }) => {
                self.update_gshell_progress(gshell_id, progress);
                self.set_tab_bar(self.current_tab_bar_snapshot());
                self.request_redraw();
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::HyperlinksChanged {
                gshell_id,
                hyperlinks,
            }) => {
                if hyperlinks.is_empty() {
                    self.terminal_hyperlinks.remove(&gshell_id);
                } else {
                    self.terminal_hyperlinks.insert(gshell_id, hyperlinks);
                }
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::SelectionText { gshell_id, text }) => {
                self.write_selection_to_clipboard(gshell_id, text);
            }
            RuntimeEvent::GShell(GShellRuntimeEvent::Closed { gshell_id }) => {
                self.close_workspace_gshell(event_loop, gshell_id);
            }
            RuntimeEvent::App(_) => {
                self.exit_and_persist(event_loop);
            }
            RuntimeEvent::Workspace(WorkspaceRuntimeEvent::RedrawRequested) => {
                self.drain_media_bridge_frames();
                self.request_redraw();
            }
            RuntimeEvent::Workspace(WorkspaceRuntimeEvent::SplitFocusedPane { direction }) => {
                self.split_focused_workspace_pane(direction);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(current_window_id) = self.current_window_id() else {
            return;
        };

        if window_id != current_window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => {
                self.exit_and_persist(event_loop);
            }
            WindowEvent::Moved(_) => {
                if let Some(render_runtime) = self.render_runtime.as_mut()
                    && render_runtime.refresh_display_timing()
                {
                    self.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                let size_info = self.resize_window_size_info(TerminalWindowSize::new(
                    size.width.max(1),
                    size.height.max(1),
                ));
                self.resize_workspace_gshells(size_info.window_size());
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let Some(render_runtime) = self.render_runtime.as_mut() else {
                    return;
                };
                match render_runtime.update_scale_factor(scale_factor) {
                    Ok(size_info) => self.resize_workspace_gshells(size_info.window_size()),
                    Err(source) => {
                        warn!(scale_factor, %source, "failed to update terminal DPI scale factor");
                    }
                }
            }
            WindowEvent::Focused(focused) => {
                if self.window_focused != focused {
                    self.window_focused = focused;
                    if !focused {
                        self.clear_ime_preedit(self.focused_gshell());
                        self.pointer_capture = None;
                        self.route_pointer_left();
                        // Some compositors do not deliver modifier key releases after
                        // focus moves away. Never carry those stale keys into the next
                        // focus session, where exact shortcuts would otherwise stop
                        // matching and normal input could be routed as Ctrl/Shift input.
                        self.reset_input_modifiers();
                    }
                    self.route_focus_changed(self.focused_gshell(), focused);
                }
                self.set_window_focused(focused);
            }
            WindowEvent::RedrawRequested => {
                self.drain_media_bridge_frames();
                self.present_workspace();
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.window_input_modifiers = WindowInputModifiers::new(
                    modifiers.state().control_key(),
                    modifiers.state().alt_key(),
                    modifiers.state().shift_key(),
                    modifiers.state().super_key(),
                );
                self.paste_controller.set_modifiers(HostPasteModifiers {
                    control: modifiers.state().control_key(),
                    shift: modifiers.state().shift_key(),
                });
                self.route_effective_input_modifiers();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = Some(position);
                self.route_pointer_moved(position);
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor_position = None;
                if self.pointer_capture.is_none() {
                    self.route_pointer_left();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    if state == ElementState::Pressed
                        && self.window_input_modifiers.control_key()
                        && self.try_open_hyperlink_at_cursor()
                    {
                        self.hyperlink_pointer_consumed = true;
                        return;
                    }
                    if state == ElementState::Released && self.hyperlink_pointer_consumed {
                        self.hyperlink_pointer_consumed = false;
                        return;
                    }
                }
                if state == ElementState::Pressed && button == MouseButton::Left {
                    self.try_focus_pane_at_cursor();
                }
                self.route_pointer_button(state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(position) = self.cursor_position
                    && let Some((gshell_id, local_position)) = self.pointer_input_at(position)
                {
                    self.route_window_input(
                        gshell_id,
                        WindowInputEvent::Scroll {
                            delta: winit_scroll_delta_to_port(delta),
                            position: local_position,
                            modifiers: self.window_input_modifiers,
                        },
                    );
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let winit::event::KeyEvent {
                    state,
                    repeat,
                    logical_key,
                    physical_key,
                    text,
                    ..
                } = event;
                let logical_key = winit_key_to_port(logical_key);
                let state = winit_element_state_to_port(state);
                let text = text.map(|text| text.to_string());
                self.paste_controller.observe_key_event(state, physical_key);
                self.route_effective_input_modifiers();

                if self.try_handle_keyboard_binding(event_loop, state, &logical_key, physical_key) {
                    return;
                }

                let focused_gshell = self.focused_gshell();
                let plugin_event = WindowInputEvent::Key {
                    state,
                    repeat,
                    logical_key: logical_key.clone(),
                    text: text.clone(),
                };
                if self.render_runtime.as_mut().is_some_and(|runtime| {
                    runtime.route_wgpu_pane_input(
                        RenderTargetId::new(focused_gshell.value()),
                        &plugin_event,
                    )
                }) {
                    return;
                }

                self.route_window_input(focused_gshell, plugin_event);
            }
            WindowEvent::Ime(Ime::Enabled) => {
                self.ime_enabled = true;
                self.route_window_input(self.focused_gshell(), WindowInputEvent::ImeEnabled);
                self.update_ime_cursor_area();
            }
            WindowEvent::Ime(Ime::Preedit(text, cursor_range)) => {
                let focused_gshell = self.focused_gshell();
                let target_id = RenderTargetId::new(focused_gshell.value());
                if self.render_runtime.as_mut().is_some_and(|runtime| {
                    runtime.route_wgpu_pane_input(
                        target_id,
                        &WindowInputEvent::ImePreedit {
                            text: text.clone(),
                            cursor_range,
                        },
                    )
                }) {
                    return;
                }
                self.route_window_input(
                    focused_gshell,
                    WindowInputEvent::ImePreedit {
                        text: text.clone(),
                        cursor_range,
                    },
                );
                let preedit = (!text.is_empty())
                    .then_some(RenderSurfaceImePreeditSnapshot { text, cursor_range });
                self.set_ime_preedit(target_id, preedit);
                self.update_ime_cursor_area();
            }
            WindowEvent::Ime(Ime::Commit(text)) => {
                let focused_gshell = self.focused_gshell();
                self.clear_ime_preedit(focused_gshell);
                self.route_window_input(focused_gshell, WindowInputEvent::Ime(text));
                self.update_ime_cursor_area();
            }
            WindowEvent::Ime(Ime::Disabled) => {
                self.ime_enabled = false;
                let focused_gshell = self.focused_gshell();
                self.route_window_input(focused_gshell, WindowInputEvent::ImeDisabled);
                self.clear_ime_preedit(focused_gshell);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.update_ime_cursor_area();
        self.flush_redraw_request();
        let control_flow = self
            .render_runtime
            .as_ref()
            .and_then(WgpuTerminalWindowRuntime::next_render_deadline)
            .map_or(ControlFlow::Wait, ControlFlow::WaitUntil);
        event_loop.set_control_flow(control_flow);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WorkspaceContentLayout {
    y_px: u32,
    content_size: TerminalWindowSize,
}

fn workspace_content_layout(
    window_size: TerminalWindowSize,
    cell_height_px: u32,
    show_tab_bar: bool,
    position: TabBarPosition,
) -> WorkspaceContentLayout {
    let bar_height_px = if show_tab_bar {
        cell_height_px.min(window_size.height_px().saturating_sub(1))
    } else {
        0
    };
    WorkspaceContentLayout {
        y_px: if position == TabBarPosition::Top {
            bar_height_px
        } else {
            0
        },
        content_size: TerminalWindowSize::new(
            window_size.width_px(),
            window_size.height_px().saturating_sub(bar_height_px).max(1),
        ),
    }
}

fn matches_keyboard_binding(
    binding: &KeyboardBinding,
    modifiers: WindowInputModifiers,
    state: WindowInputElementState,
    logical_key: &WindowInputKey,
    physical_key: winit::keyboard::PhysicalKey,
) -> bool {
    matches!(
        state,
        WindowInputElementState::Pressed | WindowInputElementState::Released
    ) && matches_binding_modifiers(&binding.mods, modifiers)
        && matches_binding_key(&binding.key, logical_key, physical_key)
}

fn matches_binding_modifiers(spec: &str, actual: WindowInputModifiers) -> bool {
    let mut control = false;
    let mut alt = false;
    let mut shift = false;
    let mut super_key = false;

    for modifier in spec
        .split('|')
        .map(str::trim)
        .filter(|modifier| !modifier.is_empty())
    {
        if modifier.eq_ignore_ascii_case("control") {
            control = true;
        } else if modifier.eq_ignore_ascii_case("alt") {
            alt = true;
        } else if modifier.eq_ignore_ascii_case("shift") {
            shift = true;
        } else if modifier.eq_ignore_ascii_case("super") {
            super_key = true;
        } else {
            return false;
        }
    }

    actual.control_key() == control
        && actual.alt_key() == alt
        && actual.shift_key() == shift
        && actual.super_key() == super_key
}

fn matches_binding_key(
    spec: &str,
    actual: &WindowInputKey,
    physical_key: winit::keyboard::PhysicalKey,
) -> bool {
    if let winit::keyboard::PhysicalKey::Code(key_code) = physical_key
        && physical_key_name(key_code).is_some_and(|name| spec.eq_ignore_ascii_case(name))
    {
        return true;
    }

    match actual {
        WindowInputKey::Character(character) => {
            (spec.eq_ignore_ascii_case("space") && character == " ")
                || spec.eq_ignore_ascii_case(character)
        }
        WindowInputKey::Named(named) => spec.eq_ignore_ascii_case(named_key_name(*named)),
        WindowInputKey::Unidentified => false,
    }
}

fn physical_key_name(key: winit::keyboard::KeyCode) -> Option<&'static str> {
    use winit::keyboard::KeyCode;

    Some(match key {
        KeyCode::KeyA => "A",
        KeyCode::KeyB => "B",
        KeyCode::KeyC => "C",
        KeyCode::KeyD => "D",
        KeyCode::KeyE => "E",
        KeyCode::KeyF => "F",
        KeyCode::KeyG => "G",
        KeyCode::KeyH => "H",
        KeyCode::KeyI => "I",
        KeyCode::KeyJ => "J",
        KeyCode::KeyK => "K",
        KeyCode::KeyL => "L",
        KeyCode::KeyM => "M",
        KeyCode::KeyN => "N",
        KeyCode::KeyO => "O",
        KeyCode::KeyP => "P",
        KeyCode::KeyQ => "Q",
        KeyCode::KeyR => "R",
        KeyCode::KeyS => "S",
        KeyCode::KeyT => "T",
        KeyCode::KeyU => "U",
        KeyCode::KeyV => "V",
        KeyCode::KeyW => "W",
        KeyCode::KeyX => "X",
        KeyCode::KeyY => "Y",
        KeyCode::KeyZ => "Z",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::Space => "Space",
        KeyCode::Enter => "Enter",
        KeyCode::Tab => "Tab",
        KeyCode::Backspace => "Backspace",
        KeyCode::Escape => "Escape",
        KeyCode::ArrowUp => "Up",
        KeyCode::ArrowDown => "Down",
        KeyCode::ArrowRight => "Right",
        KeyCode::ArrowLeft => "Left",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::Insert => "Insert",
        KeyCode::Delete => "Delete",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::F1 => "F1",
        KeyCode::F2 => "F2",
        KeyCode::F3 => "F3",
        KeyCode::F4 => "F4",
        KeyCode::F5 => "F5",
        KeyCode::F6 => "F6",
        KeyCode::F7 => "F7",
        KeyCode::F8 => "F8",
        KeyCode::F9 => "F9",
        KeyCode::F10 => "F10",
        KeyCode::F11 => "F11",
        KeyCode::F12 => "F12",
        KeyCode::CapsLock => "CapsLock",
        KeyCode::ScrollLock => "ScrollLock",
        KeyCode::NumLock => "NumLock",
        KeyCode::PrintScreen => "PrintScreen",
        KeyCode::Pause => "Pause",
        KeyCode::ContextMenu => "ContextMenu",
        _ => return None,
    })
}

fn named_key_name(key: WindowInputNamedKey) -> &'static str {
    match key {
        WindowInputNamedKey::F1 => "F1",
        WindowInputNamedKey::F2 => "F2",
        WindowInputNamedKey::F3 => "F3",
        WindowInputNamedKey::F4 => "F4",
        WindowInputNamedKey::F5 => "F5",
        WindowInputNamedKey::F6 => "F6",
        WindowInputNamedKey::F7 => "F7",
        WindowInputNamedKey::F8 => "F8",
        WindowInputNamedKey::F9 => "F9",
        WindowInputNamedKey::F10 => "F10",
        WindowInputNamedKey::F11 => "F11",
        WindowInputNamedKey::F12 => "F12",
        WindowInputNamedKey::Enter => "Enter",
        WindowInputNamedKey::Tab => "Tab",
        WindowInputNamedKey::Backspace => "Backspace",
        WindowInputNamedKey::Escape => "Escape",
        WindowInputNamedKey::ArrowUp => "Up",
        WindowInputNamedKey::ArrowDown => "Down",
        WindowInputNamedKey::ArrowRight => "Right",
        WindowInputNamedKey::ArrowLeft => "Left",
        WindowInputNamedKey::Home => "Home",
        WindowInputNamedKey::End => "End",
        WindowInputNamedKey::Insert => "Insert",
        WindowInputNamedKey::Delete => "Delete",
        WindowInputNamedKey::PageUp => "PageUp",
        WindowInputNamedKey::PageDown => "PageDown",
        WindowInputNamedKey::CapsLock => "CapsLock",
        WindowInputNamedKey::ScrollLock => "ScrollLock",
        WindowInputNamedKey::NumLock => "NumLock",
        WindowInputNamedKey::PrintScreen => "PrintScreen",
        WindowInputNamedKey::Pause => "Pause",
        WindowInputNamedKey::ContextMenu => "ContextMenu",
        WindowInputNamedKey::Shift => "Shift",
        WindowInputNamedKey::Control => "Control",
        WindowInputNamedKey::Alt => "Alt",
        WindowInputNamedKey::Super => "Super",
    }
}

fn render_target_at_position(
    placements: &[RenderSurfacePlacement],
    position: PhysicalPosition<f64>,
) -> Option<RenderTargetId> {
    render_surface_at_position(placements, position).map(|placement| placement.target_id)
}

fn directional_neighbor_target(
    placements: &[RenderSurfacePlacement],
    focused_target: RenderTargetId,
    direction: PaneDirection,
) -> Option<RenderTargetId> {
    let focused = placements
        .iter()
        .find(|placement| placement.target_id == focused_target)?;
    let focused_left = u64::from(focused.x_px);
    let focused_top = u64::from(focused.y_px);
    let focused_right = focused_left + u64::from(focused.width_px);
    let focused_bottom = focused_top + u64::from(focused.height_px);

    placements
        .iter()
        .filter(|candidate| candidate.target_id != focused_target)
        .filter_map(|candidate| {
            let left = u64::from(candidate.x_px);
            let top = u64::from(candidate.y_px);
            let right = left + u64::from(candidate.width_px);
            let bottom = top + u64::from(candidate.height_px);
            let (primary_gap, perpendicular_distance, overlap) = match direction {
                PaneDirection::Left if right <= focused_left => (
                    focused_left - right,
                    (top + bottom).abs_diff(focused_top + focused_bottom),
                    axis_overlap(top, bottom, focused_top, focused_bottom),
                ),
                PaneDirection::Right if left >= focused_right => (
                    left - focused_right,
                    (top + bottom).abs_diff(focused_top + focused_bottom),
                    axis_overlap(top, bottom, focused_top, focused_bottom),
                ),
                PaneDirection::Up if bottom <= focused_top => (
                    focused_top - bottom,
                    (left + right).abs_diff(focused_left + focused_right),
                    axis_overlap(left, right, focused_left, focused_right),
                ),
                PaneDirection::Down if top >= focused_bottom => (
                    top - focused_bottom,
                    (left + right).abs_diff(focused_left + focused_right),
                    axis_overlap(left, right, focused_left, focused_right),
                ),
                _ => return None,
            };
            (overlap > 0).then_some((
                primary_gap,
                perpendicular_distance,
                u64::MAX - overlap,
                candidate.target_id.value(),
                candidate.target_id,
            ))
        })
        .min_by_key(|score| (score.0, score.1, score.2, score.3))
        .map(|score| score.4)
}

fn axis_overlap(first_start: u64, first_end: u64, second_start: u64, second_end: u64) -> u64 {
    first_end
        .min(second_end)
        .saturating_sub(first_start.max(second_start))
}

fn render_surface_at_position(
    placements: &[RenderSurfacePlacement],
    position: PhysicalPosition<f64>,
) -> Option<&RenderSurfacePlacement> {
    if !position.x.is_finite() || !position.y.is_finite() || position.x < 0.0 || position.y < 0.0 {
        return None;
    }

    placements.iter().find(|placement| {
        position.x >= f64::from(placement.x_px)
            && position.x < f64::from(placement.x_px.saturating_add(placement.width_px))
            && position.y >= f64::from(placement.y_px)
            && position.y < f64::from(placement.y_px.saturating_add(placement.height_px))
    })
}

fn surface_local_pointer_position(
    placement: RenderSurfacePlacement,
    content_origin_x_px: u32,
    content_origin_y_px: u32,
    position: PhysicalPosition<f64>,
) -> WindowPointerPosition {
    WindowPointerPosition::new(
        position.x - f64::from(placement.x_px) - f64::from(content_origin_x_px),
        position.y - f64::from(placement.y_px) - f64::from(content_origin_y_px),
    )
}

fn clamp_pointer_position_to_content(
    position: WindowPointerPosition,
    content_width_px: u32,
    content_height_px: u32,
) -> Option<WindowPointerPosition> {
    if !position.x_px.is_finite() || !position.y_px.is_finite() {
        return None;
    }

    Some(WindowPointerPosition::new(
        position
            .x_px
            .clamp(0.0, f64::from(content_width_px.saturating_sub(1))),
        position
            .y_px
            .clamp(0.0, f64::from(content_height_px.saturating_sub(1))),
    ))
}

fn open_terminal_hyperlink(uri: &str) -> Result<(), String> {
    if !is_supported_terminal_hyperlink(uri) {
        return Err("unsupported or unsafe URI scheme".to_string());
    }

    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    };

    command
        .arg(uri)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn is_supported_terminal_hyperlink(uri: &str) -> bool {
    if uri.is_empty() || uri.chars().any(char::is_control) {
        return false;
    }
    let Some((scheme, _)) = uri.split_once(':') else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto" | "file"
    )
}

fn winit_mouse_button_to_port(button: MouseButton) -> WindowPointerButton {
    match button {
        MouseButton::Left => WindowPointerButton::Primary,
        MouseButton::Right => WindowPointerButton::Secondary,
        MouseButton::Middle => WindowPointerButton::Middle,
        MouseButton::Back => WindowPointerButton::Back,
        MouseButton::Forward => WindowPointerButton::Forward,
        MouseButton::Other(value) => WindowPointerButton::Other(value),
    }
}

fn winit_scroll_delta_to_port(delta: MouseScrollDelta) -> WindowScrollDelta {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => WindowScrollDelta::Lines { x, y },
        MouseScrollDelta::PixelDelta(position) => WindowScrollDelta::Pixels {
            x: position.x,
            y: position.y,
        },
    }
}

fn winit_element_state_to_port(state: ElementState) -> WindowInputElementState {
    match state {
        ElementState::Pressed => WindowInputElementState::Pressed,
        ElementState::Released => WindowInputElementState::Released,
    }
}

fn winit_key_to_port(key: Key) -> WindowInputKey {
    match key {
        Key::Named(named) => match named {
            NamedKey::Space => WindowInputKey::Character(" ".to_string()),
            NamedKey::F1 => WindowInputKey::Named(WindowInputNamedKey::F1),
            NamedKey::F2 => WindowInputKey::Named(WindowInputNamedKey::F2),
            NamedKey::F3 => WindowInputKey::Named(WindowInputNamedKey::F3),
            NamedKey::F4 => WindowInputKey::Named(WindowInputNamedKey::F4),
            NamedKey::F5 => WindowInputKey::Named(WindowInputNamedKey::F5),
            NamedKey::F6 => WindowInputKey::Named(WindowInputNamedKey::F6),
            NamedKey::F7 => WindowInputKey::Named(WindowInputNamedKey::F7),
            NamedKey::F8 => WindowInputKey::Named(WindowInputNamedKey::F8),
            NamedKey::F9 => WindowInputKey::Named(WindowInputNamedKey::F9),
            NamedKey::F10 => WindowInputKey::Named(WindowInputNamedKey::F10),
            NamedKey::F11 => WindowInputKey::Named(WindowInputNamedKey::F11),
            NamedKey::F12 => WindowInputKey::Named(WindowInputNamedKey::F12),
            NamedKey::Enter => WindowInputKey::Named(WindowInputNamedKey::Enter),
            NamedKey::Tab => WindowInputKey::Named(WindowInputNamedKey::Tab),
            NamedKey::Backspace => WindowInputKey::Named(WindowInputNamedKey::Backspace),
            NamedKey::Escape => WindowInputKey::Named(WindowInputNamedKey::Escape),
            NamedKey::ArrowUp => WindowInputKey::Named(WindowInputNamedKey::ArrowUp),
            NamedKey::ArrowDown => WindowInputKey::Named(WindowInputNamedKey::ArrowDown),
            NamedKey::ArrowRight => WindowInputKey::Named(WindowInputNamedKey::ArrowRight),
            NamedKey::ArrowLeft => WindowInputKey::Named(WindowInputNamedKey::ArrowLeft),
            NamedKey::Home => WindowInputKey::Named(WindowInputNamedKey::Home),
            NamedKey::End => WindowInputKey::Named(WindowInputNamedKey::End),
            NamedKey::Insert => WindowInputKey::Named(WindowInputNamedKey::Insert),
            NamedKey::Delete => WindowInputKey::Named(WindowInputNamedKey::Delete),
            NamedKey::PageUp => WindowInputKey::Named(WindowInputNamedKey::PageUp),
            NamedKey::PageDown => WindowInputKey::Named(WindowInputNamedKey::PageDown),
            NamedKey::CapsLock => WindowInputKey::Named(WindowInputNamedKey::CapsLock),
            NamedKey::ScrollLock => WindowInputKey::Named(WindowInputNamedKey::ScrollLock),
            NamedKey::NumLock => WindowInputKey::Named(WindowInputNamedKey::NumLock),
            NamedKey::PrintScreen => WindowInputKey::Named(WindowInputNamedKey::PrintScreen),
            NamedKey::Pause => WindowInputKey::Named(WindowInputNamedKey::Pause),
            NamedKey::ContextMenu => WindowInputKey::Named(WindowInputNamedKey::ContextMenu),
            NamedKey::Shift => WindowInputKey::Named(WindowInputNamedKey::Shift),
            NamedKey::Control => WindowInputKey::Named(WindowInputNamedKey::Control),
            NamedKey::Alt | NamedKey::AltGraph => WindowInputKey::Named(WindowInputNamedKey::Alt),
            NamedKey::Super | NamedKey::Meta | NamedKey::Hyper => {
                WindowInputKey::Named(WindowInputNamedKey::Super)
            }
            _ => WindowInputKey::Unidentified,
        },
        Key::Character(text) => WindowInputKey::Character(text.to_string()),
        _ => WindowInputKey::Unidentified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_binding_matches_key_and_exact_modifiers() {
        let binding = KeyboardBinding {
            key: "Space".to_string(),
            mods: "Control|Shift".to_string(),
            action: KeyboardAction::ToggleViMode,
        };
        let space = WindowInputKey::Character(" ".to_string());

        assert!(matches_keyboard_binding(
            &binding,
            WindowInputModifiers::new(true, false, true, false),
            WindowInputElementState::Pressed,
            &space,
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Space),
        ));
        assert!(!matches_keyboard_binding(
            &binding,
            WindowInputModifiers::new(true, true, true, false),
            WindowInputElementState::Pressed,
            &space,
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Space),
        ));
    }

    #[test]
    fn default_ctrl_shift_d_is_consumed_as_horizontal_split() {
        let config = GerminalConfig::default();
        let binding = config
            .keyboard
            .bindings
            .iter()
            .find(|binding| binding.action == KeyboardAction::SplitHorizontal)
            .expect("horizontal split should have a default binding");

        assert!(matches_keyboard_binding(
            binding,
            WindowInputModifiers::new(true, false, true, false),
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("D".to_string()),
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyD),
        ));
    }

    #[test]
    fn default_ctrl_shift_n_is_consumed_as_new_window() {
        let config = GerminalConfig::default();
        let binding = config
            .keyboard
            .bindings
            .iter()
            .find(|binding| binding.action == KeyboardAction::NewWindow)
            .expect("new window should have a default binding");

        assert!(matches_keyboard_binding(
            binding,
            WindowInputModifiers::new(true, false, true, false),
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("n".to_string()),
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyN),
        ));
    }

    #[test]
    fn default_host_search_uses_ctrl_shift_f() {
        let config = GerminalConfig::default();
        let binding = config
            .keyboard
            .bindings
            .iter()
            .find(|binding| binding.action == KeyboardAction::ToggleSearch)
            .expect("host search should have a default binding");

        assert!(matches_keyboard_binding(
            binding,
            WindowInputModifiers::new(true, false, true, false),
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("F".to_string()),
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyF),
        ));
    }

    #[test]
    fn default_clipboard_bindings_match_physical_keys_on_release() {
        let config = GerminalConfig::default();
        for (key, key_code, action) in [
            ("C", winit::keyboard::KeyCode::KeyC, KeyboardAction::Copy),
            ("V", winit::keyboard::KeyCode::KeyV, KeyboardAction::Paste),
        ] {
            let binding = config
                .keyboard
                .bindings
                .iter()
                .find(|binding| binding.action == action)
                .expect("clipboard action should have a default binding");
            assert_eq!(binding.key, key);
            assert!(matches_keyboard_binding(
                binding,
                WindowInputModifiers::new(true, false, true, false),
                WindowInputElementState::Released,
                &WindowInputKey::Unidentified,
                winit::keyboard::PhysicalKey::Code(key_code),
            ));
        }
    }

    #[test]
    fn default_tab_switching_supports_arrows_and_vim_aliases() {
        let config = GerminalConfig::default();
        for (key, action) in [
            ("Left", KeyboardAction::PreviousTab),
            ("H", KeyboardAction::PreviousTab),
            ("Right", KeyboardAction::NextTab),
            ("L", KeyboardAction::NextTab),
        ] {
            assert!(config.keyboard.bindings.iter().any(|binding| {
                binding.key == key && binding.mods == "Control|Shift" && binding.action == action
            }));
        }
    }

    #[test]
    fn default_tab_reordering_uses_ctrl_shift_alt_h_and_l() {
        let config = GerminalConfig::default();
        for (key, action) in [
            ("H", KeyboardAction::MoveTabLeft),
            ("L", KeyboardAction::MoveTabRight),
        ] {
            assert!(config.keyboard.bindings.iter().any(|binding| {
                binding.key == key
                    && binding.mods == "Control|Shift|Alt"
                    && binding.action == action
            }));
        }
    }

    #[test]
    fn default_directional_pane_focus_uses_ctrl_alt_arrows() {
        let config = GerminalConfig::default();
        for (key, action) in [
            ("Left", KeyboardAction::FocusPaneLeft),
            ("Right", KeyboardAction::FocusPaneRight),
            ("Up", KeyboardAction::FocusPaneUp),
            ("Down", KeyboardAction::FocusPaneDown),
        ] {
            assert!(config.keyboard.bindings.iter().any(|binding| {
                binding.key == key && binding.mods == "Control|Alt" && binding.action == action
            }));
        }
    }

    #[test]
    fn default_directional_pane_resize_uses_alt_shift_arrows() {
        let config = GerminalConfig::default();
        for (key, action) in [
            ("Left", KeyboardAction::ResizePaneLeft),
            ("Right", KeyboardAction::ResizePaneRight),
            ("Up", KeyboardAction::ResizePaneUp),
            ("Down", KeyboardAction::ResizePaneDown),
        ] {
            assert!(config.keyboard.bindings.iter().any(|binding| {
                binding.key == key && binding.mods == "Alt|Shift" && binding.action == action
            }));
        }
    }

    #[test]
    fn top_tab_bar_offsets_terminal_content_by_one_cell() {
        let layout = workspace_content_layout(
            TerminalWindowSize::new(800, 600),
            24,
            true,
            TabBarPosition::Top,
        );

        assert_eq!(layout.y_px, 24);
        assert_eq!(layout.content_size, TerminalWindowSize::new(800, 576));
    }

    #[test]
    fn bottom_tab_bar_keeps_terminal_origin_and_reserves_one_cell() {
        let layout = workspace_content_layout(
            TerminalWindowSize::new(800, 600),
            24,
            true,
            TabBarPosition::Bottom,
        );

        assert_eq!(layout.y_px, 0);
        assert_eq!(layout.content_size, TerminalWindowSize::new(800, 576));
    }

    #[test]
    fn single_tab_uses_the_entire_window() {
        let layout = workspace_content_layout(
            TerminalWindowSize::new(800, 600),
            24,
            false,
            TabBarPosition::Bottom,
        );

        assert_eq!(layout.y_px, 0);
        assert_eq!(layout.content_size, TerminalWindowSize::new(800, 600));
    }

    #[test]
    fn directional_neighbor_uses_visual_adjacency_for_nested_panes() {
        let placements = vec![
            RenderSurfacePlacement::new(RenderTargetId::new(1), 0, 0, 50, 100),
            RenderSurfacePlacement::new(RenderTargetId::new(2), 50, 0, 50, 50),
            RenderSurfacePlacement::new(RenderTargetId::new(3), 50, 50, 50, 50),
        ];

        assert_eq!(
            directional_neighbor_target(&placements, RenderTargetId::new(1), PaneDirection::Right),
            Some(RenderTargetId::new(2))
        );
        assert_eq!(
            directional_neighbor_target(&placements, RenderTargetId::new(2), PaneDirection::Down),
            Some(RenderTargetId::new(3))
        );
        assert_eq!(
            directional_neighbor_target(&placements, RenderTargetId::new(3), PaneDirection::Left),
            Some(RenderTargetId::new(1))
        );
        assert_eq!(
            directional_neighbor_target(&placements, RenderTargetId::new(2), PaneDirection::Up),
            None
        );
    }

    #[test]
    fn space_binding_uses_physical_key_when_logical_key_is_missing() {
        let binding = KeyboardBinding {
            key: "Space".to_string(),
            mods: "Control|Shift".to_string(),
            action: KeyboardAction::ToggleViMode,
        };

        assert!(matches_keyboard_binding(
            &binding,
            WindowInputModifiers::new(true, false, true, false),
            WindowInputElementState::Pressed,
            &WindowInputKey::Unidentified,
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Space),
        ));
    }

    #[test]
    fn maps_extended_terminal_named_keys() {
        let cases = [
            (NamedKey::F2, WindowInputNamedKey::F2),
            (NamedKey::F12, WindowInputNamedKey::F12),
            (NamedKey::Insert, WindowInputNamedKey::Insert),
            (NamedKey::PageUp, WindowInputNamedKey::PageUp),
            (NamedKey::PageDown, WindowInputNamedKey::PageDown),
            (NamedKey::CapsLock, WindowInputNamedKey::CapsLock),
            (NamedKey::NumLock, WindowInputNamedKey::NumLock),
            (NamedKey::Shift, WindowInputNamedKey::Shift),
            (NamedKey::Control, WindowInputNamedKey::Control),
            (NamedKey::Alt, WindowInputNamedKey::Alt),
            (NamedKey::Super, WindowInputNamedKey::Super),
        ];

        for (winit_key, port_key) in cases {
            assert_eq!(
                winit_key_to_port(Key::Named(winit_key)),
                WindowInputKey::Named(port_key)
            );
        }
    }

    #[test]
    fn maps_named_space_to_terminal_text_key() {
        assert_eq!(
            winit_key_to_port(Key::Named(NamedKey::Space)),
            WindowInputKey::Character(" ".to_string())
        );
    }

    #[test]
    fn pointer_position_selects_the_containing_pane() {
        use germinal_ports::rendering::workspace_layout::RenderSurfacePlacement;

        let placements = [
            RenderSurfacePlacement::new(RenderTargetId::new(1), 0, 0, 50, 40),
            RenderSurfacePlacement::new(RenderTargetId::new(2), 50, 0, 50, 40),
        ];

        assert_eq!(
            render_target_at_position(&placements, PhysicalPosition::new(49.9, 20.0)),
            Some(RenderTargetId::new(1)),
        );
        assert_eq!(
            render_target_at_position(&placements, PhysicalPosition::new(50.0, 20.0)),
            Some(RenderTargetId::new(2)),
        );
        assert_eq!(
            render_target_at_position(&placements, PhysicalPosition::new(-1.0, 20.0)),
            None
        );
    }

    #[test]
    fn pointer_position_is_relative_to_pane_content_origin() {
        let placement = RenderSurfacePlacement::new(RenderTargetId::new(2), 500, 20, 300, 200);

        assert_eq!(
            surface_local_pointer_position(placement, 8, 12, PhysicalPosition::new(540.5, 70.25)),
            WindowPointerPosition::new(32.5, 38.25)
        );
    }

    #[test]
    fn captured_pointer_position_clamps_to_pane_content_edges() {
        assert_eq!(
            clamp_pointer_position_to_content(WindowPointerPosition::new(-20.0, 300.0), 280, 176,),
            Some(WindowPointerPosition::new(0.0, 175.0))
        );
        assert_eq!(
            clamp_pointer_position_to_content(WindowPointerPosition::new(45.5, 80.25), 280, 176,),
            Some(WindowPointerPosition::new(45.5, 80.25))
        );
    }

    #[test]
    fn terminal_hyperlink_activation_allows_only_explicit_safe_schemes() {
        assert!(is_supported_terminal_hyperlink("https://example.com/docs"));
        assert!(is_supported_terminal_hyperlink("mailto:user@example.com"));
        assert!(is_supported_terminal_hyperlink("file:///tmp/readme.txt"));
        assert!(!is_supported_terminal_hyperlink("javascript:alert(1)"));
        assert!(!is_supported_terminal_hyperlink(
            "https://example.com\ncommand"
        ));
        assert!(!is_supported_terminal_hyperlink("example.com"));
    }
}
