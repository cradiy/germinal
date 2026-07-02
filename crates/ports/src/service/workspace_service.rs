use germinal_domain::gshell::vo::gshell_id::GShellId;

use crate::error::BoxResult;

pub trait IWorkspaceService {
	fn focused_gshell(&self) -> GShellId;
	fn restore_workspace(&self) -> BoxResult<()>;
	fn persist_workspace(&self) -> BoxResult<()>;
}
