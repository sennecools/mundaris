//! Scorer: structural validity, silhouette metrics and budgets (packet step 3:
//! scorer before pictures).

use glam::{DVec3, Vec2, Vec3};

use crate::grow::Skeleton;
use crate::mesh::{LOD_COUNT, Mesh};
use crate::raster::{Camera, Canvas, draw};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StructureMetrics {
    pub nodes: usize,
    pub internodes: usize,
    pub axes: usize,
    pub max_order: u32,
    /// Every node reaches the root through its parents.
    pub connected: bool,
    /// Internodes thicker than their parent internode.
    pub taper_violations: usize,
    /// Internodes deeply interpenetrating a non-adjacent internode.
    pub intersecting: usize,
    pub markers: usize,
    pub markers_left: usize,
    pub shed_branches: u32,
    pub organs: usize,
    pub height_m: f64,
    pub width_m: f64,
    pub base_radius_m: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SilhouetteMetrics {
    /// Width / height of the whole silhouette (mean of two side views).
    pub aspect: f64,
    /// Fraction of the silhouette's crown bounding box that is covered.
    pub crown_fill: f64,
    /// Box-counting dimension of the silhouette outline.
    pub fractal_dimension: f64,
    /// Width at 10 % height / max width (trunk visibility; 1 = blob to ground).
    pub base_width_ratio: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlantMetrics {
    pub structure: StructureMetrics,
    pub silhouette: SilhouetteMetrics,
    pub triangles: [usize; LOD_COUNT],
    pub vertices: [usize; LOD_COUNT],
    pub bytes: [usize; LOD_COUNT],
}

pub fn structure(sk: &Skeleton) -> StructureMetrics {
    let nodes = &sk.nodes;
    let mut m = StructureMetrics {
        nodes: nodes.len(),
        internodes: sk.internodes(),
        markers: sk.markers,
        markers_left: sk.markers_left,
        shed_branches: sk.shed_branches,
        organs: sk.parts.len(),
        connected: true,
        base_radius_m: nodes.first().map(|n| n.radius).unwrap_or(0.0),
        ..Default::default()
    };
    let mut lo = DVec3::splat(f64::MAX);
    let mut hi = DVec3::splat(f64::MIN);
    for (i, n) in nodes.iter().enumerate() {
        lo = lo.min(n.pos);
        hi = hi.max(n.pos);
        m.max_order = m.max_order.max(n.order);
        match n.parent {
            None if i != 0 => m.connected = false,
            Some(p) if p as usize >= i => m.connected = false,
            Some(p) => {
                // The joint node of a new axis may be thinner than the branch
                // (pipe model sums to the parent), so compare along axes only.
                let parent = &nodes[p as usize];
                if parent.axis == n.axis && n.radius > parent.radius * 1.0001 && p != 0 {
                    m.taper_violations += 1;
                }
                if n.axis != parent.axis && p != 0 && n.radius > parent.radius * 1.0001 {
                    m.taper_violations += 1;
                }
            }
            None => {}
        }
        if !n.children.is_empty()
            && n.main_child.is_none()
            && n.children.iter().all(|&c| nodes[c as usize].axis != n.axis)
        {
            // Sympodial hand-over: fine.
        }
    }
    if !nodes.is_empty() {
        m.height_m = hi.z - lo.z.min(0.0);
        m.width_m = (hi.x - lo.x).max(hi.y - lo.y);
    }
    m.axes = nodes
        .iter()
        .enumerate()
        .filter(|(i, n)| {
            n.parent
                .is_some_and(|p| nodes[p as usize].main_child != Some(*i as u32) || p == 0)
        })
        .count();
    m.intersecting = intersections(sk);
    m
}

fn seg_dist(p1: DVec3, q1: DVec3, p2: DVec3, q2: DVec3) -> f64 {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let (s, t);
    if a <= 1e-12 && e <= 1e-12 {
        return r.length();
    }
    if a <= 1e-12 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= 1e-12 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut ss = if denom > 1e-12 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut tt = (b * ss + f) / e;
            if tt < 0.0 {
                tt = 0.0;
                ss = (-c / a).clamp(0.0, 1.0);
            } else if tt > 1.0 {
                tt = 1.0;
                ss = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = ss;
            t = tt;
        }
    }
    ((p1 + d1 * s) - (p2 + d2 * t)).length()
}

/// Internodes whose capsule overlaps a non-adjacent capsule by more than half
/// the sum of radii.
fn intersections(sk: &Skeleton) -> usize {
    let nodes = &sk.nodes;
    let segs: Vec<(usize, usize)> = nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.parent.map(|p| (p as usize, i)))
        .collect();
    if segs.is_empty() {
        return 0;
    }
    let max_len = segs
        .iter()
        .map(|&(p, i)| (nodes[i].pos - nodes[p].pos).length())
        .fold(0.0, f64::max);
    let max_r = nodes.iter().map(|n| n.radius).fold(0.0, f64::max);
    let cell = (max_len + 2.0 * max_r).max(1e-3);
    let key = |p: DVec3| {
        (
            (p.x / cell).floor() as i64,
            (p.y / cell).floor() as i64,
            (p.z / cell).floor() as i64,
        )
    };
    let mut grid: std::collections::HashMap<(i64, i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    for (k, &(p, i)) in segs.iter().enumerate() {
        grid.entry(key((nodes[p].pos + nodes[i].pos) * 0.5))
            .or_default()
            .push(k);
    }
    let mut hit = vec![false; segs.len()];
    for (k, &(p, i)) in segs.iter().enumerate() {
        let c = key((nodes[p].pos + nodes[i].pos) * 0.5);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = grid.get(&(c.0 + dx, c.1 + dy, c.2 + dz)) else {
                        continue;
                    };
                    for &o in list {
                        if o <= k {
                            continue;
                        }
                        let (q, j) = segs[o];
                        // Adjacent: share a node, or siblings from one node.
                        if p == q || p == j || i == q || i == j {
                            continue;
                        }
                        let gp = nodes[p].parent.map(|x| x as usize);
                        let gq = nodes[q].parent.map(|x| x as usize);
                        if gp == Some(q) || gq == Some(p) {
                            continue;
                        }
                        let d = seg_dist(nodes[p].pos, nodes[i].pos, nodes[q].pos, nodes[j].pos);
                        if d < 0.5 * (nodes[i].radius + nodes[j].radius) {
                            hit[k] = true;
                            hit[o] = true;
                        }
                    }
                }
            }
        }
    }
    hit.iter().filter(|h| **h).count()
}

pub fn silhouette(mesh: &Mesh) -> SilhouetteMetrics {
    const N: usize = 256;
    let (lo, hi) = mesh.bounds();
    if mesh.vertices.is_empty() {
        return SilhouetteMetrics::default();
    }
    let ext = (hi - lo).max(Vec3::splat(1e-3));
    let px = (N as f32 - 16.0) / ext.x.max(ext.y).max(ext.z);
    let centre = Vec3::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, lo.z);
    let mut acc = SilhouetteMetrics::default();
    for az in [0.0f32, std::f32::consts::FRAC_PI_2] {
        let mut c = Canvas::new(N, N, Vec3::ZERO);
        let cam = Camera {
            azimuth: az,
            elevation: 0.0,
            target: centre,
            anchor_px: Vec2::new(N as f32 * 0.5, N as f32 - 8.0),
            px_per_m: px,
            clip: [0, 0, N, N],
        };
        draw(&mut c, mesh, &cam);
        let s = mask_metrics(&c.coverage, N);
        acc.aspect += s.aspect * 0.5;
        acc.crown_fill += s.crown_fill * 0.5;
        acc.fractal_dimension += s.fractal_dimension * 0.5;
        acc.base_width_ratio += s.base_width_ratio * 0.5;
    }
    acc
}

fn mask_metrics(mask: &[bool], n: usize) -> SilhouetteMetrics {
    let mut rows: Vec<(usize, usize, usize)> = Vec::new(); // (y, x0, x1)
    let (mut y0, mut y1) = (n, 0);
    let (mut gx0, mut gx1) = (n, 0);
    for y in 0..n {
        let r = &mask[y * n..(y + 1) * n];
        if let (Some(a), Some(b)) = (r.iter().position(|v| *v), r.iter().rposition(|v| *v)) {
            rows.push((y, a, b));
            y0 = y0.min(y);
            y1 = y1.max(y);
            gx0 = gx0.min(a);
            gx1 = gx1.max(b);
        }
    }
    if rows.is_empty() {
        return SilhouetteMetrics::default();
    }
    let h = (y1 - y0 + 1) as f64;
    let w = (gx1 - gx0 + 1) as f64;
    let maxw = rows.iter().map(|r| r.2 - r.1 + 1).max().unwrap() as f64;
    // Crown: rows wider than 30 % of the max width.
    let crown: Vec<_> = rows
        .iter()
        .filter(|r| (r.2 - r.1 + 1) as f64 > 0.3 * maxw)
        .collect();
    let (cy0, cy1) = (
        crown.iter().map(|r| r.0).min().unwrap(),
        crown.iter().map(|r| r.0).max().unwrap(),
    );
    let mut covered = 0usize;
    for y in cy0..=cy1 {
        covered += (gx0..=gx1).filter(|&x| mask[y * n + x]).count();
    }
    let crown_fill = covered as f64 / ((cy1 - cy0 + 1) as f64 * w);
    let base_row = y1.saturating_sub(((h * 0.1) as usize).max(1));
    let base_w = rows
        .iter()
        .find(|r| r.0 == base_row)
        .map(|r| (r.2 - r.1 + 1) as f64)
        .unwrap_or(0.0);
    // Box counting on the outline.
    let boundary = |x: usize, y: usize| {
        mask[y * n + x]
            && (x == 0
                || y == 0
                || x == n - 1
                || y == n - 1
                || !mask[y * n + x - 1]
                || !mask[y * n + x + 1]
                || !mask[(y - 1) * n + x]
                || !mask[(y + 1) * n + x])
    };
    let mut pts = Vec::new();
    for s in [2usize, 4, 8, 16, 32] {
        let mut boxes = std::collections::BTreeSet::new();
        for y in 0..n {
            for x in 0..n {
                if boundary(x, y) {
                    boxes.insert((x / s, y / s));
                }
            }
        }
        pts.push(((s as f64).ln(), (boxes.len().max(1) as f64).ln()));
    }
    let k = pts.len() as f64;
    let mx = pts.iter().map(|p| p.0).sum::<f64>() / k;
    let my = pts.iter().map(|p| p.1).sum::<f64>() / k;
    let slope = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum::<f64>()
        / pts.iter().map(|p| (p.0 - mx).powi(2)).sum::<f64>();
    SilhouetteMetrics {
        aspect: w / h,
        crown_fill,
        fractal_dimension: -slope,
        base_width_ratio: base_w / maxw,
    }
}

/// Budgets and validity bands; returns human-readable failures.
#[derive(Debug, Clone, Copy)]
pub struct Bands {
    pub max_triangles: [usize; LOD_COUNT],
    pub max_intersecting_fraction: f64,
    pub fractal_dimension: (f64, f64),
    pub crown_fill: (f64, f64),
}

impl Bands {
    /// Medium-detail profile (DECISIONS.md 2026-10-10), proposed.
    pub const HERO: Bands = Bands {
        max_triangles: [12_000, 3_500, 700],
        max_intersecting_fraction: 0.03,
        fractal_dimension: (1.05, 1.7),
        crown_fill: (0.25, 0.92),
    };
}

impl PlantMetrics {
    pub fn failures(&self, b: &Bands) -> Vec<String> {
        let mut f = Vec::new();
        let s = &self.structure;
        if !s.connected {
            f.push("disconnected".into());
        }
        if s.taper_violations > 0 {
            f.push(format!("taper x{}", s.taper_violations));
        }
        let frac = s.intersecting as f64 / s.internodes.max(1) as f64;
        if frac > b.max_intersecting_fraction {
            f.push(format!("intersect {:.1}%", frac * 100.0));
        }
        for l in 0..LOD_COUNT {
            if self.triangles[l] > b.max_triangles[l] {
                f.push(format!("lod{l} tris {}", self.triangles[l]));
            }
        }
        let d = self.silhouette.fractal_dimension;
        if d < b.fractal_dimension.0 || d > b.fractal_dimension.1 {
            f.push(format!("fd {d:.2}"));
        }
        let c = self.silhouette.crown_fill;
        if c < b.crown_fill.0 || c > b.crown_fill.1 {
            f.push(format!("fill {c:.2}"));
        }
        // Monotone LODs.
        if self.triangles[1] > self.triangles[0] || self.triangles[2] > self.triangles[1] {
            f.push("lods not decreasing".into());
        }
        f
    }
}
