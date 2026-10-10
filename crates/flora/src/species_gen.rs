//! Per-planet species generation (user direction 2026-10-10): planet seed +
//! strangeness + palette → a set of species with body plans, genomes,
//! niches and colours. Earth-like planets lean on the tree/shrub grower;
//! strange planets on the alien body plans. Every draw is counter-hashed
//! from the planet seed, so a planet always gets the same flora.

use crate::bodyplan::{BodyPlan, PlanGenome};
use crate::hash::{Stream, name_key};
use crate::niche::{Envelope, Layer, Niche};
use crate::palette::{Palette, PlanetLife};
use crate::SpeciesFile;

const GEN: u64 = 0x5350_4547;

/// Climate affinity of a body plan (convergent heuristics, genesis §4.4):
/// temperature range °C, minimum and maximum moisture, height range m, layer.
struct Affinity {
    t: (f64, f64),
    m: (f64, f64),
    height: (f64, f64),
    canopy: bool,
}

fn affinity(plan: BodyPlan, giant: bool) -> Affinity {
    use BodyPlan::*;
    match plan {
        Cap => Affinity { t: (0.0, 26.0), m: (0.55, 1.0), height: if giant { (3.0, 8.0) } else { (0.3, 1.4) }, canopy: giant },
        Coral => Affinity { t: (10.0, 34.0), m: (0.5, 1.0), height: (0.8, 2.6), canopy: false },
        Anemone => Affinity { t: (4.0, 30.0), m: (0.6, 1.0), height: (0.4, 1.6), canopy: false },
        Gourd => Affinity { t: (15.0, 40.0), m: (0.1, 0.5), height: (0.5, 2.2), canopy: false },
        Bladder => Affinity { t: (0.0, 25.0), m: (0.65, 1.0), height: (0.6, 2.8), canopy: false },
        FrondFan => Affinity { t: (12.0, 35.0), m: (0.45, 1.0), height: if giant { (2.5, 6.0) } else { (0.8, 2.5) }, canopy: giant },
        FlowerStalk => Affinity { t: (5.0, 30.0), m: (0.3, 0.9), height: (0.8, 2.6), canopy: false },
        Crystal => Affinity { t: (-20.0, 10.0), m: (0.0, 1.0), height: if giant { (3.0, 9.0) } else { (0.5, 3.0) }, canopy: giant },
    }
}

/// Plan genome sampled inside the plan's bounds.
fn sample_plan(plan: BodyPlan, height: f64, st: &Stream, k: u64) -> PlanGenome {
    let u = |i: u64| st.unit(k, 100 + i);
    let r = |i: u64, lo: f64, hi: f64| lo + (hi - lo) * u(i);
    let ri = |i: u64, lo: u32, hi: u32| lo + ((hi - lo + 1) as f64 * u(i)) as u32 % (hi - lo + 1);
    use BodyPlan::*;
    let mut g = PlanGenome {
        plan,
        height_m: height,
        heads: 1,
        head_radius_m: height * 0.3,
        stalk_radius_m: height * 0.04,
        droop: r(0, -0.3, 0.6),
        lean: r(1, 0.1, 0.6),
        twist: r(2, -0.4, 0.4),
        ribs: 0,
        rib_depth: 0.0,
        segments: 1,
        spikes: 0,
        spike_length_m: height * 0.3,
        petals: 0,
        petal_length_m: height * 0.4,
        petal_width: 0.3,
        bulge: r(3, 0.7, 1.8),
        flare: r(4, 0.0, 0.8),
        tip_knob: 0.0,
        pattern: r(5, 0.0, 1.0),
    };
    match plan {
        Cap => {
            g.heads = ri(6, 1, 5);
            g.head_radius_m = height * r(7, 0.35, 0.8);
            g.stalk_radius_m = height * r(8, 0.04, 0.09);
            g.ribs = ri(9, 0, 16);
            g.rib_depth = r(10, 0.0, 0.25);
        }
        Coral => {
            g.heads = ri(6, 1, 3);
            g.segments = ri(7, 3, 5);
            g.stalk_radius_m = height * r(8, 0.05, 0.1);
            g.tip_knob = r(9, 0.3, 1.8);
        }
        Anemone => {
            g.head_radius_m = height * r(6, 0.3, 0.6);
            g.spikes = ri(7, 24, 70);
            g.spike_length_m = height * r(8, 0.6, 1.1);
            g.stalk_radius_m = height * r(9, 0.02, 0.04);
            g.droop = r(10, 0.2, 0.9);
            g.tip_knob = if u(11) < 0.6 { r(12, 1.0, 2.5) } else { 0.0 };
            g.ribs = ri(13, 0, 12);
            g.rib_depth = r(14, 0.0, 0.2);
        }
        Gourd => {
            g.heads = ri(6, 1, 4);
            g.head_radius_m = height * r(7, 0.3, 0.55);
            g.ribs = ri(8, 6, 16);
            g.rib_depth = r(9, 0.08, 0.3);
            g.spikes = if u(10) < 0.7 { ri(11, 5, 16) } else { 0 };
            g.spike_length_m = height * r(12, 0.15, 0.4);
            g.stalk_radius_m = height * r(13, 0.015, 0.03);
            g.droop = r(14, -0.4, 0.4);
        }
        Bladder => {
            g.heads = ri(6, 1, 5);
            g.segments = ri(7, 1, 3);
            g.head_radius_m = height * r(8, 0.12, 0.25);
            g.stalk_radius_m = height * r(9, 0.012, 0.025);
        }
        FrondFan => {
            g.petals = ri(6, 6, 16);
            g.petal_length_m = height * r(7, 0.9, 1.4);
            g.petal_width = r(8, 0.15, 0.4);
            g.ribs = ri(9, 0, 10);
            g.droop = r(10, 0.2, 0.9);
            g.stalk_radius_m = height * 0.05;
            g.head_radius_m = height * 0.25;
            g.tip_knob = if u(11) < 0.4 { r(12, 0.5, 1.5) } else { 0.0 };
        }
        FlowerStalk => {
            g.heads = ri(6, 1, 5);
            g.segments = ri(7, 1, 5);
            g.petals = ri(8, 5, 20);
            g.petal_length_m = height * r(9, 0.12, 0.3);
            g.petal_width = r(10, 0.12, 0.5);
            g.stalk_radius_m = height * r(11, 0.012, 0.025);
            g.head_radius_m = height * r(12, 0.05, 0.12);
            g.droop = r(13, -0.3, 0.5);
        }
        Crystal => {
            g.heads = ri(6, 2, 7);
            g.head_radius_m = height * r(7, 0.08, 0.18);
            g.ribs = ri(8, 4, 7);
            g.lean = r(9, 0.3, 0.8);
            g.flare = r(10, 0.0, 0.4);
        }
    }
    g
}

/// Pronounceable species name from a seed (two or three syllables).
fn name(st: &Stream, k: u64) -> String {
    const ON: &[&str] = &["v", "k", "th", "s", "m", "r", "z", "l", "dr", "qu", "n", "b", "sh", "t"];
    const NU: &[&str] = &["a", "e", "i", "o", "u", "ae", "io", "ou", "y"];
    const CO: &[&str] = &["", "n", "l", "r", "s", "th", "x"];
    let syl = 2 + (st.unit(k, 1) < 0.4) as u64;
    let mut s = String::new();
    for i in 0..syl {
        let pick = |list: &[&'static str], j: u64| -> &'static str { list[(st.unit(k, 10 + i * 3 + j) * list.len() as f64) as usize % list.len()] };
        s.push_str(pick(ON, 0));
        s.push_str(pick(NU, 1));
        if i + 1 == syl {
            s.push_str(pick(CO, 2));
        }
    }
    s
}

/// Generate `count` species for the planet. `templates` are authored tree
/// and shrub species (their genomes are jittered, not replaced); the rest
/// of the slots go to alien body plans, more of them the stranger the
/// planet.
pub fn generate(planet: &PlanetLife, templates: &[SpeciesFile], count: usize) -> Vec<SpeciesFile> {
    let st = Stream::new(planet.seed, GEN);
    let palette = Palette::for_planet(planet);
    let s = planet.strangeness;
    let n_woody = ((1.0 - s) * 0.55 * count as f64).round().clamp(1.0, templates.len().max(1) as f64) as usize;
    let mut out = Vec::new();
    // Woody species from the templates, genome lightly jittered.
    for (i, t) in templates.iter().take(n_woody).enumerate() {
        let mut sp = t.clone();
        let k = i as u64;
        sp.name = format!("{}-{}", name(&st, 1000 + k), t.name);
        let j = |x: f64, a: u64| x * (0.85 + 0.3 * st.unit(k, a));
        sp.genome.height_m = j(sp.genome.height_m, 1);
        sp.genome.branch_angle_deg = j(sp.genome.branch_angle_deg, 2).clamp(5.0, 120.0);
        sp.genome.phyllotaxis_deg = (sp.genome.phyllotaxis_deg + 40.0 * s * st.signed(k, 3)).rem_euclid(360.0);
        sp.genome.organ_size_m = j(sp.genome.organ_size_m, 4);
        out.push(sp);
    }
    // Alien species: draw plans without repeating a plan before all were
    // used, weighted toward giants as strangeness grows.
    let mut bag: Vec<BodyPlan> = Vec::new();
    let template_niche = templates.first().map(|t| t.niche.clone());
    for k in 0..(count - out.len()) as u64 {
        if bag.is_empty() {
            bag = BodyPlan::ALL.to_vec();
        }
        let idx = (st.unit(k, 0) * bag.len() as f64) as usize % bag.len();
        let plan = bag.remove(idx);
        let giant = st.unit(k, 1) < 0.15 + 0.5 * s;
        let aff = affinity(plan, giant);
        let height = aff.height.0 + (aff.height.1 - aff.height.0) * st.unit(k, 2);
        let genome_plan = sample_plan(plan, height, &st, k);
        let t_mid = 0.5 * (aff.t.0 + aff.t.1) + 4.0 * st.signed(k, 3);
        let t_half = 0.5 * (aff.t.1 - aff.t.0);
        let niche = Niche {
            layer: if aff.canopy { Layer::Canopy } else { Layer::Shrub },
            temperature_c: Envelope { min: t_mid - t_half, max: t_mid + t_half, falloff: 6.0 },
            moisture: Envelope { min: aff.m.0, max: aff.m.1, falloff: 0.15 },
            height_m: Envelope { min: 0.0, max: 2500.0 + 1200.0 * st.unit(k, 4), falloff: 400.0 },
            max_slope_deg: 40.0,
            slope_falloff_deg: 8.0,
            soil_min: 0.0,
            prior: 0.6 + 0.8 * st.unit(k, 5),
            scale: (0.7, 1.3),
        };
        let mut genome = templates.first().map(|t| t.genome.clone()).unwrap_or_else(|| panic!("species templates needed"));
        genome.height_m = height;
        let _ = &template_niche;
        let key = name_key(&format!("{}-{k}", planet.body));
        let sp = SpeciesFile {
            schema: crate::genome::SCHEMA,
            name: format!("{}-{}", name(&st, k), plan_name(plan)),
            genome,
            plan: Some(genome_plan),
            niche,
            look_override: None,
            look: palette.alien_look(planet, key),
        };
        out.push(sp);
    }
    out
}

pub fn plan_name(p: BodyPlan) -> &'static str {
    use BodyPlan::*;
    match p {
        Cap => "cap",
        Coral => "coral",
        Anemone => "anemone",
        Gourd => "gourd",
        Bladder => "bladder",
        FrondFan => "frond",
        FlowerStalk => "flower",
        Crystal => "crystal",
    }
}
