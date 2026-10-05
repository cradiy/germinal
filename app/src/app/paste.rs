use std::{thread, time::Duration};

use arboard::Clipboard;
use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_ports::event::{
    runtime_event::{AppRuntimeEvent, ClipboardPasteOutcome, RuntimeEvent},
    runtime_event_dispatcher::IRuntimeEventDispatcher,
    window_input_event::WindowInputElementState,
};
use germinal_ports::pty_host::terminal_clipboard::TerminalClipboard;
use thiserror::Error;
use winit::keyboard::{KeyCode, PhysicalKey};

const CLIPBOARD_RETRY_DELAY: Duration = Duration::from_millis(20);

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
    // X11 serves copied data while this handle is alive, even after focus changes.
    clipboard: Option<Clipboard>,
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

    pub fn reset_modifiers(&mut self) {
        self.modifiers = HostPasteModifiers::default();
        self.pressed = HostPressedModifiers::default();
    }

    pub fn request_clipboard_paste<D>(&self, gshell_id: GShellId, dispatcher: D)
    where
        D: IRuntimeEventDispatcher,
    {
        thread::spawn(move || {
            let outcome = read_clipboard_text_with_retry(read_host_clipboard_text, thread::sleep);
            let _ = dispatcher.dispatch(RuntimeEvent::App(AppRuntimeEvent::ClipboardPasteReady {
                gshell_id,
                outcome,
            }));
        });
    }

    pub fn write_clipboard_text(&mut self, text: String) -> Result<(), CopyError> {
        self.write_terminal_clipboard_text(TerminalClipboard::Clipboard, text)
    }

    pub fn write_terminal_clipboard_text(
        &mut self,
        target: TerminalClipboard,
        text: String,
    ) -> Result<(), CopyError> {
        if self.clipboard.is_none() {
            self.clipboard = Some(Clipboard::new().map_err(CopyError::OpenClipboard)?);
        }
        let clipboard = self.clipboard.as_mut().expect("clipboard initialized");
        write_clipboard_text(clipboard, target, text).map_err(CopyError::WriteClipboard)
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
}

fn read_host_clipboard_text() -> Result<String, PasteError> {
    let mut clipboard = Clipboard::new().map_err(PasteError::OpenClipboard)?;
    clipboard.get_text().map_err(PasteError::ReadClipboard)
}

fn read_clipboard_text_with_retry(
    mut read: impl FnMut() -> Result<String, PasteError>,
    mut wait: impl FnMut(Duration),
) -> ClipboardPasteOutcome {
    for attempt in 0..=1 {
        match read() {
            Ok(text) if text.is_empty() => return ClipboardPasteOutcome::Unavailable,
            Ok(text) => return ClipboardPasteOutcome::Text(text),
            Err(error) if attempt == 0 && clipboard_error_is_retryable(&error) => {
                wait(CLIPBOARD_RETRY_DELAY);
            }
            Err(PasteError::ReadClipboard(arboard::Error::ContentNotAvailable)) => {
                return ClipboardPasteOutcome::Unavailable;
            }
            Err(error) => return ClipboardPasteOutcome::Failed(error.to_string()),
        }
    }

    unreachable!("clipboard retry loop always returns")
}

fn clipboard_error_is_retryable(error: &PasteError) -> bool {
    matches!(
        error,
        PasteError::OpenClipboard(arboard::Error::ClipboardOccupied)
            | PasteError::ReadClipboard(
                arboard::Error::ContentNotAvailable | arboard::Error::ClipboardOccupied
            )
    )
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
    use std::cell::Cell;

    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "writes clipboard and primary selection; requires an isolated X11 server without a clipboard manager"]
    fn x11_copy_survives_return_focus_reset_and_replacement() {
        let display = std::env::var("GERMINAL_TEST_X11_DISPLAY")
            .expect("explicit isolated X11 display required; never use the desktop display");
        assert_eq!(std::env::var("DISPLAY").as_deref(), Ok(display.as_str()));
        assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());

        if let Ok(expected) = std::env::var("GERMINAL_TEST_CLIPBOARD_READER") {
            let mut reader = HostPasteController::default();
            assert_eq!(
                reader
                    .read_terminal_clipboard_text(TerminalClipboard::Clipboard)
                    .unwrap(),
                expected
            );
            assert_eq!(
                reader
                    .read_terminal_clipboard_text(TerminalClipboard::Selection)
                    .unwrap(),
                format!("primary {expected}")
            );
            return;
        }

        let mut owner = HostPasteController::default();
        for text in ["first 中文 😈", "replacement"] {
            owner.write_clipboard_text(text.to_owned()).unwrap();
            owner
                .write_terminal_clipboard_text(
                    TerminalClipboard::Selection,
                    format!("primary {text}"),
                )
                .unwrap();
            owner.reset_modifiers();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "app::paste::tests::x11_copy_survives_return_focus_reset_and_replacement",
                    "--ignored",
                    "--nocapture",
                ])
                .env("GERMINAL_TEST_CLIPBOARD_READER", text)
                .output()
                .expect("independent clipboard reader");
            assert!(
                output.status.success(),
                "reader failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

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

    #[test]
    fn focus_reset_clears_reported_and_physically_tracked_modifiers() {
        let mut controller = HostPasteController::default();
        controller.set_modifiers(HostPasteModifiers {
            control: true,
            shift: true,
        });
        controller.observe_key_event(
            WindowInputElementState::Pressed,
            PhysicalKey::Code(KeyCode::ControlLeft),
        );
        controller.observe_key_event(
            WindowInputElementState::Pressed,
            PhysicalKey::Code(KeyCode::ShiftRight),
        );

        controller.reset_modifiers();

        assert_eq!(
            controller.effective_modifiers(),
            HostPasteModifiers::default()
        );
    }

    #[test]
    fn clipboard_text_is_retried_once_when_temporarily_unavailable() {
        let attempts = Cell::new(0);
        let waited = Cell::new(None);

        let outcome = read_clipboard_text_with_retry(
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() == 1 {
                    Err(PasteError::ReadClipboard(
                        arboard::Error::ContentNotAvailable,
                    ))
                } else {
                    Ok("second attempt".to_owned())
                }
            },
            |duration| waited.set(Some(duration)),
        );

        assert_eq!(
            outcome,
            ClipboardPasteOutcome::Text("second attempt".to_owned())
        );
        assert_eq!(attempts.get(), 2);
        assert_eq!(waited.get(), Some(CLIPBOARD_RETRY_DELAY));
    }

    #[test]
    fn clipboard_failure_is_not_retried_when_it_is_not_transient() {
        let attempts = Cell::new(0);

        let outcome = read_clipboard_text_with_retry(
            || {
                attempts.set(attempts.get() + 1);
                Err(PasteError::ReadClipboard(arboard::Error::ConversionFailure))
            },
            |_| panic!("permanent clipboard errors must not wait for a retry"),
        );

        assert!(matches!(outcome, ClipboardPasteOutcome::Failed(_)));
        assert_eq!(attempts.get(), 1);
    }
}
