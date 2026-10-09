use glam::DVec3;
use astrum_math::{Direction3, surface::SurfaceLocation};
use astrum_world::terrain::*;

const RADIUS: f64 = 6_371_000.0;

fn generator(version: TerrainGeneratorVersion) -> TerrainGenerator {
    let bands = [
        (80.0, 1_000_000.0, 2),
        (40.0, 800_000.0, 2),
        (600.0, 16_384.0, 3),
        (20.0, 512.0, 2),
        (4.0, 64.0, 1),
    ]
    .map(|(amplitude, wavelength, octaves)| {
        TerrainBandConfig::new(
            amplitude,
            TerrainScale::Metres {
                longest_wavelength_m: wavelength,
            },
            octaves,
        )
        .unwrap()
    });
    let controls = TerrainControls::new(0.0, 1.0, 0.5, 1.0, 0.1, 0.1).unwrap();
    let config = TerrainConfig::new(bands, controls)
        .unwrap()
        .with_erosion(ErosionConfig::new(3, 0.8).unwrap());
    let def = TerrainDefinition::new(TerrainIdentity(42), TerrainSeed(17), version, config);
    TerrainGenerator::new(&def, RADIUS).unwrap()
}

fn sample(g: &TerrainGenerator, v: DVec3, footprint: f64) -> f64 {
    let location = SurfaceLocation::new(Direction3::try_new(v).unwrap());
    g.evaluate_point(TerrainQuery {
        location,
        footprint: TerrainFootprint::new(footprint).unwrap(),
    })
    .unwrap()
    .height_m()
}

#[test]
fn footprint_profile_difference_is_certified_for_both_generators() {
    let footprints = [0.0, 32.0, 128.0, 512.0, 2_048.0, 16_384.0, 100_000.0];
    for version in [TerrainGeneratorVersion::V1, TerrainGeneratorVersion::V2] {
        let g = generator(version);
        for &a in &footprints {
            assert_eq!(
                g.profile_difference_bound_m(
                    TerrainFootprint::new(a).unwrap(),
                    TerrainFootprint::new(a).unwrap()
                )
                .unwrap(),
                0.0
            );
            for &b in &footprints {
                let bound = g
                    .profile_difference_bound_m(
                        TerrainFootprint::new(a).unwrap(),
                        TerrainFootprint::new(b).unwrap(),
                    )
                    .unwrap();
                for i in 0..24 {
                    let z = 1.0 - 2.0 * (i as f64 + 0.5) / 24.0;
                    let angle = i as f64 * 2.399963229728653;
                    let dir = DVec3::new(
                        (1.0 - z * z).sqrt() * angle.cos(),
                        (1.0 - z * z).sqrt() * angle.sin(),
                        z,
                    );
                    let difference = (sample(&g, dir, a) - sample(&g, dir, b)).abs();
                    assert!(
                        difference <= bound,
                        "{version:?} {a}->{b}: {difference} > {bound}"
                    );
                }
            }
        }
    }
}

#[test]
fn coarse_to_fine_difference_bound_covers_filtered_tails() {
    for version in [TerrainGeneratorVersion::V1, TerrainGeneratorVersion::V2] {
        let g = generator(version);
        let coarse = TerrainFootprint::new(100_000.0).unwrap();
        let fine = TerrainFootprint::new(0.0).unwrap();
        let bound = g.profile_difference_bound_m(coarse, fine).unwrap();
        assert!(bound > 0.0);
        for v in [DVec3::X, DVec3::Y, DVec3::Z, DVec3::new(1.0, -2.0, 3.0)] {
            assert!((sample(&g, v, 100_000.0) - sample(&g, v, 0.0)).abs() <= bound);
        }
    }
}
