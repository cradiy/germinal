use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_domain::workspace::vo::pane_split_direction::PaneSplitDirection;
use germinal_gnative_protocol::{gnative::session::GNativeSessionAccepted, seq::Seq};

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
    SelectionText {
        gshell_id: GShellId,
        text: Option<String>,
    },
    Closed {
        gshell_id: GShellId,
    },
}
