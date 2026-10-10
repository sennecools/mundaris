//! River carving (§7.5, §7.6): a cube-sphere spatial hash of the river
//! segments and the f64 carve the Tier B producer mirrors.
use super::{HydrologyParams, NO_RECEIVER, RiverGraph, rivers::chord_projection};
use crate::terrain::{
    tier_a::erosion::{NEIGHBOUR_STEPS, face_step},
    world_map::locate,
};
use glam::DVec3;

/// Segments by hash cell, compressed rows (`start[c]..start[c + 1]` index
/// `segments`, each the upstream vertex of a segment).
#[derive(Debug, Clone, PartialEq)]
pub struct RiverHash {
    /// Cells per face edge.
    pub cells: usize,
    pub start: Vec<u32>,
    pub segments: Vec<u32>,
    /// Widest reach (m) a carve can have: the valley half-width cap.
    pub reach_m: f64,
}

impl RiverHash {
    /// Hash cell of a unit direction.
    pub fn cell_of(&self, d: DVec3) -> usize {
        let (face, u, v) = locate(d);
        let c = self.cells;
        let to = |x: f64| (((x + 1.0) * 0.5 * c as f64) as usize).min(c - 1);
        (face * c + to(v)) * c + to(u)
    }

    /// Build with cells at least three times the reach at face centres, so
    /// the 3×3 block around a segment point covers the reach everywhere
    /// (corner cells shrink to about a third).
    pub fn build(graph: &RiverGraph, map_cells: usize, radius_m: f64, p: &HydrologyParams) -> Self {
        let reach_m = p.valley_max_m;
        let cells =
            ((2.0 * radius_m / (3.0 * reach_m)).floor() as usize).clamp(4, map_cells.max(4));
        let mut hash = Self {
            cells,
            start: Vec::new(),
            segments: Vec::new(),
            reach_m,
        };
        let step = 1.0 / cells as f64; // half a cell at face centres (unit sphere)
        let mut pairs: Vec<(u32, u32)> = Vec::new();
        for (a, b) in graph.segments() {
            let (pa, pb) = (
                graph.vertices[a as usize].pos,
                graph.vertices[b as usize].pos,
            );
            let samples = ((pa - pb).length() / step).ceil().max(1.0) as usize;
            for s in 0..=samples {
                let t = s as f64 / samples as f64;
                let d = pa.lerp(pb, t).normalize();
                let cell = hash.cell_of(d);
                let face = (cell / (cells * cells)) as u32;
                let (i, j) = ((cell % cells) as i32, ((cell / cells) % cells) as i32);
                pairs.push((cell as u32, a));
                for (di, dj) in NEIGHBOUR_STEPS {
                    let (g, x, y) = face_step(face, i, j, di, dj, cells as i32);
                    pairs.push((
                        ((g as usize * cells + y as usize) * cells + x as usize) as u32,
                        a,
                    ));
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        let total = 6 * cells * cells;
        hash.start = vec![0; total + 1];
        for (c, _) in &pairs {
            hash.start[*c as usize + 1] += 1;
        }
        for c in 0..total {
            hash.start[c + 1] += hash.start[c];
        }
        hash.segments = pairs.iter().map(|(_, s)| *s).collect();
        hash
    }

    /// Segments near `d`.
    pub fn near(&self, d: DVec3) -> &[u32] {
        let c = self.cell_of(d);
        &self.segments[self.start[c] as usize..self.start[c + 1] as usize]
    }
}

/// One carved sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarveSample {
    /// Carved height (m), never above the input.
    pub height_m: f64,
    /// River water surface (m) when inside a channel.
    pub water_m: Option<f64>,
    /// 0 far from rivers, 1 on the valley floor (for materials).
    pub valley: f64,
    /// Distance to the nearest channel centreline (m).
    pub distance_m: f64,
}

/// V-valley profile over normalised distance `x` (0 at the channel edge, 1
/// at the valley rim): linear sides meeting the terrain with zero slope.
pub fn v_profile(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x) * (1.0 - x)
}

/// Floodplain profile: a nearly flat floor out to half the valley, then
/// smooth low banks.
pub fn floodplain_profile(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    let t = ((x - 0.5) / 0.5).clamp(0.0, 1.0);
    0.03 * x + 0.97 * t * t * (3.0 - 2.0 * t)
}

/// Carve `h` (m) at unit direction `d`: `min` over nearby segments of
/// `mix(bed, h, profile)` (§7.5), so carving only lowers terrain.
pub fn carve(
    graph: &RiverGraph,
    hash: &RiverHash,
    radius_m: f64,
    p: &HydrologyParams,
    d: DVec3,
    h: f64,
) -> CarveSample {
    let mut out = CarveSample {
        height_m: h,
        water_m: None,
        valley: 0.0,
        distance_m: f64::INFINITY,
    };
    for &s in hash.near(d) {
        let a = &graph.vertices[s as usize];
        if a.downstream == NO_RECEIVER {
            continue;
        }
        let b = &graph.vertices[a.downstream as usize];
        let (chord, t) = chord_projection(d, a.pos, b.pos);
        let distance = chord * radius_m;
        let lerp = |x: f64, y: f64| x + (y - x) * t;
        let width = lerp(a.width_m, b.width_m);
        let bed = lerp(a.bed_m, b.bed_m);
        let length = ((a.pos - b.pos).length() * radius_m).max(1.0);
        let slope = (a.bed_m - b.bed_m) / length;
        let incision = lerp(a.incision_m, b.incision_m);
        let half_valley = (width * p.valley_factor)
            .max(incision / p.valley_side_slope)
            .clamp(p.valley_min_m, p.valley_max_m);
        let edge = 0.5 * width;
        let x = ((distance - edge) / (half_valley - edge).max(1.0)).max(0.0);
        let plain = (1.0 - slope / p.floodplain_slope).clamp(0.0, 1.0);
        let profile = v_profile(x) * (1.0 - plain) + floodplain_profile(x) * plain;
        let carved = bed + (h - bed) * profile;
        if distance < out.distance_m {
            out.distance_m = distance;
        }
        if carved < out.height_m {
            out.height_m = carved;
        }
        if x < 1.0 && h > bed {
            out.valley = out.valley.max(1.0 - profile);
        }
        if distance <= edge {
            let surface = bed + lerp(a.depth_m, b.depth_m);
            out.water_m = Some(out.water_m.map_or(surface, |w: f64| w.max(surface)));
        }
    }
    out
}
