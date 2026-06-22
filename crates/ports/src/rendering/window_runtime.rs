use germinal_domain::pty_host::{size_info::TerminalSizeInfo, window_size::TerminalWindowSize};

use crate::rendering::surface_snapshot::RenderSurfaceSnapshot;

pub trait ITerminalWindowRuntime {
	fn request_window_redraw(&self);
	fn set_surface_snapshot(&mut self, snapshot: RenderSurfaceSnapshot);
	fn resize_surface_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo;
	fn take_redraw_request(&mut self) -> bool;
	fn terminal_size_info(&self) -> TerminalSizeInfo;
	fn render(&mut self);
}

pub trait IRenderRuntimeStore {
	type WindowRuntime: ITerminalWindowRuntime;

	fn window_runtime(&self) -> Option<&Self::WindowRuntime>;
	fn window_runtime_mut(&mut self) -> Option<&mut Self::WindowRuntime>;
	fn set_window_runtime(&mut self, runtime: Self::WindowRuntime);
}
