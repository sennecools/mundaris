//! Tier A macro erosion (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6.5.7; M2 design
//! §1), CPU oracle of the GPU `erode_*` passes.
//!
//! Stream power on multiple flow directions (Freeman): per iteration
//! `Δh = U − K(1 − 0.8·hardness)·Q^m·S^n + deposition + thermal`, where `Q` is
//! the precipitation-weighted drainage area (km²) and `S` the MFD slope.
//! Every pass gathers from its neighbours only (no atomics), so the GPU result
//! is deterministic:
//! - route: `D = Σ_k s_k^p` over downhill slopes, `S = Σ s_k^(p+1) / D`;
//! - accumulate: one Jacobi step `Q_i = P_i·a_i + Σ_j Q_j·s_ji^p / D_j` and
//!   the same for the sediment flux; ocean texels are sinks;
//! - update: incision clamped so a texel never drops below its lowest
//!   neighbour (unconditionally stable), deposition of sediment above the
//!   transport capacity (pits fill to their spill point), thermal talus as a
//!   symmetric pair flux (mass conserving), uplift.
//!
//! Jacobi moves water one texel per iteration, so the bake runs a coarse to
//! fine cascade (by default `n/4 × 200 → n/2 × 100 → n × 60` iterations): each
//! level starts from the box-downsampled relief plus the bicubically upsampled
//! eroded difference of the level above, and the upsampled discharge.
//! Neighbours cross cube faces through the integer face-edge table
//! [`FACE_EDGES`], shared with the GPU.
use super::{area_weight, par_fill, texel_uv};
use crate::terrain::{
    archetype::PlanetParams,
    world_map::{CubeMap, texel_direction},
};
use glam::DVec3;

/// Cube face-edge adjacency (`CubeFace::ALL` order): entry `4·face + edge`
/// for edges `0: i < 0`, `1: i ≥ n`, `2: j < 0`, `3: j ≥ n` is `[neighbour
/// face, entered edge (same numbering), 1 if the along-edge coordinate
/// reverses]`. Mirrored by `renderer::tier_a::FACE_EDGES`.
pub const FACE_EDGES: [[u32; 3]; 24] = [
    [4, 1, 0],
    [5, 0, 0],
    [3, 1, 1],
    [2, 1, 0],
    [5, 1, 0],
    [4, 0, 0],
    [3, 0, 0],
    [2, 0, 1],
    [1, 3, 1],
    [0, 3, 0],
    [4, 3, 0],
    [5, 3, 1],
    [1, 2, 0],
    [0, 2, 1],
    [5, 2, 1],
    [4, 2, 0],
    [1, 1, 0],
    [0, 0, 0],
    [3, 3, 0],
    [2, 2, 0],
    [0, 1, 0],
    [1, 0, 0],
    [3, 2, 1],
    [2, 3, 1],
];

/// Neighbour steps `(di, dj)` in a fixed order (edges first, then diagonals).
pub const NEIGHBOUR_STEPS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (-1, 1),
    (1, -1),
    (-1, -1),
];

/// Clearance kept above the lowest neighbour by incision (m).
pub const INCISION_CLEARANCE_M: f64 = 0.01;
/// Height above the spill point to which deposition fills a pit (m).
pub const PIT_FILL_M: f64 = 0.1;

/// Texel reached from `(face, i, j)` by the step `(di, dj)` (each −1, 0 or 1)
/// on `n`-cell faces. Crossing one face edge goes through [`FACE_EDGES`]; the
/// diagonal missing at a cube corner returns the texel itself (a self
/// neighbour has zero slope and weight, so the relation stays symmetric).
pub fn face_step(face: u32, i: i32, j: i32, di: i32, dj: i32, n: i32) -> (u32, i32, i32) {
    let (a, b) = (i + di, j + dj);
    let inside = |c: i32| (0..n).contains(&c);
    match (inside(a), inside(b)) {
        (true, true) => (face, a, b),
        (false, false) => (face, i, j),
        (false, true) | (true, false) => {
            let (edge, along) = if !inside(a) {
                (if a < 0 { 0 } else { 1 }, b)
            } else {
                (if b < 0 { 2 } else { 3 }, a)
            };
            let [g, entered, flip] = FACE_EDGES[(4 * face + edge) as usize];
            let t = if flip == 1 { n - 1 - along } else { along };
            match entered {
                0 => (g, 0, t),
                1 => (g, n - 1, t),
                2 => (g, t, 0),
                _ => (g, t, n - 1),
            }
        }
    }
}

/// Texel area (km²) at chart position `(u, v)`: the gnomonic solid angle of a
/// `2/n` square times `R²`.
pub fn texel_area_km2(u: f64, v: f64, n: usize, radius_m: f64) -> f64 {
    let side = 2.0 * radius_m / n as f64;
    side * side / (1.0 + u * u + v * v).powf(1.5) * 1.0e-6
}

/// `1 / ln(1 + body area in km²)`: `flow = ln(1 + Q)` times this is in 0..1.
pub fn flow_normaliser(radius_m: f64) -> f64 {
    let area_km2 = 4.0 * std::f64::consts::PI * radius_m * radius_m * 1.0e-6;
    1.0 / (1.0 + area_km2).ln()
}

/// Neighbour graph of one level.
pub struct Level {
    pub n: usize,
    pub directions: Vec<DVec3>,
    pub areas: Vec<f64>,
    pub neighbours: Vec<[u32; 8]>,
    /// Distances (m) to the neighbours; 0 for a self neighbour.
    pub distances: Vec<[f64; 8]>,
}

impl Level {
    pub fn new(n: usize, radius_m: f64) -> Self {
        let texels = 6 * n * n;
        let directions: Vec<DVec3> = (0..texels)
            .map(|k| texel_direction(k / (n * n), k % n, (k / n) % n, n))
            .collect();
        let areas = (0..texels)
            .map(|k| {
                let (u, v) = texel_uv(k % n, (k / n) % n, n);
                texel_area_km2(u, v, n, radius_m)
            })
            .collect();
        let mut neighbours = vec![[0u32; 8]; texels];
        let mut distances = vec![[0.0f64; 8]; texels];
        for k in 0..texels {
            let (face, i, j) = ((k / (n * n)) as u32, (k % n) as i32, ((k / n) % n) as i32);
            for (slot, (di, dj)) in NEIGHBOUR_STEPS.iter().enumerate() {
                let (g, a, b) = face_step(face, i, j, *di, *dj, n as i32);
                let m = (g as usize * n + b as usize) * n + a as usize;
                neighbours[k][slot] = m as u32;
                distances[k][slot] = if m == k {
                    0.0
                } else {
                    (directions[k] - directions[m]).length() * radius_m
                };
            }
        }
        Self {
            n,
            directions,
            areas,
            neighbours,
            distances,
        }
    }
}

/// 2×2 box average within each face (as the field mips).
pub fn box_down(map: &CubeMap<f32>) -> CubeMap<f32> {
    let n = map.n() / 2;
    let mut out = CubeMap::new(n, 0.0f32);
    for face in 0..6 {
        for j in 0..n {
            for i in 0..n {
                let at = |x: usize, y: usize| f64::from(map.data()[map.index(face, x, y)]);
                let sum = at(2 * i, 2 * j)
                    + at(2 * i + 1, 2 * j)
                    + at(2 * i, 2 * j + 1)
                    + at(2 * i + 1, 2 * j + 1);
                let k = out.index(face, i, j);
                out.data_mut()[k] = (0.25 * sum) as f32;
            }
        }
    }
    out
}

/// Inputs of the erosion stage (pre-erosion fields at full resolution).
pub struct ErosionInput<'a> {
    pub face_cells: usize,
    pub radius_m: f64,
    pub params: &'a PlanetParams,
    pub elevation: &'a CubeMap<f32>,
    pub uplift: &'a CubeMap<f32>,
    pub hardness: &'a CubeMap<f32>,
    /// Moisture 0..1, the rain weight of the drainage area.
    pub precipitation: &'a CubeMap<f32>,
}

/// Volumes (km³) moved by the erosion stage, summed over the cascade.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ErosionStats {
    pub eroded_km3: f64,
    pub deposited_km3: f64,
    pub uplifted_km3: f64,
    /// Largest per-level `|ΔV − (U − E + D)| / max(E, U)`: thermal moves and
    /// deposition limits conserve mass, so this is rounding only.
    pub balance_error: f64,
    pub iterations: u32,
}

/// Eroded fields at full resolution.
#[derive(Debug, Clone, PartialEq)]
pub struct ErosionOutput {
    pub elevation: CubeMap<f32>,
    /// Precipitation-weighted drainage area (km²).
    pub discharge: CubeMap<f32>,
    /// Accumulated deposit thickness (m).
    pub deposit: CubeMap<f32>,
    pub stats: ErosionStats,
}

/// Erosion constants of one bake, in the units the passes use.
#[derive(Debug, Clone, Copy)]
pub struct ErosionConstants {
    pub strength: f64,
    /// Uplift per iteration where tectonic uplift is 1 (m).
    pub uplift_m: f64,
    pub tan_talus: f64,
    pub deposition: f64,
    pub capacity: f64,
    pub m: f64,
    pub n: f64,
    pub p: f64,
    pub thermal: f64,
}

impl ErosionConstants {
    pub fn new(params: &PlanetParams) -> Self {
        let iterations: u32 = params
            .erosion_cascade
            .iter()
            .map(|(_, iterations)| *iterations)
            .sum();
        Self {
            strength: params.erosion_strength,
            uplift_m: params.erosion_uplift_m / f64::from(iterations.max(1)),
            tan_talus: params.talus_deg.to_radians().tan(),
            deposition: params.deposition,
            capacity: params.erosion_capacity,
            m: params.erosion_area_exponent,
            n: params.erosion_slope_exponent,
            p: params.erosion_flow_exponent,
            thermal: params.thermal_rate,
        }
    }
}

/// Erosion state of one level. `w` is the water surface: the relief with
/// depressions filled to their spill point plus a [`PIT_FILL_M`] gradient per
/// texel, on which water and sediment are routed so they cross lakes to the
/// sea. It is recomputed by [`refill`] at each level start and every
/// [`REFILL_INTERVAL`] iterations, and follows the relief in between with one
/// fill step per iteration.
struct State {
    h: Vec<f64>,
    w: Vec<f64>,
    q: Vec<f64>,
    qs_out: Vec<f64>,
    deposit: Vec<f64>,
}

/// Water surface of land with no outlet found yet.
pub const UNFILLED_M: f64 = 1.0e7;
/// The fill pyramid stops above this face size (or at the level itself).
pub const FILL_PYRAMID_MIN_CELLS: usize = 16;
/// Fill passes of the pyramid's coarsest level per face cell (from
/// [`UNFILLED_M`]: one texel of drainage path per pass).
pub const FILL_SEED_PASSES_PER_CELL: usize = 4;
/// Fill passes per finer pyramid level (from its parent's upper bound).
pub const FILL_PASSES: usize = 8;
/// Erosion iterations between water-surface refills.
pub const REFILL_INTERVAL: u32 = 20;

fn map_of(n: usize, values: &[f64]) -> CubeMap<f32> {
    let mut map = CubeMap::new(n, 0.0f32);
    for (slot, v) in map.data_mut().iter_mut().zip(values) {
        *slot = *v as f32;
    }
    map
}

fn values(map: &CubeMap<f32>) -> Vec<f64> {
    map.data().iter().map(|v| f64::from(*v)).collect()
}

/// Sample `coarse` at every texel direction of `level` (bicubic or bilinear).
fn upsample(level: &Level, coarse: &CubeMap<f32>, cubic: bool) -> Vec<f64> {
    let mut out = vec![0.0; level.directions.len()];
    par_fill(&mut out, |k| {
        let d = level.directions[k];
        if cubic {
            coarse.bicubic(d)
        } else {
            coarse.bilinear(d).max(0.0)
        }
    });
    out
}

/// One fill step (Planchon–Darboux, Jacobi) of the water surface at texel
/// `k` over relief `h` from candidate surfaces `s`:
/// `max(h, min(s_k + slack, min_m s_m + step))`; the ocean (`h < 0`) is its
/// own surface. From above (`s ≥` the filled surface) it converges by one
/// texel of drainage path per step; `slack` lets new depressions fill.
fn fill_step(level: &Level, h: &[f64], s: &[f64], slack: f64, step: f64, k: usize) -> f64 {
    if h[k] < 0.0 {
        return h[k];
    }
    let mut lowest = s[k] + slack;
    for slot in 0..8 {
        let m = level.neighbours[k][slot] as usize;
        if m != k {
            lowest = lowest.min(s[m] + step);
        }
    }
    h[k].max(lowest)
}

/// Index of the parent texel (half resolution, same face) of texel `k`.
pub fn parent_texel(k: usize, n: usize) -> usize {
    let face = k / (n * n);
    let (i, j) = (k % n, (k / n) % n);
    let half = n / 2;
    (face * half + j / 2) * half + i / 2
}

/// 2×2 maximum within each face.
fn max_down(h: &[f64], n: usize) -> Vec<f64> {
    let half = n / 2;
    let mut out = vec![f64::NEG_INFINITY; 6 * half * half];
    for (k, v) in h.iter().enumerate() {
        let p = parent_texel(k, n);
        out[p] = out[p].max(*v);
    }
    out
}

/// The filled water surface of relief `h` on `pyramid[0]`, exact up to the
/// fill gradient: a max-pooled pyramid keeps every barrier, so the filled
/// surface of a coarser level (with the fill step scaled by the level ratio)
/// bounds the finer one from above; its coarsest level fills from
/// [`UNFILLED_M`], and each finer level starts from its parent's surface and
/// converges in [`FILL_PASSES`] steps.
fn refill(pyramid: &[Level], h: &[f64]) -> Vec<f64> {
    let mut heights = vec![h.to_vec()];
    for k in 1..pyramid.len() {
        let next = max_down(&heights[k - 1], pyramid[k - 1].n);
        heights.push(next);
    }
    let top = pyramid.len() - 1;
    let mut w: Vec<f64> = heights[top]
        .iter()
        .map(|v| if *v < 0.0 { *v } else { UNFILLED_M })
        .collect();
    for k in (0..=top).rev() {
        let level = &pyramid[k];
        let hk = &heights[k];
        if k < top {
            let parent = &w;
            w = (0..hk.len())
                .map(|t| {
                    if hk[t] < 0.0 {
                        hk[t]
                    } else {
                        hk[t].max(parent[parent_texel(t, level.n)])
                    }
                })
                .collect();
        }
        let passes = if k == top {
            FILL_SEED_PASSES_PER_CELL * level.n
        } else {
            FILL_PASSES
        };
        let step = PIT_FILL_M * (1u64 << k) as f64;
        for _ in 0..passes {
            let mut next = vec![0.0; w.len()];
            par_fill(&mut next, |t| fill_step(level, hk, &w, 0.0, step, t));
            w = next;
        }
    }
    w
}

/// Graphs of a level and its max-pooled fill pyramid (halving to
/// [`FILL_PYRAMID_MIN_CELLS`]).
fn fill_pyramid(n: usize, radius_m: f64) -> Vec<Level> {
    let mut pyramid = vec![Level::new(n, radius_m)];
    let mut cells = n;
    while cells / 2 >= FILL_PYRAMID_MIN_CELLS {
        cells /= 2;
        pyramid.push(Level::new(cells, radius_m));
    }
    pyramid
}

/// One erosion iteration on `level`; returns the volumes (eroded, deposited,
/// uplifted) in km²·m.
fn iterate(
    level: &Level,
    c: &ErosionConstants,
    uplift: &[f64],
    hardness: &[f64],
    precipitation: &[f64],
    state: &mut State,
) -> (f64, f64, f64) {
    let texels = level.directions.len();
    let (h, w) = (&state.h, &state.w);
    // Route on the water surface: Σ s^p and the MFD slope.
    let mut route = vec![(0.0f64, 0.0f64); texels];
    par_fill(&mut route, |k| {
        let (mut total, mut weighted) = (0.0, 0.0);
        for slot in 0..8 {
            let m = level.neighbours[k][slot] as usize;
            let distance = level.distances[k][slot];
            if m == k || distance <= 0.0 {
                continue;
            }
            let s = (w[k] - w[m]) / distance;
            if s > 0.0 {
                let sp = s.powf(c.p);
                total += sp;
                weighted += sp * s;
            }
        }
        (total, if total > 0.0 { weighted / total } else { 0.0 })
    });
    // Accumulate: one Jacobi step of discharge and sediment flux; the ocean
    // (w < 0) is a sink.
    let mut flux = vec![(0.0f64, 0.0f64); texels];
    par_fill(&mut flux, |k| {
        let mut q = precipitation[k] * level.areas[k];
        let mut qs = 0.0;
        for slot in 0..8 {
            let m = level.neighbours[k][slot] as usize;
            let distance = level.distances[k][slot];
            if m == k || distance <= 0.0 || w[m] < 0.0 || route[m].0 <= 0.0 {
                continue;
            }
            let s = (w[m] - w[k]) / distance;
            if s > 0.0 {
                let share = s.powf(c.p) / route[m].0;
                q += state.q[m] * share;
                qs += state.qs_out[m] * share;
            }
        }
        (q, qs)
    });
    // Update: incision, deposition, thermal talus, uplift.
    let mut next = vec![[0.0f64; 6]; texels];
    par_fill(&mut next, |k| {
        let hk = h[k];
        let area = level.areas[k];
        let (mut low, mut up, mut thermal) = (f64::INFINITY, f64::INFINITY, 0.0);
        for slot in 0..8 {
            let m = level.neighbours[k][slot] as usize;
            let distance = level.distances[k][slot];
            if m == k || distance <= 0.0 {
                continue;
            }
            let hm = h[m];
            low = low.min(hm);
            if hm > hk {
                up = up.min(hm);
            }
            let talus = c.tan_talus * distance;
            let share = c.thermal * area.min(level.areas[m]) / area;
            let drop = hk - hm;
            if drop > talus {
                thermal -= share * (drop - talus);
            } else if -drop > talus {
                thermal += share * (-drop - talus);
            }
        }
        let (q, qs_in) = flux[k];
        let (erode, deposit, qs_out) = if hk >= 0.0 {
            let slope = route[k].1;
            // Stream power per unit K; transport capacity is that power
            // times the discharge (a sediment flux, independent of rock).
            let power = if q > 0.0 && slope > 0.0 {
                q.powf(c.m) * slope.powf(c.n)
            } else {
                0.0
            };
            let rate = c.strength * (1.0 - 0.8 * hardness[k]) * power;
            let erode = rate.min((hk - low - INCISION_CLEARANCE_M).max(0.0));
            let capacity = c.capacity * c.strength * power * q;
            // Lakes fill up to their surface; elsewhere deposits stay below
            // half the step to the lowest higher neighbour (no new dams).
            let limit = (w[k] - hk).max(if up.is_finite() { 0.5 * (up - hk) } else { 0.0 });
            let deposit = (c.deposition * (qs_in - capacity).max(0.0) / area).min(limit);
            (
                erode,
                deposit,
                (qs_in - deposit * area).max(0.0) + erode * area,
            )
        } else {
            let deposit = (c.deposition * qs_in / area).min((-hk - PIT_FILL_M).max(0.0));
            (0.0, deposit, 0.0)
        };
        let rise = c.uplift_m * uplift[k];
        [
            hk - erode + deposit + thermal + rise,
            q,
            qs_out,
            deposit,
            erode * area,
            rise * area,
        ]
    });
    // Candidate water surfaces move with their beds (lake depth kept), so
    // uplift does not leave lakes behind their rising outlets; one fill step
    // on the new relief follows.
    let surface: Vec<f64> = (0..texels)
        .map(|k| next[k][0] + (state.w[k] - state.h[k]).max(0.0))
        .collect();
    let (mut eroded, mut deposited, mut uplifted) = (0.0, 0.0, 0.0);
    for (k, v) in next.into_iter().enumerate() {
        state.h[k] = v[0];
        state.q[k] = v[1];
        state.qs_out[k] = v[2];
        state.deposit[k] += v[3];
        eroded += v[4];
        deposited += v[3] * level.areas[k];
        uplifted += v[5];
    }
    let slack = c.uplift_m + PIT_FILL_M;
    par_fill(&mut state.w, |k| {
        fill_step(level, &state.h, &surface, slack, PIT_FILL_M, k)
    });
    (eroded, deposited, uplifted)
}

/// Run the erosion cascade.
pub fn erode(input: &ErosionInput) -> ErosionOutput {
    let n = input.face_cells;
    let c = ErosionConstants::new(input.params);
    let active = active_levels(&input.params.erosion_cascade, n);
    let down = |map: &CubeMap<f32>, divisor: u32| {
        let mut out = map.clone();
        let mut d = 1;
        while d < divisor {
            out = box_down(&out);
            d *= 2;
        }
        out
    };
    let mut stats = ErosionStats::default();
    let mut previous: Option<(CubeMap<f32>, State, usize)> = None;
    let last = active.len().saturating_sub(1);
    for (index, (divisor, iterations)) in active.iter().enumerate() {
        let pyramid = fill_pyramid(n / *divisor as usize, input.radius_m);
        let level = &pyramid[0];
        let base = down(input.elevation, *divisor);
        let uplift = values(&down(input.uplift, *divisor));
        let hardness = values(&down(input.hardness, *divisor));
        let precipitation = values(&down(input.precipitation, *divisor));
        let texels = level.directions.len();
        let mut state = match previous.take() {
            None => State {
                h: values(&base),
                w: Vec::new(),
                q: vec![0.0; texels],
                qs_out: vec![0.0; texels],
                deposit: vec![0.0; texels],
            },
            Some((coarse_base, coarse, cn)) => {
                let difference: Vec<f64> = coarse
                    .h
                    .iter()
                    .zip(coarse_base.data())
                    .map(|(h, b)| h - f64::from(*b))
                    .collect();
                let lift = upsample(level, &map_of(cn, &difference), true);
                State {
                    h: values(&base)
                        .iter()
                        .zip(&lift)
                        .map(|(b, d)| b + d)
                        .collect(),
                    w: Vec::new(),
                    q: upsample(level, &map_of(cn, &coarse.q), false),
                    qs_out: upsample(level, &map_of(cn, &coarse.qs_out), false),
                    deposit: upsample(level, &map_of(cn, &coarse.deposit), false),
                }
            }
        };
        state.w = refill(&pyramid, &state.h);
        let start: f64 = state.h.iter().zip(&level.areas).map(|(h, a)| h * a).sum();
        let (mut eroded, mut deposited, mut uplifted) = (0.0, 0.0, 0.0);
        for iteration in 0..*iterations {
            let (e, d, u) = iterate(level, &c, &uplift, &hardness, &precipitation, &mut state);
            eroded += e;
            deposited += d;
            uplifted += u;
            if (iteration + 1) % REFILL_INTERVAL == 0
                || (index == last && iteration + 1 == *iterations)
            {
                state.w = refill(&pyramid, &state.h);
            }
        }
        let end: f64 = state.h.iter().zip(&level.areas).map(|(h, a)| h * a).sum();
        let error = ((end - start) - (uplifted - eroded + deposited)).abs()
            / eroded.max(uplifted).max(1e-9);
        stats.balance_error = stats.balance_error.max(error);
        stats.eroded_km3 += eroded * 1e-3;
        stats.deposited_km3 += deposited * 1e-3;
        stats.uplifted_km3 += uplifted * 1e-3;
        stats.iterations += iterations;
        previous = Some((base, state, level.n));
    }
    let Some((base, state, level_n)) = previous else {
        return ErosionOutput {
            elevation: input.elevation.clone(),
            discharge: CubeMap::new(n, 0.0f32),
            deposit: CubeMap::new(n, 0.0f32),
            stats,
        };
    };
    // Lakes are filled to their spill point (sediment and lake beds; M3
    // decides open water): the result is the water surface, and the fill
    // counts as deposit.
    let filled_deposit: Vec<f64> = state
        .deposit
        .iter()
        .zip(state.w.iter().zip(&state.h))
        .map(|(d, (w, h))| d + (w - h).max(0.0))
        .collect();
    if level_n == n {
        return ErosionOutput {
            elevation: map_of(n, &state.w),
            discharge: map_of(n, &state.q),
            deposit: map_of(n, &filled_deposit),
            stats,
        };
    }
    // The finest level is coarser than the bake: upsample its result.
    let full = Level::new(n, input.radius_m);
    let difference: Vec<f64> = state
        .w
        .iter()
        .zip(base.data())
        .map(|(w, b)| w - f64::from(*b))
        .collect();
    let lift = upsample(&full, &map_of(level_n, &difference), true);
    let elevation: Vec<f64> = values(input.elevation)
        .iter()
        .zip(&lift)
        .map(|(b, d)| b + d)
        .collect();
    ErosionOutput {
        elevation: map_of(n, &elevation),
        discharge: map_of(n, &upsample(&full, &map_of(level_n, &state.q), false)),
        deposit: map_of(
            n,
            &upsample(&full, &map_of(level_n, &filled_deposit), false),
        ),
        stats,
    }
}

/// Cascade levels that run at face size `n`: iterations > 0 and at least 4
/// cells per face.
pub fn active_levels(cascade: &[(u32, u32); 3], n: usize) -> Vec<(u32, u32)> {
    cascade
        .iter()
        .copied()
        .filter(|(divisor, iterations)| *iterations > 0 && n / *divisor as usize >= 4)
        .collect()
}

/// Drainage of an elevation map along steepest descent (8 neighbours).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrainageStats {
    /// Area-weighted share of land (h ≥ 0) whose steepest-descent path
    /// reaches the ocean (h < 0).
    pub draining_fraction: f64,
    /// Land texels with no lower neighbour.
    pub land_pits: usize,
    pub land_texels: usize,
}

/// Steepest-descent drainage of `elevation` (face cells from the map).
pub fn drainage(elevation: &CubeMap<f32>, radius_m: f64) -> DrainageStats {
    let n = elevation.n();
    let level = Level::new(n, radius_m);
    let h = elevation.data();
    let texels = 6 * n * n;
    // Receiver: steepest lower neighbour, or none.
    let receiver: Vec<Option<usize>> = (0..texels)
        .map(|k| {
            let mut best = (0.0, None);
            for slot in 0..8 {
                let m = level.neighbours[k][slot] as usize;
                let distance = level.distances[k][slot];
                if m == k || distance <= 0.0 {
                    continue;
                }
                let s = (f64::from(h[k]) - f64::from(h[m])) / distance;
                if s > best.0 {
                    best = (s, Some(m));
                }
            }
            best.1
        })
        .collect();
    // 0 unknown, 1 drains to the ocean, 2 ends in a pit.
    let mut fate = vec![0u8; texels];
    let mut path = Vec::new();
    for start in 0..texels {
        let mut k = start;
        path.clear();
        let result = loop {
            if fate[k] != 0 {
                break fate[k];
            }
            if h[k] < 0.0 {
                break 1;
            }
            path.push(k);
            match receiver[k] {
                Some(m) => k = m,
                None => break 2,
            }
        };
        for &k in &path {
            fate[k] = result;
        }
        if h[start] < 0.0 {
            fate[start] = 1;
        }
    }
    let (mut draining, mut land) = (0.0, 0.0);
    let (mut pits, mut land_texels) = (0, 0);
    for k in 0..texels {
        if h[k] < 0.0 {
            continue;
        }
        let (u, v) = texel_uv(k % n, (k / n) % n, n);
        let w = f64::from(area_weight(u, v));
        land += w;
        land_texels += 1;
        if fate[k] == 1 {
            draining += w;
        }
        if receiver[k].is_none() {
            pits += 1;
        }
    }
    DrainageStats {
        draining_fraction: draining / land.max(1e-300),
        land_pits: pits,
        land_texels,
    }
}

/// Least-squares fit of `ln(S·c) = a + b·ln Q` over channel texels (land,
/// discharge above `min_discharge_km2`, steepest drop above twice the lake
/// fill gradient, so filled lakes do not count as channels) where
/// `scale(k)` gives `c` (`None` skips the texel).
/// Steady-state stream power `U = K(1 − 0.8·hardness)·Q^m·S^n` gives
/// `b = −m/n` with `c = (1 − 0.8·hardness)/U`.
pub fn slope_area_fit(
    elevation: &CubeMap<f32>,
    discharge: &CubeMap<f32>,
    radius_m: f64,
    min_discharge_km2: f64,
    scale: impl Fn(usize) -> Option<f64>,
) -> SlopeArea {
    let n = elevation.n();
    let level = Level::new(n, radius_m);
    let h = elevation.data();
    let mut points = Vec::new();
    for k in 0..6 * n * n {
        let q = f64::from(discharge.data()[k]);
        let Some(c) = scale(k).filter(|_| h[k] >= 0.0 && q >= min_discharge_km2) else {
            continue;
        };
        let (slope, drop) = (0..8)
            .filter(|&slot| level.distances[k][slot] > 0.0)
            .map(|slot| {
                let m = level.neighbours[k][slot] as usize;
                let drop = f64::from(h[k]) - f64::from(h[m]);
                (drop / level.distances[k][slot], drop)
            })
            .fold((0.0f64, 0.0f64), |a, b| if b.0 > a.0 { b } else { a });
        if drop > 2.0 * PIT_FILL_M {
            points.push((q.ln(), (slope * c).ln()));
        }
    }
    // Log-binned means (bins of 0.25 in ln Q with at least 10 texels), as
    // slope–area plots are read: the fit measures the trend, the raw R² the
    // scatter of a transient landscape around it.
    let raw = least_squares(&points);
    let mut bins: Vec<(i64, f64, usize)> = Vec::new();
    for (x, y) in &points {
        let bin = (x / 0.25).floor() as i64;
        match bins.iter_mut().find(|b| b.0 == bin) {
            Some(b) => {
                b.1 += y;
                b.2 += 1;
            }
            None => bins.push((bin, *y, 1)),
        }
    }
    let means: Vec<(f64, f64)> = bins
        .iter()
        .filter(|b| b.2 >= 10)
        .map(|b| (b.0 as f64 * 0.25 + 0.125, b.1 / b.2 as f64))
        .collect();
    let binned = least_squares(&means);
    SlopeArea {
        exponent: binned.0,
        r2: binned.1,
        raw_exponent: raw.0,
        raw_r2: raw.1,
        texels: points.len(),
        bins: means.len(),
    }
}

/// Slope–area fit of [`slope_area_fit`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlopeArea {
    /// Exponent and R² of the fit through log-binned means.
    pub exponent: f64,
    pub r2: f64,
    /// Exponent and R² of the fit through every channel texel.
    pub raw_exponent: f64,
    pub raw_r2: f64,
    pub texels: usize,
    pub bins: usize,
}

/// Least squares `y = a + b·x`: `(b, R²)`.
fn least_squares(points: &[(f64, f64)]) -> (f64, f64) {
    let count = points.len() as f64;
    if points.len() < 3 {
        return (0.0, 0.0);
    }
    let (mx, my) = points
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x / count, b + y / count));
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in points {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    (sxy / sxx.max(1e-300), sxy * sxy / (sxx * syy).max(1e-300))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{
        tier_a::{bake_full, tests::inputs},
        world_map::{locate, neighbours},
    };

    #[test]
    fn face_edge_table_matches_the_cube_geometry() {
        for n in [4usize, 8, 16] {
            for k in 0..6 * n * n {
                let face = (k / (n * n)) as u32;
                let (i, j) = ((k % n) as i32, ((k / n) % n) as i32);
                // Edge steps agree with the geometric neighbours.
                let geometric = neighbours(k, n);
                for (slot, step) in NEIGHBOUR_STEPS[..4].iter().enumerate() {
                    let (g, a, b) = face_step(face, i, j, step.0, step.1, n as i32);
                    let m = (g as usize * n + b as usize) * n + a as usize;
                    assert_eq!(m, geometric[slot], "n {n} texel {k} step {step:?}");
                }
                // The relation is symmetric with multiplicity (what the
                // gather-only routing needs to conserve water), and only the
                // missing corner diagonal maps to the texel itself.
                let list = |face: u32, i: i32, j: i32| -> Vec<usize> {
                    NEIGHBOUR_STEPS
                        .iter()
                        .map(|s| {
                            let (g, a, b) = face_step(face, i, j, s.0, s.1, n as i32);
                            (g as usize * n + b as usize) * n + a as usize
                        })
                        .collect()
                };
                let mine = list(face, i, j);
                assert!(mine.iter().filter(|m| **m == k).count() <= 1);
                for &m in &mine {
                    if m == k {
                        continue;
                    }
                    let theirs = list((m / (n * n)) as u32, (m % n) as i32, ((m / n) % n) as i32);
                    let forward = mine.iter().filter(|x| **x == m).count();
                    let back = theirs.iter().filter(|x| **x == k).count();
                    assert_eq!(forward, back, "n {n} texel {k} neighbour {m}");
                }
            }
        }
        // Diagonals across an edge land next to the geometric position.
        let n = 16;
        let (g, a, b) = face_step(0, 0, 5, -1, 1, n);
        let d = texel_direction(g as usize, a as usize, b as usize, n as usize);
        let expected = texel_direction(0, 0, 6, n as usize);
        assert!(
            d.angle_between(expected) < 2.5 / n as f64,
            "{:?}",
            locate(d)
        );
    }

    #[test]
    fn erosion_drains_the_relief_and_conserves_mass() {
        let n = 64;
        let input = inputs(n, 7);
        let output = bake_full(&input).unwrap();
        let d = &output.diagnostics;
        let stats = d.erosion.unwrap();
        let before = drainage(&d.pre_erosion, input.radius_m);
        let after = drainage(&d.eroded, input.radius_m);
        let fit = slope_area_fit(&d.eroded, &d.discharge, input.radius_m, 200.0, |k| {
            let u = f64::from(d.uplift.data()[k]);
            (u > 0.3).then(|| (1.0 - 0.8 * f64::from(d.hardness.data()[k])) / u)
        });
        println!(
            "n={n}: {stats:?}\n drainage before {before:?}\n after {after:?}\n \
             slope-area {fit:?}"
        );
        assert!(stats.balance_error < 1e-3, "{}", stats.balance_error);
        assert!(stats.eroded_km3 > 0.0 && stats.deposited_km3 > 0.0);
        assert!(after.draining_fraction > before.draining_fraction);
        assert!(
            after.draining_fraction >= 0.85,
            "{}",
            after.draining_fraction
        );
        assert!(after.land_pits * 5 <= before.land_pits.max(1));
        assert!((-0.9..=-0.2).contains(&fit.exponent) && fit.r2 > 0.3);
    }
}
