#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WgpuViewportUniform {
	pub width_px:  f32,
	pub height_px: f32,
}

impl WgpuViewportUniform {
	pub const fn new(width_px: f32, height_px: f32) -> Self { Self { width_px, height_px } }

	pub fn as_std140_bytes(&self) -> [u8; 8] {
		let mut bytes = [0u8; 8];

		bytes[0..4].copy_from_slice(&self.width_px.to_ne_bytes());
		bytes[4..8].copy_from_slice(&self.height_px.to_ne_bytes());

		bytes
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalShaderSpec {
	pub source:         &'static str,
	pub vertex_entry:   &'static str,
	pub fragment_entry: &'static str,

	/// Current name.
	pub viewport_binding: u32,

	/// Compatibility alias for older tests/code.
	pub viewport_bind_group: u32,
}

impl WgpuTerminalShaderSpec {
	pub const fn new() -> Self {
		Self {
			source:              WGPU_TERMINAL_SHADER_WGSL,
			vertex_entry:        "vs_main",
			fragment_entry:      "fs_main",
			viewport_binding:    0,
			viewport_bind_group: 0,
		}
	}

	pub const fn shader_source(&self) -> &'static str { self.source }
}

impl Default for WgpuTerminalShaderSpec {
	fn default() -> Self { Self::new() }
}

pub const WGPU_VERTEX_KIND_BACKGROUND: u32 = 0;
pub const WGPU_VERTEX_KIND_GLYPH: u32 = 1;
pub const WGPU_VERTEX_KIND_UNDERLINE: u32 = 2;

pub const WGPU_TERMINAL_SHADER_WGSL: &str = r#"
struct Viewport {
    width_px: f32,
    height_px: f32,
}

@group(0) @binding(0)
var<uniform> viewport: Viewport;

@group(1) @binding(0)
var glyph_atlas: texture_2d<f32>;

@group(1) @binding(1)
var glyph_sampler: sampler;

struct VertexInput {
    @location(0) position_px: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) kind: u32,
    @location(4) glyph_codepoint: u32,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) kind: u32,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;

    let x = (input.position_px.x / viewport.width_px) * 2.0 - 1.0;
    let y = 1.0 - (input.position_px.y / viewport.height_px) * 2.0;

    output.position = vec4<f32>(x, y, 0.0, 1.0);
    output.uv = input.uv;
    output.color = input.color;
    output.kind = input.kind;

    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if (input.kind == 1u) {
        let glyph_sample = textureSample(
            glyph_atlas,
            glyph_sampler,
            input.uv,
        );

        let color_sum = glyph_sample.r + glyph_sample.g + glyph_sample.b;

        if (color_sum > 0.001) {
            return glyph_sample;
        }

        return vec4<f32>(
            input.color.rgb,
            input.color.a * glyph_sample.a,
        );
    }

    return input.color;
}
"#;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn viewport_uniform_serializes_to_8_bytes() {
		let viewport = WgpuViewportUniform::new(1280.0, 720.0);
		let bytes = viewport.as_std140_bytes();

		assert_eq!(bytes.len(), 8);

		let width = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
		let height = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());

		assert_eq!(width, 1280.0);
		assert_eq!(height, 720.0);
	}

	#[test]
	fn shader_spec_uses_terminal_entries() {
		let spec = WgpuTerminalShaderSpec::new();

		assert_eq!(spec.vertex_entry, "vs_main");
		assert_eq!(spec.fragment_entry, "fs_main");
		assert_eq!(spec.viewport_binding, 0);
		assert_eq!(spec.viewport_bind_group, 0);
		assert!(spec.shader_source().contains("fn vs_main"));
		assert!(spec.shader_source().contains("fn fs_main"));
		assert!(spec.source.contains("@group(0) @binding(0)"));
		assert!(spec.source.contains("@group(1) @binding(0)"));
		assert!(spec.source.contains("@group(1) @binding(1)"));
		assert!(spec.source.contains("textureSample"));
	}
}
