mod boilerplate;

use germinal_application::service::{
	gshell_service::GShellServiceState, layout_service::LayoutServiceState,
	render_service::RenderServiceState, worker_service::WorkerServiceState,
	workspace_service::WorkspaceServiceState,
};
use germinal_domain::workspace::entity::workspace::Workspace;
use germinal_infra::{
	pty::PlatformPtyBackend,
	pty_host::worker::PlatformTerminalWorkerBackend,
	rendering::pty_surface::window_runtime::{
		WgpuTerminalWindowRuntime, WgpuTerminalWindowRuntimeFactory,
	},
	repositories::sqlite_repository::SqliteRepository,
};
use germinal_ports::{
	event::{
		gshell_input::{GShellInput, GShellInputEvent},
		runtime_event::{GShellRuntimeEvent, RuntimeEvent},
		window_input_event::{
			WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
			WindowInputNamedKey,
		},
	},
	pty_host::window_size::TerminalWindowSize,
	rendering::render_target_id::RenderTargetId,
	service::{
		gshell_service::IGShellService, render_service::IRenderService,
		workspace_service::IWorkspaceService,
	},
};
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
	workspace_service_state:          WorkspaceServiceState,
	gshell_service_state:             GShellServiceState,
	worker_service_state:             WorkerServiceState,
	render_service_state:             RenderServiceState,
	layout_service_state:             LayoutServiceState,
	workspace_persistence_repository: SqliteRepository<Workspace>,
	runtime_event_dispatcher:         AppRuntimeEventDispatcher,
	pty_backend:                      PlatformPtyBackend,
	terminal_worker_backend:          PlatformTerminalWorkerBackend,
	render_runtime_factory:           WgpuTerminalWindowRuntimeFactory,
	render_runtime:                   Option<WgpuTerminalWindowRuntime>,
	render_window_id:                 Option<WindowId>,
}

impl App {
	pub fn new(runtime_event_proxy: EventLoopProxy<RuntimeEvent>) -> Result<Self, String> {
		let app = Self {
			workspace_service_state:          WorkspaceServiceState::new(),
			gshell_service_state:             GShellServiceState::new(),
			worker_service_state:             WorkerServiceState::new(),
			render_service_state:             RenderServiceState::new(),
			layout_service_state:             LayoutServiceState::default(),
			workspace_persistence_repository: SqliteRepository::new(
				"germinal-workspace.sqlite3",
				"workspace",
			)?,
			runtime_event_dispatcher:         AppRuntimeEventDispatcher { proxy: runtime_event_proxy },
			pty_backend:                      PlatformPtyBackend::new(),
			terminal_worker_backend:          PlatformTerminalWorkerBackend::new(),
			render_runtime_factory:           WgpuTerminalWindowRuntimeFactory::new(),
			render_runtime:                   None,
			render_window_id:                 None,
		};

		app.restore_workspace()?;

		Ok(app)
	}

	pub fn run(&mut self, event_loop: EventLoop<RuntimeEvent>) -> Result<(), String> {
		event_loop.run_app(self).map_err(|error| error.to_string())
	}

	fn ensure_window_runtime(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
		if self.render_runtime.is_some() {
			return Ok(());
		}

		let window = std::sync::Arc::new(
			event_loop
				.create_window(
					winit::window::Window::default_attributes()
						.with_title("Germinal")
						.with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0)),
				)
				.map_err(|error| error.to_string())?,
		);
		let window_id = window.id();
		window.set_ime_allowed(true);

		let runtime = self.render_runtime_factory.create_window_runtime(window)?;
		self.render_runtime = Some(runtime);
		self.render_window_id = Some(window_id);
		Ok(())
	}

	fn current_window_id(&self) -> WindowId {
		self.render_window_id.expect("window runtime must be initialized before use")
	}

	fn exit_and_persist(&self, event_loop: &ActiveEventLoop) {
		if let Err(error) = self.persist_workspace() {
			eprintln!("failed to persist workspace: {error}");
		}

		event_loop.exit();
	}
}

impl ApplicationHandler<RuntimeEvent> for App {
	fn resumed(&mut self, event_loop: &ActiveEventLoop) {
		let focused_gshell = self.focused_gshell();

		if self.ensure_window_runtime(event_loop).is_err() {
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
		let current_window_id = self.current_window_id();

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
				self.present_workspace();
			}
			WindowEvent::ModifiersChanged(modifiers) => {
				self.route_input_to_gshell(GShellInput {
					gshell_id: self.focused_gshell(),
					event:     GShellInputEvent::Window(WindowInputEvent::ModifiersChanged(
						WindowInputModifiers::new(modifiers.state().control_key(), modifiers.state().alt_key()),
					)),
				});
			}
			WindowEvent::KeyboardInput { event, .. } => {
				let winit::event::KeyEvent { state, logical_key, text, .. } = event;

				self.route_input_to_gshell(GShellInput {
					gshell_id: self.focused_gshell(),
					event:     GShellInputEvent::Window(WindowInputEvent::Key {
						state: winit_element_state_to_port(state),
						logical_key: winit_key_to_port(logical_key),
						text,
					}),
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
		Key::Character(text) => WindowInputKey::Character(text),
		_ => WindowInputKey::Unidentified,
	}
}
