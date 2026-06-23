use std::cell::{Cell, RefCell};

use germinal_domain::{gshell::vo::gshell_id::GShellId, workspace::entity::workspace::Workspace};
use germinal_ports::{
	event::runtime_event_dispatcher::RuntimeEventDispatcher, repository::IRepository,
	service::workspace_service::IWorkspaceService,
};

#[derive(kudi::DepInj)]
#[target(WorkspaceService)]
pub struct WorkspaceServiceState {
	runtime_event_proxy:      RuntimeEventDispatcher,
	persistence_workspace_id: Cell<Option<u64>>,
	workspace:                RefCell<Workspace>,
}

impl WorkspaceServiceState {
	pub fn new(runtime_event_proxy: RuntimeEventDispatcher) -> Self {
		Self {
			runtime_event_proxy,
			persistence_workspace_id: Cell::new(None),
			workspace: RefCell::new(Workspace::main()),
		}
	}

	pub fn runtime_event_proxy(&self) -> RuntimeEventDispatcher { self.runtime_event_proxy.clone() }

	pub fn focused_gshell(&self) -> GShellId { self.workspace.borrow().focused_gshell() }

	pub fn workspace(&self) -> Workspace { self.workspace.borrow().clone() }

	fn persistence_workspace_id(&self) -> Option<u64> { self.persistence_workspace_id.get() }

	fn bind_workspace(&self, persistence_id: u64, workspace: Workspace) {
		self.persistence_workspace_id.set(Some(persistence_id));
		*self.workspace.borrow_mut() = workspace;
	}
}

impl<Deps> IWorkspaceService for WorkspaceService<Deps>
where Deps: AsRef<WorkspaceServiceState> + IRepository<Id = u64, Aggregate = Workspace>
{
	fn focused_gshell(&self) -> GShellId {
		<Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).focused_gshell()
	}

	fn runtime_event_proxy(&self) -> RuntimeEventDispatcher {
		<Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref()).runtime_event_proxy()
	}

	fn restore_workspace(&self) -> Result<(), String> {
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

	fn persist_workspace(&self) -> Result<(), String> {
		let state = <Deps as AsRef<WorkspaceServiceState>>::as_ref(self.prj_ref());
		let persistence_id = state
			.persistence_workspace_id()
			.ok_or_else(|| "workspace persistence id is not initialized".to_string())?;

		self.prj_ref().update(persistence_id, state.workspace())
	}
}
