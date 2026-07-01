use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::{
	input::GNativeInputEvent,
	session::{GNativeSessionAccepted, GNativeSessionDescriptor},
};

pub trait IGNativeTunnel {
	fn ensure_session_descriptor(
		&self,
		gshell_id: GShellId,
		protocol_version: u32,
	) -> Result<GNativeSessionDescriptor, String>;
	fn accept_session(&self, gshell_id: GShellId) -> Result<GNativeSessionAccepted, String>;
	fn send_input(&self, gshell_id: GShellId, input: GNativeInputEvent) -> Result<(), String>;
	fn close_session(&self, gshell_id: GShellId) -> Result<(), String>;
}

pub trait IGNativeTunnelProvider {
	type GNativeTunnel: IGNativeTunnel;

	fn gnative_tunnel(&self) -> &Self::GNativeTunnel;
}
