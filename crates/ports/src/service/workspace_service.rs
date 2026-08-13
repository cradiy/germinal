use germinal_domain::gshell::vo::gshell_id::GShellId;
use thiserror::Error;

use crate::{
	pty_host::window_size::TerminalWindowSize,
	rendering::workspace_layout::RenderSurfacePlacement,
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

pub trait IWorkspaceService {
	fn focused_gshell(&self) -> GShellId;
	fn focus_gshell(&self, gshell_id: GShellId) -> bool;
	fn focus_next_gshell(&self) -> GShellId;
	fn visible_gshells(&self) -> Vec<GShellId>;
	fn workspace_render_layout(
		&self,
		window_size: TerminalWindowSize,
	) -> Vec<RenderSurfacePlacement>;
	fn restore_workspace(&self) -> Result<(), WorkspaceServiceError>;
	fn persist_workspace(&self) -> Result<(), WorkspaceServiceError>;
}
