use std::{cell::RefCell, collections::HashMap};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};
use germinal_gnative_protocol::gnative::{
	input::{
		GNativeInputElementState, GNativeInputEvent, GNativeInputKey, GNativeInputModifiers,
		GNativeInputNamedKey,
	},
	session::{GNativeSessionAccepted, GNativeSessionDescriptor},
};
use germinal_ports::{
	event::{
		gshell_input::{GShellInput, GShellInputEvent},
		window_input_event::{WindowInputEvent, WindowInputModifiers},
	},
	service::{
		gnative_rpc_client::{IGNativeRpcClient, IGNativeRpcClientProvider},
		gnative_service::IGNativeService,
		worker_service::IWorkerService,
	},
};

#[derive(kudi::DepInj)]
#[target(GNativeService)]
pub struct GNativeServiceState {
	sessions:  RefCell<HashMap<GShellId, GNativeSessionRuntime>>,
	modifiers: RefCell<HashMap<GShellId, WindowInputModifiers>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GNativeSessionRuntime {
	pub descriptor: GNativeSessionDescriptor,
	pub accepted:   GNativeSessionAccepted,
}

impl GNativeServiceState {
	pub fn new() -> Self {
		Self { sessions: RefCell::new(HashMap::new()), modifiers: RefCell::new(HashMap::new()) }
	}

	pub fn upsert_session(&self, runtime: GNativeSessionRuntime) {
		self.sessions.borrow_mut().insert(runtime.descriptor.gshell_id, runtime);
	}

	pub fn remove_session(&self, gshell_id: GShellId) -> Option<GNativeSessionRuntime> {
		self.modifiers.borrow_mut().remove(&gshell_id);
		self.sessions.borrow_mut().remove(&gshell_id)
	}

	pub fn session_of(&self, gshell_id: GShellId) -> Option<GNativeSessionRuntime> {
		self.sessions.borrow().get(&gshell_id).cloned()
	}

	pub fn set_modifiers(&self, gshell_id: GShellId, modifiers: WindowInputModifiers) {
		self.modifiers.borrow_mut().insert(gshell_id, modifiers);
	}

	pub fn modifiers_of(&self, gshell_id: GShellId) -> WindowInputModifiers {
		self
			.modifiers
			.borrow()
			.get(&gshell_id)
			.copied()
			.unwrap_or(WindowInputModifiers::new(false, false))
	}
}

impl Default for GNativeServiceState {
	fn default() -> Self { Self::new() }
}

impl<Deps> IGNativeService for GNativeService<Deps>
where Deps: AsRef<GNativeServiceState> + IWorkerService + IGNativeRpcClientProvider
{
	fn ensure_gshell_gnative(&self, _gshell_id: GShellId) { self.prj_ref().start_worker_pool(); }

	fn enter_gnative_session(&self, descriptor: GNativeSessionDescriptor) -> Result<(), String> {
		let accepted = self.prj_ref().gnative_rpc_client().connect_and_handshake(&descriptor)?;
		let runtime = GNativeSessionRuntime { descriptor, accepted };
		let state = <Deps as AsRef<GNativeServiceState>>::as_ref(self.prj_ref());
		state.upsert_session(runtime);
		Ok(())
	}

	fn exit_gnative_session(&self, gshell_id: GShellId) {
		let state = <Deps as AsRef<GNativeServiceState>>::as_ref(self.prj_ref());
		state.remove_session(gshell_id);
		if let Err(error) = self.prj_ref().gnative_rpc_client().close_session(gshell_id) {
			eprintln!("failed to close gnative session for {}: {error}", gshell_id.value());
		}
	}

	fn route_gnative_input(&self, input: GShellInput) {
		let state = <Deps as AsRef<GNativeServiceState>>::as_ref(self.prj_ref());
		let gshell_id = input.gshell_id;

		if let GShellInputEvent::Window(WindowInputEvent::ModifiersChanged(modifiers)) = &input.event {
			state.set_modifiers(gshell_id, *modifiers);
			return;
		}

		let Some(event) = gnative_input_event_from(input, state.modifiers_of(gshell_id)) else {
			return;
		};

		if let Err(error) = self.prj_ref().gnative_rpc_client().send_input(gshell_id, event) {
			eprintln!("failed to send gnative input for {}: {error}", gshell_id.value());
		}
	}

	fn resize_gnative_session(&self, gshell_id: GShellId, term_size: TerminalGridSize) {
		let event = GNativeInputEvent::Resize {
			columns: term_size.columns() as u32,
			rows:    term_size.rows() as u32,
		};

		if let Err(error) = self.prj_ref().gnative_rpc_client().send_input(gshell_id, event) {
			eprintln!("failed to send gnative resize for {}: {error}", gshell_id.value());
		}
	}
}

fn gnative_input_event_from(
	input: GShellInput,
	modifiers: WindowInputModifiers,
) -> Option<GNativeInputEvent> {
	match input.event {
		GShellInputEvent::Bytes(bytes) => Some(GNativeInputEvent::Bytes(bytes)),
		GShellInputEvent::Paste(text) => Some(GNativeInputEvent::Paste(text)),
		GShellInputEvent::Window(window_event) => match window_event {
			WindowInputEvent::ModifiersChanged(_) => None,
			WindowInputEvent::Key { state, logical_key, text } => Some(GNativeInputEvent::Key {
				state:       gnative_input_state_from(state),
				logical_key: gnative_input_key_from(&logical_key),
				text:        text.as_deref().map(ToOwned::to_owned),
				modifiers:   gnative_input_modifiers_from(modifiers),
			}),
			WindowInputEvent::Ime(text) => Some(GNativeInputEvent::Ime(text)),
			WindowInputEvent::Paste(text) => Some(GNativeInputEvent::Paste(text)),
		},
	}
}

fn gnative_input_state_from(
	state: germinal_ports::event::window_input_event::WindowInputElementState,
) -> GNativeInputElementState {
	match state {
		germinal_ports::event::window_input_event::WindowInputElementState::Pressed => {
			GNativeInputElementState::Pressed
		}
		germinal_ports::event::window_input_event::WindowInputElementState::Released => {
			GNativeInputElementState::Released
		}
	}
}

fn gnative_input_modifiers_from(modifiers: WindowInputModifiers) -> GNativeInputModifiers {
	GNativeInputModifiers { control: modifiers.control_key(), alt: modifiers.alt_key() }
}

fn gnative_input_key_from(
	key: &germinal_ports::event::window_input_event::WindowInputKey,
) -> GNativeInputKey {
	match key {
		germinal_ports::event::window_input_event::WindowInputKey::Named(named) => {
			GNativeInputKey::Named(match named {
				germinal_ports::event::window_input_event::WindowInputNamedKey::Enter => {
					GNativeInputNamedKey::Enter
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::Tab => {
					GNativeInputNamedKey::Tab
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::Backspace => {
					GNativeInputNamedKey::Backspace
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::Escape => {
					GNativeInputNamedKey::Escape
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::ArrowUp => {
					GNativeInputNamedKey::ArrowUp
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::ArrowDown => {
					GNativeInputNamedKey::ArrowDown
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::ArrowRight => {
					GNativeInputNamedKey::ArrowRight
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::ArrowLeft => {
					GNativeInputNamedKey::ArrowLeft
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::Home => {
					GNativeInputNamedKey::Home
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::End => {
					GNativeInputNamedKey::End
				}
				germinal_ports::event::window_input_event::WindowInputNamedKey::Delete => {
					GNativeInputNamedKey::Delete
				}
			})
		}
		germinal_ports::event::window_input_event::WindowInputKey::Character(text) => {
			GNativeInputKey::Character(text.to_string())
		}
		germinal_ports::event::window_input_event::WindowInputKey::Unidentified => {
			GNativeInputKey::Unidentified
		}
	}
}

#[cfg(test)]
mod tests {
	use germinal_domain::gshell::vo::gshell_id::GShellId;
	use germinal_gnative_protocol::gnative::{
		input::{GNativeInputElementState, GNativeInputEvent, GNativeInputKey, GNativeInputModifiers},
		session::{GNativeSessionAccepted, GNativeSessionDescriptor},
	};
	use germinal_ports::event::{
		gshell_input::{GShellInput, GShellInputEvent},
		window_input_event::{
			WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
		},
	};

	use super::{GNativeServiceState, GNativeSessionRuntime, gnative_input_event_from};

	#[test]
	fn state_stores_session_runtime_by_gshell_id() {
		let state = GNativeServiceState::new();
		let runtime = GNativeSessionRuntime {
			descriptor: GNativeSessionDescriptor {
				gshell_id:        GShellId::new(9),
				endpoint:         "unix:///tmp/test.sock".to_string(),
				token:            "secret".to_string(),
				protocol_version: 1,
			},
			accepted:   GNativeSessionAccepted {
				gshell_id:        GShellId::new(9),
				protocol_version: 1,
			},
		};

		state.upsert_session(runtime.clone());

		assert_eq!(state.session_of(GShellId::new(9)), Some(runtime));
	}

	#[test]
	fn remove_session_clears_runtime_and_modifiers() {
		let state = GNativeServiceState::new();
		let gshell_id = GShellId::new(10);
		state.upsert_session(GNativeSessionRuntime {
			descriptor: GNativeSessionDescriptor {
				gshell_id,
				endpoint: "unix:///tmp/test.sock".to_string(),
				token: "secret".to_string(),
				protocol_version: 1,
			},
			accepted:   GNativeSessionAccepted { gshell_id, protocol_version: 1 },
		});
		state.set_modifiers(gshell_id, WindowInputModifiers::new(true, true));

		let removed = state.remove_session(gshell_id);

		assert!(removed.is_some());
		assert_eq!(state.session_of(gshell_id), None);
		assert_eq!(state.modifiers_of(gshell_id), WindowInputModifiers::new(false, false));
	}

	#[test]
	fn maps_window_key_input_to_gnative_key_event() {
		let input = GShellInput {
			gshell_id: GShellId::new(1),
			event:     GShellInputEvent::Window(WindowInputEvent::Key {
				state:       WindowInputElementState::Pressed,
				logical_key: WindowInputKey::Character("a".into()),
				text:        Some("a".into()),
			}),
		};

		let mapped = gnative_input_event_from(input, WindowInputModifiers::new(true, false));
		assert_eq!(
			mapped,
			Some(GNativeInputEvent::Key {
				state:       GNativeInputElementState::Pressed,
				logical_key: GNativeInputKey::Character("a".to_string()),
				text:        Some("a".to_string()),
				modifiers:   GNativeInputModifiers { control: true, alt: false },
			})
		);
	}
}
