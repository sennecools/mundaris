//! Grower checks: determinism, golden bytes, scorer bands for every species.

use std::path::Path;

use astrum_flora::genome::{FoliageStyle, GrowthForm, OrganKind};
use astrum_flora::metrics::Bands;
use astrum_flora::params::{ParamKind, ParamValue, Params};
use astrum_flora::{Genome, Kit, SpeciesFile, grow_plant, load_species_dir, variant_seed};

fn species() -> Vec<SpeciesFile> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/flora/species");
    // Authored species with the planet palette (not the generated set).
    let mut s = load_species_dir(&dir).unwrap();
    let planet = astrum_flora::palette::load_planet(&dir.join("../planets"), "rust").unwrap();
    astrum_flora::palette::apply(&mut s, &planet);
    s
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[test]
fn same_seed_same_bytes() {
    let kit = Kit::builtin();
    for sp in species() {
        let seed = variant_seed(&sp, 0);
        let a = grow_plant(&sp, &kit, seed);
        let b = grow_plant(&sp, &kit, seed);
        for l in 0..astrum_flora::LOD_COUNT {
            assert_eq!(
                a.lods[l].to_bytes(),
                b.lods[l].to_bytes(),
                "{} lod{l}",
                sp.name
            );
        }
        let c = grow_plant(&sp, &kit, variant_seed(&sp, 1));
        assert_ne!(
            a.lods[0].to_bytes(),
            c.lods[0].to_bytes(),
            "{}: variants differ",
            sp.name
        );
    }
}

/// Golden image of one plant. Same machine/toolchain only: growth uses libm
/// `powf`/`sin`/`cos`, which are not guaranteed bit-identical across
/// platforms (genesis §9.3 determinism scope is still open). Update the
/// constant deliberately when the grower changes.
const GOLDEN_BUSHEL_12345: u64 = 0x67c2_3fce_1565_6532;

#[test]
fn golden_seed() {
    let kit = Kit::builtin();
    let sp = species().into_iter().find(|s| s.name == "bushel").unwrap();
    let p = grow_plant(&sp, &kit, 12345);
    let h = fnv(&p.lods[0].to_bytes());
    println!(
        "bushel seed 12345 lod0 fnv {h:#018x} tris {}",
        p.metrics.triangles[0]
    );
    assert_eq!(
        h, GOLDEN_BUSHEL_12345,
        "grower output changed; update the golden if intended"
    );
}

#[test]
fn species_pass_scorer_bands() {
    let kit = Kit::builtin();
    // Both foliage styles (the realistic leaves stay selectable).
    for style in [FoliageStyle::Stylised, FoliageStyle::Realistic] {
        for mut sp in species() {
            sp.style = style;
            for v in 0..3 {
                let p = grow_plant(&sp, &kit, variant_seed(&sp, v));
                let fails = p.metrics.failures(&Bands::for_style(sp.style));
                assert!(fails.is_empty(), "{} {style:?} v{v}: {fails:?}", sp.name);
            }
        }
    }
}

#[test]
fn species_roundtrip_ron() {
    for sp in species() {
        let text = sp.to_ron();
        // The resolved look is derived (palette), not stored.
        let mut expected = sp.clone();
        expected.look = Default::default();
        assert_eq!(SpeciesFile::from_ron(&text).unwrap(), expected);
        assert!(!text.contains("look"), "{text}");
    }
}

#[test]
fn every_form_grows() {
    let kit = Kit::builtin();
    let base = species().into_iter().find(|s| s.name == "bushel").unwrap();
    for form in GrowthForm::ALL {
        for organ in [OrganKind::Blade, OrganKind::Membrane, OrganKind::Leafless] {
            let mut sp = base.clone();
            sp.genome.form = *form;
            sp.genome.organ = organ;
            let p = grow_plant(&sp, &kit, 7);
            assert!(p.metrics.structure.connected, "{form:?}");
            assert!(p.metrics.triangles[0] > 0, "{form:?} {organ:?}");
            assert!(
                p.lods[0]
                    .vertices
                    .iter()
                    .all(|v| v.position.iter().all(|c| c.is_finite()))
            );
        }
    }
}

#[test]
fn params_check_ranges_and_kinds() {
    let mut g = species()[0].genome.clone();
    assert!(g.set_param("height_m", ParamValue::Float(12.0)).is_ok());
    assert_eq!(g.height_m, 12.0);
    assert!(g.set_param("height_m", ParamValue::Float(1e9)).is_err());
    assert!(g.set_param("height_m", ParamValue::Int(3)).is_err());
    assert!(g.set_param("nope", ParamValue::Float(1.0)).is_err());
    assert!(
        g.set_param("form", ParamValue::Choice(GrowthForm::Shrub.index()))
            .is_ok()
    );
    assert_eq!(g.form, GrowthForm::Shrub);
    // Choice labels match the enum variant names.
    for d in Genome::descriptors() {
        if let ParamKind::Choice { options } = d.kind {
            for (i, o) in options.iter().enumerate() {
                let mut g2 = g.clone();
                (d.set)(&mut g2, ParamValue::Choice(i));
                let ron = ron::to_string(&g2).unwrap();
                assert!(
                    ron.contains(&format!("{}:{o}", d.key)),
                    "{} {o}: {ron}",
                    d.key
                );
            }
        }
    }
}

/// Studio grows and rasterises off the UI thread (LANES.md interface log).
#[test]
fn growth_types_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<SpeciesFile>();
    check::<Kit>();
    check::<astrum_flora::GrownPlant>();
    check::<astrum_flora::raster::Canvas>();
    check::<astrum_flora::raster::Camera>();
}
