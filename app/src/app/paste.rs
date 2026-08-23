use arboard::Clipboard;
use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_ports::event::{
    gshell_input::{GShellInput, GShellInputEvent},
    window_input_event::WindowInputElementState,
};
use germinal_ports::pty_host::terminal_clipboard::TerminalClipboard;
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

#[derive(Debug, Error)]
pub enum PasteError {
    #[error("failed to open clipboard: {0}")]
    OpenClipboard(#[source] arboard::Error),
    #[error("failed to read clipboard text: {0}")]
    ReadClipboard(#[source] arboard::Error),
}

#[derive(Debug, Error)]
pub enum CopyError {
    #[error("failed to open clipboard: {0}")]
    OpenClipboard(#[source] arboard::Error),
    #[error("failed to write clipboard text: {0}")]
    WriteClipboard(#[source] arboard::Error),
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

    pub fn clipboard_paste_input(
        &mut self,
        gshell_id: GShellId,
    ) -> Result<Option<GShellInput>, PasteError> {
        let text = self.read_clipboard_text()?;
        if text.is_empty() {
            return Ok(None);
        }

        Ok(Some(GShellInput {
            gshell_id,
            event: GShellInputEvent::Paste(text),
        }))
    }

    pub fn write_clipboard_text(&mut self, text: String) -> Result<(), CopyError> {
        self.write_terminal_clipboard_text(TerminalClipboard::Clipboard, text)
    }

    pub fn write_terminal_clipboard_text(
        &mut self,
        target: TerminalClipboard,
        text: String,
    ) -> Result<(), CopyError> {
        let mut clipboard = Clipboard::new().map_err(CopyError::OpenClipboard)?;
        write_clipboard_text(&mut clipboard, target, text).map_err(CopyError::WriteClipboard)
    }

    pub fn read_terminal_clipboard_text(
        &mut self,
        target: TerminalClipboard,
    ) -> Result<String, PasteError> {
        let mut clipboard = Clipboard::new().map_err(PasteError::OpenClipboard)?;
        read_clipboard_text(&mut clipboard, target).map_err(PasteError::ReadClipboard)
    }

    pub fn effective_modifiers(&self) -> HostPasteModifiers {
        HostPasteModifiers {
            control: self.modifiers.control || self.pressed.control_key(),
            shift: self.modifiers.shift || self.pressed.shift_key(),
        }
    }

    fn read_clipboard_text(&mut self) -> Result<String, PasteError> {
        let mut clipboard = Clipboard::new().map_err(PasteError::OpenClipboard)?;
        clipboard.get_text().map_err(PasteError::ReadClipboard)
    }
}

#[cfg(target_os = "linux")]
fn write_clipboard_text(
    clipboard: &mut Clipboard,
    target: TerminalClipboard,
    text: String,
) -> Result<(), arboard::Error> {
    use arboard::{LinuxClipboardKind, SetExtLinux};

    let target = match target {
        TerminalClipboard::Clipboard => LinuxClipboardKind::Clipboard,
        TerminalClipboard::Selection => LinuxClipboardKind::Primary,
    };
    clipboard.set().clipboard(target).text(text)
}

#[cfg(not(target_os = "linux"))]
fn write_clipboard_text(
    clipboard: &mut Clipboard,
    _target: TerminalClipboard,
    text: String,
) -> Result<(), arboard::Error> {
    clipboard.set_text(text)
}

#[cfg(target_os = "linux")]
fn read_clipboard_text(
    clipboard: &mut Clipboard,
    target: TerminalClipboard,
) -> Result<String, arboard::Error> {
    use arboard::{GetExtLinux, LinuxClipboardKind};

    let target = match target {
        TerminalClipboard::Clipboard => LinuxClipboardKind::Clipboard,
        TerminalClipboard::Selection => LinuxClipboardKind::Primary,
    };
    clipboard.get().clipboard(target).text()
}

#[cfg(not(target_os = "linux"))]
fn read_clipboard_text(
    clipboard: &mut Clipboard,
    _target: TerminalClipboard,
) -> Result<String, arboard::Error> {
    clipboard.get_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_physical_modifiers_fill_missing_modifiers_changed_events() {
        let mut controller = HostPasteController::default();
        controller.observe_key_event(
            WindowInputElementState::Pressed,
            PhysicalKey::Code(KeyCode::ControlLeft),
        );
        controller.observe_key_event(
            WindowInputElementState::Pressed,
            PhysicalKey::Code(KeyCode::ShiftLeft),
        );

        assert_eq!(
            controller.effective_modifiers(),
            HostPasteModifiers {
                control: true,
                shift: true,
            }
        );
    }
}
