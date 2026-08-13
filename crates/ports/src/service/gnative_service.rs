use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::session::GNativeSessionAccepted;
use thiserror::Error;

use crate::{
	event::gshell_input::GShellInput,
	pty_host::size_info::TerminalSizeInfo,
	service::gnative_tunnel::GNativeTunnelError,
};

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
	fn begin_gnative_session(&self, gshell_id: GShellId) -> Result<(), GNativeServiceError>;
	fn activate_gnative_session(&self, accepted: GNativeSessionAccepted);
	fn fail_gnative_session(&self, gshell_id: GShellId);
	fn exit_gnative_session(&self, gshell_id: GShellId);
	fn route_gnative_input(&self, input: GShellInput);
	fn resize_gnative_session(&self, gshell_id: GShellId, size_info: TerminalSizeInfo);
}
