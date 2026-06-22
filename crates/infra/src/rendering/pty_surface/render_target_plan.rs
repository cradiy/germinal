use germinal_ports::pty_host::{size_info::TerminalSizeInfo, window_size::TerminalWindowSize};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuTerminalRenderTargetPlan {
	pub width_px:    u32,
	pub height_px:   u32,
	pub clear_color: WgpuTerminalClearColor,
	pub load_op:     WgpuTerminalLoadOp,
	pub store:       bool,
}

impl WgpuTerminalRenderTargetPlan {
	pub fn new(width_px: u32, height_px: u32) -> Self {
		Self {
			width_px,
			height_px,
			clear_color: WgpuTerminalClearColor::default(),
			load_op: WgpuTerminalLoadOp::Clear,
			store: true,
		}
	}

	pub fn from_window_size(window_size: TerminalWindowSize) -> Self {
		Self::new(window_size.width_px(), window_size.height_px())
	}

	pub fn from_size_info(size_info: TerminalSizeInfo) -> Self {
		Self::from_window_size(size_info.window_size())
	}

	pub fn with_clear_color(mut self, clear_color: WgpuTerminalClearColor) -> Self {
		self.clear_color = clear_color;
		self
	}

	pub fn with_load_op(mut self, load_op: WgpuTerminalLoadOp) -> Self {
		self.load_op = load_op;
		self
	}

	pub fn with_store(mut self, store: bool) -> Self {
		self.store = store;
		self
	}

	pub fn viewport_width_px(&self) -> f32 { self.width_px as f32 }

	pub fn viewport_height_px(&self) -> f32 { self.height_px as f32 }

	pub fn is_empty(&self) -> bool { self.width_px == 0 || self.height_px == 0 }

	pub fn wgpu_color(&self) -> wgpu::Color { self.clear_color.into() }

	pub fn wgpu_load_op(&self) -> wgpu::LoadOp<wgpu::Color> {
		match self.load_op {
			WgpuTerminalLoadOp::Clear => wgpu::LoadOp::Clear(self.wgpu_color()),
			WgpuTerminalLoadOp::Load => wgpu::LoadOp::Load,
		}
	}
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuTerminalClearColor {
	pub red:   f64,
	pub green: f64,
	pub blue:  f64,
	pub alpha: f64,
}

impl WgpuTerminalClearColor {
	pub const fn new(red: f64, green: f64, blue: f64, alpha: f64) -> Self {
		Self { red, green, blue, alpha }
	}

	pub const fn black() -> Self { Self::new(0.0, 0.0, 0.0, 1.0) }

	pub const fn transparent() -> Self { Self::new(0.0, 0.0, 0.0, 0.0) }
}

impl Default for WgpuTerminalClearColor {
	fn default() -> Self { Self::black() }
}

impl From<WgpuTerminalClearColor> for wgpu::Color {
	fn from(color: WgpuTerminalClearColor) -> Self {
		Self { r: color.red, g: color.green, b: color.blue, a: color.alpha }
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WgpuTerminalLoadOp {
	Clear,
	Load,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn builds_default_render_target_plan() {
		let plan = WgpuTerminalRenderTargetPlan::new(1280, 720);

		assert_eq!(plan.width_px, 1280);
		assert_eq!(plan.height_px, 720);
		assert_eq!(plan.viewport_width_px(), 1280.0);
		assert_eq!(plan.viewport_height_px(), 720.0);
		assert_eq!(plan.clear_color, WgpuTerminalClearColor::black());
		assert_eq!(plan.load_op, WgpuTerminalLoadOp::Clear);
		assert!(plan.store);
		assert!(!plan.is_empty());
	}

	#[test]
	fn detects_empty_render_target() {
		assert!(WgpuTerminalRenderTargetPlan::new(0, 720).is_empty());
		assert!(WgpuTerminalRenderTargetPlan::new(1280, 0).is_empty());
		assert!(WgpuTerminalRenderTargetPlan::new(0, 0).is_empty());
		assert!(!WgpuTerminalRenderTargetPlan::new(1280, 720).is_empty());
	}

	#[test]
	fn converts_clear_color_to_wgpu_color() {
		let plan = WgpuTerminalRenderTargetPlan::new(1280, 720)
			.with_clear_color(WgpuTerminalClearColor::new(0.1, 0.2, 0.3, 1.0));

		let color = plan.wgpu_color();

		assert_eq!(color.r, 0.1);
		assert_eq!(color.g, 0.2);
		assert_eq!(color.b, 0.3);
		assert_eq!(color.a, 1.0);
	}

	#[test]
	fn builds_wgpu_load_op() {
		let clear_plan =
			WgpuTerminalRenderTargetPlan::new(1280, 720).with_load_op(WgpuTerminalLoadOp::Clear);

		match clear_plan.wgpu_load_op() {
			wgpu::LoadOp::Clear(color) => {
				assert_eq!(color, wgpu::Color::BLACK);
			}
			other => panic!("expected clear load op, got {other:?}"),
		}

		let load_plan =
			WgpuTerminalRenderTargetPlan::new(1280, 720).with_load_op(WgpuTerminalLoadOp::Load);

		assert!(matches!(load_plan.wgpu_load_op(), wgpu::LoadOp::Load));
	}
}
