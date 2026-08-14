use crate::rendering::surface_snapshot::RenderSurfaceSnapshot;

pub trait RendererBackend {
    fn render_surface(&self, snapshot: &RenderSurfaceSnapshot);
}
