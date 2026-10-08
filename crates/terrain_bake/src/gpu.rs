//! wgpu compute execution of base generation and erosion.

use crate::noise::{ROLE_HARDNESS, ROLE_HEIGHT, ROLE_WARP_X, pcg};
use crate::recipe::{BaseRecipe, HydraulicRecipe, NoiseKind, NoiseLayer, ThermalRecipe};
use anyhow::{Context, Result, anyhow, ensure};
use serde::Serialize;
use std::sync::mpsc;

const WORKGROUP: u32 = 16;
const EROSION_BINDINGS: u32 = 11;
/// Iterations per submission; bounds a single submission's GPU time well below
/// the operating system's device-timeout window.
const ITERATIONS_PER_SUBMIT: u32 = 32;

#[derive(Debug, Clone, Serialize)]
pub struct AdapterIdentity {
    pub name: String,
    pub backend: String,
    pub driver: String,
    pub driver_info: String,
    pub vendor: u32,
    pub device: u32,
}

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub identity: AdapterIdentity,
    base_pipeline: wgpu::ComputePipeline,
    base_layout: wgpu::BindGroupLayout,
    erosion_layout: wgpu::BindGroupLayout,
    erosion_pipelines: [wgpu::ComputePipeline; 6],
    /// Settle step: deposit load, then thermal flux and apply.
    settle_pipelines: [wgpu::ComputePipeline; 3],
    /// Bytes of GPU buffers currently allocated by this tool, and the peak.
    allocated_bytes: u64,
    pub peak_allocated_bytes: u64,
}

/// Grid state read back after a stage, in cell units.
pub struct ErosionResult {
    pub terrain: Vec<f32>,
    pub sediment: Vec<f32>,
    pub water: Vec<f32>,
}

impl Gpu {
    pub fn new() -> Result<Self> {
        pollster::block_on(Self::create())
    }

    async fn create() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .context("no GPU adapter available for terrain baking")?;
        let info = adapter.get_info();
        let supported = adapter.limits();
        ensure!(
            supported.max_storage_buffers_per_shader_stage >= EROSION_BINDINGS,
            "adapter supports only {} storage buffers per stage; erosion needs {}",
            supported.max_storage_buffers_per_shader_stage,
            EROSION_BINDINGS
        );
        let limits = wgpu::Limits {
            max_storage_buffers_per_shader_stage: EROSION_BINDINGS,
            max_storage_buffer_binding_size: supported.max_storage_buffer_binding_size,
            max_buffer_size: supported.max_buffer_size,
            ..wgpu::Limits::default()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("terrain bake"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                trace: wgpu::Trace::Off,
            })
            .await
            .context("creating terrain bake device")?;

        let base_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain bake base"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/base.wgsl").into()),
        });
        let erosion_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain bake erosion"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/erosion.wgsl").into()),
        });

        let base_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain bake base"),
            entries: &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, false),
                storage_entry(3, false),
            ],
        });
        let mut erosion_entries = vec![uniform_entry(0)];
        for binding in 1..EROSION_BINDINGS {
            erosion_entries.push(storage_entry(binding, binding == 10));
        }
        let erosion_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain bake erosion"),
            entries: &erosion_entries,
        });

        let base_pipeline = pipeline(&device, &base_layout, &base_shader, "base_main");
        let erosion_pipelines = [
            "flux_main",
            "sediment_main",
            "water_main",
            "erode_main",
            "thermal_flux_main",
            "thermal_apply_main",
        ]
        .map(|entry| pipeline(&device, &erosion_layout, &erosion_shader, entry));
        let settle_pipelines = ["settle_main", "thermal_flux_main", "thermal_apply_main"]
            .map(|entry| pipeline(&device, &erosion_layout, &erosion_shader, entry));

        Ok(Self {
            device,
            queue,
            identity: AdapterIdentity {
                name: info.name,
                backend: format!("{:?}", info.backend),
                driver: info.driver,
                driver_info: info.driver_info,
                vendor: info.vendor,
                device: info.device,
            },
            base_pipeline,
            base_layout,
            erosion_layout,
            erosion_pipelines,
            settle_pipelines,
            allocated_bytes: 0,
            peak_allocated_bytes: 0,
        })
    }

    fn buffer(&mut self, label: &str, size: u64, usage: wgpu::BufferUsages) -> TrackedBuffer {
        self.allocated_bytes += size;
        self.peak_allocated_bytes = self.peak_allocated_bytes.max(self.allocated_bytes);
        TrackedBuffer {
            buffer: self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            }),
            size,
        }
    }

    fn release(&mut self, buffers: impl IntoIterator<Item = TrackedBuffer>) {
        for tracked in buffers {
            self.allocated_bytes -= tracked.size;
            tracked.buffer.destroy();
        }
    }

    fn storage(&mut self, label: &str, size: u64) -> TrackedBuffer {
        self.buffer(
            label,
            size,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
        )
    }

    /// Evaluate base height (metres) and hardness at pixel centres of an `n²` grid.
    pub fn base(
        &mut self,
        base: &BaseRecipe,
        recipe_seed: u64,
        n: u32,
    ) -> Result<(Vec<f32>, Vec<f32>)> {
        let cells = u64::from(n) * u64::from(n);
        let mut layer_bytes = Vec::new();
        for (index, layer) in base.layers.iter().enumerate() {
            push_layer(&mut layer_bytes, layer, ROLE_HEIGHT + index as u32);
        }
        let fallback = base.hardness.layer;
        let warp = base.warp.as_ref().map_or(&fallback, |warp| &warp.layer);
        push_layer(&mut layer_bytes, warp, ROLE_WARP_X);
        push_layer(&mut layer_bytes, &base.hardness.layer, ROLE_HARDNESS);

        let seed_base = pcg((recipe_seed as u32) ^ pcg((recipe_seed >> 32) as u32));
        let mut params = Vec::new();
        for word in [
            n,
            base.layers.len() as u32,
            u32::from(base.warp.is_some()),
            seed_base,
        ] {
            params.extend_from_slice(&word.to_le_bytes());
        }
        for value in [
            base.amplitude_m,
            base.warp.as_ref().map_or(0.0, |warp| warp.amplitude),
            base.hardness.base,
            base.hardness.variation,
        ] {
            params.extend_from_slice(&(value as f32).to_le_bytes());
        }

        let uniform = self.buffer(
            "base params",
            params.len() as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );
        let layers = self.storage("base layers", layer_bytes.len() as u64);
        let height = self.storage("base height", cells * 4);
        let hardness = self.storage("base hardness", cells * 4);
        self.queue.write_buffer(&uniform.buffer, 0, &params);
        self.queue.write_buffer(&layers.buffer, 0, &layer_bytes);
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain bake base"),
            layout: &self.base_layout,
            entries: &[
                binding(0, &uniform),
                binding(1, &layers),
                binding(2, &height),
                binding(3, &hardness),
            ],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.base_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let groups = n.div_ceil(WORKGROUP);
            pass.dispatch_workgroups(groups, groups, 1);
        }
        self.queue.submit([encoder.finish()]);
        let heights = self.read_f32(&height, cells)?;
        let hardness_values = self.read_f32(&hardness, cells)?;
        self.release([uniform, layers, height, hardness]);
        Ok((heights, hardness_values))
    }

    /// Run `iterations` erosion steps from `terrain_cells` (heights in cells),
    /// then `hydraulic.settle_iterations` settle steps: the remaining suspended
    /// load is deposited and slopes steeper than the hardest-rock talus relax at
    /// full rate, so concentrated deposits spread into small cones instead of
    /// remaining as single-cell pillars.
    /// Cells at or below `outlet_level_cells` drain water and sediment.
    // Keep the stage inputs explicit at this single internal call boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn erode(
        &mut self,
        terrain_cells: &[f32],
        hardness_values: &[f32],
        n: u32,
        iterations: u32,
        outlet_level_cells: f32,
        hydraulic: &HydraulicRecipe,
        thermal: &ThermalRecipe,
    ) -> Result<ErosionResult> {
        let cells = u64::from(n) * u64::from(n);
        ensure!(terrain_cells.len() as u64 == cells && hardness_values.len() as u64 == cells);
        let encode = |thermal_rate: f64| {
            let mut params = Vec::new();
            for word in [
                n,
                outlet_level_cells.to_bits(),
                (hydraulic.max_speed as f32).to_bits(),
                0,
            ] {
                params.extend_from_slice(&word.to_le_bytes());
            }
            for value in [
                hydraulic.dt,
                hydraulic.gravity,
                hydraulic.rain,
                hydraulic.evaporation,
                hydraulic.capacity,
                hydraulic.dissolve,
                hydraulic.deposit,
                hydraulic.min_tilt,
                hydraulic.depth_reference,
                // Hydraulic stages only relax slopes steeper than the hardest rock's
                // talus, so they remove deposition spikes without flattening the
                // hardness-dependent slopes formed by fluvial stages.
                thermal.talus_tangent * (1.0 + thermal.talus_hardness),
                thermal_rate,
                hydraulic.max_erosion_per_step,
            ] {
                params.extend_from_slice(&(value as f32).to_le_bytes());
            }
            params
        };
        let uniform_usage = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        let active_params = encode(thermal.rate);
        // dt × rate ≥ 1 moves half the largest excess per settle step.
        let settle_params = encode(1.0 / hydraulic.dt);
        let uniforms = [
            self.buffer("erosion params", active_params.len() as u64, uniform_usage),
            self.buffer("settle params", settle_params.len() as u64, uniform_usage),
        ];
        let terrain = self.storage("terrain", cells * 4);
        let terrain_tmp = self.storage("terrain tmp", cells * 4);
        let water = self.storage("water", cells * 4);
        let sediment = [
            self.storage("sediment a", cells * 4),
            self.storage("sediment b", cells * 4),
        ];
        let flux = self.storage("flux", cells * 16);
        let velocity = self.storage("velocity", cells * 8);
        let thermal_axis = self.storage("thermal axis", cells * 16);
        let thermal_diagonal = self.storage("thermal diagonal", cells * 16);
        let hardness = self.storage("hardness", cells * 4);
        self.queue
            .write_buffer(&uniforms[0].buffer, 0, &active_params);
        self.queue
            .write_buffer(&uniforms[1].buffer, 0, &settle_params);
        self.queue
            .write_buffer(&terrain.buffer, 0, &f32_bytes(terrain_cells));
        self.queue
            .write_buffer(&hardness.buffer, 0, &f32_bytes(hardness_values));
        // New buffers are zero-initialised by wgpu: no water, sediment or flux.

        // Indexed [phase][parity]: phase 0 erodes, phase 1 settles.
        let bind_groups = [0usize, 1].map(|phase| {
            [0usize, 1].map(|parity| {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terrain bake erosion"),
                    layout: &self.erosion_layout,
                    entries: &[
                        binding(0, &uniforms[phase]),
                        binding(1, &terrain),
                        binding(2, &terrain_tmp),
                        binding(3, &water),
                        binding(4, &sediment[parity]),
                        binding(5, &sediment[1 - parity]),
                        binding(6, &flux),
                        binding(7, &velocity),
                        binding(8, &thermal_axis),
                        binding(9, &thermal_diagonal),
                        binding(10, &hardness),
                    ],
                })
            })
        });

        let groups = n.div_ceil(WORKGROUP);
        let total = iterations + hydraulic.settle_iterations;
        let mut done = 0;
        while done < total {
            let batch = (total - done).min(ITERATIONS_PER_SUBMIT);
            let mut encoder = self.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                for step in done..done + batch {
                    if step < iterations {
                        pass.set_bind_group(0, &bind_groups[0][(step % 2) as usize], &[]);
                        for pipeline in &self.erosion_pipelines {
                            pass.set_pipeline(pipeline);
                            pass.dispatch_workgroups(groups, groups, 1);
                        }
                    } else {
                        // Bind so that sediment_out is the buffer the last
                        // erosion step wrote: buffer (iterations % 2).
                        let parity = 1 - (iterations % 2) as usize;
                        pass.set_bind_group(0, &bind_groups[1][parity], &[]);
                        for pipeline in &self.settle_pipelines {
                            pass.set_pipeline(pipeline);
                            pass.dispatch_workgroups(groups, groups, 1);
                        }
                    }
                }
            }
            let submission = self.queue.submit([encoder.finish()]);
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: None,
                })
                .map_err(|error| anyhow!("GPU erosion progress: {error}"))?;
            done += batch;
        }
        // The last erosion step wrote buffer (iterations % 2); settle steps clear it.
        let latest = &sediment[(iterations % 2) as usize];
        let result = ErosionResult {
            terrain: self.read_f32(&terrain, cells)?,
            sediment: self.read_f32(latest, cells)?,
            water: self.read_f32(&water, cells)?,
        };
        let [sediment_a, sediment_b] = sediment;
        let [active_uniform, settle_uniform] = uniforms;
        self.release([
            active_uniform,
            settle_uniform,
            terrain,
            terrain_tmp,
            water,
            sediment_a,
            sediment_b,
            flux,
            velocity,
            thermal_axis,
            thermal_diagonal,
            hardness,
        ]);
        Ok(result)
    }

    fn read_f32(&mut self, source: &TrackedBuffer, count: u64) -> Result<Vec<f32>> {
        let size = count * 4;
        let staging = self.buffer(
            "readback",
            size,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&source.buffer, 0, &staging.buffer, 0, size);
        let submission = self.queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        staging
            .buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| anyhow!("GPU readback progress: {error}"))?;
        receiver
            .recv()
            .context("readback callback dropped")?
            .map_err(|error| anyhow!("mapping readback: {error}"))?;
        let values = {
            let mapped = staging
                .buffer
                .slice(..)
                .get_mapped_range()
                .map_err(|error| anyhow!("reading mapped readback: {error}"))?;
            mapped
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&bytes| f32::from_le_bytes(bytes))
                .collect()
        };
        staging.buffer.unmap();
        self.release([staging]);
        Ok(values)
    }
}

struct TrackedBuffer {
    buffer: wgpu::Buffer,
    size: u64,
}

fn binding(index: u32, tracked: &TrackedBuffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding: index,
        resource: tracked.buffer.as_entire_binding(),
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    module: &wgpu::ShaderModule,
    entry: &str,
) -> wgpu::ComputePipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(entry),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(entry),
        layout: Some(&pipeline_layout),
        module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn push_layer(bytes: &mut Vec<u8>, layer: &NoiseLayer, role: u32) {
    let kind = match layer.kind {
        NoiseKind::Fbm => 0u32,
        NoiseKind::Ridged => 1,
    };
    for word in [kind, layer.frequency, layer.octaves, layer.lacunarity] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for value in [layer.gain, layer.weight, layer.sharpness] {
        bytes.extend_from_slice(&(value as f32).to_le_bytes());
    }
    bytes.extend_from_slice(&role.to_le_bytes());
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}
