use glam::DVec3;
use astrum_math::{
    Direction3,
    surface::{DirectionalCap, SurfaceLocation},
};
use astrum_world::terrain::*;

const RADIUS: f64 = 6_371_000.0;

fn definition(seed: u64, octaves: u8, strength: f64) -> TerrainGenerator {
    let bands = [
        TerrainBandConfig::new(
            80.0,
            TerrainScale::Angular {
                lowest_cycles_per_body: 2.0,
            },
            2,
        )
        .unwrap(),
        TerrainBandConfig::new(
            40.0,
            TerrainScale::Metres {
                longest_wavelength_m: 800_000.0,
            },
            2,
        )
        .unwrap(),
        TerrainBandConfig::new(
            600.0,
            TerrainScale::Metres {
                longest_wavelength_m: 16_384.0,
            },
            3,
        )
        .unwrap(),
        TerrainBandConfig::new(
            20.0,
            TerrainScale::Metres {
                longest_wavelength_m: 512.0,
            },
            2,
        )
        .unwrap(),
        TerrainBandConfig::new(
            4.0,
            TerrainScale::Metres {
                longest_wavelength_m: 64.0,
            },
            1,
        )
        .unwrap(),
    ];
    let controls = TerrainControls::new(0.0, 1.0, 0.5, 1.0, 0.1, 0.1).unwrap();
    let config = TerrainConfig::new(bands, controls)
        .unwrap()
        .with_erosion(ErosionConfig::new(octaves, strength).unwrap());
    let d = TerrainDefinition::new(
        TerrainIdentity(42),
        TerrainSeed(seed),
        TerrainGeneratorVersion::V2,
        config,
    );
    TerrainGenerator::new(&d, RADIUS).unwrap()
}

fn location(v: DVec3) -> SurfaceLocation {
    SurfaceLocation::new(Direction3::try_new(v).unwrap())
}
fn query(v: DVec3, footprint: f64) -> TerrainQuery {
    TerrainQuery {
        location: location(v),
        footprint: TerrainFootprint::new(footprint).unwrap(),
    }
}
fn direction(i: usize) -> DVec3 {
    let z = 1.0 - 2.0 * (i as f64 + 0.5) / 4096.0;
    let a = i as f64 * 2.399963229728653;
    let r = (1.0 - z * z).sqrt();
    DVec3::new(r * a.cos(), r * a.sin(), z)
}

#[test]
fn feedback_derivatives_and_diagnostic_contract() {
    let steps = [1e-9, 3e-9, 1e-8, 3e-8, 1e-7];
    let (mut best, mut worst) = (f64::INFINITY, 0.0f64);
    let mut worst_absolute = 0.0f64;
    let mut errors = Vec::new();
    for octaves in [1, 3, 5] {
        for strength in [0.0, 0.35, 1.0] {
            let field = definition(0x5eed, octaves, strength);
            for n in (0..160).map(|i| direction(i * 17 + 3)).chain([
                DVec3::X,
                -DVec3::X,
                DVec3::Y,
                -DVec3::Y,
                DVec3::Z,
                -DVec3::Z,
                DVec3::ONE.normalize(),
                -DVec3::ONE.normalize(),
            ]) {
                let tangent = n
                    .cross(if n.x.abs() < 0.8 { DVec3::X } else { DVec3::Y })
                    .normalize();
                for footprint in [50_000.0, 10_000.0, 1_000.0, 100.0, 10.0, 2.0, 0.0] {
                    let q = query(n, footprint);
                    let sample = field.evaluate_point(q).unwrap();
                    let analytic = sample.tangent_gradient_m_per_unit_direction().dot(tangent);
                    let error = steps
                        .iter()
                        .map(|h| {
                            let plus = field
                                .evaluate_point(query(n + *h * tangent, footprint))
                                .unwrap()
                                .height_m();
                            let minus = field
                                .evaluate_point(query(n - *h * tangent, footprint))
                                .unwrap()
                                .height_m();
                            let half_plus = field
                                .evaluate_point(query(n + *h * 0.5 * tangent, footprint))
                                .unwrap()
                                .height_m();
                            let half_minus = field
                                .evaluate_point(query(n - *h * 0.5 * tangent, footprint))
                                .unwrap()
                                .height_m();
                            let reference = (4.0 * (half_plus - half_minus) / h
                                - (plus - minus) / (2.0 * h))
                                / 3.0;
                            (reference - analytic).abs() / analytic.abs().max(1.0)
                        })
                        .fold(f64::INFINITY, f64::min);
                    best = best.min(error);
                    worst = worst.max(error);
                    worst_absolute = worst_absolute.max(error * analytic.abs().max(1.0));
                    errors.push(error);
                    assert!(
                        error < 1e-4,
                        "octaves={octaves} strength={strength} footprint={footprint} scaled_error={error}"
                    );
                }
            }
        }
    }
    errors.sort_by(f64::total_cmp);
    eprintln!(
        "V2 tangent FD samples={}, best_scaled={best:e}, worst_scaled={worst:e}, p95_scaled={:e}, worst_absolute={worst_absolute:e}m/unit direction",
        errors.len(),
        errors[errors.len() * 95 / 100]
    );

    for strength in [0.0, 1.0] {
        let field = definition(81, 3, strength);
        let q = query(direction(313), 2.0);
        let production = field.evaluate_point(q).unwrap();
        let diagnostic = field.evaluate_without_erosion_feedback(q).unwrap();
        if strength == 0.0 {
            assert_eq!(production, diagnostic);
        } else {
            let largest_difference = (0..128)
                .map(|i| {
                    let q = query(direction(i * 31 + 7), 2.0);
                    (field.evaluate_point(q).unwrap().height_m()
                        - field
                            .evaluate_without_erosion_feedback(q)
                            .unwrap()
                            .height_m())
                    .abs()
                })
                .fold(0.0, f64::max);
            assert!(
                largest_difference > 0.01,
                "feedback difference={largest_difference}"
            );
            eprintln!(
                "feedback maximum height difference={largest_difference}m across 128 complete-active directions"
            );
        }
        let one = definition(81, 1, strength);
        assert_eq!(
            one.evaluate_point(q).unwrap(),
            one.evaluate_without_erosion_feedback(q).unwrap()
        );
    }
}

#[test]
fn deterministic_batches_certificates_and_cube_singular_directions() {
    let footprints = [50_000.0, 10_000.0, 1_000.0, 100.0, 10.0, 2.0];
    let directions: Vec<_> = (0..512)
        .map(|i| direction((i * 7) % 4096))
        .chain([
            DVec3::X,
            -DVec3::X,
            DVec3::Y,
            -DVec3::Y,
            DVec3::Z,
            -DVec3::Z,
            DVec3::new(1.0, 1.0, 1.0).normalize(),
            DVec3::new(-1.0, 1.0, -1.0).normalize(),
        ])
        .collect();
    for octaves in 1..=5 {
        for strength in [0.0, 1.0] {
            for seed in [0, 0xdead_beef] {
                let field = definition(seed, octaves, strength);
                for footprint in footprints {
                    let fp = TerrainFootprint::new(footprint).unwrap();
                    let locs: Vec<_> = directions.iter().copied().map(location).collect();
                    let mut batch = vec![TerrainSample::default(); locs.len()];
                    field.evaluate_batch(&locs, fp, &mut batch).unwrap();
                    for (chunk, out) in locs.chunks(32).zip(batch.chunks(32)) {
                        let mut chunked = vec![TerrainSample::default(); chunk.len()];
                        field.evaluate_batch(chunk, fp, &mut chunked).unwrap();
                        assert_eq!(out, chunked);
                    }
                    let cap = DirectionalCap::new(
                        Direction3::try_new(DVec3::X).unwrap(),
                        std::f64::consts::PI,
                    )
                    .unwrap();
                    let cert = field.bounds_for_region(cap, fp).unwrap();
                    let full_cert = field
                        .bounds_for_region(cap, TerrainFootprint::COMPLETE)
                        .unwrap();
                    let [lo, hi] = cert.height_interval_m();
                    for (&n, sample) in directions.iter().zip(&batch) {
                        assert!(sample.height_m() >= lo && sample.height_m() <= hi);
                        assert!(
                            sample.tangent_gradient_m_per_unit_direction().length()
                                <= cert.cartesian_height_gradient_bound_m()
                        );
                        assert!(
                            (sample
                                .normal_body(location(n), RADIUS)
                                .unwrap()
                                .unit()
                                .length()
                                - 1.0)
                                .abs()
                                < 1e-12
                        );
                        let full = field
                            .evaluate_point(TerrainQuery {
                                location: location(n),
                                footprint: TerrainFootprint::COMPLETE,
                            })
                            .unwrap();
                        assert!(
                            (full.height_m() - sample.height_m()).abs()
                                <= cert.unresolved_height_bound_m()
                        );
                    }
                    assert!(cert.unresolved_height_bound_m() >= 0.0);
                    assert!(
                        cert.cartesian_height_gradient_bound_m()
                            <= full_cert.cartesian_height_gradient_bound_m()
                    );
                }
            }
        }
    }
    for octaves in 1..=5 {
        assert!(ErosionConfig::new(octaves, 1.0).is_ok());
    }
    assert!(ErosionConfig::new(0, 0.5).is_err());
    assert!(ErosionConfig::new(6, 0.5).is_err());
}

#[test]
fn coarse_filter_rejects_features_and_activation_is_smooth() {
    let generator = definition(17, 3, 1.0);
    let n = direction(313);
    let coarse = generator.erosion_diagnostics(query(n, 50_000.0)).unwrap();
    assert_eq!(coarse.report.erosion_feature_evaluations(), 0);
    assert_eq!(coarse.report.active_erosion_octaves(), 0);
    assert_eq!(coarse.report.skipped_erosion_octaves(), 3);
    assert_eq!(coarse.contribution_m, 0.0);
    let fine = generator.erosion_diagnostics(query(n, 2.0)).unwrap();
    assert_eq!(fine.report.active_erosion_octaves(), 3);
    assert!(fine.report.erosion_feature_evaluations() > 0);
    assert!(fine.contribution_m <= 0.0);
    assert!((0.0..=1.0).contains(&fine.mountain_mask));
    for footprint in (1..=300).map(|i| i as f64 * 2.0) {
        let a = generator.evaluate_point(query(n, footprint)).unwrap();
        let b = generator
            .evaluate_point(query(n, footprint + 1e-4))
            .unwrap();
        assert!((a.height_m() - b.height_m()).abs() < 0.01);
    }
}
