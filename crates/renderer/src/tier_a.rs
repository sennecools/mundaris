//! Tier A world-map bake on the GPU (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6).
//!
//! Under ADR 0023 this bake is the authority for a world-map body's macro
//! fields; `astrum_world::terrain::tier_a` is its CPU test oracle. A bake runs
//! a fixed schedule of compute passes (continents, sea level, shape, ocean
//! blur, climate, moisture iterations, finish) that can be spread over frames.
//! The result is one storage buffer of five field runs of `6·n²` values:
//! elevation (m, sea level 0), temperature (°C), moisture (0..1), wind east and
//! wind north.

/// Shader: shared noise and cube-map sampling followed by the bake stages.
const SHADER: &str = concat!(
    include_str!("shaders/terrain_noise.wgsl"),
    include_str!("shaders/cube_map.wgsl"),
    include_str!("shaders/tier_a.wgsl")
);

/// Field runs in a finished result buffer, in order.
pub const TIER_A_RESULT_RUNS: u32 = 5;
const SCRATCH_RUNS: u32 = 11;
const RUN_OCEAN: [u32; 2] = [6, 7];
const RUN_CARRIED: [u32; 2] = [8, 9];
const HISTOGRAM_BINS: u64 = 4096;
const STATS_WORDS: u64 = HISTOGRAM_BINS + 4;
const PASS_STRIDE: u64 = 256;
const WORKGROUP: u32 = 256;
/// Diffusion passes of the ocean mask (`tier_a::OCEAN_BLUR_PASSES`).
pub const TIER_A_OCEAN_BLUR_PASSES: u32 = 16;

/// Stage flags (`TierAStage` in the world crate).
pub mod stage {
    pub const CONTINENTS: u32 = 1;
    pub const SEA_LEVEL: u32 = 2;
    pub const SHELF: u32 = 4;
    pub const TEMPERATURE: u32 = 8;
    pub const WIND: u32 = 16;
    pub const MOISTURE: u32 = 32;
}

/// Plain bake inputs, already converted to the shader's units (radians,
/// frequencies in cycles per unit direction).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TierABakeInputs {
    pub face_cells: u32,
    pub stages: u32,
    pub radius_m: f32,
    pub continent_frequency: f32,
    pub warp_frequency: f32,
    pub warp_scale: f32,
    pub octaves: u32,
    pub lacunarity: f32,
    pub gain: f32,
    pub continent_seed: u32,
    pub warp_seed: u32,
    pub temperature_seed: u32,
    pub ocean_coverage: f32,
    pub land_height_m: f32,
    pub land_exponent: f32,
    pub ocean_depth_m: f32,
    pub shelf_depth_m: f32,
    pub shelf_fraction: f32,
    pub equator_c: f32,
    pub pole_c: f32,
    pub axial_tilt_rad: f32,
    pub lapse_c_per_km: f32,
    pub ocean_moderation: f32,
    pub ocean_blur_step_rad: f32,
    pub temperature_noise_c: f32,
    pub temperature_noise_frequency: f32,
    pub wind_cells: f32,
    pub wind_meridional: f32,
    pub evaporation: f32,
    pub rain: f32,
    pub moisture_iterations: u32,
    pub moisture_step_rad: f32,
    pub moisture_spread: f32,
    pub precipitation_scale: f32,
    pub pole: [f32; 3],
}

impl TierABakeInputs {
    pub fn validate(&self, device: &wgpu::Device) -> Result<(), String> {
        let n = u64::from(self.face_cells);
        let bytes = 4 * (u64::from(SCRATCH_RUNS) + 1) * 6 * n * n;
        if !self.face_cells.is_power_of_two() || !(4..=2048).contains(&self.face_cells) {
            return Err(format!(
                "Tier A face cells {} must be a power of two",
                self.face_cells
            ));
        }
        if bytes > device.limits().max_storage_buffer_binding_size {
            return Err(format!("Tier A bake needs {bytes} bytes of storage"));
        }
        if !(1..=512).contains(&self.moisture_iterations) {
            return Err("Tier A moisture iterations out of range".into());
        }
        Ok(())
    }

    fn packed(&self) -> Vec<u8> {
        let n = self.face_cells;
        let len = 6 * n * n;
        let u = |v: [u32; 4]| {
            v.into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<u8>>()
        };
        let f = |v: [f32; 4]| {
            v.into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<u8>>()
        };
        [
            u([n, self.stages, len, 0]),
            f([
                self.radius_m,
                self.continent_frequency,
                self.warp_frequency,
                self.warp_scale,
            ]),
            f([self.octaves as f32, self.lacunarity, self.gain, 0.0]),
            u([
                self.continent_seed,
                self.warp_seed,
                self.temperature_seed,
                0,
            ]),
            f([
                self.ocean_coverage,
                self.land_height_m,
                self.land_exponent,
                self.ocean_depth_m,
            ]),
            f([self.shelf_depth_m, self.shelf_fraction, 0.0, 0.0]),
            f([
                self.equator_c,
                self.pole_c,
                self.axial_tilt_rad,
                self.lapse_c_per_km,
            ]),
            f([
                self.ocean_moderation,
                self.ocean_blur_step_rad,
                self.temperature_noise_c,
                self.temperature_noise_frequency,
            ]),
            f([
                self.wind_cells,
                self.wind_meridional,
                self.evaporation,
                self.rain,
            ]),
            f([
                self.moisture_step_rad,
                self.moisture_spread,
                self.precipitation_scale,
                0.0,
            ]),
            f([self.pole[0], self.pole[1], self.pole[2], 0.0]),
        ]
        .concat()
    }
}

/// Fields with mip chains: elevation, temperature, moisture.
pub const TIER_A_MIP_FIELDS: u32 = 3;

/// Field mip levels in a result buffer: `(offset in f32 values, face cells)`
/// of elevation; temperature and moisture of the same level follow at
/// `+6·cells²` and `+12·cells²`. Level 0 is the first three result runs;
/// levels 1.. halve the face cells down to 4 and follow the five result runs.
pub fn field_mip_layout(face_cells: u32) -> Vec<(u32, u32)> {
    let mut levels = vec![(0, face_cells)];
    let mut offset = TIER_A_RESULT_RUNS * 6 * face_cells * face_cells;
    let mut cells = face_cells / 2;
    while cells >= 4 {
        levels.push((offset, cells));
        offset += TIER_A_MIP_FIELDS * 6 * cells * cells;
        cells /= 2;
    }
    levels
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Continents,
    SeaLevel,
    Shape,
    OceanBlur,
    Climate,
    MoistureStep,
    MoistureFinish,
    FieldMip,
}

/// Compute pipelines shared by every bake.
pub(crate) struct TierAPipelines {
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<(Stage, wgpu::ComputePipeline)>,
}

impl TierAPipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Tier A bake"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let storage = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Tier A bake"),
            entries: &[
                entry(
                    0,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
                entry(1, storage),
                entry(2, storage),
                entry(
                    3,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Tier A bake"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines = [
            (Stage::Continents, "continents"),
            (Stage::SeaLevel, "sea_level"),
            (Stage::Shape, "shape"),
            (Stage::OceanBlur, "ocean_blur"),
            (Stage::Climate, "climate"),
            (Stage::MoistureStep, "moisture_step"),
            (Stage::MoistureFinish, "moisture_finish"),
            (Stage::FieldMip, "field_mip"),
        ]
        .into_iter()
        .map(|(stage, entry_point)| {
            (
                stage,
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(entry_point),
                    layout: Some(&pipeline_layout),
                    module: &module,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    cache: None,
                }),
            )
        })
        .collect();
        Self { layout, pipelines }
    }

    fn pipeline(&self, stage: Stage) -> &wgpu::ComputePipeline {
        &self
            .pipelines
            .iter()
            .find(|(s, _)| *s == stage)
            .expect("every stage has a pipeline")
            .1
    }
}

/// One body's bake in progress, then its result buffer.
pub(crate) struct TierABake {
    face_cells: u32,
    schedule: Vec<(Stage, u32, u32)>,
    next: usize,
    scratch: Option<(
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::BindGroup,
    )>,
    result: wgpu::Buffer,
    done: bool,
}

impl TierABake {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipelines: &TierAPipelines,
        inputs: &TierABakeInputs,
    ) -> Result<Self, String> {
        inputs.validate(device)?;
        let n = inputs.face_cells;
        let run_bytes = 4 * 6 * u64::from(n) * u64::from(n);
        // Pass schedule: (stage, src run, dst run).
        let mut schedule = vec![
            (Stage::Continents, 0, 0),
            (Stage::SeaLevel, 0, 0),
            (Stage::Shape, 0, RUN_OCEAN[0]),
        ];
        let blur = if inputs.ocean_blur_step_rad > 0.0 {
            TIER_A_OCEAN_BLUR_PASSES
        } else {
            0
        };
        for pass in 0..blur {
            schedule.push((
                Stage::OceanBlur,
                RUN_OCEAN[pass as usize % 2],
                RUN_OCEAN[(pass as usize + 1) % 2],
            ));
        }
        schedule.push((Stage::Climate, RUN_OCEAN[blur as usize % 2], 0));
        if inputs.stages & stage::MOISTURE != 0 {
            for pass in 0..inputs.moisture_iterations {
                schedule.push((
                    Stage::MoistureStep,
                    RUN_CARRIED[pass as usize % 2],
                    RUN_CARRIED[(pass as usize + 1) % 2],
                ));
            }
        }
        schedule.push((Stage::MoistureFinish, 0, 0));
        let mips = field_mip_layout(n);
        for level in 0..mips.len() as u32 - 1 {
            for field in 0..TIER_A_MIP_FIELDS {
                schedule.push((Stage::FieldMip, level, field));
            }
        }
        let mip_floats: u64 = mips[1..]
            .iter()
            .map(|(_, cells)| {
                u64::from(TIER_A_MIP_FIELDS) * 6 * u64::from(*cells) * u64::from(*cells)
            })
            .sum();

        let fields = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A bake fields"),
            size: run_bytes * u64::from(SCRATCH_RUNS) + 4 * mip_floats,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let stats = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A bake histogram"),
            size: 4 * STATS_WORDS,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A bake parameters"),
            size: 176,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&params, 0, &inputs.packed());
        let passes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A bake passes"),
            size: PASS_STRIDE * schedule.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut pass_bytes = vec![0u8; (PASS_STRIDE * schedule.len() as u64) as usize];
        for (index, (_, src, dst)) in schedule.iter().enumerate() {
            let at = index * PASS_STRIDE as usize;
            pass_bytes[at..at + 4].copy_from_slice(&src.to_le_bytes());
            pass_bytes[at + 4..at + 8].copy_from_slice(&dst.to_le_bytes());
        }
        queue.write_buffer(&passes, 0, &pass_bytes);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Tier A bake"),
            layout: &pipelines.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: fields.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: stats.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &passes,
                        offset: 0,
                        size: wgpu::BufferSize::new(16),
                    }),
                },
            ],
        });
        let result = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A fields"),
            size: run_bytes * u64::from(TIER_A_RESULT_RUNS) + 4 * mip_floats,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Ok(Self {
            face_cells: n,
            schedule,
            next: 0,
            scratch: Some((fields, stats, params, passes, group)),
            result,
            done: false,
        })
    }

    pub(crate) fn face_cells(&self) -> u32 {
        self.face_cells
    }

    pub(crate) fn done(&self) -> bool {
        self.done
    }

    pub(crate) fn result(&self) -> &wgpu::Buffer {
        &self.result
    }

    /// Encode up to `budget` passes; on the last one, copy the five result
    /// runs out of the scratch buffer and release it.
    pub(crate) fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &TierAPipelines,
        budget: usize,
    ) {
        let Some((fields, _, _, _, group)) = &self.scratch else {
            return;
        };
        let texels = 6 * self.face_cells * self.face_cells;
        let groups = texels.div_ceil(WORKGROUP);
        let (x, y) = if groups <= 65_535 {
            (groups, 1)
        } else {
            (65_535, groups.div_ceil(65_535))
        };
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Tier A bake"),
                timestamp_writes: None,
            });
            let end = (self.next + budget).min(self.schedule.len());
            for index in self.next..end {
                let (stage, _, _) = self.schedule[index];
                pass.set_pipeline(pipelines.pipeline(stage));
                pass.set_bind_group(0, group, &[(index as u64 * PASS_STRIDE) as u32]);
                if stage == Stage::SeaLevel {
                    pass.dispatch_workgroups(1, 1, 1);
                } else {
                    pass.dispatch_workgroups(x, y, 1);
                }
            }
            self.next = end;
        }
        if self.next == self.schedule.len() {
            let run_bytes = 4 * u64::from(texels);
            let result_runs = run_bytes * u64::from(TIER_A_RESULT_RUNS);
            encoder.copy_buffer_to_buffer(fields, run_bytes, &self.result, 0, result_runs);
            let mip_bytes = self.result.size() - result_runs;
            if mip_bytes > 0 {
                encoder.copy_buffer_to_buffer(
                    fields,
                    run_bytes * u64::from(SCRATCH_RUNS),
                    &self.result,
                    result_runs,
                    mip_bytes,
                );
            }
            self.done = true;
        }
    }

    /// Release the scratch buffers once the final copy has been submitted.
    pub(crate) fn release_scratch(&mut self) {
        if self.done {
            self.scratch = None;
        }
    }

    fn stats(&self) -> Option<&wgpu::Buffer> {
        self.scratch.as_ref().map(|s| &s.1)
    }
}

/// Read-back of a finished bake, validation only.
#[derive(Debug, Clone)]
pub struct TierAReadback {
    pub face_cells: u32,
    /// Five runs of `6·n²` values: elevation, temperature, moisture, wind east,
    /// wind north.
    pub fields: Vec<f32>,
    /// Normalised noise at sea level and at the 0.1 % and 99.9 % percentiles.
    pub sea_level: f32,
    pub noise_low: f32,
    pub noise_high: f32,
}

impl TierAReadback {
    pub fn run(&self, index: usize) -> &[f32] {
        let len = 6 * (self.face_cells * self.face_cells) as usize;
        &self.fields[index * len..(index + 1) * len]
    }

    /// Mip `level` of `field` (0 elevation, 1 temperature, 2 moisture; see
    /// [`field_mip_layout`]).
    pub fn field_mip(&self, field: usize, level: usize) -> &[f32] {
        let (offset, cells) = field_mip_layout(self.face_cells)[level];
        let start = offset as usize + field * 6 * (cells * cells) as usize;
        &self.fields[start..start + 6 * (cells * cells) as usize]
    }
}

/// Bake `inputs` on a caller-owned device and read every field back.
/// Validation only: blocking, allocates per call.
pub fn tier_a_for_validation(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    inputs: &TierABakeInputs,
) -> Result<TierAReadback, String> {
    let pipelines = TierAPipelines::new(device);
    let mut bake = TierABake::new(device, queue, &pipelines, inputs)?;
    let mut encoder = device.create_command_encoder(&Default::default());
    bake.encode(&mut encoder, &pipelines, usize::MAX);
    let read = |encoder: &mut wgpu::CommandEncoder, source: &wgpu::Buffer, size: u64| {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A validation readback"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(source, 0, &buffer, 0, size);
        buffer
    };
    let fields = read(&mut encoder, bake.result(), bake.result().size());
    let stats = read(
        &mut encoder,
        bake.stats().ok_or("bake released its scratch early")?,
        4 * STATS_WORDS,
    );
    queue.submit([encoder.finish()]);
    for buffer in [&fields, &stats] {
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| error.to_string())?;
    let floats = |buffer: &wgpu::Buffer| -> Result<Vec<f32>, String> {
        let view = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|error| error.to_string())?;
        Ok(view
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect())
    };
    let values = floats(&fields)?;
    let words = floats(&stats)?;
    let at = HISTOGRAM_BINS as usize;
    Ok(TierAReadback {
        face_cells: bake.face_cells(),
        fields: values,
        sea_level: words[at],
        noise_low: words[at + 1],
        noise_high: words[at + 2],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_a_shader_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(SHADER)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(SHADER)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(SHADER)));
    }

    #[test]
    fn packed_parameters_match_the_uniform_layout() {
        assert_eq!(TierABakeInputs::default().packed().len(), 176);
    }
}
