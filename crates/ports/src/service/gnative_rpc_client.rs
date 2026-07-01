use germinal_domain::gshell::vo::gshell_id::GShellId;
use germinal_gnative_protocol::gnative::{
	input::GNativeInputEvent,
	session::{GNativeSessionAccepted, GNativeSessionDescriptor},
};

pub trait IGNativeRpcClient {
	fn connect_and_handshake(
		&self,
		descriptor: &GNativeSessionDescriptor,
	) -> Result<GNativeSessionAccepted, String>;
	fn send_input(&self, gshell_id: GShellId, input: GNativeInputEvent) -> Result<(), String>;
	fn close_session(&self, gshell_id: GShellId) -> Result<(), String>;
}

pub trait IGNativeRpcClientProvider {
	type GNativeRpcClient: IGNativeRpcClient;

	fn gnative_rpc_client(&self) -> &Self::GNativeRpcClient;
}
