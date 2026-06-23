use std::io::{self, Write};

const ENTER_GNATIVE_PREFIX: &str = "\u{1b}Pgerminal-gnative;";
const DCS_TERMINATOR: &str = "\u{1b}\\";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GNativeLaunchDescriptor {
	pub endpoint:         String,
	pub token:            String,
	pub protocol_version: u32,
}

impl GNativeLaunchDescriptor {
	pub fn enter_control_sequence(&self) -> String {
		format!(
			"{ENTER_GNATIVE_PREFIX}version={};endpoint={};token={}{}",
			self.protocol_version, self.endpoint, self.token, DCS_TERMINATOR
		)
	}
}

pub fn write_enter_control_sequence<W: Write>(
	writer: &mut W,
	descriptor: &GNativeLaunchDescriptor,
) -> io::Result<()> {
	writer.write_all(descriptor.enter_control_sequence().as_bytes())?;
	writer.flush()
}

#[cfg(test)]
mod tests {
	use super::{GNativeLaunchDescriptor, write_enter_control_sequence};

	#[test]
	fn encodes_enter_control_sequence() {
		let descriptor = GNativeLaunchDescriptor {
			endpoint:         "/tmp/germinal.sock".to_string(),
			token:            "secret".to_string(),
			protocol_version: 1,
		};

		assert_eq!(
			descriptor.enter_control_sequence(),
			"\u{1b}Pgerminal-gnative;version=1;endpoint=/tmp/germinal.sock;token=secret\u{1b}\\"
		);
	}

	#[test]
	fn writes_enter_control_sequence_to_writer() {
		let descriptor = GNativeLaunchDescriptor {
			endpoint:         "test".to_string(),
			token:            "secret".to_string(),
			protocol_version: 7,
		};
		let mut buffer = Vec::new();

		write_enter_control_sequence(&mut buffer, &descriptor).expect("writer should accept sequence");

		assert_eq!(
			String::from_utf8(buffer).expect("sequence should be utf8"),
			"\u{1b}Pgerminal-gnative;version=7;endpoint=test;token=secret\u{1b}\\"
		);
	}
}
