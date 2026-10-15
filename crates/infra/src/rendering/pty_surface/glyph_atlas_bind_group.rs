use crate::rendering::pty_surface::glyph_atlas_texture::WgpuTerminalGlyphAtlasTexture;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WgpuTerminalGlyphAtlasBindGroupSpec {
    pub texture_binding: u32,
    pub sampler_binding: u32,
    pub visibility: wgpu::ShaderStages,
    pub view_dimension: wgpu::TextureViewDimension,
    pub sample_type_filterable: bool,
    pub sampler_binding_type: wgpu::SamplerBindingType,
}

impl WgpuTerminalGlyphAtlasBindGroupSpec {
    pub const fn new() -> Self {
        Self {
            texture_binding: 0,
            sampler_binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type_filterable: true,
            sampler_binding_type: wgpu::SamplerBindingType::Filtering,
        }
    }
}

impl Default for WgpuTerminalGlyphAtlasBindGroupSpec {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WgpuTerminalGlyphAtlasBindGroupFactory {
    spec: WgpuTerminalGlyphAtlasBindGroupSpec,
}

impl WgpuTerminalGlyphAtlasBindGroupFactory {
    pub fn new() -> Self {
        Self {
            spec: WgpuTerminalGlyphAtlasBindGroupSpec::new(),
        }
    }

    pub fn with_spec(spec: WgpuTerminalGlyphAtlasBindGroupSpec) -> Self {
        Self { spec }
    }

    pub fn spec(&self) -> WgpuTerminalGlyphAtlasBindGroupSpec {
        self.spec
    }

    pub fn create_layout(&self, device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("germinal.terminal.glyph_atlas.bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: self.spec.texture_binding,
                    visibility: self.spec.visibility,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float {
                            filterable: self.spec.sample_type_filterable,
                        },
                        view_dimension: self.spec.view_dimension,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: self.spec.sampler_binding,
                    visibility: self.spec.visibility,
                    ty: wgpu::BindingType::Sampler(self.spec.sampler_binding_type),
                    count: None,
                },
            ],
        })
    }

    pub fn create_bind_group(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        atlas_texture: &WgpuTerminalGlyphAtlasTexture,
    ) -> WgpuTerminalGlyphAtlasBindGroup {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("germinal.terminal.glyph_atlas.bind_group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: self.spec.texture_binding,
                    resource: wgpu::BindingResource::TextureView(&atlas_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: self.spec.sampler_binding,
                    resource: wgpu::BindingResource::Sampler(&atlas_texture.sampler),
                },
            ],
        });

        WgpuTerminalGlyphAtlasBindGroup {
            bind_group,
            texture_binding: self.spec.texture_binding,
            sampler_binding: self.spec.sampler_binding,
        }
    }
}

pub struct WgpuTerminalGlyphAtlasBindGroup {
    pub bind_group: wgpu::BindGroup,
    pub texture_binding: u32,
    pub sampler_binding: u32,
}
