use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::{
	input::GNativeInputEvent,
	session::{GNativeSessionAccepted, GNativeSessionDescriptor},
};

use crate::error::BoxResult;

pub trait IGNativeTunnel {
	fn ensure_session_descriptor(
		&self,
		gshell_id: GShellId,
		protocol_version: u32,
	) -> BoxResult<GNativeSessionDescriptor>;
	fn accept_session(&self, gshell_id: GShellId) -> BoxResult<GNativeSessionAccepted>;
	fn send_input(&self, gshell_id: GShellId, input: GNativeInputEvent) -> BoxResult<()>;
	fn close_session(&self, gshell_id: GShellId) -> BoxResult<()>;
}

pub trait IGNativeTunnelProvider {
	type GNativeTunnel: IGNativeTunnel;

	fn gnative_tunnel(&self) -> &Self::GNativeTunnel;
}
