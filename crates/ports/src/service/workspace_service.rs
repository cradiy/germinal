use germinal_domain::gshell::vo::gshell_id::GShellId;

use crate::event::runtime_event_dispatcher::RuntimeEventDispatcher;

pub trait IWorkspaceService {
	fn focused_gshell(&self) -> GShellId;
	fn runtime_event_proxy(&self) -> RuntimeEventDispatcher;
	fn restore_workspace(&self) -> Result<(), String>;
	fn persist_workspace(&self) -> Result<(), String>;
}
