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
pub mod collision;
pub mod hydrology;
pub mod producer;
pub mod select;
pub mod tier_a;

use anyhow::{Context, Result, ensure};
use astrum_math::{Direction3, FrameId, surface::CubePatchAddress};
use astrum_renderer::{
    AtlasBounds, AtlasInstance, AtlasProduceJob, AtlasSampleSource, AtlasSource,
    CelestialProjection, MAX_ATLAS_JOBS_PER_FRAME, PreparedView, ShadowView, SurfaceMaterial,
    TerrainAtlasConfig, TerrainAtlasFrame,
};
use astrum_world::{
    BodyId,
    terrain::{SurfaceDefinition, SurfaceGenerator, producer::ProducerRecipe},
};
use glam::{DMat3, DVec3};
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

const GRID: usize = astrum_renderer::ATLAS_BOUNDS_GRID;
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
    /// Read-back colliders; `None` without a collision policy.
    collision: Option<collision::BodyCollision>,
    /// False while a world-map source's Tier A bake is still running.
    world_ready: bool,
    /// Seconds from binding to the finished Tier A bake (world maps).
    world_ready_s: Option<f64>,
    /// Upload or bake failure of a world-map source.
    world_error: Option<String>,
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
    /// Sun shadow caster instances drawn from resident data.
    pub shadow_casters: usize,
    /// Splits forced by the restricted-quadtree constraint (§9.1).
    pub balance_splits: usize,
    /// The balance pass hit its cap this frame.
    pub balance_unresolved: bool,
    /// Drawn nodes with at least one edge snapped to a coarser neighbour.
    pub snapped_nodes: usize,
    /// Resident read-back collider pages and pages awaiting readback.
    pub collision_pages: usize,
    pub collision_in_flight: usize,
}

/// One body to consider this frame.
pub struct AtlasBodyInput<'a> {
    pub body: BodyId,
    pub body_fixed_frame: FrameId,
    pub definition: &'a SurfaceDefinition,
    pub radius_m: f64,
    pub revision: u64,
    pub material: SurfaceMaterial,
}

/// Sun shadow inputs of one frame: render settings and the sun position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowCasterPolicy {
    pub settings: astrum_renderer::ShadowSettings,
    /// Sun centre in view metres (the effective light, studio or star).
    pub sun_centre_view_m: DVec3,
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
    /// Source keys of released bodies, sent once so the renderer frees their
    /// GPU sources (and stops their bakes) at once.
    retired_sources: Vec<u64>,
    pub enabled: bool,
    pub hold: bool,
    policy: Option<LodPolicy>,
    bodies: HashMap<BodyId, BodyLod>,
    /// Replaced definitions still drawn until their rebound body is complete.
    outgoing: HashMap<BodyId, BodyLod>,
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
    collision_policy: Option<collision::CollisionPolicy>,
    /// Collision job token -> (body, bind slot, page, request frame).
    collision_tokens: HashMap<u64, (BodyId, u64, CubePatchAddress, u64)>,
    collision_stats: collision::CollisionStats,
    /// PROTOTYPE (M3 Water): hydrology workers for read-back world bakes.
    hydrology: hydrology::HydrologyJobs,
}

const SAMPLE_HISTORY: usize = 256;
const VISIT_LIMIT: usize = 60_000;
/// Below the renderer's 600-frame source eviction, so an idle body is
/// released (and its source retired) before the renderer would drop it.
const INACTIVE_RELEASE_FRAMES: u64 = 500;
/// Longest wait, after its world map is baked, for a replacement to fill
/// the view before it takes over from the definition it replaces.
const REPLACEMENT_WAIT_S: f64 = 1.5;

/// Per-frame inputs shared by every body.
#[derive(Clone, Copy)]
struct FrameContext {
    policy: LodPolicy,
    frustum: select::Frustum,
    focal: f64,
    mode: u32,
    shadows: Option<ShadowCasterPolicy>,
    shadow_body: Option<BodyId>,
    projection: CelestialProjection,
    now: Instant,
}

/// Where one body sits relative to the view this frame.
#[derive(Clone, Copy)]
struct BodyPlace {
    observer: DVec3,
    body_to_view: DMat3,
}

impl PlanetLod {
    pub fn new(policy: Option<LodPolicy>) -> Self {
        Self {
            enabled: policy.is_some(),
            retired_sources: Vec::new(),
            hold: false,
            policy,
            bodies: HashMap::new(),
            outgoing: HashMap::new(),
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
            collision_policy: None,
            collision_tokens: HashMap::new(),
            collision_stats: collision::CollisionStats::default(),
            hydrology: hydrology::HydrologyJobs::default(),
        }
    }

    /// Enable read-back colliders (pipeline §15.1) under a validated policy.
    pub fn with_collision(mut self, policy: collision::CollisionPolicy) -> Self {
        self.collision_policy = Some(policy);
        self
    }

    /// Snapshot of every bound body's colliders for `SurfaceQuery` callers.
    pub fn collider_view(&self) -> collision::ColliderView {
        // A replaced definition keeps serving collision until it is retired.
        let bodies = self
            .bodies
            .iter()
            .filter(|(body, _)| !self.outgoing.contains_key(body))
            .chain(self.outgoing.iter());
        collision::ColliderView::from_bodies(bodies.filter_map(|(body, lod)| {
            lod.collision
                .as_ref()
                .map(|collision| (*body, Arc::clone(&collision.colliders)))
        }))
    }

    /// Schedule the collision pages under query misses (body-fixed directions).
    pub fn request_collision(&mut self, misses: Vec<(BodyId, DVec3)>) {
        self.collision_stats.misses_last_frame = misses.len();
        for (body, direction) in misses {
            if let Some(collision) = self
                .bodies
                .get_mut(&body)
                .and_then(|lod| lod.collision.as_mut())
            {
                collision.want_urgent(direction, self.frame);
            }
        }
    }

    /// Store read-back collision pages; pages of rebound or released bodies
    /// are dropped as stale.
    pub fn receive_collision(&mut self, pages: Vec<astrum_renderer::AtlasCollisionPage>) {
        let Some(policy) = self.collision_policy else {
            return;
        };
        for page in pages {
            let Some((body, slot, address, requested)) = self.collision_tokens.remove(&page.token)
            else {
                self.collision_stats.stale_total += 1;
                continue;
            };
            let Some(collision) = self
                .bodies
                .get_mut(&body)
                .filter(|lod| lod.slot == slot)
                .and_then(|lod| lod.collision.as_mut())
            else {
                self.collision_stats.stale_total += 1;
                continue;
            };
            let Some(collider) =
                collision::ColliderPage::new(page.cells, page.heights, page.normals)
            else {
                self.collision_stats.stale_total += 1;
                continue;
            };
            collision.receive(address, collider, self.frame, policy.max_pages);
            let latency = self.frame.saturating_sub(requested);
            self.collision_stats.received_total += 1;
            self.collision_stats.last_latency_frames = latency;
            self.collision_stats.max_latency_frames =
                self.collision_stats.max_latency_frames.max(latency);
        }
    }

    pub fn policy(&self) -> Option<LodPolicy> {
        self.policy
    }

    /// Bodies whose terrain the atlas drew in the latest frame.
    pub fn drawn_bodies(&self) -> &[BodyId] {
        &self.drawn_bodies
    }

    /// Mark world-map sources whose Tier A bake has completed (renderer keys).
    /// Wall time from binding a world-map body to its finished Tier A bake;
    /// `None` while it runs or for other bodies.
    pub fn world_bake_seconds(&self, body: BodyId) -> Option<f64> {
        self.bodies.get(&body)?.world_ready_s
    }

    /// Finished Tier A bakes, and sources the renderer could not upload (the
    /// body then stays a plain sphere and reports the error).
    pub fn receive_ready_sources(&mut self, events: Vec<(u64, Option<String>)>) {
        for (key, error) in events {
            let Some(lod) = self.bodies.values_mut().find(|lod| lod.source_key == key) else {
                continue;
            };
            match error {
                None if !lod.world_ready => {
                    lod.world_ready = true;
                    lod.world_ready_s = Some(lod.bound_at.elapsed().as_secs_f64());
                }
                None => {}
                Some(error) => lod.world_error = Some(error),
            }
        }
    }

    /// PROTOTYPE (M3 Water): start hydrology for world bakes read back by
    /// the renderer.
    pub fn receive_world_fields(&mut self, fields: Vec<astrum_renderer::AtlasWorldFields>) {
        for field in fields {
            let radius_m = self
                .bodies
                .values()
                .chain(self.outgoing.values())
                .find(|lod| lod.source_key == field.source)
                .and_then(|lod| match lod.recipe.as_ref() {
                    ProducerRecipe::World(world) => Some(world.field.inputs().radius_m),
                    _ => None,
                });
            self.hydrology.start(field, radius_m);
        }
    }

    /// PROTOTYPE (M3 Water): packed rivers ready for the renderer.
    pub fn take_world_rivers(&mut self) -> Vec<(u64, Vec<u32>)> {
        self.hydrology.poll()
    }

    /// A rebind of `body` is still preparing, baking or filling tiles while
    /// its previous definition draws.
    pub fn replacing(&self, body: BodyId) -> bool {
        self.pending.contains_key(&body) || self.outgoing.contains_key(&body)
    }

    /// Why a world-map body's source could not be used, if it failed.
    pub fn world_bake_error(&self, body: BodyId) -> Option<&str> {
        self.bodies.get(&body)?.world_error.as_deref()
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
                    if let Some(collision) = &mut lod.collision {
                        collision.far_field(address, grid);
                    }
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
            self.outgoing.clear();
            self.tokens.clear();
            self.collision_tokens.clear();
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
            self.supersede(input.body);
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
        let world_ready = !matches!(recipe, ProducerRecipe::World(_));
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
                world_ready,
                world_ready_s: None,
                world_error: None,
                collision: self.collision_policy.map(|policy| {
                    collision::BodyCollision::new(collision::BodyColliders::new(
                        policy.physics_level(input.radius_m),
                        self.policy.map_or(0, |lod| lod.base_resident_level),
                        height_bound_m,
                    ))
                }),
            },
        );
        Ok(true)
    }

    fn release(&mut self, body: BodyId) {
        if let Some(old) = self.outgoing.remove(&body) {
            self.retire(body, old);
        }
        if let Some(lod) = self.bodies.remove(&body) {
            self.retire(body, lod);
        }
    }

    /// A new definition replaces the bound one. The bound one keeps drawing
    /// until the replacement is complete, unless an older one is already
    /// kept (rapid edits) or it has nothing to draw yet.
    fn supersede(&mut self, body: BodyId) {
        let Some(current) = self.bodies.remove(&body) else {
            return;
        };
        if current.world_ready && !current.nodes.is_empty() && !self.outgoing.contains_key(&body) {
            self.outgoing.insert(body, current);
        } else {
            self.retire(body, current);
        }
    }

    /// Frees one definition's atlas layers, tile tokens and GPU source.
    fn retire(&mut self, body: BodyId, lod: BodyLod) {
        self.retired_sources.push(lod.source_key);
        for node in lod.nodes.values() {
            self.free_layers.push(node.layer);
        }
        self.tokens
            .retain(|_, (owner, slot, _)| !(*owner == body && *slot == lod.slot));
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
        shadows: Option<ShadowCasterPolicy>,
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
        let mode = render_mode;
        // Cascaded shadows go to the drawable surface nearest the camera.
        let shadow_body = if shadows.is_some() {
            bodies
                .iter()
                .filter_map(|input| {
                    let observer = view
                        .prepare_source(input.body_fixed_frame)
                        .ok()?
                        .observer_in_source()
                        .metres();
                    Some((input.body, observer.length() - input.radius_m))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(body, _)| body)
        } else {
            None
        };
        let mut frame = TerrainAtlasFrame {
            config: Some(config),
            ..Default::default()
        };
        let now = Instant::now();
        let mut budget = policy.jobs_per_frame as usize;
        let mut collision_budget = self
            .collision_policy
            .map_or(0, |collision| collision.jobs_per_frame as usize);
        let ctx = FrameContext {
            policy,
            frustum,
            focal,
            mode,
            shadows,
            shadow_body,
            projection,
            now,
        };
        for input in bodies {
            let source = view.prepare_source(input.body_fixed_frame)?;
            let observer = source.observer_in_source().metres();
            let pixels = input.radius_m * focal / observer.length().max(1.0);
            if pixels < policy.min_body_pixels {
                continue;
            }
            // A rebind keeps the replaced definition drawing until its
            // replacement is complete, so edits never flash the plain sphere.
            let bound = match self.bind(input) {
                Ok(bound) => bound,
                Err(error) => {
                    self.last_error = Some(format!("{error:#}"));
                    false
                }
            };
            let replacing = self.outgoing.contains_key(&input.body);
            if !bound && !replacing {
                continue;
            }
            let direction =
                |v| -> Result<DVec3> { Ok(source.view_direction(Direction3::try_new(v)?)?.unit()) };
            let place = BodyPlace {
                observer,
                body_to_view: DMat3::from_cols(
                    direction(DVec3::X)?,
                    direction(DVec3::Y)?,
                    direction(DVec3::Z)?,
                ),
            };
            let mut draw_new = bound;
            if bound && replacing {
                if self.replacement_complete(input.body, &policy) {
                    if let Some(old) = self.outgoing.remove(&input.body) {
                        self.retire(input.body, old);
                    }
                } else {
                    draw_new = false;
                }
            }
            if bound {
                self.process_body(
                    input,
                    place,
                    &ctx,
                    &mut frame,
                    &mut budget,
                    &mut collision_budget,
                    true,
                    draw_new,
                )?;
            }
            if let Some(old) = self.outgoing.remove(&input.body) {
                // Draw the replaced definition in the body's slot for one pass.
                let new = self.bodies.insert(input.body, old);
                let result = self.process_body(
                    input,
                    place,
                    &ctx,
                    &mut frame,
                    &mut budget,
                    &mut collision_budget,
                    false,
                    true,
                );
                let old = match new {
                    Some(new) => self.bodies.insert(input.body, new),
                    None => self.bodies.remove(&input.body),
                }
                .expect("the replaced definition was swapped in");
                self.outgoing.insert(input.body, old);
                result?;
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
        let stale: Vec<BodyId> = self
            .outgoing
            .iter()
            .filter(|(_, lod)| frame_number - lod.last_active > INACTIVE_RELEASE_FRAMES)
            .map(|(body, _)| *body)
            .collect();
        for body in stale {
            if let Some(old) = self.outgoing.remove(&body) {
                self.retire(body, old);
            }
        }
        if self.tokens.len() > 100_000 {
            // Readbacks dropped under pressure never return; forget old tokens.
            let floor = self.next_token.saturating_sub(50_000);
            self.tokens.retain(|token, _| *token >= floor);
            self.collision_tokens.retain(|token, _| *token >= floor);
        }
        self.jobs_total += frame.jobs.len() as u64;
        frame.retired_sources = std::mem::take(&mut self.retired_sources);
        self.selection_ms = started.elapsed().as_secs_f64() * 1000.0;
        Ok(frame)
    }

    /// Selection, tile requests and instances of one bound body this frame.
    /// `request` issues tile and collider jobs; `draw` emits instances and
    /// shadow casters. A replacement fills tiles without drawing while the
    /// definition it replaces draws without requesting.
    #[allow(clippy::too_many_arguments)] // One body's share of the frame.
    fn process_body(
        &mut self,
        input: &AtlasBodyInput<'_>,
        place: BodyPlace,
        ctx: &FrameContext,
        frame: &mut TerrainAtlasFrame,
        budget: &mut usize,
        collision_budget: &mut usize,
        request: bool,
        draw: bool,
    ) -> Result<()> {
        let BodyPlace {
            observer,
            body_to_view,
        } = place;
        let FrameContext {
            policy,
            frustum,
            focal,
            mode,
            shadows,
            shadow_body,
            projection,
            now,
        } = *ctx;
        let frame_number = self.frame;
        let lod = self.bodies.get_mut(&input.body).expect("bound above");
        lod.last_active = frame_number;
        frame
            .sources
            .push((lod.source_key, Arc::clone(&lod.source)));
        // A world-map body draws as a plain sphere until its Tier A bake is
        // complete; pushing its source above keeps the bake advancing.
        if !lod.world_ready {
            return Ok(());
        }
        let radius = input.radius_m;
        let data_offset = policy.data_level_offset();
        let ranges = lod_ranges(&policy, radius, focal);
        let selection = {
            let _span = crate::engine_profile::span("Atlas selection");
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
                    restricted: true,
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

        let _requests_span = crate::engine_profile::span("Atlas requests and draw list");
        // Requests: base levels first, then missing selected nodes coarse-first.
        let mut wanted: Vec<(u8, f64, CubePatchAddress)> = Vec::new();
        if request && !self.hold {
            for level in 0..=policy.base_resident_level {
                for face in astrum_math::surface::CubeFace::ALL {
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
                if *budget == 0 {
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
                let octaves =
                    producer::detail_octaves(&lod.recipe, node, &chart, policy.tile_cells)?;
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
                    octaves,
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
                *budget -= 1;
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
            balance_splits: selection.balance_splits,
            balance_unresolved: selection.balance_unresolved,
            jobs_last_frame: jobs_this_body,
            ..BodyStats::default()
        };
        for selected in &selection.selected {
            let Some(built) = build_instance(
                &lod.nodes,
                selected,
                &NodeContext {
                    data_offset,
                    policy: &policy,
                    ranges: &ranges,
                    body_to_view,
                    observer,
                    radius,
                    now,
                    material: input.material,
                    water: producer::water_look(&lod.recipe),
                    mode,
                    morph: true,
                },
            ) else {
                stats.missing_visible += 1;
                continue;
            };
            if built.virtual_node {
                stats.virtual_nodes += 1;
                stats.missing_visible += 1;
            }
            if built.arrival < 1.0 {
                stats.fading += 1;
            }
            if draw {
                frame.instances.push(built.instance);
            }
            stats.drawn += 1;
            if selected.coarser_edges != 0 {
                stats.snapped_nodes += 1;
            }
            stats.finest_level = stats.finest_level.max(selected.address.level());
            // Mark data sources as used for LRU.
            for owner in [built.owner, built.parent_owner] {
                if let Some(node) = lod.nodes.get_mut(&owner) {
                    node.last_used = frame_number;
                }
            }
        }
        if let Some(shadow_policy) = shadows.filter(|_| draw && shadow_body == Some(input.body)) {
            let _span = crate::engine_profile::span("Atlas shadow casters");
            let view = ShadowView {
                body_to_view,
                observer_body_m: observer,
                reference_radius_m: radius,
                relief_m: lod.height_bound_m,
            };
            let body_centre = body_to_view * (-observer);
            let sun_view = (shadow_policy.sun_centre_view_m - body_centre).normalize();
            if let Some(cascades) =
                astrum_renderer::fit_cascades(&view, sun_view, projection, &shadow_policy.settings)
            {
                let mut shadow = astrum_renderer::AtlasShadowFrame {
                    cascades,
                    ..Default::default()
                };
                let detail = f64::from(shadow_policy.settings.caster_detail).max(0.05);
                for c in 0..cascades.count as usize {
                    // Cells of four texels near the cascade, growing linearly
                    // with distance beyond its radius: distant sunward
                    // casters only need coarse silhouettes.
                    let texel = f64::from(cascades.texel_m[c]);
                    let target = 4.0 * texel / detail;
                    let reach = 0.5 * texel * f64::from(shadow_policy.settings.resolution);
                    let caster_ranges: Vec<f64> = (0..=policy.max_level)
                        .map(|level| {
                            let cell = astrum_world::terrain::producer::tile_texel_m(
                                radius,
                                level,
                                policy.draw_cells,
                            );
                            if cell > target {
                                reach * cell / target
                            } else {
                                0.0
                            }
                        })
                        .collect();
                    let measured = &lod.bounds;
                    let bound = lod.height_bound_m;
                    let casters = select::select(
                        &select::SelectionInput {
                            observer_body: observer,
                            body_to_view,
                            frustum: select::Frustum::cascade(cascades, c),
                            radius_m: radius,
                            occluder_radius_m: radius - lod.height_bound_m,
                            ranges: &caster_ranges,
                            max_level: policy.max_level,
                            visit_limit: VISIT_LIMIT,
                            restricted: false,
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
                    );
                    let context = NodeContext {
                        data_offset,
                        policy: &policy,
                        ranges: &caster_ranges,
                        body_to_view,
                        observer,
                        radius,
                        now,
                        material: input.material,
                        water: producer::water_look(&lod.recipe),
                        mode,
                        morph: false,
                    };
                    shadow.casters[c] = casters
                        .selected
                        .iter()
                        .filter_map(|s| build_instance(&lod.nodes, s, &context))
                        .map(|built| built.instance)
                        .collect();
                }
                stats.shadow_casters = shadow.casters.iter().map(Vec::len).sum();
                frame.shadow = Some(shadow);
            }
        }
        if let (Some(collision_policy), Some(collision), false) = (
            self.collision_policy,
            lod.collision.as_mut(),
            self.hold || !request,
        ) {
            let _span = crate::engine_profile::span("Collision page requests");
            collision.prefetch(observer, radius, &collision_policy, frame_number);
            for address in collision.schedule(frame_number, *collision_budget) {
                let chart = select::chart(address);
                let cells = collision_policy.page_cells;
                let token = self.next_token;
                self.next_token += 1;
                self.collision_tokens
                    .insert(token, (input.body, lod.slot, address, frame_number));
                frame.collision_jobs.push(AtlasProduceJob {
                    source: lod.source_key,
                    layer: 0,
                    token,
                    chart: producer::atlas_chart(&chart),
                    radius_m: radius as f32,
                    kind: producer::tile_kind(&lod.recipe, address, &chart, cells)?,
                    octaves: producer::detail_octaves(&lod.recipe, address, &chart, cells)?,
                });
                *collision_budget -= 1;
                self.collision_stats.requested_total += 1;
            }
            frame.collision_cells = collision_policy.page_cells;
            stats.collision_pages = collision.colliders.page_count();
            stats.collision_in_flight = collision.in_flight();
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
        if draw && stats.drawn > 0 {
            self.drawn_bodies.push(input.body);
        }
        Ok(())
    }

    /// Whether a rebound body can replace the definition still drawn in its
    /// place: its world map is baked, its base levels are resident and the
    /// view is filled at the wanted detail, or a short wait has passed.
    fn replacement_complete(&self, body: BodyId, policy: &LodPolicy) -> bool {
        let Some(lod) = self.bodies.get(&body) else {
            return false;
        };
        if !lod.world_ready {
            return false;
        }
        let base_tiles: usize = (0..=policy.base_resident_level)
            .map(|level| 6usize << (2 * u32::from(level)))
            .sum();
        let resident_base = lod
            .nodes
            .keys()
            .filter(|address| address.level() <= policy.base_resident_level)
            .count();
        if resident_base < base_tiles {
            return false;
        }
        let stats = lod.stats;
        let filled = stats.drawn > 0
            && stats.missing_visible == 0
            && stats.fading == 0
            && stats.jobs_last_frame == 0;
        let waited = lod
            .world_ready_s
            .is_some_and(|ready| lod.bound_at.elapsed().as_secs_f64() - ready > REPLACEMENT_WAIT_S);
        filled || waited
    }

    /// Fill the shared terrain summary used by the developer UI and snapshots.
    pub fn annotate_terrain(
        &self,
        terrain: &mut crate::developer_snapshot::TerrainSnapshot,
        ids: &[BodyId],
        system: &astrum_world::CelestialSystem,
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
            "collision_policy": self.collision_policy,
            "collision": self.collision_stats,
            "selection_ms": self.selection_ms,
            "bodies": bodies,
            "frame_samples": self.samples,
            "last_error": self.last_error,
        })
    }
}

/// Distance below which a node of each level must be refined. The same
/// ranges set the CDLOD morph band (`build_instance`), so split decisions and
/// morphing share one error metric (§9.4).
fn lod_ranges(policy: &LodPolicy, radius_m: f64, focal_px: f64) -> Vec<f64> {
    (0..=policy.max_level)
        .map(|level| {
            let cell =
                astrum_world::terrain::producer::tile_texel_m(radius_m, level, policy.draw_cells);
            (cell * focal_px / policy.pixel_error)
                .max(policy.min_range_factor * cell * f64::from(policy.draw_cells))
        })
        .collect()
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

/// Shared inputs for turning a selected node into a draw instance.
struct NodeContext<'a> {
    data_offset: u8,
    policy: &'a LodPolicy,
    ranges: &'a [f64],
    body_to_view: DMat3,
    observer: DVec3,
    radius: f64,
    now: Instant,
    material: SurfaceMaterial,
    /// Flat water over ground below the reference radius.
    water: Option<astrum_renderer::AtlasWater>,
    mode: u32,
    /// CDLOD morphing; uniform-detail shadow casters do not morph.
    morph: bool,
}

struct BuiltInstance {
    instance: AtlasInstance,
    owner: CubePatchAddress,
    parent_owner: CubePatchAddress,
    arrival: f64,
    /// Drawn from ancestor data while its own data is missing.
    virtual_node: bool,
}

/// Instance of a selected node from its own data or its finest resident
/// ancestor; `None` when nothing resident covers it.
fn build_instance(
    nodes: &HashMap<CubePatchAddress, Resident>,
    selected: &select::Selected,
    c: &NodeContext<'_>,
) -> Option<BuiltInstance> {
    let address = selected.address;
    let data = data_address(address, c.data_offset);
    let (owner, owner_node) = finest_resident(nodes, data)?;
    let (own_origin, own_scale) = select::rect_within(address, owner);
    let own_is_self = owner == data;
    // The parent draw node's data tile supplies morph and arrival
    // endpoints; it is `data`'s parent unless both clamp to a root.
    let parent_data = address
        .parent()
        .map_or(data, |parent| data_address(parent, c.data_offset));
    let mut parent_owner = owner;
    let (parent_source, arrival) = if own_is_self && parent_data != data {
        match finest_resident(nodes, parent_data) {
            Some((parent, node)) => {
                parent_owner = parent;
                let (origin, scale) = select::rect_within(address, parent);
                let arrival = if c.policy.arrival_seconds > 0.0 {
                    (c.now.duration_since(owner_node.produced).as_secs_f64()
                        / c.policy.arrival_seconds)
                        .clamp(0.0, 1.0)
                } else {
                    1.0
                };
                (sample(node.layer, origin, scale), arrival)
            }
            None => (sample(owner_node.layer, own_origin, own_scale), 1.0),
        }
    } else {
        // Ancestor data stands in for both endpoints; only the grid morph
        // applies until the node's own data arrives.
        (sample(owner_node.layer, own_origin, own_scale), 1.0)
    };
    let level = address.level();
    let chart = select::chart(address);
    let anchor_body = chart.n0 * c.radius;
    let anchor_view = c.body_to_view * (anchor_body - c.observer);
    let (morph_start, morph_end) = if level == 0 || !c.morph {
        (f32::MAX, f32::MAX)
    } else {
        let end = c.ranges[level as usize - 1];
        let start = end - c.policy.morph_fraction * (end - c.ranges[level as usize]);
        (start as f32, end as f32)
    };
    let cell = astrum_world::terrain::producer::tile_texel_m(c.radius, level, c.policy.draw_cells);
    Some(BuiltInstance {
        instance: AtlasInstance {
            anchor_view_m: anchor_view.as_vec3().to_array(),
            radius_m: c.radius as f32,
            body_to_view: [
                c.body_to_view.x_axis.as_vec3().to_array(),
                c.body_to_view.y_axis.as_vec3().to_array(),
                c.body_to_view.z_axis.as_vec3().to_array(),
            ],
            chart: producer::atlas_chart(&chart),
            level,
            own: sample(owner_node.layer, own_origin, own_scale),
            parent: parent_source,
            morph_start_m: morph_start,
            morph_end_m: morph_end,
            arrival: arrival as f32,
            skirt_m: (c.policy.skirt_cells * cell) as f32,
            material: c.material,
            water: c.water,
            mode: c.mode,
            // Shadow casters do not morph, so they need no edge snap.
            coarser_edges: if c.morph { selected.coarser_edges } else { 0 },
            finer_edges: if c.morph { selected.finer_edges } else { 0 },
        },
        owner,
        parent_owner,
        arrival,
        virtual_node: !own_is_self,
    })
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
        use astrum_math::surface::CubeFace;
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

    pub(super) fn test_body_id() -> BodyId {
        use astrum_math::*;
        use astrum_world::*;
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
            astrum_world::terrain::TerrainIdentity(1),
            astrum_world::terrain::TerrainSeed(2),
            astrum_world::terrain::SurfaceAlgorithm::MoonFieldsV1,
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
            collision: None,
            world_ready: true,
            world_ready_s: None,
            world_error: None,
        }
    }

    #[test]
    fn missing_nodes_draw_from_the_finest_resident_ancestor() {
        use astrum_math::surface::CubeFace;
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

    #[test]
    fn morph_band_uses_the_split_ranges_and_completes_at_its_end() {
        let policy = policy();
        let radius = 109_081.776_801_130_12;
        let ranges = lod_ranges(&policy, radius, 700.0);
        for level in 1..=policy.max_level {
            let level = usize::from(level);
            // A level-L node exists while its parent is within ranges[L-1]
            // and morphs to the parent grid over the outer part of that band.
            let end = ranges[level - 1];
            let start = end - policy.morph_fraction * (end - ranges[level]);
            assert!(ranges[level] < start && start < end);
            let t = |d: f64| ((d - start) / (end - start)).clamp(0.0, 1.0);
            assert_eq!(t(end), 1.0);
            assert_eq!(t(start), 0.0);
        }
    }

    /// Synthetic terrain for seam tests: smooth, multi-scale and analytic, so
    /// every data tile samples one exact function.
    fn seam_height(direction: DVec3) -> f64 {
        200.0 * (50.0 * direction.x + 1.0).sin() * (43.0 * direction.y).sin()
            + 20.0 * (2100.0 * direction.x + 1900.0 * direction.z).sin()
            + 2.0 * (41_000.0 * direction.y + 37_000.0 * direction.z).sin()
    }

    /// f64 replica of `terrain_atlas.wgsl` `vs_main`: the drawn body-space
    /// position of grid point `st` of `instance` (node `address`).
    fn replica_vertex(
        instance: &AtlasInstance,
        address: CubePatchAddress,
        layers: &HashMap<u32, CubePatchAddress>,
        policy: &LodPolicy,
        observer: DVec3,
        st: [f64; 2],
    ) -> (DVec3, f64) {
        let cells = f64::from(policy.tile_cells);
        let draw = f64::from(policy.draw_cells);
        let radius = f64::from(instance.radius_m);
        let data = |layer: u32, s: [f64; 2]| {
            let owner = select::chart(layers[&layer]);
            let p = s.map(|c| (c * cells + 1.0).clamp(0.0, cells + 2.0));
            let base = p.map(|c| c.floor().min(cells + 1.0));
            let texel = |i: f64, j: f64| {
                seam_height(owner.direction([(i - 1.0) / cells, (j - 1.0) / cells]))
            };
            let f = [p[0] - base[0], p[1] - base[1]];
            let a = texel(base[0], base[1]) * (1.0 - f[0]) + texel(base[0] + 1.0, base[1]) * f[0];
            let b = texel(base[0], base[1] + 1.0) * (1.0 - f[0])
                + texel(base[0] + 1.0, base[1] + 1.0) * f[0];
            a * (1.0 - f[1]) + b * f[1]
        };
        let sample = |source: AtlasSampleSource, s: [f64; 2]| {
            let scale = f64::from(source.scale);
            data(
                source.layer,
                [
                    f64::from(source.origin[0]) + s[0] * scale,
                    f64::from(source.origin[1]) + s[1] * scale,
                ],
            )
        };
        let blended = |s: [f64; 2], morph: f64| {
            let own = sample(instance.own, s);
            let coarse = sample(instance.parent, s);
            let arrived = coarse + (own - coarse) * f64::from(instance.arrival);
            arrived + (coarse - arrived) * morph
        };
        let chart = select::chart(address);
        let position = |s: [f64; 2], h: f64| chart.direction(s) * (radius + h);
        let d0 = (position(st, blended(st, 0.0)) - observer).length();
        let (start, end) = (
            f64::from(instance.morph_start_m),
            f64::from(instance.morph_end_m),
        );
        let mut morph = ((d0 - start) / (end - start).max(1.0e-6)).clamp(0.0, 1.0);
        let g = st.map(|c| c * draw);
        let bits = u8::from(g[0] < 0.5)
            | (u8::from(g[0] > draw - 0.5) << 1)
            | (u8::from(g[1] < 0.5) << 2)
            | (u8::from(g[1] > draw - 0.5) << 3);
        if instance.finer_edges & bits != 0 {
            morph = 0.0;
        }
        if instance.coarser_edges & bits != 0 {
            morph = 1.0;
        }
        let morphed = [0, 1].map(|i| st[i] - (g[i] * 0.5).fract() * (2.0 / draw) * morph);
        (position(morphed, blended(morphed, morph)), morph)
    }

    fn edge_points(edge: astrum_math::surface::PatchEdge, draw: u32) -> Vec<[f64; 2]> {
        (0..=draw)
            .map(|k| {
                let [i, j] = edge.grid(k, draw);
                [
                    f64::from(i) / f64::from(draw),
                    f64::from(j) / f64::from(draw),
                ]
            })
            .collect()
    }

    fn distance_to_polyline(p: DVec3, line: &[DVec3]) -> f64 {
        line.windows(2)
            .map(|w| {
                let d = w[1] - w[0];
                let t = if d.length_squared() > 0.0 {
                    ((p - w[0]).dot(d) / d.length_squared()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (p - (w[0] + d * t)).length()
            })
            .fold(f64::INFINITY, f64::min)
    }

    #[derive(Debug, Default)]
    struct SeamReport {
        /// Coarser neighbour drawn at its own level along the shared edge.
        coarser_edges: usize,
        coarser_gap_m: f64,
        /// Coarser neighbour itself morphing towards its parent there.
        coarser_morphing_edges: usize,
        coarser_morphing_gap_m: f64,
        same_level_edges: usize,
        same_level_gap_m: f64,
        reversed_edges: usize,
        reversed_gap_m: f64,
        balance_splits: usize,
    }

    /// Select around a near-ground observer, build real instances with every
    /// data tile resident, and measure how far each node's drawn edge vertices
    /// lie from its neighbour's drawn edge polyline (§19.2).
    fn seam_report(
        defer: bool,
        up: DVec3,
        bound_m: f64,
        viewport: [f64; 2],
        max_level: u8,
        same_level_stride: usize,
    ) -> SeamReport {
        let mut same_level_seen = 0usize;
        use astrum_math::surface::PatchEdge;
        let policy = policy();
        let radius = 109_081.776_801_130_12;
        let focal = 700.0;
        let up = up.normalize();
        let observer = up * (radius + seam_height(up) + 30.0);
        let forward = DVec3::X.reject_from(up).normalize();
        let right = forward.cross(up).normalize();
        let view_to_body = DMat3::from_cols(right, up, -forward);
        let body_to_view = view_to_body.transpose();
        let ranges = lod_ranges(&policy, radius, focal);
        let selection = select::select(
            &select::SelectionInput {
                observer_body: observer,
                body_to_view,
                frustum: select::Frustum::new(focal, viewport),
                radius_m: radius,
                occluder_radius_m: radius - 250.0,
                // Tight bounds keep the selection small; heights only matter to
                // the replica, which samples `seam_height` directly.
                ranges: &ranges,
                max_level,
                visit_limit: 100_000,
                restricted: true,
            },
            |node| {
                let [x, y] = node.coordinates();
                let refinable = !defer || node.level() < 8 || (x ^ y) % 3 != 0;
                ([-bound_m, bound_m], refinable)
            },
        );
        let data_offset = policy.data_level_offset();
        let produced = Instant::now() - std::time::Duration::from_secs(60);
        let mut nodes = HashMap::new();
        let mut layers = HashMap::new();
        for s in &selection.selected {
            let mut cursor = Some(data_address(s.address, data_offset));
            while let Some(node) = cursor {
                if nodes.contains_key(&node) {
                    break;
                }
                let layer = layers.len() as u32;
                layers.insert(layer, node);
                nodes.insert(
                    node,
                    Resident {
                        layer,
                        produced,
                        last_used: 0,
                    },
                );
                cursor = node.parent();
            }
        }
        let context = NodeContext {
            data_offset,
            policy: &policy,
            ranges: &ranges,
            body_to_view,
            observer,
            radius,
            now: Instant::now(),
            material: Default::default(),
            water: None,
            mode: 0,
            morph: true,
        };
        let instances: HashMap<_, _> = selection
            .selected
            .iter()
            .map(|s| {
                let built = build_instance(&nodes, s, &context).expect("all data resident");
                assert!(!built.virtual_node && built.arrival == 1.0);
                (s.address, built.instance)
            })
            .collect();
        let vertex = |address: CubePatchAddress, st: [f64; 2]| {
            replica_vertex(
                &instances[&address],
                address,
                &layers,
                &policy,
                observer,
                st,
            )
        };
        let mut report = SeamReport {
            balance_splits: selection.balance_splits,
            ..SeamReport::default()
        };
        for s in &selection.selected {
            for edge in PatchEdge::ALL {
                let neighbour = s.address.neighbor(edge);
                let mut cursor = Some(neighbour.address);
                let mut cover = None;
                while let Some(node) = cursor {
                    if instances.contains_key(&node) {
                        cover = Some(node);
                        break;
                    }
                    cursor = node.parent();
                }
                let Some(cover) = cover else { continue };
                if cover.level() == s.address.level() {
                    same_level_seen += 1;
                    if !same_level_seen.is_multiple_of(same_level_stride) {
                        continue;
                    }
                }
                let cover_edge: Vec<(DVec3, f64)> = edge_points(neighbour.edge, policy.draw_cells)
                    .into_iter()
                    .map(|st| vertex(cover, st))
                    .collect();
                let line: Vec<DVec3> = cover_edge.iter().map(|v| v.0).collect();
                let cover_morph = cover_edge.iter().map(|v| v.1).fold(0.0, f64::max);
                let gap = edge_points(edge, policy.draw_cells)
                    .into_iter()
                    .map(|st| distance_to_polyline(vertex(s.address, st).0, &line))
                    .fold(0.0, f64::max);
                if cover.level() < s.address.level() && cover_morph > 0.0 {
                    report.coarser_morphing_edges += 1;
                    report.coarser_morphing_gap_m = report.coarser_morphing_gap_m.max(gap);
                } else if cover.level() < s.address.level() {
                    report.coarser_edges += 1;
                    report.coarser_gap_m = report.coarser_gap_m.max(gap);
                } else if neighbour.reversed {
                    report.reversed_edges += 1;
                    report.reversed_gap_m = report.reversed_gap_m.max(gap);
                } else {
                    report.same_level_edges += 1;
                    report.same_level_gap_m = report.same_level_gap_m.max(gap);
                }
            }
        }
        report
    }

    #[test]
    fn drawn_edges_meet_same_level_and_coarser_neighbours() {
        // The canonical camera direction, and a cube corner where three faces
        // meet (cross-face edges; this cube basis never reverses orientation).
        for up in [
            DVec3::new(2000.7, 1250.4, 109_120.9),
            DVec3::new(1.0, 1.0, 1.0),
        ] {
            for defer in [false, true] {
                let report = seam_report(defer, up, 2.0, [1280.0, 720.0], 20, 1);
                eprintln!("seams at {up} (deferred bounds: {defer}): {report:?}");
                assert!(report.coarser_edges > 0 && report.same_level_edges > 0);
                // The f64 replica is exact up to rounding: fine edges snapped
                // to the coarse line lie on the coarse neighbour's drawn edge,
                // and same-orientation same-level edges coincide.
                assert!(report.coarser_gap_m < 1.0e-6, "{report:?}");
                assert!(report.same_level_gap_m < 1.0e-6, "{report:?}");
            }
        }
    }

    #[test]
    fn edges_meet_while_bounds_are_unmeasured() {
        // Before tiles report measured bounds, selection uses the conservative
        // body bound (hundreds of metres), which inflates deep nodes, forces
        // balance splits and leaves coarse neighbours mid-morph at fine edges.
        let report = seam_report(
            false,
            DVec3::new(2000.7, 1250.4, 109_120.9),
            250.0,
            [640.0, 360.0],
            18,
            // Same-level edges are covered above; sample them sparsely here.
            97,
        );
        eprintln!("seams with unmeasured bounds: {report:?}");
        assert!(report.balance_splits > 0 && report.coarser_morphing_edges > 0);
        assert!(report.coarser_gap_m < 1.0e-6, "{report:?}");
        assert!(report.coarser_morphing_gap_m < 1.0e-6, "{report:?}");
        assert!(report.same_level_gap_m < 1.0e-6, "{report:?}");
    }
}
