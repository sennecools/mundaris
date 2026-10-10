//! Skeleton → meshes with three levels of detail.
//!
//! Medium-detail profile (user decision 2026-10-10, DECISIONS.md): smooth
//! shading, generalised cylinders with 4–10 sides by radius, kit organs at
//! LOD0, half the organs (area-preserving) at LOD1, organ clumps at LOD2.
//!
//! Vertex layout is wind-ready: every vertex carries the pivot of the part
//! that sways (branch base or organ attachment), its hierarchy level and a
//! stiffness, so the shader can bend hierarchically without the skeleton.

use glam::{DVec3, Vec3};

use crate::SpeciesFile;
use crate::genome::{CrossSection, Genome, SurfaceStyle};
use crate::grow::{Extra, PartKind, Skeleton};
use crate::hash::{Stream, domain};
use crate::kit::Kit;

pub const LOD_COUNT: usize = 3;

/// Material class in `FloraVertex::wind[3]`.
pub mod class {
    pub const BARK: u8 = 0;
    pub const ORGAN: u8 = 1;
    pub const FRUIT: u8 = 2;
}

/// 36-byte vertex. Positions in metres, plant frame (z up, origin at the
/// base). Colour is linear RGB × 255 with alpha = ambient occlusion.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FloraVertex {
    pub position: [f32; 3],
    /// Unit normal, snorm8 (w unused, 0).
    pub normal: [i8; 4],
    pub color: [u8; 4],
    /// Sway pivot (branch base / organ attachment).
    pub pivot: [f32; 3],
    /// (hierarchy level, stiffness 0..255, phase 0..255, material class).
    pub wind: [u8; 4],
}

impl FloraVertex {
    pub const SIZE: usize = 36;

    pub fn to_bytes(&self, out: &mut Vec<u8>) {
        for p in self.position {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out.extend(self.normal.map(|v| v as u8));
        out.extend(self.color);
        for p in self.pivot {
            out.extend_from_slice(&p.to_le_bytes());
        }
        out.extend(self.wind);
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mesh {
    pub vertices: Vec<FloraVertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bytes(&self) -> usize {
        self.vertices.len() * FloraVertex::SIZE
            + self.indices.len() * if self.vertices.len() <= 65536 { 2 } else { 4 }
    }

    /// Canonical byte image (vertices then u32 indices), for golden tests.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.vertices.len() * 36 + self.indices.len() * 4);
        for v in &self.vertices {
            v.to_bytes(&mut out);
        }
        for i in &self.indices {
            out.extend_from_slice(&i.to_le_bytes());
        }
        out
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for v in &self.vertices {
            let p = Vec3::from(v.position);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        (lo, hi)
    }

    fn push(
        &mut self,
        pos: DVec3,
        n: DVec3,
        color: [f32; 3],
        ao: f32,
        pivot: DVec3,
        wind: [u8; 4],
    ) -> u32 {
        let id = self.vertices.len() as u32;
        let n = if n.length_squared() > 1e-12 {
            n.normalize()
        } else {
            DVec3::Z
        };
        let c = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.vertices.push(FloraVertex {
            position: pos.as_vec3().into(),
            normal: [snorm(n.x), snorm(n.y), snorm(n.z), 0],
            color: [c(color[0]), c(color[1]), c(color[2]), c(ao)],
            pivot: pivot.as_vec3().into(),
            wind,
        });
        id
    }
}

fn snorm(x: f64) -> i8 {
    (x.clamp(-1.0, 1.0) * 127.0).round() as i8
}

/// Tube meshing settings; coarser levels are tried until the branch
/// triangles fit their share of the budget.
#[derive(Debug, Clone, Copy)]
struct LodSpec {
    /// Sides by radius thresholds (r > 0.15, > 0.05, > 0.015, else).
    sides: [u32; 4],
    /// Drop axes whose base radius is below `min_radius_tips × tip radius`.
    min_radius_tips: f64,
    /// Keep every n-th ring on axes of order ≥ 1 (half that on stems).
    ring_stride: usize,
    max_order: u32,
}

const TUBE_LEVELS: [LodSpec; 6] = [
    LodSpec {
        sides: [10, 8, 5, 4],
        min_radius_tips: 0.0,
        ring_stride: 1,
        max_order: 99,
    },
    LodSpec {
        sides: [8, 6, 4, 3],
        min_radius_tips: 0.0,
        ring_stride: 2,
        max_order: 99,
    },
    LodSpec {
        sides: [6, 5, 4, 3],
        min_radius_tips: 1.3,
        ring_stride: 2,
        max_order: 99,
    },
    LodSpec {
        sides: [5, 4, 3, 3],
        min_radius_tips: 2.0,
        ring_stride: 3,
        max_order: 99,
    },
    LodSpec {
        sides: [4, 4, 3, 3],
        min_radius_tips: 3.0,
        ring_stride: 4,
        max_order: 3,
    },
    LodSpec {
        sides: [4, 3, 3, 3],
        min_radius_tips: 5.0,
        ring_stride: 6,
        max_order: 2,
    },
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum OrganLod {
    /// Keep up to this fraction (budget may thin further); kept organs grow
    /// to preserve area.
    Thin(f64),
    Clumps,
}

#[derive(Debug, Clone, Copy)]
struct LodPlan {
    first_level: usize,
    /// Share of the species triangle budget.
    budget: f64,
    /// Share of the LOD budget branches may use before a coarser level.
    branch_share: f64,
    organs: OrganLod,
}

const LODS: [LodPlan; LOD_COUNT] = [
    LodPlan {
        first_level: 0,
        budget: 1.0,
        branch_share: 0.55,
        organs: OrganLod::Thin(1.0),
    },
    LodPlan {
        first_level: 2,
        budget: 0.25,
        branch_share: 0.5,
        organs: OrganLod::Thin(0.35),
    },
    LodPlan {
        first_level: 4,
        budget: 0.06,
        branch_share: 0.35,
        organs: OrganLod::Clumps,
    },
];

/// LOD0 triangle budget by growth form and height (medium-detail profile,
/// DECISIONS.md 2026-10-10; proposed, to be measured in the forests).
pub fn triangle_budget(g: &Genome) -> usize {
    use crate::genome::GrowthForm::*;
    let h = g.height_m.max(0.01);
    let b = match g.form {
        Tree => (9000.0 * (h / 15.0).sqrt()).clamp(3000.0, 10_000.0),
        Shrub | Cushion | Columnar => (3500.0 * (h / 2.0).sqrt()).clamp(1200.0, 5000.0),
        Tuft | Rosette => 900.0,
        FungalCap => 1200.0,
    };
    b as usize
}

fn sides_for(spec: &LodSpec, r: f64, section: CrossSection) -> u32 {
    let s = if r > 0.15 {
        spec.sides[0]
    } else if r > 0.05 {
        spec.sides[1]
    } else if r > 0.015 {
        spec.sides[2]
    } else {
        spec.sides[3]
    };
    match section {
        CrossSection::Triangular => 3,
        CrossSection::Ribbed => (s.max(6) / 2) * 2,
        _ => s,
    }
}

/// An axis: a chain of node indices following `main_child`, prefixed with the
/// node it branches from.
fn axes(sk: &Skeleton) -> Vec<Vec<u32>> {
    let mut out = Vec::new();
    for (i, n) in sk.nodes.iter().enumerate() {
        let Some(p) = n.parent else { continue };
        if sk.nodes[p as usize].main_child == Some(i as u32) && p != 0 {
            continue;
        }
        let mut chain = vec![p, i as u32];
        let mut c = n.main_child;
        while let Some(k) = c {
            chain.push(k);
            c = sk.nodes[k as usize].main_child;
        }
        out.push(chain);
    }
    out
}

/// Build all LODs.
pub fn build_lods(sk: &Skeleton, species: &SpeciesFile, kit: &Kit, seed: u64) -> [Mesh; LOD_COUNT] {
    let budget = triangle_budget(&species.genome) as f64;
    std::array::from_fn(|l| build_lod(sk, species, kit, seed, &LODS[l], budget))
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn branches(sk: &Skeleton, sp: &SpeciesFile, spec: &LodSpec) -> Mesh {
    let g = &sp.genome;
    let mut m = Mesh::default();
    let base_r = sk
        .nodes
        .first()
        .map(|n| n.radius)
        .unwrap_or(g.tip_radius_m)
        .max(1e-6);
    for (ai, chain) in axes(sk).iter().enumerate() {
        let first = &sk.nodes[chain[1] as usize];
        if first.order > spec.max_order {
            continue;
        }
        if first.order > 0 && first.radius < spec.min_radius_tips * g.tip_radius_m {
            continue;
        }
        tube(&mut m, sk, g, sp.look.bark, chain, spec, base_r, ai as u32);
    }
    for e in &sk.extras {
        match *e {
            Extra::Cap {
                centre,
                radius,
                height,
            } => cap(&mut m, g, sp, centre, radius, height, spec),
        }
    }
    m
}

fn build_lod(
    sk: &Skeleton,
    sp: &SpeciesFile,
    kit: &Kit,
    seed: u64,
    plan: &LodPlan,
    budget: f64,
) -> Mesh {
    let lod_budget = budget * plan.budget;
    let mut m = Mesh::default();
    for spec in &TUBE_LEVELS[plan.first_level..] {
        m = branches(sk, sp, spec);
        if (m.triangles() as f64) <= lod_budget * plan.branch_share {
            break;
        }
    }
    let organ_budget = (lod_budget - m.triangles() as f64).max(lod_budget * 0.15);
    parts(&mut m, sk, sp, kit, seed, plan.organs, organ_budget);
    m
}

#[allow(clippy::too_many_arguments)]
fn tube(
    m: &mut Mesh,
    sk: &Skeleton,
    g: &Genome,
    bark: [f32; 3],
    chain: &[u32],
    spec: &LodSpec,
    base_r: f64,
    axis_id: u32,
) {
    let nodes = &sk.nodes;
    let order = nodes[chain[1] as usize].order;
    // Rings: keep first, last and every stride-th in between.
    let stride = if order == 0 {
        1.max(spec.ring_stride / 2)
    } else {
        spec.ring_stride
    };
    let mut idx: Vec<usize> = (0..chain.len())
        .filter(|k| k % stride == 0 || *k == chain.len() - 1)
        .collect();
    idx.dedup();
    if idx.len() < 2 {
        return;
    }
    let pts: Vec<DVec3> = idx.iter().map(|&k| nodes[chain[k] as usize].pos).collect();
    let mut radii: Vec<f64> = idx
        .iter()
        .map(|&k| nodes[chain[k] as usize].radius)
        .collect();
    // The joint ring takes the branch's own radius so thin twigs do not
    // inherit the parent's girth.
    radii[0] = radii[1].max(radii[0].min(radii[1] * 1.15));
    let sides = sides_for(spec, radii[1], g.cross_section);
    let pivot = pts[0];
    let stiff = (g.stiffness * (radii[1] / base_r).sqrt().min(1.0) * 255.0) as u8;
    let phase = (Stream::new(axis_id as u64, domain::VARIANT).bits(0, 0) & 0xff) as u8;
    let wind = [order.min(3) as u8, stiff, phase, class::BARK];
    // Parallel-transported frame.
    let d0 = (pts[1] - pts[0]).normalize_or(DVec3::Z);
    let (mut u, _) = perp(d0);
    let mut rings: Vec<u32> = Vec::with_capacity(pts.len());
    let height = g.height_m.max(1e-3);
    for k in 0..pts.len() {
        let d = if k + 1 < pts.len() {
            if k > 0 {
                (pts[k + 1] - pts[k - 1]).normalize_or(d0)
            } else {
                (pts[1] - pts[0]).normalize_or(d0)
            }
        } else {
            (pts[k] - pts[k - 1]).normalize_or(d0)
        };
        u = (u - d * u.dot(d)).normalize_or(perp(d).0);
        let w = d.cross(u);
        let r = radii[k];
        let start = m.vertices.len() as u32;
        rings.push(start);
        // Bark darkens toward the base and inside the crown (cheap AO).
        let ao = (0.55 + 0.45 * (pts[k].z / height).clamp(0.0, 1.0)) as f32;
        let band = match g.surface {
            SurfaceStyle::Ringed | SurfaceStyle::Jointed if k % 2 == 1 => 0.8,
            _ => 1.0,
        };
        for s in 0..sides {
            let mut a = s as f64 / sides as f64 * std::f64::consts::TAU;
            if g.surface == SurfaceStyle::Spiralled {
                a += k as f64 * 0.35;
            }
            let (ca, sa) = (a.cos(), a.sin());
            let (rx, ry) = match g.cross_section {
                CrossSection::Ribbed => {
                    let rib = if s % 2 == 0 { 1.0 } else { 0.82 };
                    (r * rib, r * rib)
                }
                CrossSection::Flattened => (r * 1.25, r * 0.7),
                CrossSection::Triangular => (r * 1.3, r * 1.3),
                CrossSection::Round => (r, r),
            };
            let off = u * (ca * rx) + w * (sa * ry);
            let nrm = u * (ca * ry) + w * (sa * rx);
            m.push(pts[k] + off, nrm, scale3(bark, band), ao, pivot, wind);
        }
    }
    for k in 0..rings.len() - 1 {
        let (a, b) = (rings[k], rings[k + 1]);
        for s in 0..sides {
            let s1 = (s + 1) % sides;
            m.indices
                .extend_from_slice(&[a + s, a + s1, b + s1, a + s, b + s1, b + s]);
        }
    }
    // Close the tip with a short cone.
    let last = *pts.last().unwrap();
    let dl = (last - pts[pts.len() - 2]).normalize_or(d0);
    let apex = m.push(
        last + dl * radii[radii.len() - 1] * 1.5,
        dl,
        bark,
        1.0,
        pivot,
        wind,
    );
    let lr = *rings.last().unwrap();
    for s in 0..sides {
        m.indices
            .extend_from_slice(&[lr + s, lr + (s + 1) % sides, apex]);
    }
}

fn perp(d: DVec3) -> (DVec3, DVec3) {
    let r = if d.z.abs() < 0.9 { DVec3::Z } else { DVec3::X };
    let u = d.cross(r).normalize();
    (u, d.cross(u))
}

fn cap(
    m: &mut Mesh,
    _g: &Genome,
    sp: &SpeciesFile,
    c: DVec3,
    radius: f64,
    height: f64,
    spec: &LodSpec,
) {
    let seg = spec.sides[0] * 2;
    let rings = (spec.sides[0] / 2).max(2);
    let wind = [1, 200, 0, class::ORGAN];
    let col = sp.look.organ;
    let tip = sp.look.organ_tip;
    let mut rows: Vec<u32> = Vec::new();
    for r in 0..=rings {
        let t = r as f64 / rings as f64; // 0 at rim, 1 at top
        let ang = t * std::f64::consts::FRAC_PI_2;
        let rr = radius * ang.cos();
        let z = height * ang.sin();
        rows.push(m.vertices.len() as u32);
        for s in 0..seg {
            let a = s as f64 / seg as f64 * std::f64::consts::TAU;
            let p = c + DVec3::new(rr * a.cos(), rr * a.sin(), z - 0.15 * height);
            let n = DVec3::new(
                a.cos() * ang.cos() * height,
                a.sin() * ang.cos() * height,
                ang.sin() * radius,
            );
            m.push(p, n, lerp3(col, tip, t as f32), 1.0, c, wind);
        }
    }
    for r in 0..rings as usize {
        let (a, b) = (rows[r], rows[r + 1]);
        for s in 0..seg {
            let s1 = (s + 1) % seg;
            m.indices
                .extend_from_slice(&[a + s, a + s1, b + s1, a + s, b + s1, b + s]);
        }
    }
    // Underside (gills) as a flat fan, darker.
    let centre = m.push(
        c - DVec3::Z * 0.15 * height,
        -DVec3::Z,
        scale3(col, 0.5),
        0.6,
        c,
        wind,
    );
    let rim = rows[0];
    for s in 0..seg {
        m.indices
            .extend_from_slice(&[centre, rim + (s + 1) % seg, rim + s]);
    }
}

fn parts(
    m: &mut Mesh,
    sk: &Skeleton,
    sp: &SpeciesFile,
    kit: &Kit,
    seed: u64,
    mode: OrganLod,
    budget: f64,
) {
    let g = &sp.genome;
    let thin = Stream::new(seed, domain::ORGAN);
    let keep_max = match mode {
        OrganLod::Clumps => {
            // Fruits vanish at the clump level.
            clumps(m, sk, sp, budget);
            return;
        }
        OrganLod::Thin(k) => k,
    };
    let organ_tris = kit.organ(g.organ).map_or(0, |s| s.triangles.len());
    let fruit_tris = kit.fruit(g.fruit).map_or(0, |s| s.triangles.len());
    let wanted: f64 = sk
        .parts
        .iter()
        .map(|p| if p.kind == PartKind::Organ { organ_tris } else { fruit_tris } as f64)
        .sum();
    // Area-preserving thinning to the budget, never below 5 % of organs.
    let keep = (budget / wanted.max(1.0)).min(keep_max).clamp(0.05, 1.0);
    for (i, p) in sk.parts.iter().enumerate() {
        let (shape, mut size) = match p.kind {
            PartKind::Organ => (kit.organ(g.organ), p.size),
            PartKind::Fruit => (kit.fruit(g.fruit), p.size),
        };
        let Some(shape) = shape else { continue };
        if keep < 1.0 {
            if thin.unit(i as u64, 99) >= keep {
                continue;
            }
            if p.kind == PartKind::Organ {
                size *= (1.0 / keep).sqrt().min(3.0);
            }
        }
        let fwd = p.forward;
        let nrm = p.normal;
        let side = nrm.cross(fwd);
        let (color, cls) = match p.kind {
            PartKind::Organ => {
                let jit = 0.92 + 0.16 * thin.unit(i as u64, 98) as f32;
                (
                    scale3(lerp3(sp.look.organ, sp.look.organ_tip, p.tipness), jit),
                    class::ORGAN,
                )
            }
            PartKind::Fruit => (sp.look.accent, class::FRUIT),
        };
        let phase = (thin.bits(i as u64, 97) & 0xff) as u8;
        let wind = [
            (p.order + 1).min(4) as u8,
            (g.stiffness * 80.0) as u8,
            phase,
            cls,
        ];
        let base = m.vertices.len() as u32;
        // Smooth shading of a curved card: blend the face normal with the
        // direction from the organ's spine so cards read rounded.
        for v in &shape.positions {
            let local = DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
            let pos = p.pos + (fwd * local.x + side * local.y + nrm * local.z) * size;
            let n = nrm + side * (local.y * 1.2) + fwd * 0.15;
            m.push(pos, n, color, 0.75 + 0.25 * p.tipness, p.pos, wind);
        }
        for t in &shape.triangles {
            m.indices.extend_from_slice(&[
                base + t[0] as u32,
                base + t[1] as u32,
                base + t[2] as u32,
            ]);
        }
    }
}

/// LOD2 foliage: organs binned on a coarse grid, one smooth ellipsoid per bin
/// sized to the bin's organ extent.
const CLUMP_TRIS: f64 = 36.0;

fn clumps(m: &mut Mesh, sk: &Skeleton, sp: &SpeciesFile, budget: f64) {
    let organs: Vec<_> = sk
        .parts
        .iter()
        .filter(|p| p.kind == PartKind::Organ)
        .collect();
    if organs.is_empty() {
        return;
    }
    let mut lo = DVec3::splat(f64::MAX);
    let mut hi = DVec3::splat(f64::MIN);
    for p in &organs {
        lo = lo.min(p.pos);
        hi = hi.max(p.pos);
    }
    let ext = (hi - lo).max(DVec3::splat(1e-3));
    // Finest grid (3³ down to 1) whose occupied bins fit the budget.
    let mut bins: Vec<Vec<usize>> = Vec::new();
    for cells in (1..=3usize).rev() {
        bins = vec![Vec::new(); cells * cells * cells];
        for (k, p) in organs.iter().enumerate() {
            let c = ((p.pos - lo) / ext * cells as f64).floor();
            let ix = |v: f64| (v as usize).min(cells - 1);
            bins[ix(c.x) + cells * (ix(c.y) + cells * ix(c.z))].push(k);
        }
        let used = bins.iter().filter(|b| b.len() >= 2).count() as f64;
        if used * CLUMP_TRIS <= budget {
            break;
        }
    }
    for bin in bins.iter().filter(|b| b.len() >= 2) {
        let mut c = DVec3::ZERO;
        let mut tip = 0.0f32;
        let mut size = 0.0;
        for &k in bin {
            c += organs[k].pos;
            tip += organs[k].tipness;
            size += organs[k].size;
        }
        let n = bin.len() as f64;
        c /= n;
        size /= n;
        let mut var = DVec3::ZERO;
        for &k in bin {
            let d = organs[k].pos - c;
            var += d * d;
        }
        let r = (var / n).map(|v| v.sqrt() * 1.6) + DVec3::splat(size * 0.6);
        let col = lerp3(sp.look.organ, sp.look.organ_tip, tip / n as f32);
        ellipsoid(m, c, r, col);
    }
}

fn ellipsoid(m: &mut Mesh, c: DVec3, r: DVec3, col: [f32; 3]) {
    let (seg, rings) = (6u32, 4u32);
    let wind = [2, 60, 0, class::ORGAN];
    let top = m.push(c + DVec3::Z * r.z, DVec3::Z, col, 1.0, c, wind);
    let mut rows = Vec::new();
    for k in 1..rings {
        let th = k as f64 / rings as f64 * std::f64::consts::PI;
        rows.push(m.vertices.len() as u32);
        for s in 0..seg {
            let a = s as f64 / seg as f64 * std::f64::consts::TAU;
            let unit = DVec3::new(th.sin() * a.cos(), th.sin() * a.sin(), th.cos());
            let ao = (0.65 + 0.35 * (0.5 + 0.5 * th.cos())) as f32;
            m.push(c + unit * r, unit / r, col, ao, c, wind);
        }
    }
    let bottom = m.push(c - DVec3::Z * r.z, -DVec3::Z, col, 0.6, c, wind);
    for s in 0..seg {
        let s1 = (s + 1) % seg;
        m.indices
            .extend_from_slice(&[top, rows[0] + s, rows[0] + s1]);
        let l = rows[rows.len() - 1];
        m.indices.extend_from_slice(&[bottom, l + s1, l + s]);
    }
    for k in 0..rows.len() - 1 {
        let (a, b) = (rows[k], rows[k + 1]);
        for s in 0..seg {
            let s1 = (s + 1) % seg;
            m.indices
                .extend_from_slice(&[a + s, b + s, b + s1, a + s, b + s1, a + s1]);
        }
    }
}
