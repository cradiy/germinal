use std::cell::RefCell;

mod boilerplate;
mod config;
mod error;
mod logging;
mod paste;

pub use config::{GerminalConfig, load_or_create_config};
pub use error::{AppError, AppResult};
use germinal_application::service::{
	gshell_service::GShellServiceState, layout_service::LayoutServiceState,
	render_service::RenderServiceState, worker_service::WorkerServiceState,
	workspace_service::WorkspaceServiceState,
};
use germinal_domain::{
	gshell::vo::gshell_id::GShellId,
	workspace::entity::workspace::Workspace,
};
use germinal_infra::{
	gnative::gst_video_player_bridge::GstVideoPlayerBridge,
	pty::PlatformPtyBackend,
	pty_host::worker::PlatformTerminalWorkerBackend,
	rendering::pty_surface::window_runtime::{
		WgpuTerminalWindowRuntime, WgpuTerminalWindowRuntimeFactory,
	},
};
use germinal_ports::{
	event::{
		gshell_input::{GShellInput, GShellInputEvent},
		runtime_event::{GShellRuntimeEvent, RuntimeEvent},
		runtime_event_dispatcher::IRuntimeEventDispatcher,
		window_input_event::{
			WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
			WindowInputNamedKey,
		},
	},
	pty_host::{size_info::TerminalSizeInfo, window_size::TerminalWindowSize},
	rendering::render_target_id::RenderTargetId,
	service::{
		gnative_service::IGNativeService, gshell_service::IGShellService,
		render_service::IRenderService, workspace_service::IWorkspaceService,
	},
};
pub use logging::init_logging;
use paste::{HostPasteAction, HostPasteController, HostPasteModifiers};
use tracing::{debug, error, warn};
use winit::{
	application::ApplicationHandler,
	dpi::PhysicalPosition,
	event::{ElementState, Ime, MouseButton, WindowEvent},
	event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
	keyboard::{Key, NamedKey},
	window::WindowId,
};

#[derive(Clone)]
pub struct AppRuntimeEventDispatcher {
	proxy: EventLoopProxy<RuntimeEvent>,
}

pub struct App {
	workspace_service_state:  WorkspaceServiceState,
	gshell_service_state:     GShellServiceState,
	worker_service_state:     WorkerServiceState,
	render_service_state:     RenderServiceState,
	layout_service_state:     LayoutServiceState,
	workspace_repository:     RefCell<Option<Workspace>>,
	runtime_event_dispatcher: AppRuntimeEventDispatcher,
	pty_backend:              PlatformPtyBackend,
	gnative_tunnel: germinal_infra::gnative::tunnel::GNativeTunnel<AppRuntimeEventDispatcher>,
	media_bridge:             std::sync::Arc<GstVideoPlayerBridge>,
	terminal_worker_backend:  PlatformTerminalWorkerBackend<AppRuntimeEventDispatcher>,
	render_runtime_factory:   WgpuTerminalWindowRuntimeFactory,
	render_runtime:           Option<WgpuTerminalWindowRuntime>,
	render_window_id:         Option<WindowId>,
	paste_controller:         HostPasteController,
	window_input_modifiers:   WindowInputModifiers,
	cursor_position:          Option<PhysicalPosition<f64>>,
	pane_navigation_enabled:  bool,
	config:                   GerminalConfig,
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
		let pane_navigation_enabled = workspace.active_tab().pane_count() > 1;
		let runtime_event_dispatcher = AppRuntimeEventDispatcher { proxy: runtime_event_proxy };
		let media_dispatcher = {
			let runtime_event_dispatcher = runtime_event_dispatcher.clone();
			std::sync::Arc::new(move |event: RuntimeEvent| runtime_event_dispatcher.dispatch(event))
		};
		let media_bridge = std::sync::Arc::new(
			GstVideoPlayerBridge::new(media_dispatcher).map_err(AppError::MediaBridge)?,
		);
		let terminal_profile = config.terminal_profile();
		let window_title = config.window.title.clone();

		let app = Self {
			workspace_service_state: WorkspaceServiceState::with_workspace(workspace),
			gshell_service_state: GShellServiceState::new(),
			worker_service_state: WorkerServiceState::new(),
			render_service_state: RenderServiceState::new(),
			layout_service_state: LayoutServiceState::new(terminal_profile),
			workspace_repository: RefCell::new(None),
			runtime_event_dispatcher: runtime_event_dispatcher.clone(),
			pty_backend: PlatformPtyBackend::new(),
			gnative_tunnel: germinal_infra::gnative::tunnel::GNativeTunnel::new()
				.map_err(AppError::CreateGNativeTunnel)?,
			media_bridge: std::sync::Arc::clone(&media_bridge),
			terminal_worker_backend: PlatformTerminalWorkerBackend::new(runtime_event_dispatcher),
			render_runtime_factory: WgpuTerminalWindowRuntimeFactory::new(terminal_profile, window_title),
			render_runtime: None,
			render_window_id: None,
			paste_controller: HostPasteController::default(),
			window_input_modifiers: WindowInputModifiers::new(false, false),
			cursor_position: None,
			pane_navigation_enabled,
			config,
		};

		app.gnative_tunnel.configure(
			app.runtime_event_dispatcher.clone(),
			app.snapshot_wake_pending(),
			app.surface_snapshot_sender(),
		);
		app.gnative_tunnel.configure_media_bridge(media_bridge);

		app.restore_workspace().map_err(AppError::RestoreWorkspace)?;

		Ok(app)
	}

	pub fn run(&mut self, event_loop: EventLoop<RuntimeEvent>) -> AppResult<()> {
		event_loop.run_app(self).map_err(AppError::RunEventLoop)
	}

	fn ensure_window_runtime(&mut self, event_loop: &ActiveEventLoop) -> AppResult<()> {
		if self.render_runtime.is_some() {
			return Ok(());
		}

		let window = std::sync::Arc::new(
			event_loop
				.create_window(
					winit::window::Window::default_attributes()
						.with_title(self.config.window.title.as_str())
						.with_inner_size(winit::dpi::LogicalSize::new(
							f64::from(self.config.window.width_px),
							f64::from(self.config.window.height_px),
						)),
				)
				.map_err(AppError::CreateWindow)?,
		);
		let window_id = window.id();
		window.set_ime_allowed(true);

		let runtime = self
			.render_runtime_factory
			.create_window_runtime(window)
			.map_err(AppError::CreateWindowRuntime)?;
		self.render_runtime = Some(runtime);
		self.render_window_id = Some(window_id);
		Ok(())
	}

	fn current_window_id(&self) -> Option<WindowId> { self.render_window_id }

	fn ensure_workspace_gshells(&mut self) {
		let window_size = self.current_terminal_size_info().window_size();
		let placements = self.workspace_render_layout(window_size);
		self.set_workspace_render_layout(placements.clone());

		let surface_snapshot_tx = self.surface_snapshot_sender();
		let snapshot_wake_pending = self.snapshot_wake_pending();
		for placement in placements {
			let size_info = self.terminal_size_info_for_surface(placement);
			self.ensure_gshell(
				GShellId::new(placement.target_id.value()),
				size_info.pty_size(),
				size_info.grid_size(),
				surface_snapshot_tx.clone(),
				std::sync::Arc::clone(&snapshot_wake_pending),
			);
		}
	}

	fn resize_workspace_gshells(&mut self, window_size: TerminalWindowSize) {
		let placements = self.workspace_render_layout(window_size);
		self.set_workspace_render_layout(placements.clone());

		for placement in placements {
			let size_info = self.terminal_size_info_for_surface(placement);
			self.resize_gshell(
				GShellId::new(placement.target_id.value()),
				size_info.pty_size(),
				size_info.grid_size(),
			);
		}
	}

	fn current_gshell_size_info(&self, gshell_id: GShellId) -> Option<TerminalSizeInfo> {
		let window_size = self.current_terminal_size_info().window_size();
		self
			.workspace_render_layout(window_size)
			.into_iter()
			.find(|placement| placement.target_id.value() == gshell_id.value())
			.map(|placement| self.terminal_size_info_for_surface(placement))
	}

	fn try_handle_paste_shortcut(
		&mut self,
		state: WindowInputElementState,
		logical_key: &WindowInputKey,
		physical_key: winit::keyboard::PhysicalKey,
	) -> bool {
		match self.paste_controller.handle_shortcut(
			self.focused_gshell(),
			state,
			logical_key,
			physical_key,
		) {
			Ok(HostPasteAction::NotHandled) => false,
			Ok(HostPasteAction::Handled) => true,
			Ok(HostPasteAction::HandledEmpty) => {
				debug!("paste shortcut matched but clipboard text was empty");
				true
			}
			Ok(HostPasteAction::Dispatch(input)) => {
				self.route_input_to_gshell(input);
				true
			}
			Err(error) => {
				warn!(error = %error, "failed to paste from clipboard");
				true
			}
		}
	}

	fn try_handle_pane_navigation(
		&mut self,
		state: WindowInputElementState,
		logical_key: &WindowInputKey,
	) -> bool {
		if !matches_pane_cycle_shortcut(
			self.pane_navigation_enabled,
			self.window_input_modifiers,
			state,
			logical_key,
		) {
			return false;
		}

		if state == WindowInputElementState::Pressed {
			let focused_gshell = self.focus_next_gshell();
			self.set_focused_render_target(RenderTargetId::new(focused_gshell.value()));
		}

		true
	}

	fn try_focus_pane_at_cursor(&mut self) -> bool {
		let Some(cursor_position) = self.cursor_position else {
			return false;
		};
		let window_size = self.current_terminal_size_info().window_size();
		let placements = self.workspace_render_layout(window_size);
		let Some(target_id) = render_target_at_position(&placements, cursor_position) else {
			debug!(x = cursor_position.x, y = cursor_position.y, "pane focus click missed workspace");
			return false;
		};
		let gshell_id = GShellId::new(target_id.value());

		if !self.focus_gshell(gshell_id) {
			debug!(target_id = target_id.value(), "pane focus click resolved to an unknown target");
			return false;
		}

		debug!(x = cursor_position.x, y = cursor_position.y, target_id = target_id.value(), "focused pane from pointer");
		self.set_focused_render_target(target_id);
		true
	}

	fn drain_media_bridge_frames(&self) {
		let Some(render_runtime) = self.render_runtime.as_ref() else {
			return;
		};

		for pending in self.media_bridge.drain_pending_video_surface_frames() {
			let import_result =
				render_runtime.import_video_surface_dma_buf_frame(&pending.surface_id, &pending.frame);
			if let Err(error) = import_result {
				warn!(surface_id = %pending.surface_id, error = %error, "failed to import video surface frame");
			}
		}
	}

	fn exit_and_persist(&self, event_loop: &ActiveEventLoop) {
		if let Err(error) = self.persist_workspace() {
			error!(error = %error, "failed to persist workspace");
		}

		event_loop.exit();
	}
}

impl ApplicationHandler<RuntimeEvent> for App {
	fn resumed(&mut self, event_loop: &ActiveEventLoop) {
		if let Err(error) = self.ensure_window_runtime(event_loop) {
			error!(error = %error, "failed to initialize Germinal window runtime");
			self.exit_and_persist(event_loop);
			return;
		}

		self.ensure_workspace_gshells();
		let focused_gshell = self.focused_gshell();
		self.set_focused_render_target(RenderTargetId::new(focused_gshell.value()));
		self.prepare_render_backend();
	}

	fn user_event(&mut self, event_loop: &ActiveEventLoop, event: RuntimeEvent) {
		match event {
			RuntimeEvent::GShell(GShellRuntimeEvent::EnterGNative { gshell_id }) => {
				self.begin_gnative_mode(gshell_id);
				if let Err(error) = self.begin_gnative_session(gshell_id) {
					self.exit_gnative_mode(gshell_id);
					error!(gshell_id = gshell_id.value(), error = %error, "failed to enter gnative session");
				}
			}
			RuntimeEvent::GShell(GShellRuntimeEvent::GNativeConnected { accepted }) => {
				let gshell_id = accepted.gshell_id;
				self.activate_gnative_session(accepted);
				self.enter_gnative_mode(gshell_id);
				if let Some(size_info) = self.current_gshell_size_info(gshell_id) {
					self.resize_gshell(gshell_id, size_info.pty_size(), size_info.grid_size());
				}
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
				self.exit_gnative_session(gshell_id);
				self.exit_gnative_mode(gshell_id);
				self.consume_latest_terminal_snapshot();
				self.request_redraw();
			}
			RuntimeEvent::GShell(GShellRuntimeEvent::FrameReady { .. }) => {
				self.consume_latest_terminal_snapshot();
				self.request_redraw();
			}
			RuntimeEvent::GShell(GShellRuntimeEvent::Closed { .. }) => {
				self.exit_and_persist(event_loop);
			}
			RuntimeEvent::App(_) => {
				self.exit_and_persist(event_loop);
			}
			RuntimeEvent::Workspace(_) => {
				self.drain_media_bridge_frames();
				self.request_redraw();
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
			WindowEvent::Resized(size) => {
				let size_info = self
					.resize_window_size_info(TerminalWindowSize::new(size.width.max(1), size.height.max(1)));
				self.resize_workspace_gshells(size_info.window_size());
			}
			WindowEvent::Focused(focused) => {
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
				);
				self.paste_controller.set_modifiers(HostPasteModifiers {
					control: modifiers.state().control_key(),
					shift:   modifiers.state().shift_key(),
				});
				self.route_input_to_gshell(GShellInput {
					gshell_id: self.focused_gshell(),
					event:     GShellInputEvent::Window(WindowInputEvent::ModifiersChanged(
						WindowInputModifiers::new(modifiers.state().control_key(), modifiers.state().alt_key()),
					)),
				});
			}
			WindowEvent::CursorMoved { position, .. } => {
				self.cursor_position = Some(position);
			}
			WindowEvent::CursorLeft { .. } => {
				self.cursor_position = None;
			}
			WindowEvent::MouseInput {
				state: ElementState::Pressed,
				button: MouseButton::Left,
				..
			} => {
				self.try_focus_pane_at_cursor();
			}
			WindowEvent::KeyboardInput { event, .. } => {
				let winit::event::KeyEvent { state, logical_key, physical_key, text, .. } = event;
				let logical_key = winit_key_to_port(logical_key);
				let state = winit_element_state_to_port(state);
				let text = text.map(|text| text.to_string());
				self.paste_controller.observe_key_event(state, physical_key);

				if self.try_handle_pane_navigation(state, &logical_key) {
					return;
				}

				if self.try_handle_paste_shortcut(state, &logical_key, physical_key) {
					return;
				}

				self.route_input_to_gshell(GShellInput {
					gshell_id: self.focused_gshell(),
					event:     GShellInputEvent::Window(WindowInputEvent::Key { state, logical_key, text }),
				});
			}
			WindowEvent::Ime(Ime::Commit(text)) => {
				self.route_input_to_gshell(GShellInput {
					gshell_id: self.focused_gshell(),
					event:     GShellInputEvent::Window(WindowInputEvent::Ime(text)),
				});
			}
			_ => {}
		}
	}

	fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) { self.flush_redraw_request(); }
}

fn matches_pane_cycle_shortcut(
	enabled: bool,
	modifiers: WindowInputModifiers,
	state: WindowInputElementState,
	logical_key: &WindowInputKey,
) -> bool {
	enabled
		&& modifiers.control_key()
		&& matches!(state, WindowInputElementState::Pressed | WindowInputElementState::Released)
		&& matches!(logical_key, WindowInputKey::Named(WindowInputNamedKey::Tab))
}

fn render_target_at_position(
	placements: &[germinal_ports::rendering::workspace_layout::RenderSurfacePlacement],
	position: PhysicalPosition<f64>,
) -> Option<RenderTargetId> {
	if !position.x.is_finite() || !position.y.is_finite() || position.x < 0.0 || position.y < 0.0 {
		return None;
	}

	placements
		.iter()
		.find(|placement| {
			position.x >= f64::from(placement.x_px)
				&& position.x < f64::from(placement.x_px.saturating_add(placement.width_px))
				&& position.y >= f64::from(placement.y_px)
				&& position.y < f64::from(placement.y_px.saturating_add(placement.height_px))
		})
		.map(|placement| placement.target_id)
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
			NamedKey::F1 => WindowInputKey::Named(WindowInputNamedKey::F1),
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
			NamedKey::Delete => WindowInputKey::Named(WindowInputNamedKey::Delete),
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
	fn ctrl_tab_cycles_panes_only_when_navigation_is_enabled() {
		let modifiers = WindowInputModifiers::new(true, false);
		let tab = WindowInputKey::Named(WindowInputNamedKey::Tab);

		assert!(matches_pane_cycle_shortcut(
			true,
			modifiers,
			WindowInputElementState::Pressed,
			&tab,
		));
		assert!(!matches_pane_cycle_shortcut(
			false,
			modifiers,
			WindowInputElementState::Pressed,
			&tab,
		));
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
		assert_eq!(render_target_at_position(&placements, PhysicalPosition::new(-1.0, 20.0)), None);
	}
}
