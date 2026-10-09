//! Read-back terrain colliders and the far-field collider (pipeline §15.1–15.2,
//! ADR 0023).
//!
//! The GPU terrain producer evaluates collision pages at one fixed physics
//! level per body; pages are read back asynchronously into CPU heightfields.
//! Height queries resolve from resident pages, and a miss schedules its page
//! and returns `Pending`. The far-field collider is the measured `[min, max]`
//! of the body's always-resident base atlas tiles: coarse, conservative and
//! available once those tiles are produced.
use anyhow::{Result, ensure};
use astrum_math::{
    Direction3,
    surface::{CubePatchAddress, SurfaceLocation},
};
use astrum_renderer::{MAX_COLLISION_CELLS, MAX_COLLISION_JOBS_PER_FRAME};
use astrum_world::{
    BodyId,
    terrain::{
        producer::tile_texel_m,
        surface_query::{QueryResult, SurfaceHeight, SurfaceQuery},
    },
};
use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// Authored, validated collision policy (content `collision` block).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollisionPolicy {
    pub schema: u32,
    /// Largest collider sample spacing; selects each body's physics level.
    pub texel_m: f64,
    /// Cells per collision page edge.
    pub page_cells: u32,
    /// Resident pages per body before least-recently-used eviction.
    pub max_pages: usize,
    /// Pages within this surface distance of a near-surface observer stay resident.
    pub prefetch_radius_m: f64,
    /// Prefetch only below this altitude above the reference radius.
    pub prefetch_altitude_m: f64,
    /// Collision pages produced per frame over all bodies.
    pub jobs_per_frame: u32,
}

/// Deepest physics level considered; far below any useful collider spacing.
const MAX_PHYSICS_LEVEL: u8 = 24;
/// A page still missing this many frames after its request is requested again
/// (its readback batch was dropped). Pages normally arrive in 2-4 frames.
const RETRY_FRAMES: u64 = 20;
/// Misses kept per view between frames.
const MAX_MISSES: usize = 256;

impl CollisionPolicy {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported collision schema");
        ensure!(
            self.texel_m.is_finite() && (0.05..=64.0).contains(&self.texel_m),
            "collision texel_m must be within 0.05..=64"
        );
        ensure!(
            self.page_cells.is_power_of_two()
                && (8..=MAX_COLLISION_CELLS).contains(&self.page_cells),
            "collision page_cells must be a power of two in 8..={MAX_COLLISION_CELLS}"
        );
        ensure!(
            (4..=4096).contains(&self.max_pages),
            "collision max_pages must be within 4..=4096"
        );
        ensure!(
            self.prefetch_radius_m.is_finite()
                && (0.0..=10_000.0).contains(&self.prefetch_radius_m),
            "collision prefetch_radius_m must be within 0..=10000"
        );
        ensure!(
            self.prefetch_altitude_m.is_finite()
                && (0.0..=1.0e6).contains(&self.prefetch_altitude_m),
            "collision prefetch_altitude_m must be within 0..=1e6"
        );
        ensure!(
            (1..=MAX_COLLISION_JOBS_PER_FRAME as u32).contains(&self.jobs_per_frame),
            "collision jobs_per_frame must be within 1..={MAX_COLLISION_JOBS_PER_FRAME}"
        );
        Ok(())
    }

    /// Coarsest level whose collision page texels are at most `texel_m`.
    pub fn physics_level(&self, radius_m: f64) -> u8 {
        (0..=MAX_PHYSICS_LEVEL)
            .find(|&level| tile_texel_m(radius_m, level, self.page_cells) <= self.texel_m)
            .unwrap_or(MAX_PHYSICS_LEVEL)
    }
}

/// The page at `level` containing `direction`, and the direction's chart
/// coordinates `st` inside it.
pub fn page_at(direction: DVec3, level: u8) -> Option<(CubePatchAddress, [f64; 2])> {
    let location = SurfaceLocation::new(Direction3::try_new(direction).ok()?);
    let (face, uv) = location.face_uv();
    let count = 1u64 << level;
    let scaled = uv.map(|c| (c + 1.0) * 0.5 * count as f64);
    let index = scaled.map(|c| (c.floor().max(0.0) as u64).min(count - 1) as u32);
    let address = CubePatchAddress::try_new(face, level, index[0], index[1]).ok()?;
    let st = [0, 1].map(|i| (scaled[i] - f64::from(index[i])).clamp(0.0, 1.0));
    Some((address, st))
}

/// One read-back heightfield page: `(cells + 1)^2` samples at `st = (i, j) / cells`.
#[derive(Debug, PartialEq)]
pub struct ColliderPage {
    cells: u32,
    heights: Vec<f32>,
    normals: Vec<[f32; 3]>,
}

impl ColliderPage {
    /// `None` unless the sample counts match and every value is finite.
    pub fn new(cells: u32, heights: Vec<f32>, normals: Vec<[f32; 3]>) -> Option<Self> {
        let samples = ((cells + 1) * (cells + 1)) as usize;
        (cells > 0
            && heights.len() == samples
            && normals.len() == samples
            && heights.iter().all(|h| h.is_finite())
            && normals.iter().flatten().all(|c| c.is_finite()))
        .then_some(Self {
            cells,
            heights,
            normals,
        })
    }

    /// Bilinear height and normal at chart coordinates `st`.
    pub fn sample(&self, st: [f64; 2]) -> SurfaceHeight {
        let cells = f64::from(self.cells);
        let side = self.cells as usize + 1;
        let p = st.map(|c| c.clamp(0.0, 1.0) * cells);
        let base = p.map(|c| (c.floor() as usize).min(self.cells as usize - 1));
        let f = [p[0] - base[0] as f64, p[1] - base[1] as f64];
        let index = |dx: usize, dy: usize| (base[1] + dy) * side + base[0] + dx;
        let weights = [
            (index(0, 0), (1.0 - f[0]) * (1.0 - f[1])),
            (index(1, 0), f[0] * (1.0 - f[1])),
            (index(0, 1), (1.0 - f[0]) * f[1]),
            (index(1, 1), f[0] * f[1]),
        ];
        let mut height_m = 0.0;
        let mut normal = DVec3::ZERO;
        for (i, w) in weights {
            height_m += f64::from(self.heights[i]) * w;
            normal += DVec3::from_array(self.normals[i].map(f64::from)) * w;
        }
        SurfaceHeight {
            height_m,
            normal: normal.normalize_or(DVec3::Z),
        }
    }
}

/// Read-only colliders of one body, shared with queries through `Arc`.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyColliders {
    level: u8,
    pages: HashMap<CubePatchAddress, Arc<ColliderPage>>,
    far_field_level: u8,
    /// Measured row-major 4x4 `[min, max]` grids of far-field-level tiles.
    far_field: HashMap<CubePatchAddress, [[f64; 2]; 16]>,
    height_bound_m: f64,
}

impl BodyColliders {
    pub fn new(level: u8, far_field_level: u8, height_bound_m: f64) -> Self {
        Self {
            level,
            pages: HashMap::new(),
            far_field_level,
            far_field: HashMap::new(),
            height_bound_m,
        }
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn height_at(&self, direction: DVec3) -> Option<SurfaceHeight> {
        let (address, st) = page_at(direction, self.level)?;
        Some(self.pages.get(&address)?.sample(st))
    }

    /// Conservative radial-offset range around `direction`: the measured grid
    /// cell of its far-field tile, or the recipe's global bound.
    pub fn far_field_bounds_m(&self, direction: DVec3) -> [f64; 2] {
        let global = [-self.height_bound_m, self.height_bound_m];
        let Some((address, st)) = page_at(direction, self.far_field_level) else {
            return global;
        };
        self.far_field.get(&address).map_or(global, |grid| {
            let cell = st.map(|c| ((c * 4.0).floor() as usize).min(3));
            grid[cell[1] * 4 + cell[0]]
        })
    }
}

/// Snapshot of every body's colliders plus the queries that missed since the
/// last `take_misses`. Implements the asynchronous `SurfaceQuery`.
#[derive(Debug, Clone, Default)]
pub struct ColliderView {
    bodies: HashMap<BodyId, Arc<BodyColliders>>,
    misses: Vec<(BodyId, DVec3)>,
}

impl ColliderView {
    pub(super) fn from_bodies(bodies: impl Iterator<Item = (BodyId, Arc<BodyColliders>)>) -> Self {
        Self {
            bodies: bodies.collect(),
            misses: Vec::new(),
        }
    }

    /// Adopt newer body snapshots, keeping misses not yet forwarded.
    pub fn refresh(&mut self, newer: ColliderView) {
        self.bodies = newer.bodies;
    }

    pub fn take_misses(&mut self) -> Vec<(BodyId, DVec3)> {
        std::mem::take(&mut self.misses)
    }

    pub fn body(&self, body: BodyId) -> Option<&BodyColliders> {
        self.bodies.get(&body).map(Arc::as_ref)
    }
}

impl SurfaceQuery for ColliderView {
    fn height_at(&mut self, body: BodyId, direction: DVec3) -> QueryResult<SurfaceHeight> {
        match self
            .bodies
            .get(&body)
            .and_then(|colliders| colliders.height_at(direction))
        {
            Some(height) => QueryResult::Ready(height),
            None => {
                if self.misses.len() < MAX_MISSES {
                    self.misses.push((body, direction));
                }
                QueryResult::Pending
            }
        }
    }

    fn far_field_bounds_m(&self, body: BodyId, direction: DVec3) -> Option<[f64; 2]> {
        self.bodies
            .get(&body)
            .map(|colliders| colliders.far_field_bounds_m(direction))
    }
}

/// Mutable collider residency of one bound body.
#[derive(Debug)]
pub(super) struct BodyCollision {
    pub(super) colliders: Arc<BodyColliders>,
    /// Pages wanted (prefetch or query miss) with the frame they were last wanted.
    wanted: HashMap<CubePatchAddress, u64>,
    /// Requested pages with their request frame.
    in_flight: HashMap<CubePatchAddress, u64>,
    last_used: HashMap<CubePatchAddress, u64>,
    /// Pages under query misses; scheduled ahead of prefetch.
    urgent: HashSet<CubePatchAddress>,
}

impl BodyCollision {
    pub(super) fn new(colliders: BodyColliders) -> Self {
        Self {
            colliders: Arc::new(colliders),
            wanted: HashMap::new(),
            in_flight: HashMap::new(),
            last_used: HashMap::new(),
            urgent: HashSet::new(),
        }
    }

    /// Want the page under a query miss, ahead of prefetched pages.
    pub(super) fn want_urgent(&mut self, direction: DVec3, frame: u64) {
        if let Some((address, _)) = page_at(direction, self.colliders.level) {
            self.urgent.insert(address);
        }
        self.want(direction, frame);
    }

    pub(super) fn want(&mut self, direction: DVec3, frame: u64) {
        if let Some((address, _)) = page_at(direction, self.colliders.level) {
            self.wanted.insert(address, frame);
            if self.colliders.pages.contains_key(&address) {
                self.last_used.insert(address, frame);
            }
        }
    }

    /// Want every page within `policy.prefetch_radius_m` of the observer's
    /// sub-point while it is below `policy.prefetch_altitude_m`.
    pub(super) fn prefetch(
        &mut self,
        observer_body: DVec3,
        radius_m: f64,
        policy: &CollisionPolicy,
        frame: u64,
    ) {
        let altitude = observer_body.length() - radius_m;
        if altitude.is_nan()
            || altitude >= policy.prefetch_altitude_m
            || policy.prefetch_radius_m <= 0.0
        {
            return;
        }
        let up = observer_body.normalize();
        let east = up.any_orthonormal_vector();
        let north = up.cross(east);
        let page_m = tile_texel_m(radius_m, self.colliders.level, policy.page_cells)
            * f64::from(policy.page_cells);
        let spacing = 0.5 * page_m;
        let steps = (policy.prefetch_radius_m / spacing).ceil() as i32;
        for i in -steps..=steps {
            for j in -steps..=steps {
                let offset = east * (f64::from(i) * spacing) + north * (f64::from(j) * spacing);
                if offset.length() <= policy.prefetch_radius_m + spacing {
                    self.want(up * radius_m + offset, frame);
                }
            }
        }
    }

    /// Pages to request now: wanted, not resident and not recently requested,
    /// most recently wanted first.
    pub(super) fn schedule(&mut self, frame: u64, budget: usize) -> Vec<CubePatchAddress> {
        let mut candidates: Vec<(bool, u64, CubePatchAddress)> = self
            .wanted
            .iter()
            .filter(|(address, _)| {
                !self.colliders.pages.contains_key(address)
                    && self
                        .in_flight
                        .get(address)
                        .is_none_or(|&requested| frame - requested > RETRY_FRAMES)
            })
            .map(|(&address, &wanted)| (self.urgent.contains(&address), wanted, address))
            .collect();
        candidates.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        candidates.truncate(budget);
        for &(_, _, address) in &candidates {
            self.in_flight.insert(address, frame);
            self.urgent.remove(&address);
        }
        candidates
            .into_iter()
            .map(|(_, _, address)| address)
            .collect()
    }

    /// Store a read-back page and evict least-recently-used pages beyond
    /// `max_pages`. Wanted entries older than the eviction horizon are dropped.
    pub(super) fn receive(
        &mut self,
        address: CubePatchAddress,
        page: ColliderPage,
        frame: u64,
        max_pages: usize,
    ) {
        self.in_flight.remove(&address);
        self.last_used.insert(address, frame);
        let colliders = Arc::make_mut(&mut self.colliders);
        colliders.pages.insert(address, Arc::new(page));
        if colliders.pages.len() > max_pages {
            let mut by_age: Vec<(u64, CubePatchAddress)> = colliders
                .pages
                .keys()
                .map(|a| (self.last_used.get(a).copied().unwrap_or(0), *a))
                .collect();
            by_age.sort_unstable();
            let excess = colliders.pages.len() - max_pages;
            let evicted: HashSet<CubePatchAddress> =
                by_age.into_iter().take(excess).map(|(_, a)| a).collect();
            colliders.pages.retain(|a, _| !evicted.contains(a));
            for address in &evicted {
                self.last_used.remove(address);
                self.wanted.remove(address);
            }
        }
        self.wanted
            .retain(|_, wanted| frame - *wanted <= RETRY_FRAMES);
    }

    /// Record a measured far-field tile (atlas bounds of the far-field level).
    pub(super) fn far_field(&mut self, address: CubePatchAddress, grid: [[f64; 2]; 16]) {
        if address.level() == self.colliders.far_field_level
            && self.colliders.far_field.get(&address) != Some(&grid)
        {
            Arc::make_mut(&mut self.colliders)
                .far_field
                .insert(address, grid);
        }
    }

    pub(super) fn in_flight(&self) -> usize {
        self.in_flight.len()
    }
}

/// Snapshot counters for diagnostics.
#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct CollisionStats {
    pub requested_total: u64,
    pub received_total: u64,
    pub stale_total: u64,
    pub misses_last_frame: usize,
    /// Frames between request and arrival of the latest page, and the maximum seen.
    pub last_latency_frames: u64,
    pub max_latency_frames: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrum_math::surface::CubeFace;

    fn policy() -> CollisionPolicy {
        CollisionPolicy {
            schema: 1,
            texel_m: 1.0,
            page_cells: 32,
            max_pages: 8,
            prefetch_radius_m: 60.0,
            prefetch_altitude_m: 2000.0,
            jobs_per_frame: 8,
        }
    }

    fn flat_page(cells: u32, height: f32) -> ColliderPage {
        let samples = ((cells + 1) * (cells + 1)) as usize;
        ColliderPage::new(cells, vec![height; samples], vec![[0.0, 0.0, 1.0]; samples]).unwrap()
    }

    #[test]
    fn policy_validates_and_picks_the_physics_level() {
        let p = policy();
        assert!(p.validate().is_ok());
        assert!(
            CollisionPolicy {
                page_cells: 33,
                ..p
            }
            .validate()
            .is_err()
        );
        assert!(CollisionPolicy { texel_m: 0.0, ..p }.validate().is_err());
        let radius = 109_081.776_801_130_12;
        let level = p.physics_level(radius);
        assert!(tile_texel_m(radius, level, 32) <= 1.0);
        assert!(tile_texel_m(radius, level - 1, 32) > 1.0);
    }

    #[test]
    fn pages_cover_their_directions_and_sample_bilinearly() {
        let direction = DVec3::new(0.3, -0.2, 0.93).normalize();
        let (address, st) = page_at(direction, 12).unwrap();
        let chart = super::super::select::chart(address);
        assert!((chart.direction(st) - direction).length() < 1e-12);
        let cells = 4;
        let side = (cells + 1) as usize;
        // Height = 10 * s + 20 * t is reproduced exactly by bilinear sampling.
        let heights = (0..side * side)
            .map(|k| (10.0 * (k % side) as f64 / 4.0 + 20.0 * (k / side) as f64 / 4.0) as f32)
            .collect();
        let page = ColliderPage::new(cells, heights, vec![[0.0, 1.0, 0.0]; side * side]).unwrap();
        let sample = page.sample([0.3, 0.7]);
        assert!((sample.height_m - (3.0 + 14.0)).abs() < 1e-5);
        assert_eq!(sample.normal, DVec3::Y);
        assert!(
            ColliderPage::new(
                cells,
                vec![f32::NAN; side * side],
                vec![[0.0; 3]; side * side]
            )
            .is_none()
        );
    }

    #[test]
    fn misses_are_pending_then_ready_and_far_field_is_conservative() {
        let body = super::super::tests::test_body_id();
        let p = policy();
        let mut state = BodyCollision::new(BodyColliders::new(10, 1, 500.0));
        let direction = DVec3::new(0.1, 0.2, 0.97).normalize();
        let mut view = ColliderView {
            bodies: HashMap::from([(body, Arc::clone(&state.colliders))]),
            misses: Vec::new(),
        };
        assert_eq!(view.height_at(body, direction), QueryResult::Pending);
        // Unmeasured far field falls back to the global bound.
        assert_eq!(
            view.far_field_bounds_m(body, direction),
            Some([-500.0, 500.0])
        );
        let misses = view.take_misses();
        assert_eq!(misses.len(), 1);
        state.want(misses[0].1, 1);
        let scheduled = state.schedule(1, 4);
        assert_eq!(scheduled.len(), 1);
        // In flight: not requested again until the retry horizon.
        assert!(state.schedule(2, 4).is_empty());
        state.receive(scheduled[0], flat_page(p.page_cells, 42.0), 4, p.max_pages);
        let (far, _) = page_at(direction, 1).unwrap();
        state.far_field(far, [[-30.0, 80.0]; 16]);
        view.refresh(ColliderView {
            bodies: HashMap::from([(body, Arc::clone(&state.colliders))]),
            misses: Vec::new(),
        });
        let QueryResult::Ready(height) = view.height_at(body, direction) else {
            panic!("page resident");
        };
        assert_eq!(height.height_m, 42.0);
        let bounds = view.far_field_bounds_m(body, direction).unwrap();
        assert!(bounds[0] <= height.height_m && height.height_m <= bounds[1]);
        assert!(view.take_misses().is_empty());
    }

    #[test]
    fn prefetch_covers_the_observer_and_eviction_keeps_recent_pages() {
        let radius = 109_081.776_801_130_12;
        let p = policy();
        let level = p.physics_level(radius);
        let mut state = BodyCollision::new(BodyColliders::new(level, 1, 500.0));
        let up = DVec3::new(0.2, 0.1, 0.97).normalize();
        state.prefetch(up * (radius + 5.0), radius, &p, 1);
        let wanted: Vec<_> = state.schedule(1, 64);
        assert!(wanted.len() >= 9, "{}", wanted.len());
        assert!(wanted.contains(&page_at(up, level).unwrap().0));
        // A query miss is scheduled ahead of a full prefetch set.
        let mut busy = BodyCollision::new(BodyColliders::new(level, 1, 500.0));
        busy.prefetch(up * (radius + 5.0), radius, &p, 2);
        let elsewhere = DVec3::new(-0.6, 0.3, 0.74).normalize();
        busy.want_urgent(elsewhere, 1);
        assert_eq!(
            busy.schedule(2, 1),
            vec![page_at(elsewhere, level).unwrap().0]
        );
        // Far above the prefetch altitude nothing new is wanted.
        let mut high = BodyCollision::new(BodyColliders::new(level, 1, 500.0));
        high.prefetch(up * (radius + 5000.0), radius, &p, 1);
        assert!(high.schedule(1, 64).is_empty());
        for (k, address) in wanted.iter().enumerate() {
            state.receive(
                *address,
                flat_page(p.page_cells, 0.0),
                10 + k as u64,
                p.max_pages,
            );
        }
        assert_eq!(state.colliders.page_count(), p.max_pages);
        let newest = wanted.last().unwrap();
        assert!(state.colliders.pages.contains_key(newest));
        let _ = CubeFace::ALL;
    }
}
