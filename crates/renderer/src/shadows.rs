//! Sun shadow cascade maps: one Depth32Float array layer per cascade, a light
//! projection uniform per cascade (dynamic offset), and the comparison sampler
//! receivers use for percentage-closer filtering.

use crate::render_settings::MAX_CASCADES;

const LIGHT_STRIDE: u64 = 256;

pub(crate) struct ShadowMaps {
    resolution: u32,
    _texture: wgpu::Texture,
    pub array_view: wgpu::TextureView,
    pub layer_views: Vec<wgpu::TextureView>,
    pub compare_sampler: wgpu::Sampler,
    light: wgpu::Buffer,
    pub light_layout: wgpu::BindGroupLayout,
    pub light_group: wgpu::BindGroup,
}

impl ShadowMaps {
    pub(crate) fn light_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow cascade projection"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        })
    }

    pub(crate) fn new(device: &wgpu::Device, resolution: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Sun shadow cascades"),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: MAX_CASCADES,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::post::DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let array_view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Sun shadow cascade array"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let layer_views = (0..MAX_CASCADES)
            .map(|layer| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("Sun shadow cascade"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: layer,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let compare_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shadow comparison"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let light = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shadow cascade projections"),
            size: LIGHT_STRIDE * u64::from(MAX_CASCADES),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_layout = Self::light_layout(device);
        let light_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Shadow cascade projection"),
            layout: &light_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &light,
                    offset: 0,
                    size: wgpu::BufferSize::new(64),
                }),
            }],
        });
        Self {
            resolution,
            _texture: texture,
            array_view,
            layer_views,
            compare_sampler,
            light,
            light_layout,
            light_group,
        }
    }

    pub(crate) fn resolution(&self) -> u32 {
        self.resolution
    }

    pub(crate) fn write_projections(&self, queue: &wgpu::Queue, cascades: &crate::Cascades) {
        let mut bytes = vec![0u8; (LIGHT_STRIDE * u64::from(MAX_CASCADES)) as usize];
        for (c, matrix) in cascades.view_to_clip.iter().enumerate() {
            let at = c * LIGHT_STRIDE as usize;
            for (i, value) in matrix.to_cols_array().iter().enumerate() {
                bytes[at + i * 4..at + i * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        queue.write_buffer(&self.light, 0, &bytes);
    }

    pub(crate) fn offset(cascade: usize) -> u32 {
        (cascade as u64 * LIGHT_STRIDE) as u32
    }
}
