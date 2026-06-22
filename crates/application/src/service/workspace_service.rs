use std::cell::Cell;

use germinal_domain::{gshell::vo::gshell_id::GShellId, workspace::entity::workspace::Workspace};
use germinal_ports::{
	event::runtime_event_dispatcher::RuntimeEventDispatcher,
	repository::IRepository,
	service::workspace_service::{
		IWorkspacePersistenceRepositoryProvider, IWorkspaceRuntimeRepositoryProvider, IWorkspaceService,
	},
};

#[derive(kudi::DepInj)]
#[target(WorkspaceService)]
pub struct WorkspaceServiceState {
	runtime_event_proxy:      RuntimeEventDispatcher,
	runtime_workspace_id:     Cell<Option<u64>>,
	persistence_workspace_id: Cell<Option<u64>>,
}

impl WorkspaceServiceState {
	pub fn new(runtime_event_proxy: RuntimeEventDispatcher) -> Self {
		Self {
			runtime_event_proxy,
			runtime_workspace_id: Cell::new(None),
			persistence_workspace_id: Cell::new(None),
		}
	}

	pub fn runtime_event_proxy(&self) -> RuntimeEventDispatcher { self.runtime_event_proxy.clone() }

	fn runtime_workspace_id(&self) -> Option<u64> { self.runtime_workspace_id.get() }

	fn persistence_workspace_id(&self) -> Option<u64> { self.persistence_workspace_id.get() }

	fn bind_workspace_ids(&self, runtime_id: u64, persistence_id: u64) {
		self.runtime_workspace_id.set(Some(runtime_id));
		self.persistence_workspace_id.set(Some(persistence_id));
	}
}

impl<Deps> IWorkspaceService for WorkspaceService<Deps>
where Deps: AsRef<WorkspaceServiceState>
		+ IWorkspaceRuntimeRepositoryProvider
		+ IWorkspacePersistenceRepositoryProvider
{
	fn focused_gshell(&self) -> GShellId {
		let state: &WorkspaceServiceState = self.prj_ref().as_ref();
		let Some(runtime_id) = state.runtime_workspace_id() else {
			return Workspace::main().focused_gshell();
		};

		self
			.prj_ref()
			.workspace_runtime_repository()
			.get(runtime_id)
			.ok()
			.flatten()
			.unwrap_or_default()
			.focused_gshell()
	}

	fn runtime_event_proxy(&self) -> RuntimeEventDispatcher {
		self.prj_ref().as_ref().runtime_event_proxy()
	}

	fn restore_workspace(&self) -> Result<(), String> {
		let state: &WorkspaceServiceState = self.prj_ref().as_ref();
		let runtime_repository = self.prj_ref().workspace_runtime_repository();
		let persistence_repository = self.prj_ref().workspace_persistence_repository();

		let persistence_id = if let Some((persistence_id, workspace)) =
			persistence_repository.list()?.into_iter().next()
		{
			let runtime_id = runtime_repository.insert(workspace)?;
			state.bind_workspace_ids(runtime_id, persistence_id);
			return Ok(());
		} else {
			persistence_repository.insert(Workspace::main())?
		};

		let workspace = persistence_repository
			.get(persistence_id)?
			.ok_or_else(|| "persisted workspace disappeared after insert".to_string())?;
		let runtime_id = runtime_repository.insert(workspace)?;
		state.bind_workspace_ids(runtime_id, persistence_id);
		Ok(())
	}

	fn persist_workspace(&self) -> Result<(), String> {
		let state: &WorkspaceServiceState = self.prj_ref().as_ref();
		let runtime_repository = self.prj_ref().workspace_runtime_repository();
		let persistence_repository = self.prj_ref().workspace_persistence_repository();

		let runtime_id = state
			.runtime_workspace_id()
			.ok_or_else(|| "workspace runtime id is not initialized".to_string())?;
		let persistence_id = state
			.persistence_workspace_id()
			.ok_or_else(|| "workspace persistence id is not initialized".to_string())?;

		let workspace = runtime_repository.get(runtime_id)?.unwrap_or_else(Workspace::main);
		persistence_repository.update(persistence_id, workspace)
	}
}
