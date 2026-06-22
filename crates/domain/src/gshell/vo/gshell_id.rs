use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GShellId(u64);

impl GShellId {
	pub const fn new(value: u64) -> Self { Self(value) }

	pub const fn value(self) -> u64 { self.0 }
}
