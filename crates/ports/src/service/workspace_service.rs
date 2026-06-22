use germinal_domain::{gshell::vo::gshell_id::GShellId, workspace::entity::workspace::Workspace};

use crate::{event::runtime_event_dispatcher::RuntimeEventDispatcher, repository::IRepository};

pub trait IWorkspaceRuntimeRepositoryProvider {
	type WorkspaceRuntimeRepository: IRepository<Id = u64, Aggregate = Workspace>;

	fn workspace_runtime_repository(&self) -> &Self::WorkspaceRuntimeRepository;
}

pub trait IWorkspacePersistenceRepositoryProvider {
	type WorkspacePersistenceRepository: IRepository<Id = u64, Aggregate = Workspace>;

	fn workspace_persistence_repository(&self) -> &Self::WorkspacePersistenceRepository;
}

pub trait IWorkspaceService {
	fn focused_gshell(&self) -> GShellId;
	fn runtime_event_proxy(&self) -> RuntimeEventDispatcher;
	fn restore_workspace(&self) -> Result<(), String>;
	fn persist_workspace(&self) -> Result<(), String>;
}
