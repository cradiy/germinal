use germinal_domain::gshell::vo::gshell_id::GShellId;
use serde::{Deserialize, Serialize};

pub const GNATIVE_TUNNEL_ENDPOINT_ENV: &str = "GERMINAL_GNATIVE_TUNNEL_ENDPOINT";
pub const GNATIVE_TUNNEL_TOKEN_ENV: &str = "GERMINAL_GNATIVE_TUNNEL_TOKEN";
pub const GNATIVE_TUNNEL_PROTOCOL_VERSION_ENV: &str = "GERMINAL_GNATIVE_TUNNEL_PROTOCOL_VERSION";
pub const GNATIVE_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GNativeSessionDescriptor {
	pub gshell_id:        GShellId,
	pub endpoint:         String,
	pub token:            String,
	pub protocol_version: u32,
}

impl GNativeSessionDescriptor {
	pub fn tunnel_env(&self) -> Vec<(String, String)> {
		vec![
			(GNATIVE_TUNNEL_ENDPOINT_ENV.to_string(), self.endpoint.clone()),
			(GNATIVE_TUNNEL_TOKEN_ENV.to_string(), self.token.clone()),
			(GNATIVE_TUNNEL_PROTOCOL_VERSION_ENV.to_string(), self.protocol_version.to_string()),
		]
	}
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
