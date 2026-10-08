//! Bounded, algorithm-agnostic GPU field-page compute experiments.
//!
//! Caller-supplied WGSL uses a one-dimensional `main` entry point and one bind
//! group with three storage buffers:
//!
//! - binding 0: read-only `array<f32>` input samples;
//! - binding 1: read-only `array<f32>` producer parameters;
//! - binding 2: read-write `array<vec4<f32>>` output records.
//!
//! Input and parameter buffers are populated from caller data. A dispatch
//! submits work to the supplied queue and returns without waiting for GPU
//! completion. The returned page owns the output storage buffer. Readback is a
//! separate validation-only operation that explicitly waits for its copy.
//! Per-dispatch byte caps do not limit concurrent jobs: callers must bound
//! submitted work and retained results, and account for queued input buffers
//! until their submissions complete.

use std::sync::mpsc;

use wgpu::util::DeviceExt;

/// Maximum accepted caller-provided WGSL source size.
pub const MAX_FIELD_SHADER_BYTES: usize = 256 * 1024;
/// Maximum bytes in either input storage buffer.
pub const MAX_FIELD_INPUT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum bytes in an output field page or validation readback.
pub const MAX_FIELD_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
/// Maximum combined input, parameter, and output bytes in one request.
pub const MAX_FIELD_JOB_BYTES: usize = 24 * 1024 * 1024;

/// Inputs and dispatch shape for one field-page compute experiment.
#[derive(Debug, Clone)]
pub struct FieldComputeJob<'a> {
    /// WGSL compute module. Its `main` entry point must declare
    /// `@compute @workgroup_size(workgroup_size_x)` and the documented bindings.
    pub wgsl: &'a str,
    /// Read-only f32 samples bound at group 0, binding 0.
    pub input: &'a [f32],
    /// Read-only f32 producer parameters bound at group 0, binding 1.
    pub parameters: &'a [f32],
    /// Number of `vec4<f32>` records in the output page.
    pub output_records: u32,
    /// Must match the WGSL `main` entry point's one-dimensional workgroup size.
    pub workgroup_size_x: u32,
    /// Caller-owned nonzero content/request generation copied to the result.
    pub generation: u64,
}

/// Opaque output storage returned after a compute dispatch is submitted.
///
/// This is not a `TileData` and is not a CPU terrain authority. It retains the
/// derived GPU page and its caller-supplied generation metadata only.
pub struct ResidentFieldPage {
    output: wgpu::Buffer,
    output_records: u32,
    output_bytes: u64,
    input_bytes: u64,
    parameter_bytes: u64,
    generation: u64,
    workgroups_x: u32,
}

impl ResidentFieldPage {
    pub fn output_records(&self) -> u32 {
        self.output_records
    }

    pub fn output_bytes(&self) -> u64 {
        self.output_bytes
    }

    /// Input storage bytes retained by the submitted command until completion.
    pub fn input_bytes(&self) -> u64 {
        self.input_bytes
    }

    /// Parameter storage bytes retained by the submitted command until completion.
    pub fn parameter_bytes(&self) -> u64 {
        self.parameter_bytes
    }

    /// Additional staging bytes allocated if validation readback is requested.
    pub fn validation_readback_bytes(&self) -> u64 {
        self.output_bytes
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn workgroups_x(&self) -> u32 {
        self.workgroups_x
    }

    /// Buffer handle for renderer or caller binding in a later GPU operation.
    /// The page remains opaque as to its contents and algorithm.
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.output
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FieldComputeError {
    #[error("field compute shader or request is invalid: {0}")]
    InvalidRequest(&'static str),
    #[error("field compute request exceeds the configured or device storage limit")]
    CapacityExceeded,
    #[error("field compute shader validation failed: {0}")]
    ShaderValidation(String),
    #[error("GPU progress failed during validation readback: {0}")]
    GpuProgress(String),
}

/// Generic GPU executor for caller-owned field-generation kernels.
#[derive(Debug, Default, Clone, Copy)]
pub struct FieldComputeService;

impl FieldComputeService {
    /// Compile and submit one bounded field-page dispatch.
    ///
    /// This awaits only wgpu's shader/pipeline validation scope. It does not
    /// wait for the submitted compute work to finish.
    pub async fn dispatch(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        job: FieldComputeJob<'_>,
    ) -> Result<ResidentFieldPage, FieldComputeError> {
        let (input_bytes, parameter_bytes, output_bytes, workgroups_x) =
            validate_job(device, &job)?;
        validate_wgsl_workgroup(job.wgsl, job.workgroup_size_x)?;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Caller field-page compute experiment"),
            source: wgpu::ShaderSource::Wgsl(job.wgsl.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Generic field-page compute bindings"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(2, false),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Generic field-page compute layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Generic field-page compute pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(error) = device.pop_error_scope().await {
            return Err(FieldComputeError::ShaderValidation(error.to_string()));
        }

        let input_data = pack_f32(job.input);
        let parameter_data = pack_f32(job.parameters);
        let input_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Field-page compute input"),
            contents: &input_data,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let parameter_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Field-page compute parameters"),
            contents: &parameter_data,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Resident derived field page"),
            size: output_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Field-page compute inputs and output"),
            layout: &layout,
            entries: &[
                buffer_entry(0, &input_buffer),
                buffer_entry(1, &parameter_buffer),
                buffer_entry(2, &output),
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Field-page compute encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Field-page compute dispatch"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups_x, 1, 1);
        }
        queue.submit([encoder.finish()]);

        // wgpu command submission retains referenced input buffers until the
        // dispatch has consumed them. Only the output is part of the result.
        drop((input_buffer, parameter_buffer));
        Ok(ResidentFieldPage {
            output,
            output_records: job.output_records,
            output_bytes,
            input_bytes,
            parameter_bytes,
            generation: job.generation,
            workgroups_x,
        })
    }

    /// Explicit validation readback for tests and bounded diagnostics.
    ///
    /// This allocates at most [`MAX_FIELD_OUTPUT_BYTES`] and waits for the copy
    /// submission to finish. Do not call this on the ordinary native path.
    pub fn readback_validation(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        page: &ResidentFieldPage,
    ) -> Result<Vec<[f32; 4]>, FieldComputeError> {
        if page.output_bytes == 0 || page.output_bytes > MAX_FIELD_OUTPUT_BYTES as u64 {
            return Err(FieldComputeError::CapacityExceeded);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Field-page validation readback"),
            size: page.output_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Field-page validation readback encoder"),
        });
        encoder.copy_buffer_to_buffer(&page.output, 0, &readback, 0, page.output_bytes);
        let submission = queue.submit([encoder.finish()]);
        let (sender, receiver) = mpsc::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| FieldComputeError::GpuProgress(error.to_string()))?;
        receiver
            .recv()
            .map_err(|error| FieldComputeError::GpuProgress(error.to_string()))?
            .map_err(|error| FieldComputeError::GpuProgress(error.to_string()))?;
        let mapped = readback.get_mapped_range(..);
        let values = mapped
            .as_chunks::<16>()
            .0
            .iter()
            .map(|record| {
                let word = |index: usize| {
                    let start = index * 4;
                    f32::from_le_bytes([
                        record[start],
                        record[start + 1],
                        record[start + 2],
                        record[start + 3],
                    ])
                };
                [word(0), word(1), word(2), word(3)]
            })
            .collect();
        drop(mapped);
        readback.unmap();
        Ok(values)
    }
}

fn validate_job(
    device: &wgpu::Device,
    job: &FieldComputeJob<'_>,
) -> Result<(u64, u64, u64, u32), FieldComputeError> {
    if job.wgsl.is_empty() || job.wgsl.len() > MAX_FIELD_SHADER_BYTES {
        return Err(FieldComputeError::InvalidRequest(
            "WGSL source size is invalid",
        ));
    }
    if job.input.is_empty() || job.parameters.is_empty() {
        return Err(FieldComputeError::InvalidRequest(
            "input and parameter storage arrays must be nonempty",
        ));
    }
    if job.output_records == 0 || job.generation == 0 {
        return Err(FieldComputeError::InvalidRequest(
            "output record count and generation must be nonzero",
        ));
    }
    if job.workgroup_size_x == 0 || job.workgroup_size_x > 256 {
        return Err(FieldComputeError::InvalidRequest(
            "workgroup_size_x must be in 1..=256",
        ));
    }
    if job.workgroup_size_x > device.limits().max_compute_workgroup_size_x
        || job.workgroup_size_x > device.limits().max_compute_invocations_per_workgroup
    {
        return Err(FieldComputeError::CapacityExceeded);
    }
    let input_bytes = byte_len(job.input.len())?;
    let parameter_bytes = byte_len(job.parameters.len())?;
    let (output_bytes, workgroups_x) = checked_output_layout(
        job.output_records,
        job.workgroup_size_x,
        device.limits().max_compute_workgroups_per_dimension,
    )?;
    let total_bytes = input_bytes
        .checked_add(parameter_bytes)
        .and_then(|bytes| bytes.checked_add(output_bytes))
        .ok_or(FieldComputeError::CapacityExceeded)?;
    if input_bytes > MAX_FIELD_INPUT_BYTES as u64
        || parameter_bytes > MAX_FIELD_INPUT_BYTES as u64
        || output_bytes > MAX_FIELD_OUTPUT_BYTES as u64
        || total_bytes > MAX_FIELD_JOB_BYTES as u64
        || input_bytes > u64::from(device.limits().max_storage_buffer_binding_size)
        || parameter_bytes > u64::from(device.limits().max_storage_buffer_binding_size)
        || output_bytes > u64::from(device.limits().max_storage_buffer_binding_size)
        || [input_bytes, parameter_bytes, output_bytes]
            .iter()
            .any(|bytes| *bytes > device.limits().max_buffer_size)
    {
        return Err(FieldComputeError::CapacityExceeded);
    }
    Ok((input_bytes, parameter_bytes, output_bytes, workgroups_x))
}

fn validate_wgsl_workgroup(wgsl: &str, expected_x: u32) -> Result<(), FieldComputeError> {
    let module = naga::front::wgsl::parse_str(wgsl)
        .map_err(|error| FieldComputeError::ShaderValidation(error.emit_to_string(wgsl)))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|error| FieldComputeError::ShaderValidation(error.to_string()))?;
    let entry = module
        .entry_points
        .iter()
        .find(|entry| entry.name == "main" && entry.stage == naga::ShaderStage::Compute)
        .ok_or(FieldComputeError::InvalidRequest(
            "WGSL must define a compute entry point named main",
        ))?;
    if entry.workgroup_size_overrides.is_some() {
        return Err(FieldComputeError::InvalidRequest(
            "WGSL workgroup-size overrides are not supported",
        ));
    }
    if entry.workgroup_size != [expected_x, 1, 1] {
        return Err(FieldComputeError::InvalidRequest(
            "WGSL main workgroup size must equal the requested x size and use y=z=1",
        ));
    }
    Ok(())
}

fn checked_output_layout(
    output_records: u32,
    workgroup_size_x: u32,
    max_workgroups_x: u32,
) -> Result<(u64, u32), FieldComputeError> {
    if output_records == 0 || workgroup_size_x == 0 {
        return Err(FieldComputeError::InvalidRequest(
            "output record count and workgroup size must be nonzero",
        ));
    }
    let output_bytes = u64::from(output_records)
        .checked_mul(16)
        .ok_or(FieldComputeError::CapacityExceeded)?;
    let workgroups_x = output_records
        .checked_add(workgroup_size_x - 1)
        .ok_or(FieldComputeError::CapacityExceeded)?
        / workgroup_size_x;
    if workgroups_x > max_workgroups_x {
        return Err(FieldComputeError::CapacityExceeded);
    }
    Ok((output_bytes, workgroups_x))
}

fn byte_len(elements: usize) -> Result<u64, FieldComputeError> {
    u64::try_from(elements)
        .ok()
        .and_then(|count| count.checked_mul(4))
        .ok_or(FieldComputeError::CapacityExceeded)
}

fn pack_f32(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
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

fn buffer_entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

#[cfg(test)]
mod tests {
    use super::{FieldComputeError, checked_output_layout, validate_wgsl_workgroup};

    const VALID_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read> parameters: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<vec4<f32>>;
@compute @workgroup_size(4)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < arrayLength(&output)) {
        let value = input[id.x] * parameters[0];
        output[id.x] = vec4<f32>(value);
    }
}
"#;

    #[test]
    fn output_layout_ceil_divides_and_checks_device_workgroup_limit() {
        assert_eq!(checked_output_layout(5, 4, 2).unwrap(), (80, 2));
        assert!(matches!(
            checked_output_layout(9, 4, 2),
            Err(FieldComputeError::CapacityExceeded)
        ));
    }

    #[test]
    fn output_layout_rejects_workgroup_count_overflow() {
        assert!(matches!(
            checked_output_layout(u32::MAX, 2, u32::MAX),
            Err(FieldComputeError::CapacityExceeded)
        ));
    }

    #[test]
    fn shader_workgroup_shape_must_match_the_dispatch_contract() {
        assert!(validate_wgsl_workgroup(VALID_SHADER, 4).is_ok());
        assert!(matches!(
            validate_wgsl_workgroup(VALID_SHADER, 8),
            Err(FieldComputeError::InvalidRequest(_))
        ));
    }

    #[test]
    fn shader_rejects_multidimensional_workgroups_and_size_overrides() {
        let two_dimensional = VALID_SHADER.replace("@workgroup_size(4)", "@workgroup_size(4, 2)");
        assert!(matches!(
            validate_wgsl_workgroup(&two_dimensional, 4),
            Err(FieldComputeError::InvalidRequest(_))
        ));

        let override_size = VALID_SHADER
            .replace(
                "@group(0) @binding(0)",
                "override WORKGROUP_SIZE: u32 = 4;\n@group(0) @binding(0)",
            )
            .replace("@workgroup_size(4)", "@workgroup_size(WORKGROUP_SIZE)");
        assert!(matches!(
            validate_wgsl_workgroup(&override_size, 4),
            Err(FieldComputeError::InvalidRequest(_))
        ));
    }
}
