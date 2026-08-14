use arboard::Clipboard;
use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_ports::event::{
    gshell_input::{GShellInput, GShellInputEvent},
    window_input_event::{WindowInputElementState, WindowInputKey},
};
use thiserror::Error;
use winit::keyboard::{KeyCode, PhysicalKey};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostPasteModifiers {
    pub control: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct HostPressedModifiers {
    left_control: bool,
    right_control: bool,
    left_shift: bool,
    right_shift: bool,
}

impl HostPressedModifiers {
    fn control_key(self) -> bool {
        self.left_control || self.right_control
    }

    fn shift_key(self) -> bool {
        self.left_shift || self.right_shift
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostPasteAction {
    NotHandled,
    Handled,
    HandledEmpty,
    Dispatch(GShellInput),
}

#[derive(Debug, Error)]
pub enum PasteError {
    #[error("failed to open clipboard: {0}")]
    OpenClipboard(#[source] arboard::Error),
    #[error("failed to read clipboard text: {0}")]
    ReadClipboard(#[source] arboard::Error),
}

#[derive(Default)]
pub struct HostPasteController {
    modifiers: HostPasteModifiers,
    pressed: HostPressedModifiers,
}

impl HostPasteController {
    pub fn set_modifiers(&mut self, modifiers: HostPasteModifiers) {
        self.modifiers = modifiers;
    }

    pub fn observe_key_event(&mut self, state: WindowInputElementState, physical_key: PhysicalKey) {
        let pressed = state == WindowInputElementState::Pressed;
        match physical_key {
            PhysicalKey::Code(KeyCode::ControlLeft) => self.pressed.left_control = pressed,
            PhysicalKey::Code(KeyCode::ControlRight) => self.pressed.right_control = pressed,
            PhysicalKey::Code(KeyCode::ShiftLeft) => self.pressed.left_shift = pressed,
            PhysicalKey::Code(KeyCode::ShiftRight) => self.pressed.right_shift = pressed,
            _ => {}
        }
    }

    pub fn handle_shortcut(
        &mut self,
        gshell_id: GShellId,
        state: WindowInputElementState,
        logical_key: &WindowInputKey,
        physical_key: PhysicalKey,
    ) -> Result<HostPasteAction, PasteError> {
        if !matches_paste_shortcut(
            HostPasteModifiers {
                control: self.modifiers.control || self.pressed.control_key(),
                shift: self.modifiers.shift || self.pressed.shift_key(),
            },
            state,
            logical_key,
            physical_key,
        ) {
            return Ok(HostPasteAction::NotHandled);
        }

        if state == WindowInputElementState::Released {
            return Ok(HostPasteAction::Handled);
        }

        let text = self.read_clipboard_text()?;
        if text.is_empty() {
            return Ok(HostPasteAction::HandledEmpty);
        }

        Ok(HostPasteAction::Dispatch(GShellInput {
            gshell_id,
            event: GShellInputEvent::Paste(text),
        }))
    }

    fn read_clipboard_text(&mut self) -> Result<String, PasteError> {
        let mut clipboard = Clipboard::new().map_err(PasteError::OpenClipboard)?;
        clipboard.get_text().map_err(PasteError::ReadClipboard)
    }
}

fn matches_paste_shortcut(
    modifiers: HostPasteModifiers,
    state: WindowInputElementState,
    logical_key: &WindowInputKey,
    physical_key: PhysicalKey,
) -> bool {
    if !modifiers.control || !modifiers.shift {
        return false;
    }

    if !matches!(
        state,
        WindowInputElementState::Pressed | WindowInputElementState::Released
    ) {
        return false;
    }

    matches!(physical_key, PhysicalKey::Code(KeyCode::KeyV))
        || matches!(
            logical_key,
            WindowInputKey::Character(text) if text.eq_ignore_ascii_case("v")
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_shift_v_pressed_requests_paste_dispatch() {
        assert!(matches_paste_shortcut(
            HostPasteModifiers {
                control: true,
                shift: true
            },
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("v".into()),
            PhysicalKey::Code(KeyCode::KeyV),
        ));
    }

    #[test]
    fn ctrl_v_without_shift_does_not_trigger_host_paste() {
        assert!(!matches_paste_shortcut(
            HostPasteModifiers {
                control: true,
                shift: false
            },
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("v".into()),
            PhysicalKey::Code(KeyCode::KeyV),
        ));
    }

    #[test]
    fn released_shortcut_is_still_consumed() {
        assert!(matches_paste_shortcut(
            HostPasteModifiers {
                control: true,
                shift: true
            },
            WindowInputElementState::Released,
            &WindowInputKey::Character("V".into()),
            PhysicalKey::Code(KeyCode::KeyV),
        ));
    }

    #[test]
    fn physical_v_without_character_still_triggers_paste() {
        assert!(matches_paste_shortcut(
            HostPasteModifiers {
                control: true,
                shift: true
            },
            WindowInputElementState::Pressed,
            &WindowInputKey::Unidentified,
            PhysicalKey::Code(KeyCode::KeyV),
        ));
    }

    #[test]
    fn tracked_physical_modifiers_trigger_paste_without_modifiers_changed_event() {
        let mut controller = HostPasteController::default();
        controller.observe_key_event(
            WindowInputElementState::Pressed,
            PhysicalKey::Code(KeyCode::ControlLeft),
        );
        controller.observe_key_event(
            WindowInputElementState::Pressed,
            PhysicalKey::Code(KeyCode::ShiftLeft),
        );

        assert!(matches!(
            controller.handle_shortcut(
                GShellId::new(7),
                WindowInputElementState::Pressed,
                &WindowInputKey::Unidentified,
                PhysicalKey::Code(KeyCode::KeyV),
            ),
            Err(_)
                | Ok(HostPasteAction::Handled)
                | Ok(HostPasteAction::HandledEmpty)
                | Ok(HostPasteAction::Dispatch(_))
        ));
    }
}
