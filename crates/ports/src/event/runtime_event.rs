use germinal_domain::gshell::vo::gshell_id::GShellId;

use crate::{gnative::session::GNativeSessionDescriptor, seq::Seq};

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GShellRuntimeEvent {
	EnterGNative { descriptor: GNativeSessionDescriptor },
	ExitGNative { gshell_id: GShellId },
	FrameReady { gshell_id: GShellId, seq: Seq },
	Closed { gshell_id: GShellId },
}
