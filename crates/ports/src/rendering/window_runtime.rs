use crate::{
    pty_host::{size_info::TerminalSizeInfo, window_size::TerminalWindowSize},
    rendering::{
        render_target_id::RenderTargetId, surface_snapshot::RenderSurfaceSnapshot,
        tab_bar::TabBarSnapshot, workspace_layout::RenderSurfacePlacement,
    },
};

pub trait ITerminalWindowRuntime {
    fn request_window_redraw(&self);
    fn set_surface_snapshot(&mut self, snapshot: RenderSurfaceSnapshot);
    fn remove_render_target(&mut self, target_id: RenderTargetId);
    fn surface_snapshots_mut(&mut self) -> Vec<&mut RenderSurfaceSnapshot>;
    fn set_workspace_layout(&mut self, placements: Vec<RenderSurfacePlacement>);
    fn set_tab_bar(&mut self, tab_bar: Option<TabBarSnapshot>);
    fn set_window_title(&mut self, title: &str);
    fn resize_surface_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo;
    fn terminal_size_info_for_window_size(
        &self,
        window_size: TerminalWindowSize,
    ) -> TerminalSizeInfo;
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
