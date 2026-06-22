use serde::{Deserialize, Serialize};

use crate::{aggregate_root::AggregateRoot, pty_host::terminal_size::TerminalGridSize};

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyHost {
	grid_size: TerminalGridSize,
}

impl PtyHost {
	pub const fn new(grid_size: TerminalGridSize) -> Self { Self { grid_size } }

	pub const fn grid_size(&self) -> TerminalGridSize { self.grid_size }

	pub const fn rows(&self) -> usize { self.grid_size.rows() }

	pub const fn columns(&self) -> usize { self.grid_size.columns() }

	pub fn resize(&mut self, grid_size: TerminalGridSize) -> bool {
		if self.grid_size == grid_size {
			return false;
		}

		self.grid_size = grid_size;
		true
	}
}

impl AggregateRoot for PtyHost {}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn resize_updates_host_sizes() {
		let mut pty_host = PtyHost::new(TerminalGridSize::new(80, 24));

		assert!(pty_host.resize(TerminalGridSize::new(100, 30)));

		assert_eq!(pty_host.grid_size(), TerminalGridSize::new(100, 30));
		assert_eq!(pty_host.columns(), 100);
		assert_eq!(pty_host.rows(), 30);
	}

	#[test]
	fn resize_ignores_same_grid_size() {
		let mut pty_host = PtyHost::new(TerminalGridSize::new(80, 24));

		assert!(!pty_host.resize(TerminalGridSize::new(80, 24)));
	}
}
