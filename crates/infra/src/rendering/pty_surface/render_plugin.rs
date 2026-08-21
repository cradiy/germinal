use std::{
    error::Error,
    panic::{AssertUnwindSafe, catch_unwind},
    time::Duration,
};

use germinal_ports::{
    event::window_input_event::WindowInputEvent,
    rendering::{render_target_id::RenderTargetId, workspace_layout::RenderSurfacePlacement},
};
use tracing::warn;

pub type WgpuPaneRenderError = Box<dyn Error + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WgpuPaneRenderResult {
    pub request_redraw: bool,
}

impl WgpuPaneRenderResult {
    pub const fn idle() -> Self {
        Self {
            request_redraw: false,
        }
    }

    pub const fn redraw() -> Self {
        Self {
            request_redraw: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WgpuPaneInputResult {
    pub request_redraw: bool,
}

impl WgpuPaneInputResult {
    pub const fn handled() -> Self {
        Self {
            request_redraw: false,
        }
    }

    pub const fn redraw() -> Self {
        Self {
            request_redraw: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuPaneResizeEvent {
    pub placement: RenderSurfacePlacement,
    pub scale_factor: f64,
}

pub struct WgpuPaneRenderContext<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub command_encoder: &'a mut wgpu::CommandEncoder,
    pub target_view: &'a wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    pub placement: RenderSurfacePlacement,
    pub scale_factor: f64,
    pub elapsed: Duration,
}

impl WgpuPaneRenderContext<'_> {
    pub fn begin_render_pass<'pass>(
        &'pass mut self,
        label: Option<&'pass str>,
    ) -> wgpu::RenderPass<'pass> {
        let color_attachments = [Some(wgpu::RenderPassColorAttachment {
            view: self.target_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut render_pass = self
            .command_encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label,
                color_attachments: &color_attachments,
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        render_pass.set_viewport(
            self.placement.x_px as f32,
            self.placement.y_px as f32,
            self.placement.width_px as f32,
            self.placement.height_px as f32,
            0.0,
            1.0,
        );
        render_pass.set_scissor_rect(
            self.placement.x_px,
            self.placement.y_px,
            self.placement.width_px,
            self.placement.height_px,
        );
        render_pass
    }
}

pub trait WgpuPaneRenderer: Send + 'static {
    fn render(
        &mut self,
        context: WgpuPaneRenderContext<'_>,
    ) -> Result<WgpuPaneRenderResult, WgpuPaneRenderError>;

    fn input(&mut self, _event: &WindowInputEvent) -> WgpuPaneInputResult {
        WgpuPaneInputResult::handled()
    }

    fn resize(&mut self, _event: WgpuPaneResizeEvent) -> WgpuPaneInputResult {
        WgpuPaneInputResult::redraw()
    }
}

pub struct WgpuPaneRenderPlugin {
    target_id: RenderTargetId,
    renderer: Box<dyn WgpuPaneRenderer>,
    enabled: bool,
}

impl WgpuPaneRenderPlugin {
    pub fn new(target_id: RenderTargetId, renderer: impl WgpuPaneRenderer) -> Self {
        Self {
            target_id,
            renderer: Box::new(renderer),
            enabled: true,
        }
    }

    pub const fn target_id(&self) -> RenderTargetId {
        self.target_id
    }

    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn render(
        &mut self,
        context: WgpuPaneRenderContext<'_>,
    ) -> WgpuPanePluginFrameResult {
        if !self.enabled {
            return WgpuPanePluginFrameResult::default();
        }

        match catch_unwind(AssertUnwindSafe(|| self.renderer.render(context))) {
            Ok(Ok(result)) => WgpuPanePluginFrameResult {
                rendered: true,
                request_redraw: result.request_redraw,
            },
            Ok(Err(error)) => {
                warn!(
                    target_id = self.target_id.value(),
                    error = %error,
                    "disabled wgpu pane plugin after render error"
                );
                self.enabled = false;
                WgpuPanePluginFrameResult::default()
            }
            Err(_) => {
                warn!(
                    target_id = self.target_id.value(),
                    "disabled wgpu pane plugin after render panic"
                );
                self.enabled = false;
                WgpuPanePluginFrameResult::default()
            }
        }
    }

    pub(crate) fn input(&mut self, event: &WindowInputEvent) -> WgpuPaneInputResult {
        if !self.enabled {
            return WgpuPaneInputResult::handled();
        }

        match catch_unwind(AssertUnwindSafe(|| self.renderer.input(event))) {
            Ok(result) => result,
            Err(_) => {
                warn!(
                    target_id = self.target_id.value(),
                    "disabled wgpu pane plugin after input panic"
                );
                self.enabled = false;
                WgpuPaneInputResult::handled()
            }
        }
    }

    pub(crate) fn resize(&mut self, event: WgpuPaneResizeEvent) -> WgpuPaneInputResult {
        if !self.enabled {
            return WgpuPaneInputResult::handled();
        }

        match catch_unwind(AssertUnwindSafe(|| self.renderer.resize(event))) {
            Ok(result) => result,
            Err(_) => {
                warn!(
                    target_id = self.target_id.value(),
                    "disabled wgpu pane plugin after resize panic"
                );
                self.enabled = false;
                WgpuPaneInputResult::handled()
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WgpuPanePluginFrameResult {
    pub rendered: bool,
    pub request_redraw: bool,
}

#[cfg(test)]
mod tests {
    use germinal_ports::{
        event::window_input_event::{WindowInputEvent, WindowInputModifiers},
        rendering::render_target_id::RenderTargetId,
    };

    use super::*;

    struct InputRenderer {
        input_count: usize,
    }

    impl WgpuPaneRenderer for InputRenderer {
        fn render(
            &mut self,
            _context: WgpuPaneRenderContext<'_>,
        ) -> Result<WgpuPaneRenderResult, WgpuPaneRenderError> {
            Ok(WgpuPaneRenderResult::idle())
        }

        fn input(&mut self, _event: &WindowInputEvent) -> WgpuPaneInputResult {
            self.input_count += 1;
            WgpuPaneInputResult::redraw()
        }
    }

    #[test]
    fn plugin_routes_input_and_requests_redraw() {
        let mut plugin =
            WgpuPaneRenderPlugin::new(RenderTargetId::new(9), InputRenderer { input_count: 0 });

        let result = plugin.input(&WindowInputEvent::ModifiersChanged(
            WindowInputModifiers::new(true, false, false, false),
        ));

        assert_eq!(plugin.target_id(), RenderTargetId::new(9));
        assert!(result.request_redraw);
        assert!(plugin.is_enabled());
    }

    struct PanickingInputRenderer;

    impl WgpuPaneRenderer for PanickingInputRenderer {
        fn render(
            &mut self,
            _context: WgpuPaneRenderContext<'_>,
        ) -> Result<WgpuPaneRenderResult, WgpuPaneRenderError> {
            Ok(WgpuPaneRenderResult::idle())
        }

        fn input(&mut self, _event: &WindowInputEvent) -> WgpuPaneInputResult {
            panic!("input failed")
        }
    }

    #[test]
    fn plugin_is_disabled_after_input_panic() {
        let mut plugin = WgpuPaneRenderPlugin::new(RenderTargetId::new(3), PanickingInputRenderer);

        plugin.input(&WindowInputEvent::PointerLeft);

        assert!(!plugin.is_enabled());
    }
}
