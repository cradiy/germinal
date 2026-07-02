use std::cell::{Cell, RefCell};

use germinal_domain::{gshell::vo::gshell_id::GShellId, workspace::entity::workspace::Workspace};
use germinal_ports::{
	error::BoxResult, repository::IRepository, service::workspace_service::IWorkspaceService,
};
use thiserror::Error;

#[derive(kudi::DepInj)]
#[target(WorkspaceService)]
pub struct WorkspaceServiceState {
	persistence_workspace_id: Cell<Option<u64>>,
	workspace:                RefCell<Workspace>,
}

impl WorkspaceServiceState {
	pub fn new() -> Self {
		Self {
			persistence_workspace_id: Cell::new(None),
			workspace:                RefCell::new(Workspace::main()),
		}
	}

	pub fn focused_gshell(&self) -> GShellId { self.workspace.borrow().focused_gshell() }

	pub fn workspace(&self) -> Workspace { self.workspace.borrow().clone() }

	fn persistence_workspace_id(&self) -> Option<u64> { self.persistence_workspace_id.get() }

	fn bind_workspace(&self, persistence_id: u64, workspace: Workspace) {
		self.persistence_workspace_id.set(Some(persistence_id));
		*self.workspace.borrow_mut() = workspace;
	}
}

#[derive(Debug, Error)]
enum WorkspaceServiceError {
	#[error("workspace persistence id is not initialized")]
	PersistenceIdNotInitialized,
}

impl<Deps> IWorkspaceService for WorkspaceService<Deps>
where Deps: AsRef<WorkspaceServiceState> + IRepository<Id = u64, Aggregate = Workspace>
{
	fn focused_gshell(&self) -> GShellId {
		<Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).focused_gshell()
	}

	fn restore_workspace(&self) -> BoxResult<()> {
		let state = <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref());
		let repository = self.prj_ref();

		if let Some((persistence_id, workspace)) = repository.list()?.into_iter().next() {
			state.bind_workspace(persistence_id, workspace);
			return Ok(());
		}

		let workspace = Workspace::main();
		let persistence_id = repository.insert(workspace.clone())?;
		state.bind_workspace(persistence_id, workspace);
		Ok(())
	}

	fn persist_workspace(&self) -> BoxResult<()> {
		let state = <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref());
		let persistence_id = state.persistence_workspace_id().ok_or_else(|| {
			Box::<dyn std::error::Error + Send + Sync>::from(
				WorkspaceServiceError::PersistenceIdNotInitialized,
			)
		})?;

		self.prj_ref().update(persistence_id, state.workspace())
	}
}
