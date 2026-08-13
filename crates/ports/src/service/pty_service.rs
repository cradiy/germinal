use std::sync::{Arc, atomic::AtomicBool, mpsc::Sender};

use germinal_domain::{
	gshell::vo::gshell_id::GShellId,
	pty_host::{pty_host_id::PtyHostId, terminal_size::TerminalGridSize},
};

use crate::{
	event::gshell_input::GShellInputEvent, pty_host::terminal_size::TerminalPtySize,
	rendering::surface_snapshot::RenderSurfaceSnapshot,
};

pub trait IPtyService {
	fn ensure_gshell_pty(
		&self,
		gshell_id: GShellId,
		pty_host_id: PtyHostId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
		surface_snapshot_tx: Sender<RenderSurfaceSnapshot>,
		snapshot_wake_pending: Arc<AtomicBool>,
	);
	fn send_pty_host_input(&self, pty_host_id: PtyHostId, event: GShellInputEvent);
	fn remove_pty_host(&self, pty_host_id: PtyHostId);
	fn resize_pty_host(
		&self,
		pty_host_id: PtyHostId,
		pty_size: TerminalPtySize,
		term_size: TerminalGridSize,
	);
}
