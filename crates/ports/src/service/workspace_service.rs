use germinal_domain::gshell::vo::gshell_id::GShellId;

pub trait IWorkspaceService {
	fn focused_gshell(&self) -> GShellId;
	fn restore_workspace(&self) -> Result<(), String>;
	fn persist_workspace(&self) -> Result<(), String>;
}
