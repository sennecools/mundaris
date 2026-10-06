use super::{
    ReconstructedTileVertex, ResidentTileReport, TileData, TileDraw, TileKey, TileSlotState,
};
use crate::regional_resident::{RegionalResidentDraw, RegionalResidentReport, RegionalSlotReport};
use crate::{RenderPreparationError, ResidentHierarchyDraw, ResidentHierarchyReport};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use wgpu::util::DeviceExt;

const PARAM_BYTES: u64 = 384;
const PARAM_STRIDE: u64 = 512;
const SLOT_COUNT: usize = 5;
const REGIONAL_SLOT_BASE: usize = SLOT_COUNT;
const VALIDATION_VERTEX_BYTES: u64 = 64;

#[derive(Clone, Copy)]
enum PrepareKind {
    Render,
    Validation,
}

pub(crate) struct ResidentTileRenderer {
    pipeline: wgpu::RenderPipeline,
    validation_pipeline: wgpu::ComputePipeline,
    tile_layout: wgpu::BindGroupLayout,
    tile_slots: Vec<ResidentTileGpuSlot>,
    tile_groups: BTreeMap<(usize, usize), wgpu::BindGroup>,
    params_layout: wgpu::BindGroupLayout,
    params_buffer: wgpu::Buffer,
    validation_buffer: wgpu::Buffer,
    params_group: wgpu::BindGroup,
    grid_vertices: Option<wgpu::Buffer>,
    grid_indices: Option<wgpu::Buffer>,
    grid_capacity_bytes: u64,
    index_count: u32,
    grid_cells: u32,
    report: ResidentTileReport,
    hierarchy_report: ResidentHierarchyReport,
    regional_report: RegionalResidentReport,
    regional_draw: Option<RegionalResidentDraw>,
    completed_submission: Arc<AtomicU64>,
    submission_serial: u64,
    prepared_draw_slots: BTreeSet<usize>,
    regional_active_patches: Vec<usize>,
    regional_cells: u32,
    allocation_count: u32,
}

struct ResidentTileGpuSlot {
    buffer: wgpu::Buffer,
    capacity: u64,
    resident_key: Option<TileKey>,
    resident_tile: Option<Arc<TileData>>,
    publication_state: TileSlotState,
    edge_buffer: wgpu::Buffer,
    edge_capacity: u64,
    own_edge_version: Option<u64>,
    parent_boundary: Option<crate::regional_edges::TileBoundary>,
    last_use_submission: u64,
}

struct SlotPreflight {
    publication_state: TileSlotState,
    required_capacity: Option<u64>,
}

impl ResidentTileGpuSlot {
    fn new(device: &wgpu::Device, index: usize) -> Self {
        Self {
            buffer: create_buffer(
                device,
                32,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                &format!("Resident terrain tile slot {index}"),
            ),
            capacity: 32,
            resident_key: None,
            resident_tile: None,
            publication_state: TileSlotState::default(),
            edge_buffer: create_buffer(
                device,
                48,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                &format!("Resident terrain edge endpoints {index}"),
            ),
            edge_capacity: 48,
            own_edge_version: None,
            parent_boundary: None,
            last_use_submission: 0,
        }
    }
}

impl ResidentTileRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        projection_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let tile_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Resident terrain tile texels"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(32),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(32),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(48),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(48),
                    },
                    count: None,
                },
            ],
        });
        let params_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Resident terrain draw and validation parameters"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX
                        | wgpu::ShaderStages::FRAGMENT
                        | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(PARAM_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(VALIDATION_VERTEX_BYTES),
                    },
                    count: None,
                },
            ],
        });
        let tile_slots = (0..SLOT_COUNT)
            .map(|index| ResidentTileGpuSlot::new(device, index))
            .collect::<Vec<_>>();
        let tile_groups = create_legacy_tile_groups(device, &tile_layout, &tile_slots);
        let params_buffer = create_buffer(
            device,
            PARAM_STRIDE * SLOT_COUNT as u64,
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            "Resident terrain patch parameters",
        );
        let validation_buffer = create_buffer(
            device,
            u64::from((TileData::MAX_CELLS + 1).pow(2)) * VALIDATION_VERTEX_BYTES,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            "Resident tile validation output",
        );
        let params_group = params_group(device, &params_layout, &params_buffer, &validation_buffer);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Resident terrain tile displacement"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/resident_tile.wgsl").into()),
        });
        let layouts = [projection_layout, &tile_layout, &params_layout];
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Resident terrain tile pipeline layout"),
            bind_group_layouts: &layouts,
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Resident terrain tile reverse-Z draw"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x2],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
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
        });
        let validation_pipeline =
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("Resident terrain tile reconstruction validator"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("validate_main"),
                compilation_options: Default::default(),
                cache: None,
            });
        Self {
            pipeline,
            validation_pipeline,
            tile_layout,
            tile_slots,
            tile_groups,
            params_layout,
            params_buffer,
            validation_buffer,
            params_group,
            grid_vertices: None,
            grid_indices: None,
            grid_capacity_bytes: 0,
            index_count: 0,
            grid_cells: 0,
            report: ResidentTileReport::default(),
            hierarchy_report: ResidentHierarchyReport::default(),
            regional_report: RegionalResidentReport::default(),
            regional_draw: None,
            completed_submission: Arc::new(AtomicU64::new(0)),
            submission_serial: 0,
            prepared_draw_slots: BTreeSet::new(),
            regional_active_patches: Vec::new(),
            regional_cells: 0,
            allocation_count: (SLOT_COUNT * 2 + 2) as u32,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draw: &TileDraw,
    ) -> Result<(), RenderPreparationError> {
        self.regional_draw = None;
        self.regional_active_patches.clear();
        let slot = self.preflight_slot(device, 0, draw)?;
        self.preflight_grid(device, draw.tile.key.cells)?;
        self.clear_frame();
        self.hierarchy_report.parent_pinned = false;
        self.hierarchy_report.draw_children = false;
        self.hierarchy_report.morph_fraction = 0.0;
        self.publish_slot(device, queue, 0, draw, slot, PrepareKind::Render)?;
        self.ensure_grid(device, draw.tile.key.cells)?;
        let params = pack_params(
            draw,
            draw,
            glam::DVec3::ZERO,
            1.0,
            1.0,
            0,
            [0, 0],
            0,
            0,
            0.0,
        );
        self.write_params(queue, 0, &params, PrepareKind::Render);
        self.prepared_draw_slots.clear();
        self.prepared_draw_slots.insert(0);
        self.finish_report();
        Ok(())
    }

    pub(crate) fn prepare_hierarchy(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draw: &ResidentHierarchyDraw,
    ) -> Result<(), RenderPreparationError> {
        self.prepare_hierarchy_for(device, queue, draw, PrepareKind::Render)
    }

    fn prepare_hierarchy_for(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draw: &ResidentHierarchyDraw,
        kind: PrepareKind,
    ) -> Result<(), RenderPreparationError> {
        if matches!(kind, PrepareKind::Render) {
            self.regional_draw = None;
            self.regional_active_patches.clear();
        }
        draw.validate()
            .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
        let mut slot_preflights: [Option<SlotPreflight>; SLOT_COUNT] =
            std::array::from_fn(|_| None);
        slot_preflights[0] = Some(self.preflight_slot(device, 0, &draw.parent)?);
        for (child_index, child) in draw.children.iter().enumerate() {
            if let Some(child) = child {
                slot_preflights[child_index + 1] =
                    Some(self.preflight_slot(device, child_index + 1, child)?);
            }
        }
        let cells = draw.parent.tile.key.cells;
        self.preflight_grid(device, cells)?;
        self.clear_for_kind(kind);
        self.publish_slot(
            device,
            queue,
            0,
            &draw.parent,
            slot_preflights[0]
                .take()
                .ok_or(RenderPreparationError::InvalidResidentTile)?,
            kind,
        )?;
        for (child_index, child) in draw.children.iter().enumerate() {
            if let Some(child) = child {
                self.publish_slot(
                    device,
                    queue,
                    child_index + 1,
                    child,
                    slot_preflights[child_index + 1]
                        .take()
                        .ok_or(RenderPreparationError::InvalidResidentTile)?,
                    kind,
                )?;
            }
        }
        self.ensure_grid(device, cells)?;

        let parent_params = pack_params(
            &draw.parent,
            &draw.parent,
            glam::DVec3::ZERO,
            draw.morph_fraction,
            1.0,
            1,
            [0, 0],
            0,
            0,
            0.0,
        );
        self.write_params(queue, 0, &parent_params, kind);
        for (child_index, child) in draw.children.iter().enumerate() {
            let Some(child) = child else { continue };
            let delta = draw
                .child_anchor_delta_body(child_index)
                .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
            let quadrant = ResidentHierarchyDraw::quadrant(child_index)
                .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
            let params = pack_params(
                child,
                &draw.parent,
                delta,
                draw.morph_fraction,
                1.0,
                2,
                quadrant,
                child_index + 1,
                0,
                0.0,
            );
            self.write_params(queue, child_index + 1, &params, kind);
        }
        self.hierarchy_report.parent_pinned = true;
        self.hierarchy_report.draw_children = draw.draw_children;
        self.hierarchy_report.morph_fraction = draw.morph_fraction;
        if matches!(kind, PrepareKind::Render) {
            self.prepared_draw_slots.clear();
            self.prepared_draw_slots.insert(0);
            if draw.draw_children {
                self.prepared_draw_slots.extend(1..SLOT_COUNT);
            }
        }
        self.finish_report();
        Ok(())
    }

    /// Prepare a regional upload/draw transaction without waiting for prior GPU
    /// use. Unsafe uploads are deferred and remain visible in the report.
    pub(crate) fn prepare_regional(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        draw: &RegionalResidentDraw,
    ) -> Result<(), RenderPreparationError> {
        draw.validate()
            .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
        self.preflight_grid(device, draw.cells)?;
        let params_bytes = PARAM_STRIDE
            .checked_mul(draw.capacity as u64)
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        let edge_bytes = edge_buffer_bytes(draw.cells)?;
        if params_bytes > device.limits().max_buffer_size
            || edge_bytes > device.limits().max_storage_buffer_binding_size as u64
            || edge_bytes > device.limits().max_buffer_size
        {
            return Err(RenderPreparationError::InvalidResidentTile);
        }

        let mut proposed_states = BTreeMap::<usize, TileSlotState>::new();
        let mut upload_bytes = BTreeMap::<usize, Option<u64>>::new();
        for upload in &draw.uploads {
            let physical_slot = REGIONAL_SLOT_BASE + upload.slot;
            let mut state = self
                .tile_slots
                .get(physical_slot)
                .map_or_else(TileSlotState::default, |slot| {
                    slot.publication_state.clone()
                });
            if !state.accept_publication(&upload.tile.publication, &upload.tile.tile.key) {
                return Err(RenderPreparationError::InvalidResidentTile);
            }
            let resident = self
                .tile_slots
                .get(physical_slot)
                .is_some_and(|slot| slot.resident_key.as_ref() == Some(&upload.tile.tile.key));
            let required = if resident {
                None
            } else {
                upload
                    .tile
                    .tile
                    .validate()
                    .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
                let bytes = (upload.tile.tile.texels.len() as u64)
                    .checked_mul(32)
                    .ok_or(RenderPreparationError::InvalidResidentTile)?;
                let capacity = bytes
                    .checked_next_power_of_two()
                    .ok_or(RenderPreparationError::InvalidResidentTile)?;
                if capacity > device.limits().max_storage_buffer_binding_size as u64
                    || capacity > device.limits().max_buffer_size
                {
                    return Err(RenderPreparationError::InvalidResidentTile);
                }
                Some(bytes)
            };
            proposed_states.insert(upload.slot, state);
            upload_bytes.insert(upload.slot, required);
        }
        for patch in &draw.patches {
            for (slot_index, tile_draw) in [
                (patch.own_slot, &patch.own),
                (patch.parent_slot, &patch.parent),
            ] {
                let physical_slot = REGIONAL_SLOT_BASE + slot_index;
                let mut state = proposed_states
                    .get(&slot_index)
                    .cloned()
                    .or_else(|| {
                        self.tile_slots
                            .get(physical_slot)
                            .map(|slot| slot.publication_state.clone())
                    })
                    .unwrap_or_default();
                if !state.accept_publication(&tile_draw.publication, &tile_draw.tile.key) {
                    return Err(RenderPreparationError::InvalidResidentTile);
                }
                if let Some(existing) = proposed_states.get(&slot_index)
                    && existing.generation() != state.generation()
                {
                    return Err(RenderPreparationError::InvalidResidentTile);
                }
                proposed_states.insert(slot_index, state);
            }
        }

        let completed = self.completed_submission.load(Ordering::Acquire);
        let upload_by_slot: BTreeMap<_, _> = draw
            .uploads
            .iter()
            .map(|upload| (upload.slot, upload))
            .collect();
        let mut blocked_dependencies = false;
        for patch in &draw.patches {
            for (slot_index, key) in [
                (patch.own_slot, &patch.own.tile.key),
                (patch.parent_slot, &patch.parent.tile.key),
            ] {
                if self
                    .tile_slots
                    .get(REGIONAL_SLOT_BASE + slot_index)
                    .is_some_and(|slot| slot.resident_key.as_ref() == Some(key))
                {
                    continue;
                }
                let Some(upload) = upload_by_slot.get(&slot_index) else {
                    return Err(RenderPreparationError::InvalidResidentTile);
                };
                if upload.tile.tile.key != *key {
                    return Err(RenderPreparationError::InvalidResidentTile);
                }
                let slot_in_flight = self
                    .tile_slots
                    .get(REGIONAL_SLOT_BASE + slot_index)
                    .is_some_and(|slot| slot.last_use_submission > completed);
                if slot_in_flight
                    || self
                        .prepared_draw_slots
                        .contains(&(REGIONAL_SLOT_BASE + slot_index))
                {
                    blocked_dependencies = true;
                }
            }
        }
        if blocked_dependencies {
            let mut report = self.regional_report.clone();
            report.capacity = draw.capacity;
            report.tile_upload_bytes = 0;
            report.boundary_upload_bytes = 0;
            report.metadata_upload_bytes = 0;
            report.tile_upload_count = 0;
            report.boundary_upload_count = 0;
            report.deferred_upload_count = draw.uploads.len() as u64;
            report.transfer_staging_bytes = 0;
            let diagnostic_mode = draw
                .patches
                .iter()
                .map(|patch| patch.own.mode)
                .find(|mode| (6..=10).contains(mode));
            if let (Some(previous), Some(reference)) = (
                self.regional_draw.as_mut(),
                draw.patches.first().map(|patch| &patch.own),
            ) {
                let reference_anchor = reference
                    .tile
                    .anchor_position_body()
                    .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
                let reference_mode = diagnostic_mode.unwrap_or(reference.mode);
                for patch in &mut previous.patches {
                    for tile_draw in [&mut patch.own, &mut patch.parent] {
                        let tile_anchor = tile_draw
                            .tile
                            .anchor_position_body()
                            .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
                        tile_draw.anchor_view_m = reference.anchor_view_m
                            + reference.body_to_view * (tile_anchor - reference_anchor);
                        tile_draw.body_to_view = reference.body_to_view;
                        tile_draw.sun_body = reference.sun_body;
                        tile_draw.mode = reference_mode;
                    }
                }
            }
            if let Some(previous) = self.regional_draw.clone() {
                let active_patch_indices = self.regional_active_patches.clone();
                for patch_index in active_patch_indices {
                    if let Some(patch) = previous.patches.get(patch_index) {
                        let parent_delta = if patch.quadrant.is_some() {
                            patch
                                .own
                                .tile
                                .anchor_position_body()
                                .and_then(|own| {
                                    patch
                                        .parent
                                        .tile
                                        .anchor_position_body()
                                        .map(|parent| own - parent)
                                })
                                .unwrap_or(glam::DVec3::ZERO)
                        } else {
                            glam::DVec3::ZERO
                        };
                        let params = pack_params(
                            &patch.own,
                            &patch.parent,
                            parent_delta,
                            patch.morph_fraction,
                            patch.boundary_fraction,
                            if patch.quadrant.is_some() { 4 } else { 3 },
                            patch.quadrant.unwrap_or([0, 0]),
                            patch.own_slot,
                            patch.parent_slot,
                            2.0,
                        );
                        self.write_params(queue, patch_index, &params, PrepareKind::Render);
                        report.metadata_upload_bytes += PARAM_BYTES;
                    }
                }
            }
            report.slots = (0..draw.capacity)
                .map(|index| {
                    let old = self.tile_slots.get(REGIONAL_SLOT_BASE + index);
                    let in_flight = old.is_some_and(|slot| slot.last_use_submission > completed);
                    let pinned = self
                        .regional_report
                        .slots
                        .get(index)
                        .is_some_and(|slot| slot.pinned);
                    RegionalSlotReport {
                        key: old.and_then(|slot| slot.resident_key.clone()),
                        generation: old.map_or(0, |slot| slot.publication_state.generation()),
                        reuse_safe: !in_flight && !pinned,
                        pinned,
                        in_flight,
                    }
                })
                .collect();
            report.in_flight_count = report.slots.iter().filter(|slot| slot.in_flight).count();
            report.pinned_count = report.slots.iter().filter(|slot| slot.pinned).count();
            report.resident_count = report
                .slots
                .iter()
                .filter(|slot| slot.key.is_some())
                .count();
            report.evictable_count = report.resident_count.saturating_sub(report.pinned_count);
            let regional_slots = self.tile_slots.iter().skip(REGIONAL_SLOT_BASE);
            report.allocated_slot_count = self.tile_slots.len().saturating_sub(REGIONAL_SLOT_BASE);
            report.tile_capacity_bytes = regional_slots.clone().map(|slot| slot.capacity).sum();
            report.boundary_capacity_bytes =
                regional_slots.clone().map(|slot| slot.edge_capacity).sum();
            report.active_tile_capacity_bytes = self
                .tile_slots
                .iter()
                .skip(REGIONAL_SLOT_BASE)
                .take(draw.capacity)
                .map(|slot| slot.capacity)
                .sum();
            report.active_boundary_capacity_bytes = self
                .tile_slots
                .iter()
                .skip(REGIONAL_SLOT_BASE)
                .take(draw.capacity)
                .map(|slot| slot.edge_capacity)
                .sum();
            report.metadata_capacity_bytes = self.params_buffer.size();
            report.grid_capacity_bytes = self.grid_capacity_bytes;
            report.validation_capacity_bytes = self.validation_buffer.size();
            report.fallback_active = true;
            self.regional_report = report;
            return Ok(());
        }
        if self.regional_cells != 0 && self.regional_cells != draw.cells {
            return Err(RenderPreparationError::InvalidResidentTile);
        }

        // All structural and payload checks have passed. Allocate the bounded
        // regional buffers only after the transaction is known to be valid.
        self.ensure_regional_resources(
            device,
            draw.capacity,
            draw.cells,
            params_bytes,
            edge_bytes,
        )?;
        self.clear_for_kind(PrepareKind::Render);
        let mut next_active = Vec::new();
        let mut report = RegionalResidentReport {
            capacity: draw.capacity,
            cumulative_tile_upload_bytes: self.regional_report.cumulative_tile_upload_bytes,
            cumulative_boundary_upload_bytes: self.regional_report.cumulative_boundary_upload_bytes,
            cumulative_tile_upload_count: self.regional_report.cumulative_tile_upload_count,
            cumulative_boundary_upload_count: self.regional_report.cumulative_boundary_upload_count,
            ..RegionalResidentReport::default()
        };

        for upload in &draw.uploads {
            let physical_slot = REGIONAL_SLOT_BASE + upload.slot;
            let slot = &self.tile_slots[physical_slot];
            let needs_content = upload_bytes[&upload.slot].is_some();
            let safe = !self.prepared_draw_slots.contains(&physical_slot)
                && slot.last_use_submission <= completed;
            if needs_content && !safe {
                report.deferred_upload_count += 1;
                continue;
            }
            if let Some(required) = upload_bytes[&upload.slot] {
                self.ensure_tile_capacity(device, physical_slot, required)?;
                let bytes = pack_tile(&upload.tile.tile);
                queue.write_buffer(&self.tile_slots[physical_slot].buffer, 0, &bytes);
                let slot = &mut self.tile_slots[physical_slot];
                slot.publication_state = proposed_states
                    .remove(&upload.slot)
                    .ok_or(RenderPreparationError::InvalidResidentTile)?;
                slot.resident_key = Some(upload.tile.tile.key.clone());
                slot.resident_tile = Some(Arc::clone(&upload.tile.tile));
                slot.own_edge_version = None;
                slot.parent_boundary = None;
                report.tile_upload_bytes += bytes.len() as u64;
                report.tile_upload_count += 1;
                report.cumulative_tile_upload_bytes = report
                    .cumulative_tile_upload_bytes
                    .saturating_add(bytes.len() as u64);
                report.cumulative_tile_upload_count =
                    report.cumulative_tile_upload_count.saturating_add(1);
            } else {
                self.tile_slots[physical_slot].publication_state = proposed_states
                    .remove(&upload.slot)
                    .ok_or(RenderPreparationError::InvalidResidentTile)?;
            }
        }
        for (slot_index, state) in proposed_states {
            if let Some(slot) = self.tile_slots.get_mut(REGIONAL_SLOT_BASE + slot_index)
                && slot.resident_key.as_ref() == state.requested_key.as_ref()
            {
                slot.publication_state = state;
            }
        }

        let mut used_slots = BTreeSet::new();
        let mut active_pairs = BTreeSet::new();
        for (patch_index, patch) in draw.patches.iter().enumerate() {
            let own_physical_slot = REGIONAL_SLOT_BASE + patch.own_slot;
            let parent_physical_slot = REGIONAL_SLOT_BASE + patch.parent_slot;
            let own_ready = self.tile_slots[own_physical_slot].resident_key.as_ref()
                == Some(&patch.own.tile.key);
            let parent_ready = self.tile_slots[parent_physical_slot].resident_key.as_ref()
                == Some(&patch.parent.tile.key);
            if !own_ready || !parent_ready {
                continue;
            }
            let delta = patch
                .own
                .tile
                .anchor_position_body()
                .and_then(|own_anchor| {
                    patch
                        .parent
                        .tile
                        .anchor_position_body()
                        .map(|parent_anchor| own_anchor - parent_anchor)
                })
                .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
            crate::RenderPrecisionBudget::near_debug()
                .try_view_relative_position(delta)
                .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
            if self.tile_slots[own_physical_slot].own_edge_version
                != Some(patch.boundary_endpoints.version)
            {
                let packed = pack_own_boundaries(&patch.boundary_endpoints, draw.cells);
                queue.write_buffer(&self.tile_slots[own_physical_slot].edge_buffer, 0, &packed);
                self.tile_slots[own_physical_slot].own_edge_version =
                    Some(patch.boundary_endpoints.version);
                report.boundary_upload_bytes += packed.len() as u64;
                report.boundary_upload_count += 1;
                report.cumulative_boundary_upload_bytes = report
                    .cumulative_boundary_upload_bytes
                    .saturating_add(packed.len() as u64);
                report.cumulative_boundary_upload_count =
                    report.cumulative_boundary_upload_count.saturating_add(1);
            }
            if self.tile_slots[parent_physical_slot]
                .parent_boundary
                .as_ref()
                != Some(&patch.boundary_endpoints.parent)
            {
                let packed = pack_parent_boundary(&patch.boundary_endpoints, draw.cells);
                queue.write_buffer(
                    &self.tile_slots[parent_physical_slot].edge_buffer,
                    edge_layer_offset(draw.cells, 2)?,
                    &packed,
                );
                self.tile_slots[parent_physical_slot].parent_boundary =
                    Some(patch.boundary_endpoints.parent.clone());
                report.boundary_upload_bytes += packed.len() as u64;
                report.boundary_upload_count += 1;
                report.cumulative_boundary_upload_bytes = report
                    .cumulative_boundary_upload_bytes
                    .saturating_add(packed.len() as u64);
                report.cumulative_boundary_upload_count =
                    report.cumulative_boundary_upload_count.saturating_add(1);
            }
            let parent_delta = if let Some(quadrant) = patch.quadrant {
                let parent_st = [f64::from(quadrant[0]) * 0.5, f64::from(quadrant[1]) * 0.5];
                let _ = parent_st;
                delta
            } else {
                glam::DVec3::ZERO
            };
            let kind = if patch.quadrant.is_some() { 4 } else { 3 };
            let quadrant = patch.quadrant.unwrap_or([0, 0]);
            if !self
                .tile_groups
                .contains_key(&(own_physical_slot, parent_physical_slot))
            {
                let group = create_tile_group(
                    device,
                    &self.tile_layout,
                    &self.tile_slots[own_physical_slot],
                    &self.tile_slots[parent_physical_slot],
                );
                self.tile_groups
                    .insert((own_physical_slot, parent_physical_slot), group);
            }
            active_pairs.insert((own_physical_slot, parent_physical_slot));
            let params = pack_params(
                &patch.own,
                &patch.parent,
                parent_delta,
                patch.morph_fraction,
                patch.boundary_fraction,
                kind,
                quadrant,
                patch.own_slot,
                patch.parent_slot,
                if patch.quality_fallback { 1.0 } else { 0.0 },
            );
            self.write_params(queue, patch_index, &params, PrepareKind::Render);
            report.metadata_upload_bytes += PARAM_BYTES;
            used_slots.insert(own_physical_slot);
            used_slots.insert(parent_physical_slot);
            next_active.push(patch_index);
        }
        self.prepared_draw_slots = used_slots.clone();
        self.regional_active_patches = next_active;
        self.regional_draw = Some(draw.clone());
        self.tile_groups.retain(|(own, parent), _| {
            (*own < SLOT_COUNT && *parent < SLOT_COUNT) || active_pairs.contains(&(*own, *parent))
        });
        let pinned: BTreeSet<_> = used_slots;
        let regional_slots =
            &self.tile_slots[REGIONAL_SLOT_BASE..REGIONAL_SLOT_BASE + draw.capacity];
        report.resident_count = regional_slots
            .iter()
            .filter(|slot| slot.resident_key.is_some())
            .count();
        report.pinned_count = pinned.len();
        report.evictable_count = report.resident_count.saturating_sub(report.pinned_count);
        let allocated_regional_slots = self.tile_slots.iter().skip(REGIONAL_SLOT_BASE);
        report.allocated_slot_count = self.tile_slots.len().saturating_sub(REGIONAL_SLOT_BASE);
        report.tile_capacity_bytes = allocated_regional_slots
            .clone()
            .map(|slot| slot.capacity)
            .sum();
        report.boundary_capacity_bytes = allocated_regional_slots
            .map(|slot| slot.edge_capacity)
            .sum();
        report.active_tile_capacity_bytes = regional_slots.iter().map(|slot| slot.capacity).sum();
        report.active_boundary_capacity_bytes =
            regional_slots.iter().map(|slot| slot.edge_capacity).sum();
        report.metadata_capacity_bytes = self.params_buffer.size();
        report.grid_capacity_bytes = self.grid_capacity_bytes;
        report.validation_capacity_bytes = self.validation_buffer.size();
        report.slots = regional_slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let in_flight = slot.last_use_submission > completed;
                let is_pinned = pinned.contains(&(REGIONAL_SLOT_BASE + index));
                RegionalSlotReport {
                    key: slot.resident_key.clone(),
                    generation: slot.publication_state.generation(),
                    reuse_safe: !in_flight && !is_pinned,
                    pinned: is_pinned,
                    in_flight,
                }
            })
            .collect();
        report.in_flight_count = report.slots.iter().filter(|slot| slot.in_flight).count();
        report.transfer_staging_bytes = report
            .tile_upload_bytes
            .saturating_add(report.boundary_upload_bytes);
        report.active_morph_count = draw
            .patches
            .iter()
            .enumerate()
            .filter(|(index, patch)| {
                self.regional_active_patches.contains(index)
                    && (patch.morph_fraction < 1.0 || patch.boundary_fraction < 1.0)
            })
            .count();
        self.regional_report = report;
        self.regional_cells = draw.cells;
        self.finish_report();
        Ok(())
    }

    pub(crate) fn draw_regional(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        projection: &wgpu::BindGroup,
        draw: &RegionalResidentDraw,
    ) {
        let active_draw = self.regional_draw.as_ref().unwrap_or(draw);
        for &patch_index in &self.regional_active_patches {
            if let Some(patch) = active_draw.patches.get(patch_index) {
                self.draw_patch(
                    pass,
                    projection,
                    REGIONAL_SLOT_BASE + patch.own_slot,
                    REGIONAL_SLOT_BASE + patch.parent_slot,
                    patch_index,
                );
            }
        }
    }

    pub(crate) fn regional_report(&self) -> RegionalResidentReport {
        let mut report = self.regional_report.clone();
        let completed = self.completed_submission.load(Ordering::Acquire);
        for (index, slot_report) in report.slots.iter_mut().enumerate() {
            slot_report.in_flight = self
                .tile_slots
                .get(REGIONAL_SLOT_BASE + index)
                .is_some_and(|slot| slot.last_use_submission > completed);
            slot_report.reuse_safe = !slot_report.in_flight && !slot_report.pinned;
        }
        report.in_flight_count = report.slots.iter().filter(|slot| slot.in_flight).count();
        report
    }

    /// Associate slots read by the just-submitted frame with a nonblocking
    /// queue-completion callback. No device polling or wait occurs here.
    pub(crate) fn on_submitted(&mut self, queue: &wgpu::Queue) {
        if self.prepared_draw_slots.is_empty() {
            return;
        }
        self.submission_serial = self.submission_serial.saturating_add(1);
        let serial = self.submission_serial;
        for &slot in &self.prepared_draw_slots {
            if let Some(slot) = self.tile_slots.get_mut(slot) {
                slot.last_use_submission = serial;
            }
        }
        self.prepared_draw_slots.clear();
        let completed = Arc::clone(&self.completed_submission);
        queue.on_submitted_work_done(move || {
            completed.fetch_max(serial, Ordering::Release);
        });
    }

    fn ensure_regional_resources(
        &mut self,
        device: &wgpu::Device,
        capacity: usize,
        cells: u32,
        params_bytes: u64,
        edge_bytes: u64,
    ) -> Result<(), RenderPreparationError> {
        let required_slot_count = REGIONAL_SLOT_BASE
            .checked_add(capacity)
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        if required_slot_count > self.tile_slots.len() {
            let old_len = self.tile_slots.len();
            self.tile_slots.extend(
                (self.tile_slots.len()..required_slot_count)
                    .map(|index| ResidentTileGpuSlot::new(device, index)),
            );
            self.rebuild_tile_groups(device);
            self.allocation_count = self
                .allocation_count
                .saturating_add((required_slot_count - old_len) as u32);
        }
        if self.params_buffer.size() < params_bytes {
            self.params_buffer = create_buffer(
                device,
                params_bytes,
                wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                "Regional resident patch parameters",
            );
            self.params_group = params_group(
                device,
                &self.params_layout,
                &self.params_buffer,
                &self.validation_buffer,
            );
            self.allocation_count = self.allocation_count.saturating_add(1);
        }
        let mut edge_changed = false;
        for slot in self.tile_slots.iter_mut().skip(REGIONAL_SLOT_BASE) {
            if slot.edge_capacity != edge_bytes {
                slot.edge_buffer = create_buffer(
                    device,
                    edge_bytes,
                    wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    "Regional resident static boundary endpoints",
                );
                slot.edge_capacity = edge_bytes;
                slot.own_edge_version = None;
                slot.parent_boundary = None;
                edge_changed = true;
                self.allocation_count = self.allocation_count.saturating_add(1);
            }
        }
        if edge_changed {
            self.rebuild_tile_groups(device);
        }
        self.ensure_grid(device, cells)?;
        Ok(())
    }

    fn rebuild_tile_groups(&mut self, device: &wgpu::Device) {
        self.tile_groups.clear();
        for own in 0..SLOT_COUNT {
            for parent in 0..SLOT_COUNT {
                let group = create_tile_group(
                    device,
                    &self.tile_layout,
                    &self.tile_slots[own],
                    &self.tile_slots[parent],
                );
                self.tile_groups.insert((own, parent), group);
            }
        }
    }

    fn preflight_slot(
        &self,
        device: &wgpu::Device,
        index: usize,
        draw: &TileDraw,
    ) -> Result<SlotPreflight, RenderPreparationError> {
        let slot = self
            .tile_slots
            .get(index)
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        draw.tile
            .validate_layout()
            .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
        let mut publication_state = slot.publication_state.clone();
        if !publication_state.accept_publication(&draw.publication, &draw.tile.key) {
            return Err(RenderPreparationError::InvalidResidentTile);
        }
        let required_capacity = if slot.resident_key.as_ref() == Some(&draw.tile.key) {
            None
        } else {
            draw.tile
                .validate()
                .map_err(|_| RenderPreparationError::InvalidResidentTile)?;
            let payload_bytes = (draw.tile.texels.len() as u64)
                .checked_mul(32)
                .ok_or(RenderPreparationError::InvalidResidentTile)?;
            let capacity = payload_bytes
                .checked_next_power_of_two()
                .ok_or(RenderPreparationError::InvalidResidentTile)?;
            if capacity > device.limits().max_storage_buffer_binding_size as u64
                || capacity > device.limits().max_buffer_size
            {
                return Err(RenderPreparationError::InvalidResidentTile);
            }
            Some(payload_bytes)
        };
        Ok(SlotPreflight {
            publication_state,
            required_capacity,
        })
    }

    fn publish_slot(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        index: usize,
        draw: &TileDraw,
        preflight: SlotPreflight,
        kind: PrepareKind,
    ) -> Result<(), RenderPreparationError> {
        if self.tile_slots[index].resident_key.as_ref() == Some(&draw.tile.key) {
            self.tile_slots[index].publication_state = preflight.publication_state;
            return Ok(());
        }
        let required = preflight
            .required_capacity
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        self.ensure_tile_capacity(device, index, required)?;
        let pack_start = std::time::Instant::now();
        let packed = pack_tile(&draw.tile);
        let pack_duration = pack_start.elapsed();
        let upload_start = std::time::Instant::now();
        queue.write_buffer(&self.tile_slots[index].buffer, 0, &packed);
        let upload_api_duration = upload_start.elapsed();
        {
            let slot = &mut self.tile_slots[index];
            slot.publication_state = preflight.publication_state;
            slot.resident_key = Some(draw.tile.key.clone());
            slot.resident_tile = Some(Arc::clone(&draw.tile));
            slot.own_edge_version = None;
            slot.parent_boundary = None;
        }
        match kind {
            PrepareKind::Render => {
                self.report.tile_content_upload_bytes = self
                    .report
                    .tile_content_upload_bytes
                    .saturating_add(packed.len() as u64);
                self.report.tile_content_upload_count =
                    self.report.tile_content_upload_count.saturating_add(1);
                self.report.tile_content_pack_bytes = self
                    .report
                    .tile_content_pack_bytes
                    .saturating_add(packed.len() as u64);
                self.report.tile_content_pack_duration += pack_duration;
                self.report.tile_content_upload_api_duration += upload_api_duration;
                self.report.cumulative_content_upload_bytes = self
                    .report
                    .cumulative_content_upload_bytes
                    .saturating_add(packed.len() as u64);
                self.report.cumulative_content_upload_count = self
                    .report
                    .cumulative_content_upload_count
                    .saturating_add(1);
                self.hierarchy_report.slot_content_upload_bytes[index] = packed.len() as u64;
            }
            PrepareKind::Validation => {
                self.report.validation_content_upload_bytes = self
                    .report
                    .validation_content_upload_bytes
                    .saturating_add(packed.len() as u64);
                self.report.validation_content_upload_count = self
                    .report
                    .validation_content_upload_count
                    .saturating_add(1);
                self.report.validation_pack_duration += pack_duration;
                self.report.validation_upload_api_duration += upload_api_duration;
                self.report.cumulative_validation_upload_bytes = self
                    .report
                    .cumulative_validation_upload_bytes
                    .saturating_add(packed.len() as u64);
                self.report.cumulative_validation_upload_count = self
                    .report
                    .cumulative_validation_upload_count
                    .saturating_add(1);
            }
        }
        Ok(())
    }

    fn ensure_tile_capacity(
        &mut self,
        device: &wgpu::Device,
        index: usize,
        required: u64,
    ) -> Result<(), RenderPreparationError> {
        if required > device.limits().max_storage_buffer_binding_size as u64
            || required > device.limits().max_buffer_size
        {
            return Err(RenderPreparationError::InvalidResidentTile);
        }
        let slot = self
            .tile_slots
            .get_mut(index)
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        if required > slot.capacity {
            slot.capacity = required.next_power_of_two();
            slot.buffer = create_buffer(
                device,
                slot.capacity,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                "Resident terrain bounded hierarchy slot",
            );
            self.allocation_count += 1;
            self.rebuild_tile_groups(device);
        }
        Ok(())
    }

    fn ensure_grid(
        &mut self,
        device: &wgpu::Device,
        cells: u32,
    ) -> Result<(), RenderPreparationError> {
        if self.grid_cells != cells {
            self.create_grid(device, cells)?;
        }
        Ok(())
    }

    fn preflight_grid(
        &self,
        device: &wgpu::Device,
        cells: u32,
    ) -> Result<(), RenderPreparationError> {
        let side = cells
            .checked_add(1)
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        let vertex_bytes = u64::from(side)
            .checked_mul(u64::from(side))
            .and_then(|count| count.checked_mul(8))
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        let index_bytes = u64::from(cells)
            .checked_mul(u64::from(cells))
            .and_then(|count| count.checked_mul(24))
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        if cells == 0
            || cells > TileData::MAX_CELLS
            || vertex_bytes > device.limits().max_buffer_size
            || index_bytes > device.limits().max_buffer_size
        {
            return Err(RenderPreparationError::InvalidResidentTile);
        }
        Ok(())
    }

    fn write_params(
        &mut self,
        queue: &wgpu::Queue,
        slot: usize,
        params: &[u8; PARAM_BYTES as usize],
        kind: PrepareKind,
    ) {
        queue.write_buffer(&self.params_buffer, slot as u64 * PARAM_STRIDE, params);
        match kind {
            PrepareKind::Render => {
                self.report.metadata_upload_bytes = self
                    .report
                    .metadata_upload_bytes
                    .saturating_add(params.len() as u64)
            }
            PrepareKind::Validation => {
                self.report.validation_metadata_upload_bytes = self
                    .report
                    .validation_metadata_upload_bytes
                    .saturating_add(params.len() as u64)
            }
        }
    }

    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, projection: &wgpu::BindGroup) {
        self.draw_patch(pass, projection, 0, 0, 0);
    }

    pub(crate) fn draw_hierarchy(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        projection: &wgpu::BindGroup,
        draw_children: bool,
    ) {
        if draw_children {
            for patch_index in 1..SLOT_COUNT {
                self.draw_patch(pass, projection, patch_index, 0, patch_index);
            }
        } else {
            self.draw_patch(pass, projection, 0, 0, 0);
        }
    }

    fn draw_patch(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        projection: &wgpu::BindGroup,
        own_slot: usize,
        parent_slot: usize,
        params_slot: usize,
    ) {
        let (Some(vertices), Some(indices)) = (&self.grid_vertices, &self.grid_indices) else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, projection, &[]);
        let Some(tile_group) = self.tile_groups.get(&(own_slot, parent_slot)) else {
            return;
        };
        pass.set_bind_group(1, tile_group, &[]);
        pass.set_bind_group(
            2,
            &self.params_group,
            &[(params_slot as u64 * PARAM_STRIDE) as u32],
        );
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }

    pub(crate) fn report(&self) -> ResidentTileReport {
        self.report.clone()
    }

    pub(crate) fn hierarchy_report(&self) -> ResidentHierarchyReport {
        let mut report = self.hierarchy_report.clone();
        report.resources = self.report();
        report.resident_keys = std::array::from_fn(|index| {
            self.tile_slots
                .get(index)
                .and_then(|slot| slot.resident_key.clone())
        });
        report.slot_generations = std::array::from_fn(|index| {
            self.tile_slots
                .get(index)
                .map_or(0, |slot| slot.publication_state.generation())
        });
        report
    }

    pub(crate) fn clear_frame(&mut self) {
        self.prepared_draw_slots.clear();
        self.regional_active_patches.clear();
        self.regional_draw = None;
        self.clear_for_kind(PrepareKind::Render);
        self.hierarchy_report.parent_pinned = false;
        self.hierarchy_report.draw_children = false;
        self.hierarchy_report.morph_fraction = 0.0;
        self.finish_report();
    }

    fn clear_for_kind(&mut self, kind: PrepareKind) {
        match kind {
            PrepareKind::Render => {
                self.report.tile_content_upload_bytes = 0;
                self.report.tile_content_upload_count = 0;
                self.report.tile_content_pack_bytes = 0;
                self.report.tile_content_pack_duration = std::time::Duration::ZERO;
                self.report.tile_content_upload_api_duration = std::time::Duration::ZERO;
                self.report.metadata_upload_bytes = 0;
                self.hierarchy_report.slot_content_upload_bytes = [0; SLOT_COUNT];
            }
            PrepareKind::Validation => {
                self.report.validation_content_upload_bytes = 0;
                self.report.validation_content_upload_count = 0;
                self.report.validation_pack_duration = std::time::Duration::ZERO;
                self.report.validation_upload_api_duration = std::time::Duration::ZERO;
                self.report.validation_metadata_upload_bytes = 0;
            }
        }
    }

    fn finish_report(&mut self) {
        self.report.cpu_retained_payload_bytes = self
            .tile_slots
            .iter()
            .filter_map(|slot| slot.resident_tile.as_ref())
            .map(|tile| tile.retained_payload_bytes())
            .sum();
        self.report.gpu_tile_payload_bytes = self
            .tile_slots
            .iter()
            .filter_map(|slot| slot.resident_tile.as_ref())
            .map(|tile| tile.texels.len() as u64 * 32)
            .sum();
        self.report.resident_key = self
            .tile_slots
            .first()
            .and_then(|slot| slot.resident_key.clone());
        self.report.slot_generation = self
            .tile_slots
            .first()
            .map_or(0, |slot| slot.publication_state.generation());
        self.report.grid_bytes = self.grid_capacity_bytes;
        self.report.allocation_count = self.allocation_count;
        self.report.allocation_capacity_bytes = self
            .tile_slots
            .iter()
            .map(|slot| slot.capacity)
            .sum::<u64>()
            + self
                .tile_slots
                .iter()
                .map(|slot| slot.edge_capacity)
                .sum::<u64>()
            + self.params_buffer.size()
            + self.validation_buffer.size()
            + self.grid_capacity_bytes;
        self.report.validation_output_buffer_bytes = self.validation_buffer.size();
        self.hierarchy_report.resources = self.report.clone();
        self.hierarchy_report.resident_keys = std::array::from_fn(|index| {
            self.tile_slots
                .get(index)
                .and_then(|slot| slot.resident_key.clone())
        });
        self.hierarchy_report.slot_generations = std::array::from_fn(|index| {
            self.tile_slots
                .get(index)
                .map_or(0, |slot| slot.publication_state.generation())
        });
    }

    pub(crate) fn validate_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        projection_group: &wgpu::BindGroup,
        draw: &TileDraw,
    ) -> Result<Vec<ReconstructedTileVertex>, RenderPreparationError> {
        let preflight = self.preflight_slot(device, 0, draw)?;
        self.preflight_grid(device, draw.tile.key.cells)?;
        self.clear_for_kind(PrepareKind::Validation);
        self.publish_slot(device, queue, 0, draw, preflight, PrepareKind::Validation)?;
        self.ensure_grid(device, draw.tile.key.cells)?;
        let params = pack_params(
            draw,
            draw,
            glam::DVec3::ZERO,
            1.0,
            1.0,
            0,
            [0, 0],
            0,
            0,
            0.0,
        );
        self.write_params(queue, 0, &params, PrepareKind::Validation);
        self.finish_report();
        self.validate_grid_gpu(
            device,
            queue,
            projection_group,
            0,
            0,
            0,
            draw.tile.key.cells,
        )
    }

    pub(crate) fn validate_hierarchy_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        projection_group: &wgpu::BindGroup,
        draw: &ResidentHierarchyDraw,
        patch_index: usize,
    ) -> Result<Vec<ReconstructedTileVertex>, RenderPreparationError> {
        if patch_index >= SLOT_COUNT
            || (patch_index > 0 && draw.children[patch_index - 1].is_none())
        {
            return Err(RenderPreparationError::InvalidResidentTile);
        }
        self.prepare_hierarchy_for(device, queue, draw, PrepareKind::Validation)?;
        self.validate_grid_gpu(
            device,
            queue,
            projection_group,
            patch_index,
            0,
            patch_index,
            draw.parent.tile.key.cells,
        )
    }

    pub(crate) fn validate_regional_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        projection_group: &wgpu::BindGroup,
        draw: &RegionalResidentDraw,
        patch_index: usize,
    ) -> Result<Vec<ReconstructedTileVertex>, RenderPreparationError> {
        let patch = draw
            .patches
            .get(patch_index)
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        self.prepare_regional(device, queue, draw)?;
        if !self.regional_active_patches.contains(&patch_index) {
            return Err(RenderPreparationError::InvalidResidentTile);
        }
        let output = self.validate_grid_gpu(
            device,
            queue,
            projection_group,
            REGIONAL_SLOT_BASE + patch.own_slot,
            REGIONAL_SLOT_BASE + patch.parent_slot,
            patch_index,
            draw.cells,
        )?;
        self.regional_report.validation_readback_bytes =
            u64::from(draw.cells + 1).pow(2) * VALIDATION_VERTEX_BYTES;
        // The diagnostic waits for its readback, so it can release its own
        // slot use immediately without depending on an ordinary-frame callback.
        self.prepared_draw_slots.clear();
        self.completed_submission
            .fetch_max(self.submission_serial, Ordering::Release);
        Ok(output)
    }

    // Mirrors the GPU dispatch tuple: output slot pair, parameter slot, and grid resolution.
    #[allow(clippy::too_many_arguments)]
    fn validate_grid_gpu(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        projection_group: &wgpu::BindGroup,
        own_slot: usize,
        parent_slot: usize,
        params_slot: usize,
        cells: u32,
    ) -> Result<Vec<ReconstructedTileVertex>, RenderPreparationError> {
        let count = (cells + 1)
            .checked_pow(2)
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        let bytes = u64::from(count) * VALIDATION_VERTEX_BYTES;
        self.hierarchy_report.validation_readback_bytes = bytes;
        let readback = create_buffer(
            device,
            bytes,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            "Resident terrain validation readback",
        );
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Resident terrain validation encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Resident terrain CPU/GPU reconstruction proof"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.validation_pipeline);
            pass.set_bind_group(0, projection_group, &[]);
            let tile_group = self
                .tile_groups
                .get(&(own_slot, parent_slot))
                .ok_or(RenderPreparationError::InvalidResidentTile)?;
            pass.set_bind_group(1, tile_group, &[]);
            pass.set_bind_group(
                2,
                &self.params_group,
                &[(params_slot as u64 * PARAM_STRIDE) as u32],
            );
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.validation_buffer, 0, &readback, 0, bytes);
        let submission = queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
        receiver
            .recv()
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
        let mapped = readback.get_mapped_range(..);
        let mut output = Vec::with_capacity(count as usize);
        for record in mapped.as_chunks::<{ VALIDATION_VERTEX_BYTES as usize }>().0 {
            let values: [f32; 16] = std::array::from_fn(|index| {
                let start = index * 4;
                f32::from_le_bytes(record[start..start + 4].try_into().unwrap())
            });
            output.push(ReconstructedTileVertex {
                position_local_m: values[0..3].try_into().unwrap(),
                position_view_m: values[4..7].try_into().unwrap(),
                normal_body: values[8..11].try_into().unwrap(),
                material: values[12..16].try_into().unwrap(),
            });
        }
        drop(mapped);
        readback.unmap();
        Ok(output)
    }
    fn create_grid(
        &mut self,
        device: &wgpu::Device,
        cells: u32,
    ) -> Result<(), RenderPreparationError> {
        let side = cells
            .checked_add(1)
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        let vertex_count = side
            .checked_mul(side)
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        let mut vertices = Vec::with_capacity(vertex_count as usize * 8);
        for y in 0..=cells {
            for x in 0..=cells {
                vertices.extend_from_slice(&(x as f32 / cells as f32).to_le_bytes());
                vertices.extend_from_slice(&(y as f32 / cells as f32).to_le_bytes());
            }
        }
        let index_count = cells
            .checked_mul(cells)
            .and_then(|value| value.checked_mul(6))
            .ok_or(RenderPreparationError::InvalidResidentTile)?;
        let mut indices = Vec::with_capacity(index_count as usize * 4);
        for y in 0..cells {
            for x in 0..cells {
                let a = y * side + x;
                let b = a + 1;
                let c = a + side;
                let d = c + 1;
                for index in [a, b, c, b, d, c] {
                    indices.extend_from_slice(&index.to_le_bytes());
                }
            }
        }
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Reusable resident tile grid coordinates"),
            contents: &vertices,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Reusable resident tile grid indices"),
            contents: &indices,
            usage: wgpu::BufferUsages::INDEX,
        });
        self.grid_vertices = Some(vertex_buffer);
        self.grid_indices = Some(index_buffer);
        self.grid_capacity_bytes = vertices.len() as u64 + indices.len() as u64;
        self.grid_cells = cells;
        self.index_count = index_count;
        self.allocation_count += 2;
        Ok(())
    }
}

fn pack_tile(tile: &TileData) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(tile.texels.len() * 32);
    for texel in &tile.texels {
        for value in [texel.radial_offset_m, 0.0, 0.0, 0.0]
            .into_iter()
            .chain(texel.material)
        {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn edge_layer_offset(cells: u32, layer: u64) -> Result<u64, RenderPreparationError> {
    u64::from(cells + 1)
        .checked_mul(4)
        .and_then(|vertices| vertices.checked_mul(48))
        .and_then(|bytes| bytes.checked_mul(layer))
        .ok_or(RenderPreparationError::InvalidResidentTile)
}

fn edge_buffer_bytes(cells: u32) -> Result<u64, RenderPreparationError> {
    edge_layer_offset(cells, 3)
}

fn pack_own_boundaries(
    endpoints: &crate::regional_resident::RegionalBoundaryEndpoints,
    cells: u32,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(2 * 4 * (cells as usize + 1) * 48);
    for boundary in [&endpoints.own_coarse, &endpoints.own_fine] {
        for edge in &boundary.edges {
            for vertex in edge {
                pack_boundary_vertex(&mut bytes, vertex);
            }
        }
    }
    bytes
}

fn pack_parent_boundary(
    endpoints: &crate::regional_resident::RegionalBoundaryEndpoints,
    _cells: u32,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 * endpoints.parent.edges[0].len() * 48);
    for edge in &endpoints.parent.edges {
        for vertex in edge {
            pack_boundary_vertex(&mut bytes, vertex);
        }
    }
    bytes
}

fn pack_boundary_vertex(bytes: &mut Vec<u8>, vertex: &crate::regional_edges::BoundaryVertex) {
    for value in [
        vertex.position_local_m.x as f32,
        vertex.position_local_m.y as f32,
        vertex.position_local_m.z as f32,
        0.0,
        vertex.normal_varying_body.x as f32,
        vertex.normal_varying_body.y as f32,
        vertex.normal_varying_body.z as f32,
        0.0,
    ]
    .into_iter()
    .chain(vertex.material.map(|value| value as f32))
    {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}

// Packs the complete fixed WGSL uniform ABI without intermediate allocations.
#[allow(clippy::too_many_arguments)]
fn pack_params(
    draw: &TileDraw,
    parent: &TileDraw,
    parent_delta: glam::DVec3,
    morph_fraction: f32,
    boundary_fraction: f32,
    patch_kind: u32,
    quadrant: [u32; 2],
    own_slot: usize,
    parent_slot: usize,
    fallback_kind: f32,
) -> [u8; PARAM_BYTES as usize] {
    let mut values = [0.0_f32; 96];
    values[..44].copy_from_slice(&pack_tile_params(draw));
    values[44..88].copy_from_slice(&pack_tile_params(parent));
    values[42] = f32::from(draw.tile.key.address.level());
    values[43] = own_slot as f32;
    values[79] = fallback_kind;
    values[86] = f32::from(parent.tile.key.address.level());
    values[87] = parent_slot as f32;
    values[88..92].copy_from_slice(&[
        parent_delta.x as f32,
        parent_delta.y as f32,
        parent_delta.z as f32,
        morph_fraction,
    ]);
    values[92..96].copy_from_slice(&[
        patch_kind as f32,
        quadrant[0] as f32,
        quadrant[1] as f32,
        if patch_kind >= 3 {
            boundary_fraction
        } else {
            0.0
        },
    ]);
    let mut bytes = [0_u8; PARAM_BYTES as usize];
    for (value, output) in values.into_iter().zip(bytes.as_chunks_mut::<4>().0) {
        output.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn pack_tile_params(draw: &TileDraw) -> [f32; 44] {
    let address = draw.tile.key.address;
    let [n, u, v] = address.face().basis();
    let [x, y] = address.coordinates();
    let scale = 2.0_f64.powi(i32::from(address.level()));
    let q0 = n
        + u * (2.0 * (f64::from(x) + 0.5) / scale - 1.0)
        + v * (2.0 * (f64::from(y) + 0.5) / scale - 1.0);
    let n0 = q0.normalize();
    let columns = draw.body_to_view.to_cols_array_2d();
    let mut values = [0.0_f32; 44];
    let mut cursor = 0;
    let push = |values: &mut [f32; 44], cursor: &mut usize, xyz: [f64; 3], w: f64| {
        for value in xyz {
            values[*cursor] = value as f32;
            *cursor += 1;
        }
        values[*cursor] = w as f32;
        *cursor += 1;
    };
    push(&mut values, &mut cursor, draw.anchor_view_m.to_array(), 1.0);
    for column in columns {
        push(&mut values, &mut cursor, column.map(f64::from), 0.0);
    }
    push(&mut values, &mut cursor, n.to_array(), 0.0);
    push(&mut values, &mut cursor, u.to_array(), 0.0);
    push(&mut values, &mut cursor, v.to_array(), 0.0);
    push(
        &mut values,
        &mut cursor,
        [2.0 / scale, q0.length(), f64::from(draw.tile.key.cells)],
        f64::from(draw.mode),
    );
    push(&mut values, &mut cursor, draw.sun_body.to_array(), 0.0);
    push(
        &mut values,
        &mut cursor,
        n0.to_array(),
        draw.tile.anchor_radius_m,
    );
    push(
        &mut values,
        &mut cursor,
        [
            draw.tile.min_max_radial_offset_m[0],
            draw.tile.min_max_radial_offset_m[1],
            0.0,
        ],
        0.0,
    );
    values
}
fn create_buffer(
    device: &wgpu::Device,
    size: u64,
    usage: wgpu::BufferUsages,
    label: &str,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(4),
        usage,
        mapped_at_creation: false,
    })
}

fn create_legacy_tile_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    slots: &[ResidentTileGpuSlot],
) -> BTreeMap<(usize, usize), wgpu::BindGroup> {
    let mut groups = BTreeMap::new();
    for own in 0..SLOT_COUNT {
        for parent in 0..SLOT_COUNT {
            groups.insert(
                (own, parent),
                create_tile_group(device, layout, &slots[own], &slots[parent]),
            );
        }
    }
    groups
}

fn create_tile_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    own: &ResidentTileGpuSlot,
    parent: &ResidentTileGpuSlot,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Resident terrain own/parent slot binding"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: own.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: parent.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: own.edge_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: parent.edge_buffer.as_entire_binding(),
            },
        ],
    })
}

fn params_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    params: &wgpu::Buffer,
    validation: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Resident terrain parameters"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: params,
                    offset: 0,
                    size: wgpu::BufferSize::new(PARAM_BYTES),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: validation.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod regional_pressure_tests {
    use super::*;
    use crate::regional_edges::build_boundaries;
    use crate::regional_resident::{
        RegionalBoundaryEndpoints, RegionalPatchDraw, RegionalTileUpload,
    };
    use crate::resident_tile::{TILE_FILTER_VERSION, TILE_FORMAT_VERSION, TileTexel};
    use glam::{DMat3, DVec3};
    use mundaris_math::surface::{CubeFace, CubePatchAddress};

    fn tile(address: CubePatchAddress, revision: u64) -> Arc<TileData> {
        const CELLS: u32 = 2;
        Arc::new(TileData {
            key: TileKey {
                body_identity: 912,
                definition_words: vec![31, 47],
                radius_bits: 80_000.0_f64.to_bits(),
                surface_revision: revision,
                material_revision: 2,
                format_version: TILE_FORMAT_VERSION,
                filter_version: TILE_FILTER_VERSION,
                address,
                cells: CELLS,
            },
            anchor_radius_m: 80_000.0,
            min_max_radial_offset_m: [-1.0, 1.0],
            texels: vec![
                TileTexel {
                    radial_offset_m: 0.0,
                    material: [0.25, 0.25, 0.25, 0.25],
                };
                ((CELLS + 3) * (CELLS + 3)) as usize
            ],
        })
    }

    fn draw(
        tile: Arc<TileData>,
        state: &mut TileSlotState,
        anchor_view_m: DVec3,
        body_to_view: DMat3,
        sun_body: DVec3,
    ) -> TileDraw {
        let publication = state.request(&tile.key).unwrap();
        TileDraw {
            tile,
            publication,
            anchor_view_m,
            body_to_view,
            mode: 10,
            sun_body,
        }
    }

    fn regional(
        draw: TileDraw,
        boundary: crate::regional_edges::TileBoundary,
        version: u64,
    ) -> RegionalResidentDraw {
        RegionalResidentDraw {
            capacity: 1,
            cells: 2,
            uploads: vec![RegionalTileUpload {
                slot: 0,
                tile: draw.clone(),
            }],
            patches: vec![RegionalPatchDraw {
                own_slot: 0,
                parent_slot: 0,
                own: draw.clone(),
                parent: draw,
                morph_fraction: 1.0,
                boundary_fraction: 1.0,
                quadrant: None,
                quality_fallback: false,
                boundary_endpoints: Arc::new(RegionalBoundaryEndpoints {
                    version,
                    own_coarse: boundary.clone(),
                    own_fine: boundary.clone(),
                    parent: boundary,
                }),
            }],
        }
    }

    #[test]
    #[ignore = "requires a real GPU adapter; exercises unsubmitted regional upload pressure"]
    fn blocked_replacement_keeps_and_rebases_previous_cover() {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                })
                .await
                .expect("GPU adapter required for regional pressure diagnostic");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("Regional pressure diagnostic"),
                    required_features: crate::gpu_profile::available_features(&adapter),
                    required_limits: wgpu::Limits::default(),
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    trace: wgpu::Trace::Off,
                })
                .await
                .unwrap();
            let projection_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("Regional pressure projection layout"),
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
            let mut renderer = ResidentTileRenderer::new(
                &device,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                &projection_layout,
            );
            let address_a = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 256, 256).unwrap();
            let address_b = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 257, 256).unwrap();
            let tile_a = tile(address_a, 3);
            let tile_b = tile(address_b, 4);
            let boundary_a = build_boundaries(&BTreeMap::from([(address_a, Arc::clone(&tile_a))]))
                .unwrap()
                .remove(&address_a)
                .unwrap();
            let boundary_b = build_boundaries(&BTreeMap::from([(address_b, Arc::clone(&tile_b))]))
                .unwrap()
                .remove(&address_b)
                .unwrap();
            let mut slot_state = TileSlotState::default();
            let first_draw = draw(
                tile_a.clone(),
                &mut slot_state,
                DVec3::ZERO,
                DMat3::IDENTITY,
                DVec3::Z,
            );
            let first = regional(first_draw, boundary_a, 1);
            renderer.prepare_regional(&device, &queue, &first).unwrap();
            assert_eq!(renderer.regional_active_patches, [0]);

            // No command buffer has been submitted; slot zero still belongs to
            // the prepared cover, so the replacement must be deferred.
            let current_rotation = DMat3::from_rotation_y(0.23);
            let current_anchor_view = DVec3::new(24.0, -3.0, 7.0);
            let replacement_draw = draw(
                tile_b.clone(),
                &mut slot_state,
                current_anchor_view,
                current_rotation,
                DVec3::Y,
            );
            let replacement = regional(replacement_draw.clone(), boundary_b, 2);
            renderer
                .prepare_regional(&device, &queue, &replacement)
                .unwrap();

            assert!(renderer.regional_report.fallback_active);
            assert_eq!(renderer.regional_report.deferred_upload_count, 1);
            assert_eq!(renderer.regional_active_patches, [0]);
            assert_eq!(
                renderer.tile_slots[REGIONAL_SLOT_BASE]
                    .resident_key
                    .as_ref(),
                Some(&tile_a.key)
            );
            let retained = &renderer.regional_draw.as_ref().unwrap().patches[0];
            let previous_anchor = tile_a.anchor_position_body().unwrap();
            let requested_anchor = tile_b.anchor_position_body().unwrap();
            let expected_anchor =
                current_anchor_view + current_rotation * (previous_anchor - requested_anchor);
            assert!((retained.own.anchor_view_m - expected_anchor).length() < 1.0e-8);
            assert_eq!(retained.own.body_to_view, current_rotation);
            assert_eq!(retained.parent.body_to_view, current_rotation);
            assert_eq!(retained.own.sun_body, DVec3::Y);
            assert_eq!(retained.own.mode, 10);
            assert_eq!(renderer.regional_report.metadata_upload_bytes, PARAM_BYTES);
        });
    }
}
