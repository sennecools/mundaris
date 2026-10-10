//! PROTOTYPE (M3 Water): macro hydrology on the Tier A world map
//! (`docs/ASTRUM_TERRAIN_PIPELINE.md` §7). Runs on a CPU worker on the
//! read-back (or CPU-oracle) elevation and moisture, once per bake:
//!
//! 1. [`flood`]: priority-flood depression filling with an epsilon gradient
//!    (every land texel gets a strictly descending path to the ocean, or to
//!    the lowest texel of an ocean-free body), steepest-descent receivers on
//!    the filled surface, lakes as connected filled regions with a flat level
//!    and an outlet;
//! 2. accumulation of precipitation-weighted drainage area (km², rain from
//!    `moisture`, so wet mountains feed big rivers and desert rivers stay
//!    small), repeated once with endorheic lakes as sinks;
//! 3. [`rivers`]: river graph (vertices with downstream links, a forest
//!    rooted at mouths), Douglas–Peucker simplification, hydraulic geometry,
//!    beds forced strictly monotonic downstream, and a cube-sphere spatial
//!    hash of the segments;
//! 4. [`carve`]: the f64 Tier B carve (`h = min(h, mix(bed, h, profile))`)
//!    and the water surface of channels and lakes.
//!
//! Prototype shortcuts (hardened after the user's look): constants live in
//! [`HydrologyParams::prototype`] instead of the archetype RON; no meanders
//! (§7.4), deltas or waterfalls (§7.7); two valley profiles (V-valley and
//! floodplain) blended by bed slope; no GPU mirror tests yet.
pub mod carve;
pub mod flood;
pub mod rivers;
#[cfg(test)]
mod tests;

use crate::terrain::{tier_a::erosion::Level, world_map::CubeMap};
pub use carve::{CarveSample, RiverHash};
pub use flood::{Fill, Lake};
pub use rivers::{RiverGraph, RiverVertex};

/// Receiver of a sink (ocean texel, or the outlet of an ocean-free body).
pub const NO_RECEIVER: u32 = u32::MAX;

/// Tunables of the hydrology prototype.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HydrologyParams {
    /// Fill gradient added per texel across flats and depressions (m).
    pub epsilon_m: f64,
    /// A filled region is a lake when it is deeper than this somewhere (m).
    pub lake_min_depth_m: f64,
    /// Smallest lake, in texels.
    pub lake_min_texels: usize,
    /// A lake is endorheic when its catchment discharge is below its area
    /// times this ratio (evaporation outweighs inflow).
    pub endorheic_ratio: f64,
    /// Precipitation-weighted drainage area at which a channel starts (km²).
    pub river_min_discharge_km2: f64,
    /// Mean runoff per unit of precipitation-weighted area (m³/s per km²).
    pub runoff_m3s_per_km2: f64,
    /// Hydraulic geometry: `width = a·Q^b`, `depth = c·Q^d` (Q in m³/s).
    pub width_a: f64,
    pub width_b: f64,
    pub depth_c: f64,
    pub depth_d: f64,
    /// Valley incision below the macro surface: this fraction of the local
    /// relief (highest neighbour minus the texel), capped (m).
    pub incision_relief_fraction: f64,
    pub incision_max_m: f64,
    /// Valley side slope (rise over run) that sets the valley half-width
    /// from the incision depth.
    pub valley_side_slope: f64,
    /// Bed drop forced between consecutive vertices (m).
    pub bed_step_m: f64,
    /// Valley half-width as a multiple of the channel width, and its floor
    /// and cap (m).
    pub valley_factor: f64,
    pub valley_min_m: f64,
    pub valley_max_m: f64,
    /// Bed slope (m/m) below which a valley becomes a floodplain.
    pub floodplain_slope: f64,
    /// Douglas–Peucker tolerance in texels.
    pub simplify_texels: f64,
    /// Catmull–Rom points inserted per simplified segment.
    pub curve_points: usize,
}

impl HydrologyParams {
    /// PROTOTYPE: hand-picked values for a Terra body with ~1.3 km texels.
    pub fn prototype() -> Self {
        Self {
            epsilon_m: 1.0e-3,
            lake_min_depth_m: 2.0,
            lake_min_texels: 3,
            endorheic_ratio: 4.0,
            river_min_discharge_km2: 400.0,
            runoff_m3s_per_km2: 0.01,
            width_a: 2.7,
            width_b: 0.5,
            depth_c: 0.3,
            depth_d: 0.4,
            incision_relief_fraction: 0.6,
            incision_max_m: 400.0,
            valley_side_slope: 0.3,
            bed_step_m: 0.05,
            valley_factor: 12.0,
            valley_min_m: 250.0,
            valley_max_m: 2_500.0,
            floodplain_slope: 0.004,
            simplify_texels: 0.5,
            curve_points: 3,
        }
    }
}

/// Inputs: the macro elevation (m, sea level 0) and moisture (0..1) of one
/// body at the same resolution.
pub struct HydrologyInput<'a> {
    pub elevation: &'a CubeMap<f32>,
    pub moisture: &'a CubeMap<f32>,
    pub radius_m: f64,
    pub params: HydrologyParams,
}

/// Result of [`run`].
#[derive(Debug, Clone)]
pub struct Hydrology {
    pub fill: Fill,
    /// Precipitation-weighted drainage area (km²) after endorheic sinks.
    pub discharge: CubeMap<f32>,
    pub rivers: RiverGraph,
    pub hash: RiverHash,
    pub params: HydrologyParams,
    pub radius_m: f64,
}

/// Run every stage.
pub fn run(input: &HydrologyInput) -> Hydrology {
    let n = input.elevation.n();
    assert_eq!(n, input.moisture.n(), "fields share one resolution");
    let level = Level::new(n, input.radius_m);
    let mut fill = flood::fill(input.elevation, &level, &input.params);
    // Rain per texel: moisture times area.
    let rain: Vec<f64> = (0..6 * n * n)
        .map(|k| f64::from(input.moisture.data()[k]).max(0.0) * level.areas[k])
        .collect();
    let first = accumulate(&fill, &rain, &[]);
    flood::classify_endorheic(&mut fill, &first, &input.params);
    let sinks: Vec<u32> = fill
        .lakes
        .iter()
        .filter(|l| l.endorheic)
        .map(|l| l.outlet)
        .collect();
    let discharge_values = if sinks.is_empty() {
        first
    } else {
        accumulate(&fill, &rain, &sinks)
    };
    let mut discharge = CubeMap::new(n, 0.0f32);
    for (o, q) in discharge.data_mut().iter_mut().zip(&discharge_values) {
        *o = *q as f32;
    }
    let rivers = rivers::extract(&fill, &discharge_values, &level, &input.params);
    let hash = RiverHash::build(&rivers, n, input.radius_m, &input.params);
    Hydrology {
        fill,
        discharge,
        rivers,
        hash,
        params: input.params,
        radius_m: input.radius_m,
    }
}

/// Accumulate `rain` down the receivers in descending filled order; texels
/// in `sinks` keep their inflow (endorheic lake outlets).
pub fn accumulate(fill: &Fill, rain: &[f64], sinks: &[u32]) -> Vec<f64> {
    let mut q = rain.to_vec();
    let mut sink = vec![false; q.len()];
    for &k in sinks {
        sink[k as usize] = true;
    }
    for &k in &fill.order {
        let r = fill.receiver[k as usize];
        if r == NO_RECEIVER || sink[k as usize] {
            continue;
        }
        q[r as usize] += q[k as usize];
    }
    q
}

impl Hydrology {
    /// Carve the height `h` (m) at unit direction `d` (see [`carve::carve`]).
    pub fn carve(&self, d: glam::DVec3, h: f64) -> CarveSample {
        carve::carve(&self.rivers, &self.hash, self.radius_m, &self.params, d, h)
    }

    /// Deepest a carve can lower terrain below the macro surface (m): the
    /// bound margin for culling, colliders and camera clearance.
    pub fn carve_depth_bound_m(&self) -> f64 {
        self.rivers
            .vertices
            .iter()
            .filter(|v| v.texel != rivers::INSERTED)
            .map(|v| self.fill.filled[v.texel as usize].max(0.0) - v.bed_m)
            .fold(0.0, f64::max)
            * 1.1
            + 10.0
    }
}
