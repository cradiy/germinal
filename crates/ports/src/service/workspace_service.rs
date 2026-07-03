use germinal_domain::gshell::vo::gshell_id::GShellId;
use thiserror::Error;

use crate::repository::RepositoryError;

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
	fn restore_workspace(&self) -> Result<(), WorkspaceServiceError>;
	fn persist_workspace(&self) -> Result<(), WorkspaceServiceError>;
}
