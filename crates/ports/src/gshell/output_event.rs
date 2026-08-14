use germinal_domain::gshell::gshell_id::GShellId;

use crate::seq::Seq;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GShellOutputEvent {
    pub gshell_id: GShellId,
    pub seq: Seq,
    pub bytes: Vec<u8>,
}

impl GShellOutputEvent {
    pub fn new(gshell_id: GShellId, seq: Seq, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            gshell_id,
            seq,
            bytes: bytes.into(),
        }
    }
}
