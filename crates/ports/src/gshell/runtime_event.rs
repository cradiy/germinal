use germinal_domain::gshell::gshell_id::GShellId;

use crate::gshell::output_event::GShellOutputEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GShellRuntimeEvent {
	Output(GShellOutputEvent),
	Exited { gshell_id: GShellId, exit_code: Option<i32> },
}

impl GShellRuntimeEvent {
	pub fn output(event: GShellOutputEvent) -> Self { Self::Output(event) }

	pub fn exited(gshell_id: GShellId, exit_code: Option<i32>) -> Self {
		Self::Exited { gshell_id, exit_code }
	}
}
