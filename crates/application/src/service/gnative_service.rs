use std::{cell::RefCell, collections::HashMap};

use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::{
    input::{
        GNativeInputElementState, GNativeInputEvent, GNativeInputKey, GNativeInputModifiers,
        GNativeInputNamedKey, GNativePointerButton, GNativePointerPosition, GNativeScrollDelta,
    },
    session::GNativeSessionAccepted,
};
use germinal_ports::{
    event::{
        gshell_input::{GShellInput, GShellInputEvent},
        window_input_event::{WindowInputEvent, WindowInputModifiers},
    },
    pty_host::size_info::TerminalSizeInfo,
    service::{
        gnative_service::{GNativeServiceError, IGNativeService},
        gnative_tunnel::{IGNativeTunnel, IGNativeTunnelProvider},
        worker_service::IWorkerService,
    },
};
use tracing::warn;

#[derive(kudi::DepInj)]
#[target(GNativeService)]
pub struct GNativeServiceState {
    sessions: RefCell<HashMap<GShellId, GNativeSessionRuntime>>,
    modifiers: RefCell<HashMap<GShellId, WindowInputModifiers>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GNativeSessionRuntime {
    pub accepted: GNativeSessionAccepted,
}

impl GNativeServiceState {
    pub fn new() -> Self {
        Self {
            sessions: RefCell::new(HashMap::new()),
            modifiers: RefCell::new(HashMap::new()),
        }
    }

    pub fn upsert_session(&self, runtime: GNativeSessionRuntime) {
        self.sessions
            .borrow_mut()
            .insert(runtime.accepted.gshell_id, runtime);
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
        self.modifiers
            .borrow()
            .get(&gshell_id)
            .copied()
            .unwrap_or(WindowInputModifiers::new(false, false, false, false))
    }
}

impl Default for GNativeServiceState {
    fn default() -> Self {
        Self::new()
    }
}

impl<Deps> IGNativeService for GNativeService<Deps>
where
    Deps: AsRef<GNativeServiceState> + IWorkerService + IGNativeTunnelProvider,
{
    fn ensure_gshell_gnative(&self, _gshell_id: GShellId) {
        self.prj_ref().start_worker_pool();
    }

    fn begin_gnative_session(&self, gshell_id: GShellId) -> Result<(), GNativeServiceError> {
        self.prj_ref()
            .gnative_tunnel()
            .begin_accept_session(gshell_id)
            .map_err(|source| GNativeServiceError::EnterSession {
                gshell_id: gshell_id.value(),
                source,
            })
    }

    fn activate_gnative_session(&self, accepted: GNativeSessionAccepted) {
        let runtime = GNativeSessionRuntime { accepted };
        let state = <Deps as AsRef<GNativeServiceState>>::as_ref(self.prj_ref());
        state.upsert_session(runtime);
    }

    fn fail_gnative_session(&self, gshell_id: GShellId) {
        let state = <Deps as AsRef<GNativeServiceState>>::as_ref(self.prj_ref());
        state.remove_session(gshell_id);
    }

    fn exit_gnative_session(&self, gshell_id: GShellId) {
        let state = <Deps as AsRef<GNativeServiceState>>::as_ref(self.prj_ref());
        state.remove_session(gshell_id);
        if let Err(error) = self.prj_ref().gnative_tunnel().close_session(gshell_id) {
            warn!(gshell_id = gshell_id.value(), error = %error, "failed to close gnative session");
        }
    }

    fn route_gnative_input(&self, input: GShellInput) {
        let state = <Deps as AsRef<GNativeServiceState>>::as_ref(self.prj_ref());
        let gshell_id = input.gshell_id;

        if let GShellInputEvent::Window(WindowInputEvent::ModifiersChanged(modifiers)) =
            &input.event
        {
            state.set_modifiers(gshell_id, *modifiers);
        }

        let Some(event) = gnative_input_event_from(input, state.modifiers_of(gshell_id)) else {
            return;
        };

        if let Err(error) = self.prj_ref().gnative_tunnel().send_input(gshell_id, event) {
            warn!(gshell_id = gshell_id.value(), error = %error, "failed to send gnative input");
        }
    }

    fn resize_gnative_session(&self, gshell_id: GShellId, size_info: TerminalSizeInfo) {
        let event = gnative_resize_event(size_info);

        if let Err(error) = self.prj_ref().gnative_tunnel().send_input(gshell_id, event) {
            warn!(gshell_id = gshell_id.value(), error = %error, "failed to send gnative resize");
        }
    }
}

fn gnative_resize_event(size_info: TerminalSizeInfo) -> GNativeInputEvent {
    let grid_size = size_info.grid_size();
    let cell_size = size_info.cell_size();

    GNativeInputEvent::Resize {
        columns: grid_size.columns() as u32,
        rows: grid_size.rows() as u32,
        content_width_px: size_info.content_width_px(),
        content_height_px: size_info.content_height_px(),
        cell_width_px: cell_size.width_px(),
        cell_height_px: cell_size.height_px(),
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
            WindowInputEvent::ModifiersChanged(modifiers) => Some(
                GNativeInputEvent::ModifiersChanged(gnative_input_modifiers_from(modifiers)),
            ),
            WindowInputEvent::FocusChanged(focused) => {
                Some(GNativeInputEvent::FocusChanged(focused))
            }
            WindowInputEvent::Key {
                state,
                logical_key,
                text,
            } => Some(GNativeInputEvent::Key {
                state: gnative_input_state_from(state),
                logical_key: gnative_input_key_from(&logical_key),
                text: text.as_deref().map(ToOwned::to_owned),
                modifiers: gnative_input_modifiers_from(modifiers),
            }),
            WindowInputEvent::Ime(text) => Some(GNativeInputEvent::Ime(text)),
            WindowInputEvent::Paste(text) => Some(GNativeInputEvent::Paste(text)),
            WindowInputEvent::PointerMoved {
                position,
                modifiers,
            } => Some(GNativeInputEvent::PointerMoved {
                position: gnative_pointer_position_from(position),
                modifiers: gnative_input_modifiers_from(modifiers),
            }),
            WindowInputEvent::PointerLeft => Some(GNativeInputEvent::PointerLeft),
            WindowInputEvent::PointerButton {
                state,
                button,
                position,
                modifiers,
            } => Some(GNativeInputEvent::PointerButton {
                state: gnative_input_state_from(state),
                button: gnative_pointer_button_from(button),
                position: gnative_pointer_position_from(position),
                modifiers: gnative_input_modifiers_from(modifiers),
            }),
            WindowInputEvent::Scroll {
                delta,
                position,
                modifiers,
            } => Some(GNativeInputEvent::Scroll {
                delta: match delta {
                    germinal_ports::event::window_input_event::WindowScrollDelta::Lines {
                        x,
                        y,
                    } => GNativeScrollDelta::Lines { x, y },
                    germinal_ports::event::window_input_event::WindowScrollDelta::Pixels {
                        x,
                        y,
                    } => GNativeScrollDelta::Pixels { x, y },
                },
                position: gnative_pointer_position_from(position),
                modifiers: gnative_input_modifiers_from(modifiers),
            }),
        },
    }
}

fn gnative_pointer_position_from(
    position: germinal_ports::event::window_input_event::WindowPointerPosition,
) -> GNativePointerPosition {
    GNativePointerPosition {
        x_px: position.x_px,
        y_px: position.y_px,
    }
}

fn gnative_pointer_button_from(
    button: germinal_ports::event::window_input_event::WindowPointerButton,
) -> GNativePointerButton {
    match button {
        germinal_ports::event::window_input_event::WindowPointerButton::Primary => {
            GNativePointerButton::Primary
        }
        germinal_ports::event::window_input_event::WindowPointerButton::Secondary => {
            GNativePointerButton::Secondary
        }
        germinal_ports::event::window_input_event::WindowPointerButton::Middle => {
            GNativePointerButton::Middle
        }
        germinal_ports::event::window_input_event::WindowPointerButton::Back => {
            GNativePointerButton::Back
        }
        germinal_ports::event::window_input_event::WindowPointerButton::Forward => {
            GNativePointerButton::Forward
        }
        germinal_ports::event::window_input_event::WindowPointerButton::Other(value) => {
            GNativePointerButton::Other(value)
        }
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
    GNativeInputModifiers {
        control: modifiers.control_key(),
        alt: modifiers.alt_key(),
        shift: modifiers.shift_key(),
        super_key: modifiers.super_key(),
    }
}

fn gnative_input_key_from(
    key: &germinal_ports::event::window_input_event::WindowInputKey,
) -> GNativeInputKey {
    match key {
        germinal_ports::event::window_input_event::WindowInputKey::Named(named) => {
            let named = match named {
                germinal_ports::event::window_input_event::WindowInputNamedKey::F1 => {
                    GNativeInputNamedKey::F1
                }
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
                _ => return GNativeInputKey::Unidentified,
            };
            GNativeInputKey::Named(named)
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
        input::{
            GNativeInputElementState, GNativeInputEvent, GNativeInputKey, GNativeInputModifiers,
            GNativeInputNamedKey, GNativePointerPosition, GNativeScrollDelta,
        },
        session::GNativeSessionAccepted,
    };
    use germinal_ports::{
        event::{
            gshell_input::{GShellInput, GShellInputEvent},
            window_input_event::{
                WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
                WindowInputNamedKey, WindowPointerPosition, WindowScrollDelta,
            },
        },
        pty_host::{
            cell_size::TerminalCellSize,
            size_info::{TerminalPadding, TerminalSizeInfo},
            window_size::TerminalWindowSize,
        },
    };

    use super::{
        GNativeServiceState, GNativeSessionRuntime, gnative_input_event_from,
        gnative_input_key_from, gnative_resize_event,
    };

    #[test]
    fn state_stores_session_runtime_by_gshell_id() {
        let state = GNativeServiceState::new();
        let runtime = GNativeSessionRuntime {
            accepted: GNativeSessionAccepted {
                gshell_id: GShellId::new(9),
                protocol_version: 1,
            },
        };

        state.upsert_session(runtime.clone());

        assert_eq!(state.session_of(GShellId::new(9)), Some(runtime));
    }

    #[test]
    fn extended_pty_keys_do_not_change_the_gnative_protocol() {
        assert_eq!(
            gnative_input_key_from(&WindowInputKey::Named(WindowInputNamedKey::F2)),
            GNativeInputKey::Unidentified,
        );
        assert_eq!(
            gnative_input_key_from(&WindowInputKey::Named(WindowInputNamedKey::PageDown)),
            GNativeInputKey::Unidentified,
        );
    }

    #[test]
    fn remove_session_clears_runtime_and_modifiers() {
        let state = GNativeServiceState::new();
        let gshell_id = GShellId::new(10);
        state.upsert_session(GNativeSessionRuntime {
            accepted: GNativeSessionAccepted {
                gshell_id,
                protocol_version: 1,
            },
        });
        state.set_modifiers(gshell_id, WindowInputModifiers::new(true, true, true, true));

        let removed = state.remove_session(gshell_id);

        assert!(removed.is_some());
        assert_eq!(state.session_of(gshell_id), None);
        assert_eq!(
            state.modifiers_of(gshell_id),
            WindowInputModifiers::new(false, false, false, false)
        );
    }

    #[test]
    fn maps_window_key_input_to_gnative_key_event() {
        let input = GShellInput {
            gshell_id: GShellId::new(1),
            event: GShellInputEvent::Window(WindowInputEvent::Key {
                state: WindowInputElementState::Pressed,
                logical_key: WindowInputKey::Character("a".into()),
                text: Some("a".into()),
            }),
        };

        let mapped =
            gnative_input_event_from(input, WindowInputModifiers::new(true, false, false, false));
        assert_eq!(
            mapped,
            Some(GNativeInputEvent::Key {
                state: GNativeInputElementState::Pressed,
                logical_key: GNativeInputKey::Character("a".to_string()),
                text: Some("a".to_string()),
                modifiers: GNativeInputModifiers {
                    control: true,
                    alt: false,
                    shift: false,
                    super_key: false,
                },
            })
        );
    }

    #[test]
    fn maps_window_named_f1_to_gnative_named_f1() {
        let input = GShellInput {
            gshell_id: GShellId::new(2),
            event: GShellInputEvent::Window(WindowInputEvent::Key {
                state: WindowInputElementState::Pressed,
                logical_key: WindowInputKey::Named(WindowInputNamedKey::F1),
                text: None,
            }),
        };

        let mapped =
            gnative_input_event_from(input, WindowInputModifiers::new(false, false, false, false));
        assert_eq!(
            mapped,
            Some(GNativeInputEvent::Key {
                state: GNativeInputElementState::Pressed,
                logical_key: GNativeInputKey::Named(GNativeInputNamedKey::F1),
                text: None,
                modifiers: GNativeInputModifiers {
                    control: false,
                    alt: false,
                    shift: false,
                    super_key: false,
                },
            })
        );
    }

    #[test]
    fn maps_window_focus_to_gnative_focus_event() {
        let input = GShellInput {
            gshell_id: GShellId::new(2),
            event: GShellInputEvent::Window(WindowInputEvent::FocusChanged(true)),
        };

        assert_eq!(
            gnative_input_event_from(input, WindowInputModifiers::new(false, false, false, false)),
            Some(GNativeInputEvent::FocusChanged(true))
        );
    }

    #[test]
    fn maps_pixel_scroll_with_local_position_and_all_modifiers() {
        let modifiers = WindowInputModifiers::new(true, false, true, true);
        let input = GShellInput {
            gshell_id: GShellId::new(2),
            event: GShellInputEvent::Window(WindowInputEvent::Scroll {
                delta: WindowScrollDelta::Pixels { x: 0.25, y: -12.5 },
                position: WindowPointerPosition::new(41.75, 9.125),
                modifiers,
            }),
        };

        assert_eq!(
            gnative_input_event_from(input, WindowInputModifiers::new(false, false, false, false)),
            Some(GNativeInputEvent::Scroll {
                delta: GNativeScrollDelta::Pixels { x: 0.25, y: -12.5 },
                position: GNativePointerPosition {
                    x_px: 41.75,
                    y_px: 9.125
                },
                modifiers: GNativeInputModifiers {
                    control: true,
                    alt: false,
                    shift: true,
                    super_key: true,
                },
            })
        );
    }

    #[test]
    fn resize_event_includes_grid_content_and_cell_dimensions() {
        let size_info = TerminalSizeInfo::new(
            TerminalWindowSize::new(101, 55),
            TerminalCellSize::new(10, 20),
            TerminalPadding::new(3, 4),
        );

        assert_eq!(
            gnative_resize_event(size_info),
            GNativeInputEvent::Resize {
                columns: 9,
                rows: 2,
                content_width_px: 95,
                content_height_px: 47,
                cell_width_px: 10,
                cell_height_px: 20,
            }
        );
    }
}
