//! Alien body plans (user direction 2026-10-10, reference line-up): caps,
//! coral, anemones, gourds, bladders, frond fans, flower stalks and crystals,
//! next to the tree/shrub grower. Each plan is built by rules — radial and
//! phyllotactic placement, taper, droop under gravity, lathe profiles with
//! ribs, dichotomous branching — from one shared [`PlanGenome`], never from
//! hand-modelled meshes. Three LODs by resolution.

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::genome::Look;
use crate::hash::{Stream, domain};
use crate::mesh::{LOD_COUNT, Mesh, class};
use crate::params::{ParamDesc, ParamKind, ParamValue, Params};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyPlan {
    /// Stalks with dome/umbrella caps, gills underneath, spots.
    Cap,
    /// Thick dichotomous branching with swollen tips.
    Coral,
    /// Radial tentacles on a domed base.
    Anemone,
    /// Ribbed gourd/pod bodies with a crown of spikes.
    Gourd,
    /// Balloon bladders on thin stalks.
    Bladder,
    /// Rosette of large curved fronds.
    FrondFan,
    /// Tall stalks with petal whorls and a radial terminal flower.
    FlowerStalk,
    /// Faceted prism clusters with pointed tips.
    Crystal,
}

impl BodyPlan {
    pub const ALL: &'static [BodyPlan] = &[
        BodyPlan::Cap,
        BodyPlan::Coral,
        BodyPlan::Anemone,
        BodyPlan::Gourd,
        BodyPlan::Bladder,
        BodyPlan::FrondFan,
        BodyPlan::FlowerStalk,
        BodyPlan::Crystal,
    ];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap()
    }
}

/// Shared genome of the alien body plans; each plan reads the fields it
/// needs. Lengths in metres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanGenome {
    pub plan: BodyPlan,
    pub height_m: f64,
    /// Heads/stems/bodies per individual.
    pub heads: u32,
    pub head_radius_m: f64,
    pub stalk_radius_m: f64,
    /// Bend under gravity, −1 (upright, stiff) .. 1 (drooping).
    pub droop: f64,
    /// Lean of heads away from the centre, 0..1.
    pub lean: f64,
    /// Twist along stalks/tentacles, turns.
    pub twist: f64,
    pub ribs: u32,
    /// Rib depth as a fraction of the radius, 0..0.5.
    pub rib_depth: f64,
    /// Stacked units / branching depth / stalk nodes.
    pub segments: u32,
    pub spikes: u32,
    pub spike_length_m: f64,
    pub petals: u32,
    pub petal_length_m: f64,
    /// Petal width relative to its length.
    pub petal_width: f64,
    /// Profile fullness: < 1 slender, > 1 swollen.
    pub bulge: f64,
    /// Rim/top flare, 0..1.
    pub flare: f64,
    /// Knob at tips, relative to tip radius (0 none).
    pub tip_knob: f64,
    /// Accent pattern amount (spots/stripes), 0..1.
    pub pattern: f64,
}

macro_rules! plan_float {
    ($key:ident, $group:literal, $unit:literal, $help:literal, $min:expr, $max:expr) => {
        ParamDesc {
            key: stringify!($key),
            label: "",
            group: $group,
            unit: $unit,
            help: $help,
            kind: ParamKind::Float { min: $min, max: $max, log: false },
            get: |g: &PlanGenome| ParamValue::Float(g.$key),
            set: |g: &mut PlanGenome, v| g.$key = v.as_f64(),
        }
    };
}
macro_rules! plan_int {
    ($key:ident, $group:literal, $help:literal, $min:expr, $max:expr) => {
        ParamDesc {
            key: stringify!($key),
            label: "",
            group: $group,
            unit: "",
            help: $help,
            kind: ParamKind::Int { min: $min, max: $max },
            get: |g: &PlanGenome| ParamValue::Int(g.$key as i64),
            set: |g: &mut PlanGenome, v| {
                if let ParamValue::Int(i) = v {
                    g.$key = i as u32
                }
            },
        }
    };
}

const PLANS: &[&str] = &["Cap", "Coral", "Anemone", "Gourd", "Bladder", "FrondFan", "FlowerStalk", "Crystal"];

static PLAN_PARAMS: std::sync::LazyLock<Vec<ParamDesc<PlanGenome>>> = std::sync::LazyLock::new(|| {
    vec![
        ParamDesc {
            key: "plan",
            label: "",
            group: "Body",
            unit: "",
            help: "Body plan",
            kind: ParamKind::Choice { options: PLANS },
            get: |g: &PlanGenome| ParamValue::Choice(g.plan.index()),
            set: |g: &mut PlanGenome, v| {
                if let ParamValue::Choice(i) = v {
                    g.plan = BodyPlan::ALL[i]
                }
            },
        },
        plan_float!(height_m, "Body", "m", "Overall height", 0.05, 30.0),
        plan_int!(heads, "Body", "Heads, stems or bodies", 1, 12),
        plan_float!(head_radius_m, "Body", "m", "Head/body radius", 0.02, 10.0),
        plan_float!(stalk_radius_m, "Body", "m", "Stalk radius", 0.005, 2.0),
        plan_float!(droop, "Shape", "", "Bend under gravity", -1.0, 1.0),
        plan_float!(lean, "Shape", "", "Lean away from the centre", 0.0, 1.0),
        plan_float!(twist, "Shape", "turns", "Twist along stalks", -2.0, 2.0),
        plan_float!(bulge, "Shape", "", "Profile fullness", 0.3, 3.0),
        plan_float!(flare, "Shape", "", "Rim/top flare", 0.0, 1.0),
        plan_int!(ribs, "Surface", "Ribs", 0, 24),
        plan_float!(rib_depth, "Surface", "", "Rib depth", 0.0, 0.5),
        plan_float!(pattern, "Surface", "", "Spots/stripes", 0.0, 1.0),
        plan_int!(segments, "Parts", "Stacked units / branching depth / nodes", 1, 8),
        plan_int!(spikes, "Parts", "Spikes/tentacles", 0, 120),
        plan_float!(spike_length_m, "Parts", "m", "Spike length", 0.01, 5.0),
        plan_int!(petals, "Parts", "Petals/fronds", 0, 40),
        plan_float!(petal_length_m, "Parts", "m", "Petal/frond length", 0.01, 6.0),
        plan_float!(petal_width, "Parts", "", "Petal width / length", 0.05, 1.0),
        plan_float!(tip_knob, "Parts", "", "Tip knob size", 0.0, 4.0),
    ]
});

impl Params for PlanGenome {
    fn descriptors() -> &'static [ParamDesc<Self>] {
        PLAN_PARAMS.as_slice()
    }
}

impl PlanGenome {
    pub fn validate(&self) -> Result<(), &'static str> {
        for d in Self::descriptors() {
            if d.kind.check((d.get)(self)).is_err() {
                return Err(d.key);
            }
        }
        Ok(())
    }
}

/// Triangle budget per LOD for alien plants (medium-poly rule).
pub const PLAN_BUDGET: [usize; LOD_COUNT] = [5000, 1400, 400];

/// Mesh resolution per LOD (multiplies sides and segments).
const RES: [f64; LOD_COUNT] = [1.0, 0.5, 0.28];

fn perp(d: DVec3) -> (DVec3, DVec3) {
    let r = if d.z.abs() < 0.9 { DVec3::Z } else { DVec3::X };
    let u = d.cross(r).normalize();
    (u, d.cross(u))
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f64) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0) as f32;
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Mesh builder with the shared primitives.
struct Builder {
    m: Mesh,
    res: f64,
}

impl Builder {
    fn n(&self, base: f64, min: u32) -> u32 {
        ((base * self.res).round() as u32).max(min)
    }

    /// Tube along `pts` with per-point radius and colour; closed tip.
    fn tube(&mut self, pts: &[DVec3], radii: &[f64], cols: &[[f32; 3]], sides: u32, level: u8) {
        if pts.len() < 2 {
            return;
        }
        let wind = [level, 120, 0, class::ORGAN];
        let d0 = (pts[1] - pts[0]).normalize_or(DVec3::Z);
        let (mut u, _) = perp(d0);
        let mut rings = Vec::new();
        for k in 0..pts.len() {
            let d = if k + 1 < pts.len() { (pts[k + 1] - pts[k.saturating_sub(1)]).normalize_or(d0) } else { (pts[k] - pts[k - 1]).normalize_or(d0) };
            u = (u - d * u.dot(d)).normalize_or(perp(d).0);
            let w = d.cross(u);
            rings.push(self.m.vertices.len() as u32);
            for s in 0..sides {
                let a = s as f64 / sides as f64 * std::f64::consts::TAU;
                let n = u * a.cos() + w * a.sin();
                self.m.push(pts[k] + n * radii[k], n, cols[k], 0.8 + 0.2 * (k as f32 / pts.len() as f32), pts[0], wind);
            }
        }
        for k in 0..rings.len() - 1 {
            let (a, b) = (rings[k], rings[k + 1]);
            for s in 0..sides {
                let s1 = (s + 1) % sides;
                self.m.indices.extend_from_slice(&[a + s, a + s1, b + s1, a + s, b + s1, b + s]);
            }
        }
        let last = *pts.last().unwrap();
        let dl = (last - pts[pts.len() - 2]).normalize_or(d0);
        let apex = self.m.push(last + dl * radii[radii.len() - 1] * 0.6, dl, *cols.last().unwrap(), 1.0, pts[0], wind);
        let lr = *rings.last().unwrap();
        for s in 0..sides {
            self.m.indices.extend_from_slice(&[lr + s, lr + (s + 1) % sides, apex]);
        }
    }

    /// Surface of revolution around `axis` through `base`: `profile` gives
    /// (radius, height) from bottom to top; ribs modulate the radius.
    #[allow(clippy::too_many_arguments)]
    fn lathe(
        &mut self,
        base: DVec3,
        axis: DVec3,
        profile: &[(f64, f64)],
        segs: u32,
        ribs: u32,
        rib_depth: f64,
        color: &dyn Fn(f64, f64) -> [f32; 3],
        level: u8,
    ) {
        let axis = axis.normalize_or(DVec3::Z);
        let (u, w) = perp(axis);
        let wind = [level, 160, 0, class::ORGAN];
        let pos = |r: f64, h: f64, a: f64| {
            let rib = if ribs > 0 { 1.0 - rib_depth * (0.5 - 0.5 * (ribs as f64 * a).cos()) } else { 1.0 };
            base + axis * h + (u * a.cos() + w * a.sin()) * r * rib
        };
        let mut rows = Vec::new();
        let np = profile.len();
        for (i, &(r, h)) in profile.iter().enumerate() {
            rows.push(self.m.vertices.len() as u32);
            let t = i as f64 / (np - 1).max(1) as f64;
            for s in 0..segs {
                let a = s as f64 / segs as f64 * std::f64::consts::TAU;
                // Normal from the surface tangents (profile and around).
                let (r0, h0) = profile[i.saturating_sub(1)];
                let (r1, h1) = profile[(i + 1).min(np - 1)];
                let along = pos(r1, h1, a) - pos(r0, h0, a);
                let around = pos(r, h, a + 0.01) - pos(r, h, a - 0.01);
                let mut n = around.cross(along).normalize_or(axis);
                let radial = (pos(r, h, a) - (base + axis * h)).normalize_or(axis);
                if n.dot(radial) < 0.0 && r > 1e-6 {
                    n = -n;
                }
                self.m.push(pos(r, h, a), n, color(t, a), 0.75 + 0.25 * t as f32, base, wind);
            }
        }
        for i in 0..np - 1 {
            let (a, b) = (rows[i], rows[i + 1]);
            for s in 0..segs {
                let s1 = (s + 1) % segs;
                self.m.indices.extend_from_slice(&[a + s, a + s1, b + s1, a + s, b + s1, b + s]);
            }
        }
    }

    /// Bent ribbon (petal, frond, tentacle blade): `spine` points, width per
    /// point, `side` the across direction at the base. Two-sided card.
    fn ribbon(&mut self, spine: &[DVec3], widths: &[f64], side: DVec3, cols: &[[f32; 3]], level: u8) {
        if spine.len() < 2 {
            return;
        }
        let wind = [level, 60, 0, class::ORGAN];
        let base = self.m.vertices.len() as u32;
        for k in 0..spine.len() {
            let d = if k + 1 < spine.len() { spine[k + 1] - spine[k] } else { spine[k] - spine[k - 1] }.normalize_or(DVec3::Z);
            let s = (side - d * side.dot(d)).normalize_or(perp(d).0);
            let n = d.cross(s);
            for e in [-1.0, 1.0] {
                // Slight cup across the ribbon reads as a curved petal.
                let cup = n * (0.15 * widths[k]);
                self.m.push(spine[k] + s * e * widths[k] * 0.5 + cup, n + s * e * 0.4, cols[k], 0.85, spine[0], wind);
            }
        }
        for k in 0..spine.len() as u32 - 1 {
            let (a, b) = (base + 2 * k, base + 2 * k + 2);
            self.m.indices.extend_from_slice(&[a, a + 1, b + 1, a, b + 1, b]);
        }
    }

    fn sphere(&mut self, c: DVec3, r: DVec3, color: [f32; 3], level: u8) {
        let segs = self.n(10.0, 5);
        let rings = self.n(7.0, 3).max(3);
        let profile: Vec<(f64, f64)> = (0..=rings)
            .map(|i| {
                let a = std::f64::consts::PI * i as f64 / rings as f64;
                (a.sin(), -a.cos())
            })
            .collect();
        let start = self.m.vertices.len();
        self.lathe(DVec3::ZERO, DVec3::Z, &profile, segs, 0, 0.0, &|_, _| color, level);
        for v in &mut self.m.vertices[start..] {
            let p = DVec3::from(v.position.map(|x| x as f64));
            v.position = (c + p * r).as_vec3().into();
        }
    }
}

/// Points along a stalk from `base` in direction `dir` that bends toward
/// `down` by `droop` and twists; `n` points.
fn bent_curve(base: DVec3, dir: DVec3, length: f64, droop: f64, twist: f64, n: usize) -> Vec<DVec3> {
    let mut pts = vec![base];
    let mut d = dir.normalize_or(DVec3::Z);
    let step = length / (n - 1).max(1) as f64;
    let (u, _) = perp(d);
    for k in 1..n {
        let t = k as f64 / (n - 1) as f64;
        // Gravity pulls the far part down (droop > 0) or straightens up (< 0).
        d = (d - DVec3::Z * droop * 0.35 * t + u * twist.sin() * 0.15).normalize_or(d);
        let p = *pts.last().unwrap() + d * step;
        pts.push(p);
    }
    pts
}

/// Grow the three LODs of an alien plant. `look`: bark = stalk, organ =
/// body, organ_tip = tips/gradient, accent = pattern/petals.
pub fn grow_plan(g: &PlanGenome, look: &Look, seed: u64) -> [Mesh; LOD_COUNT] {
    let g = &individual(g, seed);
    std::array::from_fn(|l| {
        // Medium-poly budget per LOD: rebuild coarser until it fits.
        let mut res = RES[l];
        loop {
            let mut b = Builder { m: Mesh::default(), res };
            build(&mut b, g, look, seed, l);
            let tris = b.m.triangles() as f64;
            if tris <= PLAN_BUDGET[l] as f64 || res < 0.05 {
                return b.m;
            }
            res *= (PLAN_BUDGET[l] as f64 / tris).sqrt().min(0.9);
        }
    })
}

fn build(b: &mut Builder, g: &PlanGenome, look: &Look, seed: u64, lod: usize) {
    let st = Stream::new(seed, domain::TUFT ^ 0x77);
    let golden = 137.508_f64.to_radians();
    match g.plan {
        BodyPlan::Cap => {
            for h in 0..g.heads {
                let a = h as f64 * golden + st.signed(h as u64, 0) * 0.4;
                let out = DVec3::new(a.cos(), a.sin(), 0.0);
                let size = if h == 0 { 1.0 } else { 0.45 + 0.4 * st.unit(h as u64, 1) };
                let base = out * g.head_radius_m * 0.5 * (h > 0) as u8 as f64;
                let dir = (DVec3::Z + out * g.lean * (h > 0) as u8 as f64 * 0.8).normalize();
                let pts = bent_curve(base, dir, g.height_m * size, g.droop * 0.3, g.twist, b.n(8.0, 3) as usize);
                let radii: Vec<f64> = (0..pts.len()).map(|k| g.stalk_radius_m * size * (1.0 + g.flare * (1.0 - k as f64 / pts.len() as f64).powi(3))).collect();
                let cols: Vec<[f32; 3]> = (0..pts.len()).map(|k| mix3(look.bark, look.organ_tip, k as f64 / pts.len() as f64)).collect();
                b.tube(&pts, &radii, &cols, b.n(8.0, 3), 1);
                let top = *pts.last().unwrap();
                let axis = (top - pts[pts.len() - 2]).normalize_or(DVec3::Z);
                let r = g.head_radius_m * size;
                let rings = b.n(8.0, 3) as usize;
                let dome = 0.25 + 0.4 * g.bulge;
                let mut profile = vec![(r * 0.05, -0.05 * r)];
                for i in 0..=rings {
                    let t = i as f64 / rings as f64;
                    let ang = t * std::f64::consts::FRAC_PI_2;
                    let rim_droop = g.flare * 0.3 * r * (1.0 - t).powi(2);
                    profile.push((r * ang.cos().max(0.0).max(0.02), r * dome * ang.sin() - rim_droop));
                }
                // Underside gills via ribs on the first ring (darker).
                let spots = g.pattern;
                let sseed = seed ^ h as u64;
                let color = move |t: f64, a: f64| {
                    let sp = Stream::new(sseed, 0x5907);
                    let cell = ((a * 4.0).floor() as u64) * 31 + (t * 5.0).floor() as u64;
                    if t > 0.15 && sp.unit(cell, 0) < spots * 0.35 {
                        look.accent
                    } else {
                        mix3(look.organ, look.organ_tip, t)
                    }
                };
                b.lathe(top, axis, &profile, b.n(16.0, 6), g.ribs.min(24), g.rib_depth * 0.5, &color, 2);
            }
        }
        BodyPlan::Coral => {
            // Dichotomous branching: each segment splits in two, rising, with
            // thickness conserved (pipe-like) and swollen tips.
            #[allow(clippy::too_many_arguments)]
            fn branch(b: &mut Builder, g: &PlanGenome, look: &Look, st: &Stream, p: DVec3, d: DVec3, len: f64, r: f64, depth: u32, id: u64) {
                let n = b.n(4.0, 2) as usize + 1;
                let pts = bent_curve(p, d, len, -0.4 + g.droop * 0.5, g.twist, n);
                let t_depth = 1.0 - depth as f64 / g.segments.max(1) as f64;
                let radii: Vec<f64> = (0..n).map(|k| r * (1.0 - 0.25 * k as f64 / n as f64)).collect();
                let cols: Vec<[f32; 3]> = (0..n).map(|k| mix3(look.organ, look.organ_tip, t_depth + 0.2 * k as f64 / n as f64)).collect();
                b.tube(&pts, &radii, &cols, b.n(7.0, 3), depth.min(3) as u8 + 1);
                let end = *pts.last().unwrap();
                if depth == 0 {
                    let k = r * (1.0 + g.tip_knob);
                    b.sphere(end, DVec3::splat(k), look.accent, 3);
                    return;
                }
                let (u, w) = perp(d);
                let a = st.unit(id, 3) * std::f64::consts::TAU;
                let spread = (0.45 + 0.4 * g.lean) * (1.0 + 0.3 * st.signed(id, 4));
                for (s, sign) in [(0u64, 1.0), (1, -1.0)] {
                    let side = u * a.cos() + w * a.sin();
                    let nd = (d * spread.cos() + side * sign * spread.sin() + DVec3::Z * 0.3).normalize();
                    branch(b, g, look, st, end, nd, len * (0.72 + 0.1 * st.unit(id, 5 + s)), r * 0.74, depth - 1, id * 2 + s + 1);
                }
            }
            for h in 0..g.heads {
                let a = h as f64 * golden;
                let d = (DVec3::Z + DVec3::new(a.cos(), a.sin(), 0.0) * 0.35 * (g.heads > 1) as u8 as f64).normalize();
                let depth = g.segments.min(if lod == 0 { 6 } else if lod == 1 { 4 } else { 2 });
                branch(b, g, look, &st, DVec3::ZERO, d, g.height_m * 0.32, g.stalk_radius_m, depth, 1 + h as u64 * 1000);
            }
        }
        BodyPlan::Anemone => {
            let r = g.head_radius_m;
            let profile: Vec<(f64, f64)> = (0..=6)
                .map(|i| {
                    let t = i as f64 / 6.0;
                    (r * (1.0 - t * t).sqrt().max(0.02), g.height_m * 0.25 * t.powf(1.0 / g.bulge.max(0.3)))
                })
                .collect();
            b.lathe(DVec3::ZERO, DVec3::Z, &profile, b.n(14.0, 6), g.ribs, g.rib_depth, &|t, _| mix3(look.organ, look.bark, 1.0 - t), 1);
            let count = ((g.spikes as f64) * b.res.max(0.35)).round().max(3.0) as u32;
            for k in 0..count {
                let t = (k as f64 + 0.5) / count as f64;
                let a = k as f64 * golden;
                let elev = (1.0 - t).acos() * 0.9; // fill the dome
                let out = DVec3::new(a.cos(), a.sin(), 0.0);
                let nrm = (out * elev.sin() + DVec3::Z * elev.cos()).normalize();
                let base = DVec3::new(0.0, 0.0, g.height_m * 0.22 * elev.cos()) + out * r * elev.sin() * 0.9;
                let len = g.spike_length_m * (0.7 + 0.5 * st.unit(k as u64, 1));
                let pts = bent_curve(base, (nrm + DVec3::Z * 0.6).normalize(), len, g.droop, g.twist * (1.0 + st.signed(k as u64, 2)), b.n(6.0, 3) as usize);
                let radii: Vec<f64> = (0..pts.len()).map(|i| g.stalk_radius_m * (1.0 - 0.75 * i as f64 / pts.len() as f64)).collect();
                let cols: Vec<[f32; 3]> = (0..pts.len()).map(|i| mix3(look.organ, look.organ_tip, i as f64 / pts.len() as f64)).collect();
                b.tube(&pts, &radii, &cols, b.n(5.0, 3), 2);
                if g.tip_knob > 0.0 && lod < 2 {
                    b.sphere(*pts.last().unwrap(), DVec3::splat(g.stalk_radius_m * g.tip_knob), look.accent, 3);
                }
            }
        }
        BodyPlan::Gourd => {
            for h in 0..g.heads {
                let a = h as f64 * golden;
                let size = if h == 0 { 1.0 } else { 0.5 + 0.4 * st.unit(h as u64, 0) };
                let c = DVec3::new(a.cos(), a.sin(), 0.0) * g.head_radius_m * 1.3 * (h > 0) as u8 as f64;
                let r = g.head_radius_m * size;
                let hgt = g.height_m * size;
                let rings = b.n(12.0, 4) as usize;
                let profile: Vec<(f64, f64)> = (0..=rings)
                    .map(|i| {
                        let t = i as f64 / rings as f64;
                        let body = (std::f64::consts::PI * t).sin().powf(1.0 / g.bulge.max(0.3));
                        (r * body.max(0.03) * (1.0 + g.flare * 0.3 * t), hgt * t)
                    })
                    .collect();
                let pat = g.pattern;
                let ribs = g.ribs;
                let color = move |t: f64, a: f64| {
                    let valley = 0.5 - 0.5 * (ribs as f64 * a).cos();
                    mix3(mix3(look.organ, look.organ_tip, t), look.accent, pat * (1.0 - valley) * 0.8)
                };
                b.lathe(c, DVec3::Z, &profile, b.n(18.0, 6), g.ribs, g.rib_depth, &color, 1);
                // Crown of spikes on top.
                let spikes = ((g.spikes as f64 * b.res).round() as u32).min(40);
                for k in 0..spikes {
                    let aa = k as f64 / spikes.max(1) as f64 * std::f64::consts::TAU;
                    let dir = (DVec3::Z + DVec3::new(aa.cos(), aa.sin(), 0.0) * (0.3 + g.lean)).normalize();
                    let base = c + DVec3::Z * hgt * 0.97;
                    let pts = bent_curve(base, dir, g.spike_length_m * size, g.droop, 0.0, 3);
                    let radii = [g.stalk_radius_m * size, g.stalk_radius_m * size * 0.6, g.stalk_radius_m * 0.15];
                    b.tube(&pts, &radii, &[look.organ_tip, look.organ_tip, look.accent], 3, 2);
                }
            }
        }
        BodyPlan::Bladder => {
            for h in 0..g.heads {
                let a = h as f64 * golden + st.signed(h as u64, 0) * 0.3;
                let out = DVec3::new(a.cos(), a.sin(), 0.0);
                let dir = (DVec3::Z + out * g.lean * (h > 0) as u8 as f64).normalize();
                let len = g.height_m * (0.55 + 0.45 * st.unit(h as u64, 1));
                let pts = bent_curve(out * g.head_radius_m * 0.3 * (h > 0) as u8 as f64, dir, len, g.droop * 0.5, g.twist, b.n(7.0, 3) as usize);
                let radii = vec![g.stalk_radius_m; pts.len()];
                let cols: Vec<[f32; 3]> = (0..pts.len()).map(|k| mix3(look.bark, look.organ, k as f64 / pts.len() as f64)).collect();
                b.tube(&pts, &radii, &cols, b.n(6.0, 3), 1);
                // Stacked bladders along the upper stalk.
                let segs = g.segments.max(1);
                for s in 0..segs {
                    let t = 1.0 - s as f64 / segs as f64 * 0.6;
                    let idx = ((pts.len() - 1) as f64 * t).round() as usize;
                    let r = g.head_radius_m * (1.0 - 0.25 * s as f64) * (0.8 + 0.3 * st.unit(h as u64, 10 + s as u64));
                    let c = pts[idx] + DVec3::Z * r * 0.6;
                    let col = mix3(look.organ, look.organ_tip, 0.4 + 0.6 * t);
                    b.sphere(c, DVec3::new(r, r, r * (0.8 + 0.5 * g.bulge.min(2.0) / 2.0)), col, 2);
                }
            }
        }
        BodyPlan::FrondFan => {
            let count = ((g.petals as f64) * b.res.max(0.4)).round().max(3.0) as u32;
            for k in 0..count {
                let a = k as f64 * golden;
                let out = DVec3::new(a.cos(), a.sin(), 0.0);
                let rise = 0.5 + 0.5 * st.unit(k as u64, 0);
                let dir = (DVec3::Z * rise + out * (1.0 - rise + g.lean)).normalize();
                let n = b.n(18.0, 3) as usize;
                let spine = bent_curve(DVec3::Z * g.stalk_radius_m, dir, g.petal_length_m, g.droop, g.twist * 0.3, n);
                let widths: Vec<f64> = (0..n)
                    .map(|i| {
                        let t = i as f64 / (n - 1) as f64;
                        // Leaflet notches along the frond (sawtooth width).
                        let notch = if g.ribs > 0 { 0.75 + 0.25 * ((t * g.ribs as f64 * 2.0).fract() - 0.5).abs() * 2.0 } else { 1.0 };
                        g.petal_length_m * g.petal_width * (std::f64::consts::PI * t).sin().powf(0.6) * notch
                    })
                    .collect();
                let cols: Vec<[f32; 3]> = (0..n).map(|i| mix3(look.organ, look.organ_tip, i as f64 / n as f64)).collect();
                let side = out.cross(DVec3::Z);
                if g.ribs > 0 {
                    // Pinnate frond: a narrow midrib with leaflet pairs that
                    // shorten toward the tip and droop.
                    let mid: Vec<f64> = widths.iter().map(|w| w * 0.25).collect();
                    b.ribbon(&spine, &mid, side, &cols, 2);
                    let pairs = ((g.ribs as f64 * 1.5 * b.res.max(0.3)).round() as usize).max(2);
                    for q in 0..pairs {
                        let t = 0.12 + 0.83 * q as f64 / pairs as f64;
                        let idx = ((n - 1) as f64 * t).round() as usize;
                        let len = g.petal_length_m * g.petal_width * 1.6 * (1.0 - 0.7 * t);
                        let d = (spine[(idx + 1).min(n - 1)] - spine[idx.saturating_sub(1)]).normalize_or(dir);
                        for e in [-1.0, 1.0] {
                            let ldir = (side * e + d * 0.5 + DVec3::Z * 0.2).normalize();
                            let m = b.n(5.0, 2) as usize + 1;
                            let ls = bent_curve(spine[idx], ldir, len, g.droop * 0.8, 0.0, m);
                            let lw: Vec<f64> = (0..m).map(|i| len * 0.35 * (std::f64::consts::PI * (i as f64 + 0.5) / m as f64).sin()).collect();
                            let lc: Vec<[f32; 3]> = (0..m).map(|_| mix3(look.organ, look.organ_tip, t)).collect();
                            b.ribbon(&ls, &lw, d.cross(ldir), &lc, 3);
                        }
                    }
                } else {
                    b.ribbon(&spine, &widths, side, &cols, 2);
                }
            }
            if g.tip_knob > 0.0 {
                b.sphere(DVec3::Z * g.height_m * 0.15, DVec3::splat(g.head_radius_m * 0.3 * g.tip_knob), look.accent, 1);
            }
        }
        BodyPlan::FlowerStalk => {
            for h in 0..g.heads {
                let a = h as f64 * golden;
                let out = DVec3::new(a.cos(), a.sin(), 0.0);
                let dir = (DVec3::Z + out * g.lean * 0.6 * (h > 0) as u8 as f64).normalize();
                let len = g.height_m * (0.65 + 0.35 * st.unit(h as u64, 0));
                let pts = bent_curve(out * g.stalk_radius_m * 3.0 * (h > 0) as u8 as f64, dir, len, g.droop * 0.4, g.twist, b.n(10.0, 3) as usize);
                let radii: Vec<f64> = (0..pts.len()).map(|k| g.stalk_radius_m * (1.0 - 0.5 * k as f64 / pts.len() as f64)).collect();
                b.tube(&pts, &radii, &vec![look.bark; pts.len()], b.n(6.0, 3), 1);
                // Petal whorls at nodes, a big radial flower on top.
                let nodes = g.segments.min(6);
                for s in 0..=nodes {
                    let top = s == nodes;
                    let t = if top { 1.0 } else { 0.35 + 0.55 * s as f64 / nodes.max(1) as f64 };
                    let idx = ((pts.len() - 1) as f64 * t).round() as usize;
                    let c = pts[idx];
                    let petals = if top { g.petals } else { (g.petals / 2).max(3) };
                    let plen = g.petal_length_m * if top { 1.0 } else { 0.45 };
                    let petals = ((petals as f64) * b.res.max(0.4)).round().max(3.0) as u32;
                    for p in 0..petals {
                        let aa = p as f64 / petals as f64 * std::f64::consts::TAU + s as f64;
                        let o = DVec3::new(aa.cos(), aa.sin(), 0.0);
                        let pdir = (o + DVec3::Z * (0.6 - 0.8 * g.droop)).normalize();
                        let n = b.n(4.0, 2) as usize + 1;
                        let spine = bent_curve(c, pdir, plen, g.droop * 0.6, 0.0, n);
                        let widths: Vec<f64> = (0..n).map(|i| plen * g.petal_width * (std::f64::consts::PI * (i as f64 / (n - 1) as f64)).sin().max(0.05)).collect();
                        let cols: Vec<[f32; 3]> = (0..n).map(|i| mix3(look.accent, look.organ_tip, (i as f64 / n as f64) * g.pattern)).collect();
                        b.ribbon(&spine, &widths, o.cross(DVec3::Z), &cols, 3);
                    }
                    if top {
                        b.sphere(c, DVec3::splat(g.head_radius_m * 0.35), look.organ, 3);
                    }
                }
            }
        }
        BodyPlan::Crystal => {
            for h in 0..g.heads {
                let a = h as f64 * golden + st.signed(h as u64, 0) * 0.4;
                let out = DVec3::new(a.cos(), a.sin(), 0.0);
                let size = if h == 0 { 1.0 } else { 0.4 + 0.5 * st.unit(h as u64, 1) };
                let axis = (DVec3::Z + out * g.lean * (h > 0) as u8 as f64 * 1.2).normalize();
                let r = g.head_radius_m * size;
                let len = g.height_m * size;
                let sides = g.ribs.clamp(3, 8);
                // Prism with a pointed tip: lathe with as many segments as
                // faces gives flat facets.
                let profile = [(r * 0.85, -0.05 * len), (r, 0.0), (r * (1.0 + 0.1 * g.flare), len * 0.75), (r * 0.02, len)];
                let base = out * g.head_radius_m * 0.9 * (h > 0) as u8 as f64;
                b.lathe(base, axis, &profile, sides, 0, 0.0, &|t, _| mix3(look.organ, look.organ_tip, t), 1);
            }
        }
    }
}

/// An individual of the species: the genome varied by its seed (size, head
/// and part counts, posture), so variants of one species differ visibly
/// while staying recognisably the same plant.
pub fn individual(g: &PlanGenome, seed: u64) -> PlanGenome {
    let st = Stream::new(seed, domain::VARIANT ^ 0x51);
    let j = |i: u64, a: f64| 1.0 + a * st.signed(i, 0);
    let count = |i: u64, n: u32, d: i64, lo: u32| ((n as i64 + (st.signed(i, 1) * (d as f64 + 0.49)).round() as i64).max(lo as i64)) as u32;
    let mut o = g.clone();
    o.height_m *= j(0, 0.18);
    o.head_radius_m *= j(1, 0.18);
    o.stalk_radius_m *= j(2, 0.12);
    o.droop = (o.droop + 0.2 * st.signed(3, 0)).clamp(-1.0, 1.0);
    o.lean = (o.lean + 0.2 * st.signed(4, 0)).clamp(0.0, 1.0);
    o.twist += 0.2 * st.signed(5, 0);
    o.heads = count(6, o.heads, 1, 1).min(12);
    o.segments = count(7, o.segments, 1, 1).min(8);
    o.spikes = (o.spikes as f64 * j(8, 0.2)).round() as u32;
    o.petals = count(9, o.petals, 2, 0).min(40);
    o.spike_length_m *= j(10, 0.15);
    o.petal_length_m *= j(11, 0.15);
    o.bulge = (o.bulge * j(12, 0.15)).clamp(0.3, 3.0);
    o
}
