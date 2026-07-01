use germinal_domain::gshell::vo::gshell_id::GShellId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeSessionDescriptor {
	pub gshell_id:        GShellId,
	pub endpoint:         String,
	pub token:            String,
	pub protocol_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeAppHello {
	pub token:            String,
	pub protocol_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeSessionAccepted {
	pub gshell_id:        GShellId,
	pub protocol_version: u32,
}
