use germinal_domain::{
    gshell::vo::gshell_id::GShellId,
    workspace::vo::{
        pane_resize_direction::PaneResizeDirection, pane_split_direction::PaneSplitDirection,
    },
};
use thiserror::Error;

use crate::{
    pty_host::window_size::TerminalWindowSize, rendering::workspace_layout::RenderSurfacePlacement,
    repository::RepositoryError,
};

#[derive(Debug, Error)]
pub enum WorkspaceServiceError {
    #[error("workspace repository operation failed: {source}")]
    Repository {
        #[source]
        source: RepositoryError,
    },
    #[error("workspace persistence id is not initialized")]
    PersistenceIdNotInitialized,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceGShellCloseOutcome {
    CloseWorkspace,
    Closed {
        closed_gshells: Vec<GShellId>,
        focused_gshell: GShellId,
    },
}

pub trait IWorkspaceService {
    fn focused_gshell(&self) -> GShellId;
    fn focus_gshell(&self, gshell_id: GShellId) -> bool;
    fn focus_next_gshell(&self) -> GShellId;
    fn focus_previous_gshell(&self) -> GShellId;
    fn create_tab_gshell(&self) -> GShellId;
    fn activate_next_tab(&self) -> GShellId;
    fn activate_previous_tab(&self) -> GShellId;
    fn move_active_tab_left(&self) -> bool;
    fn move_active_tab_right(&self) -> bool;
    fn tab_count(&self) -> usize;
    fn active_tab_index(&self) -> usize;
    fn tab_titles(&self) -> Vec<String>;
    fn tab_gshells(&self) -> Vec<GShellId>;
    fn update_gshell_title(&self, gshell_id: GShellId, title: Option<String>);
    fn split_focused_gshell(&self, direction: PaneSplitDirection) -> GShellId;
    fn swap_focused_gshell_with(&self, other: GShellId) -> bool;
    fn resize_focused_gshell(&self, direction: PaneResizeDirection) -> bool;
    fn close_gshell(&self, gshell_id: GShellId) -> Option<WorkspaceGShellCloseOutcome>;
    fn visible_gshells(&self) -> Vec<GShellId>;
    fn workspace_render_layout(
        &self,
        window_size: TerminalWindowSize,
    ) -> Vec<RenderSurfacePlacement>;
    fn restore_workspace(&self) -> Result<(), WorkspaceServiceError>;
    fn persist_workspace(&self) -> Result<(), WorkspaceServiceError>;
}
