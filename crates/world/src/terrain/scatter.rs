//! Scatter placement core (pipeline §12; milestone M5 Life).
//!
//! PROTOTYPE: CPU oracle for the GPU scatter compute pass. Every instance is a
//! deterministic function of its cell `(seed, face, level, i, j)`; nothing is
//! stored. Integer hashing only (PCG3D, no `fract(sin)`, §17.1). The WGSL
//! mirror is `crates/renderer/src/shaders/scatter.wgsl`; keep them in step.
//! Not wired into the renderer or the app yet. Constants marked `PROTOTYPE:`
//! are hardcoded and move to species data (RON) once the look is accepted.
//!
//! # Cell ownership and cube-face edges
//! A cell is a square of the face chart `(u, v) in [-1, 1]^2`. Its candidates
//! are jittered strictly inside that square, so every candidate is owned by
//! exactly one `(face, i, j)` cell. Cells on either side of a face edge never
//! produce the same candidate; the shared edge needs no agreement. Known
//! limit: spacing/competition across the edge is not resolved (each face
//! only sees its own neighbours); the 3x3 re-hash of §12.2 step 6 must look
//! across the edge when this is hardened.
//!
//! # No-pop rule
//! Competition (spacing) is resolved on the full candidate set, independent
//! of LOD. Only afterwards is the LOD thinning applied, as a pure function of
//! the candidate's own `rank`. So the instances kept at a coarse LOD are
//! always a subset of those kept at any finer LOD.

use std::f64::consts::PI;

/// PCG3D integer hash (Jarzynski & Olano 2020). Identical to the terrain
/// noise hash; copied so this module has no cross-module dependency.
pub fn pcg3d(input: [u32; 3]) -> [u32; 3] {
    let mut v = input.map(|x| x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223));
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    v = v.map(|x| x ^ (x >> 16));
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    v
}

/// Hash bits to a unit float in [0, 1) using 24 bits.
fn unit(bits: u32) -> f64 {
    (bits >> 8) as f64 / 16_777_216.0
}

/// Candidate slots per cell (PROTOTYPE: fixed; the GPU loop is unrolled).
pub const SLOTS_PER_CELL: u32 = 2;

/// Number of placeholder species.
pub const SPECIES_COUNT: usize = 3;

/// Trapezoid range: 1 inside `[min, max]`, falling linearly to 0 over
/// `falloff` outside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    pub falloff: f64,
}

impl Range {
    pub const fn new(min: f64, max: f64, falloff: f64) -> Self {
        Self { min, max, falloff }
    }
    pub fn eval(&self, x: f64) -> f64 {
        let d = (self.min - x).max(x - self.max).max(0.0);
        (1.0 - d / self.falloff.max(1e-9)).clamp(0.0, 1.0)
    }
}

/// Placeholder species definition (§12.3, reduced).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Species {
    pub name: &'static str,
    /// Cleared radius around the instance, metres (spacing rule).
    pub footprint_m: f64,
    pub temperature_c: Range,
    pub moisture: Range,
    /// Height above sea level, metres (treeline lives here).
    pub height_m: Range,
    /// Hard slope limit, radians, with a soft falloff.
    pub max_slope: f64,
    pub slope_falloff: f64,
    /// -1 avoids rock, +1 needs rock, 0 indifferent.
    pub rock_affinity: f64,
    /// Minimum sediment (soil depth proxy) before the species grows, 0..1.
    pub soil_min: f64,
    /// Multiplicative bonus within `water_within_m` of water.
    pub water_bonus: f64,
    pub water_within_m: f64,
    /// Peak instances per candidate slot, 0..1.
    pub base_density: f64,
    /// Prior weight in the species choice hash.
    pub prior_weight: f64,
    /// Forest/clearing patches: noise scale (m) and covered fraction.
    pub patch_scale_m: f64,
    pub patch_coverage: f64,
    /// Scale range multiplier.
    pub scale: (f64, f64),
}

const DEG: f64 = PI / 180.0;

/// PROTOTYPE: the three placeholder species. Index = species id.
pub const SPECIES: [Species; SPECIES_COUNT] = [
    Species {
        name: "conifer",
        footprint_m: 3.0,
        temperature_c: Range::new(-8.0, 16.0, 4.0),
        moisture: Range::new(0.30, 1.0, 0.10),
        height_m: Range::new(0.0, 3000.0, 200.0),
        max_slope: 38.0 * DEG,
        slope_falloff: 8.0 * DEG,
        rock_affinity: -1.0,
        soil_min: 0.15,
        water_bonus: 0.15,
        water_within_m: 40.0,
        base_density: 0.85,
        prior_weight: 0.5,
        patch_scale_m: 700.0,
        patch_coverage: 0.6,
        scale: (0.8, 1.3),
    },
    Species {
        name: "broadleaf_shrub",
        footprint_m: 1.2,
        temperature_c: Range::new(-2.0, 32.0, 6.0),
        moisture: Range::new(0.20, 1.0, 0.10),
        height_m: Range::new(0.0, 2400.0, 300.0),
        max_slope: 45.0 * DEG,
        slope_falloff: 10.0 * DEG,
        rock_affinity: -0.5,
        soil_min: 0.05,
        water_bonus: 0.3,
        water_within_m: 30.0,
        base_density: 0.7,
        prior_weight: 0.35,
        patch_scale_m: 300.0,
        patch_coverage: 0.7,
        scale: (0.6, 1.4),
    },
    Species {
        name: "boulder",
        footprint_m: 1.5,
        temperature_c: Range::new(-80.0, 80.0, 10.0),
        moisture: Range::new(0.0, 1.0, 0.10),
        height_m: Range::new(0.0, 9000.0, 500.0),
        max_slope: 60.0 * DEG,
        slope_falloff: 10.0 * DEG,
        rock_affinity: 1.0,
        soil_min: 0.0,
        water_bonus: 0.0,
        water_within_m: 0.0,
        base_density: 0.35,
        prior_weight: 0.15,
        patch_scale_m: 120.0,
        patch_coverage: 0.5,
        scale: (0.4, 2.5),
    },
];

/// Terrain facts at a candidate position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiteInputs {
    pub temperature_c: f64,
    /// 0..1.
    pub moisture: f64,
    /// Slope angle from horizontal, radians.
    pub slope: f64,
    /// Height above sea level, metres.
    pub height_m: f64,
    /// Tier A rock hardness, 0..1.
    pub hardness: f64,
    /// Tier A sediment, 0..1.
    pub sediment: f64,
    /// Distance to the nearest water, metres.
    pub water_distance_m: f64,
    /// Exposed rock fraction (bedrock + scree weight), 0..1.
    pub rock_fraction: f64,
}

/// Suitability times peak density in 0..1 (patch noise excluded; see
/// [`patch_factor`]).
pub fn density(species: &Species, s: &SiteInputs) -> f64 {
    // Underwater or in the water channel: nothing grows (no aquatic species yet).
    if s.height_m <= 0.0 || s.water_distance_m < 2.0 {
        return 0.0;
    }
    let slope_ok = Range::new(0.0, species.max_slope, species.slope_falloff).eval(s.slope);
    let env = species.temperature_c.eval(s.temperature_c)
        * species.moisture.eval(s.moisture)
        * species.height_m.eval(s.height_m)
        * slope_ok;
    let a = species.rock_affinity;
    let rock = s.rock_fraction.clamp(0.0, 1.0);
    let rock_factor = if a < 0.0 {
        1.0 + a * rock
    } else {
        // Rock lovers: scale by exposed rock, boosted by hard bedrock.
        (1.0 - a) + a * rock * (0.4 + 0.6 * s.hardness.clamp(0.0, 1.0))
    };
    let soil = if species.soil_min > 0.0 {
        ((s.sediment - species.soil_min) / 0.15 + 1.0).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let water = if s.water_distance_m <= species.water_within_m {
        1.0 + species.water_bonus
    } else {
        1.0
    };
    (species.base_density * env * rock_factor * soil * water).clamp(0.0, 1.0)
}

/// One jittered candidate of a cell, before the site is known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub face: u32,
    pub cell_i: u32,
    pub cell_j: u32,
    pub level: u32,
    pub slot: u32,
    /// Face chart coordinates, always inside the owning cell.
    pub u: f64,
    pub v: f64,
    /// Species chosen by the weighted prior hash.
    pub species: usize,
    /// Stable thinning rank in [0, 1): kept at LOD `l` iff `rank < lod_keep(l)`.
    pub rank: f64,
    /// Acceptance roll in [0, 1) compared against the density.
    pub accept_roll: f64,
    /// Random yaw, radians.
    pub yaw: f64,
    /// Scale in the species range.
    pub scale: f64,
}

impl Candidate {
    /// Instance key `(level, face, i, j, slot)` packed for the `removed` set (§12.6).
    pub fn key(&self) -> u64 {
        ((self.level as u64) << 56)
            | ((self.face as u64) << 52)
            | ((self.cell_i as u64) << 28)
            | ((self.cell_j as u64) << 4)
            | self.slot as u64
    }
}

/// Fraction of candidates kept at LOD `lod` (0 = finest, keeps all).
pub fn lod_keep(lod: u32) -> f64 {
    0.5f64.powi(lod.min(30) as i32)
}

/// Candidates of one cell. `cell_size` is the cell width in chart units
/// (`2 / cells_per_face_edge`); `level` salts the hash per scatter tier.
pub fn place_cell(
    seed: u32,
    face: u32,
    cell_i: u32,
    cell_j: u32,
    level: u32,
    cell_size: f64,
) -> Vec<Candidate> {
    let total: f64 = SPECIES.iter().map(|s| s.prior_weight).sum();
    let mut out = Vec::with_capacity(SLOTS_PER_CELL as usize);
    for slot in 0..SLOTS_PER_CELL {
        let base = [
            cell_i ^ seed,
            cell_j ^ seed.rotate_left(13) ^ (face << 24),
            level.wrapping_mul(0x9e37_79b9) ^ slot ^ (face << 8),
        ];
        let h0 = pcg3d(base);
        let h1 = pcg3d([h0[0], h0[1] ^ 0x5bd1_e995, h0[2]]);
        // Jitter strictly inside the cell (24-bit fraction < 1).
        let u = -1.0 + (cell_i as f64 + unit(h0[0])) * cell_size;
        let v = -1.0 + (cell_j as f64 + unit(h0[1])) * cell_size;
        let pick = unit(h0[2]) * total;
        let mut acc = 0.0;
        let mut species = SPECIES_COUNT - 1;
        for (k, sp) in SPECIES.iter().enumerate() {
            acc += sp.prior_weight;
            if pick < acc {
                species = k;
                break;
            }
        }
        let sp = &SPECIES[species];
        out.push(Candidate {
            face,
            cell_i,
            cell_j,
            level,
            slot,
            u,
            v,
            species,
            rank: unit(h1[0]),
            accept_roll: unit(h1[1]),
            yaw: unit(h1[2]) * 2.0 * PI,
            scale: sp.scale.0 + (sp.scale.1 - sp.scale.0) * unit(h0[2].rotate_left(11) ^ h1[0]),
        });
    }
    out
}

/// Position and shape of a forest/clearing patch lookup.
#[derive(Debug, Clone, Copy)]
pub struct PatchQuery {
    pub seed: u32,
    pub face: u32,
    pub species: usize,
    pub u: f64,
    pub v: f64,
    pub metres_per_unit: f64,
    pub scale_m: f64,
    pub coverage: f64,
}

/// Forest/clearing factor 0..1 from smooth value noise on a per-face lattice
/// of `scale_m` cells. PROTOTYPE: the lattice restarts per face, so patches
/// are discontinuous across face edges (harden with cross-face lattice ids).
pub fn patch_factor(q: &PatchQuery) -> f64 {
    let x = q.u * q.metres_per_unit / q.scale_m;
    let y = q.v * q.metres_per_unit / q.scale_m;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let corner = |dx: i32, dy: i32| {
        let h = pcg3d([
            (x0 as i32 + dx) as u32 ^ q.seed,
            (y0 as i32 + dy) as u32 ^ (q.face << 24),
            0x7a7c_0000 ^ q.species as u32,
        ]);
        unit(h[0])
    };
    let s = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let a = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
    let b = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
    let n = a + (b - a) * sy;
    // Noise above (1 - coverage) is forest; soft edge of 0.1.
    let t = ((n - (1.0 - q.coverage)) / 0.1 + 0.5).clamp(0.0, 1.0);
    s(t)
}

/// An accepted instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instance {
    pub candidate: Candidate,
}

/// Accept a candidate given its site and patch factor: the roll must be
/// below `density * patch`. Returns the instance when accepted.
pub fn accept(c: &Candidate, site: &SiteInputs, patch: f64) -> Option<Instance> {
    let d = density(&SPECIES[c.species], site) * patch.clamp(0.0, 1.0);
    (c.accept_roll < d).then_some(Instance { candidate: *c })
}

/// Whether two instances (positions in chart units) respect the spacing rule:
/// centres at least the sum of footprints apart (PROTOTYPE: 0.8 of the sum for
/// the same species so groves can pack).
pub fn spacing_ok(a: &Instance, b: &Instance, metres_per_unit: f64) -> bool {
    let (ca, cb) = (&a.candidate, &b.candidate);
    let fa = SPECIES[ca.species].footprint_m * ca.scale;
    let fb = SPECIES[cb.species].footprint_m * cb.scale;
    let mut need = fa + fb;
    if ca.species == cb.species {
        need *= 0.8;
    }
    let d = ((ca.u - cb.u).powi(2) + (ca.v - cb.v).powi(2)).sqrt() * metres_per_unit;
    d >= need
}

/// Priority for competition: bigger footprint wins, then the lower key.
fn wins(a: &Instance, b: &Instance) -> bool {
    let fa = SPECIES[a.candidate.species].footprint_m * a.candidate.scale;
    let fb = SPECIES[b.candidate.species].footprint_m * b.candidate.scale;
    fa > fb || (fa == fb && a.candidate.key() < b.candidate.key())
}

/// Drop every instance that violates the spacing rule against a winner.
/// Order independent. Run on the full set before LOD thinning.
pub fn resolve_spacing(instances: &[Instance], metres_per_unit: f64) -> Vec<Instance> {
    instances
        .iter()
        .filter(|a| {
            !instances.iter().any(|b| {
                a.candidate.key() != b.candidate.key()
                    && !spacing_ok(a, b, metres_per_unit)
                    && wins(b, a)
            })
        })
        .copied()
        .collect()
}

/// LOD thinning: keep instances whose rank is below the LOD keep fraction.
pub fn thin(instances: &[Instance], lod: u32) -> Vec<Instance> {
    let keep = lod_keep(lod);
    instances
        .iter()
        .filter(|i| i.candidate.rank < keep)
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> SiteInputs {
        SiteInputs {
            temperature_c: 8.0,
            moisture: 0.7,
            slope: 0.1,
            height_m: 300.0,
            hardness: 0.3,
            sediment: 0.6,
            water_distance_m: 500.0,
            rock_fraction: 0.05,
        }
    }

    #[test]
    fn placement_is_deterministic() {
        let a = place_cell(7, 2, 31, 12, 1, 2.0 / 1024.0);
        let b = place_cell(7, 2, 31, 12, 1, 2.0 / 1024.0);
        assert_eq!(a, b);
        assert_ne!(a, place_cell(8, 2, 31, 12, 1, 2.0 / 1024.0));
    }

    #[test]
    fn candidates_stay_in_their_cell() {
        let size = 2.0 / 64.0;
        for face in 0..6 {
            for &(i, j) in &[(0, 0), (63, 63), (0, 63), (63, 0), (10, 20)] {
                for c in place_cell(3, face, i, j, 0, size) {
                    let (lu, lv) = (-1.0 + i as f64 * size, -1.0 + j as f64 * size);
                    assert!(c.u >= lu && c.u < lu + size, "u outside cell");
                    assert!(c.v >= lv && c.v < lv + size, "v outside cell");
                    assert!(c.u >= -1.0 && c.u < 1.0 && c.v >= -1.0 && c.v < 1.0);
                }
            }
        }
    }

    #[test]
    fn face_edge_cells_are_owned_once() {
        // Neighbouring cells across a face edge are different (face, i, j)
        // keys, so no candidate can be emitted twice.
        let size = 2.0 / 64.0;
        let a = place_cell(5, 0, 63, 10, 0, size);
        let b = place_cell(5, 1, 0, 10, 0, size);
        for ca in &a {
            for cb in &b {
                assert_ne!(ca.key(), cb.key());
            }
        }
    }

    const MPU: f64 = 128.0 * 3.0;

    fn field(lod: u32, seed: u32) -> Vec<Instance> {
        let size = 2.0 / 128.0;
        let mut all = Vec::new();
        let mut s = site();
        s.rock_fraction = 0.3;
        for i in 0..40 {
            for j in 0..40 {
                for c in place_cell(seed, 0, i, j, 0, size) {
                    if let Some(inst) = accept(&c, &s, 1.0) {
                        all.push(inst);
                    }
                }
            }
        }
        thin(&resolve_spacing(&all, MPU), lod)
    }

    #[test]
    fn coarse_lod_is_subset_of_fine() {
        let fine = field(0, 11);
        assert!(fine.len() > 50);
        let keys = |v: &[Instance]| v.iter().map(|i| i.candidate.key()).collect::<Vec<_>>();
        let mut prev = keys(&fine);
        for lod in 1..5 {
            let cur = keys(&field(lod, 11));
            assert!(cur.iter().all(|k| prev.contains(k)), "lod {lod} not subset");
            assert!(cur.len() <= prev.len());
            prev = cur;
        }
    }

    #[test]
    fn spacing_is_respected_and_order_independent() {
        let mut v = field(0, 4);
        for (n, a) in v.iter().enumerate() {
            for b in &v[n + 1..] {
                assert!(spacing_ok(a, b, MPU));
            }
        }
        v.reverse();
        assert_eq!(resolve_spacing(&v, MPU).len(), v.len());
    }

    #[test]
    fn tree_density_monotonic_in_moisture() {
        let mut last = 0.0;
        for k in 0..=18 {
            let mut s = site();
            s.moisture = k as f64 * 0.05;
            let d = density(&SPECIES[0], &s);
            assert!(d >= last - 1e-12, "moisture {} dropped", s.moisture);
            last = d;
        }
        assert!(last > 0.3);
    }

    #[test]
    fn no_trees_above_treeline_or_on_steep_rock() {
        let mut s = site();
        s.height_m = 3500.0;
        assert_eq!(density(&SPECIES[0], &s), 0.0);
        let mut s = site();
        s.slope = 60.0 * DEG;
        assert_eq!(density(&SPECIES[0], &s), 0.0);
        let mut s = site();
        s.rock_fraction = 1.0;
        s.sediment = 0.0;
        assert_eq!(density(&SPECIES[0], &s), 0.0);
        let mut s = site();
        s.height_m = -5.0;
        assert_eq!(density(&SPECIES[2], &s), 0.0);
    }

    #[test]
    fn boulders_favour_hard_scree() {
        let mut soil = site();
        soil.rock_fraction = 0.02;
        soil.hardness = 0.2;
        let mut rock = site();
        rock.rock_fraction = 0.9;
        rock.hardness = 0.9;
        rock.sediment = 0.0;
        rock.slope = 30.0 * DEG;
        assert!(density(&SPECIES[2], &rock) > 4.0 * density(&SPECIES[2], &soil));
        assert!(density(&SPECIES[0], &rock) < density(&SPECIES[0], &soil));
    }

    #[test]
    fn patch_factor_is_deterministic_and_bounded() {
        let q = PatchQuery {
            seed: 1,
            face: 0,
            species: 0,
            u: 0.123,
            v: -0.4,
            metres_per_unit: 6.0e6,
            scale_m: 700.0,
            coverage: 0.6,
        };
        let a = patch_factor(&q);
        assert_eq!(a, patch_factor(&q));
        assert!((0.0..=1.0).contains(&a));
    }
}
