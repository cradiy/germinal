use std::{
    sync::{Arc, atomic::AtomicBool, mpsc::Sender},
    time::Duration,
};

use crate::{
    pty_host::{size_info::TerminalSizeInfo, window_size::TerminalWindowSize},
    rendering::{
        render_target_id::RenderTargetId, surface_snapshot::RenderSurfaceImePreeditSnapshot,
        surface_snapshot::RenderSurfaceSnapshot, tab_bar::TabBarSnapshot,
        workspace_layout::RenderSurfacePlacement,
    },
};

pub trait IRenderService {
    fn prepare_render_backend(&mut self);
    fn surface_snapshot_sender(&self) -> Sender<RenderSurfaceSnapshot>;
    fn snapshot_wake_pending(&self) -> Arc<AtomicBool>;
    fn consume_latest_terminal_snapshot(&mut self);
    fn current_terminal_size_info(&self) -> TerminalSizeInfo;
    fn terminal_size_info_for_surface(&self, placement: RenderSurfacePlacement)
    -> TerminalSizeInfo;
    fn set_workspace_render_layout(&mut self, placements: Vec<RenderSurfacePlacement>);
    fn set_tab_bar(&mut self, tab_bar: Option<TabBarSnapshot>);
    fn set_window_title(&mut self, title: &str);
    fn ring_bell(&mut self, visual_duration: Duration, request_attention: bool);
    fn resize_window_size_info(&mut self, window_size: TerminalWindowSize) -> TerminalSizeInfo;
    fn set_window_focused(&mut self, focused: bool);
    fn set_focused_render_target(&mut self, target_id: RenderTargetId);
    fn set_ime_preedit(
        &mut self,
        target_id: RenderTargetId,
        preedit: Option<RenderSurfaceImePreeditSnapshot>,
    );
    fn remove_render_target(&mut self, target_id: RenderTargetId);
    fn request_redraw(&mut self);
    fn flush_redraw_request(&mut self);
    fn present_workspace(&mut self);
}
