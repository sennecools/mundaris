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
    pub const ROCK: u8 = 3;
}

/// 36-byte vertex. Positions in metres, plant frame (z up, origin at the
/// base). Colour is sqrt-encoded linear RGB (decode: square) with alpha =
/// ambient occlusion (linear).
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

    pub(crate) fn push(
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
        let c = |x: f32| (x.clamp(0.0, 1.0).sqrt() * 255.0).round() as u8;
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
    /// Stylised foliage: the organs grouped into about `count` soft puffs
    /// (icospheres with `detail` subdivisions).
    Puffs { count: usize, detail: u32 },
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

/// Stylised LODs (art direction 2026-10-10): chunky branches from the coarse
/// tube levels (twigs vanish inside the puffs), foliage as a few big puffs.
const STYLISED_LODS: [LodPlan; LOD_COUNT] = [
    LodPlan {
        first_level: 3,
        budget: 0.5,
        branch_share: 0.35,
        organs: OrganLod::Puffs { count: 34, detail: 1 },
    },
    LodPlan {
        first_level: 4,
        budget: 0.15,
        branch_share: 0.3,
        organs: OrganLod::Puffs { count: 14, detail: 0 },
    },
    LodPlan {
        first_level: 4,
        budget: 0.05,
        branch_share: 0.25,
        organs: OrganLod::Puffs { count: 5, detail: 0 },
    },
];

/// LOD0 triangle budget by growth form and height (medium-detail profile,
/// DECISIONS.md 2026-10-10; proposed, to be measured in the forests).
pub fn triangle_budget(g: &Genome) -> usize {
    use crate::genome::GrowthForm::*;
    let h = g.height_m.max(0.01);
    let b = match g.form {
        Tree => (11_000.0 * (h / 15.0).sqrt()).clamp(3000.0, 12_000.0),
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
    let plans = match species.style {
        crate::genome::FoliageStyle::Realistic => &LODS,
        crate::genome::FoliageStyle::Stylised => &STYLISED_LODS,
    };
    std::array::from_fn(|l| build_lod(sk, species, kit, seed, &plans[l], budget))
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
    // Stylised trunks are chunkier (art direction 2026-10-10).
    let girth = if sp.style == crate::genome::FoliageStyle::Stylised { 1.8 } else { 1.0 };
    for (ai, chain) in axes(sk).iter().enumerate() {
        let first = &sk.nodes[chain[1] as usize];
        if first.order > spec.max_order {
            continue;
        }
        if first.order > 0 && first.radius < spec.min_radius_tips * g.tip_radius_m {
            continue;
        }
        tube(&mut m, sk, g, sp.look.bark, chain, spec, base_r, ai as u32, girth);
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
    girth: f64,
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
        let r = radii[k] * girth;
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
        OrganLod::Puffs { count, detail } => {
            // Fringe cards: the organ's spray (cluster) card when the kit has
            // one, else the organ itself.
            let card = kit.shape_named(&format!("{:?}Spray", g.organ)).or_else(|_| kit.shape_named("Spray")).ok().or(kit.organ(g.organ));
            puffs(m, sk, sp, seed, count, detail, card);
            return;
        }
        OrganLod::Thin(k) => k,
    };
    let organ_tris = kit.organ(g.organ).map_or(0, |s| s.triangles.len());
    let fruit_tris = kit.fruit(g.fruit).map_or(0, |s| s.triangles.len());
    let organs = sk
        .parts
        .iter()
        .filter(|p| p.kind == PartKind::Organ)
        .count() as f64;
    let fruits = sk.parts.len() as f64 - organs;
    let wanted = organs * organ_tris as f64 + fruits * fruit_tris as f64;
    // Area-preserving thinning to the budget. Below half the organs, kept
    // organs become foliage sprays (one kit card standing for a cluster),
    // which read as leaf mass instead of oversized single leaves.
    let mut keep = (budget / wanted.max(1.0)).min(keep_max).clamp(0.02, 1.0);
    // Sprays only below LOD0 (LOD0 keeps the true organ shape).
    let spray = kit
        .shape_named(&format!("{:?}Spray", g.organ))
        .or_else(|_| kit.shape_named("Spray"))
        .ok()
        .filter(|_| keep < 0.5 && organ_tris > 0 && keep_max < 1.0);
    if let Some(s) = spray {
        let tris = organs * s.triangles.len() as f64 + fruits * fruit_tris as f64;
        keep = (budget / tris.max(1.0)).min(keep_max).clamp(0.02, 1.0);
    }
    let organ_area = kit.organ(g.organ).map_or(1.0, shape_area);
    let spray_scale = spray.map_or(1.0, |s| {
        (2.5 * organ_area / (shape_area(s) * keep))
            .sqrt()
            .clamp(1.0, 6.0)
    });
    for (i, p) in sk.parts.iter().enumerate() {
        let (shape, mut size) = match p.kind {
            PartKind::Organ => (spray.or(kit.organ(g.organ)), p.size),
            PartKind::Fruit => (kit.fruit(g.fruit), p.size),
        };
        let Some(shape) = shape else { continue };
        if keep < 1.0 {
            if thin.unit(i as u64, 99) >= keep {
                continue;
            }
            if p.kind == PartKind::Organ {
                size *= if spray.is_some() {
                    spray_scale
                } else {
                    (1.0 / keep).sqrt().min(3.0)
                };
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
        ellipsoid(
            m,
            c,
            r,
            col,
            sp.genome.crown_shape == crate::genome::CrownShape::Cone,
        );
    }
}

fn ellipsoid(m: &mut Mesh, c: DVec3, r: DVec3, col: [f32; 3], pointed: bool) {
    let (seg, rings) = (6u32, 4u32);
    let wind = [2, 60, 0, class::ORGAN];
    // Cone crowns get pointed clumps (shed-snow silhouette survives LOD2).
    let apex = if pointed { 2.0 } else { 1.0 };
    let top = m.push(c + DVec3::Z * r.z * apex, DVec3::Z, col, 1.0, c, wind);
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

/// Unit icosphere with `detail` midpoint subdivisions (0 = icosahedron).
fn icosphere(detail: u32) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5f64.sqrt()) / 2.0;
    let mut v: Vec<DVec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| DVec3::from(*p).normalize())
    .collect();
    let mut f: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..detail {
        let mut mid = std::collections::HashMap::new();
        let mut split = |a: u32, b: u32, v: &mut Vec<DVec3>| {
            *mid.entry((a.min(b), a.max(b))).or_insert_with(|| {
                v.push(((v[a as usize] + v[b as usize]) * 0.5).normalize());
                v.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(f.len() * 4);
        for &[a, b, c] in &f {
            let ab = split(a, b, &mut v);
            let bc = split(b, c, &mut v);
            let ca = split(c, a, &mut v);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        f = next;
    }
    (v, f)
}

/// Stylised foliage: the organs grouped by k-means into about `count` puffs
/// (lumpy icospheres sized to their group). Normals blend the puff's own
/// with the whole crown's, so the crown shades as one soft shape; colour
/// runs from `organ` inside/below to `organ_tip` outside/above. At
/// `detail` ≥ 1 the fruits stay as small chunky accents.
#[allow(clippy::too_many_arguments)]
fn puffs(m: &mut Mesh, sk: &Skeleton, sp: &SpeciesFile, seed: u64, count: usize, detail: u32, card: Option<&crate::kit::KitShape>) {
    let organs: Vec<_> = sk.parts.iter().filter(|p| p.kind == PartKind::Organ).collect();
    if organs.is_empty() {
        return;
    }
    let rng = Stream::new(seed, domain::ORGAN);
    let pts: Vec<DVec3> = organs.iter().map(|p| p.pos).collect();
    let k = count.min(pts.len()).max(1);
    // Farthest-point start from a seeded organ, then Lloyd iterations.
    let mut centres = vec![pts[(rng.bits(0, 11) % pts.len() as u64) as usize]];
    while centres.len() < k {
        let far = pts
            .iter()
            .copied()
            .max_by(|a, b| {
                let da = centres.iter().map(|c| a.distance_squared(*c)).fold(f64::MAX, f64::min);
                let db = centres.iter().map(|c| b.distance_squared(*c)).fold(f64::MAX, f64::min);
                da.total_cmp(&db)
            })
            .unwrap();
        centres.push(far);
    }
    let mut owner = vec![0usize; pts.len()];
    for _ in 0..8 {
        for (i, p) in pts.iter().enumerate() {
            owner[i] = (0..k).min_by(|&a, &b| p.distance_squared(centres[a]).total_cmp(&p.distance_squared(centres[b]))).unwrap();
        }
        let mut sum = vec![DVec3::ZERO; k];
        let mut n = vec![0.0; k];
        for (i, p) in pts.iter().enumerate() {
            sum[owner[i]] += *p;
            n[owner[i]] += 1.0;
        }
        for c in 0..k {
            if n[c] > 0.0 {
                centres[c] = sum[c] / n[c];
            }
        }
    }
    let crown = pts.iter().copied().sum::<DVec3>() / pts.len() as f64;
    let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
    for p in &pts {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let span = (hi.z - lo.z).max(1e-3);
    let pointed = sp.genome.crown_shape == crate::genome::CrownShape::Cone;
    // Gradient inside the crown: deep shaded inner colour to bright warm
    // tips, by height in the crown and by how much the surface faces out.
    let gradient = |pos: DVec3, nrm: DVec3, crown_n: DVec3| {
        let h = ((pos.z - lo.z) / span).clamp(0.0, 1.0);
        let t = (0.55 * h + 0.45 * (0.5 + 0.5 * crown_n.dot(nrm))).clamp(0.0, 1.0);
        let t = (t * t * (3.0 - 2.0 * t)) as f32;
        lerp3(scale3(sp.look.organ, 0.55), scale3(sp.look.organ_tip, 1.25), t)
    };
    let (unit, faces) = icosphere(detail);
    let wind = [2, 60, 0, class::ORGAN];
    for (c, &centre) in centres.iter().enumerate() {
        let members: Vec<usize> = (0..pts.len()).filter(|&i| owner[i] == c).collect();
        if members.is_empty() {
            continue;
        }
        let n = members.len() as f64;
        let spread = (members.iter().map(|&i| pts[i].distance_squared(centre)).sum::<f64>() / n).sqrt();
        let size = members.iter().map(|&i| organs[i].size).sum::<f64>() / n;
        let r = 1.05 * spread + 0.8 * size;
        let radii = DVec3::new(r, r, 0.85 * r);
        let base = m.vertices.len() as u32;
        let out_dir = (centre - crown).normalize_or(DVec3::Z);
        for (vi, u) in unit.iter().enumerate() {
            let lump = 1.0 + 0.34 * (rng.unit(c as u64 * 64 + vi as u64, 12) - 0.5);
            let mut local = *u * radii * lump;
            if pointed {
                // Conifer tiers: each puff narrows into a short cone.
                let up = u.z.max(0.0);
                local.x *= 1.0 - 0.5 * up;
                local.y *= 1.0 - 0.5 * up;
                local.z += 0.45 * r * up;
            }
            let pos = centre + local;
            let crown_n = (pos - crown).normalize_or(DVec3::Z);
            let nrm = (0.5 * *u + 0.5 * crown_n).normalize();
            let col = gradient(pos, nrm, crown_n);
            // Faces turned into the crown are darker (cheap AO).
            let ao = (0.6 + 0.4 * (0.5 + 0.5 * u.dot(out_dir))) as f32;
            m.push(pos, nrm, col, ao, centre, wind);
        }
        for f in &faces {
            m.indices.extend_from_slice(&[base + f[0], base + f[1], base + f[2]]);
        }
        // Ragged leafy edge: a fringe of enlarged organ cards poking out of
        // the hull, shaded with the puff's rounded normal so they read as
        // part of the clump, not as single leaves.
        let Some(shape) = card else { continue };
        // Up to 12 (LOD0) / 5 cards, within ~the hull's own triangle count.
        let fringe = (faces.len() / shape.triangles.len().max(1)).clamp(2, if detail >= 1 { 12 } else { 5 });
        let leaf = 2.6 * size.max(0.08 * r);
        for k in 0..fringe {
            let key = c as u64 * 1024 + k as u64;
            // Direction on the puff, biased away from the crown centre and up.
            let z = 2.0 * rng.unit(key, 14) - 1.0;
            let a = rng.unit(key, 15) * std::f64::consts::TAU;
            let s = (1.0 - z * z).max(0.0).sqrt();
            let d = (DVec3::new(s * a.cos(), s * a.sin(), z) + 0.8 * out_dir + 0.3 * DVec3::Z).normalize_or(DVec3::Z);
            let anchor = centre + d * radii * 0.85;
            let (t1, _) = perp(d);
            let tilt = rng.unit(key, 16) * std::f64::consts::TAU;
            let tangent = t1 * tilt.cos() + d.cross(t1) * tilt.sin();
            let fwd = (0.75 * d + 0.66 * tangent).normalize();
            let nrm_card = d.cross(fwd).cross(fwd).normalize_or(d) * -1.0;
            let side = nrm_card.cross(fwd);
            let crown_n = (anchor - crown).normalize_or(DVec3::Z);
            let shade_n = (0.5 * d + 0.5 * crown_n).normalize();
            let col = gradient(anchor + d * leaf * 0.5, shade_n, crown_n);
            let b = m.vertices.len() as u32;
            for v in &shape.positions {
                let p = anchor + (fwd * v[0] as f64 + side * v[1] as f64 + nrm_card * v[2] as f64) * leaf;
                m.push(p, shade_n, col, 0.9, centre, wind);
            }
            for t in &shape.triangles {
                m.indices.extend_from_slice(&[b + t[0] as u32, b + t[1] as u32, b + t[2] as u32]);
            }
        }
    }
    if detail >= 1 {
        let (fu, ff) = icosphere(0);
        let fruit_wind = [3, 60, 0, class::FRUIT];
        for (i, p) in sk.parts.iter().filter(|p| p.kind == PartKind::Fruit).take(16).enumerate() {
            let base = m.vertices.len() as u32;
            let r = p.size * 0.6;
            let lift = (p.pos - crown).normalize_or(DVec3::Z) * r;
            for (vi, u) in fu.iter().enumerate() {
                let lump = 1.0 + 0.2 * (rng.unit(9000 + i as u64 * 16 + vi as u64, 13) - 0.5);
                m.push(p.pos + lift + *u * r * lump, *u, sp.look.accent, 1.0, p.pos, fruit_wind);
            }
            for f in &ff {
                m.indices.extend_from_slice(&[base + f[0], base + f[1], base + f[2]]);
            }
        }
    }
}

/// Area of a kit shape in its unit frame.
fn shape_area(s: &crate::kit::KitShape) -> f64 {
    s.triangles
        .iter()
        .map(|t| {
            let p = |i: u16| glam::DVec3::from(s.positions[i as usize].map(|c| c as f64));
            0.5 * (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).length()
        })
        .sum::<f64>()
        .max(1e-6)
}
