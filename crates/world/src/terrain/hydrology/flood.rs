//! Priority-flood depression filling (§7.1, Barnes et al. 2014 with an
//! epsilon gradient), steepest-descent receivers and lakes.
use super::{HydrologyParams, NO_RECEIVER};
use crate::terrain::{
    tier_a::erosion::Level,
    world_map::{CubeMap, neighbours},
};
use std::{cmp::Ordering, collections::BinaryHeap};

/// No lake at a texel.
pub const NO_LAKE: u32 = u32::MAX;

/// A filled depression with a flat water level.
#[derive(Debug, Clone, PartialEq)]
pub struct Lake {
    /// Spill level (m): the fill level of the depression.
    pub spill_m: f64,
    /// Water level (m): the spill level, or lower for an endorheic lake.
    pub level_m: f64,
    /// Deepest original elevation inside (m).
    pub bottom_m: f64,
    /// Last lake texel on the path out (its receiver lies outside).
    pub outlet: u32,
    pub texels: usize,
    pub area_km2: f64,
    /// Evaporation outweighs inflow: no outflow, lowered level.
    pub endorheic: bool,
    /// Inflow over evaporation capacity (1 for open lakes).
    pub water_balance: f64,
}

/// Filled surface, receivers and lakes.
#[derive(Debug, Clone)]
pub struct Fill {
    pub n: usize,
    pub original: Vec<f64>,
    pub filled: Vec<f64>,
    /// Steepest lower neighbour on `filled`, [`NO_RECEIVER`] for sinks.
    pub receiver: Vec<u32>,
    /// Non-sink texels in descending `filled` order (donors before receivers).
    pub order: Vec<u32>,
    pub ocean: Vec<bool>,
    /// Lake index per texel or [`NO_LAKE`].
    pub lake_id: Vec<u32>,
    pub lakes: Vec<Lake>,
    /// Land texels that were pits (no lower neighbour) before filling.
    pub pits_before: usize,
}

impl Fill {
    /// Lake water level per texel (m), or `f32::MIN` without a lake,
    /// dilated by one texel so the shoreline is the contour where the
    /// level meets the interpolated terrain, not the texel edge.
    pub fn lake_level_map(&self) -> CubeMap<f32> {
        let mut map = CubeMap::new(self.n, f32::MIN);
        for (k, id) in self.lake_id.iter().enumerate() {
            if *id != NO_LAKE {
                map.data_mut()[k] = self.lakes[*id as usize].level_m as f32;
            }
        }
        let core = map.clone();
        for k in 0..core.data().len() {
            for m in neighbours(k, self.n) {
                let level = core.data()[m];
                if level > map.data()[k] && self.lake_id[k] == NO_LAKE {
                    map.data_mut()[k] = level;
                }
            }
        }
        map
    }
}

#[derive(PartialEq)]
struct Entry(f64, u32);
impl Eq for Entry {}
impl Ord for Entry {
    // Min-heap on height, ties by index (deterministic).
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
    }
}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Fill `elevation` on the neighbour graph `level`.
pub fn fill(elevation: &CubeMap<f32>, level: &Level, params: &HydrologyParams) -> Fill {
    let n = elevation.n();
    let texels = 6 * n * n;
    let original: Vec<f64> = elevation.data().iter().map(|h| f64::from(*h)).collect();
    let mut ocean: Vec<bool> = original.iter().map(|h| *h < 0.0).collect();
    if !ocean.iter().any(|o| *o) {
        // No ocean: the lowest texel is the single outlet.
        let lowest = (0..texels)
            .min_by(|a, b| original[*a].total_cmp(&original[*b]).then(a.cmp(b)))
            .expect("texels");
        ocean[lowest] = true;
    }
    let pits_before = (0..texels)
        .filter(|&k| {
            !ocean[k]
                && (0..8).all(|s| {
                    let m = level.neighbours[k][s] as usize;
                    m == k || original[m] >= original[k]
                })
        })
        .count();

    let mut filled = original.clone();
    let mut visited = ocean.clone();
    let mut heap = BinaryHeap::with_capacity(texels / 4);
    for k in 0..texels {
        if ocean[k] {
            heap.push(Entry(original[k], k as u32));
        }
    }
    let mut pop_order = Vec::with_capacity(texels);
    while let Some(Entry(h, k)) = heap.pop() {
        if !ocean[k as usize] {
            pop_order.push(k);
        }
        for s in 0..8 {
            let m = level.neighbours[k as usize][s] as usize;
            if visited[m] {
                continue;
            }
            visited[m] = true;
            filled[m] = original[m].max(h + params.epsilon_m);
            heap.push(Entry(filled[m], m as u32));
        }
    }

    // Steepest descent on the filled surface. Every land texel has a strictly
    // lower neighbour (the one that flooded it), so receivers never cycle.
    let mut receiver = vec![NO_RECEIVER; texels];
    for k in 0..texels {
        if ocean[k] {
            continue;
        }
        let mut best = (0.0, NO_RECEIVER);
        for s in 0..8 {
            let m = level.neighbours[k][s] as usize;
            let distance = level.distances[k][s];
            if m == k || distance <= 0.0 {
                continue;
            }
            let slope = (filled[k] - filled[m]) / distance;
            if slope > best.0 {
                best = (slope, m as u32);
            }
        }
        debug_assert!(
            best.1 != NO_RECEIVER,
            "land texel {k} has a lower neighbour"
        );
        receiver[k] = best.1;
    }
    pop_order.reverse();

    let (lake_id, lakes) = find_lakes(&original, &filled, &receiver, level, params);
    Fill {
        n,
        original,
        filled,
        receiver,
        order: pop_order,
        ocean,
        lake_id,
        lakes,
        pits_before,
    }
}

/// Connected regions (8-neighbour) filled above the original surface, kept
/// as lakes when deeper than the minimum somewhere and large enough.
fn find_lakes(
    original: &[f64],
    filled: &[f64],
    receiver: &[u32],
    level: &Level,
    params: &HydrologyParams,
) -> (Vec<u32>, Vec<Lake>) {
    let texels = original.len();
    // Flats raised only by the epsilon gradient are not water.
    let wet = |k: usize| filled[k] - original[k] > 0.25 * params.lake_min_depth_m.min(1.0);
    let mut lake_id = vec![NO_LAKE; texels];
    let mut lakes = Vec::new();
    let mut component = vec![NO_LAKE; texels];
    let mut stack = Vec::new();
    let mut members = Vec::new();
    for start in 0..texels {
        if component[start] != NO_LAKE || !wet(start) {
            continue;
        }
        let id = start as u32;
        component[start] = id;
        stack.push(start);
        members.clear();
        while let Some(k) = stack.pop() {
            members.push(k);
            for s in 0..8 {
                let m = level.neighbours[k][s] as usize;
                if component[m] == NO_LAKE && wet(m) {
                    component[m] = id;
                    stack.push(m);
                }
            }
        }
        let depth = members
            .iter()
            .map(|&k| filled[k] - original[k])
            .fold(0.0, f64::max);
        if depth < params.lake_min_depth_m || members.len() < params.lake_min_texels {
            continue;
        }
        let spill = members.iter().map(|&k| filled[k]).fold(f64::MIN, f64::max);
        let bottom = members
            .iter()
            .map(|&k| original[k])
            .fold(f64::MAX, f64::min);
        // Follow the receivers from any member until they leave the lake.
        let mut outlet = members[0];
        loop {
            let r = receiver[outlet];
            if r == NO_RECEIVER || component[r as usize] != id {
                break;
            }
            outlet = r as usize;
        }
        let index = lakes.len() as u32;
        for &k in &members {
            lake_id[k] = index;
        }
        lakes.push(Lake {
            spill_m: spill,
            level_m: spill,
            bottom_m: bottom,
            outlet: outlet as u32,
            texels: members.len(),
            area_km2: members.iter().map(|&k| level.areas[k]).sum(),
            endorheic: false,
            water_balance: 1.0,
        });
    }
    (lake_id, lakes)
}

/// Mark lakes whose inflow (discharge at the outlet) is below their area
/// times the evaporation ratio as endorheic, and lower their level with the
/// water balance (a dry salt flat when it is small).
pub fn classify_endorheic(fill: &mut Fill, discharge: &[f64], params: &HydrologyParams) {
    for lake in &mut fill.lakes {
        let inflow = discharge[lake.outlet as usize];
        let capacity = lake.area_km2 * params.endorheic_ratio;
        if inflow < capacity {
            let balance = (inflow / capacity.max(1e-9)).clamp(0.0, 1.0);
            lake.endorheic = true;
            lake.water_balance = balance;
            // Volume scales roughly with depth²: depth ∝ √balance.
            lake.level_m = lake.bottom_m + (lake.spill_m - lake.bottom_m) * balance.sqrt();
        }
    }
}
