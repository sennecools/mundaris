//! Rocks from geology (genesis design §6, G5 prototype).
//!
//! A rock is a signed distance field built from its geology class: joint
//! planes (orthogonal blocks for granite, bedding for sediment, hexagonal
//! columns for basalt, random fractures for regolith breccia), rounded by
//! weathering (`wetness × age`), with low-frequency noise. It is meshed with
//! naive surface nets (one vertex per sign-changing cell, a quad per
//! sign-changing edge), three LODs by grid resolution. Colour comes from the
//! geology's strata bands; upward faces carry moss/lichen in the planet's
//! foliage pigment where it is wet.
//!
//! Pure, deterministic CPU code like the plant grower.

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::hash::{Stream, domain};
use crate::mesh::{FloraVertex, LOD_COUNT, Mesh};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Geology {
    /// Orthogonal joint blocks; corestones round with wetness × age.
    Granitic,
    /// Bedded slabs and ledges with strata colour bands.
    Sedimentary,
    /// Columnar joints (hexagonal prisms).
    Basaltic,
    /// Airless regolith: angular breccia.
    Regolith,
}

/// One rock archetype (`content/flora/rocks/<name>.ron`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RockFile {
    pub schema: u32,
    pub name: String,
    pub geology: Geology,
    /// Typical largest dimension, m.
    pub size_m: f64,
    /// Half-extents relative to size (x, y, z), z up.
    pub aspect: (f64, f64, f64),
    /// Joint/fracture planes cutting the block (granite, regolith) or columns
    /// (basalt).
    pub joints: u32,
    /// Strata spacing as a fraction of the height (sedimentary).
    pub strata: f64,
    /// Edge rounding at full weathering, fraction of size.
    pub rounding: f64,
    /// Surface noise amplitude, fraction of size.
    pub roughness: f64,
    /// Rock colour bands (linear RGB), bottom to top.
    pub bands: Vec<[f32; 3]>,
}

impl RockFile {
    pub fn from_ron(text: &str) -> Result<Self, String> {
        let r: RockFile = ron::from_str(text).map_err(|e| e.to_string())?;
        if r.schema != 1 {
            return Err(format!("rock schema {} unsupported", r.schema));
        }
        if !(0.05..=50.0).contains(&r.size_m) || r.bands.is_empty() || r.joints > 16 {
            return Err(format!("rock `{}` out of range", r.name));
        }
        Ok(r)
    }
}

/// Site/planet conditions that weather a rock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weathering {
    /// 0 dry .. 1 wet.
    pub wetness: f64,
    /// 0 fresh .. 1 old surface.
    pub age: f64,
    /// Moss/lichen colour (linear RGB) and how much of it grows (0..1).
    pub moss: [f32; 3],
    pub moss_cover: f64,
}

fn round_box(p: DVec3, b: DVec3, r: f64) -> f64 {
    let q = p.abs() - b + DVec3::splat(r);
    q.max(DVec3::ZERO).length() + q.x.max(q.y.max(q.z)).min(0.0) - r
}

fn smooth_max(a: f64, b: f64, k: f64) -> f64 {
    if k <= 0.0 {
        return a.max(b);
    }
    let h = (0.5 + 0.5 * (a - b) / k).clamp(0.0, 1.0);
    b + (a - b) * h + k * h * (1.0 - h)
}

/// Value noise in 3D on the lattice hash (for surface roughness).
fn noise3(seed: u64, p: DVec3) -> f64 {
    let i = p.floor();
    let f = p - i;
    let w = f * f * (DVec3::splat(3.0) - 2.0 * f);
    let st = Stream::new(seed, domain::ROCK);
    let h = |x: i64, y: i64, z: i64| st.unit(((x as u64) << 21) ^ ((y as u64) << 42) ^ z as u64, 0);
    let (x, y, z) = (i.x as i64, i.y as i64, i.z as i64);
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let x00 = lerp(h(x, y, z), h(x + 1, y, z), w.x);
    let x10 = lerp(h(x, y + 1, z), h(x + 1, y + 1, z), w.x);
    let x01 = lerp(h(x, y, z + 1), h(x + 1, y, z + 1), w.x);
    let x11 = lerp(h(x, y + 1, z + 1), h(x + 1, y + 1, z + 1), w.x);
    lerp(lerp(x00, x10, w.y), lerp(x01, x11, w.y), w.z) * 2.0 - 1.0
}

/// The rock's signed distance field (metres) for one variant.
pub struct RockSdf<'a> {
    rock: &'a RockFile,
    seed: u64,
    half: DVec3,
    round: f64,
    planes: Vec<(DVec3, f64)>,
    columns: Vec<(DVec3, f64, f64)>,
}

impl<'a> RockSdf<'a> {
    pub fn new(rock: &'a RockFile, seed: u64, w: &Weathering) -> Self {
        let st = Stream::new(seed, domain::ROCK);
        let s = rock.size_m * (0.8 + 0.4 * st.unit(0, 1));
        let half = DVec3::new(
            rock.aspect.0 * (0.85 + 0.3 * st.unit(0, 2)),
            rock.aspect.1 * (0.85 + 0.3 * st.unit(0, 3)),
            rock.aspect.2 * (0.85 + 0.3 * st.unit(0, 4)),
        ) * s;
        let weather = (w.wetness * (0.4 + 0.6 * w.age)).clamp(0.0, 1.0);
        let base_round = match rock.geology {
            Geology::Regolith => 0.02 + 0.1 * w.age,
            _ => 0.04 + 0.96 * weather,
        };
        let round = (rock.rounding * base_round * s).min(half.min_element() * 0.95);
        let mut planes = Vec::new();
        let mut columns = Vec::new();
        match rock.geology {
            Geology::Granitic | Geology::Regolith => {
                // Joint/fracture planes cutting corners off the block.
                for k in 0..rock.joints as u64 {
                    let n = DVec3::new(st.signed(k + 10, 0), st.signed(k + 10, 1), st.signed(k + 10, 2) * 0.7 + 0.2)
                        .normalize_or(DVec3::Z);
                    // Offset: how deep the joint cuts (support of the box along n).
                    let support = (n.abs() * half).element_sum();
                    let cut = support * (0.55 + 0.35 * st.unit(k + 10, 3));
                    planes.push((n, cut));
                }
            }
            Geology::Basaltic => {
                // Hexagonal columns of varying heights around the centre.
                let r = half.x.min(half.y) * 0.42;
                let count = rock.joints.clamp(1, 7) as u64;
                for k in 0..count {
                    let c = if k == 0 {
                        DVec3::ZERO
                    } else {
                        let a = (k - 1) as f64 * std::f64::consts::TAU / 6.0 + 0.3 * st.signed(k + 30, 0);
                        DVec3::new(a.cos(), a.sin(), 0.0) * r * 1.75
                    };
                    let height = half.z * (0.5 + 0.9 * st.unit(k + 30, 1));
                    columns.push((c, r * (0.92 + 0.1 * st.unit(k + 30, 2)), height));
                }
            }
            Geology::Sedimentary => {}
        }
        RockSdf { rock, seed, half, round, planes, columns }
    }

    pub fn half_extent(&self) -> DVec3 {
        let mut h = self.half;
        for &(c, r, _) in &self.columns {
            h = h.max(DVec3::new(c.x.abs() + r * 1.2, c.y.abs() + r * 1.2, h.z));
        }
        h + DVec3::splat(self.rock.roughness * self.rock.size_m * 1.5)
    }

    pub fn eval(&self, p: DVec3) -> f64 {
        let r = self.rock;
        let mut d = match r.geology {
            Geology::Basaltic => {
                let mut best = f64::MAX;
                for &(c, rad, h) in &self.columns {
                    let q = p - c;
                    // Hexagonal prism (Inigo Quilez), z up, base on the ground.
                    let k = DVec3::new(-0.866_025_4, 0.5, 0.577_350_3);
                    let mut a = DVec3::new(q.x.abs(), q.y.abs(), 0.0);
                    let dotk = (k.x * a.x + k.y * a.y).min(0.0);
                    a.x -= 2.0 * dotk * k.x;
                    a.y -= 2.0 * dotk * k.y;
                    let dx = ((a.x - a.x.clamp(-k.z * rad, k.z * rad)).powi(2) + (a.y - rad).powi(2)).sqrt() * (a.y - rad).signum();
                    let dz = (q.z - h * 0.5 + self.half.z).abs() - h * 0.5;
                    let col = dx.max(dz).min(0.0) + DVec3::new(dx.max(0.0), dz.max(0.0), 0.0).length() - self.round * 0.3;
                    // Smooth union: no hair-thin gaps between touching columns.
                    let k = 0.06 * self.rock.size_m;
                    let h = (0.5 + 0.5 * (col - best) / k).clamp(0.0, 1.0);
                    best = if best == f64::MAX { col } else { col + (best - col) * h - k * h * (1.0 - h) };
                }
                best
            }
            _ => round_box(p, self.half, self.round),
        };
        for &(n, cut) in &self.planes {
            d = smooth_max(d, p.dot(n) - cut, self.round * 0.6);
        }
        if r.geology == Geology::Sedimentary {
            // Bedding: shallow grooves between strata make ledges.
            let spacing = (r.strata * 2.0 * self.half.z).max(1e-3);
            let t = ((p.z + self.half.z) / spacing).fract();
            // Narrow grooves at the bedding planes (t near 0 or 1).
            let groove = (1.0 - (t - 0.5).abs() * 2.0).powi(8);
            d += groove * 0.06 * r.size_m;
        }
        let s = r.size_m;
        d + r.roughness * s * (noise3(self.seed, p / (0.35 * s)) + 0.5 * noise3(self.seed ^ 1, p / (0.12 * s)))
    }

    fn normal(&self, p: DVec3) -> DVec3 {
        let e = 0.01 * self.rock.size_m;
        DVec3::new(
            self.eval(p + DVec3::X * e) - self.eval(p - DVec3::X * e),
            self.eval(p + DVec3::Y * e) - self.eval(p - DVec3::Y * e),
            self.eval(p + DVec3::Z * e) - self.eval(p - DVec3::Z * e),
        )
        .normalize_or(DVec3::Z)
    }
}

/// Grid cells per axis for LOD0..2.
pub const ROCK_GRID: [usize; LOD_COUNT] = [17, 10, 6];

/// Mesh one LOD with naive surface nets. The rock sits with its base at z = 0
/// (sunk by 15 % of its height so it does not float).
pub fn mesh_rock(sdf: &RockSdf, cells: usize, w: &Weathering) -> Mesh {
    let ext = sdf.half_extent() * 1.1;
    let lo = -ext;
    let step = 2.0 * ext / cells as f64;
    let n = cells + 1;
    let at = |i: usize, j: usize, k: usize| lo + DVec3::new(i as f64 * step.x, j as f64 * step.y, k as f64 * step.z);
    let mut field = vec![0.0f64; n * n * n];
    for k in 0..n {
        for j in 0..n {
            for i in 0..n {
                field[(k * n + j) * n + i] = sdf.eval(at(i, j, k));
            }
        }
    }
    let f = |i: usize, j: usize, k: usize| field[(k * n + j) * n + i];
    let mut cell_vertex = vec![u32::MAX; cells * cells * cells];
    let mut mesh = Mesh::default();
    let sink = sdf.half.z * 2.0 * 0.15;
    let corners = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0), (0, 0, 1), (1, 0, 1), (0, 1, 1), (1, 1, 1)];
    let edges = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    for k in 0..cells {
        for j in 0..cells {
            for i in 0..cells {
                let v: Vec<f64> = corners.iter().map(|&(a, b, c)| f(i + a, j + b, k + c)).collect();
                if v.iter().all(|x| *x > 0.0) || v.iter().all(|x| *x <= 0.0) {
                    continue;
                }
                let mut sum = DVec3::ZERO;
                let mut cnt = 0.0;
                for &(a, b) in &edges {
                    if (v[a] > 0.0) != (v[b] > 0.0) {
                        let t = v[a] / (v[a] - v[b]);
                        let pa = at(i + corners[a].0, j + corners[a].1, k + corners[a].2);
                        let pb = at(i + corners[b].0, j + corners[b].1, k + corners[b].2);
                        sum += pa + (pb - pa) * t;
                        cnt += 1.0;
                    }
                }
                let p = sum / cnt;
                let nrm = sdf.normal(p);
                let col = rock_color(sdf, p, nrm, w);
                let pos = DVec3::new(p.x, p.y, p.z + sdf.half.z - sink);
                cell_vertex[(k * cells + j) * cells + i] = mesh.vertices.len() as u32;
                let c = |x: f32| (x.clamp(0.0, 1.0).sqrt() * 255.0).round() as u8;
                let sn = |x: f64| (x.clamp(-1.0, 1.0) * 127.0).round() as i8;
                // Cheap AO: darker toward the ground.
                let ao = (0.55 + 0.45 * ((pos.z) / (2.0 * sdf.half.z)).clamp(0.0, 1.0)) as f32;
                mesh.vertices.push(FloraVertex {
                    position: pos.as_vec3().into(),
                    normal: [sn(nrm.x), sn(nrm.y), sn(nrm.z), 0],
                    color: [c(col[0]), c(col[1]), c(col[2]), c(ao)],
                    pivot: [0.0; 3],
                    wind: [0, 255, 0, crate::mesh::class::ROCK],
                });
            }
        }
    }
    // A quad per sign-changing grid edge, joining the four cells around it.
    let cv = |i: usize, j: usize, k: usize| cell_vertex[(k * cells + j) * cells + i];
    for k in 1..cells {
        for j in 1..cells {
            for i in 0..cells {
                // x-edges between (i,j,k) and (i+1,j,k) corners.
                let (a, b) = (f(i, j, k), f(i + 1, j, k));
                if (a > 0.0) != (b > 0.0) {
                    let q = [cv(i, j - 1, k - 1), cv(i, j, k - 1), cv(i, j, k), cv(i, j - 1, k)];
                    push_quad(&mut mesh, q, a > 0.0);
                }
            }
        }
    }
    for k in 1..cells {
        for j in 0..cells {
            for i in 1..cells {
                let (a, b) = (f(i, j, k), f(i, j + 1, k));
                if (a > 0.0) != (b > 0.0) {
                    let q = [cv(i - 1, j, k - 1), cv(i - 1, j, k), cv(i, j, k), cv(i, j, k - 1)];
                    push_quad(&mut mesh, q, a > 0.0);
                }
            }
        }
    }
    for k in 0..cells {
        for j in 1..cells {
            for i in 1..cells {
                let (a, b) = (f(i, j, k), f(i, j, k + 1));
                if (a > 0.0) != (b > 0.0) {
                    let q = [cv(i - 1, j - 1, k), cv(i, j - 1, k), cv(i, j, k), cv(i - 1, j, k)];
                    push_quad(&mut mesh, q, a > 0.0);
                }
            }
        }
    }
    mesh
}

fn push_quad(m: &mut Mesh, q: [u32; 4], flip: bool) {
    if q.contains(&u32::MAX) {
        return;
    }
    let tris = if flip { [q[0], q[2], q[1], q[0], q[3], q[2]] } else { [q[0], q[1], q[2], q[0], q[2], q[3]] };
    m.indices.extend_from_slice(&tris);
}

fn rock_color(sdf: &RockSdf, p: DVec3, n: DVec3, w: &Weathering) -> [f32; 3] {
    let r = sdf.rock;
    let h = ((p.z + sdf.half.z) / (2.0 * sdf.half.z)).clamp(0.0, 0.999);
    let bands = &r.bands;
    let mut c = match r.geology {
        Geology::Sedimentary if n.z > 0.4 => bands[(sdf.seed as usize + bands.len() - 1) % bands.len()],
        Geology::Sedimentary => {
            let spacing = r.strata.max(1e-3);
            let idx = ((h / spacing).floor() as usize + (sdf.seed as usize % bands.len())) % bands.len();
            bands[idx]
        }
        _ => bands[(sdf.seed as usize) % bands.len()],
    };
    // Grain: small brightness noise.
    let g = 1.0 + 0.12 * noise3(sdf.seed ^ 7, p / (0.08 * r.size_m)) as f32;
    c = c.map(|v| v * g);
    // Moss/lichen on upward faces where wet.
    let up = n.z.max(0.0);
    let moss = (w.moss_cover * crate::niche::smoothstep(0.35, 0.85, up) * (0.6 + 0.4 * noise3(sdf.seed ^ 9, p / (0.3 * r.size_m)) * 0.5 + 0.2))
        .clamp(0.0, 1.0) as f32;
    [0, 1, 2].map(|a| c[a] + (w.moss[a] - c[a]) * moss)
}

/// A grown rock variant: three LOD meshes.
pub fn grow_rock(rock: &RockFile, seed: u64, w: &Weathering) -> [Mesh; LOD_COUNT] {
    let sdf = RockSdf::new(rock, seed, w);
    std::array::from_fn(|l| mesh_rock(&sdf, ROCK_GRID[l], w))
}

/// Load every rock archetype of a directory, sorted by file name.
pub fn load_rocks_dir(dir: &std::path::Path) -> Result<Vec<RockFile>, String> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "ron"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            RockFile::from_ron(&text).map_err(|e| format!("{}: {e}", p.display()))
        })
        .collect()
}

/// Mesh checks for the rock scorer: closed surface (every edge shared by
/// exactly two triangles), no degenerate triangles, upward base.
pub fn closed_edges_fraction(m: &Mesh) -> f64 {
    let mut edges: std::collections::BTreeMap<(u32, u32), u32> = std::collections::BTreeMap::new();
    for t in m.indices.chunks(3) {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    if edges.is_empty() {
        return 0.0;
    }
    edges.values().filter(|c| **c == 2).count() as f64 / edges.len() as f64
}
