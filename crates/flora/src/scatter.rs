//! CPU reference of the forest placement field (mirrors
//! `crates/renderer/src/shaders/scatter_niche.wgsl`, checked by the GPU test
//! `crates/renderer/tests/flora_niche_gpu.rs`) and the generator of the
//! per-species niche table the GPU pass compiles in.
//!
//! Model: each species has a niche suitability `s_k` (niche.rs). The canopy
//! suitability is the best canopy species; four octaves of world-anchored
//! value noise against a threshold that falls with suitability make forest
//! cores, fringes and clearings (the M5 forest field). Within a layer, the
//! species is drawn with weights `s_k² · prior_k`. Nothing is stored: every
//! plant is a function of its base cell.

use crate::SpeciesFile;
use crate::genome::CrownShape;
use crate::niche::{Layer, Site, smoothstep};

/// Most species the GPU table and bucket layout hold.
pub const MAX_SPECIES: usize = 6;
/// Most rock archetypes (entries after the species in the GPU tables).
pub const MAX_ROCKS: usize = 2;

/// PCG3D integer hash (Jarzynski & Olano 2020), as `sc_pcg3d`.
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

/// 24-bit unit float, as `sc_unit`.
pub fn unit(bits: u32) -> f64 {
    (bits >> 8) as f64 / 16_777_216.0
}

/// Smooth value noise in 0..1 over base-cell coordinates, as `sc_value`.
pub fn value(face: u32, x: f64, y: f64, salt: u32) -> f64 {
    let (i, j) = (x.floor(), y.floor());
    let f = (x - i, y - j);
    let w = (f.0 * f.0 * (3.0 - 2.0 * f.0), f.1 * f.1 * (3.0 - 2.0 * f.1));
    let (cx, cy) = (i as i64 as u32, j as i64 as u32);
    let k = face * 64 + salt;
    let h = |a: u32, b: u32| unit(pcg3d([a, b, k])[0]);
    let a = h(cx, cy);
    let b = h(cx.wrapping_add(1), cy);
    let d = h(cx, cy.wrapping_add(1));
    let e = h(cx.wrapping_add(1), cy.wrapping_add(1));
    let mix = |p: f64, q: f64, t: f64| p + (q - p) * t;
    mix(mix(a, b, w.0), mix(d, e, w.0), w.1)
}

/// Forest field at one site, as `ForestSite` in WGSL.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ForestSite {
    /// Probabilities of a tree / shrub / boulder in a base cell (sum ≤ 1).
    pub tree: f64,
    pub shrub: f64,
    pub boulder: f64,
    /// 1 in a forest core, 0 outside.
    pub core: f64,
    /// Best canopy / shrub suitability.
    pub canopy: f64,
    pub shrubland: f64,
    /// Canopy size factor (suitability edge and core).
    pub stature: f64,
    /// Expected ground cover of the plants (`tree·stature + 0.4·shrub`):
    /// the far tint strength.
    pub cover: f64,
    /// Canopy colour for the far tint (linear RGB).
    pub color: [f64; 3],
}

/// Species niches in GPU order (≤ MAX_SPECIES).
pub struct Placement<'a> {
    pub species: &'a [SpeciesFile],
}

const DEG: f64 = std::f64::consts::PI / 180.0;

impl Placement<'_> {
    fn count(&self) -> usize {
        self.species.len().min(MAX_SPECIES)
    }

    pub fn suit(&self, k: usize, s: &Site) -> f64 {
        self.species[k].niche.suitability(s)
    }

    fn layer_suit(&self, layer: Layer, s: &Site) -> f64 {
        (0..self.count())
            .filter(|&k| self.species[k].niche.layer == layer)
            .map(|k| self.suit(k, s))
            .fold(0.0, f64::max)
    }

    /// Forest field (mirror of `sc_forest`); `footprint` in base cells, 0
    /// for single plants.
    pub fn forest(&self, face: u32, ci: f64, cj: f64, footprint: f64, s: &Site) -> ForestSite {
        let suit = self.layer_suit(Layer::Canopy, s);
        let f1 = smoothstep(100.0, 200.0, footprint);
        let f2 = smoothstep(25.0, 50.0, footprint);
        let f3 = smoothstep(6.0, 12.0, footprint);
        let f4 = smoothstep(1.5, 3.0, footprint);
        let mix = |p: f64, q: f64, t: f64| p + (q - p) * t;
        let o1 = mix(value(face, ci / 400.0, cj / 400.0, 40), 0.5, f1);
        let o2 = mix(value(face, ci / 100.0, cj / 100.0, 41), 0.5, f2);
        let o3 = mix(value(face, ci / 25.0, cj / 25.0, 42), 0.5, f3);
        let o4 = mix(value(face, ci / 6.0, cj / 6.0, 43), 0.5, f4);
        let n = 0.45 * o1 + 0.3 * o2 + 0.15 * o3 + 0.1 * o4;
        // Octaves faded to their mean leave the noise's spread out of `n`;
        // average over it instead (16 equal-mass nodes at the measured
        // quantiles of the noise sum), so the far tint is the expected cover
        // of the plants it stands for. Per-octave value noise sd 0.215.
        let sigma = 0.215
            * ((0.45 * f1).powi(2) + (0.3 * f2).powi(2) + (0.15 * f3).powi(2) + (0.1 * f4).powi(2)).sqrt();
        let shrubby = self.layer_suit(Layer::Shrub, s);
        let rock = smoothstep(25.0 * DEG, 45.0 * DEG, s.slope);
        let boulder = 0.02 + 0.25 * rock * (1.0 - smoothstep(60.0 * DEG, 70.0 * DEG, s.slope));
        let threshold = 1.0 - 0.72 * suit;
        let soft = 0.05;
        let mut site = ForestSite { canopy: suit, shrubland: shrubby, ..Default::default() };
        let nodes: &[f64] = if sigma > 0.0 { &NOISE_QUANTILES } else { &[0.0] };
        let w = 1.0 / nodes.len() as f64;
        for &x in nodes {
            let nn = n + sigma * x;
            let core = smoothstep(threshold - soft, threshold + soft, nn) * smoothstep(0.0, 0.08, suit);
            let fringe = smoothstep(threshold - 0.3 - soft, threshold - 0.03 + soft, nn) * (1.0 - core);
            let mut tree = suit * (0.9 * core + 0.18 * fringe * fringe + 0.06 * fringe) + 0.025 * suit;
            let mut shrub = shrubby * (0.35 * fringe + 0.06 * core + 0.04);
            let mut bould = boulder;
            let total = tree + shrub + bould;
            if total > 1.0 {
                tree /= total;
                shrub /= total;
                bould /= total;
            }
            let stature = mix(0.45, 1.0, suit) * mix(0.6, 1.0, core);
            site.tree += w * tree;
            site.shrub += w * shrub;
            site.boulder += w * bould;
            site.core += w * core;
            site.stature += w * stature;
            site.cover += w * (tree * stature + 0.4 * shrub).clamp(0.0, 1.0);
        }
        // Canopy colour: species colours weighted like the species choice.
        let mut c = [0.0; 3];
        let mut wsum = 0.0;
        for k in 0..self.count() {
            let sp = &self.species[k];
            if sp.niche.layer != Layer::Canopy {
                continue;
            }
            let su = self.suit(k, s);
            let w = su * su * sp.niche.prior;
            let col = canopy_color(sp);
            for a in 0..3 {
                c[a] += w * col[a];
            }
            wsum += w;
        }
        site.color = if wsum > 1e-6 { c.map(|v| v / wsum) } else { DEFAULT_CANOPY };
        site
    }

    /// Species of `layer` drawn by `roll` in 0..1 with weights `s² · prior`
    /// (mirror of `fl_pick`): (index, suitability), or None.
    pub fn pick(&self, layer: Layer, s: &Site, roll: f64) -> Option<(usize, f64)> {
        let w = |k: usize| {
            let su = self.suit(k, s);
            su * su * self.species[k].niche.prior
        };
        let members: Vec<usize> = (0..self.count()).filter(|&k| self.species[k].niche.layer == layer).collect();
        let total: f64 = members.iter().map(|&k| w(k)).sum();
        if total <= 0.0 {
            return None;
        }
        let target = roll * total;
        let mut acc = 0.0;
        for &k in &members {
            acc += w(k);
            if target < acc {
                return Some((k, self.suit(k, s)));
            }
        }
        members.iter().rev().find(|&&k| w(k) > 0.0).map(|&k| (k, self.suit(k, s)))
    }
}

/// Standardised quantiles of the forest noise sum at (i + 0.5) / 16 (measured
/// over 400k cells; mean 0.498, sd 0.1226).
#[allow(clippy::approx_constant)] // measured values, not π/3
pub const NOISE_QUANTILES: [f64; 16] = [
    -1.8306, -1.3436, -1.0457, -0.8138, -0.6127, -0.4298, -0.2542, -0.0872, 0.0801, 0.2506, 0.4245, 0.6083,
    0.8109, 1.0471, 1.3482, 1.8451,
];

/// Fallback canopy colour (the old M5 broadleaf tint).
pub const DEFAULT_CANOPY: [f64; 3] = [0.04, 0.07, 0.025];

/// Canopy colour seen from afar: the organ colour darkened by crown
/// self-shadowing.
pub fn canopy_color(sp: &SpeciesFile) -> [f64; 3] {
    let o = sp.look.organ;
    let t = sp.look.organ_tip;
    [0, 1, 2].map(|a| 0.75 * (0.6 * o[a] as f64 + 0.4 * t[a] as f64))
}

/// Procedural far shape (M5 kinds): 0 conifer, 1 broadleaf, 2 shrub.
pub fn far_kind(sp: &SpeciesFile) -> u32 {
    match (sp.niche.layer, sp.genome.crown_shape) {
        (Layer::Shrub, _) => 2,
        (Layer::Canopy, CrownShape::Cone) => 0,
        (Layer::Canopy, _) => 1,
    }
}

pub const WGSL_BEGIN: &str = "// BEGIN FLORA SPECIES";
pub const WGSL_END: &str = "// END FLORA SPECIES";

fn f(x: f64) -> String {
    let s = format!("{:.6}", x as f32);
    s
}

/// The generated niche table (`scatter_species.wgsl`), markers included.
pub fn species_wgsl(species: &[SpeciesFile], rocks: &[crate::palette::PlanetRock]) -> String {
    let n = species.len().min(MAX_SPECIES);
    let mut canopy = 0u32;
    let mut shrub = 0u32;
    for (k, sp) in species.iter().take(n).enumerate() {
        match sp.niche.layer {
            Layer::Canopy => canopy |= 1 << k,
            Layer::Shrub => shrub |= 1 << k,
        }
    }
    let mut out = String::new();
    out.push_str(WGSL_BEGIN);
    out.push_str("\n// Generated from content/flora/species by astrum_flora::scatter::species_wgsl.\n");
    out.push_str("// Do not edit; the renderer regenerates it at start-up and a test keeps\n");
    out.push_str("// this checked-in copy equal to the content.\n");
    out.push_str(&format!("const FL_SPECIES: u32 = {n}u;\n"));
    out.push_str(&format!("const FL_CANOPY_MASK: u32 = {canopy}u;\n"));
    out.push_str(&format!("const FL_SHRUB_MASK: u32 = {shrub}u;\n"));
    out.push_str(
        "struct FlNiche {\n    t: vec3<f32>,\n    m: vec3<f32>,\n    h: vec3<f32>,\n    slope: vec2<f32>,\n    soil: f32,\n    prior: f32,\n    scale: vec2<f32>,\n    far_kind: u32,\n    color: vec3<f32>,\n}\n",
    );
    out.push_str("fn fl_niche(k: u32) -> FlNiche {\n    switch k {\n");
    let row = |sp: &SpeciesFile| {
        let ni = &sp.niche;
        let c = canopy_color(sp);
        format!(
            "FlNiche(vec3<f32>({}, {}, {}), vec3<f32>({}, {}, {}), vec3<f32>({}, {}, {}), vec2<f32>({}, {}), {}, {}, vec2<f32>({}, {}), {}u, vec3<f32>({}, {}, {}))",
            f(ni.temperature_c.min),
            f(ni.temperature_c.max),
            f(ni.temperature_c.falloff.max(1e-6)),
            f(ni.moisture.min),
            f(ni.moisture.max),
            f(ni.moisture.falloff.max(1e-6)),
            f(ni.height_m.min),
            f(ni.height_m.max),
            f(ni.height_m.falloff.max(1e-6)),
            f(ni.max_slope_deg.to_radians()),
            f(ni.slope_falloff_deg.to_radians()),
            f(ni.soil_min),
            f(ni.prior),
            f(ni.scale.0),
            f(ni.scale.1),
            far_kind(sp),
            f(c[0]),
            f(c[1]),
            f(c[2]),
        )
    };
    for (k, sp) in species.iter().take(n).enumerate() {
        out.push_str(&format!("        // {}\n        case {k}u: {{ return {}; }}\n", sp.name, row(sp)));
    }
    out.push_str(
        "        default: { return FlNiche(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), vec2<f32>(0.0, 0.0), 1.0, 0.0, vec2<f32>(1.0, 1.0), 1u, vec3<f32>(0.04, 0.07, 0.025)); }\n    }\n}\n",
    );
    let nr = rocks.len().min(MAX_ROCKS);
    out.push_str(&format!("const FL_ROCKS: u32 = {nr}u;\n"));
    out.push_str(&format!("// Bucket entry of rock 0 (after the MAX_SPECIES plant entries).\nconst FL_ROCK_ENTRY: u32 = {MAX_SPECIES}u;\n"));
    out.push_str("// Bedrock hardness envelope (min, max, falloff) of rock archetype `r`.\n");
    out.push_str("fn fl_rock(r: u32) -> vec3<f32> {\n    switch r {\n");
    for (k, r) in rocks.iter().take(nr).enumerate() {
        out.push_str(&format!(
            "        // {}\n        case {k}u: {{ return vec3<f32>({}, {}, {}); }}\n",
            r.name,
            f(r.hardness.min),
            f(r.hardness.max),
            f(r.hardness.falloff.max(1e-6))
        ));
    }
    out.push_str("        default: { return vec3<f32>(0.0, 0.0, 1.0); }\n    }\n}\n");
    out.push_str(WGSL_END);
    out.push('\n');
    out
}

/// Replace the niche table between the markers of `shader` with `table`.
/// Returns `shader` unchanged when the markers are missing.
pub fn splice_species(shader: &str, table: &str) -> String {
    match (shader.find(WGSL_BEGIN), shader.find(WGSL_END)) {
        (Some(a), Some(b)) if b > a => {
            let end = b + WGSL_END.len();
            let end = if shader[end..].starts_with('\n') { end + 1 } else { end };
            format!("{}{}{}", &shader[..a], table, &shader[end..])
        }
        _ => shader.to_string(),
    }
}

/// Rock archetype for bedrock `hardness`, drawn by `roll` with weights from
/// the hardness envelopes (mirror of `fl_pick_rock`).
pub fn pick_rock(rocks: &[crate::palette::PlanetRock], hardness: f64, roll: f64) -> Option<usize> {
    let n = rocks.len().min(MAX_ROCKS);
    let total: f64 = rocks[..n].iter().map(|r| r.hardness.eval(hardness)).sum();
    if total <= 0.0 {
        return None;
    }
    let mut acc = 0.0;
    let mut last = None;
    for (k, r) in rocks[..n].iter().enumerate() {
        let w = r.hardness.eval(hardness);
        acc += w;
        if roll * total < acc {
            return Some(k);
        }
        if w > 0.0 {
            last = Some(k);
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_noise_is_smooth_and_in_range() {
        let mut prev = value(2, 100.0, 50.0, 40);
        for i in 1..200 {
            let v = value(2, 100.0 + i as f64 * 0.01, 50.0, 40);
            assert!((0.0..=1.0).contains(&v));
            assert!((v - prev).abs() < 0.05);
            prev = v;
        }
    }

    #[test]
    fn splice_replaces_only_the_table() {
        let src = format!("a\n{WGSL_BEGIN}\nold\n{WGSL_END}\nb\n");
        let out = splice_species(&src, &format!("{WGSL_BEGIN}\nnew\n{WGSL_END}\n"));
        assert_eq!(out, format!("a\n{WGSL_BEGIN}\nnew\n{WGSL_END}\nb\n"));
    }
}
