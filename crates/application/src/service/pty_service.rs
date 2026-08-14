use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc,
        atomic::AtomicBool,
        mpsc::{Sender, SyncSender},
    },
    time::{Duration, Instant},
};

use germinal_domain::{
    gshell::vo::gshell_id::GShellId,
    pty_host::{pty_host_id::PtyHostId, terminal_size::TerminalGridSize},
};
use germinal_gnative_protocol::gnative::session::GNATIVE_PROTOCOL_VERSION;
use germinal_ports::{
    event::{
        gshell_input::GShellInputEvent,
        runtime_event_dispatcher::IRuntimeEventDispatcherProvider,
        window_input_event::{
            WindowInputElementState, WindowInputEvent, WindowInputKey, WindowInputModifiers,
            WindowPointerButton, WindowPointerPosition, WindowScrollDelta,
        },
    },
    pty_host::{
        pty_backend::{IPtyBackend, IPtyBackendProvider},
        pty_input::{PtyInput, PtyInputSender},
        terminal_input_mode::TerminalInputModeState,
        terminal_size::TerminalPtySize,
        worker_input::{
            TerminalDisplayScroll, TerminalSelectionKind, TerminalSelectionPoint,
            TerminalWorkerInput,
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
    PtyMouseEncoder, PtyScrollAction, encode_focus_changed, encode_ime_commit, encode_key_event,
    encode_paste, mouse_reporting_enabled,
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
}

#[derive(kudi::DepInj)]
#[target(PtyService)]
pub struct PtyServiceState {
    pty_host_runtimes: RefCell<HashMap<PtyHostId, PtyPaneRuntime>>,
    modifiers: RefCell<WindowInputModifiers>,
}

impl PtyServiceState {
    pub fn new() -> Self {
        Self {
            pty_host_runtimes: RefCell::new(HashMap::new()),
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
        pty_size: TerminalPtySize,
        term_size: TerminalGridSize,
        surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
        snapshot_wake_pending: Arc<AtomicBool>,
    ) {
        let state: &PtyServiceState = self.prj_ref().as_ref();
        if state.pty_host_runtimes.borrow().contains_key(&pty_host_id) {
            return;
        }

        let proxy = self.prj_ref().runtime_event_dispatcher().clone();
        let Some(terminal_worker_sender) = self.prj_ref().spawn_terminal_worker(
            gshell_id,
            term_size,
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
            pty_size,
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
            },
        );
    }

    fn send_pty_host_input(&self, pty_host_id: PtyHostId, event: GShellInputEvent) {
        let state: &PtyServiceState = self.prj_ref().as_ref();
        match event {
            GShellInputEvent::Bytes(bytes) => send_pty_host_bytes(state, pty_host_id, bytes),
            GShellInputEvent::Paste(text) => send_pty_host_paste(state, pty_host_id, &text),
            GShellInputEvent::CopySelection => request_pty_host_selection(state, pty_host_id),
            GShellInputEvent::Window(window_input) => match window_input {
                WindowInputEvent::ModifiersChanged(modifiers) => {
                    *state.modifiers.borrow_mut() = modifiers;
                }
                WindowInputEvent::FocusChanged(focused) => {
                    send_pty_host_focus(state, pty_host_id, focused);
                }
                WindowInputEvent::Key {
                    state: key_state,
                    logical_key,
                    text,
                } => {
                    let modifiers = *state.modifiers.borrow();
                    send_pty_host_key(
                        state,
                        pty_host_id,
                        modifiers,
                        key_state,
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
    }

    fn resize_pty_host(
        &self,
        pty_host_id: PtyHostId,
        pty_size: TerminalPtySize,
        term_size: TerminalGridSize,
    ) {
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
            .send(TerminalWorkerInput::Resize(term_size));
    }
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

fn send_pty_host_bytes(state: &PtyServiceState, pty_host_id: PtyHostId, bytes: Vec<u8>) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };

    return_to_live_display(runtime);
    let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_paste(state: &PtyServiceState, pty_host_id: PtyHostId, text: &str) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };
    let Some(bytes) = encode_paste(runtime.input_modes.load(), text) else {
        return;
    };

    return_to_live_display(runtime);
    let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_focus(state: &PtyServiceState, pty_host_id: PtyHostId, focused: bool) {
    let runtimes = state.pty_host_runtimes.borrow();
    let Some(runtime) = runtimes.get(&pty_host_id) else {
        return;
    };
    let Some(bytes) = encode_focus_changed(runtime.input_modes.load(), focused) else {
        return;
    };

    let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
}

fn send_pty_host_key(
    state: &PtyServiceState,
    pty_host_id: PtyHostId,
    modifiers: WindowInputModifiers,
    key_state: WindowInputElementState,
    logical_key: &WindowInputKey,
    text: Option<&str>,
) {
    let mut runtimes = state.pty_host_runtimes.borrow_mut();
    let Some(runtime) = runtimes.get_mut(&pty_host_id) else {
        return;
    };
    let Some(bytes) = encode_key_event(
        runtime.input_modes.load(),
        modifiers,
        key_state,
        logical_key,
        text,
    ) else {
        return;
    };

    return_to_live_display(runtime);
    let _ = runtime.pty_input_sender.send(PtyInput::Bytes(bytes));
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
    let modes = runtime.input_modes.load();
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
    let modes = runtime.input_modes.load();
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
    let action = runtime
        .mouse
        .scroll(runtime.input_modes.load(), delta, position, modifiers);
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

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use germinal_domain::pty_host::pty_host_id::PtyHostId;
    use germinal_ports::{
        event::window_input_event::{
            WindowInputElementState, WindowInputModifiers, WindowPointerButton,
            WindowPointerPosition,
        },
        pty_host::{
            pty_input::pty_input_channel,
            terminal_input_mode::TerminalInputModeState,
            terminal_size::TerminalPtySize,
            worker_input::{
                TerminalDisplayScroll, TerminalSelectionKind, TerminalSelectionPoint,
                TerminalSelectionSide, TerminalWorkerInput,
            },
        },
    };

    use super::{
        PtyClickTracker, PtyMouseEncoder, PtyPaneRuntime, PtyServiceState,
        request_pty_host_selection, return_to_live_display, send_pty_host_pointer_button,
        send_pty_host_pointer_moved,
    };

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
}
