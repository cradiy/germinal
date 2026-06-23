use crate::{
	aggregate_root::AggregateRoot, gshell::entity::gshell_mode::GShellMode,
	pty_host::vo::pty_host_id::PtyHostId,
};

#[derive(Debug, PartialEq, Eq)]
pub struct GShell {
	pty_host_id: PtyHostId,
	mode:        GShellMode,
}

impl GShell {
	pub fn new(pty_host_id: PtyHostId) -> Self { Self { pty_host_id, mode: GShellMode::Pty } }

	pub fn pty_host_id(&self) -> PtyHostId { self.pty_host_id }

	pub fn mode(&self) -> GShellMode { self.mode }

	pub fn enter_gnative(&mut self) { self.mode = GShellMode::GNative; }

	pub fn exit_gnative(&mut self) { self.mode = GShellMode::Pty; }
}

impl Default for GShell {
	fn default() -> Self { Self::new(PtyHostId::new(0)) }
}

impl AggregateRoot for GShell {}
