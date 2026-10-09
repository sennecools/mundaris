use glam::DVec3;
use astrum_math::{
    Direction3,
    surface::{DirectionalCap, SurfaceLocation},
};
use astrum_world::terrain::*;

fn fixture() -> TerrainDefinition {
    let bands = std::array::from_fn(|i| {
        TerrainBandConfig::new(
            10.0 / (i as f64 + 1.0),
            TerrainScale::Metres {
                longest_wavelength_m: 4096.0 / 2f64.powi(i as i32),
            },
            2,
        )
        .unwrap()
    });
    TerrainDefinition::new(
        TerrainIdentity(9),
        TerrainSeed(4),
        TerrainGeneratorVersion::V1,
        TerrainConfig::new(
            bands,
            TerrainControls::new(0.1, 1.0, 0.4, 0.7, 0.1, 0.1).unwrap(),
        )
        .unwrap(),
    )
}
fn loc(v: DVec3) -> SurfaceLocation {
    SurfaceLocation::new(Direction3::try_new(v).unwrap())
}

fn direction(seed: u64, i: usize) -> DVec3 {
    // Seeded low-discrepancy area-weighted sphere survey; no ambient RNG.
    let z = 1.0 - 2.0 * (i as f64 + 0.5) / 4096.0;
    let phase = (seed as f64) * 0.6180339887498949;
    let a = i as f64 * 2.399963229728653 + phase;
    let r = (1.0 - z * z).sqrt();
    DVec3::new(r * a.cos(), r * a.sin(), z)
}

fn survey_definition(seed: u64) -> TerrainDefinition {
    let mut x = seed.wrapping_add(0x9e3779b97f4a7c15);
    let mut unit = || {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        (x.wrapping_mul(0x2545f4914f6cdd1d) >> 11) as f64 / (1u64 << 53) as f64
    };
    let bands = std::array::from_fn(|_| {
        let amp = 2.0 + unit() * 8.0;
        let wavelength = 2000.0 + unit() * 8000.0;
        TerrainBandConfig::new(
            amp,
            TerrainScale::Metres {
                longest_wavelength_m: wavelength,
            },
            2,
        )
        .unwrap()
    });
    // Include controls at both legal edges, not merely interior random values.
    let controls = match seed % 4 {
        0 => TerrainControls::new(-1.0, 0.01, 0.0, 0.0, 0.0, 1.0e-6).unwrap(),
        1 => TerrainControls::new(1.0, 8.0, 1.0, 1.0, 1.0, 1.0).unwrap(),
        _ => TerrainControls::new(
            unit() * 2.0 - 1.0,
            0.25 + unit() * 4.0,
            unit(),
            unit(),
            unit(),
            0.001 + unit() * 0.5,
        )
        .unwrap(),
    };
    TerrainDefinition::new(
        TerrainIdentity(seed + 100),
        TerrainSeed(seed),
        TerrainGeneratorVersion::V1,
        TerrainConfig::new(bands, controls).unwrap(),
    )
}

#[test]
fn compiled_field_is_deterministic_batch_equivalent_and_certified() {
    let def = fixture();
    let generator = TerrainGenerator::new(&def, 6.371e6).unwrap();
    let points: Vec<_> = (0..128)
        .map(|i| {
            let z = 1.0 - (i as f64 + 0.5) / 64.0;
            let a = i as f64 * 2.399963229728653;
            let r = (1.0 - z * z).sqrt();
            loc(DVec3::new(r * a.cos(), r * a.sin(), z))
        })
        .collect();
    let mut samples = vec![TerrainSample::default(); points.len()];
    let rho = TerrainFootprint::new(20.0).unwrap();
    let report = generator
        .evaluate_batch(&points, rho, &mut samples)
        .unwrap();
    for (p, s) in points.iter().zip(&samples) {
        assert_eq!(
            *s,
            generator
                .evaluate_point(TerrainQuery {
                    location: *p,
                    footprint: rho
                })
                .unwrap()
        );
        assert!(s.height_m().abs() <= def.config().absolute_height_bound_m());
        assert!(
            s.tangent_gradient_m_per_unit_direction().length()
                <= report.cartesian_height_gradient_bound_m()
        );
    }
    let cert = generator
        .bounds_for_region(
            DirectionalCap::new(Direction3::try_new(DVec3::X).unwrap(), 0.2).unwrap(),
            rho,
        )
        .unwrap();
    for s in &samples {
        assert!(
            (cert.height_interval_m()[0]..=cert.height_interval_m()[1]).contains(&s.height_m())
        );
    }
    assert!(
        cert.represented_height_bound_m() + cert.unresolved_height_bound_m()
            >= def.config().absolute_height_bound_m()
    );
}

#[test]
fn seeded_fields_obey_global_bounds_filter_residuals_and_derivatives() {
    let footprints = [0.0, 2.0, 1_000.0, 50_000.0].map(|m| TerrainFootprint::new(m).unwrap());
    for seed in 0..8 {
        let def = survey_definition(seed);
        let generator = TerrainGenerator::new(&def, 10_000.0).unwrap();
        let locations: Vec<_> = (0..4096).map(|i| loc(direction(seed, i))).collect();
        let mut per_profile = Vec::new();
        for footprint in footprints {
            let mut batch = vec![TerrainSample::default(); locations.len()];
            let report = generator
                .evaluate_batch(&locations, footprint, &mut batch)
                .unwrap();
            assert_eq!(report.sample_count(), locations.len());
            for (i, (location, sample)) in locations.iter().zip(&batch).enumerate() {
                let scalar = generator
                    .evaluate_point(TerrainQuery {
                        location: *location,
                        footprint,
                    })
                    .unwrap();
                assert_eq!(
                    *sample,
                    scalar,
                    "seed={seed}, i={i}, rho={}",
                    footprint.metres()
                );
                assert!(
                    sample.height_m().abs() <= def.config().absolute_height_bound_m(),
                    "seed={seed}, i={i}"
                );
            }
            per_profile.push(batch);
        }
        let full = &per_profile[0];
        // The same ordered input in fixed 32-sample chunks must preserve bits.
        for (profile_index, footprint) in footprints.iter().copied().enumerate() {
            for start in (0..locations.len()).step_by(32) {
                let mut chunk = vec![TerrainSample::default(); 32];
                generator
                    .evaluate_batch(&locations[start..start + 32], footprint, &mut chunk)
                    .unwrap();
                assert_eq!(
                    chunk,
                    per_profile[profile_index][start..start + 32],
                    "chunk seed={seed}"
                );
            }
        }
        for (profile, footprint) in per_profile.iter().zip(footprints).skip(1) {
            let cert = generator
                .bounds_for_region(
                    DirectionalCap::new(
                        Direction3::try_new(DVec3::X).unwrap(),
                        std::f64::consts::PI,
                    )
                    .unwrap(),
                    footprint,
                )
                .unwrap();
            for (i, (full_sample, filtered)) in full.iter().zip(profile).enumerate() {
                assert!(
                    (filtered.height_m() - full_sample.height_m()).abs()
                        <= cert.unresolved_height_bound_m(),
                    "residual seed={seed}, i={i}, rho={}, delta={}, bound={}",
                    footprint.metres(),
                    (filtered.height_m() - full_sample.height_m()).abs(),
                    cert.unresolved_height_bound_m()
                );
            }
        }
        // Independent tangent central differences, swept over steps to reject a
        // fortuitous step-size match. Avoid tiny gradients/poor conditioning.
        for i in (17..4096).step_by(61) {
            let n = direction(seed, i);
            let axis = if n.z.abs() < 0.8 { DVec3::Z } else { DVec3::Y };
            let tangent = n.cross(axis).normalize();
            let location = loc(n);
            let sample = generator
                .evaluate_point(TerrainQuery {
                    location,
                    footprint: TerrainFootprint::COMPLETE,
                })
                .unwrap();
            let analytic = sample.tangent_gradient_m_per_unit_direction().dot(tangent);
            if analytic.abs() < 1.0e-5 {
                continue;
            }
            for step in [1.0e-4, 1.0e-5, 1.0e-6] {
                let plus = loc((n + tangent * step).normalize());
                let minus = loc((n - tangent * step).normalize());
                let hp = generator
                    .evaluate_point(TerrainQuery {
                        location: plus,
                        footprint: TerrainFootprint::COMPLETE,
                    })
                    .unwrap()
                    .height_m();
                let hm = generator
                    .evaluate_point(TerrainQuery {
                        location: minus,
                        footprint: TerrainFootprint::COMPLETE,
                    })
                    .unwrap()
                    .height_m();
                let numeric = (hp - hm) / (2.0 * step);
                assert!(
                    (numeric - analytic).abs() <= analytic.abs().max(1.0) * 1.0e-4,
                    "derivative seed={seed}, i={i}, step={step}, analytic={analytic}, numeric={numeric}"
                );
            }
        }
    }
}

#[test]
fn invalid_derived_control_bounds_are_rejected_before_generator_publication() {
    let base = survey_definition(3);
    let controls = TerrainControls::new(0.0, f64::MAX, 0.5, 0.5, 1.0, f64::MIN_POSITIVE).unwrap();
    let config = TerrainConfig::new(
        std::array::from_fn(|i| base.config().band(TerrainBand::ALL[i])),
        controls,
    )
    .unwrap();
    let invalid = TerrainDefinition::new(
        TerrainIdentity(999),
        TerrainSeed(9),
        TerrainGeneratorVersion::V1,
        config,
    );
    assert!(matches!(
        TerrainGenerator::new(&invalid, 10_000.0),
        Err(TerrainError::InvalidConfig)
    ));
}

#[test]
fn zero_weight_fields_skip_primitives_and_footprint_taper_is_continuous() {
    let def = survey_definition(1);
    let generator = TerrainGenerator::new(&def, 10_000.0).unwrap();
    let locations: Vec<_> = (0..32).map(|i| loc(direction(1, i))).collect();
    let mut output = vec![TerrainSample::default(); locations.len()];
    let report = generator
        .evaluate_batch(
            &locations,
            TerrainFootprint::new(1.0e12).unwrap(),
            &mut output,
        )
        .unwrap();
    assert_eq!(report.primitive_calls(), 0);
    let a = generator
        .evaluate_point(TerrainQuery {
            location: locations[5],
            footprint: TerrainFootprint::new(20.0).unwrap(),
        })
        .unwrap();
    let b = generator
        .evaluate_point(TerrainQuery {
            location: locations[5],
            footprint: TerrainFootprint::new(20.0001).unwrap(),
        })
        .unwrap();
    assert!((a.height_m() - b.height_m()).abs() < 1.0e-3);
}
