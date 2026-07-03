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
use germinal_domain::workspace::entity::workspace::Workspace;
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
	pty_host::window_size::TerminalWindowSize,
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
	event::{ElementState, Ime, WindowEvent},
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
	config:                   GerminalConfig,
}

impl App {
	pub fn new(
		runtime_event_proxy: EventLoopProxy<RuntimeEvent>,
		config: GerminalConfig,
	) -> AppResult<Self> {
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
			workspace_service_state: WorkspaceServiceState::new(),
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
		let focused_gshell = self.focused_gshell();

		if let Err(error) = self.ensure_window_runtime(event_loop) {
			error!(error = %error, "failed to initialize Germinal window runtime");
			self.exit_and_persist(event_loop);
			return;
		}

		let size_info = self.current_terminal_size_info();
		let pty_size = size_info.pty_size();
		let term_size = size_info.grid_size();
		let surface_snapshot_tx = self.surface_snapshot_sender();
		let snapshot_wake_pending = self.snapshot_wake_pending();

		self.ensure_gshell(
			focused_gshell,
			pty_size,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		);
		self.set_focused_render_target(RenderTargetId::new(focused_gshell.value()));
		self.prepare_render_backend();
	}

	fn user_event(&mut self, event_loop: &ActiveEventLoop, event: RuntimeEvent) {
		match event {
			RuntimeEvent::GShell(GShellRuntimeEvent::EnterGNative { gshell_id }) => {
				if let Err(error) = self.enter_gnative_session(gshell_id) {
					error!(gshell_id = gshell_id.value(), error = %error, "failed to enter gnative session");
				} else {
					self.enter_gnative_mode(gshell_id);
					let size_info = self.current_terminal_size_info();
					self.resize_gshell(gshell_id, size_info.pty_size(), size_info.grid_size());
				}
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
				self.resize_gshell(self.focused_gshell(), size_info.pty_size(), size_info.grid_size());
			}
			WindowEvent::Focused(focused) => {
				self.set_window_focused(focused);
			}
			WindowEvent::RedrawRequested => {
				self.drain_media_bridge_frames();
				self.present_workspace();
			}
			WindowEvent::ModifiersChanged(modifiers) => {
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
			WindowEvent::KeyboardInput { event, .. } => {
				let winit::event::KeyEvent { state, logical_key, physical_key, text, .. } = event;
				let logical_key = winit_key_to_port(logical_key);
				let state = winit_element_state_to_port(state);
				let text = text.map(|text| text.to_string());
				self.paste_controller.observe_key_event(state, physical_key);

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
