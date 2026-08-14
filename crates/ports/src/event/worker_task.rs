use germinal_domain::pty_host::pty_host_id::PtyHostId;

use crate::{pty_host::terminal_size::TerminalPtySize, seq::Seq};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerTask {
    PtyBytes {
        pty_host_id: PtyHostId,
        bytes: Vec<u8>,
        seq: Seq,
    },
    PtyResize {
        pty_host_id: PtyHostId,
        size: TerminalPtySize,
        seq: Seq,
    },
}
