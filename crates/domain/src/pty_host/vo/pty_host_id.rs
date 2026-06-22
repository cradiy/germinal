use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PtyHostId(u64);

impl PtyHostId {
	pub const fn new(value: u64) -> Self { Self(value) }

	pub const fn value(self) -> u64 { self.0 }
}
