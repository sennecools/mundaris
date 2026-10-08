//! Bounded conventional-compute cluster selection and indirect indexed draws.
//!
//! This adapter is opt-in. It owns only derived GPU data and retires/reuses a
//! region slot after the queue completion callback for its last submission.

use crate::{
    cluster::{
        ClusterDebug, ClusterMode, ClusterReport, ClusterSettings, GPU_CAP_BYTES, REGION_CAP,
        RegionView,
    },
    cluster_region::{
        Cluster, ClusterLevel, RegionProduct, RegionVertex, TransitionMesh, TransitionVertex,
    },
};
use glam::{DMat3, DVec3};
use std::{
    collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use thiserror::Error;
use wgpu::util::DeviceExt;

const MAX_COMMANDS: usize = 8_192;
const MAX_CLUSTER_BYTES: u64 = 64 * 1024 * 1024;
// CPU expansion is separate from the builder's scratch pool and stays small;
// larger products simply remain on the resident-reference path.
const MAX_PACK_SCRATCH_BYTES: usize = 8 * 1024 * 1024;
const CLUSTER_META_BYTES: u64 = 32;
const SELECT_PARAMS_BYTES: u64 = 176;
const DRAW_PARAMS_BYTES: u64 = 224;
const INDIRECT_BYTES: usize = 20;
const RENDER_VERTEX_BYTES: usize = 128;
const TRANSITION_TIME: Duration = Duration::from_millis(250);

#[derive(Debug, Error)]
pub(crate) enum ClusterGpuError {
    #[error("cluster region product is invalid or has an unsupported replacement set")]
    InvalidProduct,
    #[error("cluster GPU data exceeds the fixed byte or command budget")]
    ResourceCap,
    #[error("all region slots are pinned or still in flight")]
    SlotsUnavailable,
}

#[derive(Clone, Copy)]
struct ClusterMetaCpu {
    sphere: [f32; 4],
    first_index: u32,
    index_count: u32,
    base_vertex: u32,
    detail_set: u32,
}

struct PackedRegion {
    vertices: Vec<u8>,
    indices: Vec<u8>,
    metadata: Vec<u8>,
    commands: Vec<u8>,
    command_count: usize,
    cluster_refs: Vec<ClusterRef>,
    sphere_radius: f32,
    coarse_error_m: f32,
    coarse_eligible: bool,
    gpu_bytes: u64,
}

struct GpuRegionSlot {
    product: Arc<RegionProduct>,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    _metadata_buffer: wgpu::Buffer,
    indirect_buffer: wgpu::Buffer,
    select_params: wgpu::Buffer,
    draw_params: wgpu::Buffer,
    select_group: wgpu::BindGroup,
    render_group: wgpu::BindGroup,
    command_count: usize,
    cluster_refs: Vec<ClusterRef>,
    sphere_radius: f32,
    coarse_error_m: f32,
    coarse_eligible: bool,
    gpu_bytes: u64,
    last_use_submission: u64,
    pinned: bool,
    cut: CutState,
}

#[derive(Clone, Copy)]
struct CutState {
    stable_morph: f32,
    start_morph: f32,
    target_morph: f32,
    started: Option<Instant>,
}

impl Default for CutState {
    fn default() -> Self {
        Self {
            stable_morph: 1.0,
            start_morph: 1.0,
            target_morph: 1.0,
            started: None,
        }
    }
}

impl CutState {
    fn current_morph(self, now: Instant) -> f32 {
        let Some(started) = self.started else {
            return self.stable_morph;
        };
        let t = (now.duration_since(started).as_secs_f32() / TRANSITION_TIME.as_secs_f32())
            .clamp(0.0, 1.0);
        self.start_morph + (self.target_morph - self.start_morph) * t
    }

    fn advance(&mut self, target_morph: f32, now: Instant, can_transition: bool) -> (f32, bool) {
        let current = self.current_morph(now);
        if let Some(started) = self.started {
            if (target_morph - self.target_morph).abs() > f32::EPSILON {
                self.start_morph = current;
                self.target_morph = target_morph;
                self.started = Some(now);
            } else if now.duration_since(started) >= TRANSITION_TIME {
                self.stable_morph = self.target_morph;
                self.start_morph = self.target_morph;
                self.started = None;
            }
        } else if (target_morph - self.stable_morph).abs() > f32::EPSILON {
            if can_transition {
                self.start_morph = self.stable_morph;
                self.target_morph = target_morph;
                self.started = Some(now);
            } else {
                self.stable_morph = target_morph;
                self.start_morph = target_morph;
            }
        }
        let morph = self.current_morph(now);
        if self.started.is_some() && now.duration_since(self.started.unwrap()) >= TRANSITION_TIME {
            self.stable_morph = self.target_morph;
            self.start_morph = self.target_morph;
            self.started = None;
            return (self.stable_morph, false);
        }
        (morph, self.started.is_some())
    }

    fn resume(&mut self, morph: f32, target: f32, active: bool, now: Instant) {
        self.stable_morph = morph;
        self.start_morph = morph;
        self.target_morph = target;
        self.started = active.then_some(now);
    }
}

#[derive(Clone, Copy)]
struct FrozenCut {
    morph: f32,
    target_morph: f32,
    active: bool,
    selected_set: u32,
}

struct FrozenSelection {
    projection: crate::CelestialProjection,
    views: BTreeMap<usize, RegionView>,
    cuts: BTreeMap<usize, FrozenCut>,
}

#[derive(Clone, Copy)]
struct EdgeUse {
    key: u64,
    triangle: u32,
    cluster: u32,
    component: u8,
}

pub(crate) struct ClusterGpu {
    format: wgpu::TextureFormat,
    select_layout: wgpu::BindGroupLayout,
    draw_layout: wgpu::BindGroupLayout,
    select_pipeline_layout: wgpu::PipelineLayout,
    draw_pipeline_layout: wgpu::PipelineLayout,
    select_pipeline: Option<wgpu::ComputePipeline>,
    draw_pipeline: Option<wgpu::RenderPipeline>,
    slots: Vec<Option<GpuRegionSlot>>,
    gpu_bytes: u64,
    prepared_slots: BTreeSet<usize>,
    selected_slots: BTreeSet<usize>,
    completed_submission: Arc<AtomicU64>,
    submission_serial: u64,
    frozen: Option<FrozenSelection>,
    report: ClusterReport,
    upload_bytes: u64,
    rebuilds: u64,
    cache_hits: u64,
    last_reason: Option<String>,
}

impl ClusterGpu {
    /// Build only fixed layouts here. Pipeline compilation is lazy until the
    /// first prototype product is admitted; Reference mode allocates no regions.
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        projection_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let select_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Finite cluster selection resources"),
            entries: &[
                buffer_layout(
                    0,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Uniform,
                    SELECT_PARAMS_BYTES,
                ),
                buffer_layout(
                    1,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: true },
                    CLUSTER_META_BYTES,
                ),
                buffer_layout(
                    2,
                    wgpu::ShaderStages::COMPUTE,
                    wgpu::BufferBindingType::Storage { read_only: false },
                    INDIRECT_BYTES as u64,
                ),
            ],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Finite cluster draw material and visual settings"),
            entries: &[buffer_layout(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                wgpu::BufferBindingType::Uniform,
                DRAW_PARAMS_BYTES,
            )],
        });
        let select_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Finite cluster selection pipeline layout"),
                bind_group_layouts: &[&select_layout],
                push_constant_ranges: &[],
            });
        let draw_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Finite cluster render pipeline layout"),
            bind_group_layouts: &[projection_layout, &draw_layout],
            push_constant_ranges: &[],
        });
        Self {
            format,
            select_layout,
            draw_layout,
            select_pipeline_layout,
            draw_pipeline_layout,
            select_pipeline: None,
            draw_pipeline: None,
            slots: std::iter::repeat_with(|| None).take(REGION_CAP).collect(),
            gpu_bytes: 0,
            prepared_slots: BTreeSet::new(),
            selected_slots: BTreeSet::new(),
            completed_submission: Arc::new(AtomicU64::new(0)),
            submission_serial: 0,
            frozen: None,
            report: ClusterReport::default(),
            upload_bytes: 0,
            rebuilds: 0,
            cache_hits: 0,
            last_reason: None,
        }
    }

    fn ensure_pipelines(&mut self, device: &wgpu::Device) {
        if self.select_pipeline.is_some() && self.draw_pipeline.is_some() {
            return;
        }
        let select_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Finite cluster frustum and projected-detail selection"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/cluster_select.wgsl").into()),
        });
        self.select_pipeline = Some(device.create_compute_pipeline(
            &wgpu::ComputePipelineDescriptor {
                label: Some("Finite cluster bounded selection"),
                layout: Some(&self.select_pipeline_layout),
                module: &select_shader,
                entry_point: Some("select_main"),
                compilation_options: Default::default(),
                cache: None,
            },
        ));
        let draw_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Finite cluster conventional indirect rasterization"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/cluster_render.wgsl").into()),
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x3,
            4 => Float32x4, 5 => Float32x4, 6 => Float32x2, 7 => Float32x3,
            8 => Float32x3, 9 => Float32x3, 10 => Uint32
        ];
        self.draw_pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Finite cluster reverse-Z renderer"),
                layout: Some(&self.draw_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &draw_shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: RENDER_VERTEX_BYTES as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &draw_shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::GreaterEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            }),
        );
    }

    /// Start a new renderer frame before region admission. Slots returned by
    /// `prepare_region` stay protected through the matching queue submission.
    pub(crate) fn begin_frame(&mut self) {
        self.selected_slots.clear();
        self.prepared_slots.clear();
        self.upload_bytes = 0;
    }

    /// Admit an immutable product into a free or completion-retired slot.
    /// Matching key + boundary version returns the existing resident slot.
    pub(crate) fn prepare_region(
        &mut self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        product: &Arc<RegionProduct>,
    ) -> Result<usize, ClusterGpuError> {
        self.ensure_pipelines(device);
        if let Some((index, _)) = self.slots.iter().enumerate().find(|(_, slot)| {
            slot.as_ref().is_some_and(|resident| {
                resident.product.input.key == product.input.key
                    && resident.product.input.boundary_version == product.input.boundary_version
            })
        }) {
            self.cache_hits = self.cache_hits.saturating_add(1);
            self.prepared_slots.insert(index);
            self.last_reason = None;
            return Ok(index);
        }
        if self.frozen.is_some() {
            self.last_reason = Some("freeze_pins_current_region_products".into());
            return Err(ClusterGpuError::SlotsUnavailable);
        }
        let packed = match pack_region(product) {
            Ok(packed) => packed,
            Err(error) => {
                self.last_reason = Some(error.to_string());
                return Err(error);
            }
        };
        let completed = self.completed_submission.load(Ordering::Acquire);
        let slot_index = self
            .slots
            .iter()
            .position(Option::is_none)
            .or_else(|| {
                self.slots.iter().enumerate().position(|(index, slot)| {
                    !self.prepared_slots.contains(&index)
                        && slot.as_ref().is_some_and(|resident| {
                            !resident.pinned && resident.last_use_submission <= completed
                        })
                })
            })
            .ok_or_else(|| {
                self.last_reason = Some("all_region_slots_pinned_or_in_flight".into());
                ClusterGpuError::SlotsUnavailable
            })?;
        let reclaim = self.slots[slot_index]
            .as_ref()
            .map_or(0, |slot| slot.gpu_bytes);
        let next_total = self
            .gpu_bytes
            .saturating_sub(reclaim)
            .saturating_add(packed.gpu_bytes);
        let limits = device.limits();
        let largest_binding = packed.metadata.len().max(packed.commands.len()) as u64;
        let largest_buffer = packed
            .vertices
            .len()
            .max(packed.indices.len())
            .max(packed.metadata.len())
            .max(packed.commands.len()) as u64;
        if next_total > MAX_CLUSTER_BYTES
            || next_total > GPU_CAP_BYTES
            || largest_buffer > limits.max_buffer_size
            || largest_binding > limits.max_storage_buffer_binding_size as u64
        {
            self.last_reason = Some("cluster_gpu_byte_cap_exceeded".into());
            return Err(ClusterGpuError::ResourceCap);
        }
        let new_gpu_bytes = packed.gpu_bytes;
        self.slots[slot_index] = None;
        self.gpu_bytes = self.gpu_bytes.saturating_sub(reclaim);
        let slot = create_gpu_slot(
            device,
            &self.select_layout,
            &self.draw_layout,
            Arc::clone(product),
            packed,
        )?;
        self.slots[slot_index] = Some(slot);
        self.prepared_slots.insert(slot_index);
        self.gpu_bytes = self.gpu_bytes.saturating_add(new_gpu_bytes);
        self.upload_bytes = self
            .upload_bytes
            .saturating_add(self.slots[slot_index].as_ref().unwrap().gpu_bytes);
        self.rebuilds = self.rebuilds.saturating_add(1);
        self.last_reason = None;
        Ok(slot_index)
    }

    /// Upload per-frame uniforms and encode every region's selection dispatch
    /// before the caller begins the shared scene render pass.
    pub(crate) fn encode_selection(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        projection: crate::CelestialProjection,
        views: &[RegionView],
        settings: ClusterSettings,
    ) {
        if settings.mode == ClusterMode::Reference {
            self.frozen = None;
            for slot in self.slots.iter_mut().filter_map(Option::as_mut) {
                slot.pinned = false;
            }
            self.report = ClusterReport {
                cpu_bytes: self
                    .retained_products()
                    .iter()
                    .map(|p| p.resident_bytes)
                    .sum::<usize>()
                    + self.auxiliary_cpu_bytes(),
                gpu_bytes: self.gpu_bytes,
                ..Default::default()
            };
            return;
        }
        if self.slots.iter().all(Option::is_none) {
            self.report = ClusterReport {
                enabled: true,
                active_mode: settings.mode,
                reason: Some("waiting for first region product".into()),
                ..Default::default()
            };
            return;
        }
        let now = Instant::now();
        if settings.freeze && self.frozen.is_none() {
            let mut frozen_views = BTreeMap::new();
            let mut cuts = BTreeMap::new();
            for view in views {
                let index = view.slot;
                if let Some(slot) = self.slots.get_mut(index).and_then(Option::as_mut) {
                    let target = if settings.mode == ClusterMode::Lod
                        && select_target(projection, *view, slot) == 1
                    {
                        0.0
                    } else {
                        1.0
                    };
                    let (morph, active) =
                        slot.cut
                            .advance(target, now, !slot.product.transitions.is_empty());
                    frozen_views.insert(index, *view);
                    cuts.insert(
                        index,
                        FrozenCut {
                            morph,
                            target_morph: slot.cut.target_morph,
                            active,
                            selected_set: if active {
                                2
                            } else if slot.cut.target_morph < 0.5 {
                                1
                            } else {
                                0
                            },
                        },
                    );
                    slot.pinned = true;
                }
            }
            self.frozen = Some(FrozenSelection {
                projection,
                views: frozen_views,
                cuts,
            });
        } else if !settings.freeze && self.frozen.is_some() {
            let frozen = self.frozen.take().unwrap();
            for (index, cut) in frozen.cuts {
                if let Some(slot) = self.slots.get_mut(index).and_then(Option::as_mut) {
                    slot.cut
                        .resume(cut.morph, cut.target_morph, cut.active, now);
                    slot.pinned = false;
                }
            }
        }

        let frozen = self.frozen.as_ref();
        let mut selected = 0usize;
        let mut selected_fine = 0usize;
        let mut selected_coarse = 0usize;
        let mut selected_transition = 0usize;
        let mut triangles = 0usize;
        let mut commands = 0usize;
        let mut pending = 0usize;
        let mut fallbacks = 0usize;
        let selection_started = Instant::now();

        for view in views {
            let index = view.slot;
            let Some(slot) = self.slots.get_mut(index).and_then(Option::as_mut) else {
                continue;
            };
            if view.quality_fallback {
                fallbacks += 1;
            }
            if view.pending_finer {
                pending += 1;
            }
            let frozen_view = frozen.and_then(|state| state.views.get(&index)).copied();
            let selection_view = frozen_view.unwrap_or(*view);
            let selection_projection = frozen.map_or(projection, |state| state.projection);
            let cut_snapshot = frozen.and_then(|state| state.cuts.get(&index)).copied();
            let (morph, transition_active) = if let Some(cut) = cut_snapshot {
                (cut.morph, cut.active)
            } else {
                let target = if settings.mode == ClusterMode::Lod
                    && select_target(selection_projection, selection_view, slot) == 1
                {
                    0.0
                } else {
                    1.0
                };
                slot.cut
                    .advance(target, now, !slot.product.transitions.is_empty())
            };

            let select_bytes = pack_selection_params(
                selection_projection,
                selection_view,
                slot,
                settings,
                transition_active,
                cut_snapshot.map(|cut| cut.selected_set),
            );
            let draw_bytes = pack_draw_params(*view, slot, settings, morph);
            queue.write_buffer(&slot.select_params, 0, &select_bytes);
            queue.write_buffer(&slot.draw_params, 0, &draw_bytes);
            let region_origin =
                region_origin_view(selection_view, slot.product.bounds_center_local);
            let region_visible = cluster_sphere_visible(
                selection_projection,
                view_to_f32(region_origin),
                slot.sphere_radius,
            );
            let selected_set = if let Some(cut) = cut_snapshot {
                cut.selected_set
            } else if transition_active {
                2
            } else if settings.mode == ClusterMode::Lod {
                select_target(selection_projection, selection_view, slot)
            } else {
                0
            };
            for cluster in &slot.cluster_refs {
                if cluster.detail_set == selected_set
                    && region_visible
                    && cluster_sphere_visible(
                        selection_projection,
                        cluster
                            .center_view(view_to_f32(region_origin), selection_view.body_to_view),
                        cluster.radius,
                    )
                {
                    self.selected_slots.insert(index);
                    selected += 1;
                    triangles += cluster.index_count as usize / 3;
                    match cluster.detail_set {
                        0 => selected_fine += 1,
                        1 => selected_coarse += 1,
                        2 => selected_transition += 1,
                        _ => {}
                    }
                }
            }
            commands = commands.saturating_add(slot.command_count);
            self.prepared_slots.insert(index);
        }
        let selection_micros = selection_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;

        if let Some(pipeline) = self.select_pipeline.as_ref() {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Finite cluster visibility and detail cut"),
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            for view in views {
                let index = view.slot;
                if let Some(slot) = self.slots.get(index).and_then(Option::as_ref) {
                    pass.set_bind_group(0, &slot.select_group, &[]);
                    pass.dispatch_workgroups(slot.command_count.div_ceil(64) as u32, 1, 1);
                }
            }
        }
        self.report.enabled = true;
        self.report.active_mode = settings.mode;
        self.report.freeze_active = self.frozen.is_some();
        self.report.resident_regions = self.slots.iter().filter(|slot| slot.is_some()).count();
        self.report.pending_regions = pending;
        self.report.selected_clusters = selected;
        self.report.selected_fine_clusters = selected_fine;
        self.report.selected_coarse_clusters = selected_coarse;
        self.report.selected_transition_clusters = selected_transition;
        self.report.resident_clusters = self
            .slots
            .iter()
            .filter_map(Option::as_ref)
            .map(|slot| slot.command_count)
            .sum();
        self.report.submitted_triangles = triangles;
        self.report.draw_commands = commands;
        self.report.cpu_bytes = self
            .slots
            .iter()
            .filter_map(Option::as_ref)
            .map(|slot| {
                slot.product
                    .resident_bytes
                    .saturating_add(slot.cluster_refs.capacity() * size_of::<ClusterRef>())
            })
            .sum();
        self.report.gpu_bytes = self.gpu_bytes;
        self.report.build_micros = self
            .slots
            .iter()
            .filter_map(Option::as_ref)
            .map(|slot| slot.product.build_micros)
            .sum();
        self.report.selection_cpu_micros = selection_micros;
        self.report.upload_bytes = self.upload_bytes;
        self.report.gpu_selection_ms = None;
        self.report.gpu_render_ms = None;
        self.report.fallback_regions = fallbacks;
        self.report.cache_hits = self.cache_hits;
        self.report.rebuilds = self.rebuilds;
        let mut reasons = BTreeSet::new();
        if let Some(reason) = &self.last_reason {
            reasons.insert(reason.clone());
        }
        for slot in self.slots.iter().filter_map(Option::as_ref) {
            if let Some(reason) = &slot.product.fallback_reason {
                reasons.insert(format!("fine_only:{reason}"));
            }
        }
        self.report.reason =
            (!reasons.is_empty()).then(|| reasons.into_iter().collect::<Vec<_>>().join("; "));
        self.report.counters_scope = "selected clusters/triangles mirror GPU predicates over widened packed f32 bounds; they are not GPU readbacks; draw_commands counts fixed indirect calls";
    }

    pub(crate) fn draw_region(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        projection_group: &wgpu::BindGroup,
        slot_index: usize,
    ) {
        let Some(pipeline) = self.draw_pipeline.as_ref() else {
            return;
        };
        let Some(slot) = self.slots.get(slot_index).and_then(Option::as_ref) else {
            return;
        };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, projection_group, &[]);
        pass.set_bind_group(1, &slot.render_group, &[]);
        pass.set_vertex_buffer(0, slot.vertex_buffer.slice(..));
        pass.set_index_buffer(slot.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for command in 0..slot.command_count {
            pass.draw_indexed_indirect(
                &slot.indirect_buffer,
                command as u64 * INDIRECT_BYTES as u64,
            );
        }
    }

    pub(crate) fn on_submitted(&mut self, queue: &wgpu::Queue) {
        if self.prepared_slots.is_empty() {
            return;
        }
        self.submission_serial = self.submission_serial.saturating_add(1);
        let serial = self.submission_serial;
        for index in std::mem::take(&mut self.prepared_slots) {
            if let Some(slot) = self.slots.get_mut(index).and_then(Option::as_mut) {
                slot.last_use_submission = serial;
            }
        }
        let completed = Arc::clone(&self.completed_submission);
        queue.on_submitted_work_done(move || {
            completed.fetch_max(serial, Ordering::Release);
        });
    }

    pub(crate) fn selected_slots(&self) -> &BTreeSet<usize> {
        &self.selected_slots
    }
    pub(crate) fn report(&self) -> ClusterReport {
        self.report.clone()
    }

    /// Products still retained by resident GPU slots, for the shared CPU
    /// product budget to include even after its own cache entry is evicted.
    pub(crate) fn retained_products(&self) -> Vec<Arc<RegionProduct>> {
        self.slots
            .iter()
            .filter_map(Option::as_ref)
            .map(|slot| Arc::clone(&slot.product))
            .collect()
    }
    pub(crate) fn auxiliary_cpu_bytes(&self) -> usize {
        self.slots
            .iter()
            .filter_map(Option::as_ref)
            .map(|s| s.cluster_refs.capacity() * size_of::<ClusterRef>())
            .sum()
    }
    pub(crate) fn retire_unwanted(&mut self, wanted: &[(crate::TileKey, u64)]) {
        let completed = self.completed_submission.load(Ordering::Acquire);
        for slot in &mut self.slots {
            let retire = slot.as_ref().is_some_and(|s| {
                !s.pinned
                    && s.last_use_submission <= completed
                    && !wanted.iter().any(|(key, version)| {
                        *key == s.product.input.key && *version == s.product.input.boundary_version
                    })
            });
            if retire && let Some(old) = slot.take() {
                self.gpu_bytes = self.gpu_bytes.saturating_sub(old.gpu_bytes);
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ClusterRef {
    center: [f32; 3],
    radius: f32,
    index_count: u32,
    detail_set: u32,
}

impl ClusterRef {
    fn center_view(self, origin: [f32; 3], basis: DMat3) -> [f32; 3] {
        let x = view_to_f32(basis.x_axis);
        let y = view_to_f32(basis.y_axis);
        let z = view_to_f32(basis.z_axis);
        let body = [
            x[0] * self.center[0] + y[0] * self.center[1] + z[0] * self.center[2],
            x[1] * self.center[0] + y[1] * self.center[1] + z[1] * self.center[2],
            x[2] * self.center[0] + y[2] * self.center[1] + z[2] * self.center[2],
        ];
        [
            origin[0] + body[0],
            origin[1] + body[1],
            origin[2] + body[2],
        ]
    }
}

fn cluster_sphere_visible(
    projection: crate::CelestialProjection,
    center: [f32; 3],
    radius: f32,
) -> bool {
    if center.iter().any(|value| !value.is_finite()) || !radius.is_finite() || radius < 0.0 {
        return false;
    }
    projection
        .frustum_planes()
        .into_iter()
        .all(|(normal, distance)| {
            let normal = view_to_f32(normal);
            let distance = distance as f32;
            normal[0] * center[0] + normal[1] * center[1] + normal[2] * center[2] + distance
                >= -radius
        })
}

fn select_target(
    projection: crate::CelestialProjection,
    view: RegionView,
    slot: &GpuRegionSlot,
) -> u32 {
    let center = view_to_f32(region_origin_view(view, slot.product.bounds_center_local));
    if !slot.coarse_eligible {
        return 0;
    }
    bool_u32(projected_error_below_limit(
        center,
        slot.sphere_radius,
        slot.coarse_error_m,
        projection.focal_pixels() as f32,
        projection.near_m() as f32,
    ))
}

fn projected_error_below_limit(
    center: [f32; 3],
    radius: f32,
    epsilon: f32,
    focal_pixels: f32,
    near_m: f32,
) -> bool {
    let dmin = -center[2] - radius - epsilon;
    if dmin <= near_m {
        return false;
    }
    let off_axis = center[0].abs().max(center[1].abs()) + radius + epsilon;
    let projected = (focal_pixels * epsilon / dmin) * (1.0 + off_axis / dmin);
    projected < crate::cluster::FINITE_MESH_ERROR_PX as f32
}

fn region_origin_view(view: RegionView, center_local: DVec3) -> DVec3 {
    view.anchor_view_m + view.body_to_view * center_local
}

fn view_to_f32(value: DVec3) -> [f32; 3] {
    [value.x as f32, value.y as f32, value.z as f32]
}

fn pack_selection_params(
    projection: crate::CelestialProjection,
    view: RegionView,
    slot: &GpuRegionSlot,
    settings: ClusterSettings,
    transition_active: bool,
    forced_set: Option<u32>,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(SELECT_PARAMS_BYTES as usize);
    for (normal, distance) in projection.frustum_planes() {
        push_vec4(
            &mut bytes,
            [
                normal.x as f32,
                normal.y as f32,
                normal.z as f32,
                distance as f32,
            ],
        );
    }
    let origin = view_to_f32(region_origin_view(view, slot.product.bounds_center_local));
    push_vec4(&mut bytes, [origin[0], origin[1], origin[2], 1.0]);
    push_mat3(&mut bytes, view.body_to_view);
    push_vec4(
        &mut bytes,
        [
            projection.focal_pixels() as f32,
            projection.near_m() as f32,
            slot.coarse_error_m,
            slot.sphere_radius,
        ],
    );
    push_uvec4(
        &mut bytes,
        [
            slot.command_count as u32,
            forced_set.map_or(bool_u32(settings.mode == ClusterMode::Lod), |set| set + 2),
            bool_u32(transition_active),
            bool_u32(slot.coarse_eligible),
        ],
    );
    bytes
}

fn pack_draw_params(
    view: RegionView,
    slot: &GpuRegionSlot,
    settings: ClusterSettings,
    morph: f32,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(DRAW_PARAMS_BYTES as usize);
    let origin = view_to_f32(region_origin_view(view, slot.product.bounds_center_local));
    push_vec4(&mut bytes, [origin[0], origin[1], origin[2], 1.0]);
    push_mat3(&mut bytes, view.body_to_view);
    push_vec4(
        &mut bytes,
        [
            view.sun_body.x as f32,
            view.sun_body.y as f32,
            view.sun_body.z as f32,
            0.0,
        ],
    );
    for color in view.appearance.natural_colors {
        push_vec4(&mut bytes, [color[0], color[1], color[2], 0.0]);
    }
    push_vec4(
        &mut bytes,
        [
            view.appearance.base_color[0],
            view.appearance.base_color[1],
            view.appearance.base_color[2],
            0.0,
        ],
    );
    push_vec4(
        &mut bytes,
        [
            view.appearance.dark_color[0],
            view.appearance.dark_color[1],
            view.appearance.dark_color[2],
            0.0,
        ],
    );
    push_vec4(
        &mut bytes,
        [
            view.appearance.ambient,
            view.appearance.diffuse,
            view.appearance.curvature_darkening,
            view.appearance.curvature_lightening,
        ],
    );
    let debug = match settings.debug {
        ClusterDebug::Lit => 0,
        ClusterDebug::Clusters => 1,
        ClusterDebug::Lod => 2,
        ClusterDebug::Residency => 3,
    };
    let residency = if view.quality_fallback {
        1
    } else if view.pending_finer {
        2
    } else {
        0
    };
    push_uvec4(
        &mut bytes,
        [
            debug,
            bool_u32(settings.triangle_edges),
            bool_u32(settings.cluster_edges),
            residency,
        ],
    );
    push_vec4(&mut bytes, [morph, 0.0, 0.0, 0.0]);
    bytes
}

fn pack_region(product: &RegionProduct) -> Result<PackedRegion, ClusterGpuError> {
    if product.levels.is_empty()
        || product.levels.len() > 2
        || product.transitions.len() > 1
        || product.input.vertices.is_empty()
        || !product.bounds_center_local.is_finite()
        || !product.bounds_radius_m.is_finite()
        || product.bounds_radius_m < 0.0
    {
        return Err(ClusterGpuError::InvalidProduct);
    }
    let mut command_count = product
        .levels
        .iter()
        .map(|level| level.clusters.len())
        .sum::<usize>()
        .saturating_add(
            product
                .transitions
                .iter()
                .map(|transition| transition.clusters.len())
                .sum::<usize>(),
        );
    if command_count == 0 || command_count > MAX_COMMANDS {
        return Err(ClusterGpuError::ResourceCap);
    }
    let index_count = product
        .levels
        .iter()
        .map(|level| level.indices.len())
        .sum::<usize>()
        .saturating_add(
            product
                .transitions
                .iter()
                .map(|transition| transition.indices.len())
                .sum::<usize>(),
        );
    let vertex_bytes = index_count
        .checked_mul(RENDER_VERTEX_BYTES)
        .ok_or(ClusterGpuError::ResourceCap)?;
    let index_bytes = index_count
        .checked_mul(4)
        .ok_or(ClusterGpuError::ResourceCap)?;
    let meta_bytes = command_count
        .checked_mul(CLUSTER_META_BYTES as usize)
        .ok_or(ClusterGpuError::ResourceCap)?;
    let indirect_bytes = command_count
        .checked_mul(INDIRECT_BYTES)
        .ok_or(ClusterGpuError::ResourceCap)?;
    let scratch = vertex_bytes
        .saturating_add(index_bytes)
        .saturating_add(meta_bytes.saturating_mul(2))
        .saturating_add(indirect_bytes)
        .saturating_add(command_count.saturating_mul(size_of::<ClusterRef>()))
        .saturating_add(index_count.saturating_mul(size_of::<EdgeUse>()))
        .saturating_add(index_count) // exact cluster-range coverage bitmap
        .saturating_add(index_count / 3); // triangle edge masks
    if scratch > MAX_PACK_SCRATCH_BYTES {
        return Err(ClusterGpuError::ResourceCap);
    }
    let gpu_bytes = (vertex_bytes
        .saturating_add(index_bytes)
        .saturating_add(meta_bytes)
        .saturating_add(indirect_bytes) as u64)
        .saturating_add(SELECT_PARAMS_BYTES)
        .saturating_add(DRAW_PARAMS_BYTES);
    if gpu_bytes > MAX_CLUSTER_BYTES || gpu_bytes > GPU_CAP_BYTES {
        return Err(ClusterGpuError::ResourceCap);
    }

    let mut vertices = Vec::with_capacity(vertex_bytes);
    let mut indices = Vec::with_capacity(index_bytes);
    let mut command_bytes = vec![0u8; indirect_bytes];
    let mut metas: Vec<ClusterMetaCpu> = Vec::with_capacity(command_count);
    let mut sphere_radius = conservative_f32(product.bounds_radius_m)?;
    let center = product.bounds_center_local;

    for (set, level) in product.levels.iter().take(2).enumerate() {
        append_level(
            product,
            level,
            set as u32,
            &mut vertices,
            &mut indices,
            &mut metas,
            &mut sphere_radius,
        )?;
    }
    for transition in &product.transitions {
        append_transition(
            product,
            transition,
            &mut vertices,
            &mut indices,
            &mut metas,
            &mut sphere_radius,
        )?;
    }
    command_count = metas.len();
    if command_count == 0
        || vertices.len() != vertex_bytes
        || indices.len() != index_bytes
        || metas.len() * CLUSTER_META_BYTES as usize != meta_bytes
    {
        return Err(ClusterGpuError::InvalidProduct);
    }
    let mut metadata = Vec::with_capacity(meta_bytes);
    let mut cluster_refs = Vec::with_capacity(command_count);
    for meta in &metas {
        for value in meta.sphere {
            metadata.extend_from_slice(&value.to_le_bytes());
        }
        cluster_refs.push(ClusterRef {
            center: [meta.sphere[0], meta.sphere[1], meta.sphere[2]],
            radius: meta.sphere[3],
            index_count: meta.index_count,
            detail_set: meta.detail_set,
        });
        for value in [
            meta.first_index,
            meta.index_count,
            meta.base_vertex,
            meta.detail_set,
        ] {
            metadata.extend_from_slice(&value.to_le_bytes());
        }
    }
    let coarse = product.levels.get(1);
    let coarse_error = coarse
        .and_then(|level| level.mesh_deviation_bound_m)
        .unwrap_or(0.0) as f32;
    let coarse_eligible = coarse.is_some_and(|level| {
        level
            .mesh_deviation_bound_m
            .is_some_and(|v| v.is_finite() && v >= 0.0)
            && level
                .normal_deviation_bound_rad
                .is_some_and(|v| v.is_finite() && v <= crate::cluster::FINITE_NORMAL_ERROR_RAD)
            && level
                .material_deviation_bound
                .is_some_and(|v| v.is_finite() && v <= crate::cluster::FINITE_MATERIAL_ERROR)
            && !product.transitions.is_empty()
    });
    let _ = center;
    Ok(PackedRegion {
        vertices,
        indices,
        metadata,
        commands: std::mem::take(&mut command_bytes),
        command_count,
        cluster_refs,
        sphere_radius,
        coarse_error_m: coarse_error,
        coarse_eligible,
        gpu_bytes,
    })
}

fn append_level(
    product: &RegionProduct,
    level: &ClusterLevel,
    detail_set: u32,
    vertices: &mut Vec<u8>,
    indices: &mut Vec<u8>,
    metadata: &mut Vec<ClusterMetaCpu>,
    region_radius: &mut f32,
) -> Result<(), ClusterGpuError> {
    let triangle_masks = boundary_masks(
        &level.indices,
        &level.clusters,
        product.input.vertices.len(),
    )?;
    for (cluster_ordinal, cluster) in level.clusters.iter().enumerate() {
        let start = cluster.first_index as usize;
        let end = start
            .checked_add(cluster.index_count as usize)
            .ok_or(ClusterGpuError::InvalidProduct)?;
        if cluster.index_count == 0 || cluster.index_count % 3 != 0 || end > level.indices.len() {
            return Err(ClusterGpuError::InvalidProduct);
        }
        let first_index =
            u32::try_from(indices.len() / 4).map_err(|_| ClusterGpuError::ResourceCap)?;
        let color = stable_cluster_color(product, cluster.stable_id);
        let lod = lod_color(detail_set);
        let mut cluster_radius = cluster.sphere_radius_m;
        for triangle_start in (start..end).step_by(3) {
            let mask = triangle_masks[triangle_start / 3];
            for corner in 0..3 {
                let source_index = level.indices[triangle_start + corner] as usize;
                let source = product
                    .input
                    .vertices
                    .get(source_index)
                    .ok_or(ClusterGpuError::InvalidProduct)?;
                let vertex = RenderVertex::same(
                    source,
                    product.bounds_center_local,
                    barycentric(corner),
                    color,
                    lod,
                    mask,
                    detail_set,
                );
                validate_render_vertex(vertex)?;
                *region_radius = region_radius.max(conservative_length_f32(
                    source.position - product.bounds_center_local,
                )?);
                cluster_radius =
                    cluster_radius.max(source.position.distance(cluster.sphere_center_local));
                push_render_vertex(vertices, vertex);
                let packed_index =
                    u32::try_from(indices.len() / 4).map_err(|_| ClusterGpuError::ResourceCap)?;
                indices.extend_from_slice(&packed_index.to_le_bytes());
            }
        }
        add_meta(
            metadata,
            cluster,
            cluster_radius,
            product.bounds_center_local,
            first_index,
            detail_set,
            region_radius,
        )?;
        let _ = cluster_ordinal;
    }
    Ok(())
}

fn append_transition(
    product: &RegionProduct,
    transition: &TransitionMesh,
    vertices: &mut Vec<u8>,
    indices: &mut Vec<u8>,
    metadata: &mut Vec<ClusterMetaCpu>,
    region_radius: &mut f32,
) -> Result<(), ClusterGpuError> {
    let triangle_masks = boundary_masks(
        &transition.indices,
        &transition.clusters,
        transition.vertices.len(),
    )?;
    for cluster in &transition.clusters {
        let start = cluster.first_index as usize;
        let end = start
            .checked_add(cluster.index_count as usize)
            .ok_or(ClusterGpuError::InvalidProduct)?;
        if cluster.index_count == 0
            || cluster.index_count % 3 != 0
            || end > transition.indices.len()
        {
            return Err(ClusterGpuError::InvalidProduct);
        }
        let first_index =
            u32::try_from(indices.len() / 4).map_err(|_| ClusterGpuError::ResourceCap)?;
        let color = stable_cluster_color(product, cluster.stable_id);
        let lod = lod_color(2);
        let mut cluster_radius = cluster.sphere_radius_m;
        for triangle_start in (start..end).step_by(3) {
            let mask = triangle_masks[triangle_start / 3];
            for corner in 0..3 {
                let source_index = transition.indices[triangle_start + corner] as usize;
                let source = transition
                    .vertices
                    .get(source_index)
                    .ok_or(ClusterGpuError::InvalidProduct)?;
                let vertex = RenderVertex::transition(
                    source,
                    product.bounds_center_local,
                    barycentric(corner),
                    color,
                    lod,
                    mask,
                );
                validate_render_vertex(vertex)?;
                *region_radius = region_radius.max(conservative_length_f32(
                    source.fine.position - product.bounds_center_local,
                )?);
                *region_radius = region_radius.max(conservative_length_f32(
                    source.coarse.position - product.bounds_center_local,
                )?);
                cluster_radius =
                    cluster_radius.max(source.fine.position.distance(cluster.sphere_center_local));
                cluster_radius = cluster_radius
                    .max(source.coarse.position.distance(cluster.sphere_center_local));
                push_render_vertex(vertices, vertex);
                let packed_index =
                    u32::try_from(indices.len() / 4).map_err(|_| ClusterGpuError::ResourceCap)?;
                indices.extend_from_slice(&packed_index.to_le_bytes());
            }
        }
        add_meta(
            metadata,
            cluster,
            cluster_radius,
            product.bounds_center_local,
            first_index,
            2,
            region_radius,
        )?;
    }
    Ok(())
}

fn add_meta(
    metadata: &mut Vec<ClusterMetaCpu>,
    cluster: &Cluster,
    radius_m: f64,
    center: DVec3,
    first_index: u32,
    set: u32,
    region_radius: &mut f32,
) -> Result<(), ClusterGpuError> {
    if !cluster.sphere_center_local.is_finite() || !radius_m.is_finite() || radius_m < 0.0 {
        return Err(ClusterGpuError::InvalidProduct);
    }
    let relative = cluster.sphere_center_local - center;
    let sphere = [relative.x as f32, relative.y as f32, relative.z as f32];
    if sphere.iter().any(|value| !value.is_finite()) {
        return Err(ClusterGpuError::InvalidProduct);
    }
    let packed_relative = DVec3::new(
        f64::from(sphere[0]),
        f64::from(sphere[1]),
        f64::from(sphere[2]),
    );
    let center_rounding = relative.distance(packed_relative);
    let packed_radius = conservative_f32(radius_m + center_rounding)?;
    let packed_sphere = [sphere[0], sphere[1], sphere[2], packed_radius];
    *region_radius = region_radius.max(conservative_f32(
        packed_relative.length() + f64::from(packed_radius),
    )?);
    if !region_radius.is_finite() {
        return Err(ClusterGpuError::InvalidProduct);
    }
    metadata.push(ClusterMetaCpu {
        sphere: packed_sphere,
        first_index,
        index_count: cluster.index_count,
        base_vertex: 0,
        detail_set: set,
    });
    Ok(())
}

#[derive(Clone, Copy)]
struct RenderVertex {
    fine_position: [f32; 3],
    coarse_position: [f32; 3],
    fine_normal: [f32; 3],
    coarse_normal: [f32; 3],
    fine_material: [f32; 4],
    coarse_material: [f32; 4],
    uv: [f32; 2],
    barycentric: [f32; 3],
    cluster_color: [f32; 3],
    lod_color: [f32; 3],
    metadata: u32,
}

impl RenderVertex {
    fn same(
        source: &RegionVertex,
        center: DVec3,
        bary: [f32; 3],
        color: [f32; 3],
        lod: [f32; 3],
        edge_mask: u8,
        set: u32,
    ) -> Self {
        let position = f32x3(source.position - center);
        let normal = f32x3(source.normal);
        let material = source.material.map(|v| v as f32);
        Self {
            fine_position: position,
            coarse_position: position,
            fine_normal: normal,
            coarse_normal: normal,
            fine_material: material,
            coarse_material: material,
            uv: source.uv.map(|v| v as f32),
            barycentric: bary,
            cluster_color: color,
            lod_color: lod,
            metadata: u32::from(edge_mask) | (set << 4),
        }
    }
    fn transition(
        source: &TransitionVertex,
        center: DVec3,
        bary: [f32; 3],
        color: [f32; 3],
        lod: [f32; 3],
        edge_mask: u8,
    ) -> Self {
        Self {
            fine_position: f32x3(source.fine.position - center),
            coarse_position: f32x3(source.coarse.position - center),
            fine_normal: f32x3(source.fine.normal),
            coarse_normal: f32x3(source.coarse.normal),
            fine_material: source.fine.material.map(|v| v as f32),
            coarse_material: source.coarse.material.map(|v| v as f32),
            uv: source.fine.uv.map(|v| v as f32),
            barycentric: bary,
            cluster_color: color,
            lod_color: lod,
            metadata: u32::from(edge_mask) | (2 << 4),
        }
    }
}

fn validate_render_vertex(vertex: RenderVertex) -> Result<(), ClusterGpuError> {
    if [
        vertex.fine_position.as_slice(),
        vertex.coarse_position.as_slice(),
        vertex.fine_normal.as_slice(),
        vertex.coarse_normal.as_slice(),
        vertex.fine_material.as_slice(),
        vertex.coarse_material.as_slice(),
        vertex.barycentric.as_slice(),
        vertex.cluster_color.as_slice(),
        vertex.lod_color.as_slice(),
    ]
    .into_iter()
    .flatten()
    .any(|value| !value.is_finite())
        || vertex.uv.iter().any(|value| !value.is_finite())
    {
        return Err(ClusterGpuError::InvalidProduct);
    }
    Ok(())
}

fn push_render_vertex(output: &mut Vec<u8>, vertex: RenderVertex) {
    for vector in [
        vertex.fine_position,
        vertex.coarse_position,
        vertex.fine_normal,
        vertex.coarse_normal,
    ] {
        for value in vector {
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
    for vector in [vertex.fine_material, vertex.coarse_material] {
        for value in vector {
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
    for value in vertex.uv {
        output.extend_from_slice(&value.to_le_bytes());
    }
    for vector in [vertex.barycentric, vertex.cluster_color, vertex.lod_color] {
        for value in vector {
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
    output.extend_from_slice(&vertex.metadata.to_le_bytes());
}

fn boundary_masks(
    indices: &[u32],
    clusters: &[Cluster],
    vertex_count: usize,
) -> Result<Vec<u8>, ClusterGpuError> {
    if !indices.len().is_multiple_of(3) {
        return Err(ClusterGpuError::InvalidProduct);
    }
    let mut coverage = vec![false; indices.len()];
    let mut edges = Vec::with_capacity(indices.len());
    for (cluster_id, cluster) in clusters.iter().enumerate() {
        let start = cluster.first_index as usize;
        let end = start
            .checked_add(cluster.index_count as usize)
            .ok_or(ClusterGpuError::InvalidProduct)?;
        if cluster.index_count == 0 || cluster.index_count % 3 != 0 || end > indices.len() {
            return Err(ClusterGpuError::InvalidProduct);
        }
        for covered in &mut coverage[start..end] {
            if std::mem::replace(covered, true) {
                return Err(ClusterGpuError::InvalidProduct);
            }
        }
        for triangle in (start..end).step_by(3) {
            let source = [
                indices[triangle],
                indices[triangle + 1],
                indices[triangle + 2],
            ];
            if source.iter().any(|index| *index as usize >= vertex_count) {
                return Err(ClusterGpuError::InvalidProduct);
            }
            for component in 0..3 {
                let a = source[(component + 1) % 3].min(source[(component + 2) % 3]);
                let b = source[(component + 1) % 3].max(source[(component + 2) % 3]);
                edges.push(EdgeUse {
                    key: (u64::from(a) << 32) | u64::from(b),
                    triangle: (triangle / 3) as u32,
                    cluster: cluster_id as u32,
                    component: component as u8,
                });
            }
        }
    }
    if coverage.iter().any(|covered| !covered) {
        return Err(ClusterGpuError::InvalidProduct);
    }
    edges.sort_unstable_by_key(|edge| edge.key);
    let mut masks = vec![0u8; indices.len() / 3];
    let mut start = 0;
    while start < edges.len() {
        let mut end = start + 1;
        while end < edges.len() && edges[end].key == edges[start].key {
            end += 1;
        }
        let first_cluster = edges[start].cluster;
        let boundary = end - start == 1
            || edges[start..end]
                .iter()
                .any(|edge| edge.cluster != first_cluster);
        if boundary {
            for edge in &edges[start..end] {
                masks[edge.triangle as usize] |= 1 << edge.component;
            }
        }
        start = end;
    }
    Ok(masks)
}

fn stable_cluster_color(product: &RegionProduct, stable_id: u32) -> [f32; 3] {
    let mut hasher = DefaultHasher::new();
    product.input.key.hash(&mut hasher);
    stable_id.hash(&mut hasher);
    let h = (hasher.finish() % 360) as f32 / 60.0;
    let c = 0.78;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = 0.12;
    [r + m, g + m, b + m]
}

fn lod_color(set: u32) -> [f32; 3] {
    match set {
        0 => [0.20, 0.78, 0.86],
        1 => [0.94, 0.64, 0.25],
        _ => [0.63, 0.48, 0.93],
    }
}
fn barycentric(corner: usize) -> [f32; 3] {
    let mut value = [0.0; 3];
    value[corner] = 1.0;
    value
}
fn f32x3(value: DVec3) -> [f32; 3] {
    [value.x as f32, value.y as f32, value.z as f32]
}
fn conservative_length_f32(value: DVec3) -> Result<f32, ClusterGpuError> {
    conservative_f32(value.length())
}
fn conservative_f32(value: f64) -> Result<f32, ClusterGpuError> {
    if !value.is_finite() || value < 0.0 {
        return Err(ClusterGpuError::InvalidProduct);
    }
    let narrowed = value as f32;
    if !narrowed.is_finite() {
        return Err(ClusterGpuError::InvalidProduct);
    }
    let rounded_up = f32::from_bits(narrowed.to_bits().saturating_add(1));
    if rounded_up.is_finite() {
        Ok(rounded_up)
    } else {
        Err(ClusterGpuError::InvalidProduct)
    }
}
fn bool_u32(value: bool) -> u32 {
    if value { 1 } else { 0 }
}
fn push_vec4(bytes: &mut Vec<u8>, value: [f32; 4]) {
    for x in value {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
}
fn push_uvec4(bytes: &mut Vec<u8>, value: [u32; 4]) {
    for x in value {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
}
fn push_mat3(bytes: &mut Vec<u8>, value: DMat3) {
    for axis in [value.x_axis, value.y_axis, value.z_axis] {
        push_vec4(bytes, [axis.x as f32, axis.y as f32, axis.z as f32, 0.0]);
    }
}

fn buffer_layout(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
    size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

fn create_gpu_slot(
    device: &wgpu::Device,
    select_layout: &wgpu::BindGroupLayout,
    draw_layout: &wgpu::BindGroupLayout,
    product: Arc<RegionProduct>,
    packed: PackedRegion,
) -> Result<GpuRegionSlot, ClusterGpuError> {
    let make = |label: &str, bytes: &[u8], usage: wgpu::BufferUsages| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytes,
            usage,
        })
    };
    let vertex_buffer = make(
        "Finite cluster expanded render vertices",
        &packed.vertices,
        wgpu::BufferUsages::VERTEX,
    );
    let index_buffer = make(
        "Finite cluster fixed indirect indices",
        &packed.indices,
        wgpu::BufferUsages::INDEX,
    );
    let metadata_buffer = make(
        "Finite cluster sphere and command metadata",
        &packed.metadata,
        wgpu::BufferUsages::STORAGE,
    );
    let indirect_buffer = make(
        "Finite cluster bounded indirect commands",
        &packed.commands,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::INDIRECT,
    );
    let select_params = make(
        "Finite cluster selection uniform",
        &vec![0; SELECT_PARAMS_BYTES as usize],
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let draw_params = make(
        "Finite cluster render uniform",
        &vec![0; DRAW_PARAMS_BYTES as usize],
        wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    );
    let select_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Finite cluster compute group"),
        layout: select_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: select_params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: metadata_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: indirect_buffer.as_entire_binding(),
            },
        ],
    });
    let render_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Finite cluster draw group"),
        layout: draw_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: draw_params.as_entire_binding(),
        }],
    });
    Ok(GpuRegionSlot {
        product,
        vertex_buffer,
        index_buffer,
        _metadata_buffer: metadata_buffer,
        indirect_buffer,
        select_params,
        draw_params,
        select_group,
        render_group,
        command_count: packed.command_count,
        cluster_refs: packed.cluster_refs,
        sphere_radius: packed.sphere_radius,
        coarse_error_m: packed.coarse_error_m,
        coarse_eligible: packed.coarse_eligible,
        gpu_bytes: packed.gpu_bytes,
        last_use_submission: 0,
        pinned: false,
        cut: CutState::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SELECT_SHADER: &str = include_str!("shaders/cluster_select.wgsl");
    const RENDER_SHADER: &str = include_str!("shaders/cluster_render.wgsl");

    fn validate_shader(source: &str) -> naga::Module {
        let module = naga::front::wgsl::parse_str(source).expect("cluster WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("cluster WGSL validates");
        module
    }

    fn struct_span(module: &naga::Module, name: &str) -> u32 {
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some(name))
            .expect("expected WGSL ABI type");
        let naga::TypeInner::Struct { span, .. } = &ty.inner else {
            panic!("WGSL ABI type must be a struct");
        };
        *span
    }

    #[test]
    fn gpu_selector_reference_uses_near_off_axis_and_strict_equality() {
        let near_center = [0.0, 0.0, -1.0];
        assert!(!projected_error_below_limit(
            near_center,
            0.1,
            0.001,
            1000.0,
            0.1
        ));
        let off_axis: [f32; 3] = [4.0, 0.0, -10.0];
        let on_axis: [f32; 3] = [0.0, 0.0, -10.0];
        let error = 0.001;
        let focal = 1000.0;
        let near = 0.1;
        let off_dmin = -off_axis[2] - 0.5 - error;
        let on_dmin = -on_axis[2] - 0.5 - error;
        let off_bound = (focal * error / off_dmin)
            * (1.0 + (off_axis[0].abs().max(off_axis[1].abs()) + 0.5 + error) / off_dmin);
        let on_bound = (focal * error / on_dmin)
            * (1.0 + (on_axis[0].abs().max(on_axis[1].abs()) + 0.5 + error) / on_dmin);
        assert!(off_bound > on_bound);
        assert_eq!(
            projected_error_below_limit(off_axis, 0.5, error, focal, near),
            off_bound < crate::cluster::FINITE_MESH_ERROR_PX as f32
        );
        let center = [0.0, 0.0, -10.0];
        let radius = 0.0;
        let dmin = -center[2] - radius;
        let epsilon =
            crate::cluster::FINITE_MESH_ERROR_PX as f32 * dmin / (focal * (1.0 + 0.0 / dmin));
        assert!(!projected_error_below_limit(
            center, radius, epsilon, focal, near
        ));
        assert!(projected_error_below_limit(
            center,
            radius,
            epsilon * 0.99,
            focal,
            near
        ));
        assert!(SELECT_SHADER.contains(
            "let off_axis = max(abs(center.x), abs(center.y)) + region_radius + epsilon;"
        ));
        assert!(SELECT_SHADER.contains("if projected_error < 0.15"));
    }

    #[test]
    fn cpu_frustum_sphere_test_matches_shader_f32_plane_predicate() {
        let projection =
            crate::CelestialProjection::try_new(800, 600, std::f64::consts::FRAC_PI_2, 0.1)
                .expect("valid test projection");
        for (center, radius) in [
            ([0.0, 0.0, -1.0], 0.1),
            ([0.0, 0.0, -0.05], 0.001),
            ([8.0, 0.0, -1.0], 0.0),
            ([8.0, 0.0, -1.0], 10.0),
        ] {
            let shader_reference =
                projection
                    .frustum_planes()
                    .into_iter()
                    .all(|(normal, distance)| {
                        let normal = view_to_f32(normal);
                        normal[0] * center[0]
                            + normal[1] * center[1]
                            + normal[2] * center[2]
                            + distance as f32
                            >= -radius
                    });
            assert_eq!(
                cluster_sphere_visible(projection, center, radius),
                shader_reference
            );
        }
        assert!(SELECT_SHADER.contains("dot(plane.xyz, center) + plane.w < -radius"));
    }

    #[test]
    fn cluster_shaders_validate_and_match_host_abi_sizes() {
        let select = validate_shader(SELECT_SHADER);
        let render = validate_shader(RENDER_SHADER);
        assert_eq!(
            struct_span(&select, "SelectionParams") as u64,
            SELECT_PARAMS_BYTES
        );
        assert_eq!(
            struct_span(&select, "ClusterMeta") as u64,
            CLUSTER_META_BYTES
        );
        assert_eq!(
            struct_span(&select, "DrawIndexedIndirectArgs") as usize,
            INDIRECT_BYTES
        );
        assert_eq!(struct_span(&render, "DrawParams") as u64, DRAW_PARAMS_BYTES);
        assert_eq!(
            struct_span(&render, "Projection") as usize,
            size_of::<glam::Mat4>()
        );
        assert!(
            select
                .entry_points
                .iter()
                .any(|entry| entry.name == "select_main"
                    && entry.stage == naga::ShaderStage::Compute)
        );
        assert!(
            render
                .entry_points
                .iter()
                .any(|entry| entry.name == "vs_main" && entry.stage == naga::ShaderStage::Vertex)
        );
        assert!(
            render
                .entry_points
                .iter()
                .any(|entry| entry.name == "fs_main" && entry.stage == naga::ShaderStage::Fragment)
        );
    }

    #[test]
    fn packed_vertex_stride_and_serializer_are_128_bytes() {
        assert_eq!(size_of::<RenderVertex>(), RENDER_VERTEX_BYTES);
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Float32x3, 3 => Float32x3,
            4 => Float32x4, 5 => Float32x4, 6 => Float32x2, 7 => Float32x3,
            8 => Float32x3, 9 => Float32x3, 10 => Uint32
        ];
        let offsets: Vec<_> = attributes
            .iter()
            .map(|attribute| attribute.offset)
            .collect();
        assert_eq!(
            offsets.as_slice(),
            &[0u64, 12, 24, 36, 48, 64, 80, 88, 100, 112, 124]
        );
        assert_eq!(attributes[10].offset + 4, RENDER_VERTEX_BYTES as u64);
        let vertex = RenderVertex {
            fine_position: [0.0; 3],
            coarse_position: [0.0; 3],
            fine_normal: [0.0; 3],
            coarse_normal: [0.0; 3],
            fine_material: [0.0; 4],
            coarse_material: [0.0; 4],
            uv: [0.0; 2],
            barycentric: [0.0; 3],
            cluster_color: [0.0; 3],
            lod_color: [0.0; 3],
            metadata: 0,
        };
        let mut bytes = Vec::new();
        push_render_vertex(&mut bytes, vertex);
        assert_eq!(bytes.len(), RENDER_VERTEX_BYTES);
    }

    #[test]
    fn lod_transitions_remain_continuous_when_crossing_near_plane() {
        let started = Instant::now();
        let mut cut = CutState::default();
        let (fine_morph, active) = cut.advance(0.0, started, true);
        assert_eq!(fine_morph, 1.0);
        assert!(active);
        let near_center = [0.0, 0.0, -0.15];
        assert!(!projected_error_below_limit(
            near_center,
            0.05,
            0.01,
            1000.0,
            0.1
        ));
        let (continued_morph, still_active) =
            cut.advance(1.0, started + Duration::from_millis(1), true);
        assert!(still_active);
        assert!(continued_morph > 0.99);
        assert!(SELECT_SHADER.contains("if params.controls.z == 1u {"));
    }

    /// Numerical shader conformance only. This deliberately tiny GPU fixture
    /// does not exercise the native application, scene integration, pacing,
    /// or visual acceptance.
    #[test]
    #[ignore = "requires a locally available GPU adapter; numerical shader check only, not native acceptance"]
    fn cluster_compute_readback_matches_cpu_selection_reference() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let adapter = match instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                })
                .await
            {
                Ok(adapter) => adapter,
                Err(error) => {
                    eprintln!("SKIP: cluster compute readback needs a GPU adapter: {error}");
                    return;
                }
            };
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("Cluster selector numerical test device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    trace: wgpu::Trace::Off,
                })
                .await
                .expect("test device creation");

            let projection =
                crate::CelestialProjection::try_new(800, 600, std::f64::consts::FRAC_PI_2, 0.1)
                    .expect("test projection");
            let mut metadata = Vec::new();
            let records: [([f32; 4], u32, u32); 4] = [
                ([0.0, 0.0, 0.0, 0.01], 0, 0),
                ([0.0, 0.0, 0.0, 0.01], 3, 1),
                ([0.0, 0.0, 0.0, 0.01], 6, 2),
                ([100.0, 0.0, 0.0, 0.01], 9, 0),
            ];
            for (center, first_index, detail_set) in records {
                for value in center {
                    metadata.extend_from_slice(&value.to_le_bytes());
                }
                for value in [first_index, 3, 0, detail_set] {
                    metadata.extend_from_slice(&value.to_le_bytes());
                }
            }
            let metadata_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Cluster selector numerical metadata"),
                contents: &metadata,
                usage: wgpu::BufferUsages::STORAGE,
            });
            let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Cluster selector numerical parameters"),
                size: SELECT_PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let command_bytes = 4 * INDIRECT_BYTES as u64;
            let command_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Cluster selector numerical indirect arguments"),
                size: command_bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::INDIRECT
                    | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Cluster selector numerical readback"),
                size: command_bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Cluster selector numerical layout"),
                entries: &[
                    buffer_layout(
                        0,
                        wgpu::ShaderStages::COMPUTE,
                        wgpu::BufferBindingType::Uniform,
                        SELECT_PARAMS_BYTES,
                    ),
                    buffer_layout(
                        1,
                        wgpu::ShaderStages::COMPUTE,
                        wgpu::BufferBindingType::Storage { read_only: true },
                        CLUSTER_META_BYTES,
                    ),
                    buffer_layout(
                        2,
                        wgpu::ShaderStages::COMPUTE,
                        wgpu::BufferBindingType::Storage { read_only: false },
                        INDIRECT_BYTES as u64,
                    ),
                ],
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Cluster selector numerical group"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: metadata_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: command_buffer.as_entire_binding(),
                    },
                ],
            });
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Cluster selector numerical shader"),
                source: wgpu::ShaderSource::Wgsl(SELECT_SHADER.into()),
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Cluster selector numerical pipeline layout"),
                bind_group_layouts: &[&layout],
                push_constant_ranges: &[],
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Cluster selector numerical pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("select_main"),
                compilation_options: Default::default(),
                cache: None,
            });

            struct Case {
                label: &'static str,
                origin: [f32; 3],
                error: f32,
                region_radius: f32,
                focal: f32,
                near: f32,
                mode: u32,
                transition: u32,
                coarse_eligible: u32,
                expected_set: u32,
            }
            let focal = projection.focal_pixels() as f32;
            let cases = [
                Case {
                    label: "fine-error",
                    origin: [0.0, 0.0, -10.0],
                    error: 0.01,
                    region_radius: 0.1,
                    focal,
                    near: 0.1,
                    mode: 1,
                    transition: 0,
                    coarse_eligible: 1,
                    expected_set: 0,
                },
                Case {
                    label: "coarse-on-axis",
                    origin: [0.0, 0.0, -10.0],
                    error: 0.0035,
                    region_radius: 0.1,
                    focal,
                    near: 0.1,
                    mode: 1,
                    transition: 0,
                    coarse_eligible: 1,
                    expected_set: 1,
                },
                Case {
                    label: "strict-equality",
                    origin: [0.0, 0.0, -1.5],
                    error: 0.5,
                    region_radius: 0.0,
                    focal: 0.2,
                    near: 0.1,
                    mode: 1,
                    transition: 0,
                    coarse_eligible: 1,
                    expected_set: 0,
                },
                Case {
                    label: "near-plane",
                    origin: [0.0, 0.0, -0.15],
                    error: 0.01,
                    region_radius: 0.05,
                    focal,
                    near: 0.1,
                    mode: 1,
                    transition: 0,
                    coarse_eligible: 1,
                    expected_set: 0,
                },
                Case {
                    label: "off-axis-bound",
                    origin: [4.5, 0.0, -10.0],
                    error: 0.0035,
                    region_radius: 0.1,
                    focal,
                    near: 0.1,
                    mode: 1,
                    transition: 0,
                    coarse_eligible: 1,
                    expected_set: 0,
                },
                Case {
                    label: "outside-frustum",
                    origin: [100.0, 0.0, -10.0],
                    error: 0.0035,
                    region_radius: 0.1,
                    focal,
                    near: 0.1,
                    mode: 0,
                    transition: 0,
                    coarse_eligible: 1,
                    expected_set: 0,
                },
                Case {
                    label: "missing-coarse",
                    origin: [0.0, 0.0, -10.0],
                    error: 0.001,
                    region_radius: 0.1,
                    focal,
                    near: 0.1,
                    mode: 1,
                    transition: 0,
                    coarse_eligible: 0,
                    expected_set: 0,
                },
                Case {
                    label: "transition",
                    origin: [0.0, 0.0, -10.0],
                    error: 0.01,
                    region_radius: 0.1,
                    focal,
                    near: 0.1,
                    mode: 1,
                    transition: 1,
                    coarse_eligible: 1,
                    expected_set: 2,
                },
                Case {
                    label: "culling-fine",
                    origin: [0.0, 0.0, -10.0],
                    error: 0.001,
                    region_radius: 0.1,
                    focal,
                    near: 0.1,
                    mode: 0,
                    transition: 0,
                    coarse_eligible: 1,
                    expected_set: 0,
                },
            ];
            let planes = projection.frustum_planes();
            for case in cases {
                let mut params = Vec::with_capacity(SELECT_PARAMS_BYTES as usize);
                for (normal, distance) in planes {
                    push_vec4(
                        &mut params,
                        [
                            normal.x as f32,
                            normal.y as f32,
                            normal.z as f32,
                            distance as f32,
                        ],
                    );
                }
                push_vec4(
                    &mut params,
                    [case.origin[0], case.origin[1], case.origin[2], 1.0],
                );
                push_mat3(&mut params, DMat3::IDENTITY);
                push_vec4(
                    &mut params,
                    [case.focal, case.near, case.error, case.region_radius],
                );
                push_uvec4(
                    &mut params,
                    [4, case.mode, case.transition, case.coarse_eligible],
                );
                assert_eq!(
                    params.len(),
                    SELECT_PARAMS_BYTES as usize,
                    "{} ABI",
                    case.label
                );
                queue.write_buffer(&params_buffer, 0, &params);

                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Cluster selector numerical encoder"),
                });
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("Cluster selector numerical dispatch"),
                        timestamp_writes: None,
                    });
                    pass.set_pipeline(&pipeline);
                    pass.set_bind_group(0, &bind_group, &[]);
                    pass.dispatch_workgroups(1, 1, 1);
                }
                encoder.copy_buffer_to_buffer(&command_buffer, 0, &readback, 0, command_bytes);
                let submission = queue.submit([encoder.finish()]);
                let (sender, receiver) = std::sync::mpsc::channel();
                readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        let _ = sender.send(result);
                    });
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: Some(submission),
                        timeout: None,
                    })
                    .expect("selector test GPU progress");
                receiver
                    .recv()
                    .expect("selector map callback")
                    .expect("selector readback map");
                let mapped = readback.slice(..).get_mapped_range();
                let actual: Vec<[u32; 5]> = mapped
                    .as_chunks::<INDIRECT_BYTES>()
                    .0
                    .iter()
                    .map(|record| {
                        std::array::from_fn(|word| {
                            let start = word * 4;
                            u32::from_le_bytes(record[start..start + 4].try_into().unwrap())
                        })
                    })
                    .collect();
                drop(mapped);
                readback.unmap();

                let mut selected_set = 0;
                if case.mode == 1
                    && case.coarse_eligible == 1
                    && projected_error_below_limit(
                        case.origin,
                        case.region_radius,
                        case.error,
                        case.focal,
                        case.near,
                    )
                {
                    selected_set = 1;
                }
                if case.transition == 1 {
                    selected_set = 2;
                }
                assert_eq!(
                    selected_set, case.expected_set,
                    "reference case {}",
                    case.label
                );
                for (index, cluster) in [
                    ([0.0, 0.0, 0.0], 0u32),
                    ([0.0, 0.0, 0.0], 1),
                    ([0.0, 0.0, 0.0], 2),
                    ([100.0, 0.0, 0.0], 0),
                ]
                .into_iter()
                .enumerate()
                {
                    let center = [
                        case.origin[0] + cluster.0[0],
                        case.origin[1] + cluster.0[1],
                        case.origin[2] + cluster.0[2],
                    ];
                    let visible = cluster_sphere_visible(projection, center, 0.01);
                    let expected_count = if cluster.1 == selected_set && visible {
                        3
                    } else {
                        0
                    };
                    assert_eq!(
                        actual[index],
                        [expected_count, 1, (index * 3) as u32, 0, 0],
                        "case {} cluster {index}",
                        case.label
                    );
                }
            }
        });
    }
}
