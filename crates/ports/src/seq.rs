use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Seq(u64);

impl Seq {
	pub const ZERO: Self = Self(0);

	pub const fn new(value: u64) -> Self { Self(value) }

	pub const fn value(self) -> u64 { self.0 }

	pub const fn next(self) -> Self { Self(self.0 + 1) }
}

impl Default for Seq {
	fn default() -> Self { Self::ZERO }
}
