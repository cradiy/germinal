use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::AtomicBool,
        mpsc::{Sender, SyncSender},
    },
    time::{Duration, Instant},
};

use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::pty_host_id::PtyHostId};
use germinal_gnative_protocol::gnative::session::GNATIVE_PROTOCOL_VERSION;
use germinal_ports::{
    event::{
        gshell_input::GShellInputEvent,
        runtime_event_dispatcher::IRuntimeEventDispatcherProvider,
        window_input_event::{
            WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
            WindowInputNamedKey, WindowPointerButton, WindowPointerPosition, WindowScrollDelta,
        },
    },
    pty_host::{
        pty_backend::{IPtyBackend, IPtyBackendProvider},
        pty_input::{PtyInput, PtyInputSender},
        spawn_config::PtySpawnConfig,
        terminal_input_mode::{TerminalInputModeState, TerminalInputModes},
        terminal_size::TerminalPtySize,
        worker_input::{
            TerminalDisplayScroll, TerminalSelectionKind, TerminalSelectionPoint, TerminalViMotion,
            TerminalViSearchDirection, TerminalViSearchPrompt, TerminalViSelectionKind,
            TerminalViTextObject, TerminalWorkerInput,
        },
    },
    rendering::surface_snapshot::RenderSurfaceSnapshot,
    service::{
        gnative_tunnel::{IGNativeTunnel, IGNativeTunnelProvider},
        pty_service::IPtyService,
        worker_service::IWorkerService,
    },
};
use tracing::warn;

use super::pty_input_encoder::{
    PtyMouseEncoder, PtyScrollAction, encode_focus_changed, encode_ime_commit,
    encode_key_event_with_repeat, encode_paste, mouse_reporting_enabled,
};

const MULTI_CLICK_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy)]
struct PtyClick {
    at: Instant,
    column: u16,
    row: u16,
    count: u8,
}

#[derive(Debug, Default)]
struct PtyClickTracker {
    last: Option<PtyClick>,
}

impl PtyClickTracker {
    fn record(&mut self, point: TerminalSelectionPoint) -> u8 {
        self.record_at(point, Instant::now())
    }

    fn record_at(&mut self, point: TerminalSelectionPoint, now: Instant) -> u8 {
        let count = self
            .last
            .filter(|last| {
                now.saturating_duration_since(last.at) <= MULTI_CLICK_INTERVAL
                    && last.column == point.column
                    && last.row == point.row
            })
            .map_or(1, |last| if last.count < 3 { last.count + 1 } else { 1 });
        self.last = Some(PtyClick {
            at: now,
            column: point.column,
            row: point.row,
            count,
        });
        count
    }

    fn reset(&mut self) {
        self.last = None;
    }
}

struct PtyPaneRuntime {
    pty_input_sender: PtyInputSender,
    terminal_worker_sender: SyncSender<TerminalWorkerInput>,
    input_modes: TerminalInputModeState,
    mouse: PtyMouseEncoder,
    click_tracker: PtyClickTracker,
    selection_dragging: bool,
    selection_end: Option<TerminalSelectionPoint>,
    display_scrolled: bool,
    vi_mode: bool,
    vi_pending_g: bool,
    vi_selection_kind: Option<TerminalViSelectionKind>,
    vi_pending_text_object: Option<TerminalViTextObject>,
    vi_search_input: Option<ViSearchInput>,
    vi_last_search: Option<ViSearch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViSearchInput {
    direction: TerminalViSearchDirection,
    query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViSearch {
    direction: TerminalViSearchDirection,
    pattern: String,
}

#[derive(kudi::DepInj)]
#[target(PtyService)]
pub struct PtyServiceState {
    pty_host_runtimes: RefCell<HashMap<PtyHostId, PtyPaneRuntime>>,
    reported_working_directories: RefCell<HashMap<PtyHostId, PathBuf>>,
    modifiers: RefCell<WindowInputModifiers>,
}

impl PtyServiceState {
    pub fn new() -> Self {
        Self {
            pty_host_runtimes: RefCell::new(HashMap::new()),
            reported_working_directories: RefCell::new(HashMap::new()),
            modifiers: RefCell::new(WindowInputModifiers::new(false, false, false, false)),
        }
    }
}

impl Default for PtyServiceState {
    fn default() -> Self {
        Self::new()
    }
}

impl<Deps> IPtyService for PtyService<Deps>
where
    Deps: AsRef<PtyServiceState>
        + IRuntimeEventDispatcherProvider
        + IGNativeTunnelProvider
        + IPtyBackendProvider
        + IWorkerService<TerminalWorkerSender = SyncSender<TerminalWorkerInput>>,
{
    fn ensure_gshell_pty(
        &self,
        gshell_id: GShellId,
        pty_host_id: PtyHostId,
        spawn_config: PtySpawnConfig,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) {
        let state: &PtyServiceState = self.prj_ref().as_ref();
        if state.pty_host_runtimes.borrow().contains_key(&pty_host_id) {
            return;
        }

        let proxy = self.prj_ref().runtime_event_dispatcher().clone();
        let pty_size = spawn_config.initial_size;
        let Some(terminal_worker_sender) = self.prj_ref().spawn_terminal_worker(
            gshell_id,
            pty_size,
            surface_snapshot_tx,
            snapshot_wake_pending,
        ) else {
            return;
        };
        let shell_env = match self
            .prj_ref()
            .gnative_tunnel()
            .ensure_session_descriptor(gshell_id, GNATIVE_PROTOCOL_VERSION)
        {
            Ok(descriptor) => descriptor.tunnel_env(),
            Err(error) => {
                warn!(gshell_id = gshell_id.value(), error = %error, "failed to prepare gnative tunnel");
                return;
            }
        };

        let pty_input_sender = self.prj_ref().pty_backend().spawn_pty(
            proxy,
            gshell_id,
            pty_host_id,
            spawn_config,
            shell_env,
            terminal_worker_sender.clone(),
        );

        let input_modes = TerminalInputModeState::default();
        let _ = terminal_worker_sender.send(TerminalWorkerInput::SetPtyInput {
            sender: pty_input_sender.clone(),
            input_modes: input_modes.clone(),
        });

        state.pty_host_runtimes.borrow_mut().insert(
            pty_host_id,
            PtyPaneRuntime {
                pty_input_sender,
                terminal_worker_sender,
                input_modes,
                mouse: PtyMouseEncoder::new(pty_size),
                click_tracker: PtyClickTracker::default(),
                selection_dragging: false,
                selection_end: None,
                display_scrolled: false,
                vi_mode: false,
                vi_pending_g: false,
                vi_selection_kind: None,
                vi_pending_text_object: None,
                vi_search_input: None,
                vi_last_search: None,
            },
        );
    }

    fn send_pty_host_input(&self, pty_host_id: PtyHostId, event: GShellInputEvent) {
        let state: &PtyServiceState = self.prj_ref().as_ref();
        match event {
            GShellInputEvent::Bytes(bytes) => send_pty_host_bytes(state, pty_host_id, bytes),
            GShellInputEvent::Paste(text) => send_pty_host_paste(state, pty_host_id, &text),
            GShellInputEvent::CopySelection => request_pty_host_selection(state, pty_host_id),
            GShellInputEvent::Osc52ClipboardLoadResponse {
                clipboard,
                request_id,
                text,
            } => {
                let runtimes = state.pty_host_runtimes.borrow();
                if let Some(runtime) = runtimes.get(&pty_host_id) {
                    let _ = runtime.terminal_worker_sender.send(
                        TerminalWorkerInput::Osc52ClipboardLoadResponse {
                            clipboard,
                            request_id,
                            text,
                        },
                    );
                }
            }
            GShellInputEvent::ToggleViMode => toggle_pty_host_vi_mode(state, pty_host_id),
            GShellInputEvent::ToggleSearch => toggle_pty_host_search(state, pty_host_id),
            GShellInputEvent::Window(window_input) => match window_input {
                WindowInputEvent::ModifiersChanged(modifiers) => {
                    *state.modifiers.borrow_mut() = modifiers;
                }
                WindowInputEvent::FocusChanged(focused) => {
                    send_pty_host_focus(state, pty_host_id, focused);
                }
                WindowInputEvent::Key {
                    state: key_state,
                    repeat,
                    logical_key,
                    text,
                } => {
                    let modifiers = *state.modifiers.borrow();
                    send_pty_host_key_event(
                        state,
                        pty_host_id,
                        modifiers,
                        key_state,
                        repeat,
                        &logical_key,
                        text.as_deref(),
                    );
                }
                WindowInputEvent::Ime(text) => {
                    if let Some(bytes) = encode_ime_commit(&text) {
                        send_pty_host_bytes(state, pty_host_id, bytes);
                    }
                }
                WindowInputEvent::Paste(text) => send_pty_host_paste(state, pty_host_id, &text),
                WindowInputEvent::PointerMoved {
                    position,
                    modifiers,
                } => {
                    send_pty_host_pointer_moved(state, pty_host_id, position, modifiers);
                }
                WindowInputEvent::PointerLeft => {
                    if let Some(runtime) =
                        state.pty_host_runtimes.borrow_mut().get_mut(&pty_host_id)
                    {
                        runtime.mouse.pointer_left();
                        runtime.click_tracker.reset();
                        runtime.selection_dragging = false;
                        runtime.selection_end = None;
                    }
                }
                WindowInputEvent::PointerButton {
                    state: button_state,
                    button,
                    position,
                    modifiers,
                } => {
                    send_pty_host_pointer_button(
                        state,
                        pty_host_id,
                        button_state,
                        button,
                        position,
                        modifiers,
                    );
                }
                WindowInputEvent::Scroll {
                    delta,
                    position,
                    modifiers,
                } => {
                    send_pty_host_scroll(state, pty_host_id, delta, position, modifiers);
                }
            },
        }
    }

    fn remove_pty_host(&self, pty_host_id: PtyHostId) {
        let state: &PtyServiceState = self.prj_ref().as_ref();
        state.pty_host_runtimes.borrow_mut().remove(&pty_host_id);
        state
            .reported_working_directories
            .borrow_mut()
            .remove(&pty_host_id);
    }

    fn pty_host_working_directory(&self, pty_host_id: PtyHostId) -> Option<PathBuf> {
        let state: &PtyServiceState = self.prj_ref().as_ref();
        if let Some(working_directory) = state
            .reported_working_directories
            .borrow()
            .get(&pty_host_id)
            .cloned()
        {
            return Some(working_directory);
        }
        let runtimes = state.pty_host_runtimes.borrow();
        let runtime = runtimes.get(&pty_host_id)?;
        let process_id = runtime.pty_input_sender.child_process_id()?;
        process_working_directory(process_id)
    }

    fn update_pty_host_working_directory(
        &self,
        pty_host_id: PtyHostId,
        working_directory: PathBuf,
    ) {
        let state: &PtyServiceState = self.prj_ref().as_ref();
        if state.pty_host_runtimes.borrow().contains_key(&pty_host_id) {
            state
                .reported_working_directories
                .borrow_mut()
                .insert(pty_host_id, working_directory);
        }
    }

    fn resize_pty_host(&self, pty_host_id: PtyHostId, pty_size: TerminalPtySize) {
        let state: &PtyServiceState = self.prj_ref().as_ref();
        let mut runtimes = state.pty_host_runtimes.borrow_mut();
        let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
            return;
        };

        runtime.mouse.resize(pty_size);
        runtime.click_tracker.reset();
        runtime.selection_dragging = false;
        runtime.selection_end = None;
        let _ = runtime.pty_input_sender.send(PtyInput::Resize(pty_size));
        let _ = runtime
            .terminal_worker_sender
            .send(TerminalWorkerInput::Resize(pty_size));
    }
}

#[cfg(target_os = "linux")]
fn process_working_directory(process_id: u32) -> Option<PathBuf> {
    let path = std::fs::read_link(format!("/proc/{process_id}/cwd")).ok()?;
    path.is_dir().then_some(path)
}

#[cfg(not(target_os = "linux"))]
fn process_working_directory(_process_id: u32) -> Option<PathBuf> {
    None
}

fn request_pty_host_selection(state: &PtyServiceState, pty_host_id: PtyHostId) {
    let runtimes = state.pty_host_runtimes.borrow();
    let Some(runtime) = runtimes.get(&pty_host_id) else {
        return;
    };

    let _ = runtime
        .terminal_worker_sender
        .send(TerminalWorkerInput::RequestSelectionText);
}

fn toggle_pty_host_vi_mode(state: &PtyServiceState, pty_host_id: PtyHostId) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };

    if runtime.vi_mode {
        leave_pty_host_vi_mode(runtime);
        return;
    }
    if host_search_active(runtime) {
        leave_pty_host_search(runtime);
    }

    runtime.vi_mode = true;
    runtime.vi_pending_g = false;
    runtime.vi_selection_kind = None;
    runtime.vi_pending_text_object = None;
    runtime.vi_search_input = None;

    let _ = runtime
        .terminal_worker_sender
        .send(TerminalWorkerInput::SetViMode(true));
}

fn toggle_pty_host_search(state: &PtyServiceState, pty_host_id: PtyHostId) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };

    if runtime.vi_mode {
        if runtime.vi_search_input.is_some() {
            runtime.vi_search_input = None;
            let _ = runtime
                .terminal_worker_sender
                .send(TerminalWorkerInput::SetViSearchPrompt(None));
        } else {
            start_vi_search(runtime, TerminalViSearchDirection::Forward);
        }
        return;
    }
    if host_search_active(runtime) {
        leave_pty_host_search(runtime);
        return;
    }

    runtime.vi_search_input = Some(ViSearchInput {
        direction: TerminalViSearchDirection::Forward,
        query: String::new(),
    });
    let _ = runtime
        .terminal_worker_sender
        .send(TerminalWorkerInput::SetSearchMode(true));
    publish_vi_search_prompt(runtime);
}

fn send_pty_host_bytes(state: &PtyServiceState, pty_host_id: PtyHostId, bytes: Vec<u8>) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };
    if runtime.vi_mode || host_search_active(runtime) {
        return;
    }

    return_to_live_display(runtime);
    let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_paste(state: &PtyServiceState, pty_host_id: PtyHostId, text: &str) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };
    if runtime.vi_mode || host_search_active(runtime) {
        return;
    }
    let Some(bytes) = encode_paste(runtime.input_modes.load(), text) else {
        return;
    };

    return_to_live_display(runtime);
    let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_focus(state: &PtyServiceState, pty_host_id: PtyHostId, focused: bool) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };
    if !focused {
        runtime.click_tracker.reset();
        runtime.selection_dragging = false;
        runtime.selection_end = None;
    }
    let Some(bytes) = encode_focus_changed(runtime.input_modes.load(), focused) else {
        return;
    };

    let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

#[cfg(test)]
fn send_pty_host_key(
    state: &PtyServiceState,
    pty_host_id: PtyHostId,
    modifiers: WindowInputModifiers,
    key_state: WindowInputElementState,
    logical_key: &WindowInputKey,
    text: Option<&str>,
) {
    send_pty_host_key_event(
        state,
        pty_host_id,
        modifiers,
        key_state,
        false,
        logical_key,
        text,
    );
}

fn send_pty_host_key_event(
    state: &PtyServiceState,
    pty_host_id: PtyHostId,
    modifiers: WindowInputModifiers,
    key_state: WindowInputElementState,
    repeat: bool,
    logical_key: &WindowInputKey,
    text: Option<&str>,
) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };
    if host_search_active(runtime) {
        send_host_search_key(runtime, modifiers, key_state, logical_key);
        return;
    }
    if runtime.vi_mode {
        send_vi_mode_key(runtime, modifiers, key_state, logical_key);
        return;
    }
    let Some(bytes) = encode_key_event_with_repeat(
        runtime.input_modes.load(),
        modifiers,
        key_state,
        repeat,
        logical_key,
        text,
    ) else {
        return;
    };

    return_to_live_display(runtime);
    let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn host_search_active(runtime: &PtyPaneRuntime) -> bool {
    !runtime.vi_mode && runtime.vi_search_input.is_some()
}

fn send_host_search_key(
    runtime: &mut PtyPaneRuntime,
    modifiers: WindowInputModifiers,
    key_state: WindowInputElementState,
    logical_key: &WindowInputKey,
) {
    if key_state != WindowInputElementState::Pressed {
        return;
    }

    if matches!(
        logical_key,
        WindowInputKey::Named(WindowInputNamedKey::Escape)
    ) {
        leave_pty_host_search(runtime);
        return;
    }

    if matches!(
        logical_key,
        WindowInputKey::Named(WindowInputNamedKey::Backspace)
    ) {
        if let Some(search) = runtime.vi_search_input.as_mut() {
            search.query.pop();
        }
        publish_vi_search_prompt(runtime);
        return;
    }

    if matches!(
        logical_key,
        WindowInputKey::Named(WindowInputNamedKey::Enter)
    ) {
        let Some(search_input) = runtime.vi_search_input.as_ref() else {
            return;
        };
        let pattern = if search_input.query.is_empty() {
            runtime
                .vi_last_search
                .as_ref()
                .map(|search| search.pattern.clone())
        } else {
            Some(search_input.query.clone())
        };
        let Some(pattern) = pattern else {
            return;
        };
        let direction = if modifiers.shift_key() {
            TerminalViSearchDirection::Backward
        } else {
            TerminalViSearchDirection::Forward
        };
        runtime.vi_last_search = Some(ViSearch {
            direction,
            pattern: pattern.clone(),
        });
        let _ = runtime
            .terminal_worker_sender
            .send(TerminalWorkerInput::ViSearch { pattern, direction });
        return;
    }

    if modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() {
        return;
    }
    let WindowInputKey::Character(text) = logical_key else {
        return;
    };
    let Some(search) = runtime.vi_search_input.as_mut() else {
        return;
    };
    search
        .query
        .extend(text.chars().filter(|character| !character.is_control()));
    publish_vi_search_prompt(runtime);
}

fn leave_pty_host_search(runtime: &mut PtyPaneRuntime) {
    runtime.vi_search_input = None;
    let _ = runtime
        .terminal_worker_sender
        .send(TerminalWorkerInput::SetViSearchPrompt(None));
    let _ = runtime
        .terminal_worker_sender
        .send(TerminalWorkerInput::SetSearchMode(false));
}

fn send_vi_mode_key(
    runtime: &mut PtyPaneRuntime,
    modifiers: WindowInputModifiers,
    key_state: WindowInputElementState,
    logical_key: &WindowInputKey,
) {
    if key_state != WindowInputElementState::Pressed {
        return;
    }

    if runtime.vi_search_input.is_some() {
        send_vi_search_input_key(runtime, modifiers, logical_key);
        return;
    }

    if matches!(
        logical_key,
        WindowInputKey::Named(WindowInputNamedKey::Escape)
    ) {
        runtime.vi_pending_g = false;
        runtime.vi_pending_text_object = None;
        if runtime.vi_selection_kind.take().is_some() {
            let _ = runtime
                .terminal_worker_sender
                .send(TerminalWorkerInput::SetViSelection(None));
        }
        return;
    }

    let WindowInputKey::Character(key) = logical_key else {
        runtime.vi_pending_g = false;
        return;
    };

    if modifiers.control_key() && !modifiers.alt_key() && !modifiers.super_key() {
        runtime.vi_pending_g = false;
        runtime.vi_pending_text_object = None;
        let motion = match key.to_ascii_lowercase().as_str() {
            "u" => Some(TerminalViMotion::HalfPageUp),
            "d" => Some(TerminalViMotion::HalfPageDown),
            "b" => Some(TerminalViMotion::PageUp),
            "f" => Some(TerminalViMotion::PageDown),
            _ => None,
        };
        if let Some(motion) = motion {
            let _ = runtime
                .terminal_worker_sender
                .send(TerminalWorkerInput::ViMotion(motion));
        }
        return;
    }
    if modifiers.alt_key() || modifiers.super_key() {
        runtime.vi_pending_g = false;
        runtime.vi_pending_text_object = None;
        return;
    }

    if let Some(text_object) = runtime.vi_pending_text_object.take() {
        runtime.vi_pending_g = false;
        if key == "w" {
            let _ = runtime
                .terminal_worker_sender
                .send(TerminalWorkerInput::SelectViTextObject(text_object));
        }
        return;
    }

    match key.as_str() {
        "i" | "a" => {
            if runtime.vi_selection_kind.is_some() {
                runtime.vi_pending_text_object = Some(if key == "i" {
                    TerminalViTextObject::InnerWord
                } else {
                    TerminalViTextObject::AroundWord
                });
                runtime.vi_pending_g = false;
                return;
            }
            leave_pty_host_vi_mode(runtime);
            return;
        }
        "q" => {
            leave_pty_host_vi_mode(runtime);
            return;
        }
        "y" => {
            runtime.vi_pending_g = false;
            if runtime.vi_selection_kind.take().is_some() {
                let _ = runtime
                    .terminal_worker_sender
                    .send(TerminalWorkerInput::RequestSelectionText);
                let _ = runtime
                    .terminal_worker_sender
                    .send(TerminalWorkerInput::SetViSelection(None));
            }
            return;
        }
        "v" => {
            runtime.vi_pending_g = false;
            runtime.vi_selection_kind = toggle_vi_selection_kind(
                runtime.vi_selection_kind,
                TerminalViSelectionKind::Character,
            );
            let _ = runtime
                .terminal_worker_sender
                .send(TerminalWorkerInput::SetViSelection(
                    runtime.vi_selection_kind,
                ));
            return;
        }
        "V" => {
            runtime.vi_pending_g = false;
            runtime.vi_selection_kind =
                toggle_vi_selection_kind(runtime.vi_selection_kind, TerminalViSelectionKind::Line);
            let _ = runtime
                .terminal_worker_sender
                .send(TerminalWorkerInput::SetViSelection(
                    runtime.vi_selection_kind,
                ));
            return;
        }
        "/" => {
            start_vi_search(runtime, TerminalViSearchDirection::Forward);
            return;
        }
        "?" => {
            start_vi_search(runtime, TerminalViSearchDirection::Backward);
            return;
        }
        "n" | "N" => {
            runtime.vi_pending_g = false;
            runtime.vi_pending_text_object = None;
            let Some(search) = runtime.vi_last_search.clone() else {
                return;
            };
            let direction = if key == "n" {
                search.direction
            } else {
                search.direction.opposite()
            };
            let _ = runtime
                .terminal_worker_sender
                .send(TerminalWorkerInput::ViSearch {
                    pattern: search.pattern,
                    direction,
                });
            return;
        }
        _ => {}
    }

    let motion = match key.as_str() {
        "h" => Some(TerminalViMotion::Left),
        "j" => Some(TerminalViMotion::Down),
        "k" => Some(TerminalViMotion::Up),
        "l" => Some(TerminalViMotion::Right),
        "0" => Some(TerminalViMotion::First),
        "^" => Some(TerminalViMotion::FirstOccupied),
        "$" => Some(TerminalViMotion::Last),
        "w" => Some(TerminalViMotion::WordRight),
        "b" => Some(TerminalViMotion::WordLeft),
        "e" => Some(TerminalViMotion::WordRightEnd),
        "H" => Some(TerminalViMotion::High),
        "M" => Some(TerminalViMotion::Middle),
        "L" => Some(TerminalViMotion::Low),
        "G" => Some(TerminalViMotion::Bottom),
        "g" if runtime.vi_pending_g => Some(TerminalViMotion::Top),
        "g" => {
            runtime.vi_pending_g = true;
            None
        }
        _ => None,
    };

    if key != "g" || motion.is_some() {
        runtime.vi_pending_g = false;
    }
    if let Some(motion) = motion {
        let _ = runtime
            .terminal_worker_sender
            .send(TerminalWorkerInput::ViMotion(motion));
    }
}

fn leave_pty_host_vi_mode(runtime: &mut PtyPaneRuntime) {
    runtime.vi_mode = false;
    runtime.vi_pending_g = false;
    runtime.vi_pending_text_object = None;
    runtime.vi_search_input = None;
    runtime.display_scrolled = false;
    if runtime.vi_selection_kind.take().is_some() {
        let _ = runtime
            .terminal_worker_sender
            .send(TerminalWorkerInput::SetViSelection(None));
    }
    let _ = runtime
        .terminal_worker_sender
        .send(TerminalWorkerInput::SetViMode(false));
}

fn start_vi_search(runtime: &mut PtyPaneRuntime, direction: TerminalViSearchDirection) {
    runtime.vi_pending_g = false;
    runtime.vi_pending_text_object = None;
    runtime.vi_search_input = Some(ViSearchInput {
        direction,
        query: String::new(),
    });
    publish_vi_search_prompt(runtime);
}

fn send_vi_search_input_key(
    runtime: &mut PtyPaneRuntime,
    modifiers: WindowInputModifiers,
    logical_key: &WindowInputKey,
) {
    if matches!(
        logical_key,
        WindowInputKey::Named(WindowInputNamedKey::Escape)
    ) {
        runtime.vi_search_input = None;
        let _ = runtime
            .terminal_worker_sender
            .send(TerminalWorkerInput::SetViSearchPrompt(None));
        return;
    }

    if matches!(
        logical_key,
        WindowInputKey::Named(WindowInputNamedKey::Backspace)
    ) {
        if let Some(search) = runtime.vi_search_input.as_mut() {
            search.query.pop();
        }
        publish_vi_search_prompt(runtime);
        return;
    }

    if matches!(
        logical_key,
        WindowInputKey::Named(WindowInputNamedKey::Enter)
    ) {
        let Some(search_input) = runtime.vi_search_input.take() else {
            return;
        };
        let pattern = if search_input.query.is_empty() {
            runtime
                .vi_last_search
                .as_ref()
                .map(|search| search.pattern.clone())
        } else {
            Some(search_input.query)
        };
        let _ = runtime
            .terminal_worker_sender
            .send(TerminalWorkerInput::SetViSearchPrompt(None));
        let Some(pattern) = pattern else {
            return;
        };
        runtime.vi_last_search = Some(ViSearch {
            direction: search_input.direction,
            pattern: pattern.clone(),
        });
        let _ = runtime
            .terminal_worker_sender
            .send(TerminalWorkerInput::ViSearch {
                pattern,
                direction: search_input.direction,
            });
        return;
    }

    if modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() {
        return;
    }
    let WindowInputKey::Character(text) = logical_key else {
        return;
    };
    let Some(search) = runtime.vi_search_input.as_mut() else {
        return;
    };
    search
        .query
        .extend(text.chars().filter(|character| !character.is_control()));
    publish_vi_search_prompt(runtime);
}

fn publish_vi_search_prompt(runtime: &PtyPaneRuntime) {
    let prompt = runtime
        .vi_search_input
        .as_ref()
        .map(|search| TerminalViSearchPrompt {
            direction: search.direction,
            query: search.query.clone(),
        });
    let _ = runtime
        .terminal_worker_sender
        .send(TerminalWorkerInput::SetViSearchPrompt(prompt));
}

fn toggle_vi_selection_kind(
    current: Option<TerminalViSelectionKind>,
    requested: TerminalViSelectionKind,
) -> Option<TerminalViSelectionKind> {
    if current == Some(requested) {
        None
    } else {
        Some(requested)
    }
}

fn return_to_live_display(runtime: &mut PtyPaneRuntime) {
    if !runtime.display_scrolled {
        return;
    }

    let _ = runtime
        .terminal_worker_sender
        .send(TerminalWorkerInput::ScrollDisplay(
            TerminalDisplayScroll::Bottom,
        ));
    runtime.display_scrolled = false;
}

fn send_pty_host_pointer_moved(
    state: &PtyServiceState,
    pty_host_id: PtyHostId,
    position: WindowPointerPosition,
    modifiers: WindowInputModifiers,
) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };
    let modes = host_navigation_input_modes(runtime);
    let selection_point = runtime.mouse.selection_point(position);
    let bytes = runtime.mouse.moved(modes, position, modifiers);

    if let Some(bytes) = bytes {
        let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
        return;
    }
    if mouse_reporting_enabled(modes) || !runtime.selection_dragging {
        return;
    }
    let Some(point) = selection_point else {
        return;
    };
    if runtime.selection_end == Some(point) {
        return;
    }
    runtime.selection_end = Some(point);

    let _ = runtime
        .terminal_worker_sender
        .send(TerminalWorkerInput::UpdateSelection(point));
}

fn send_pty_host_pointer_button(
    state: &PtyServiceState,
    pty_host_id: PtyHostId,
    button_state: WindowInputElementState,
    button: WindowPointerButton,
    position: WindowPointerPosition,
    modifiers: WindowInputModifiers,
) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };
    let modes = host_navigation_input_modes(runtime);
    let selection_point = runtime.mouse.selection_point(position);
    let bytes = runtime
        .mouse
        .button(modes, button_state, button, position, modifiers);

    if let Some(bytes) = bytes {
        runtime.click_tracker.reset();
        runtime.selection_dragging = false;
        runtime.selection_end = None;
        let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
        return;
    }
    if mouse_reporting_enabled(modes) || button != WindowPointerButton::Primary {
        if mouse_reporting_enabled(modes) {
            runtime.click_tracker.reset();
            runtime.selection_dragging = false;
            runtime.selection_end = None;
        }
        return;
    }

    match button_state {
        WindowInputElementState::Released => {
            runtime.selection_dragging = false;
            runtime.selection_end = None;
        }
        WindowInputElementState::Pressed => {
            let Some(point) = selection_point else {
                return;
            };
            let kind = match runtime.click_tracker.record(point) {
                1 => TerminalSelectionKind::Character,
                2 => TerminalSelectionKind::Word,
                _ => TerminalSelectionKind::Line,
            };
            runtime.selection_dragging = true;
            runtime.selection_end = Some(point);
            let _ = runtime
                .terminal_worker_sender
                .send(TerminalWorkerInput::StartSelection { kind, point });
        }
    }
}

fn send_pty_host_scroll(
    state: &PtyServiceState,
    pty_host_id: PtyHostId,
    delta: WindowScrollDelta,
    position: WindowPointerPosition,
    modifiers: WindowInputModifiers,
) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };
    let modes = host_navigation_input_modes(runtime);
    let action = runtime.mouse.scroll(modes, delta, position, modifiers);
    match action {
        PtyScrollAction::ReportToPty(reports) => {
            for bytes in reports {
                let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
            }
        }
        PtyScrollAction::ScrollDisplay(lines) if lines != 0 => {
            let _ = runtime
                .terminal_worker_sender
                .send(TerminalWorkerInput::ScrollDisplay(
                    TerminalDisplayScroll::Delta(lines),
                ));
            if lines > 0 {
                runtime.display_scrolled = true;
            }
        }
        PtyScrollAction::ScrollDisplay(_) => {}
    }
}

fn host_navigation_input_modes(runtime: &PtyPaneRuntime) -> TerminalInputModes {
    if runtime.vi_mode || host_search_active(runtime) {
        TerminalInputModes::default()
    } else {
        runtime.input_modes.load()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use germinal_domain::pty_host::pty_host_id::PtyHostId;
    use germinal_ports::{
        event::window_input_event::{
            WindowInputElementState, WindowInputKey, WindowInputModifiers, WindowInputNamedKey,
            WindowPointerButton, WindowPointerPosition,
        },
        pty_host::{
            pty_input::pty_input_channel,
            terminal_input_mode::TerminalInputModeState,
            terminal_size::TerminalPtySize,
            worker_input::{
                TerminalDisplayScroll, TerminalSelectionKind, TerminalSelectionPoint,
                TerminalSelectionSide, TerminalViMotion, TerminalViSearchDirection,
                TerminalViSearchPrompt, TerminalViSelectionKind, TerminalViTextObject,
                TerminalWorkerInput,
            },
        },
    };

    use super::{
        PtyClickTracker, PtyMouseEncoder, PtyPaneRuntime, PtyServiceState,
        request_pty_host_selection, return_to_live_display, send_pty_host_focus, send_pty_host_key,
        send_pty_host_pointer_button, send_pty_host_pointer_moved, send_vi_mode_key,
        toggle_pty_host_search, toggle_pty_host_vi_mode,
    };

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_a_live_process_working_directory_from_proc() {
        assert_eq!(
            super::process_working_directory(std::process::id()),
            std::env::current_dir().ok()
        );
    }

    #[test]
    fn input_returns_a_scrolled_pane_to_the_live_display_once() {
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(2);
        let mut runtime = PtyPaneRuntime {
            pty_input_sender,
            terminal_worker_sender,
            input_modes: TerminalInputModeState::default(),
            mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
            click_tracker: PtyClickTracker::default(),
            selection_dragging: false,
            selection_end: None,
            display_scrolled: true,
            vi_mode: false,
            vi_pending_g: false,
            vi_selection_kind: None,
            vi_pending_text_object: None,
            vi_search_input: None,
            vi_last_search: None,
        };

        return_to_live_display(&mut runtime);
        assert!(!runtime.display_scrolled);
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::ScrollDisplay(
                TerminalDisplayScroll::Bottom
            ))
        ));

        return_to_live_display(&mut runtime);
        assert!(terminal_worker_rx.try_recv().is_err());
    }

    #[test]
    fn vi_mode_toggle_routes_to_the_terminal_worker() {
        let state = PtyServiceState::new();
        let pty_host_id = PtyHostId::new(1);
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(1);
        state.pty_host_runtimes.borrow_mut().insert(
            pty_host_id,
            PtyPaneRuntime {
                pty_input_sender,
                terminal_worker_sender,
                input_modes: TerminalInputModeState::default(),
                mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
                click_tracker: PtyClickTracker::default(),
                selection_dragging: false,
                selection_end: None,
                display_scrolled: false,
                vi_mode: false,
                vi_pending_g: false,
                vi_selection_kind: None,
                vi_pending_text_object: None,
                vi_search_input: None,
                vi_last_search: None,
            },
        );

        toggle_pty_host_vi_mode(&state, pty_host_id);

        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViMode(true))
        ));
        assert!(
            state
                .pty_host_runtimes
                .borrow()
                .get(&pty_host_id)
                .is_some_and(|runtime| runtime.vi_mode)
        );

        let modifiers = WindowInputModifiers::new(false, false, false, false);
        send_pty_host_key(
            &state,
            pty_host_id,
            modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("k".into()),
            Some("k"),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::ViMotion(TerminalViMotion::Up))
        ));

        send_pty_host_key(
            &state,
            pty_host_id,
            modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("x".into()),
            Some("x"),
        );
        assert!(terminal_worker_rx.try_recv().is_err());
    }

    #[test]
    fn host_search_intercepts_input_and_navigates_in_both_directions() {
        let state = PtyServiceState::new();
        let pty_host_id = PtyHostId::new(2);
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(16);
        state.pty_host_runtimes.borrow_mut().insert(
            pty_host_id,
            PtyPaneRuntime {
                pty_input_sender,
                terminal_worker_sender,
                input_modes: TerminalInputModeState::default(),
                mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
                click_tracker: PtyClickTracker::default(),
                selection_dragging: false,
                selection_end: None,
                display_scrolled: false,
                vi_mode: false,
                vi_pending_g: false,
                vi_selection_kind: None,
                vi_pending_text_object: None,
                vi_search_input: None,
                vi_last_search: None,
            },
        );

        toggle_pty_host_search(&state, pty_host_id);
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetSearchMode(true))
        ));
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSearchPrompt(Some(
                TerminalViSearchPrompt {
                    direction: TerminalViSearchDirection::Forward,
                    ref query,
                }
            ))) if query.is_empty()
        ));

        let no_modifiers = WindowInputModifiers::new(false, false, false, false);
        send_pty_host_key(
            &state,
            pty_host_id,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("needle".into()),
            Some("needle"),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSearchPrompt(Some(
                TerminalViSearchPrompt {
                    direction: TerminalViSearchDirection::Forward,
                    ref query,
                }
            ))) if query == "needle"
        ));

        for (modifiers, direction) in [
            (no_modifiers, TerminalViSearchDirection::Forward),
            (
                WindowInputModifiers::new(false, false, true, false),
                TerminalViSearchDirection::Backward,
            ),
        ] {
            send_pty_host_key(
                &state,
                pty_host_id,
                modifiers,
                WindowInputElementState::Pressed,
                &WindowInputKey::Named(WindowInputNamedKey::Enter),
                None,
            );
            assert!(matches!(
                terminal_worker_rx.try_recv(),
                Ok(TerminalWorkerInput::ViSearch { ref pattern, direction: actual })
                    if pattern == "needle" && actual == direction
            ));
        }

        send_pty_host_key(
            &state,
            pty_host_id,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Named(WindowInputNamedKey::Escape),
            None,
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSearchPrompt(None))
        ));
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetSearchMode(false))
        ));
    }

    #[test]
    fn vi_mode_recognizes_gg_and_uppercase_g() {
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(2);
        let mut runtime = PtyPaneRuntime {
            pty_input_sender,
            terminal_worker_sender,
            input_modes: TerminalInputModeState::default(),
            mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
            click_tracker: PtyClickTracker::default(),
            selection_dragging: false,
            selection_end: None,
            display_scrolled: false,
            vi_mode: true,
            vi_pending_g: false,
            vi_selection_kind: None,
            vi_pending_text_object: None,
            vi_search_input: None,
            vi_last_search: None,
        };
        let no_modifiers = WindowInputModifiers::new(false, false, false, false);

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("g".into()),
        );
        assert!(terminal_worker_rx.try_recv().is_err());
        assert!(runtime.vi_pending_g);
        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("g".into()),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::ViMotion(TerminalViMotion::Top))
        ));

        send_vi_mode_key(
            &mut runtime,
            WindowInputModifiers::new(false, false, true, false),
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("G".into()),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::ViMotion(TerminalViMotion::Bottom))
        ));
    }

    #[test]
    fn vi_mode_routes_viewport_motions_without_pty_input() {
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(7);
        let mut runtime = PtyPaneRuntime {
            pty_input_sender,
            terminal_worker_sender,
            input_modes: TerminalInputModeState::default(),
            mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
            click_tracker: PtyClickTracker::default(),
            selection_dragging: false,
            selection_end: None,
            display_scrolled: false,
            vi_mode: true,
            vi_pending_g: false,
            vi_selection_kind: None,
            vi_pending_text_object: None,
            vi_search_input: None,
            vi_last_search: None,
        };

        for (key, modifiers) in [
            ("u", WindowInputModifiers::new(true, false, false, false)),
            ("d", WindowInputModifiers::new(true, false, false, false)),
            ("b", WindowInputModifiers::new(true, false, false, false)),
            ("f", WindowInputModifiers::new(true, false, false, false)),
            ("H", WindowInputModifiers::new(false, false, true, false)),
            ("M", WindowInputModifiers::new(false, false, true, false)),
            ("L", WindowInputModifiers::new(false, false, true, false)),
        ] {
            send_vi_mode_key(
                &mut runtime,
                modifiers,
                WindowInputElementState::Pressed,
                &WindowInputKey::Character(key.into()),
            );
        }

        for expected in [
            TerminalViMotion::HalfPageUp,
            TerminalViMotion::HalfPageDown,
            TerminalViMotion::PageUp,
            TerminalViMotion::PageDown,
            TerminalViMotion::High,
            TerminalViMotion::Middle,
            TerminalViMotion::Low,
        ] {
            assert!(matches!(
                terminal_worker_rx.try_recv(),
                Ok(TerminalWorkerInput::ViMotion(actual)) if actual == expected
            ));
        }
    }

    #[test]
    fn vi_mode_search_edits_commits_and_repeats_locally() {
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(16);
        let mut runtime = PtyPaneRuntime {
            pty_input_sender,
            terminal_worker_sender,
            input_modes: TerminalInputModeState::default(),
            mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
            click_tracker: PtyClickTracker::default(),
            selection_dragging: false,
            selection_end: None,
            display_scrolled: false,
            vi_mode: true,
            vi_pending_g: false,
            vi_selection_kind: None,
            vi_pending_text_object: None,
            vi_search_input: None,
            vi_last_search: None,
        };
        let no_modifiers = WindowInputModifiers::new(false, false, false, false);

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("/".into()),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSearchPrompt(Some(
                TerminalViSearchPrompt {
                    direction: TerminalViSearchDirection::Forward,
                    ref query,
                }
            ))) if query.is_empty()
        ));

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("foo".into()),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSearchPrompt(Some(
                TerminalViSearchPrompt { ref query, .. }
            ))) if query == "foo"
        ));
        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Named(WindowInputNamedKey::Backspace),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSearchPrompt(Some(
                TerminalViSearchPrompt { ref query, .. }
            ))) if query == "fo"
        ));
        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("o".into()),
        );
        let _ = terminal_worker_rx.try_recv();

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Named(WindowInputNamedKey::Enter),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSearchPrompt(None))
        ));
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::ViSearch {
                ref pattern,
                direction: TerminalViSearchDirection::Forward,
            }) if pattern == "foo"
        ));

        for (key, expected_direction) in [
            ("n", TerminalViSearchDirection::Forward),
            ("N", TerminalViSearchDirection::Backward),
        ] {
            send_vi_mode_key(
                &mut runtime,
                no_modifiers,
                WindowInputElementState::Pressed,
                &WindowInputKey::Character(key.into()),
            );
            assert!(matches!(
                terminal_worker_rx.try_recv(),
                Ok(TerminalWorkerInput::ViSearch { ref pattern, direction })
                    if pattern == "foo" && direction == expected_direction
            ));
        }
        assert!(runtime.vi_mode);
    }

    #[test]
    fn vi_mode_handles_visual_selection_and_insert_mode_locally() {
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(3);
        let mut runtime = PtyPaneRuntime {
            pty_input_sender,
            terminal_worker_sender,
            input_modes: TerminalInputModeState::default(),
            mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
            click_tracker: PtyClickTracker::default(),
            selection_dragging: false,
            selection_end: None,
            display_scrolled: false,
            vi_mode: true,
            vi_pending_g: false,
            vi_selection_kind: None,
            vi_pending_text_object: None,
            vi_search_input: None,
            vi_last_search: None,
        };
        let no_modifiers = WindowInputModifiers::new(false, false, false, false);

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("v".into()),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSelection(Some(
                TerminalViSelectionKind::Character
            )))
        ));

        send_vi_mode_key(
            &mut runtime,
            WindowInputModifiers::new(false, false, true, false),
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("V".into()),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSelection(Some(
                TerminalViSelectionKind::Line
            )))
        ));

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("i".into()),
        );
        assert!(runtime.vi_mode);
        assert_eq!(
            runtime.vi_pending_text_object,
            Some(TerminalViTextObject::InnerWord)
        );
        assert!(terminal_worker_rx.try_recv().is_err());

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Named(WindowInputNamedKey::Escape),
        );
        assert!(runtime.vi_mode);
        assert_eq!(runtime.vi_pending_text_object, None);
        assert_eq!(runtime.vi_selection_kind, None);
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSelection(None))
        ));

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("i".into()),
        );
        assert!(!runtime.vi_mode);
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViMode(false))
        ));
    }

    #[test]
    fn vi_mode_composes_vw_viw_and_vaw_without_pty_input() {
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(4);
        let mut runtime = PtyPaneRuntime {
            pty_input_sender,
            terminal_worker_sender,
            input_modes: TerminalInputModeState::default(),
            mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
            click_tracker: PtyClickTracker::default(),
            selection_dragging: false,
            selection_end: None,
            display_scrolled: false,
            vi_mode: true,
            vi_pending_g: false,
            vi_selection_kind: None,
            vi_pending_text_object: None,
            vi_search_input: None,
            vi_last_search: None,
        };
        let no_modifiers = WindowInputModifiers::new(false, false, false, false);

        for key in ["v", "w"] {
            send_vi_mode_key(
                &mut runtime,
                no_modifiers,
                WindowInputElementState::Pressed,
                &WindowInputKey::Character(key.into()),
            );
        }
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSelection(Some(
                TerminalViSelectionKind::Character
            )))
        ));
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::ViMotion(TerminalViMotion::WordRight))
        ));

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("i".into()),
        );
        assert!(runtime.vi_mode);
        assert_eq!(
            runtime.vi_pending_text_object,
            Some(TerminalViTextObject::InnerWord)
        );
        assert!(terminal_worker_rx.try_recv().is_err());

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("w".into()),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SelectViTextObject(
                TerminalViTextObject::InnerWord
            ))
        ));

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("a".into()),
        );
        assert!(runtime.vi_mode);
        assert_eq!(
            runtime.vi_pending_text_object,
            Some(TerminalViTextObject::AroundWord)
        );
        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("w".into()),
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SelectViTextObject(
                TerminalViTextObject::AroundWord
            ))
        ));
    }

    #[test]
    fn vi_mode_yank_requests_copy_and_returns_to_navigation() {
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(3);
        let mut runtime = PtyPaneRuntime {
            pty_input_sender,
            terminal_worker_sender,
            input_modes: TerminalInputModeState::default(),
            mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
            click_tracker: PtyClickTracker::default(),
            selection_dragging: false,
            selection_end: None,
            display_scrolled: false,
            vi_mode: true,
            vi_pending_g: false,
            vi_selection_kind: Some(TerminalViSelectionKind::Character),
            vi_pending_text_object: None,
            vi_search_input: None,
            vi_last_search: None,
        };
        let no_modifiers = WindowInputModifiers::new(false, false, false, false);

        send_vi_mode_key(
            &mut runtime,
            no_modifiers,
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("y".into()),
        );

        assert!(runtime.vi_mode);
        assert_eq!(runtime.vi_selection_kind, None);
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::RequestSelectionText)
        ));
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSelection(None))
        ));
    }

    #[test]
    fn vi_mode_a_exits_from_navigation() {
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(1);
        let mut runtime = PtyPaneRuntime {
            pty_input_sender,
            terminal_worker_sender,
            input_modes: TerminalInputModeState::default(),
            mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
            click_tracker: PtyClickTracker::default(),
            selection_dragging: false,
            selection_end: None,
            display_scrolled: false,
            vi_mode: true,
            vi_pending_g: false,
            vi_selection_kind: None,
            vi_pending_text_object: None,
            vi_search_input: None,
            vi_last_search: None,
        };

        send_vi_mode_key(
            &mut runtime,
            WindowInputModifiers::new(false, false, false, false),
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("a".into()),
        );

        assert!(!runtime.vi_mode);
        assert_eq!(runtime.vi_pending_text_object, None);
        assert_eq!(runtime.vi_selection_kind, None);
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViMode(false))
        ));
    }

    #[test]
    fn vi_mode_q_clears_visual_selection_and_returns_to_live_input() {
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(2);
        let mut runtime = PtyPaneRuntime {
            pty_input_sender,
            terminal_worker_sender,
            input_modes: TerminalInputModeState::default(),
            mouse: PtyMouseEncoder::new(TerminalPtySize::new(80, 24, 800, 480)),
            click_tracker: PtyClickTracker::default(),
            selection_dragging: false,
            selection_end: None,
            display_scrolled: true,
            vi_mode: true,
            vi_pending_g: false,
            vi_selection_kind: Some(TerminalViSelectionKind::Character),
            vi_pending_text_object: None,
            vi_search_input: None,
            vi_last_search: None,
        };

        send_vi_mode_key(
            &mut runtime,
            WindowInputModifiers::new(false, false, false, false),
            WindowInputElementState::Pressed,
            &WindowInputKey::Character("q".into()),
        );

        assert!(!runtime.vi_mode);
        assert!(!runtime.display_scrolled);
        assert_eq!(runtime.vi_selection_kind, None);
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViSelection(None))
        ));
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::SetViMode(false))
        ));
    }

    #[test]
    fn repeated_clicks_on_one_cell_cycle_through_single_double_and_triple() {
        let mut tracker = PtyClickTracker::default();
        let point = TerminalSelectionPoint::new(4, 2, TerminalSelectionSide::Left);
        let started_at = std::time::Instant::now();

        assert_eq!(tracker.record_at(point, started_at), 1);
        assert_eq!(
            tracker.record_at(point, started_at + Duration::from_millis(100)),
            2
        );
        assert_eq!(
            tracker.record_at(point, started_at + Duration::from_millis(200)),
            3
        );
        assert_eq!(
            tracker.record_at(point, started_at + Duration::from_millis(300)),
            1
        );
        assert_eq!(
            tracker.record_at(point, started_at + Duration::from_secs(1)),
            1
        );
    }

    #[test]
    fn pointer_input_routes_character_word_and_line_selection_to_worker() {
        let state = PtyServiceState::new();
        let pty_host_id = PtyHostId::new(1);
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(8);
        state.pty_host_runtimes.borrow_mut().insert(
            pty_host_id,
            PtyPaneRuntime {
                pty_input_sender,
                terminal_worker_sender,
                input_modes: TerminalInputModeState::default(),
                mouse: PtyMouseEncoder::new(TerminalPtySize::new(2, 10, 100, 20)),
                click_tracker: PtyClickTracker::default(),
                selection_dragging: false,
                selection_end: None,
                display_scrolled: false,
                vi_mode: false,
                vi_pending_g: false,
                vi_selection_kind: None,
                vi_pending_text_object: None,
                vi_search_input: None,
                vi_last_search: None,
            },
        );
        let position = WindowPointerPosition::new(15.0, 5.0);
        let modifiers = WindowInputModifiers::new(false, false, false, false);

        send_pty_host_pointer_button(
            &state,
            pty_host_id,
            WindowInputElementState::Pressed,
            WindowPointerButton::Primary,
            position,
            modifiers,
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::StartSelection {
                kind: TerminalSelectionKind::Character,
                point: TerminalSelectionPoint {
                    column: 1,
                    row: 0,
                    side: TerminalSelectionSide::Right,
                },
            })
        ));

        send_pty_host_pointer_moved(
            &state,
            pty_host_id,
            WindowPointerPosition::new(35.0, 5.0),
            modifiers,
        );
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::UpdateSelection(
                TerminalSelectionPoint {
                    column: 3,
                    row: 0,
                    side: TerminalSelectionSide::Right,
                }
            ))
        ));

        for expected_kind in [TerminalSelectionKind::Word, TerminalSelectionKind::Line] {
            send_pty_host_pointer_button(
                &state,
                pty_host_id,
                WindowInputElementState::Released,
                WindowPointerButton::Primary,
                position,
                modifiers,
            );
            send_pty_host_pointer_button(
                &state,
                pty_host_id,
                WindowInputElementState::Pressed,
                WindowPointerButton::Primary,
                position,
                modifiers,
            );
            assert!(matches!(
                terminal_worker_rx.try_recv(),
                Ok(TerminalWorkerInput::StartSelection { kind, .. }) if kind == expected_kind
            ));
        }

        request_pty_host_selection(&state, pty_host_id);
        assert!(matches!(
            terminal_worker_rx.try_recv(),
            Ok(TerminalWorkerInput::RequestSelectionText)
        ));
    }

    #[test]
    fn losing_focus_ends_drag_without_clearing_the_terminal_selection() {
        let state = PtyServiceState::new();
        let pty_host_id = PtyHostId::new(2);
        let (pty_input_sender, _pty_input_rx) = pty_input_channel();
        let (terminal_worker_sender, terminal_worker_rx) = mpsc::sync_channel(2);
        let selection_end = TerminalSelectionPoint::new(3, 1, TerminalSelectionSide::Right);
        state.pty_host_runtimes.borrow_mut().insert(
            pty_host_id,
            PtyPaneRuntime {
                pty_input_sender,
                terminal_worker_sender,
                input_modes: TerminalInputModeState::default(),
                mouse: PtyMouseEncoder::new(TerminalPtySize::new(2, 10, 100, 20)),
                click_tracker: PtyClickTracker::default(),
                selection_dragging: true,
                selection_end: Some(selection_end),
                display_scrolled: false,
                vi_mode: false,
                vi_pending_g: false,
                vi_selection_kind: None,
                vi_pending_text_object: None,
                vi_search_input: None,
                vi_last_search: None,
            },
        );

        send_pty_host_focus(&state, pty_host_id, false);

        let runtimes = state.pty_host_runtimes.borrow();
        let runtime = runtimes.get(&pty_host_id).unwrap();
        assert!(!runtime.selection_dragging);
        assert_eq!(runtime.selection_end, None);
        assert!(terminal_worker_rx.try_recv().is_err());
    }
}
