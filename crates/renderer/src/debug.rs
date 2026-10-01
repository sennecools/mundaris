//! Minimal line-list renderer and explicit safe little-endian GPU packing.

use crate::{PreparedView, RenderPreparationError, RenderRelativePosition};
use glam::Mat4;
use mundaris_math::{FrameId, FramePosition};

const STRIDE: u64 = 32;
const ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
const SHADER: &str = include_str!("shaders/debug.wgsl");

/// Validated right-handed, 0..1 forward-depth projection; column-major GPU matrix.
#[derive(Debug, Clone, Copy)]
pub struct DebugProjection {
    matrix: Mat4,
}
impl DebugProjection {
    pub fn try_new(
        vertical_fov_rad: f64,
        aspect: f64,
        near_m: f64,
        far_m: f64,
    ) -> Result<Self, RenderPreparationError> {
        if ![vertical_fov_rad, aspect, near_m, far_m]
            .iter()
            .all(|value| value.is_finite())
            || !(0.0..std::f64::consts::PI).contains(&vertical_fov_rad)
            || vertical_fov_rad == 0.0
            || aspect <= 0.0
            || near_m <= 0.0
            || far_m <= near_m
        {
            return Err(RenderPreparationError::InvalidProjection);
        }
        let [fov, aspect, near, far] =
            [vertical_fov_rad, aspect, near_m, far_m].map(|value| value as f32);
        if ![fov, aspect, near, far]
            .iter()
            .all(|value| value.is_finite())
            || fov <= 0.0
            || fov >= std::f32::consts::PI
            || aspect <= 0.0
            || near <= 0.0
            || far <= near
        {
            return Err(RenderPreparationError::InvalidProjection);
        }
        let matrix = Mat4::perspective_rh(fov, aspect, near, far);
        if !matrix.is_finite() {
            return Err(RenderPreparationError::InvalidProjection);
        }
        Ok(Self { matrix })
    }
    pub fn near_debug(width: u32, height: u32) -> Result<Self, RenderPreparationError> {
        if width == 0 || height == 0 {
            return Err(RenderPreparationError::InvalidProjection);
        }
        Self::try_new(
            60.0_f64.to_radians(),
            f64::from(width) / f64::from(height),
            0.05,
            10_000.0,
        )
    }
    pub fn gpu_columns(self) -> [f32; 16] {
        self.matrix.to_cols_array()
    }
    pub fn gpu_bytes(self) -> [u8; 64] {
        let mut bytes = [0; 64];
        for (value, output) in self.gpu_columns().iter().zip(bytes.as_chunks_mut::<4>().0) {
            output.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
}

/// App-supplied finite f64 line endpoints. Colors are validated before packing.
#[derive(Debug, Clone, Copy)]
pub struct DebugLine {
    pub endpoints: [FramePosition; 2],
    pub color: [f32; 4],
}

/// Reused byte storage. Each new DebugFrame clears it and binds it to one view.
#[derive(Default)]
pub struct DebugStaging {
    bytes: Vec<u8>,
}

/// Full-frame preparation, retaining the view/tree borrow through submission.
/// No relative vertices from another view can be appended. Any preparation error
/// poisons the frame: partial batches cannot be uploaded. Build a fresh frame
/// after every observer/state change; this buffer is never authoritative state.
pub struct DebugFrame<'view, 'tree, 'storage> {
    view: &'view PreparedView<'tree>,
    staging: &'storage mut DebugStaging,
    projection: DebugProjection,
    failed: bool,
    max_error_m: f64,
}
impl<'view, 'tree, 'storage> DebugFrame<'view, 'tree, 'storage> {
    pub fn new(
        view: &'view PreparedView<'tree>,
        staging: &'storage mut DebugStaging,
        projection: DebugProjection,
    ) -> Self {
        staging.bytes.clear();
        Self {
            view,
            staging,
            projection,
            failed: false,
            max_error_m: 0.0,
        }
    }
    /// Group by source frame; prepares centering once. Output is unusable on failure.
    pub fn append_lines(
        &mut self,
        source: FrameId,
        lines: &[DebugLine],
    ) -> Result<(), RenderPreparationError> {
        let result = self.append_checked(source, lines);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn append_checked(
        &mut self,
        source: FrameId,
        lines: &[DebugLine],
    ) -> Result<(), RenderPreparationError> {
        if self.failed {
            return Err(RenderPreparationError::FailedDebugFrame);
        }
        let prepared = self.view.prepare_source(source)?;
        let base = self.staging.bytes.len() / STRIDE as usize;
        for (line_index, line) in lines.iter().enumerate() {
            if !line.color.iter().all(|value| value.is_finite()) {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            for (endpoint_index, point) in line.endpoints.iter().enumerate() {
                let position = prepared.try_position(*point).map_err(|source| {
                    RenderPreparationError::Vertex {
                        index: base + line_index * 2 + endpoint_index,
                        source: Box::new(source),
                    }
                })?;
                self.max_error_m = self.max_error_m.max(position.max_component_error_m());
                pack_vertex(position, line.color, &mut self.staging.bytes);
            }
        }
        Ok(())
    }
    pub fn vertex_count(&self) -> usize {
        self.staging.bytes.len() / STRIDE as usize
    }
    pub fn max_component_error_m(&self) -> f64 {
        self.max_error_m
    }
    fn validated_bytes(&self) -> Result<&[u8], RenderPreparationError> {
        if self.failed {
            Err(RenderPreparationError::FailedDebugFrame)
        } else {
            Ok(&self.staging.bytes)
        }
    }
}

fn pack_vertex(position: RenderRelativePosition, color: [f32; 4], bytes: &mut Vec<u8>) {
    let [x, y, z] = position.gpu_xyz();
    for value in [x, y, z, 1.0].into_iter().chain(color) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

pub(crate) struct DebugRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    capacity: u64,
    depth: wgpu::TextureView,
}
impl DebugRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Debug projection"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Debug projection layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Debug projection binding"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Debug pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("View-relative debug shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Debug lines"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &ATTRIBUTES,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let capacity = 4096;
        let vertices = Self::vertex_buffer(device, capacity);
        Self {
            pipeline,
            uniform,
            bind_group,
            vertices,
            capacity,
            depth: Self::depth(device, width, height),
        }
    }
    fn vertex_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Debug line vertices"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
    fn depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Debug forward depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }
    pub(crate) fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.depth = Self::depth(device, width, height);
    }
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        frame: &DebugFrame<'_, '_, '_>,
    ) -> Result<(), RenderPreparationError> {
        let bytes = frame.validated_bytes()?;
        let count = u32::try_from(frame.vertex_count())
            .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
        if bytes.len() as u64 > self.capacity {
            self.capacity = (bytes.len() as u64).next_power_of_two();
            self.vertices = Self::vertex_buffer(device, self.capacity);
        }
        queue.write_buffer(&self.uniform, 0, &frame.projection.gpu_bytes());
        if !bytes.is_empty() {
            queue.write_buffer(&self.vertices, 0, bytes);
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("View-relative debug line pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.025,
                        g: 0.035,
                        b: 0.06,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..count, 0..1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{DVec3, Vec4};
    use mundaris_math::*;
    use std::num::NonZeroU64;
    #[test]
    fn projection_layout_and_shader_contract() {
        let projection = DebugProjection::near_debug(1280, 800).unwrap();
        for (distance, expected) in [(0.05, 0.0), (10_000.0, 1.0)] {
            let clip = projection.matrix * Vec4::new(0.0, 0.0, -distance, 1.0);
            assert!(clip.w > 0.0);
            assert!((clip.z / clip.w - expected).abs() <= 2e-6);
        }
        assert!((projection.matrix * Vec4::new(1.0, 1.0, -5.0, 1.0)).x > 0.0);
        assert!((projection.matrix * Vec4::new(1.0, 1.0, -5.0, 1.0)).y > 0.0);
        for (value, bytes) in projection
            .gpu_columns()
            .iter()
            .zip(projection.gpu_bytes().as_chunks::<4>().0)
        {
            assert_eq!(*value, f32::from_le_bytes(*bytes));
        }
        assert_eq!(STRIDE, 32);
        assert_eq!(ATTRIBUTES[0].offset, 0);
        assert_eq!(ATTRIBUTES[1].offset, 16);
        let module = naga::front::wgsl::parse_str(SHADER).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
        assert_eq!(module.entry_points.len(), 2);
    }
    #[test]
    fn invalid_projection_inputs() {
        for value in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
            assert!(DebugProjection::try_new(value, 1.0, 0.05, 10_000.0).is_err());
            assert!(DebugProjection::try_new(1.0, value, 0.05, 10_000.0).is_err());
            assert!(DebugProjection::try_new(1.0, 1.0, value, 10_000.0).is_err());
            assert!(DebugProjection::try_new(1.0, 1.0, 0.05, value).is_err());
        }
        assert!(DebugProjection::try_new(std::f64::consts::PI, 1.0, 0.05, 10_000.0).is_err());
        assert!(DebugProjection::try_new(1.0, 1.0, 2.0, 1.0).is_err());
        assert!(DebugProjection::try_new(1.0, f64::MAX, 0.05, 10_000.0).is_err());
    }
    #[test]
    fn safe_vertex_packing_and_failed_frame_cannot_submit() {
        let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let pose = FramePose::new(
            FramePosition::new(root, LocalPosition::origin()),
            UnitRotation::identity(),
        );
        let view = PreparedView::new(
            &tree.evaluate(),
            pose,
            crate::RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let point = FramePosition::new(
            root,
            LocalPosition::try_metres(DVec3::new(1.0, 2.0, -5.0)).unwrap(),
        );
        let mut staging = DebugStaging::default();
        let mut frame = DebugFrame::new(
            &view,
            &mut staging,
            DebugProjection::near_debug(1280, 800).unwrap(),
        );
        frame
            .append_lines(
                root,
                &[DebugLine {
                    endpoints: [point, point],
                    color: [0.1, 0.2, 0.3, 1.0],
                }],
            )
            .unwrap();
        assert_eq!(frame.vertex_count(), 2);
        let bytes = frame.validated_bytes().unwrap();
        assert_eq!(bytes.len(), 64);
        let values: Vec<_> = bytes[..32]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        assert_eq!(values, [1.0, 2.0, -5.0, 1.0, 0.1, 0.2, 0.3, 1.0]);
        assert!(
            frame
                .append_lines(
                    root,
                    &[DebugLine {
                        endpoints: [point, point],
                        color: [f32::NAN; 4]
                    }]
                )
                .is_err()
        );
        assert!(frame.validated_bytes().is_err());
    }
}
