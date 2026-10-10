//! Planet palette (genesis design §3, G1 prototype): leaf, bark and accent
//! colours from the star spectrum, the atmosphere and the planet seed.
//!
//! 1. Surface photon spectrum: Planck at the star's effective temperature,
//!    times a simple atmosphere (Rayleigh from pressure, absorber bands per
//!    composition), 380–1000 nm in 32 bins.
//! 2. Primary pigment: absorption bands centred near the photon-flux peak
//!    (Kiang et al. 2007: pigments follow where photons are plentiful), with
//!    a seeded offset that grows with strangeness, plus a short-wave
//!    accessory band. Reflectance = floor + span · (1 − absorption).
//! 3. Colour: CIE 1931 colour matching (Wyman, Sloan & Shirley 2013 fit),
//!    white-balanced to the planet's own surface light (von Kries), so a
//!    player adapted to the scene sees what the leaf reflects.
//! 4. Style pass in OKLCH: lightness band and chroma cap by the realism dial
//!    (plausible but stylised, DECISIONS.md), per-species hue offsets, state
//!    pigments (dry → carotenoid-like, cold → anthocyanin-like) mixed in by
//!    the species' niche, lighter young tips, bark from the ground hue,
//!    accents from a harmony rule.
//!
//! Everything is a pure function of the planet file and species names.

use serde::{Deserialize, Serialize};

use crate::genome::Look;
use crate::hash::{Stream, name_key};
use crate::niche::Layer;
use crate::SpeciesFile;

pub const PLANET_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Composition {
    /// Earth-like N₂/O₂ with water vapour.
    NitrogenOxygen,
    /// Thick CO₂.
    CarbonDioxide,
    /// Methane-rich haze (orange, Titan-like).
    Methane,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Atmosphere {
    pub pressure_bar: f64,
    pub composition: Composition,
}

/// Life-relevant facts of one planet (`content/flora/planets/<body>.ron`).
/// PROTOTYPE: the star temperature lives here until the scene carries
/// stellar data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanetLife {
    pub schema: u32,
    /// Scene body id.
    pub body: String,
    pub star_temperature_k: f64,
    pub atmosphere: Atmosphere,
    /// Mean ground albedo (linear RGB): bark and contrast reference.
    pub ground_albedo: [f64; 3],
    pub seed: u64,
    /// 0 Earth-analogue .. 1 exotic (DECISIONS.md: Rust 0.4).
    pub strangeness: f64,
    /// 0 grounded .. 1 stylised. Omitted: follows strangeness (0.5 at
    /// strangeness 0.4, rising to 0.85 at 0.7), so one value flips the look.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realism: Option<f64>,
    /// Surface age 0 fresh .. 1 old (rock weathering, genesis §6).
    #[serde(default)]
    pub geology_age: f64,
    /// Rock archetypes of the planet (`content/flora/rocks/<name>.ron`) and
    /// the bedrock hardness they occur on.
    #[serde(default)]
    pub rocks: Vec<PlanetRock>,
    /// Species generated for this planet (`species_gen.rs`), the authored
    /// species files serving as tree/shrub templates; 0 = authored only.
    #[serde(default)]
    pub generated_species: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanetRock {
    pub name: String,
    /// Tier A rock hardness 0..1 where this rock forms.
    pub hardness: crate::niche::Envelope,
}

impl PlanetLife {
    /// The realism dial, explicit or derived from strangeness.
    pub fn realism(&self) -> f64 {
        self.realism.unwrap_or_else(|| (0.5 + (self.strangeness - 0.4) * (0.35 / 0.3)).clamp(0.5, 1.0))
    }

    pub fn from_ron(text: &str) -> Result<Self, String> {
        let p: PlanetLife = ron::from_str(text).map_err(|e| e.to_string())?;
        if p.schema != PLANET_SCHEMA {
            return Err(format!("planet schema {} unsupported", p.schema));
        }
        if !(1000.0..=50_000.0).contains(&p.star_temperature_k)
            || !(0.0..=100.0).contains(&p.atmosphere.pressure_bar)
            || !(0.0..=1.0).contains(&p.strangeness)
            || !(0.0..=1.0).contains(&p.realism())
        {
            return Err("planet value out of range".into());
        }
        Ok(p)
    }
}

pub const BINS: usize = 32;
const LAMBDA0: f64 = 380.0;
const LAMBDA1: f64 = 1000.0;

pub fn wavelength(i: usize) -> f64 {
    LAMBDA0 + (LAMBDA1 - LAMBDA0) * (i as f64 + 0.5) / BINS as f64
}

fn gauss(x: f64, mu: f64, s1: f64, s2: f64) -> f64 {
    let s = if x < mu { s1 } else { s2 };
    (-0.5 * ((x - mu) / s).powi(2)).exp()
}

/// CIE 1931 2° colour matching functions (Wyman, Sloan & Shirley 2013,
/// multi-lobe fit).
pub fn cmf(l: f64) -> [f64; 3] {
    [
        1.056 * gauss(l, 599.8, 37.9, 31.0) + 0.362 * gauss(l, 442.0, 16.0, 26.7) - 0.065 * gauss(l, 501.1, 20.4, 26.2),
        0.821 * gauss(l, 568.8, 46.9, 40.5) + 0.286 * gauss(l, 530.9, 16.3, 31.1),
        1.217 * gauss(l, 437.0, 11.8, 36.0) + 0.681 * gauss(l, 459.0, 26.0, 13.8),
    ]
}

/// Photon flux per nm of a blackbody (arbitrary units).
fn planck_photons(l_nm: f64, t: f64) -> f64 {
    let l = l_nm * 1e-9;
    let c2 = 1.438_777e-2; // hc/k, m·K
    1.0 / (l.powi(4) * ((c2 / (l * t)).exp() - 1.0))
}

fn band(l: f64, centre: f64, width: f64, depth: f64) -> f64 {
    1.0 - depth * (-0.5 * ((l - centre) / width).powi(2)).exp()
}

/// Atmospheric transmission (sun ~45° high, airmass 1.4).
fn transmission(l: f64, a: &Atmosphere) -> f64 {
    let rayleigh = 0.1 * a.pressure_bar * (550.0 / l).powi(4);
    let mut t = (-1.4 * rayleigh).exp();
    let p = (a.pressure_bar / (1.0 + a.pressure_bar)).min(1.0);
    match a.composition {
        Composition::NitrogenOxygen => {
            t *= band(l, 762.0, 6.0, 0.8 * p) * band(l, 940.0, 25.0, 0.7 * p) * band(l, 720.0, 10.0, 0.3 * p);
        }
        Composition::CarbonDioxide => {
            t *= band(l, 960.0, 40.0, 0.5 * p) * (1.0 - 0.15 * p * (450.0 / l).powi(2)).max(0.0);
        }
        Composition::Methane => {
            // Organic haze dims the blue strongly; methane bands in the red/NIR.
            t *= (-(0.8 * p) * (450.0 / l).powi(3)).exp()
                * band(l, 619.0, 8.0, 0.4 * p)
                * band(l, 727.0, 12.0, 0.6 * p)
                * band(l, 890.0, 20.0, 0.9 * p);
        }
    }
    t
}

/// Surface photon spectrum at the 32 bins (normalised to max 1).
pub fn surface_spectrum(p: &PlanetLife) -> [f64; BINS] {
    let mut s = [0.0; BINS];
    for (i, v) in s.iter_mut().enumerate() {
        let l = wavelength(i);
        *v = planck_photons(l, p.star_temperature_k) * transmission(l, &p.atmosphere);
    }
    let m = s.iter().cloned().fold(0.0, f64::max).max(1e-30);
    s.map(|v| v / m)
}

/// Pigment absorption bands: (centre nm, width nm, depth).
#[derive(Debug, Clone, PartialEq)]
pub struct Pigment {
    pub bands: Vec<(f64, f64, f64)>,
}

impl Pigment {
    pub fn absorption(&self, l: f64) -> f64 {
        let mut keep = 1.0;
        for &(c, w, d) in &self.bands {
            keep *= band(l, c, w, d);
        }
        1.0 - keep
    }

    /// Reflectance with a floor and a span (leaves reflect little).
    pub fn reflectance(&self, l: f64) -> f64 {
        // Near-infrared edge: leaves reflect strongly beyond ~720 nm.
        let edge = 0.25 * crate::niche::smoothstep(700.0, 760.0, l);
        0.035 + 0.30 * (1.0 - self.absorption(l)) + edge * (1.0 - self.absorption(l))
    }
}

/// Photon-flux peak of the surface light within 400–950 nm (1 nm scan).
pub fn photon_peak(t: f64, a: &Atmosphere) -> f64 {
    let mut best = (550.0, -1.0);
    for l in 400..=950 {
        let l = l as f64;
        let v = planck_photons(l, t) * transmission(l, a);
        if v > best.1 {
            best = (l, v);
        }
    }
    best.0
}

/// Primary photosynthetic pigment of a planet.
pub fn primary_pigment(p: &PlanetLife) -> Pigment {
    let peak = photon_peak(p.star_temperature_k, &p.atmosphere);
    // Sun-like star under Earth-like air: the calibration point.
    let reference = photon_peak(5772.0, &Atmosphere { pressure_bar: 1.0, composition: Composition::NitrogenOxygen });
    let st = Stream::new(p.seed, 0x504c_5431);
    let offset = (10.0 + 50.0 * p.strangeness) * st.signed(0, 0);
    // Calibrated so a Sun-like star under Earth-like air gives chlorophyll
    // (absorbing ~665 nm): hotter stars push the band into the green
    // (yellow-orange-red foliage), cooler ones into the near infrared.
    let centre = (665.0 + 1.6 * (peak - reference) + offset).clamp(450.0, 760.0);
    let width = 50.0 + 0.3 * (centre - 700.0).max(0.0);
    let short = (0.65 * centre).clamp(430.0, 470.0);
    // Dim red stars: few visible photons, so plants absorb across the whole
    // visible as well (dark, violet-black foliage; Kiang et al. 2007).
    let dimness = ((peak - reference - 60.0) / 250.0).clamp(0.0, 1.0);
    Pigment {
        bands: vec![(centre, width, 0.97), (short, 40.0, 0.93 * (1.0 - dimness)), (590.0, 95.0, 0.9 * dimness)],
    }
}

/// Linear sRGB of a reflectance under the planet's light, white-balanced to
/// that light (white reflector → (1, 1, 1)).
pub fn reflectance_rgb(p: &PlanetLife, refl: impl Fn(f64) -> f64) -> [f64; 3] {
    let s = surface_spectrum(p);
    let (mut c, mut w) = ([0.0; 3], [0.0; 3]);
    for (i, sv) in s.iter().enumerate() {
        let l = wavelength(i);
        let m = cmf(l);
        let r = refl(l);
        for a in 0..3 {
            c[a] += sv * r * m[a];
            w[a] += sv * m[a];
        }
    }
    let rgb = xyz_to_srgb(c);
    let white = xyz_to_srgb(w);
    [0, 1, 2].map(|a| (rgb[a] / white[a].max(1e-9)).max(0.0))
}

fn xyz_to_srgb(c: [f64; 3]) -> [f64; 3] {
    [
        3.2406 * c[0] - 1.5372 * c[1] - 0.4986 * c[2],
        -0.9689 * c[0] + 1.8758 * c[1] + 0.0415 * c[2],
        0.0557 * c[0] - 0.2040 * c[1] + 1.0570 * c[2],
    ]
}

/// OKLab / OKLCH (Ottosson 2020).
pub fn srgb_to_oklab(c: [f64; 3]) -> [f64; 3] {
    let l = 0.4122214708 * c[0] + 0.5363325363 * c[1] + 0.0514459929 * c[2];
    let m = 0.2119034982 * c[0] + 0.6806995451 * c[1] + 0.1073969566 * c[2];
    let s = 0.0883024619 * c[0] + 0.2817188376 * c[1] + 0.6299787005 * c[2];
    let (l, m, s) = (l.max(0.0).cbrt(), m.max(0.0).cbrt(), s.max(0.0).cbrt());
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

pub fn oklab_to_srgb(c: [f64; 3]) -> [f64; 3] {
    let l = c[0] + 0.3963377774 * c[1] + 0.2158037573 * c[2];
    let m = c[0] - 0.1055613458 * c[1] - 0.0638541728 * c[2];
    let s = c[0] - 0.0894841775 * c[1] - 1.2914855480 * c[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        (4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s).max(0.0),
        (-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s).max(0.0),
        (-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s).max(0.0),
    ]
}

/// (L, C, h°)
pub fn to_lch(rgb: [f64; 3]) -> [f64; 3] {
    let [l, a, b] = srgb_to_oklab(rgb);
    [l, a.hypot(b), b.atan2(a).to_degrees().rem_euclid(360.0)]
}

pub fn from_lch(lch: [f64; 3]) -> [f64; 3] {
    let h = lch[2].to_radians();
    oklab_to_srgb([lch[0], lch[1] * h.cos(), lch[1] * h.sin()])
}

/// Planet-level palette: the colours every species is derived from.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    /// Physical foliage colour (before the style pass), linear RGB.
    pub physical: [f64; 3],
    /// Styled foliage, dry-stressed, cold-stressed (OKLCH).
    pub foliage: [f64; 3],
    pub dry: [f64; 3],
    pub cold: [f64; 3],
    pub bark: [f64; 3],
    pub accent: [f64; 3],
    pub ground: [f64; 3],
    pub pigment: Pigment,
}

/// Style pass: foliage lightness band and chroma cap by the realism dial.
fn style(lch: [f64; 3], realism: f64) -> [f64; 3] {
    // Above realism 0.5 the look turns stylised: brighter and much more
    // saturated (user reference 2026-10-10); at or below 0.5 unchanged.
    let stylised = ((realism - 0.5) / 0.5).clamp(0.0, 1.0);
    // Scale (not clamp) so dim-star foliage stays darker than sunlit green.
    let l = (0.75 * lch[0]).clamp(0.22, 0.45 + 0.12 * stylised);
    // Stylised floor: never grey foliage (no "blobs of nothingness").
    let c = (lch[1] * (1.0 + 0.6 * realism + 1.5 * stylised))
        .clamp(0.05 + 0.03 * realism, 0.07 + 0.05 * realism + 0.12 * stylised);
    [l, c, lch[2]]
}

impl Palette {
    pub fn for_planet(p: &PlanetLife) -> Self {
        let pigment = primary_pigment(p);
        let physical = reflectance_rgb(p, |l| pigment.reflectance(l));
        let foliage = style(to_lch(physical), p.realism());
        // Dry stress: primary pigment fades, a carotenoid-like band (blue)
        // shows: yellower, paler.
        let mut dry_p = pigment.clone();
        dry_p.bands[0].2 *= 0.55;
        dry_p.bands.push((470.0, 35.0, 0.8));
        let dry = style(to_lch(reflectance_rgb(p, |l| dry_p.reflectance(l))), p.realism());
        // Cold stress: the primary pigment breaks down and an anthocyanin-like
        // band (green) appears: redder (autumn colour).
        let mut cold_p = pigment.clone();
        cold_p.bands[0].2 *= 0.45;
        cold_p.bands.push((545.0, 38.0, 0.8));
        let cold = style(to_lch(reflectance_rgb(p, |l| cold_p.reflectance(l))), p.realism());
        let ground = to_lch(p.ground_albedo);
        // Bark: the ground's hue, dark and barely saturated.
        let bark = [0.33, 0.035 + 0.25 * ground[1].min(0.1), ground[2]];
        // Accent: harmony template from the seed (complementary or split).
        let st = Stream::new(p.seed, 0x504c_5432);
        let split = [180.0, 150.0, 210.0][(st.unit(0, 0) * 3.0) as usize % 3];
        let accent = [0.6, 0.15 + 0.04 * p.realism(), (foliage[2] + split).rem_euclid(360.0)];
        Palette { physical, foliage, dry, cold, bark, accent, ground, pigment }
    }

    /// Species look: planet foliage with a seeded hue offset and the state
    /// pigments its niche implies (cold-adapted species carry the cold hue,
    /// dry-adapted the dry hue).
    pub fn species_look(&self, p: &PlanetLife, sp: &SpeciesFile) -> Look {
        let st = Stream::new(name_key(&sp.name) ^ p.seed, 0x504c_5433);
        let n = &sp.niche;
        let t_mid = 0.5 * (n.temperature_c.min + n.temperature_c.max);
        let coldness = ((8.0 - t_mid) / 16.0).clamp(0.0, 1.0);
        let dryness = ((0.5 - n.moisture.min) / 0.35).clamp(0.0, 1.0);
        let mix = |a: [f64; 3], b: [f64; 3], t: f64| {
            // Mix in OKLab (hue-safe), back to LCh.
            let pa = from_lch(a);
            let pb = from_lch(b);
            to_lch([0, 1, 2].map(|i| pa[i] + (pb[i] - pa[i]) * t))
        };
        let mut leaf = mix(self.foliage, self.cold, 0.6 * coldness);
        leaf = mix(leaf, self.dry, 0.5 * dryness);
        let hue_jitter = (4.0 + 25.0 * p.strangeness) * st.signed(0, 0);
        leaf[2] = (leaf[2] + hue_jitter).rem_euclid(360.0);
        // Shrubs in the understory are a touch lighter; cold species darker.
        if n.layer == Layer::Shrub {
            leaf[0] += 0.03;
        }
        leaf[0] -= 0.05 * coldness;
        // Needles carry a thick waxy cuticle: glaucous, bluer and a little
        // darker (blue-spruce effect).
        if sp.genome.organ == crate::genome::OrganKind::Needle {
            leaf[2] = (leaf[2] + 28.0).rem_euclid(360.0);
            leaf[0] -= 0.03;
            leaf[1] *= 0.85;
        }
        let tip = [leaf[0] + 0.07, leaf[1] * 1.15, (leaf[2] + 4.0 * st.signed(1, 0)).rem_euclid(360.0)];
        let bark = [self.bark[0] + 0.04 * st.signed(2, 0), self.bark[1], (self.bark[2] + 12.0 * st.signed(3, 0)).rem_euclid(360.0)];
        let accent = [self.accent[0], self.accent[1], (self.accent[2] + 20.0 * st.signed(4, 0)).rem_euclid(360.0)];
        let f = |lch: [f64; 3]| from_lch(lch).map(|v| v.clamp(0.0, 1.0) as f32);
        Look { bark: f(bark), organ: f(leaf), organ_tip: f(tip), accent: f(accent) }
    }
}

/// Apply the planet palette to every species without a hand-set look.
pub fn apply(species: &mut [SpeciesFile], planet: &PlanetLife) {
    let palette = Palette::for_planet(planet);
    for sp in species.iter_mut() {
        sp.look = match &sp.look_override {
            Some(l) => l.clone(),
            None => palette.species_look(planet, sp),
        };
    }
}

/// The planet file for the life body (PROTOTYPE: Rust is the only one).
pub fn load_planet(dir: &std::path::Path, body: &str) -> Result<PlanetLife, String> {
    let path = dir.join(format!("{body}.ron"));
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    PlanetLife::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Chroma cap of the style pass at `realism` (alien species use it fully).
pub fn chroma_cap(realism: f64) -> f64 {
    let stylised = ((realism - 0.5) / 0.5).clamp(0.0, 1.0);
    0.07 + 0.05 * realism + 0.12 * stylised
}

impl Palette {
    /// The planet's hue families for alien species: the foliage hue and two
    /// more whose distance grows with strangeness (genesis §8.1: at most
    /// 2–3 dominant hue families per planet).
    pub fn hue_families(&self, p: &PlanetLife) -> [f64; 3] {
        let st = Stream::new(p.seed, 0x504c_5434);
        let h0 = self.foliage[2];
        let spread = 35.0 + 140.0 * p.strangeness;
        let d1 = spread * (0.7 + 0.3 * st.unit(0, 0));
        let d2 = -spread * (0.8 + 0.4 * st.unit(1, 0));
        [h0, (h0 + d1).rem_euclid(360.0), (h0 + d2).rem_euclid(360.0)]
    }

    /// Colours of a generated alien species: a hue family of the planet,
    /// saturated to the realism dial's chroma cap, lighter tips, darker
    /// stalks in the same hue, an accent from another family.
    pub fn alien_look(&self, p: &PlanetLife, key: u64) -> Look {
        let st = Stream::new(key ^ p.seed, 0x504c_5435);
        let fam = self.hue_families(p);
        let f = (st.unit(0, 0) * 3.0) as usize % 3;
        let h = (fam[f] + 14.0 * st.signed(1, 0)).rem_euclid(360.0);
        let cap = chroma_cap(p.realism());
        let c = cap * (0.75 + 0.25 * st.unit(2, 0));
        let l = 0.5 + 0.08 * st.signed(3, 0);
        let accent_h = (fam[(f + 1 + (st.unit(4, 0) * 2.0) as usize) % 3] + 20.0 * st.signed(5, 0)).rem_euclid(360.0);
        let lin = |lch: [f64; 3]| from_lch(lch).map(|v| v.clamp(0.0, 1.0) as f32);
        Look {
            organ: lin([l, c, h]),
            organ_tip: lin([l + 0.12, c * 1.1, (h + 15.0).rem_euclid(360.0)]),
            accent: lin([0.62, cap * 1.05, accent_h]),
            bark: lin([0.36, c * 0.45, (h - 10.0).rem_euclid(360.0)]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planet(t: f64) -> PlanetLife {
        PlanetLife {
            schema: 1,
            body: "test".into(),
            star_temperature_k: t,
            atmosphere: Atmosphere { pressure_bar: 1.0, composition: Composition::NitrogenOxygen },
            ground_albedo: [0.2, 0.15, 0.1],
            seed: 0,
            strangeness: 0.0,
            realism: Some(0.0),
            geology_age: 0.5,
            rocks: Vec::new(),
            generated_species: 0,
        }
    }

    #[test]
    fn oklab_round_trips() {
        for c in [[0.1, 0.2, 0.05], [0.5, 0.5, 0.5], [0.02, 0.01, 0.3]] {
            let back = oklab_to_srgb(srgb_to_oklab(c));
            for a in 0..3 {
                assert!((back[a] - c[a]).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn white_reflector_is_white() {
        let rgb = reflectance_rgb(&planet(5772.0), |_| 1.0);
        for v in rgb {
            assert!((v - 1.0).abs() < 1e-9);
        }
    }

    #[test]
    fn sun_like_star_gives_green_leaves() {
        let p = planet(5772.0);
        let lch = Palette::for_planet(&p).foliage;
        assert!((115.0..165.0).contains(&lch[2]), "hue {lch:?}");
    }

    #[test]
    fn dim_red_star_gives_darker_foliage_than_a_sun() {
        let sun = Palette::for_planet(&planet(5772.0)).physical;
        let m = Palette::for_planet(&planet(3200.0)).physical;
        let y = |c: [f64; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        assert!(y(m) < y(sun), "M {m:?} sun {sun:?}");
    }
}
