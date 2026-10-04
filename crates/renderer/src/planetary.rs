//! Bounded renderer-only planetary material configuration.

use crate::RenderPreparationError;
use crate::celestial::PlanetaryDraw;

/// Natural broad-scale land palettes; none describe authoritative terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanetLandProfile {
    Earth,
    Rock,
    Mars,
}

/// A small validated set of visual shells around a rendered planetary surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetaryConfig {
    pub land: PlanetLandProfile,
    pub ocean_enabled: bool,
    pub clouds_enabled: bool,
    pub atmosphere_enabled: bool,
    pub sea_datum_m: f64,
    pub cloud_altitude_m: f64,
    pub atmosphere_height_m: f64,
    pub density_falloff: f32,
    pub cloud_coverage: f32,
    pub cloud_roughness: f32,
    pub ocean_roughness: f32,
    /// Vertical optical-depth coefficients (linear RGB), not display colours.
    pub rayleigh_optical_depth: [f32; 3],
    pub haze_optical_depth: f32,
    pub sun_intensity: f32,
}

impl Default for PlanetaryConfig {
    fn default() -> Self {
        Self {
            land: PlanetLandProfile::Earth,
            ocean_enabled: true,
            clouds_enabled: true,
            atmosphere_enabled: true,
            sea_datum_m: 0.0,
            cloud_altitude_m: 12_000.0,
            atmosphere_height_m: 100_000.0,
            density_falloff: 4.0,
            cloud_coverage: 0.32,
            cloud_roughness: 0.42,
            ocean_roughness: 0.25,
            rayleigh_optical_depth: [0.035, 0.085, 0.19],
            haze_optical_depth: 0.012,
            sun_intensity: 1.0,
        }
    }
}

impl PlanetaryConfig {
    pub fn try_validate(self) -> Result<Self, RenderPreparationError> {
        if !self.sea_datum_m.is_finite()
            || !self.cloud_altitude_m.is_finite()
            || !self.atmosphere_height_m.is_finite()
            || self.sea_datum_m.abs() > 100_000.0
            || !(0.0..=500_000.0).contains(&self.cloud_altitude_m)
            || !(0.0..=2_000_000.0).contains(&self.atmosphere_height_m)
            || !self.density_falloff.is_finite()
            || !(0.1..=32.0).contains(&self.density_falloff)
            || !self.cloud_coverage.is_finite()
            || !(0.0..=1.0).contains(&self.cloud_coverage)
            || !self.cloud_roughness.is_finite()
            || !(0.0..=1.0).contains(&self.cloud_roughness)
            || !self.ocean_roughness.is_finite()
            || !(0.08..=0.8).contains(&self.ocean_roughness)
            || self
                .rayleigh_optical_depth
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=2.0).contains(v))
            || !self.haze_optical_depth.is_finite()
            || !(0.0..=1.0).contains(&self.haze_optical_depth)
            || !self.sun_intensity.is_finite()
            || !(0.0..=10.0).contains(&self.sun_intensity)
            || (self.atmosphere_enabled && self.atmosphere_height_m <= 0.0)
            || (self.clouds_enabled && self.cloud_altitude_m <= 0.0)
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        Ok(self)
    }
}

pub(crate) struct PlanetaryRenderer {
    uniforms: wgpu::Buffer,
    group: wgpu::BindGroup,
    ocean: wgpu::RenderPipeline,
    clouds: wgpu::RenderPipeline,
    atmosphere: wgpu::RenderPipeline,
    depth_layout: wgpu::BindGroupLayout,
    target_srgb: bool,
}

pub(crate) fn shell_constant(observer_radius_m: f64, radius_m: f64, altitude_m: f64) -> f32 {
    let shell_radius_m = radius_m + altitude_m;
    ((observer_radius_m - shell_radius_m) / radius_m
        * ((observer_radius_m + shell_radius_m) / radius_m)) as f32
}

impl PlanetaryRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bounded planetary layers"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/planetary.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Planetary layer uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(176),
                },
                count: None,
            }],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bounded planetary per-body uniform storage"),
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Planetary layer uniform binding"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniforms,
                    offset: 0,
                    size: wgpu::BufferSize::new(176),
                }),
            }],
        });
        let depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Completed celestial depth sample"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline = |entry_point: &str, depth_write: bool, depth_test: bool, blend: bool| {
            let layouts: Vec<&wgpu::BindGroupLayout> = if entry_point == "fs_atmosphere" {
                vec![&layout, &depth_layout]
            } else {
                vec![&layout]
            };
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Planetary shell pipeline layout"),
                bind_group_layouts: &layouts,
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry_point),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: if blend {
                            Some(wgpu::BlendState::ALPHA_BLENDING)
                        } else {
                            None
                        },
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: depth_test.then_some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: depth_write,
                    depth_compare: wgpu::CompareFunction::GreaterEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        Self {
            ocean: pipeline("fs_ocean", true, true, false),
            clouds: pipeline("fs_cloud", false, true, true),
            atmosphere: pipeline("fs_atmosphere", false, false, true),
            uniforms,
            group,
            depth_layout,
            target_srgb: format.is_srgb(),
        }
    }

    pub fn upload(
        &self,
        queue: &wgpu::Queue,
        draws: &[PlanetaryDraw],
        projection: crate::CelestialProjection,
    ) {
        for (index, draw) in draws.iter().enumerate() {
            let config = draw.config;
            let flags = u32::from(config.ocean_enabled)
                | (u32::from(config.clouds_enabled) << 1)
                | (u32::from(config.atmosphere_enabled) << 2);
            let floats = [
                draw.observer_body_radius[0],
                draw.observer_body_radius[1],
                draw.observer_body_radius[2],
                draw.radius_m,
                draw.sun_body[0],
                draw.sun_body[1],
                draw.sun_body[2],
                0.0,
                draw.body_axes_view[0][0],
                draw.body_axes_view[0][1],
                draw.body_axes_view[0][2],
                0.0,
                draw.body_axes_view[1][0],
                draw.body_axes_view[1][1],
                draw.body_axes_view[1][2],
                0.0,
                draw.body_axes_view[2][0],
                draw.body_axes_view[2][1],
                draw.body_axes_view[2][2],
                0.0,
                config.sea_datum_m as f32,
                config.cloud_altitude_m as f32,
                config.cloud_coverage,
                config.cloud_roughness,
                projection.viewport()[0] as f32,
                projection.viewport()[1] as f32,
                (projection.vertical_fov_rad() * 0.5).tan() as f32,
                projection.near_m() as f32,
                config.atmosphere_height_m as f32,
                config.density_falloff,
                config.ocean_roughness,
                config.sun_intensity,
            ];
            let mut bytes = [0u8; 176];
            for (value, output) in floats.into_iter().zip(bytes.as_chunks_mut::<4>().0) {
                output.copy_from_slice(&value.to_le_bytes());
            }
            let profile = match config.land {
                PlanetLandProfile::Earth => 0u32,
                PlanetLandProfile::Rock => 1,
                PlanetLandProfile::Mars => 2,
            };
            for (value, output) in [flags, profile, u32::from(self.target_srgb), 0]
                .into_iter()
                .zip(bytes[128..144].as_chunks_mut::<4>().0)
            {
                output.copy_from_slice(&value.to_le_bytes());
            }
            for (value, output) in draw
                .sphere_constants
                .into_iter()
                .chain(config.rayleigh_optical_depth)
                .chain([config.haze_optical_depth])
                .zip(bytes[144..].as_chunks_mut::<4>().0)
            {
                output.copy_from_slice(&value.to_le_bytes());
            }
            queue.write_buffer(&self.uniforms, index as u64 * 256, &bytes);
        }
    }

    pub fn draw_shells(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        draws: &[PlanetaryDraw],
        timestamps: Option<&crate::gpu_profile::CelestialQueries>,
    ) {
        for (i, draw) in draws.iter().enumerate() {
            let offset = [i as u32 * 256];
            if draw.config.ocean_enabled {
                if draws.len() == 1
                    && let Some(queries) = timestamps.filter(|q| q.inside_passes())
                {
                    queries.write_scope(pass, 5);
                }
                pass.set_pipeline(&self.ocean);
                pass.set_bind_group(0, &self.group, &offset);
                pass.draw(0..3, 0..1);
                if draws.len() == 1
                    && let Some(queries) = timestamps.filter(|q| q.inside_passes())
                {
                    queries.end_scope(pass, 5);
                }
            }
            if draw.config.clouds_enabled {
                if draws.len() == 1
                    && let Some(queries) = timestamps.filter(|q| q.inside_passes())
                {
                    queries.write_scope(pass, 6);
                }
                pass.set_pipeline(&self.clouds);
                pass.set_bind_group(0, &self.group, &offset);
                pass.draw(0..3, 0..1);
                if draws.len() == 1
                    && let Some(queries) = timestamps.filter(|q| q.inside_passes())
                {
                    queries.end_scope(pass, 6);
                }
            }
        }
    }

    pub fn atmosphere_group(
        &self,
        device: &wgpu::Device,
        depth: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Completed celestial depth"),
            layout: &self.depth_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(depth),
            }],
        })
    }

    pub fn draw_atmosphere(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        draws: &[PlanetaryDraw],
        depth: &wgpu::BindGroup,
    ) {
        for (i, _draw) in draws
            .iter()
            .enumerate()
            .filter(|(_, d)| d.config.atmosphere_enabled)
        {
            pass.set_pipeline(&self.atmosphere);
            pass.set_bind_group(0, &self.group, &[i as u32 * 256]);
            pass.set_bind_group(1, depth, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factored_shell_roots_retain_small_clearance_at_gameplay_and_real_radii() {
        for radius in [400_000.0, 6_371_000.0, 100_000_000.0] {
            for clearance in [0.1_f64, 2.0, 100.0, 10_000.0] {
                let datum = 350.0;
                let observer = radius + datum + clearance;
                let ro = (observer / radius) as f32;
                let c = shell_constant(observer, radius, datum);
                let q = ro + (ro * ro - c).sqrt();
                let near_m = f64::from(c / q) * radius;
                let tolerance = clearance * 1e-6 + radius * f64::EPSILON * 4.0;
                assert!(
                    (near_m - clearance).abs() < tolerance,
                    "radius={radius} clearance={clearance} actual={near_m}"
                );
                assert!(shell_constant(radius + datum - clearance, radius, datum) < 0.0);
            }
        }
    }
}
