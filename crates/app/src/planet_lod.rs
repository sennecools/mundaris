//! Planet terrain over the GPU height/normal atlas (ADR 0016).
//!
//! Each frame and terrain body: CDLOD-select the desired node cut around the
//! observer, draw every selected node from its own atlas data or from its
//! finest resident ancestor, and request missing nodes coarse-first within a
//! per-frame producer budget. There is no publication gate: a node becomes
//! drawable in the frame its producer dispatch is submitted, fading in from
//! ancestor data. Residency keeps every resident node's ancestors and the
//! base levels; eviction is LRU over unused leaf-resident nodes.
//!
//! World surface definitions stay the authority. Atlas layers, bounds and
//! node tables are disposable derived state keyed by body, definition, radius
//! and terrain revision; body motion only changes per-frame transforms.
mod producer;
pub mod select;

use anyhow::{Context, Result, ensure};
use glam::{DMat3, DVec3};
use mundaris_math::{Direction3, FrameId, surface::CubePatchAddress};
use mundaris_renderer::{
    AtlasBounds, AtlasInstance, AtlasProduceJob, AtlasSampleSource, AtlasSource,
    CelestialProjection, MAX_ATLAS_JOBS_PER_FRAME, PreparedView, TerrainAtlasConfig,
    TerrainAtlasFrame,
};
use mundaris_world::{
    BodyId,
    terrain::{SurfaceDefinition, SurfaceGenerator, producer::ProducerRecipe},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};

/// Authored, validated atlas LOD policy (content `lod` block).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LodPolicy {
    pub schema: u32,
    /// Data cells per atlas tile edge.
    pub tile_cells: u32,
    /// Grid cells per drawn CDLOD node edge. Nodes sample data tiles
    /// `log2(tile_cells / draw_cells)` levels above them.
    pub draw_cells: u32,
    pub atlas_layers: u32,
    /// Normal-map texels per geometry cell.
    pub normal_scale: u32,
    /// Target projected size of one drawn grid cell, pixels.
    pub pixel_error: f64,
    /// Lower bound of a level's range as a multiple of its node size; keeps
    /// CDLOD morph regions inside parent-level neighbours.
    pub min_range_factor: f64,
    /// Fraction of a level's distance band used for morphing to the parent grid.
    pub morph_fraction: f64,
    /// Levels `0..=base_resident_level` stay resident for complete fallback.
    pub base_resident_level: u8,
    pub max_level: u8,
    pub jobs_per_frame: u32,
    pub arrival_seconds: f64,
    /// Skirt depth in drawn grid cells (seam backstop).
    pub skirt_cells: f64,
    /// Bodies smaller than this on screen are drawn as plain spheres.
    pub min_body_pixels: f64,
}

impl LodPolicy {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported lod schema");
        ensure!(
            self.tile_cells.is_power_of_two() && (16..=256).contains(&self.tile_cells),
            "lod tile_cells must be a power of two in 16..=256"
        );
        ensure!(
            self.draw_cells.is_power_of_two() && (8..=self.tile_cells).contains(&self.draw_cells),
            "lod draw_cells must be a power of two in 8..=tile_cells"
        );
        ensure!(
            (1..=2).contains(&self.normal_scale),
            "lod normal_scale must be 1 or 2"
        );
        ensure!(
            (64..=2048).contains(&self.atlas_layers),
            "lod atlas_layers must be within 64..=2048"
        );
        ensure!(
            self.pixel_error.is_finite() && (0.25..=64.0).contains(&self.pixel_error),
            "lod pixel_error must be within 0.25..=64"
        );
        ensure!(
            self.min_range_factor.is_finite() && (1.5..=16.0).contains(&self.min_range_factor),
            "lod min_range_factor must be within 1.5..=16"
        );
        ensure!(
            self.morph_fraction.is_finite() && (0.05..=0.9).contains(&self.morph_fraction),
            "lod morph_fraction must be within 0.05..=0.9"
        );
        ensure!(
            self.base_resident_level <= 3,
            "lod base_resident_level must be at most 3"
        );
        ensure!(
            (self.base_resident_level + self.data_level_offset()..=28).contains(&self.max_level),
            "lod max_level (draw levels) must be within base levels plus the data offset..=28"
        );
        ensure!(
            (1..=MAX_ATLAS_JOBS_PER_FRAME as u32).contains(&self.jobs_per_frame),
            "lod jobs_per_frame out of range"
        );
        ensure!(
            self.arrival_seconds.is_finite() && (0.0..=5.0).contains(&self.arrival_seconds),
            "lod arrival_seconds must be within 0..=5"
        );
        ensure!(
            self.skirt_cells.is_finite() && (0.0..=64.0).contains(&self.skirt_cells),
            "lod skirt_cells must be within 0..=64"
        );
        ensure!(
            self.min_body_pixels.is_finite() && (1.0..=4096.0).contains(&self.min_body_pixels),
            "lod min_body_pixels must be within 1..=4096"
        );
        let base_nodes: u32 = (0..=u32::from(self.base_resident_level))
            .map(|level| 6 * 4u32.pow(level))
            .sum();
        ensure!(
            base_nodes * 2 < self.atlas_layers,
            "lod base levels leave too few atlas layers"
        );
        Ok(())
    }

    /// Levels between a drawn node and the data tile it samples.
    pub fn data_level_offset(&self) -> u8 {
        (self.tile_cells / self.draw_cells.max(1))
            .max(1)
            .trailing_zeros() as u8
    }

    pub fn atlas_config(&self, layer_limit: u32) -> TerrainAtlasConfig {
        TerrainAtlasConfig {
            cells: self.tile_cells,
            draw_cells: self.draw_cells,
            layers: self.atlas_layers.min(layer_limit),
            normal_scale: self.normal_scale,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Resident {
    layer: u32,
    produced: Instant,
    last_used: u64,
}

/// Measured 4x4 grid of radial-offset ranges over a data tile chart.
type BoundsGrid = [[f64; 2]; GRID_CELLS];
/// Measured bounds outlive atlas residency so eviction cannot destabilize
/// selection; this caps that CPU cache (about 260 B per entry).
const BOUNDS_CACHE_CAPACITY: usize = 65_536;

const GRID: usize = mundaris_renderer::ATLAS_BOUNDS_GRID;
const GRID_CELLS: usize = GRID * GRID;
/// Refinement may run at most this many levels ahead of measured bounds.
const MEASURED_LOOKAHEAD: u8 = 2;

struct BodyLod {
    slot: u64,
    definition: SurfaceDefinition,
    radius_bits: u64,
    revision: u64,
    recipe: Arc<ProducerRecipe>,
    source: Arc<AtlasSource>,
    source_key: u64,
    height_bound_m: f64,
    nodes: HashMap<CubePatchAddress, Resident>,
    bounds: HashMap<CubePatchAddress, BoundsGrid>,
    bounds_order: VecDeque<CubePatchAddress>,
    bound_at: Instant,
    first_complete_s: Option<f64>,
    last_active: u64,
    stats: BodyStats,
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct BodyStats {
    pub selected: usize,
    pub drawn: usize,
    pub virtual_nodes: usize,
    pub missing_visible: usize,
    pub resident: usize,
    pub finest_level: u8,
    pub visited: usize,
    pub truncated: bool,
    /// Wanted refinements waiting for measured height bounds.
    pub deferred: usize,
    pub jobs_last_frame: usize,
    pub fading: usize,
    /// Distinct data tiles sampled by drawn nodes (own and parent sources).
    pub data_tiles_in_use: usize,
}

/// One body to consider this frame.
pub struct AtlasBodyInput<'a> {
    pub body: BodyId,
    pub body_fixed_frame: FrameId,
    pub definition: &'a SurfaceDefinition,
    pub radius_m: f64,
    pub revision: u64,
    pub sun_body: DVec3,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct FrameSample {
    pub frame: u64,
    pub interval_ms: f64,
    pub host_frame_ms: Option<f64>,
    pub selection_ms: f64,
    pub jobs: usize,
}

pub struct PlanetLod {
    pub enabled: bool,
    pub debug_mode: Option<u32>,
    pub hold: bool,
    policy: Option<LodPolicy>,
    bodies: HashMap<BodyId, BodyLod>,
    pending: HashMap<BodyId, PendingBind>,
    free_layers: Vec<u32>,
    layer_count: u32,
    next_slot: u64,
    next_token: u64,
    tokens: HashMap<u64, (BodyId, u64, CubePatchAddress)>,
    frame: u64,
    jobs_total: u64,
    evictions_total: u64,
    bounds_received: u64,
    stale_bounds: u64,
    selection_ms: f64,
    samples: VecDeque<FrameSample>,
    drawn_bodies: Vec<BodyId>,
    last_error: Option<String>,
}

const SAMPLE_HISTORY: usize = 256;
const VISIT_LIMIT: usize = 60_000;
const INACTIVE_RELEASE_FRAMES: u64 = 600;

impl PlanetLod {
    pub fn new(policy: Option<LodPolicy>) -> Self {
        Self {
            enabled: policy.is_some(),
            // Developer diagnostic: force an atlas debug view (1 height, 2 normals,
            // 5 grid, 6 levels, 8 morph/fade) for every frame.
            debug_mode: std::env::var("MUNDARIS_ATLAS_DEBUG")
                .ok()
                .and_then(|value| value.parse().ok()),
            hold: false,
            policy,
            bodies: HashMap::new(),
            pending: HashMap::new(),
            free_layers: Vec::new(),
            layer_count: 0,
            next_slot: 1,
            next_token: 1,
            tokens: HashMap::new(),
            frame: 0,
            jobs_total: 0,
            evictions_total: 0,
            bounds_received: 0,
            stale_bounds: 0,
            selection_ms: 0.0,
            samples: VecDeque::new(),
            drawn_bodies: Vec::new(),
            last_error: None,
        }
    }

    pub fn policy(&self) -> Option<LodPolicy> {
        self.policy
    }

    /// Bodies whose terrain the atlas drew in the latest frame.
    pub fn drawn_bodies(&self) -> &[BodyId] {
        &self.drawn_bodies
    }

    pub fn receive_bounds(&mut self, bounds: Vec<AtlasBounds>) {
        for result in bounds {
            let Some((body, slot, address)) = self.tokens.remove(&result.token) else {
                self.stale_bounds += 1;
                continue;
            };
            // Bounds stay valid for the bound definition even if the tile was
            // evicted meanwhile; a rebind changes the slot.
            match self.bodies.get_mut(&body) {
                Some(lod) if lod.slot == slot => {
                    let grid = result
                        .cells
                        .map(|[low, high]| [f64::from(low), f64::from(high)]);
                    if lod.bounds.insert(address, grid).is_none() {
                        lod.bounds_order.push_back(address);
                        if lod.bounds_order.len() > BOUNDS_CACHE_CAPACITY
                            && let Some(oldest) = lod.bounds_order.pop_front()
                        {
                            lod.bounds.remove(&oldest);
                        }
                    }
                    self.bounds_received += 1;
                }
                _ => self.stale_bounds += 1,
            }
        }
    }

    fn ensure_layers(&mut self, config: TerrainAtlasConfig) {
        if self.layer_count != config.layers {
            self.bodies.clear();
            self.tokens.clear();
            self.layer_count = config.layers;
            self.free_layers = (0..config.layers).rev().collect();
        }
    }

    /// Bind a body to its current definition. Recipe preparation (profile
    /// pyramid, GPU source copy) runs on a background thread; until it is
    /// ready the body is not drawn by the atlas and returns `false`.
    fn bind(&mut self, input: &AtlasBodyInput<'_>) -> Result<bool> {
        let key = (input.radius_m.to_bits(), input.revision);
        if let Some(existing) = self.bodies.get(&input.body)
            && existing.definition == *input.definition
            && (existing.radius_bits, existing.revision) == key
        {
            return Ok(true);
        }
        let pending_matches = self
            .pending
            .get(&input.body)
            .is_some_and(|pending| pending.definition == *input.definition && pending.key == key);
        if !pending_matches {
            self.release(input.body);
            let (sender, receiver) = std::sync::mpsc::channel();
            let definition = input.definition.clone();
            let radius_m = input.radius_m;
            std::thread::Builder::new()
                .name("atlas-recipe".into())
                .spawn(move || {
                    let _ = sender.send(prepare_recipe(&definition, radius_m));
                })?;
            self.pending.insert(
                input.body,
                PendingBind {
                    definition: input.definition.clone(),
                    key,
                    receiver,
                },
            );
            return Ok(false);
        }
        let pending = self.pending.get(&input.body).expect("checked above");
        let (recipe, source, height_bound_m) = match pending.receiver.try_recv() {
            Ok(prepared) => {
                self.pending.remove(&input.body);
                prepared?
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(false),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending.remove(&input.body);
                anyhow::bail!("atlas recipe preparation stopped unexpectedly");
            }
        };
        let slot = self.next_slot;
        self.next_slot += 1;
        self.bodies.insert(
            input.body,
            BodyLod {
                slot,
                definition: input.definition.clone(),
                radius_bits: input.radius_m.to_bits(),
                revision: input.revision,
                recipe: Arc::new(recipe),
                source: Arc::new(source),
                source_key: slot,
                height_bound_m,
                nodes: HashMap::new(),
                bounds: HashMap::new(),
                bounds_order: VecDeque::new(),
                bound_at: Instant::now(),
                first_complete_s: None,
                last_active: self.frame,
                stats: BodyStats::default(),
            },
        );
        Ok(true)
    }

    fn release(&mut self, body: BodyId) {
        if let Some(lod) = self.bodies.remove(&body) {
            for node in lod.nodes.values() {
                self.free_layers.push(node.layer);
            }
            self.tokens.retain(|_, (owner, _, _)| *owner != body);
        }
    }

    /// Prepare producer jobs and instances for this frame.
    #[allow(clippy::too_many_arguments)] // One coherent frame preparation input.
    pub fn prepare(
        &mut self,
        view: &PreparedView<'_>,
        projection: CelestialProjection,
        bodies: &[AtlasBodyInput<'_>],
        render_mode: u32,
        layer_limit: u32,
    ) -> Result<TerrainAtlasFrame> {
        let started = Instant::now();
        self.frame += 1;
        self.drawn_bodies.clear();
        let Some(policy) = self.policy.filter(|_| self.enabled) else {
            return Ok(TerrainAtlasFrame::default());
        };
        let config = policy.atlas_config(layer_limit);
        self.ensure_layers(config);
        let viewport = projection.viewport();
        let focal = projection.focal_pixels();
        let frustum = select::Frustum::new(focal, [f64::from(viewport[0]), f64::from(viewport[1])]);
        let mode = self.debug_mode.unwrap_or(render_mode);
        let mut frame = TerrainAtlasFrame {
            config: Some(config),
            ..Default::default()
        };
        let now = Instant::now();
        let mut budget = policy.jobs_per_frame as usize;
        for input in bodies {
            let source = view.prepare_source(input.body_fixed_frame)?;
            let observer = source.observer_in_source().metres();
            let pixels = input.radius_m * focal / observer.length().max(1.0);
            if pixels < policy.min_body_pixels {
                continue;
            }
            match self.bind(input) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(error) => {
                    self.last_error = Some(format!("{error:#}"));
                    continue;
                }
            }
            let direction =
                |v| -> Result<DVec3> { Ok(source.view_direction(Direction3::try_new(v)?)?.unit()) };
            let body_to_view = DMat3::from_cols(
                direction(DVec3::X)?,
                direction(DVec3::Y)?,
                direction(DVec3::Z)?,
            );
            let frame_number = self.frame;
            let lod = self.bodies.get_mut(&input.body).expect("bound above");
            lod.last_active = frame_number;
            frame
                .sources
                .push((lod.source_key, Arc::clone(&lod.source)));
            let radius = input.radius_m;
            let data_offset = policy.data_level_offset();
            let ranges: Vec<f64> = (0..=policy.max_level)
                .map(|level| {
                    let cell = mundaris_world::terrain::producer::tile_texel_m(
                        radius,
                        level,
                        policy.draw_cells,
                    );
                    (cell * focal / policy.pixel_error)
                        .max(policy.min_range_factor * cell * f64::from(policy.draw_cells))
                })
                .collect();
            let selection = {
                let measured = &lod.bounds;
                let bound = lod.height_bound_m;
                select::select(
                    &select::SelectionInput {
                        observer_body: observer,
                        body_to_view,
                        frustum,
                        radius_m: radius,
                        occluder_radius_m: radius - lod.height_bound_m,
                        ranges: &ranges,
                        max_level: policy.max_level,
                        visit_limit: VISIT_LIMIT,
                    },
                    |address| {
                        node_bounds(
                            measured,
                            address,
                            bound,
                            policy.base_resident_level,
                            data_offset + MEASURED_LOOKAHEAD,
                        )
                    },
                )
            };

            // Requests: base levels first, then missing selected nodes coarse-first.
            let mut wanted: Vec<(u8, f64, CubePatchAddress)> = Vec::new();
            if !self.hold {
                for level in 0..=policy.base_resident_level {
                    for face in mundaris_math::surface::CubeFace::ALL {
                        let count = 1u32 << level;
                        for y in 0..count {
                            for x in 0..count {
                                let address = CubePatchAddress::try_new(face, level, x, y)?;
                                if !lod.nodes.contains_key(&address) {
                                    wanted.push((level, 0.0, address));
                                }
                            }
                        }
                    }
                }
                let mut seen = HashSet::new();
                let mut missing: Vec<_> = selection
                    .selected
                    .iter()
                    .map(|s| (data_address(s.address, data_offset), s.distance_m))
                    .filter(|(data, _)| !lod.nodes.contains_key(data) && seen.insert(*data))
                    .map(|(data, distance)| (data.level(), distance, data))
                    .collect();
                missing.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
                wanted.extend(missing);
            }
            let mut planned: HashSet<CubePatchAddress> = HashSet::new();
            let mut jobs_this_body = 0usize;
            'requests: for &(_, _, address) in &wanted {
                let nodes = &self.bodies[&input.body].nodes;
                let mut chain = Vec::new();
                let mut cursor = Some(address);
                while let Some(node) = cursor {
                    if nodes.contains_key(&node) || planned.contains(&node) {
                        break;
                    }
                    chain.push(node);
                    cursor = node.parent();
                }
                for node in chain.into_iter().rev() {
                    if budget == 0 {
                        break 'requests;
                    }
                    let Some(layer) = allocate_layer(
                        &mut self.free_layers,
                        &mut self.bodies,
                        frame_number,
                        policy.base_resident_level,
                        &mut self.evictions_total,
                    ) else {
                        break 'requests;
                    };
                    let lod = self.bodies.get_mut(&input.body).expect("bound above");
                    let chart = select::chart(node);
                    let kind = producer::tile_kind(&lod.recipe, node, &chart, policy.tile_cells)?;
                    let token = self.next_token;
                    self.next_token += 1;
                    self.tokens.insert(token, (input.body, lod.slot, node));
                    frame.jobs.push(AtlasProduceJob {
                        source: lod.source_key,
                        layer,
                        token,
                        chart: producer::atlas_chart(&chart),
                        radius_m: radius as f32,
                        kind,
                    });
                    lod.nodes.insert(
                        node,
                        Resident {
                            layer,
                            produced: now,
                            last_used: frame_number,
                        },
                    );
                    planned.insert(node);
                    budget -= 1;
                    jobs_this_body += 1;
                }
            }

            // Instances from own data or the finest resident ancestor.
            let lod = self.bodies.get_mut(&input.body).expect("bound above");
            let mut stats = BodyStats {
                selected: selection.selected.len(),
                visited: selection.visited,
                truncated: selection.truncated,
                deferred: selection.deferred,
                jobs_last_frame: jobs_this_body,
                ..BodyStats::default()
            };
            for selected in &selection.selected {
                let address = selected.address;
                let data = data_address(address, data_offset);
                let Some((owner, owner_node)) = finest_resident(&lod.nodes, data) else {
                    stats.missing_visible += 1;
                    continue;
                };
                let (own_origin, own_scale) = select::rect_within(address, owner);
                let own_is_self = owner == data;
                // The parent draw node's data tile supplies morph and arrival
                // endpoints; it is `data`'s parent unless both clamp to a root.
                let parent_data = address
                    .parent()
                    .map_or(data, |parent| data_address(parent, data_offset));
                let mut parent_owner = owner;
                let (parent_source, arrival) = if own_is_self && parent_data != data {
                    match finest_resident(&lod.nodes, parent_data) {
                        Some((parent, node)) => {
                            parent_owner = parent;
                            let (origin, scale) = select::rect_within(address, parent);
                            let arrival = if policy.arrival_seconds > 0.0 {
                                (now.duration_since(owner_node.produced).as_secs_f64()
                                    / policy.arrival_seconds)
                                    .clamp(0.0, 1.0)
                            } else {
                                1.0
                            };
                            (sample(node.layer, origin, scale), arrival)
                        }
                        None => (sample(owner_node.layer, own_origin, own_scale), 1.0),
                    }
                } else {
                    if !own_is_self {
                        // Ancestor data stands in for both endpoints; only the
                        // grid morph applies until the node's own data arrives.
                        stats.virtual_nodes += 1;
                        stats.missing_visible += 1;
                    }
                    (sample(owner_node.layer, own_origin, own_scale), 1.0)
                };
                if arrival < 1.0 {
                    stats.fading += 1;
                }
                let level = address.level();
                let chart = select::chart(address);
                let anchor_body = chart.n0 * radius;
                let anchor_view = body_to_view * (anchor_body - observer);
                let (morph_start, morph_end) = if level == 0 {
                    (f32::MAX, f32::MAX)
                } else {
                    let end = ranges[level as usize - 1];
                    let start = end - policy.morph_fraction * (end - ranges[level as usize]);
                    (start as f32, end as f32)
                };
                let cell = mundaris_world::terrain::producer::tile_texel_m(
                    radius,
                    level,
                    policy.draw_cells,
                );
                frame.instances.push(AtlasInstance {
                    anchor_view_m: anchor_view.as_vec3().to_array(),
                    radius_m: radius as f32,
                    body_to_view: [
                        body_to_view.x_axis.as_vec3().to_array(),
                        body_to_view.y_axis.as_vec3().to_array(),
                        body_to_view.z_axis.as_vec3().to_array(),
                    ],
                    chart: producer::atlas_chart(&chart),
                    level,
                    own: sample(owner_node.layer, own_origin, own_scale),
                    parent: parent_source,
                    morph_start_m: morph_start,
                    morph_end_m: morph_end,
                    arrival: arrival as f32,
                    skirt_m: (policy.skirt_cells * cell) as f32,
                    sun_body: input.sun_body.as_vec3().to_array(),
                    mode,
                });
                stats.drawn += 1;
                stats.finest_level = stats.finest_level.max(level);
                // Mark data sources as used for LRU.
                if let Some(node) = lod.nodes.get_mut(&owner) {
                    node.last_used = frame_number;
                }
                if let Some(node) = lod.nodes.get_mut(&parent_owner) {
                    node.last_used = frame_number;
                }
            }
            stats.resident = lod.nodes.len();
            stats.data_tiles_in_use = lod
                .nodes
                .values()
                .filter(|node| node.last_used == frame_number)
                .count();
            if lod.first_complete_s.is_none()
                && stats.missing_visible == 0
                && stats.fading == 0
                && jobs_this_body == 0
                && selection.deferred == 0
                && !selection.truncated
                && stats.drawn > 0
            {
                lod.first_complete_s = Some(lod.bound_at.elapsed().as_secs_f64());
            }
            lod.stats = stats;
            if stats.drawn > 0 {
                self.drawn_bodies.push(input.body);
            }
        }
        // Release bodies that have not been active for a while.
        let frame_number = self.frame;
        let inactive: Vec<BodyId> = self
            .bodies
            .iter()
            .filter(|(_, lod)| frame_number - lod.last_active > INACTIVE_RELEASE_FRAMES)
            .map(|(body, _)| *body)
            .collect();
        for body in inactive {
            self.release(body);
        }
        if self.tokens.len() > 100_000 {
            // Readbacks dropped under pressure never return; forget old tokens.
            let floor = self.next_token.saturating_sub(50_000);
            self.tokens.retain(|token, _| *token >= floor);
        }
        self.jobs_total += frame.jobs.len() as u64;
        self.selection_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(frame)
    }

    /// Fill the shared terrain summary used by the developer UI and snapshots.
    pub fn annotate_terrain(
        &self,
        terrain: &mut crate::developer_snapshot::TerrainSnapshot,
        ids: &[BodyId],
        system: &mundaris_world::CelestialSystem,
    ) {
        terrain.backend = "ATLAS CDLOD".into();
        terrain.refinement_demand_kind = Some("cdlod_pixel_error".into());
        terrain.certificate_kind = Some("gpu_band_limited_derived".into());
        terrain.target_certifiable = Some(false);
        let Some(body) = self.drawn_bodies.first().copied() else {
            terrain.active_body = None;
            terrain.ready = false;
            return;
        };
        let Some(lod) = self.bodies.get(&body) else {
            return;
        };
        terrain.active_body = ids.iter().position(|id| *id == body).and_then(|index| {
            system
                .body(body)
                .ok()
                .map(|state| crate::developer_snapshot::BodySnapshot {
                    index,
                    name: state.name().into(),
                })
        });
        terrain.generator_algorithm = Some(format!("{:?}", lod.definition.terrain().algorithm()));
        let stats = lod.stats;
        let complete = stats.missing_visible == 0
            && stats.fading == 0
            && stats.jobs_last_frame == 0
            && stats.deferred == 0;
        terrain.ready = stats.drawn > 0;
        terrain.quality_pending = Some(!complete);
        terrain.settled = Some(complete);
        terrain.construction_pending = stats.jobs_last_frame > 0;
        terrain.source_leaf_count = stats.resident;
        terrain.visible_leaf_count = stats.drawn;
        terrain.desired_radial_lod = Some(stats.finest_level);
        terrain.ready_radial_lod = Some(stats.finest_level);
        terrain.source_radial_lod = Some(stats.finest_level);
        terrain.active_morph = stats.fading > 0;
    }

    pub fn record_frame(&mut self, interval_ms: f64, host_frame_ms: Option<f64>, jobs: usize) {
        if self.samples.len() == SAMPLE_HISTORY {
            self.samples.pop_front();
        }
        self.samples.push_back(FrameSample {
            frame: self.frame,
            interval_ms,
            host_frame_ms,
            selection_ms: self.selection_ms,
            jobs,
        });
    }

    /// Developer telemetry for the shared runner and UI.
    pub fn snapshot(&self, names: impl Fn(BodyId) -> String) -> serde_json::Value {
        let bodies: Vec<serde_json::Value> = self
            .bodies
            .iter()
            .map(|(body, lod)| {
                serde_json::json!({
                    "body": names(*body),
                    "slot": lod.slot,
                    "stats": lod.stats,
                    "first_complete_s": lod.first_complete_s,
                    "bound_age_s": lod.bound_at.elapsed().as_secs_f64(),
                    "complete": lod.stats.missing_visible == 0 && lod.stats.fading == 0
                        && lod.stats.jobs_last_frame == 0 && lod.stats.deferred == 0,
                })
            })
            .collect();
        serde_json::json!({
            "schema": 1,
            "enabled": self.enabled,
            "policy": self.policy,
            "frame": self.frame,
            "layers": self.layer_count,
            "free_layers": self.free_layers.len(),
            "jobs_total": self.jobs_total,
            "evictions_total": self.evictions_total,
            "bounds_received": self.bounds_received,
            "stale_bounds": self.stale_bounds,
            "selection_ms": self.selection_ms,
            "bodies": bodies,
            "frame_samples": self.samples,
            "last_error": self.last_error,
        })
    }
}

/// Data tile that a draw node samples: its ancestor `offset` levels up, or a root.
fn data_address(address: CubePatchAddress, offset: u8) -> CubePatchAddress {
    let mut data = address;
    for _ in 0..offset.min(address.level()) {
        data = data.parent().expect("level above zero");
    }
    data
}

struct PendingBind {
    definition: SurfaceDefinition,
    key: (u64, u64),
    receiver: std::sync::mpsc::Receiver<Result<(ProducerRecipe, AtlasSource, f64)>>,
}

fn prepare_recipe(
    definition: &SurfaceDefinition,
    radius_m: f64,
) -> Result<(ProducerRecipe, AtlasSource, f64)> {
    let generator = SurfaceGenerator::new(definition, radius_m)?;
    let recipe = generator
        .producer_recipe()
        .context("body terrain is not supported by the GPU atlas producer")?;
    let source = producer::atlas_source(&recipe)?;
    let bound =
        producer::recipe_height_bound(&recipe, generator.conservative_absolute_height_bound_m());
    Ok((recipe, source, bound))
}

fn sample(layer: u32, origin: [f64; 2], scale: f64) -> AtlasSampleSource {
    AtlasSampleSource {
        layer,
        origin: [origin[0] as f32, origin[1] as f32],
        scale: scale as f32,
    }
}

fn finest_resident(
    nodes: &HashMap<CubePatchAddress, Resident>,
    address: CubePatchAddress,
) -> Option<(CubePatchAddress, Resident)> {
    let mut cursor = Some(address);
    while let Some(node) = cursor {
        if let Some(resident) = nodes.get(&node) {
            return Some((node, *resident));
        }
        cursor = node.parent();
    }
    None
}

/// Produced bounds of the node or an expanded estimate from its nearest
/// measured ancestor; the recipe's absolute bound before anything is known.
fn node_bounds(
    measured: &HashMap<CubePatchAddress, BoundsGrid>,
    address: CubePatchAddress,
    absolute_bound_m: f64,
    base_level: u8,
    lookahead: u8,
) -> ([f64; 2], bool) {
    let mut cursor = Some(address);
    while let Some(node) = cursor {
        if let Some(grid) = measured.get(&node) {
            // Union of the measured ancestor cells covering this node.
            let (origin, scale) = select::rect_within(address, node);
            let cell = |value: f64| ((value * GRID as f64).floor() as usize).min(GRID - 1);
            let (x0, y0) = (cell(origin[0]), cell(origin[1]));
            let x1 = cell(origin[0] + scale - 1.0e-9);
            let y1 = cell(origin[1] + scale - 1.0e-9);
            let mut range = [f64::INFINITY, f64::NEG_INFINITY];
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let [low, high] = grid[y * GRID + x];
                    range = [range[0].min(low), range[1].max(high)];
                }
            }
            let depth = address.level() - node.level();
            if depth == 0 {
                return (range, true);
            }
            // Finer bands can add relief the ancestor filtered away.
            let margin = 0.15 * f64::from(depth) * (range[1] - range[0]) + 0.5;
            return (
                [
                    (range[0] - margin).max(-absolute_bound_m),
                    (range[1] + margin).min(absolute_bound_m),
                ],
                depth <= lookahead,
            );
        }
        cursor = node.parent();
    }
    (
        [-absolute_bound_m, absolute_bound_m],
        address.level() < base_level,
    )
}

#[allow(clippy::too_many_arguments)]
fn allocate_layer(
    free: &mut Vec<u32>,
    bodies: &mut HashMap<BodyId, BodyLod>,
    frame: u64,
    base_level: u8,
    evictions: &mut u64,
) -> Option<u32> {
    if let Some(layer) = free.pop() {
        return Some(layer);
    }
    // LRU over leaf-resident, unused, non-base nodes of any body.
    let mut victim: Option<(BodyId, CubePatchAddress, u64)> = None;
    for (body, lod) in bodies.iter() {
        for (address, node) in &lod.nodes {
            if address.level() <= base_level || node.last_used >= frame {
                continue;
            }
            if victim.is_none_or(|(_, _, used)| node.last_used < used) {
                victim = Some((*body, *address, node.last_used));
            }
        }
    }
    let (body, address, _) = victim?;
    let lod = bodies.get_mut(&body)?;
    let node = lod.nodes.remove(&address)?;
    *evictions += 1;
    Some(node.layer)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn policy() -> LodPolicy {
        LodPolicy {
            schema: 1,
            tile_cells: 128,
            draw_cells: 32,
            atlas_layers: 512,
            normal_scale: 2,
            pixel_error: 2.0,
            min_range_factor: 3.0,
            morph_fraction: 0.3,
            base_resident_level: 1,
            max_level: 22,
            jobs_per_frame: 24,
            arrival_seconds: 0.25,
            skirt_cells: 2.0,
            min_body_pixels: 6.0,
        }
    }

    #[test]
    fn default_policy_validates_and_bad_values_are_rejected() {
        policy().validate().unwrap();
        assert!(
            LodPolicy {
                tile_cells: 100,
                ..policy()
            }
            .validate()
            .is_err()
        );
        assert!(
            LodPolicy {
                pixel_error: 0.0,
                ..policy()
            }
            .validate()
            .is_err()
        );
        assert!(
            LodPolicy {
                atlas_layers: 64,
                base_resident_level: 3,
                ..policy()
            }
            .validate()
            .is_err()
        );
    }

    fn resident(layer: u32, used: u64) -> Resident {
        Resident {
            layer,
            produced: Instant::now(),
            last_used: used,
        }
    }

    #[test]
    fn eviction_takes_oldest_unused_nodes_but_never_base_levels_or_current_nodes() {
        use mundaris_math::surface::CubeFace;
        let root = CubePatchAddress::root(CubeFace::PositiveX);
        let a = root.children().unwrap()[0];
        let b = a.children().unwrap()[1];
        let c = b.children().unwrap()[2];
        let d = b.children().unwrap()[3];
        let mut nodes = HashMap::new();
        nodes.insert(root, resident(0, 1));
        nodes.insert(a, resident(1, 1));
        nodes.insert(b, resident(2, 1)); // oldest legal victim
        nodes.insert(c, resident(3, 5)); // used this frame
        nodes.insert(d, resident(4, 2)); // next legal victim
        let body = test_body_id();
        let mut bodies = HashMap::new();
        bodies.insert(body, test_body(nodes));
        let mut evictions = 0;
        let mut free = Vec::new();
        let layer = allocate_layer(&mut free, &mut bodies, 5, 1, &mut evictions);
        assert_eq!(layer, Some(2));
        let layer = allocate_layer(&mut free, &mut bodies, 5, 1, &mut evictions);
        assert_eq!(layer, Some(4));
        let layer = allocate_layer(&mut free, &mut bodies, 5, 1, &mut evictions);
        assert_eq!(layer, None, "no further legal victims");
        let nodes = &bodies[&body].nodes;
        assert_eq!(nodes.len(), 3);
        assert!(nodes.contains_key(&c) && nodes.contains_key(&a) && nodes.contains_key(&root));
    }

    fn test_body_id() -> BodyId {
        use mundaris_math::*;
        use mundaris_world::*;
        let mut system = CelestialSystem::new(
            std::num::NonZeroU64::new(1).unwrap(),
            SimulationInstant::ZERO,
        );
        system
            .insert_body(
                "test",
                BodyProperties::new(1.0, 1.0).unwrap(),
                BodyState::new(
                    LocalPosition::origin(),
                    LinearVelocity3::zero(),
                    UnitRotation::identity(),
                    AngularVelocity3::zero(),
                ),
            )
            .unwrap()
    }

    fn test_body(nodes: HashMap<CubePatchAddress, Resident>) -> BodyLod {
        let definition = SurfaceDefinition::generated(
            mundaris_world::terrain::TerrainIdentity(1),
            mundaris_world::terrain::TerrainSeed(2),
            mundaris_world::terrain::SurfaceAlgorithm::MoonFieldsV1,
        );
        let generator = SurfaceGenerator::new(&definition, 140_000.0).unwrap();
        let recipe = generator.producer_recipe().unwrap();
        BodyLod {
            slot: 1,
            source: Arc::new(producer::atlas_source(&recipe).unwrap()),
            recipe: Arc::new(recipe),
            definition,
            radius_bits: 140_000f64.to_bits(),
            revision: 0,
            source_key: 1,
            height_bound_m: 100.0,
            nodes,
            bounds: HashMap::new(),
            bounds_order: VecDeque::new(),
            bound_at: Instant::now(),
            first_complete_s: None,
            last_active: 0,
            stats: BodyStats::default(),
        }
    }

    #[test]
    #[ignore = "requires a real GPU adapter; compares GPU atlas tiles with the CPU band-limited reference"]
    fn gpu_tiles_match_the_cpu_band_limited_reference() {
        use mundaris_math::surface::CubeFace;
        let loaded = crate::shared_system::SharedTestSystem::load_canonical(
            std::num::NonZeroU64::new(5).unwrap(),
        )
        .unwrap();
        let policy = loaded.lod;
        let camera = DVec3::from_array(loaded.camera.position_body_m).normalize();
        let (device, queue) = pollster::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                    apply_limit_buckets: false,
                })
                .await
                .expect("GPU adapter");
            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("Atlas producer validation"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    experimental_features: wgpu::ExperimentalFeatures::disabled(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    trace: wgpu::Trace::Off,
                })
                .await
                .expect("device")
        });
        let config = TerrainAtlasConfig {
            cells: policy.tile_cells,
            draw_cells: policy.draw_cells,
            layers: 32,
            normal_scale: policy.normal_scale,
        };
        // Nodes: a root, a face corner, and the canonical camera's path at
        // middle and deep levels.
        let path_node = |level: u8| {
            let (face, uv) = mundaris_math::surface::SurfaceLocation::new(
                mundaris_math::Direction3::try_new(camera).unwrap(),
            )
            .face_uv();
            let count = 1u32 << level;
            let index = |c: f64| (((c + 1.0) * 0.5 * f64::from(count)) as u32).min(count - 1);
            CubePatchAddress::try_new(face, level, index(uv[0]), index(uv[1])).unwrap()
        };
        let nodes = [
            CubePatchAddress::root(CubeFace::PositiveY),
            CubePatchAddress::try_new(CubeFace::NegativeX, 4, 15, 0).unwrap(),
            path_node(8),
            path_node(14),
            path_node(18),
        ];
        for (_, body) in loaded.system.bodies().filter(|(_, b)| b.has_surface()) {
            let definition = body.surface_definition().unwrap();
            let radius = body.properties().reference_radius_m();
            let generator = SurfaceGenerator::new(definition, radius).unwrap();
            let recipe = generator.producer_recipe().unwrap();
            let source = Arc::new(producer::atlas_source(&recipe).unwrap());
            let jobs: Vec<_> = nodes
                .iter()
                .enumerate()
                .map(|(index, &node)| {
                    let chart = select::chart(node);
                    AtlasProduceJob {
                        source: 1,
                        layer: index as u32,
                        token: index as u64 + 1,
                        chart: producer::atlas_chart(&chart),
                        radius_m: radius as f32,
                        kind: producer::tile_kind(&recipe, node, &chart, config.cells).unwrap(),
                    }
                })
                .collect();
            let produced = mundaris_renderer::produce_for_validation(
                &device,
                &queue,
                config,
                &[(1, source)],
                &jobs,
            )
            .unwrap();
            for (node, tile) in nodes.iter().zip(&produced) {
                let chart = select::chart(*node);
                let texel = mundaris_world::terrain::producer::tile_texel_m(
                    radius,
                    node.level(),
                    config.cells,
                );
                let side = config.height_side() as usize;
                let (mut max_height_error, mut max_normal_deg) = (0.0f64, 0.0f64);
                let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
                for j in (0..side).step_by(5) {
                    for i in (0..side).step_by(5) {
                        let st = [
                            (i as f64 - 1.0) / f64::from(config.cells),
                            (j as f64 - 1.0) / f64::from(config.cells),
                        ];
                        let reference = recipe.evaluate(chart.direction(st), texel).unwrap();
                        let gpu = f64::from(tile.heights[j * side + i]);
                        max_height_error = max_height_error.max((gpu - reference.height_m).abs());
                        low = low.min(gpu);
                        high = high.max(gpu);
                        if config.normal_scale == 1 {
                            let n = tile.normals[j * side + i];
                            let n = DVec3::new(n[0].into(), n[1].into(), n[2].into()).normalize();
                            max_normal_deg = max_normal_deg
                                .max(n.dot(reference.normal).clamp(-1.0, 1.0).acos().to_degrees());
                        }
                    }
                }
                let nside = config.normal_side() as usize;
                let ncells = f64::from(config.cells * config.normal_scale);
                for j in (0..nside).step_by(7) {
                    for i in (0..nside).step_by(7) {
                        let st = [(i as f64 - 1.0) / ncells, (j as f64 - 1.0) / ncells];
                        let reference = recipe.evaluate(chart.direction(st), texel).unwrap();
                        let n = tile.normals[j * nside + i];
                        let n = DVec3::new(n[0].into(), n[1].into(), n[2].into()).normalize();
                        max_normal_deg = max_normal_deg
                            .max(n.dot(reference.normal).clamp(-1.0, 1.0).acos().to_degrees());
                    }
                }
                println!(
                    "{} {:?}: max |dh| {max_height_error:.6} m, max normal {max_normal_deg:.3} deg, \
                     sampled range [{low:.3}, {high:.3}], gpu bounds {:?}",
                    body.name(),
                    node,
                    tile.bounds
                );
                assert!(max_height_error < 0.02, "height error {max_height_error}");
                assert!(max_normal_deg < 1.5, "normal error {max_normal_deg}");
                let (min_m, max_m) = tile.bounds.expect("bounds readback");
                assert!(f64::from(min_m) <= low + 1e-3 && f64::from(max_m) >= high - 1e-3);
            }
        }
    }

    #[test]
    fn missing_nodes_draw_from_the_finest_resident_ancestor() {
        use mundaris_math::surface::CubeFace;
        let root = CubePatchAddress::root(CubeFace::NegativeZ);
        let child = root.children().unwrap()[2];
        let grandchild = child.children().unwrap()[1];
        let mut nodes = HashMap::new();
        nodes.insert(root, resident(7, 0));
        nodes.insert(child, resident(8, 0));
        let (owner, node) = finest_resident(&nodes, grandchild).unwrap();
        assert_eq!((owner, node.layer), (child, 8));
        assert_eq!(select::rect_within(grandchild, child), ([0.5, 0.0], 0.5));
        // Measured ancestor bounds widen for unmeasured descendants.
        let mut grid = [[0.0, 1.0]; GRID_CELLS];
        grid[2] = [-10.0, 30.0]; // row 0, column 2: covers the grandchild quadrant
        let mut measured = HashMap::new();
        measured.insert(child, grid);
        let (estimate, refinable) = node_bounds(&measured, grandchild, 500.0, 1, 2);
        assert!(refinable);
        assert!(estimate[0] < -10.0 && estimate[1] > 30.0, "{estimate:?}");
        assert_eq!(
            node_bounds(&measured, child, 500.0, 1, 2),
            ([-10.0, 30.0], true)
        );
        let deep = grandchild.children().unwrap()[0].children().unwrap()[0];
        assert!(!node_bounds(&measured, deep, 500.0, 1, 2).1);
        assert_eq!(
            node_bounds(&HashMap::new(), grandchild, 500.0, 1, 2),
            ([-500.0, 500.0], false)
        );
    }
}
