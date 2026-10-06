use glam::DVec3;
use mundaris_math::{
    Direction3,
    surface::{CubeFace, CubePatchAddress, DirectionalCap, SurfaceLocation},
};
use mundaris_world::terrain::*;

fn definition(seed: u64, version: TerrainGeneratorVersion) -> TerrainDefinition {
    let flat = TerrainBandConfig::new(
        0.0,
        TerrainScale::Metres {
            longest_wavelength_m: 1024.0,
        },
        1,
    )
    .unwrap();
    let config = TerrainConfig::new(
        [flat; 5],
        TerrainControls::new(0.0, 1.0, 0.0, 0.0, 0.0, 0.4).unwrap(),
    )
    .unwrap()
    .with_crater_field(CraterFieldConfig::new(8, 500.0, 5000.0, 0.02, 0.006).unwrap())
    .unwrap();
    TerrainDefinition::new(TerrainIdentity(41), TerrainSeed(seed), version, config)
}

#[test]
fn crater_field_requires_explicit_algorithm_and_valid_combined_radius() {
    for version in [TerrainGeneratorVersion::V1, TerrainGeneratorVersion::V2] {
        assert!(definition(17, version).validate_radius(100_000.0).is_err());
    }
    let good = definition(17, TerrainGeneratorVersion::CrateredV1);
    assert!(good.validate_radius(100_000.0).is_ok());
    assert!(good.validate_radius(1000.0).is_err());
    let no_craters = TerrainDefinition::new(
        good.identity(),
        good.seed(),
        good.version(),
        TerrainConfig::new(
            [TerrainBandConfig::new(
                0.0,
                TerrainScale::Metres {
                    longest_wavelength_m: 1024.0,
                },
                1,
            )
            .unwrap(); 5],
            TerrainControls::new(0.0, 1.0, 0.0, 0.0, 0.0, 0.4).unwrap(),
        )
        .unwrap(),
    );
    assert!(no_craters.validate_radius(100_000.0).is_err());
}

#[test]
fn crater_catalogue_and_samples_are_independent_of_query_order_and_cube_face() {
    let generator = TerrainGenerator::new(
        &definition(17, TerrainGeneratorVersion::CrateredV1),
        100_000.0,
    )
    .unwrap();
    let mut locations = vec![];
    for face in CubeFace::ALL {
        let address = CubePatchAddress::root(face);
        for (x, y) in [(0, 0), (0, 8), (8, 0), (8, 8), (16, 16)] {
            locations.push(SurfaceLocation::new(
                address.sample_direction(x, y, 16).unwrap(),
            ));
        }
    }
    locations.extend(
        generator
            .crater_features()
            .iter()
            .map(|feature| SurfaceLocation::new(feature.center())),
    );
    let mut batch = vec![TerrainSample::default(); locations.len()];
    let report = generator
        .evaluate_batch(&locations, TerrainFootprint::COMPLETE, &mut batch)
        .unwrap();
    assert_eq!(report.sample_count(), locations.len());
    for (location, expected) in locations.iter().zip(&batch).rev() {
        let actual = generator
            .evaluate_point(TerrainQuery {
                location: *location,
                footprint: TerrainFootprint::COMPLETE,
            })
            .unwrap();
        assert_eq!(actual, *expected);
    }
    for (i, a) in locations.iter().enumerate() {
        for (j, b) in locations.iter().enumerate() {
            if a.direction() == b.direction() {
                assert_eq!(batch[i], batch[j]);
            }
        }
    }
}

#[test]
fn local_crater_bounds_include_complete_and_filtered_truth() {
    let generator = TerrainGenerator::new(
        &definition(17, TerrainGeneratorVersion::CrateredV1),
        100_000.0,
    )
    .unwrap();
    for feature in generator.crater_features() {
        let axis = feature.center();
        let tangent = axis
            .unit()
            .cross(if axis.unit().y.abs() < 0.9 {
                DVec3::Y
            } else {
                DVec3::X
            })
            .normalize();
        let cap =
            DirectionalCap::new(axis, feature.radius_m() * 1.5 / generator.radius_m()).unwrap();
        for footprint_m in [0.0, 10.0, 100.0, 1000.0] {
            let footprint = TerrainFootprint::new(footprint_m).unwrap();
            let bounds = generator.bounds_for_region(cap, footprint).unwrap();
            let [lo, hi] = bounds.height_interval_m();
            for i in 0..129 {
                let angle = cap.half_angle_rad() * i as f64 / 128.0;
                let location = SurfaceLocation::new(
                    Direction3::try_new(axis.unit() * angle.cos() + tangent * angle.sin()).unwrap(),
                );
                let full = generator
                    .evaluate_point(TerrainQuery {
                        location,
                        footprint: TerrainFootprint::COMPLETE,
                    })
                    .unwrap();
                let filtered = generator
                    .evaluate_point(TerrainQuery {
                        location,
                        footprint,
                    })
                    .unwrap();
                assert!(full.height_m() >= lo && full.height_m() <= hi);
                assert!(filtered.height_m() >= lo && filtered.height_m() <= hi);
                assert!(
                    (full.height_m() - filtered.height_m()).abs()
                        <= bounds.unresolved_height_bound_m()
                );
                assert!(
                    filtered.tangent_gradient_m_per_unit_direction().length()
                        <= bounds.cartesian_height_gradient_bound_m()
                );
            }
        }
    }
    let diagnostics = generator
        .erosion_diagnostics(TerrainQuery {
            location: SurfaceLocation::new(generator.crater_features()[0].center()),
            footprint: TerrainFootprint::COMPLETE,
        })
        .unwrap();
    assert_eq!(diagnostics.contribution_m, 0.0);
}

#[test]
fn crater_landmarks_are_identical_across_footprints_and_shared_directions() {
    let generator = TerrainGenerator::new(
        &definition(17, TerrainGeneratorVersion::CrateredV1),
        100_000.0,
    )
    .unwrap();
    let fine = TerrainFootprint::COMPLETE;
    let coarse = TerrainFootprint::new(1000.0).unwrap();
    let global = generator.profile_difference_bound_m(fine, coarse).unwrap();
    assert_eq!(global, 0.0);
    let mut omitted = false;
    for feature in generator.crater_features() {
        let cap = DirectionalCap::new(
            feature.center(),
            1.5 * feature.radius_m() / generator.radius_m(),
        )
        .unwrap();
        let a = generator.bounds_for_region(cap, fine).unwrap();
        let b = generator.bounds_for_region(cap, coarse).unwrap();
        assert_eq!(a.height_interval_m(), b.height_interval_m());
        assert_eq!(
            a.represented_height_bound_m(),
            b.represented_height_bound_m()
        );
        assert_eq!(
            a.cartesian_height_gradient_bound_m(),
            b.cartesian_height_gradient_bound_m()
        );
        assert_eq!(
            a.cartesian_height_hessian_bound_m(),
            b.cartesian_height_hessian_bound_m()
        );
        assert_eq!(b.unresolved_height_bound_m(), 0.0);
        assert!(b.cartesian_height_gradient_bound_m() > 0.0);
        assert!(b.cartesian_height_hessian_bound_m() > 0.0);
    }
    for axis in [
        DVec3::X,
        DVec3::Y,
        DVec3::Z,
        -DVec3::X,
        -DVec3::Y,
        -DVec3::Z,
    ]
    .into_iter()
    .chain(
        generator
            .crater_features()
            .iter()
            .map(|f| f.center().unit()),
    ) {
        let cap = DirectionalCap::new(Direction3::try_new(axis).unwrap(), 0.005).unwrap();
        let regional = generator
            .profile_difference_bound_for_region_m(cap, fine, coarse)
            .unwrap();
        assert_eq!(regional, 0.0);
        omitted |= regional == 0.0;
        let tangent = axis
            .cross(if axis.y.abs() < 0.9 {
                DVec3::Y
            } else {
                DVec3::X
            })
            .normalize();
        for i in -64..=64 {
            let angle = cap.half_angle_rad() * f64::from(i) / 64.0;
            let location = SurfaceLocation::new(
                Direction3::try_new(axis * angle.cos() + tangent * angle.sin()).unwrap(),
            );
            let sample = |footprint| {
                generator
                    .evaluate_point(TerrainQuery {
                        location,
                        footprint,
                    })
                    .unwrap()
            };
            assert_eq!(sample(fine), sample(coarse));
        }
    }
    assert!(
        omitted,
        "distant compact features must not force refinement everywhere"
    );
}

#[test]
fn tiny_cap_interval_encloses_filtered_geometry_and_complete_height() {
    let generator = TerrainGenerator::new(
        &definition(17, TerrainGeneratorVersion::CrateredV1),
        100_000.0,
    )
    .unwrap();
    let axis = generator.crater_features()[0].center();
    let cap = DirectionalCap::new(axis, 1e-6).unwrap();
    for footprint_m in [0.0, 10.0, 100.0, 1000.0] {
        let footprint = TerrainFootprint::new(footprint_m).unwrap();
        let [lo, hi] = generator
            .bounds_for_region(cap, footprint)
            .unwrap()
            .height_interval_m();
        for query_footprint in [TerrainFootprint::COMPLETE, footprint] {
            let height = generator
                .evaluate_point(TerrainQuery {
                    location: SurfaceLocation::new(axis),
                    footprint: query_footprint,
                })
                .unwrap()
                .height_m();
            assert!(lo <= height && height <= hi);
        }
    }
}

#[test]
fn persistent_landmarks_do_not_disable_background_noise_filtering() {
    let original = definition(17, TerrainGeneratorVersion::CrateredV1);
    let flat = TerrainBandConfig::new(
        0.0,
        TerrainScale::Metres {
            longest_wavelength_m: 256.0,
        },
        1,
    )
    .unwrap();
    let mut bands = [flat; 5];
    bands[4] = TerrainBandConfig::new(
        3.0,
        TerrainScale::Metres {
            longest_wavelength_m: 256.0,
        },
        2,
    )
    .unwrap();
    let config = TerrainConfig::new(bands, original.config().controls())
        .unwrap()
        .with_crater_field(original.config().crater_field().unwrap())
        .unwrap();
    let mixed = TerrainDefinition::new(
        original.identity(),
        original.seed(),
        original.version(),
        config,
    );
    let generator = TerrainGenerator::new(&mixed, 100_000.0).unwrap();
    let coarse = TerrainFootprint::new(100_000.0).unwrap();
    let cap = DirectionalCap::new(generator.crater_features()[0].center(), 0.01).unwrap();
    let complete = generator
        .bounds_for_region(cap, TerrainFootprint::COMPLETE)
        .unwrap();
    let filtered = generator.bounds_for_region(cap, coarse).unwrap();
    assert_eq!(complete.unresolved_height_bound_m(), 0.0);
    assert!(filtered.unresolved_height_bound_m() > 0.0);
    assert!(
        generator
            .profile_difference_bound_m(TerrainFootprint::COMPLETE, coarse)
            .unwrap()
            > 0.0
    );
    assert!(filtered.represented_height_bound_m() > generator.crater_features()[0].depth_m());
}
