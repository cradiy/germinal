use germinal_domain::gshell::gshell_id::GShellId;

use crate::{
    gshell::output_event::GShellOutputEvent, rendering::render_target_id::RenderTargetId, seq::Seq,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalApplyResult {
    pub gshell_id: GShellId,
    pub render_target_id: RenderTargetId,
    pub latest_seq: Seq,
    pub bytes_applied: usize,
    pub changed: bool,
}

pub trait TerminalOutputApplier {
    fn apply(
        &mut self,
        render_target_id: RenderTargetId,
        event: &GShellOutputEvent,
    ) -> TerminalApplyResult;
}
