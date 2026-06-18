mod boilerplate;

use germinal_application::service::{
	gshell_service::GShellServiceState, layout_service::LayoutServiceState,
	render_service::RenderServiceState, worker_service::WorkerServiceState,
	workspace_service::WorkspaceServiceState,
};
use germinal_domain::{
	pty_host::window_size::TerminalWindowSize, rendering::render_target_id::RenderTargetId,
};
use germinal_ports::{
	event::{
		gshell_input::{GShellInput, GShellInputEvent},
		runtime_event::{PaneRuntimeEvent, RuntimeEvent},
		window_input_event::{
			WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
			WindowInputNamedKey,
		},
	},
	service::{gshell_service::IGShellService, render_service::IRenderService},
};
use winit::{
	application::ApplicationHandler,
	event::{ElementState, Ime, WindowEvent},
	event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
	keyboard::{Key, NamedKey},
	window::WindowId,
};

pub struct App {
	workspace_service_state: WorkspaceServiceState,
	gshell_service_state:    GShellServiceState,
	worker_service_state:    WorkerServiceState,
	render_service_state:    RenderServiceState,
	layout_service_state:    LayoutServiceState,
}

impl App {
	pub fn new(runtime_event_proxy: EventLoopProxy<RuntimeEvent>) -> Self {
		Self {
			workspace_service_state: WorkspaceServiceState::new(runtime_event_proxy),
			gshell_service_state:    GShellServiceState::new(),
			worker_service_state:    WorkerServiceState::new(),
			render_service_state:    RenderServiceState::new(),
			layout_service_state:    LayoutServiceState::default(),
		}
	}

	pub fn run(&mut self, event_loop: EventLoop<RuntimeEvent>) -> Result<(), String> {
		event_loop.run_app(self).map_err(|error| error.to_string())
	}
}

impl ApplicationHandler<RuntimeEvent> for App {
	fn resumed(&mut self, event_loop: &ActiveEventLoop) {
		let (proxy, focused_pane) = {
			let state = &self.workspace_service_state;
			(state.runtime_event_proxy(), state.focused_pane())
		};

		if self.render_service_state.ensure_window_runtime(event_loop).is_err() {
			event_loop.exit();
			return;
		}

		let size_info = self.current_terminal_size_info();
		let pty_size = size_info.pty_size();
		let term_size = size_info.grid_size();
		let surface_snapshot_tx = self.surface_snapshot_sender();
		let snapshot_wake_pending = self.snapshot_wake_pending();

		self.ensure_pane_gshell(
			focused_pane,
			proxy,
			pty_size,
			term_size,
			surface_snapshot_tx,
			snapshot_wake_pending,
		);
		self.set_focused_render_target(RenderTargetId::new(focused_pane.value()));
		self.prepare_render_backend();
	}

	fn user_event(&mut self, event_loop: &ActiveEventLoop, event: RuntimeEvent) {
		match event {
			RuntimeEvent::Pane(PaneRuntimeEvent::FrameReady { .. }) => {
				self.consume_latest_terminal_snapshot();
				self.request_redraw();
			}
			RuntimeEvent::Pane(PaneRuntimeEvent::Closed { .. }) => {
				event_loop.exit();
			}
			RuntimeEvent::App(_) => {
				event_loop.exit();
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
		let current_window_id = self.render_service_state.current_window_id();

		if window_id != current_window_id {
			return;
		}

		match event {
			WindowEvent::CloseRequested => {
				event_loop.exit();
			}
			WindowEvent::Resized(size) => {
				let size_info = self
					.resize_window_size_info(TerminalWindowSize::new(size.width.max(1), size.height.max(1)));
				self.resize_pane_gshell(
					self.workspace_service_state.focused_pane(),
					size_info.pty_size(),
					size_info.grid_size(),
				);
			}
			WindowEvent::Focused(focused) => {
				self.set_window_focused(focused);
			}
			WindowEvent::RedrawRequested => {
				self.present_workspace();
			}
			WindowEvent::ModifiersChanged(modifiers) => {
				self.route_input_to_gshell(GShellInput {
					pane_id: self.workspace_service_state.focused_pane(),
					event:   GShellInputEvent::Window(WindowInputEvent::ModifiersChanged(
						WindowInputModifiers::new(modifiers.state().control_key(), modifiers.state().alt_key()),
					)),
				});
			}
			WindowEvent::KeyboardInput { event, .. } => {
				self.route_input_to_gshell(GShellInput {
					pane_id: self.workspace_service_state.focused_pane(),
					event:   GShellInputEvent::Window(WindowInputEvent::Key {
						state:       winit_element_state_to_port(event.state),
						logical_key: winit_key_to_port(&event.logical_key),
						text:        event.text.as_ref().map(|text| text.to_string()),
					}),
				});
			}
			WindowEvent::Ime(Ime::Commit(text)) => {
				self.route_input_to_gshell(GShellInput {
					pane_id: self.workspace_service_state.focused_pane(),
					event:   GShellInputEvent::Window(WindowInputEvent::Ime(text.to_string())),
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

fn winit_key_to_port(key: &Key) -> WindowInputKey {
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
		Key::Character(text) => WindowInputKey::Character(text.to_string()),
		_ => WindowInputKey::Unidentified,
	}
}
