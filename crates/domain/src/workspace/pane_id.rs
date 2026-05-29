#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(u64);

impl PaneId {
	pub const fn new(value: u64) -> Self { Self(value) }

	pub const fn value(self) -> u64 { self.0 }
}
