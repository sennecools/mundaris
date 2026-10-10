//! Forest placement (CPU reference): far tint = expected placed cover,
//! niche hard limits.

use astrum_flora::niche::{Layer, Site};
use astrum_flora::scatter::Placement;
use astrum_flora::{SpeciesFile, load_species_dir};

fn species() -> Vec<SpeciesFile> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/flora/species");
    load_species_dir(&dir).unwrap()
}

/// The band-limited far tint (all octaves faded) equals the mean cover of
/// the placed plants over the noise, for a spread of climates.
#[test]
fn far_tint_matches_expected_placed_cover() {
    let species = species();
    let p = Placement { species: &species };
    for t in [-12.0, -8.0, 0.0, 15.0] {
        for m in [0.3, 0.35, 0.5] {
            let s = Site { temperature_c: t, moisture: m, height_m: 500.0, slope: 0.05, sediment: 0.5 };
            let n = 20_000;
            let placed: f64 = (0..n)
                .map(|i| p.forest(2, (i % 141) as f64 * 977.3 + 13.0, (i / 141) as f64 * 811.7 + 7.0, 0.0, &s).cover)
                .sum::<f64>()
                / n as f64;
            let far = p.forest(2, 1234.0, 5678.0, 400.0, &s).cover;
            assert!((far - placed).abs() <= 0.03 * placed.max(0.05), "T {t} M {m}: placed {placed:.3} far {far:.3}");
        }
    }
}

#[test]
fn nothing_grows_under_water_or_above_its_treeline() {
    let species = species();
    let p = Placement { species: &species };
    for (k, sp) in species.iter().enumerate() {
        let top = sp.niche.height_m.max + sp.niche.height_m.falloff;
        for h in [-50.0, -1.0, 0.0, top + 1.0, top + 500.0] {
            let s = Site { temperature_c: 10.0, moisture: 0.6, height_m: h, slope: 0.0, sediment: 0.5 };
            assert_eq!(p.suit(k, &s), 0.0, "{} at {h} m", sp.name);
        }
    }
    let s = Site { temperature_c: 10.0, moisture: 0.6, height_m: -5.0, slope: 0.0, sediment: 0.5 };
    let f = p.forest(2, 100.0, 100.0, 0.0, &s);
    assert_eq!(f.tree, 0.0);
    assert_eq!(f.shrub, 0.0);
    assert!(p.pick(Layer::Canopy, &s, 0.5).is_none());
}
