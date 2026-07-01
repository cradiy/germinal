const ENTER_GNATIVE_PREFIX: &[u8] = b"\x1bPgerminal-gnative;";
const DCS_TERMINATOR: &[u8] = b"\x1b\\";

#[derive(Debug, Default)]
pub struct GNativeEnterControlSequenceDecoder {
	pending: Vec<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DecodeResult {
	pub visible_bytes: Vec<u8>,
	pub enter_gnative: bool,
}

impl GNativeEnterControlSequenceDecoder {
	pub fn decode(&mut self, bytes: &[u8]) -> DecodeResult {
		self.pending.extend_from_slice(bytes);

		let mut visible_bytes = Vec::new();
		let mut enter_gnative = false;
		let mut consumed_up_to = 0;
		let mut cursor = 0;

		while cursor < self.pending.len() {
			let Some(relative_escape_index) =
				self.pending[cursor..].iter().position(|byte| *byte == 0x1B)
			else {
				visible_bytes.extend_from_slice(&self.pending[cursor..]);
				consumed_up_to = self.pending.len();
				break;
			};

			let escape_index = cursor + relative_escape_index;
			visible_bytes.extend_from_slice(&self.pending[cursor..escape_index]);
			consumed_up_to = escape_index;

			let remaining = &self.pending[escape_index..];
			if ENTER_GNATIVE_PREFIX.starts_with(remaining) {
				break;
			}

			if !remaining.starts_with(ENTER_GNATIVE_PREFIX) {
				visible_bytes.push(0x1B);
				cursor = escape_index + 1;
				consumed_up_to = cursor;
				continue;
			}

			let payload_start = escape_index + ENTER_GNATIVE_PREFIX.len();
			let Some(terminator_index) = find_subslice(&self.pending[payload_start..], DCS_TERMINATOR)
			else {
				break;
			};

			let payload = &self.pending[payload_start..payload_start + terminator_index];
			enter_gnative = payload.is_empty();
			cursor = payload_start + terminator_index + DCS_TERMINATOR.len();
			consumed_up_to = cursor;
		}

		if consumed_up_to > 0 {
			self.pending.drain(..consumed_up_to);
		}

		DecodeResult { visible_bytes, enter_gnative }
	}
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
	if needle.is_empty() {
		return Some(0);
	}

	haystack.windows(needle.len()).position(|window| window == needle)
}

#[cfg(test)]
mod tests {
	use super::{DecodeResult, GNativeEnterControlSequenceDecoder};

	#[test]
	fn strips_enter_gnative_control_sequence() {
		let mut decoder = GNativeEnterControlSequenceDecoder::default();

		let result = decoder.decode(b"hello\x1bPgerminal-gnative;\x1b\\world");

		assert_eq!(result, DecodeResult { visible_bytes: b"helloworld".to_vec(), enter_gnative: true });
	}

	#[test]
	fn preserves_non_matching_escape_sequences() {
		let mut decoder = GNativeEnterControlSequenceDecoder::default();

		let result = decoder.decode(b"\x1b[31mred\x1b[0m");

		assert_eq!(result, DecodeResult {
			visible_bytes: b"\x1b[31mred\x1b[0m".to_vec(),
			enter_gnative: false,
		});
	}

	#[test]
	fn waits_for_a_split_terminator_before_switching_modes() {
		let mut decoder = GNativeEnterControlSequenceDecoder::default();

		let first = decoder.decode(b"prefix\x1bPgerminal-gnative;");
		assert_eq!(first, DecodeResult { visible_bytes: b"prefix".to_vec(), enter_gnative: false });

		let second = decoder.decode(b"\x1b\\suffix");
		assert_eq!(second, DecodeResult { visible_bytes: b"suffix".to_vec(), enter_gnative: true });
	}

	#[test]
	fn ignores_invalid_enter_gnative_payloads() {
		let mut decoder = GNativeEnterControlSequenceDecoder::default();

		let result = decoder.decode(b"before\x1bPgerminal-gnative;version=1\x1b\\after");

		assert_eq!(result, DecodeResult {
			visible_bytes: b"beforeafter".to_vec(),
			enter_gnative: false,
		});
	}
}
