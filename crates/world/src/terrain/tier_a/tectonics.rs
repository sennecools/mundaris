//! Tier A tectonics (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6.5.1 and §6.5.3; M2
//! design §1), CPU oracle of the GPU `tectonics` pass.
//!
//! Plates are an analytic power (Laguerre) diagram on the sphere: seeds on a
//! jittered Fibonacci lattice plus a few microplates with a negative power
//! weight, assigned by a domain-warped direction `d'`. Plate `i` owns `d'`
//! where its score `s_i = d'·c_i + w_i` is largest. Every boundary is a plane,
//! so the distance to the boundary with neighbour `j` is exactly
//! `δ_ij = R·(s_i − s_j) / |P_d(c_i − c_j)|` to first order (no stencil, one
//! pass, deterministic). Per-neighbour quantities are blended with soft weights
//! `ŵ_j ∝ exp(−(δ_ij − δ_min)/τ')` so outputs stay continuous where the nearest
//! boundary switches; `τ' = min(τ, δ_min)` shrinks at a boundary so only the
//! shared boundary counts there and both sides agree, also near triple
//! junctions. Relative Euler-pole velocity gives convergence and shear and
//! continuous class weights; uplift, trenches, ridges and rifts are
//! closed-form profiles of δ (the doc's "convolve with a falloff" becomes
//! analytic), with seeded roughness on the orogenic relief. Relief comes from
//! every boundary between the `EDGE_CANDIDATES` highest-scoring plates, in
//! any cell: past a boundary's end (a triple junction) its profiles continue
//! as a rounded cap over the distance along the boundary, and smooth maxima
//! merge the relief of boundaries that meet (lane proposal
//! `plate-junctions-PROPOSAL.md`).
//! `boundary_coord = δ_min·Σ ŵ_j·sign(j − i)` is linear through each boundary
//! and replaces `boundary_tangent` (§6.3). (Blending `Σ ŵ_j·sign·δ_ij` as first
//! designed adds km-scale offsets from neighbours a few `τ` away, different on
//! each side of a boundary.)
use super::{fbm, warp_direction};
use crate::terrain::archetype::PlanetParams;
use glam::{DQuat, DVec3};

/// Bytes per plate in the GPU `Plates` uniform.
pub const PLATE_BYTES: usize = 64;

/// One plate of the power diagram.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plate {
    /// Unit seed direction.
    pub centre: DVec3,
    /// Power weight: 0 for major plates, `−micro_shrink` for microplates.
    pub weight: f64,
    /// Euler rotation `ω·axis` (relative angular speed units).
    pub spin: DVec3,
    pub continental: bool,
    /// Rock-family hardness (§6.5.3).
    pub hardness: f64,
}

/// Plates of one body, major plates first, then microplates.
#[derive(Debug, Clone, PartialEq)]
pub struct PlateSet {
    pub plates: Vec<Plate>,
}

impl PlateSet {
    /// Packed `Plates` uniform: per plate `(centre, weight)`, `(spin,
    /// continental 0/1)`, `(hardness, 0, 0, 0)` and a zero vec4.
    pub fn packed(&self) -> Vec<f32> {
        self.plates
            .iter()
            .flat_map(|p| {
                [
                    p.centre.x,
                    p.centre.y,
                    p.centre.z,
                    p.weight,
                    p.spin.x,
                    p.spin.y,
                    p.spin.z,
                    if p.continental { 1.0 } else { 0.0 },
                    p.hardness,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                ]
            })
            .map(|v| v as f32)
            .collect()
    }
}

/// Boundary class of the nearest boundary (`aux1` byte 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum BoundaryClass {
    /// Weak or no relative motion.
    #[default]
    Interior = 0,
    /// Continent–continent convergence.
    Collision = 1,
    /// Ocean–continent convergence.
    Subduction = 2,
    /// Ocean–ocean convergence.
    IslandArc = 3,
    /// Divergence with oceanic crust.
    Ridge = 4,
    /// Continent–continent divergence.
    Rift = 5,
    Transform = 6,
}

/// Tectonic quantities at one texel.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TectonicSample {
    pub plate: u8,
    /// Distance to the nearest boundary (m, `δ_min`).
    pub distance_m: f64,
    /// `δ_min·Σ ŵ_j·sign(j − i)`, clamped (m).
    pub boundary_coord_m: f64,
    /// Elevation change from boundary profiles (m).
    pub dh_m: f64,
    /// Orogenic uplift 0..1.
    pub uplift: f64,
    /// +1 continental, −1 oceanic, blended across boundaries.
    pub crust: f64,
    /// Rock hardness 0..1 (0.5 without tectonics).
    pub hardness: f64,
    pub volcanic: f64,
    pub class: BoundaryClass,
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Uniform `[0, 1)` from the plate seed and a draw index.
fn draw(seed: u32, index: u64) -> f64 {
    (splitmix(splitmix(u64::from(seed)) ^ index) >> 11) as f64 / (1u64 << 53) as f64
}

fn unit_vector(u: f64, v: f64) -> DVec3 {
    let z = 2.0 * u - 1.0;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let phi = std::f64::consts::TAU * v;
    DVec3::new(r * phi.cos(), r * phi.sin(), z)
}

/// Plates of a body (M2 design §1): `plate_count` seeds on a Fibonacci lattice
/// rotated by a seed-drawn rotation, each jittered by `jitter·π/√N·u` along a
/// hashed tangent; microplates at hashed directions with weight
/// `−micro_shrink`. Per plate a hashed Euler axis, speed `plate_speed·(0.5 +
/// u)`, continental with probability `continental_fraction`; oceanic plates
/// are volcanic rock, continental ones crystalline or sedimentary.
pub fn plates(params: &PlanetParams, seed: u32) -> PlateSet {
    let majors = params.major_plates();
    let micros = params.microplates();
    // Shoemake: a uniform random rotation from three uniforms.
    let (u1, u2, u3) = (draw(seed, 1), draw(seed, 2), draw(seed, 3));
    let tau = std::f64::consts::TAU;
    let rotation = DQuat::from_xyzw(
        (1.0 - u1).sqrt() * (tau * u2).sin(),
        (1.0 - u1).sqrt() * (tau * u2).cos(),
        u1.sqrt() * (tau * u3).sin(),
        u1.sqrt() * (tau * u3).cos(),
    )
    .normalize();
    let golden = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    let jitter = params.plate_jitter * std::f64::consts::PI / (majors as f64).sqrt();
    let mut plates = Vec::with_capacity(majors + micros);
    for index in 0..majors + micros {
        let base = 16 * (index as u64 + 1);
        let centre = if index < majors {
            let z = 1.0 - (2.0 * index as f64 + 1.0) / majors as f64;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let phi = golden * index as f64;
            let p = DVec3::new(r * phi.cos(), r * phi.sin(), z);
            let t1 = p.any_orthonormal_vector();
            let t2 = p.cross(t1);
            let theta = tau * draw(seed, base);
            let tangent = t1 * theta.cos() + t2 * theta.sin();
            let angle = jitter * draw(seed, base + 1);
            rotation * (p * angle.cos() + tangent * angle.sin()).normalize()
        } else {
            unit_vector(draw(seed, base), draw(seed, base + 1))
        };
        let axis = unit_vector(draw(seed, base + 2), draw(seed, base + 3));
        let omega = params.plate_speed * (0.5 + draw(seed, base + 4));
        let continental = draw(seed, base + 5) < params.continental_fraction;
        let hardness = if !continental {
            params.hardness_volcanic
        } else if draw(seed, base + 6) < 0.5 {
            params.hardness_crystalline
        } else {
            params.hardness_sedimentary
        };
        plates.push(Plate {
            centre: centre.normalize(),
            weight: if index < majors {
                0.0
            } else {
                -params.micro_shrink
            },
            spin: axis * omega,
            continental,
            hardness,
        });
    }
    PlateSet { plates }
}

/// `(1 − x²)²` on `|x| < 1`, else 0.
pub fn bump(x: f64) -> f64 {
    if x.abs() < 1.0 {
        let y = 1.0 - x * x;
        y * y
    } else {
        0.0
    }
}

pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Convergence and divergence weights saturate at this relative speed.
const CLASS_RAMP: (f64, f64) = (0.05, 0.4);

/// Plate-assignment direction: `d` warped by 3-octave fBm (the M1 continent
/// warp) at the plate warp wavelength.
pub fn plate_direction(d: DVec3, params: &PlanetParams, radius_m: f64) -> DVec3 {
    if params.plate_warp_strength <= 0.0 {
        return d;
    }
    warp_direction(
        d,
        radius_m / params.plate_warp_wavelength_m,
        params.plate_warp_strength * params.plate_warp_wavelength_m / radius_m,
        params.tectonic_seed,
    )
}

/// Seed offset of the orogen roughness noise from the tectonic seed (the
/// plate warp uses offsets 0, 16 and 32).
pub const ROUGHNESS_SEED_OFFSET: u32 = 64;
/// Octaves of the orogen roughness noise.
pub const ROUGHNESS_OCTAVES: u32 = 4;

/// Zero-mean roughness noise (about ±0.6) modulating orogenic relief.
pub fn orogen_noise(d: DVec3, params: &PlanetParams, radius_m: f64) -> f64 {
    fbm(
        d,
        radius_m / params.orogen_roughness_wavelength_m,
        ROUGHNESS_OCTAVES,
        2.0,
        0.5,
        params.tectonic_seed.wrapping_add(ROUGHNESS_SEED_OFFSET),
    )
}

/// Continuous class weights of one boundary: convergent, divergent,
/// transform.
#[derive(Debug, Clone, Copy)]
struct Motion {
    conv: f64,
    div: f64,
    trans: f64,
}

impl Motion {
    /// Class weights at warped direction `w` from the relative velocity of
    /// plate `a` against `b`; `tangent` is `P_w(c_a − c_b)` (the normal
    /// towards `b` is its negative).
    fn new(w: DVec3, a: &Plate, b: &Plate, tangent: DVec3) -> Self {
        let normal = -tangent / tangent.length().max(1e-12);
        let v = (a.spin - b.spin).cross(w);
        let c = v.dot(normal);
        let t = v.dot(w.cross(normal)).abs();
        let conv = smoothstep(CLASS_RAMP.0, CLASS_RAMP.1, c);
        let div = smoothstep(CLASS_RAMP.0, CLASS_RAMP.1, -c);
        let trans = smoothstep(CLASS_RAMP.0, CLASS_RAMP.1, t) * (1.0 - conv - div);
        Self { conv, div, trans }
    }
}

/// Plates whose boundaries shape the relief at a point: the highest power
/// scores (ties to the lower index).
pub const EDGE_CANDIDATES: usize = 6;
/// Smooth-maximum temperature (m) where the relief of several boundaries
/// meets at a junction (prototype constant for the archetype's
/// `junction_blend_m`).
pub const JUNCTION_BLEND_M: f64 = 400.0;
/// Smooth-maximum temperature of the volcanic weight (0..1).
pub const VOLCANIC_BLEND: f64 = 0.1;

/// Smooth maximum of non-negative `values` with temperature `t`:
/// `m + t·ln(Σᵢ(e^{(vᵢ−m)/t} − e^{−m/t}) + e^{−m/t})`, `m` the largest. Exact
/// for one non-zero value (zeros add nothing), at most `t·ln(count)` above
/// `m`, smooth where values meet.
pub fn smooth_max(values: &[f64], t: f64) -> f64 {
    let m = values.iter().copied().fold(0.0, f64::max);
    if m <= 0.0 {
        return 0.0;
    }
    let floor = (-m / t).exp();
    let sum: f64 = values.iter().map(|v| ((v - m) / t).exp() - floor).sum();
    m + t * (sum + floor).ln()
}

/// Distance (m) along the boundary of plates `a` and `b` past its end at
/// warped direction `w`: 0 while `a` and `b` own the foot point on their
/// bisector plane, else how far the foot point lies inside another
/// candidate's cell along the boundary (linearised power margin over its rate
/// along the boundary). Symmetric in `a` and `b`.
fn beyond_end(
    w: DVec3,
    plates: &[Plate],
    (a, b): (usize, usize),
    candidates: &[usize],
    radius_m: f64,
) -> f64 {
    let (pa, pb) = (&plates[a], &plates[b]);
    let dc = pa.centre - pb.centre;
    let along = w.dot(dc);
    let f = along + (pa.weight - pb.weight);
    let tangent = dc - w * along;
    let foot = (w - tangent * (f / tangent.length_squared().max(1e-24))).normalize();
    let score = (foot.dot(pa.centre) + pa.weight).max(foot.dot(pb.centre) + pb.weight);
    let normal = dc - foot * foot.dot(dc);
    let edge = foot.cross(normal).normalize_or_zero();
    let middle = 0.5 * (pa.centre + pb.centre);
    let mut beyond = 0.0f64;
    for &k in candidates {
        if k == a || k == b {
            continue;
        }
        let pk = &plates[k];
        let margin = foot.dot(pk.centre) + pk.weight - score;
        if margin > 0.0 {
            let g = pk.centre - middle;
            let g = g - foot * foot.dot(g);
            let rate = g.dot(edge).abs().max(0.2 * g.length()).max(1e-12);
            beyond = beyond.max(radius_m * margin / rate);
        }
    }
    beyond
}

/// Profiles of one boundary for the plate `(i, a)` on whose side the point
/// lies and the other plate `(j, b)`: (dh, positive orogenic relief,
/// volcanic, class). `delta` (m) is the distance to their bisector plane,
/// `beyond` (m) the distance along the boundary past its end (0 where the
/// boundary exists). Profiles even in the signed distance use the capsule
/// distance `√(δ² + beyond²)`, so belts end in rounded caps; the one-sided
/// arc fades by `bump(beyond / arc width)` instead, which keeps it
/// continuous where the two sides meet beyond the end.
fn profiles(
    (i, a): (usize, &Plate),
    (j, b): (usize, &Plate),
    delta: f64,
    beyond: f64,
    motion: Motion,
    p: &PlanetParams,
) -> (f64, f64, f64, BoundaryClass) {
    let Motion { conv, div, trans } = motion;
    let mut dh = 0.0;
    let mut orogenic = 0.0;
    let mut volcanic = 0.0;
    let arc = bump((delta - p.arc_offset_m) / p.arc_width_m) * bump(beyond / p.arc_width_m);
    let delta = delta.hypot(beyond);
    let trench = -p.trench_depth_m * conv * bump(delta / p.trench_width_m);
    // Convergent: belts, coastal ranges and island arcs on the overriding
    // side; trenches straddle ocean-continent and ocean-ocean boundaries (the
    // same value on both sides, so the relief is continuous).
    match (a.continental, b.continental) {
        (true, true) => {
            let belt = p.collision_height_m * conv * bump(delta / p.orogen_width_m);
            dh += belt;
            orogenic += belt;
        }
        (true, false) => {
            dh += p.arc_height_m * conv * arc + trench;
            orogenic += p.arc_height_m * conv * arc;
            volcanic = conv * arc;
        }
        (false, true) => dh += trench,
        (false, false) => {
            dh += trench;
            // The lower index subducts; the other side carries the arc.
            if i > j {
                dh += p.arc_height_m * conv * arc;
                orogenic += p.arc_height_m * conv * arc;
                volcanic = conv * arc;
            }
        }
    }
    // Divergent: continental rifts, mid-ocean ridges; passive margins between
    // continental and oceanic crust stay flat.
    match (a.continental, b.continental) {
        (true, true) => dh -= p.rift_depth_m * div * bump(delta / p.rift_width_m),
        (false, false) => dh += p.ridge_height_m * div * (-delta / p.ridge_width_m).exp(),
        _ => {}
    }
    let transform = p.transform_height_m * trans * bump(delta / p.transform_width_m);
    dh += transform;
    orogenic += transform;
    let class = if conv.max(div).max(trans) < 0.25 {
        BoundaryClass::Interior
    } else if conv >= div && conv >= trans {
        match (a.continental, b.continental) {
            (true, true) => BoundaryClass::Collision,
            (false, false) => BoundaryClass::IslandArc,
            _ => BoundaryClass::Subduction,
        }
    } else if div >= trans {
        if a.continental && b.continental {
            BoundaryClass::Rift
        } else {
            BoundaryClass::Ridge
        }
    } else {
        BoundaryClass::Transform
    };
    (dh, orogenic, volcanic, class)
}

/// Tectonic quantities at unit direction `d`.
pub fn tectonic_sample(
    d: DVec3,
    set: &PlateSet,
    p: &PlanetParams,
    radius_m: f64,
) -> TectonicSample {
    let plates = &set.plates;
    let w = plate_direction(d, p, radius_m);
    // Own plate: largest power score (ties to the lower index).
    let mut own = 0;
    let mut best = f64::NEG_INFINITY;
    for (k, plate) in plates.iter().enumerate() {
        let s = w.dot(plate.centre) + plate.weight;
        if s > best {
            best = s;
            own = k;
        }
    }
    let a = &plates[own];
    // Boundary distances to every other plate.
    let deltas: Vec<(f64, DVec3)> = plates
        .iter()
        .map(|b| {
            let dc = a.centre - b.centre;
            let along = w.dot(dc);
            let f = along + (a.weight - b.weight);
            let tangent = dc - w * along;
            (radius_m * f / tangent.length().max(1e-12), tangent)
        })
        .collect();
    let delta_min = deltas
        .iter()
        .enumerate()
        .filter(|(k, _)| *k != own)
        .map(|(_, (delta, _))| *delta)
        .fold(f64::INFINITY, f64::min);
    // Softness shrinks to the boundary distance near a boundary, so only the
    // shared boundary counts there and both sides agree (continuous dh,
    // crust and hardness at every boundary, also near triple junctions).
    let tau = p.boundary_softness_m.min(delta_min).max(1.0);
    let mut total = 0.0;
    let mut out = TectonicSample {
        plate: own as u8,
        distance_m: delta_min,
        ..Default::default()
    };
    let (mut coord, mut crust, mut hardness) = (0.0, 0.0, 0.0);
    let mut nearest = (f64::INFINITY, BoundaryClass::Interior);
    let crust_of = |plate: &Plate| if plate.continental { 1.0 } else { -1.0 };
    for (j, b) in plates.iter().enumerate() {
        if j == own {
            continue;
        }
        let (delta, tangent) = deltas[j];
        let weight = (-(delta - delta_min) / tau).exp();
        let blend = 0.5 * (1.0 - smoothstep(0.0, p.crust_width_m, delta));
        let sign = if j > own { 1.0 } else { -1.0 };
        total += weight;
        coord += weight * sign;
        crust += weight * (crust_of(a) + (crust_of(b) - crust_of(a)) * blend);
        hardness += weight * (a.hardness + (b.hardness - a.hardness) * blend);
        if delta < nearest.0 {
            let motion = Motion::new(w, a, b, tangent);
            nearest = (delta, profiles((own, a), (j, b), delta, 0.0, motion, p).3);
        }
    }
    let total = total.max(1e-300);
    // Relief from every boundary between the candidate plates, evaluated in
    // any cell (belts end in caps past a triple junction instead of at the
    // third plate's cell edge); smooth maxima merge where boundaries meet.
    let mut ranked: Vec<(f64, usize)> = plates
        .iter()
        .enumerate()
        .map(|(k, plate)| (w.dot(plate.centre) + plate.weight, k))
        .collect();
    ranked.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
    let candidates: Vec<usize> = ranked.iter().take(EDGE_CANDIDATES).map(|r| r.1).collect();
    let (mut raised, mut lowered, mut orogenic, mut volcanic) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (x, &(score_x, i)) in ranked.iter().take(EDGE_CANDIDATES).enumerate() {
        for &(score_y, j) in &ranked[x + 1..candidates.len()] {
            // The side of the point: the higher score (ranked first on ties).
            debug_assert!(score_x >= score_y);
            let (pa, pb) = (&plates[i], &plates[j]);
            let dc = pa.centre - pb.centre;
            let along = w.dot(dc);
            let tangent = dc - w * along;
            let delta = radius_m * (along + pa.weight - pb.weight) / tangent.length().max(1e-12);
            let beyond = beyond_end(w, plates, (i, j), &candidates, radius_m);
            let motion = Motion::new(w, pa, pb, tangent);
            let (h, o, vol, _) = profiles((i, pa), (j, pb), delta, beyond, motion, p);
            raised.push(h.max(0.0));
            lowered.push((-h).max(0.0));
            orogenic.push(o);
            volcanic.push(vol);
        }
    }
    // Roughness of the orogenic relief (belts, arcs): seeds the drainage
    // network that erosion then incises.
    let rough = p.orogen_roughness * orogen_noise(d, p, radius_m);
    let orogenic = smooth_max(&orogenic, JUNCTION_BLEND_M);
    let dh = smooth_max(&raised, JUNCTION_BLEND_M) - smooth_max(&lowered, JUNCTION_BLEND_M);
    out.dh_m = dh + rough * orogenic;
    out.uplift = (orogenic * (1.0 + rough) / p.collision_height_m.max(1.0)).clamp(0.0, 1.0);
    out.volcanic = smooth_max(&volcanic, VOLCANIC_BLEND).clamp(0.0, 1.0);
    // The nearest distance times the soft-weighted side: linear through every
    // boundary (0 on both sides) and continuous where the nearest boundary
    // switches; blending the distances themselves would add km-scale offsets
    // from neighbours a few softness lengths away, different on each side.
    out.boundary_coord_m =
        (delta_min * coord / total).clamp(-p.boundary_clamp_m, p.boundary_clamp_m);
    out.crust = crust / total;
    let noise = fbm(
        d,
        radius_m / p.hardness_noise_wavelength_m,
        3,
        2.0,
        0.5,
        p.hardness_seed,
    );
    out.hardness =
        ((hardness / total).max(0.8 * out.volcanic) + p.hardness_noise * noise).clamp(0.0, 1.0);
    out.class = nearest.1;
    out
}

impl TectonicSample {
    /// A body without tectonics: no relief, neutral hardness.
    pub fn none() -> Self {
        Self {
            hardness: 0.5,
            ..Default::default()
        }
    }
}

impl Default for Plate {
    fn default() -> Self {
        Self {
            centre: DVec3::Z,
            weight: 0.0,
            spin: DVec3::ZERO,
            continental: false,
            hardness: 0.5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{
        archetype::TierAStage,
        tier_a::{bake_full, tests::inputs},
        world_map::neighbours,
    };

    #[test]
    fn plates_are_reproducible_and_spread() {
        let params = inputs(4, 7).params;
        let set = plates(&params, params.tectonic_seed);
        assert_eq!(set, plates(&params, params.tectonic_seed));
        assert_eq!(
            set.plates.len(),
            params.major_plates() + params.microplates()
        );
        assert!(set.plates.iter().any(|p| p.continental));
        assert!(set.plates.iter().any(|p| !p.continental));
        // Major seeds are well separated (jittered lattice).
        let majors = &set.plates[..params.major_plates()];
        let closest = majors
            .iter()
            .enumerate()
            .flat_map(|(a, p)| {
                majors[a + 1..]
                    .iter()
                    .map(move |q| p.centre.angle_between(q.centre))
            })
            .fold(f64::INFINITY, f64::min);
        assert!(closest > 0.25, "{closest}");
        assert_eq!(set.packed().len() * 4, PLATE_BYTES * set.plates.len());
    }

    #[test]
    fn boundary_coord_is_continuous_and_signed_across_boundaries() {
        let params = inputs(4, 7).params;
        let set = plates(&params, params.tectonic_seed);
        let r = 338_950.0;
        // Walk great circles in 200 m steps and find plate changes: the
        // coordinate changes sign there without a jump and dh stays
        // continuous, except within a few softness lengths of triple
        // junctions (soft weights over different neighbour sets).
        let step_m = 200.0;
        let (mut crossings, mut smooth) = (0, 0);
        for k in 0..16 {
            let a = unit_vector(draw(99, 2 * k), draw(99, 2 * k + 1));
            let axis = a.any_orthonormal_vector();
            let mut previous = tectonic_sample(a, &set, &params, r);
            for s in 1..10_000 {
                let d = DQuat::from_axis_angle(axis, step_m / r * s as f64) * a;
                let sample = tectonic_sample(d, &set, &params, r);
                if sample.plate != previous.plate {
                    crossings += 1;
                    let jump = (sample.boundary_coord_m - previous.boundary_coord_m).abs();
                    let flip = sample.boundary_coord_m * previous.boundary_coord_m <= 0.0;
                    if jump < 2.0 * step_m && flip && (sample.dh_m - previous.dh_m).abs() < 50.0 {
                        smooth += 1;
                    }
                }
                previous = sample;
            }
        }
        println!("{smooth} of {crossings} boundary crossings smooth");
        assert!(crossings > 20, "{crossings}");
        assert!(smooth as f64 >= 0.97 * crossings as f64);
    }

    #[test]
    fn ranges_follow_plate_boundaries() {
        // M2 design §6: mean uplift near boundaries exceeds the mean far from
        // them; uplift and hardness stay in range.
        let n = 64;
        let input = inputs(n, 7);
        let output = bake_full(&input).unwrap();
        let d = &output.diagnostics;
        let width = input.params.orogen_width_m;
        let mean = |keep: &dyn Fn(f32) -> bool| {
            let values: Vec<f64> = d
                .boundary_distance
                .data()
                .iter()
                .zip(d.uplift.data())
                .filter(|(delta, _)| keep(**delta))
                .map(|(_, u)| f64::from(*u))
                .collect();
            values.iter().sum::<f64>() / values.len().max(1) as f64
        };
        let near = mean(&|delta| f64::from(delta) < width);
        let far = mean(&|delta| f64::from(delta) > 2.0 * width);
        println!("uplift near boundaries {near:.3}, far {far:.3}");
        assert!(near > far + 0.05);
        assert!(d.hardness.data().iter().all(|h| (0.0..=1.0).contains(h)));
        // Ocean coverage stays exact with tectonic relief.
        assert!((output.fields.ocean_fraction - input.params.ocean_coverage).abs() < 0.01);
        // Plates are contiguous: most texels have all four neighbours on the
        // same plate.
        let plate = |k: usize| output.shape.aux1.data()[k] & 0xff;
        let interior = (0..6 * n * n)
            .filter(|&k| neighbours(k, n).iter().all(|&m| plate(m) == plate(k)))
            .count();
        assert!(interior as f64 > 0.8 * (6 * n * n) as f64);
        assert!(input.stages.contains(&TierAStage::Tectonics));
    }

    /// Triple-junction scorer: uplift gradients near junctions against the
    /// flanks of belts away from them.
    #[derive(Debug)]
    pub(crate) struct JunctionScore {
        pub junctions: usize,
        /// 95th percentile |∇uplift| per km on belt flanks > 2 orogen widths
        /// from any junction.
        pub flank_p95: f64,
        /// Largest |∇uplift| per km within one orogen width of a junction.
        pub junction_max: f64,
        /// Junctions whose neighbourhood exceeds twice the flank p95.
        pub wedges: usize,
    }

    pub(crate) fn junction_score(n: usize, seed: u64) -> JunctionScore {
        let input = inputs(n, seed);
        let (p, r) = (&input.params, input.radius_m);
        let set = plates(p, p.tectonic_seed);
        let directions = crate::terrain::world_map::texel_directions(n);
        let samples: Vec<TectonicSample> = directions
            .iter()
            .map(|d| tectonic_sample(*d, &set, p, r))
            .collect();
        let texel_km = r * std::f64::consts::FRAC_PI_2 / n as f64 / 1000.0;
        let gradient: Vec<f64> = (0..samples.len())
            .map(|k| {
                neighbours(k, n)
                    .iter()
                    .map(|&m| (samples[k].uplift - samples[m].uplift).abs())
                    .fold(0.0, f64::max)
                    / texel_km
            })
            .collect();
        let junctions: Vec<DVec3> = (0..samples.len())
            .filter(|&k| {
                let mut ids = vec![samples[k].plate];
                for m in neighbours(k, n) {
                    for q in neighbours(m, n) {
                        ids.push(samples[q].plate);
                    }
                }
                ids.sort_unstable();
                ids.dedup();
                ids.len() >= 3
            })
            .map(|k| directions[k])
            .collect();
        let width = p.orogen_width_m / r;
        let nearest = |d: DVec3| {
            junctions
                .iter()
                .map(|j| d.angle_between(*j))
                .fold(f64::INFINITY, f64::min)
        };
        let mut flanks = Vec::new();
        let mut near = vec![0.0f64; junctions.len()];
        for (k, d) in directions.iter().enumerate() {
            let distance = nearest(*d);
            if distance > 2.0 * width && samples[k].uplift > 0.05 {
                flanks.push(gradient[k]);
            } else if distance < width {
                for (j, value) in junctions.iter().zip(near.iter_mut()) {
                    if d.angle_between(*j) < width {
                        *value = value.max(gradient[k]);
                    }
                }
            }
        }
        flanks.sort_by(f64::total_cmp);
        // No belts away from junctions (NaN): nothing to compare against.
        let flank_p95 = flanks
            .get(flanks.len() * 95 / 100)
            .copied()
            .unwrap_or(f64::NAN);
        JunctionScore {
            junctions: junctions.len(),
            flank_p95,
            junction_max: near.iter().copied().fold(0.0, f64::max),
            wedges: near.iter().filter(|&&g| g > 2.0 * flank_p95).count(),
        }
    }

    #[test]
    fn belts_end_smoothly_at_triple_junctions() {
        // Before the boundary caps: junction maxima 8.2× and 6.4× the flank
        // p95 on seeds 7 and 11 (33 and 53 wedge texels); after: below 1.8×.
        for seed in [7, 11, 23] {
            let score = junction_score(128, seed);
            println!("seed {seed}: {score:?}");
            assert!(score.junctions > 50, "{score:?}");
            assert_eq!(score.wedges, 0, "seed {seed}: {score:?}");
            assert!(score.junction_max < 2.0 * score.flank_p95, "seed {seed}: {score:?}");
        }
    }
}
