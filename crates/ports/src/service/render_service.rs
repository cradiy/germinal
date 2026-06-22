use std::sync::{Arc, atomic::AtomicBool, mpsc::Sender};

use crate::{
	pty_host::{size_info::TerminalSizeInfo, window_size::TerminalWindowSize},
	rendering::{render_target_id::RenderTargetId, surface_snapshot::RenderSurfaceSnapshot},
};

pub trait IRenderService {
	fn prepare_render_backend(&mut self);
	fn surface_snapshot_sender(&self) -> Sender<RenderSurfaceSnapshot>;
	fn snapshot_wake_pending(&self) -> Arc<AtomicBool>;
	fn consume_latest_terminal_snapshot(&mut self);
	fn current_terminal_size_info(&self) -> TerminalSizeInfo;
	fn resize_window_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo;
	fn set_window_focused(&mut self, focused: bool);
	fn set_focused_render_target(&mut self, target_id: RenderTargetId);
	fn request_redraw(&mut self);
	fn flush_redraw_request(&mut self);
	fn present_workspace(&mut self);
}
