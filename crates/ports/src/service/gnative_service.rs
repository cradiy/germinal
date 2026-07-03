use germinal_domain::{gshell::vo::gshell_id::GShellId, pty_host::terminal_size::TerminalGridSize};
use thiserror::Error;

use crate::{event::gshell_input::GShellInput, service::gnative_tunnel::GNativeTunnelError};

#[derive(Debug, Error)]
pub enum GNativeServiceError {
	#[error("failed to accept gnative session for gshell {gshell_id}: {source}")]
	EnterSession {
		gshell_id: u64,
		#[source]
		source:    GNativeTunnelError,
	},
}

pub trait IGNativeService {
	fn ensure_gshell_gnative(&self, gshell_id: GShellId);
	fn enter_gnative_session(&self, gshell_id: GShellId) -> Result<(), GNativeServiceError>;
	fn exit_gnative_session(&self, gshell_id: GShellId);
	fn route_gnative_input(&self, input: GShellInput);
	fn resize_gnative_session(&self, gshell_id: GShellId, term_size: TerminalGridSize);
}
