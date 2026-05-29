use germinal_domain::{
	pty_host::terminal_size::TerminalPtySize, shared::seq::Seq, workspace::pane_id::PaneId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerTask {
	PtyBytes { pane_id: PaneId, bytes: Vec<u8>, seq: Seq },
	PtyResize { pane_id: PaneId, size: TerminalPtySize, seq: Seq },
	GNativeMessage { pane_id: PaneId, message: Vec<u8>, seq: Seq },
}
