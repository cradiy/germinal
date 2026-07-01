use std::io::{self, Write};

use germinal_gnative_protocol::gnative::session::{
	GNATIVE_TUNNEL_ENDPOINT_ENV, GNATIVE_TUNNEL_PROTOCOL_VERSION_ENV, GNATIVE_TUNNEL_TOKEN_ENV,
};

const ENTER_GNATIVE_PREFIX: &str = "\u{1b}Pgerminal-gnative;";
const DCS_TERMINATOR: &str = "\u{1b}\\";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GNativeTunnelEnv {
	pub endpoint:         String,
	pub token:            String,
	pub protocol_version: u32,
}

impl GNativeTunnelEnv {
	pub fn from_env() -> Result<Self, String> {
		let endpoint = std::env::var(GNATIVE_TUNNEL_ENDPOINT_ENV)
			.map_err(|_| format!("missing {GNATIVE_TUNNEL_ENDPOINT_ENV}"))?;
		let token = std::env::var(GNATIVE_TUNNEL_TOKEN_ENV)
			.map_err(|_| format!("missing {GNATIVE_TUNNEL_TOKEN_ENV}"))?;
		let protocol_version = std::env::var(GNATIVE_TUNNEL_PROTOCOL_VERSION_ENV)
			.map_err(|_| format!("missing {GNATIVE_TUNNEL_PROTOCOL_VERSION_ENV}"))?
			.parse::<u32>()
			.map_err(|error| error.to_string())?;

		Ok(Self { endpoint, token, protocol_version })
	}

	pub fn enter_control_sequence(&self) -> String {
		format!("{ENTER_GNATIVE_PREFIX}{DCS_TERMINATOR}")
	}
}

pub fn write_enter_control_sequence<W: Write>(
	writer: &mut W,
	tunnel_env: &GNativeTunnelEnv,
) -> io::Result<()> {
	writer.write_all(tunnel_env.enter_control_sequence().as_bytes())?;
	writer.flush()
}

#[cfg(test)]
mod tests {
	use super::{GNativeTunnelEnv, write_enter_control_sequence};

	#[test]
	fn encodes_enter_control_sequence() {
		let tunnel_env = GNativeTunnelEnv {
			endpoint:         "/tmp/germinal.sock".to_string(),
			token:            "secret".to_string(),
			protocol_version: 1,
		};

		assert_eq!(tunnel_env.enter_control_sequence(), "\u{1b}Pgerminal-gnative;\u{1b}\\");
	}

	#[test]
	fn writes_enter_control_sequence_to_writer() {
		let tunnel_env = GNativeTunnelEnv {
			endpoint:         "test".to_string(),
			token:            "secret".to_string(),
			protocol_version: 7,
		};
		let mut buffer = Vec::new();

		write_enter_control_sequence(&mut buffer, &tunnel_env).expect("writer should accept sequence");

		assert_eq!(
			String::from_utf8(buffer).expect("sequence should be utf8"),
			"\u{1b}Pgerminal-gnative;\u{1b}\\"
		);
	}
}
