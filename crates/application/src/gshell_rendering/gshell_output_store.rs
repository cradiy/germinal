use std::collections::HashMap;

use germinal_domain::{
	gshell::gshell_id::GShellId, rendering::render_target_id::RenderTargetId, shared::seq::Seq,
};
use germinal_ports::pty_host::output_applier::TerminalApplyResult;

#[derive(Debug, Default)]
pub struct GShellOutputStore {
	states: HashMap<GShellId, GShellOutputState>,
}

impl GShellOutputStore {
	pub fn new() -> Self { Self::default() }

	pub fn record_apply_result(&mut self, result: &TerminalApplyResult) {
		let state = self.states.entry(result.gshell_id).or_default();

		state.latest_seq = result.latest_seq;
		state.render_target_id = result.render_target_id;
		state.total_bytes += result.bytes_applied as u64;
		state.chunk_count += 1;
		state.changed |= result.changed;
	}

	pub fn state_of(&self, gshell_id: GShellId) -> Option<&GShellOutputState> {
		self.states.get(&gshell_id)
	}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GShellOutputState {
	pub latest_seq:       Seq,
	pub render_target_id: RenderTargetId,
	pub total_bytes:      u64,
	pub chunk_count:      u64,
	pub changed:          bool,
}

impl Default for GShellOutputState {
	fn default() -> Self {
		Self {
			latest_seq:       Seq::ZERO,
			render_target_id: RenderTargetId::new(0),
			total_bytes:      0,
			chunk_count:      0,
			changed:          false,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn record_apply_result_updates_state() {
		let mut store = GShellOutputStore::new();
		let gshell_id = GShellId::new(1);
		let render_target_id = RenderTargetId::new(10);

		store.record_apply_result(&TerminalApplyResult {
			gshell_id,
			render_target_id,
			latest_seq: Seq::new(1),
			bytes_applied: 6,
			changed: true,
		});

		store.record_apply_result(&TerminalApplyResult {
			gshell_id,
			render_target_id,
			latest_seq: Seq::new(2),
			bytes_applied: 6,
			changed: true,
		});

		let state = store.state_of(gshell_id).unwrap();

		assert_eq!(state.latest_seq, Seq::new(2));
		assert_eq!(state.render_target_id, render_target_id);
		assert_eq!(state.total_bytes, 12);
		assert_eq!(state.chunk_count, 2);
		assert!(state.changed);
	}
}
