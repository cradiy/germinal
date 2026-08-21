use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_domain::workspace::vo::pane_split_direction::PaneSplitDirection;
use germinal_gnative_protocol::{gnative::session::GNativeSessionAccepted, seq::Seq};

use crate::pty_host::hyperlink::TerminalHyperlink;
use crate::pty_host::terminal_clipboard::TerminalClipboard;
use crate::pty_host::terminal_notification::TerminalNotification;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    App(AppRuntimeEvent),
    Workspace(WorkspaceRuntimeEvent),
    GShell(GShellRuntimeEvent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppRuntimeEvent {
    ShutdownRequested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceRuntimeEvent {
    RedrawRequested,
    SplitFocusedPane { direction: PaneSplitDirection },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GShellRuntimeEvent {
    EnterGNative {
        gshell_id: GShellId,
    },
    GNativeConnected {
        accepted: GNativeSessionAccepted,
    },
    GNativeConnectionFailed {
        gshell_id: GShellId,
        reason: String,
    },
    ExitGNative {
        gshell_id: GShellId,
    },
    FrameReady {
        gshell_id: GShellId,
        seq: Seq,
    },
    TitleChanged {
        gshell_id: GShellId,
        title: Option<String>,
    },
    HyperlinksChanged {
        gshell_id: GShellId,
        hyperlinks: Vec<TerminalHyperlink>,
    },
    Bell {
        gshell_id: GShellId,
    },
    SystemNotificationRequested {
        gshell_id: GShellId,
        notification: TerminalNotification,
    },
    SystemNotificationActivated {
        gshell_id: GShellId,
    },
    Osc52ClipboardStore {
        gshell_id: GShellId,
        clipboard: TerminalClipboard,
        text: String,
    },
    Osc52ClipboardLoad {
        gshell_id: GShellId,
        clipboard: TerminalClipboard,
        request_id: u64,
    },
    SelectionText {
        gshell_id: GShellId,
        text: Option<String>,
    },
    Closed {
        gshell_id: GShellId,
    },
}
