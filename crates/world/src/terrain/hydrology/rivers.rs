//! River graph extraction (§7.3): channel texels above a discharge
//! threshold linked along their receivers, simplified per chain, with
//! hydraulic geometry and beds strictly descending downstream.
use super::{Fill, HydrologyParams, NO_RECEIVER, flood::NO_LAKE};
use crate::terrain::tier_a::erosion::Level;
use glam::DVec3;

/// `RiverVertex::texel` of a Catmull–Rom curve point.
pub const INSERTED: u32 = u32::MAX;

/// Where a river ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mouth {
    /// Not a mouth.
    None,
    Ocean,
    Lake,
}

/// One vertex of the river forest.
#[derive(Debug, Clone, PartialEq)]
pub struct RiverVertex {
    /// Unit direction (body frame).
    pub pos: DVec3,
    /// Precipitation-weighted drainage area (km²).
    pub discharge_km2: f64,
    /// Mean discharge (m³/s).
    pub q_m3s: f64,
    pub width_m: f64,
    pub depth_m: f64,
    /// Valley incision below the macro surface (m).
    pub incision_m: f64,
    /// Channel bed (m), strictly descending downstream.
    pub bed_m: f64,
    /// Next vertex downstream or [`NO_RECEIVER`] at a mouth.
    pub downstream: u32,
    pub mouth: Mouth,
    /// Tier A texel of the vertex, or [`INSERTED`] for curve points.
    pub texel: u32,
}

/// The river forest; segment `i` runs from vertex `i` to its `downstream`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RiverGraph {
    pub vertices: Vec<RiverVertex>,
}

impl RiverGraph {
    /// Upstream vertex of every segment.
    pub fn segments(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.vertices
            .iter()
            .enumerate()
            .filter(|(_, v)| v.downstream != NO_RECEIVER)
            .map(|(i, v)| (i as u32, v.downstream))
    }

    /// Vertex indices with every vertex after the ones upstream of it.
    pub fn topological_order(&self) -> Vec<u32> {
        let mut upstream = vec![0u32; self.vertices.len()];
        for v in &self.vertices {
            if v.downstream != NO_RECEIVER {
                upstream[v.downstream as usize] += 1;
            }
        }
        let mut ready: Vec<u32> = (0..self.vertices.len() as u32)
            .filter(|&i| upstream[i as usize] == 0)
            .collect();
        let mut order = Vec::with_capacity(self.vertices.len());
        while let Some(i) = ready.pop() {
            order.push(i);
            let d = self.vertices[i as usize].downstream;
            if d != NO_RECEIVER {
                upstream[d as usize] -= 1;
                if upstream[d as usize] == 0 {
                    ready.push(d);
                }
            }
        }
        order
    }

    pub fn segment_count(&self) -> usize {
        self.segments().count()
    }

    pub fn mouth_count(&self) -> usize {
        self.vertices
            .iter()
            .filter(|v| v.mouth != Mouth::None)
            .count()
    }

    /// Strahler-free summary: total channel length (km).
    pub fn length_km(&self, radius_m: f64) -> f64 {
        self.segments()
            .map(|(a, b)| {
                (self.vertices[a as usize].pos - self.vertices[b as usize].pos).length() * radius_m
            })
            .sum::<f64>()
            * 1e-3
    }
}

fn geometry(discharge_km2: f64, p: &HydrologyParams) -> (f64, f64, f64) {
    let q = discharge_km2 * p.runoff_m3s_per_km2;
    (
        q,
        p.width_a * q.powf(p.width_b),
        p.depth_c * q.powf(p.depth_d),
    )
}

/// Extract the graph from the fill and the final discharge.
pub fn extract(fill: &Fill, discharge: &[f64], level: &Level, p: &HydrologyParams) -> RiverGraph {
    let texels = fill.filled.len();
    let is_river = |k: usize| {
        !fill.ocean[k] && fill.lake_id[k] == NO_LAKE && discharge[k] >= p.river_min_discharge_km2
    };
    // Full-resolution vertices: one per channel texel, plus one per mouth
    // texel (ocean or lake) shared by every channel entering it.
    let mut vertex_of = vec![NO_RECEIVER; texels];
    let mut full: Vec<RiverVertex> = Vec::new();
    // Donors before receivers: channel texels in descending filled order.
    let channel: Vec<u32> = fill
        .order
        .iter()
        .copied()
        .filter(|&k| is_river(k as usize))
        .collect();
    for &k in &channel {
        let k = k as usize;
        let (q, width, depth) = geometry(discharge[k], p);
        let relief = (0..8)
            .map(|s| fill.original[level.neighbours[k][s] as usize] - fill.original[k])
            .fold(0.0, f64::max);
        // Valleys deepen from nothing at the channel head over the first
        // quadrupling of discharge, so no trench starts at full depth.
        let head = ((discharge[k] / p.river_min_discharge_km2 - 1.0) / 3.0).clamp(0.0, 1.0);
        let incision = (p.incision_relief_fraction * relief).min(p.incision_max_m)
            * head
            * head
            * (3.0 - 2.0 * head);
        vertex_of[k] = full.len() as u32;
        full.push(RiverVertex {
            pos: level.directions[k],
            discharge_km2: discharge[k],
            q_m3s: q,
            width_m: width,
            depth_m: depth,
            incision_m: incision,
            bed_m: fill.filled[k] - depth - incision,
            downstream: NO_RECEIVER,
            mouth: Mouth::None,
            texel: k as u32,
        });
    }
    for &k in &channel {
        let k = k as usize;
        let r = fill.receiver[k];
        if r == NO_RECEIVER {
            continue;
        }
        let r = r as usize;
        if vertex_of[r] == NO_RECEIVER {
            // A mouth: the channel enters the ocean or a lake.
            let (mouth, surface) = if fill.lake_id[r] != NO_LAKE {
                (Mouth::Lake, fill.lakes[fill.lake_id[r] as usize].level_m)
            } else {
                (Mouth::Ocean, fill.filled[r].min(0.0))
            };
            let (q, width, depth) = geometry(discharge[k], p);
            vertex_of[r] = full.len() as u32;
            full.push(RiverVertex {
                pos: level.directions[r],
                discharge_km2: discharge[k],
                q_m3s: q,
                width_m: width,
                depth_m: depth,
                incision_m: 0.0,
                bed_m: surface - depth,
                downstream: NO_RECEIVER,
                mouth,
                texel: r as u32,
            });
        } else if full[vertex_of[r] as usize].mouth != Mouth::None {
            // Another channel into the same mouth texel: widest wins.
            let v = &mut full[vertex_of[r] as usize];
            if discharge[k] > v.discharge_km2 {
                let (q, width, depth) = geometry(discharge[k], p);
                v.bed_m += v.depth_m - depth;
                (v.discharge_km2, v.q_m3s, v.width_m, v.depth_m) = (discharge[k], q, width, depth);
            }
        }
        full[vertex_of[k] as usize].downstream = vertex_of[r];
    }
    // Beds strictly descending: vertices are in donor-first order, mouths last.
    for i in 0..full.len() {
        let down = full[i].downstream;
        if down != NO_RECEIVER {
            let limit = full[i].bed_m - p.bed_step_m;
            let d = &mut full[down as usize];
            d.bed_m = d.bed_m.min(limit);
        }
    }
    let simple = simplify(full, 2.0 / fill.n as f64 * p.simplify_texels);
    smooth(simple, p.curve_points)
}

/// Insert `points` Catmull–Rom points per segment through the main stem
/// (largest upstream neighbour) and the next vertex downstream, so channels
/// curve through their vertices instead of running straight. Beds and
/// geometry interpolate linearly, which keeps beds descending.
pub fn smooth(graph: RiverGraph, points: usize) -> RiverGraph {
    if points == 0 {
        return graph;
    }
    let v = &graph.vertices;
    let mut main_up = vec![NO_RECEIVER; v.len()];
    for (i, x) in v.iter().enumerate() {
        let d = x.downstream;
        if d != NO_RECEIVER {
            let cur = main_up[d as usize];
            if cur == NO_RECEIVER || v[cur as usize].discharge_km2 < x.discharge_km2 {
                main_up[d as usize] = i as u32;
            }
        }
    }
    let mut out: Vec<RiverVertex> = v.clone();
    for (i, a) in v.iter().enumerate() {
        if a.downstream == NO_RECEIVER {
            continue;
        }
        let b = &v[a.downstream as usize];
        let p0 = if main_up[i] == NO_RECEIVER {
            a.pos * 2.0 - b.pos
        } else {
            v[main_up[i] as usize].pos
        };
        let p3 = if b.downstream == NO_RECEIVER {
            b.pos * 2.0 - a.pos
        } else {
            v[b.downstream as usize].pos
        };
        let mut previous = i;
        for s in 1..=points {
            let t = s as f64 / (points + 1) as f64;
            let (t2, t3) = (t * t, t * t * t);
            let pos = (p0 * (-t3 + 2.0 * t2 - t)
                + a.pos * (3.0 * t3 - 5.0 * t2 + 2.0)
                + b.pos * (-3.0 * t3 + 4.0 * t2 + t)
                + p3 * (t3 - t2))
                * 0.5;
            let lerp = |x: f64, y: f64| x + (y - x) * t;
            let index = out.len() as u32;
            out.push(RiverVertex {
                pos: pos.normalize(),
                discharge_km2: lerp(a.discharge_km2, b.discharge_km2),
                q_m3s: lerp(a.q_m3s, b.q_m3s),
                width_m: lerp(a.width_m, b.width_m),
                depth_m: lerp(a.depth_m, b.depth_m),
                incision_m: lerp(a.incision_m, b.incision_m),
                bed_m: lerp(a.bed_m, b.bed_m),
                downstream: a.downstream,
                mouth: Mouth::None,
                texel: INSERTED,
            });
            out[previous].downstream = index;
            previous = index as usize;
        }
    }
    RiverGraph { vertices: out }
}

/// Douglas–Peucker per chain between key vertices (sources, confluences,
/// mouths and the vertex before a mouth); `tolerance` is in unit-sphere
/// distance.
fn simplify(full: Vec<RiverVertex>, tolerance: f64) -> RiverGraph {
    let count = full.len();
    let mut upstream = vec![0u32; count];
    for v in &full {
        if v.downstream != NO_RECEIVER {
            upstream[v.downstream as usize] += 1;
        }
    }
    let key = |i: usize| {
        upstream[i] != 1
            || full[i].mouth != Mouth::None
            || full[i].downstream == NO_RECEIVER
            || full[full[i].downstream as usize].mouth != Mouth::None
    };
    let mut keep = vec![false; count];
    let mut chain = Vec::new();
    for start in 0..count {
        if !key(start) {
            continue;
        }
        keep[start] = true;
        chain.clear();
        chain.push(start);
        let mut i = start;
        while full[i].downstream != NO_RECEIVER {
            i = full[i].downstream as usize;
            chain.push(i);
            if key(i) {
                break;
            }
        }
        if chain.len() > 2 {
            douglas_peucker(&full, &chain, tolerance, &mut keep);
        }
        if let Some(&last) = chain.last() {
            keep[last] = true;
        }
    }
    // Reindex the kept vertices, linking each to the next kept one downstream.
    let mut index = vec![NO_RECEIVER; count];
    let mut out = Vec::with_capacity(keep.iter().filter(|k| **k).count());
    for i in 0..count {
        if keep[i] {
            index[i] = out.len() as u32;
            out.push(full[i].clone());
        }
    }
    for (i, v) in full.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        let mut d = v.downstream;
        while d != NO_RECEIVER && !keep[d as usize] {
            d = full[d as usize].downstream;
        }
        out[index[i] as usize].downstream = if d == NO_RECEIVER {
            NO_RECEIVER
        } else {
            index[d as usize]
        };
    }
    RiverGraph { vertices: out }
}

/// Keep the vertices of `chain` that deviate from their chord by more than
/// `tolerance`.
fn douglas_peucker(full: &[RiverVertex], chain: &[usize], tolerance: f64, keep: &mut [bool]) {
    let mut stack = vec![(0usize, chain.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (full[chain[a]].pos, full[chain[b]].pos);
        let mut worst = (0.0, a);
        for (m, &i) in chain.iter().enumerate().take(b).skip(a + 1) {
            let d = distance_to_chord(full[i].pos, pa, pb);
            if d > worst.0 {
                worst = (d, m);
            }
        }
        if worst.0 > tolerance {
            keep[chain[worst.1]] = true;
            stack.push((a, worst.1));
            stack.push((worst.1, b));
        }
    }
}

/// Distance (unit sphere) from `p` to the great-circle arc `a`–`b` (the
/// closest chord point projected back onto the sphere), and the chord
/// parameter of that point.
pub fn chord_projection(p: DVec3, a: DVec3, b: DVec3) -> (f64, f64) {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-300)).clamp(0.0, 1.0);
    ((p - (a + ab * t).normalize()).length(), t)
}

pub fn distance_to_chord(p: DVec3, a: DVec3, b: DVec3) -> f64 {
    chord_projection(p, a, b).0
}
