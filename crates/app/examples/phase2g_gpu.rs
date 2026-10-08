//! Matched GPU-vs-CPU MoonFieldsV1 recipe mirror experiment.
//!
//! Usage: cargo run --release --locked -p mundaris_app --example phase2g_gpu -- <new-output.json>
//! This is an app-composed hybrid profile evaluator, not native rendering or
//! terrain residency. CPU preparation uses body-local f64 inputs; the GPU
//! receives only those anchored f32 scalar recipes. Validation readback is
//! explicitly diagnostic and is never part of the native renderer path.

use anyhow::{Context, Result, ensure};
use glam::DVec3;
use mundaris_math::{
    Direction3,
    surface::{CubeFace, SurfaceLocation},
};
use mundaris_renderer::field_compute::{FieldComputeJob, FieldComputeService};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};
use serde_json::json;
use std::{fs, path::PathBuf, time::Instant};

const RADIUS_VALUES_M: [f64; 2] = [109_081.776_8, 1_737_400.0];
const SEEDS: [u64; 4] = [2, 7, 19, 43];
const GRID_CELLS: [u32; 3] = [32, 64, 128];
const POINTS_PER_BATCH: usize = 512;
const POINT_STRIDE: usize = 657;
const MAX_CRATERS: usize = 162;
const WORKGROUP_SIZE: u32 = 64;
const HEIGHT_TOLERANCE_M: f32 = 0.001;
const MATERIAL_TOLERANCE: f32 = 1.0e-5;
const SHADER: &str = include_str!("../src/shaders/moon_fields.wgsl");

#[derive(Default)]
struct Metrics {
    count: usize,
    height_squared: f64,
    max_gpu_f32_height: f64,
    max_gpu_full_source_height: f64,
    max_material_f32: f64,
    max_material_full_source: f64,
    failed_height: usize,
    failed_material: usize,
    first_failure: Option<serde_json::Value>,
}

#[allow(clippy::too_many_arguments)] // One identified numerical comparison sample.
fn add(
    metrics: &mut Metrics,
    gpu: [f32; 8],
    cpu_f32: [f32; 5],
    full_height: f64,
    full_material: [f64; 4],
    point: DVec3,
    seed: u64,
    radius: f64,
) {
    let height_delta = f64::from((gpu[0] - cpu_f32[0]).abs());
    metrics.count += 1;
    metrics.height_squared += height_delta * height_delta;
    metrics.max_gpu_f32_height = metrics.max_gpu_f32_height.max(height_delta);
    metrics.max_gpu_full_source_height = metrics
        .max_gpu_full_source_height
        .max((f64::from(gpu[0]) - full_height).abs());
    let mut height_failed = false;
    let tolerance = f64::from(HEIGHT_TOLERANCE_M + 2.0 * ulp_f32(cpu_f32[0]));
    if height_delta > tolerance {
        metrics.failed_height += 1;
        height_failed = true;
    }
    let mut max_material = 0.0f64;
    let mut max_material_full = 0.0f64;
    for channel in 0..4 {
        let error = f64::from((gpu[4 + channel] - cpu_f32[1 + channel]).abs());
        max_material = max_material.max(error);
        max_material_full =
            max_material_full.max((f64::from(gpu[4 + channel]) - full_material[channel]).abs());
    }
    metrics.max_material_f32 = metrics.max_material_f32.max(max_material);
    metrics.max_material_full_source = metrics.max_material_full_source.max(max_material_full);
    let material_failed = max_material > f64::from(MATERIAL_TOLERANCE);
    if material_failed {
        metrics.failed_material += 1;
    }
    if (height_failed || material_failed) && metrics.first_failure.is_none() {
        metrics.first_failure = Some(json!({
            "seed": seed, "radius_m": radius, "direction_body": point.to_array(),
            "height_gpu_m": gpu[0], "height_cpu_quantized_m": cpu_f32[0],
            "height_error_m": height_delta, "height_tolerance_m": tolerance,
            "material_gpu": [gpu[4], gpu[5], gpu[6], gpu[7]],
            "material_cpu_quantized": [cpu_f32[1], cpu_f32[2], cpu_f32[3], cpu_f32[4]],
            "material_max_error": max_material,
        }));
    }
}

fn ulp_f32(value: f32) -> f32 {
    if value == 0.0 {
        return f32::from_bits(1);
    }
    if !value.is_finite() {
        return 0.0;
    }
    let next = if value >= 0.0 {
        f32::from_bits(value.to_bits().saturating_add(1))
    } else {
        f32::from_bits(value.to_bits().saturating_sub(1))
    };
    (next - value).abs()
}

fn corpus() -> Result<Vec<(DVec3, u32, &'static str)>> {
    let mut points = Vec::new();
    // Three field-cell labels sample the same small gnomonic chart footprint
    // on each face. These are 9x9 diagnostic stencils, not full page grids.
    for face in CubeFace::ALL {
        for cells in GRID_CELLS {
            for y in 0..9 {
                for x in 0..9 {
                    let u = -0.01 + 0.02 * f64::from(x) / 8.0;
                    let v = -0.01 + 0.02 * f64::from(y) / 8.0;
                    points.push((face.direction([u, v])?.unit(), cells, "face_patch"));
                }
            }
        }
    }
    // Canonical edge/corner chart directions, including ties handled by the
    // source cube-face convention. Repeated representations are intentional.
    for face in CubeFace::ALL {
        for t in [-0.75, -0.25, 0.0, 0.25, 0.75] {
            for uv in [[-1.0, t], [1.0, t], [t, -1.0], [t, 1.0]] {
                points.push((face.direction(uv)?.unit(), 64, "chart_edge"));
            }
        }
        for u in [-1.0, 1.0] {
            for v in [-1.0, 1.0] {
                points.push((face.direction([u, v])?.unit(), 64, "chart_corner"));
            }
        }
    }
    Ok(points)
}

fn pack_point(inputs: &mundaris_world::terrain::MoonFieldPointInputs, packed: &mut Vec<f32>) {
    packed.extend([
        inputs.global_height_m() as f32,
        inputs.plains() as f32,
        inputs.highlands() as f32,
        inputs.regional() as f32,
    ]);
    packed.extend(inputs.material_emphasis_factors().map(|value| value as f32));
    packed.push(inputs.crater_count() as f32);
    let crater_start = packed.len();
    for crater in inputs.craters() {
        packed.extend([
            f32::from(crater.band()),
            crater.q2() as f32,
            crater.freshness() as f32,
            crater.regional_strength() as f32,
        ]);
    }
    packed.resize(crater_start + MAX_CRATERS * 4, 0.0);
    debug_assert_eq!(packed.len() % POINT_STRIDE, 0);
}

async fn run() -> Result<()> {
    let output_path = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("provide a new output JSON path")?,
    );
    ensure!(
        !output_path.exists(),
        "refusing to replace experiment evidence"
    );
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        })
        .await
        .context("no headless GPU adapter available")?;
    let adapter_info = adapter.get_info();
    let adapter_limits = adapter.limits();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("Phase 2G hybrid field experiment"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        })
        .await
        .context("requesting a headless GPU device")?;
    let service = FieldComputeService;
    let directions = corpus()?;
    let mut cases = Vec::new();
    let mut generation = 1u64;
    for seed in SEEDS {
        let repetitions = if seed == 2 { 3 } else { 1 };
        for radius in RADIUS_VALUES_M {
            for repetition in 0..repetitions {
                let definition = SurfaceDefinition::generated(
                    TerrainIdentity(0x4d4f_4f4e),
                    TerrainSeed(seed),
                    SurfaceAlgorithm::MoonFieldsV1,
                );
                let generator = SurfaceGenerator::new(&definition, radius)?;
                let mut metrics = Metrics::default();
                let mut source_prepare_ns = 0u128;
                let mut source_reference_ns = 0u128;
                let mut cpu_f32_ns = 0u128;
                let mut packing_ns = 0u128;
                let mut dispatch_validation_ns = 0u128;
                let mut validation_readback_ns = 0u128;
                let mut max_input_bytes = 0u64;
                let mut max_parameter_bytes = 0u64;
                let mut max_output_bytes = 0u64;
                let mut max_staging_bytes = 0u64;
                let mut max_in_flight_estimate = 0u64;
                let mut corpus_offset = 0;
                while corpus_offset < directions.len() {
                    let end = (corpus_offset + POINTS_PER_BATCH).min(directions.len());
                    let batch = &directions[corpus_offset..end];
                    let mut packed = Vec::with_capacity(batch.len() * POINT_STRIDE);
                    let mut references = Vec::with_capacity(batch.len());
                    for (direction, field_cells, label) in batch {
                        let location = SurfaceLocation::new(Direction3::try_new(*direction)?);
                        let source_prepare_started = Instant::now();
                        let inputs = generator.prepare_moon_field_point(location)?;
                        source_prepare_ns += source_prepare_started.elapsed().as_nanos();
                        let source_reference_started = Instant::now();
                        let exact = generator.evaluate_point(location)?;
                        let exact_height = exact.radius_m() - radius;
                        let exact_material = exact.material_weights();
                        source_reference_ns += source_reference_started.elapsed().as_nanos();
                        let cpu_mirror_started = Instant::now();
                        let prepared = inputs.evaluate_f32();
                        cpu_f32_ns += cpu_mirror_started.elapsed().as_nanos();
                        let pack_started = Instant::now();
                        pack_point(&inputs, &mut packed);
                        packing_ns += pack_started.elapsed().as_nanos();
                        references.push((
                            *direction,
                            *field_cells,
                            *label,
                            prepared.height_m,
                            prepared.material_weights,
                            exact_height,
                            exact_material,
                        ));
                    }
                    let inputs_for_gpu = packed;
                    let input_values = inputs_for_gpu;
                    generation = generation.checked_add(1).context("generation overflow")?;
                    let parameters = [1.0f32];
                    let dispatch_started = Instant::now();
                    let page = service
                        .dispatch(
                            &device,
                            &queue,
                            FieldComputeJob {
                                wgsl: SHADER,
                                input: &input_values,
                                parameters: &parameters,
                                output_records: u32::try_from(batch.len() * 2)?,
                                workgroup_size_x: WORKGROUP_SIZE,
                                generation,
                            },
                        )
                        .await?;
                    dispatch_validation_ns += dispatch_started.elapsed().as_nanos();
                    max_input_bytes = max_input_bytes.max(page.input_bytes());
                    max_parameter_bytes = max_parameter_bytes.max(page.parameter_bytes());
                    max_output_bytes = max_output_bytes.max(page.output_bytes());
                    max_staging_bytes = max_staging_bytes.max(page.validation_readback_bytes());
                    max_in_flight_estimate = max_in_flight_estimate
                        .max(page.input_bytes() + page.parameter_bytes() + page.output_bytes());
                    let readback_started = Instant::now();
                    let records = service.readback_validation(&device, &queue, &page)?;
                    validation_readback_ns += readback_started.elapsed().as_nanos();
                    ensure!(
                        page.generation() == generation,
                        "generation metadata mismatch"
                    );
                    for (index, reference) in references.iter().enumerate() {
                        let first = records[index * 2];
                        let second = records[index * 2 + 1];
                        let gpu = [
                            first[0], first[1], first[2], first[3], second[0], second[1],
                            second[2], second[3],
                        ];
                        let cpu = [
                            reference.3,
                            reference.4[0],
                            reference.4[1],
                            reference.4[2],
                            reference.4[3],
                        ];
                        ensure!(
                            gpu.iter().all(|value| value.is_finite()),
                            "GPU output was nonfinite at case seed={seed} radius_m={radius} sample={index}"
                        );
                        ensure!(
                            cpu.iter().all(|value| value.is_finite())
                                && reference.5.is_finite()
                                && reference.6.iter().all(|value| value.is_finite()),
                            "CPU reference was nonfinite at case seed={seed} radius_m={radius} sample={index}"
                        );
                        add(
                            &mut metrics,
                            gpu,
                            cpu,
                            reference.5,
                            reference.6,
                            reference.0,
                            seed,
                            radius,
                        );
                        if let Some(failure) = metrics.first_failure.as_mut()
                            && failure.get("field_cells").is_none()
                        {
                            failure["field_cells"] = json!(reference.1);
                            failure["corpus_region"] = json!(reference.2);
                        }
                    }
                    corpus_offset = end;
                }
                cases.push(json!({
                    "seed":seed, "radius_m":radius, "repetition":repetition,
                    "points":metrics.count, "field_cell_labels":GRID_CELLS,
                    "source_recipe_preparation_us":source_prepare_ns as f64 / 1000.0,
                    "full_source_reference_us":source_reference_ns as f64 / 1000.0,
                    "cpu_quantized_mirror_us":cpu_f32_ns as f64 / 1000.0,
                    "input_packing_us":packing_ns as f64 / 1000.0,
                    "gpu_shader_validation_and_submit_us":dispatch_validation_ns as f64 / 1000.0,
                    "gpu_validation_readback_wait_us":validation_readback_ns as f64 / 1000.0,
                    "max_gpu_vs_quantized_cpu_height_error_m":metrics.max_gpu_f32_height,
                    "rms_gpu_vs_quantized_cpu_height_error_m":
                        (metrics.height_squared / metrics.count.max(1) as f64).sqrt(),
                    "max_gpu_vs_full_source_height_error_m":metrics.max_gpu_full_source_height,
                    "max_gpu_vs_quantized_cpu_material_error":metrics.max_material_f32,
                    "max_gpu_vs_full_source_material_error":metrics.max_material_full_source,
                    "height_gate_m":"1 mm + 2 ULP of local radial offset",
                    "material_gate":MATERIAL_TOLERANCE,
                    "height_gate_failures":metrics.failed_height,
                    "material_gate_failures":metrics.failed_material,
                    "first_failure":metrics.first_failure,
                    "max_input_storage_bytes":max_input_bytes,
                    "max_parameter_storage_bytes":max_parameter_bytes,
                    "max_output_bytes":max_output_bytes,
                    "max_validation_staging_bytes":max_staging_bytes,
                    "max_submitted_job_bytes_estimate":max_in_flight_estimate,
                    "batches_sequential":true,
                    "native_readback":false,
                    "resident_native_integration":false
                }));
            }
        }
    }
    let value = json!({
        "schema_version":1,
        "experiment":"Phase 2G hybrid MoonFieldsV1 GPU recipe mirror",
        "status":"experimental app-composed profile; no native renderer integration or acceptance claim",
        "adapter":{"name":adapter_info.name,"vendor":adapter_info.vendor,
            "device":adapter_info.device,"device_type":format!("{:?}",adapter_info.device_type),
            "backend":format!("{:?}",adapter_info.backend),"driver":adapter_info.driver,
            "driver_info":adapter_info.driver_info},
        "device_limits":{"max_compute_invocations_per_workgroup":adapter_limits.max_compute_invocations_per_workgroup,
            "max_compute_workgroup_size_x":adapter_limits.max_compute_workgroup_size_x,
            "max_compute_workgroups_per_dimension":adapter_limits.max_compute_workgroups_per_dimension,
            "max_storage_buffer_binding_size":adapter_limits.max_storage_buffer_binding_size,
            "max_buffer_size":adapter_limits.max_buffer_size},
        "recipe_layout":{"input_f32_per_point":POINT_STRIDE,"max_crater_recipes":MAX_CRATERS,
            "output_vec4_records_per_point":2,"workgroup_size_x":WORKGROUP_SIZE,
            "output_height":"radial offset in metres from reference sphere; no large planet radius is added on GPU",
            "inputs":"anchored body-local recipe scalars and f64-prepared q2/freshness/strength quantized to f32"},
        "corpus":{"seed_set":SEEDS,"radii_m":RADIUS_VALUES_M,"field_cell_labels":GRID_CELLS,
            "face_patch_stencil":"9x9 samples per face per field-cell label in gnomonic uv [-0.01,0.01]^2; diagnostic sample stencil, not a full grid/page",
            "edge_stencil":"5 interior t samples on each of four chart edges per face",
            "corner_stencil":"four chart corners per face",
            "points_per_case":1602,
            "coverage":"all six cube faces, the same small chart footprint, chart edges and corners; duplicated boundary directions are intentional"},
        "cases":cases
    });
    if let Some(parent) = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(output_path, serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

fn main() -> Result<()> {
    pollster::block_on(run())
}
