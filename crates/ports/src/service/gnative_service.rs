use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};

use crate::{event::gshell_input::GShellInput, gnative::session::GNativeSessionDescriptor};

pub trait IGNativeService {
	fn ensure_gshell_gnative(&self, gshell_id: GShellId);
	fn enter_gnative_session(&self, descriptor: GNativeSessionDescriptor) -> Result<(), String>;
	fn exit_gnative_session(&self, gshell_id: GShellId);
	fn route_gnative_input(&self, input: GShellInput);
	fn resize_gnative_session(&self, gshell_id: GShellId, term_size: TerminalGridSize);
}
