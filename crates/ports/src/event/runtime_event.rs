use germinal_domain::{shared::seq::Seq, workspace::pane_id::PaneId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
	App(AppRuntimeEvent),
	Workspace(WorkspaceRuntimeEvent),
	Pane(PaneRuntimeEvent),
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
pub enum PaneRuntimeEvent {
	FrameReady { pane_id: PaneId, seq: Seq },
	Closed { pane_id: PaneId },
}
