use glam::DVec3;
use mundaris_math::{
    Direction3,
    surface::{CubeFace, SurfaceLocation},
};
use mundaris_world::terrain::{
    GeologicalControls, GeologicalParameters, SurfaceAlgorithm, SurfaceAtmosphere,
    SurfaceDefinition, SurfaceGenerator, SurfaceMaterialDefinition, SurfaceTerrainDefinition,
    TerrainIdentity, TerrainSeed,
};

const ALGORITHMS: [SurfaceAlgorithm; 3] = [
    SurfaceAlgorithm::RockyV4,
    SurfaceAlgorithm::IcyV2,
    SurfaceAlgorithm::VolcanicV2,
];

fn location(direction: DVec3) -> SurfaceLocation {
    SurfaceLocation::new(Direction3::try_new(direction).unwrap())
}

fn directions(count: usize) -> Vec<SurfaceLocation> {
    // Deterministic equal-area-ish spherical coverage, independent of cube charts.
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    (0..count)
        .map(|index| {
            let y = 1.0 - 2.0 * (index as f64 + 0.5) / count as f64;
            let radial = (1.0 - y * y).sqrt();
            let angle = golden_angle * index as f64;
            location(DVec3::new(radial * angle.cos(), y, radial * angle.sin()))
        })
        .collect()
}

fn generator(algorithm: SurfaceAlgorithm, seed: u64, radius_m: f64) -> SurfaceGenerator {
    let definition =
        SurfaceDefinition::generated(TerrainIdentity(0x51_1b_01), TerrainSeed(seed), algorithm);
    SurfaceGenerator::new(&definition, radius_m).unwrap()
}

fn control_values(controls: GeologicalControls) -> [f64; 17] {
    [
        controls.age,
        controls.activity,
        controls.resurfacing,
        controls.impact_retention,
        controls.relief_potential,
        controls.province_weights[0],
        controls.province_weights[1],
        controls.province_weights[2],
        controls.province_weights[3],
        controls.process_strengths[0],
        controls.process_strengths[1],
        controls.process_strengths[2],
        controls.process_strengths[3],
        controls.structural_direction.x,
        controls.structural_direction.y,
        controls.structural_direction.z,
        controls.structural_direction.length(),
    ]
}

fn assert_control_valid(algorithm: SurfaceAlgorithm, controls: GeologicalControls) {
    let values = control_values(controls);
    assert!(
        values.iter().all(|value| value.is_finite()),
        "{algorithm:?}: {controls:?}"
    );
    for value in [
        controls.age,
        controls.activity,
        controls.resurfacing,
        controls.impact_retention,
        controls.relief_potential,
    ]
    .into_iter()
    .chain(controls.province_weights)
    .chain(controls.process_strengths)
    {
        assert!((0.0..=1.0).contains(&value), "{algorithm:?}: {controls:?}");
    }
    assert!((controls.province_weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    assert!(controls.structural_direction.length() > 0.999999);
    assert!(controls.structural_direction.length() < 1.000001);
}

#[test]
fn director_fields_repeat_across_query_order_threads_and_cube_charts() {
    let points = directions(96);
    for algorithm in ALGORITHMS {
        let g = generator(algorithm, 0x9123_804a, 109_000.0);
        let expected: Vec<_> = points
            .iter()
            .map(|point| g.geological_controls(*point).unwrap().unwrap())
            .collect();

        for (point, controls) in points.iter().zip(&expected) {
            assert_eq!(Some(*controls), g.geological_controls(*point).unwrap());
            assert_control_valid(algorithm, *controls);
        }

        for (point, controls) in points.iter().rev().zip(expected.iter().rev()) {
            assert_eq!(Some(*controls), g.geological_controls(*point).unwrap());
        }

        let parallel = std::thread::scope(|scope| {
            let points = points.as_slice();
            let handles: Vec<_> = (0..4)
                .map(|lane| {
                    let generator = &g;
                    scope.spawn(move || {
                        (lane..points.len())
                            .step_by(4)
                            .map(|index| {
                                generator
                                    .geological_controls(points[index])
                                    .unwrap()
                                    .unwrap()
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        for lane in 0..4 {
            for (offset, controls) in parallel[lane].iter().enumerate() {
                assert_eq!(*controls, expected[lane + offset * 4]);
            }
        }

        // Cube-face edge and corner queries canonicalize to the same body direction.
        let mut seen = Vec::new();
        for face in CubeFace::ALL {
            for uv in [
                [-1.0, -1.0],
                [-1.0, 0.0],
                [-1.0, 1.0],
                [0.0, -1.0],
                [0.0, 1.0],
                [1.0, -1.0],
                [1.0, 0.0],
                [1.0, 1.0],
            ] {
                let point = SurfaceLocation::new(face.direction(uv).unwrap());
                let controls = g.geological_controls(point).unwrap().unwrap();
                let sample = g.evaluate_point(point).unwrap();
                if let Some((_, earlier_controls, earlier_sample)) = seen
                    .iter()
                    .find(|(known, _, _)| *known == point.direction())
                {
                    assert_eq!(*earlier_controls, controls, "{algorithm:?} at {point:?}");
                    assert_eq!(*earlier_sample, sample, "{algorithm:?} at {point:?}");
                } else {
                    seen.push((point.direction(), controls, sample));
                }
            }
        }
    }
}

#[test]
fn province_directors_cover_each_class_with_normalized_smooth_weights() {
    let points = directions(768);
    for algorithm in ALGORITHMS {
        let names = algorithm.province_names();
        assert_eq!(names.len(), 4);
        let mut maxima = [0.0_f64; 4];
        let mut dominant_counts = [0_usize; 4];
        let mut total_weight = [0.0_f64; 4];
        let mut weight_sq = [0.0_f64; 4];
        let sample_count = points.len() * 4;
        for seed in [7, 19, 31, 0x8a2c_5d91] {
            let g = generator(algorithm, seed, 109_000.0);
            for point in &points {
                let controls = g.geological_controls(*point).unwrap().unwrap();
                assert_control_valid(algorithm, controls);
                let dominant = controls
                    .province_weights
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .unwrap()
                    .0;
                dominant_counts[dominant] += 1;
                for index in 0..4 {
                    maxima[index] = maxima[index].max(controls.province_weights[index]);
                    total_weight[index] += controls.province_weights[index];
                    weight_sq[index] += controls.province_weights[index].powi(2);
                }
            }
        }
        for index in 0..4 {
            assert!(
                maxima[index] > 0.40,
                "{algorithm:?} province {} never develops a clear region",
                names[index]
            );
            assert!(
                dominant_counts[index] > 0,
                "{algorithm:?} province {} is never dominant",
                names[index]
            );
            let mean = total_weight[index] / sample_count as f64;
            let variance = weight_sq[index] / sample_count as f64 - mean * mean;
            assert!(
                variance > 1e-4,
                "{algorithm:?} province {} has no spatial variation",
                names[index]
            );
        }
    }
}

#[test]
fn seed_changes_geological_history_and_not_only_feature_placement() {
    let points = directions(192);
    for algorithm in ALGORITHMS {
        let first = generator(algorithm, 0x1234_5678, 109_000.0);
        let second = generator(algorithm, 0x8765_4321, 109_000.0);
        let a: Vec<_> = points
            .iter()
            .map(|point| first.geological_controls(*point).unwrap().unwrap())
            .collect();
        let b: Vec<_> = points
            .iter()
            .map(|point| second.geological_controls(*point).unwrap().unwrap())
            .collect();
        assert_ne!(a, b, "{algorithm:?} controls did not vary with seed");

        let mean = |samples: &[GeologicalControls], field: fn(&GeologicalControls) -> f64| {
            samples.iter().map(&field).sum::<f64>() / samples.len() as f64
        };
        let history_a = [
            mean(&a, |c| c.age),
            mean(&a, |c| c.activity),
            mean(&a, |c| c.resurfacing),
            mean(&a, |c| c.impact_retention),
            mean(&a, |c| c.relief_potential),
            mean(&a, |c| c.process_strengths[0]),
            mean(&a, |c| c.process_strengths[1]),
            mean(&a, |c| c.process_strengths[2]),
            mean(&a, |c| c.process_strengths[3]),
        ];
        let history_b = [
            mean(&b, |c| c.age),
            mean(&b, |c| c.activity),
            mean(&b, |c| c.resurfacing),
            mean(&b, |c| c.impact_retention),
            mean(&b, |c| c.relief_potential),
            mean(&b, |c| c.process_strengths[0]),
            mean(&b, |c| c.process_strengths[1]),
            mean(&b, |c| c.process_strengths[2]),
            mean(&b, |c| c.process_strengths[3]),
        ];
        assert_ne!(
            history_a, history_b,
            "{algorithm:?} changed only regional placement"
        );
    }
}

#[test]
fn geometry_and_directors_stay_finite_bounded_and_independent_of_material_and_atmosphere() {
    let points = directions(48);
    for algorithm in ALGORITHMS {
        for seed in [7, 31] {
            for radius_m in [5_000.0, 109_000.0, 1_200_000.0] {
                let definition = SurfaceDefinition::generated(
                    TerrainIdentity(0x51_1b_01),
                    TerrainSeed(seed),
                    algorithm,
                );
                let g = SurfaceGenerator::new(&definition, radius_m).unwrap();
                let [minimum_radius, maximum_radius] = g.conservative_radius_envelope_m();
                for point in &points {
                    let controls = g.geological_controls(*point).unwrap().unwrap();
                    assert_control_valid(algorithm, controls);
                    let sample = g.evaluate_point(*point).unwrap();
                    assert!(sample.radius_m().is_finite());
                    assert!((minimum_radius..=maximum_radius).contains(&sample.radius_m()));
                    assert!(sample.normal().is_finite());
                    assert!((sample.normal().length() - 1.0).abs() < 1e-12);
                    assert!(sample.terrain().height_m().is_finite());
                    assert!(
                        sample
                            .terrain()
                            .tangent_gradient_m_per_unit_direction()
                            .is_finite()
                    );
                }

                let changed_material = SurfaceDefinition::new(
                    definition.identity(),
                    definition.seed(),
                    *definition.shape(),
                    definition.terrain(),
                    SurfaceMaterialDefinition::new(
                        definition.material().version(),
                        (definition.material().composition() + 0.37).fract(),
                        (definition.material().regional_contrast() + 0.41).fract(),
                    )
                    .unwrap(),
                    definition.atmosphere(),
                )
                .unwrap();
                let material_generator =
                    SurfaceGenerator::new(&changed_material, radius_m).unwrap();
                for point in &points {
                    assert_eq!(
                        g.geological_controls(*point).unwrap(),
                        material_generator.geological_controls(*point).unwrap()
                    );
                    let a = g.evaluate_point(*point).unwrap();
                    let b = material_generator.evaluate_point(*point).unwrap();
                    assert_eq!(a.shape(), b.shape());
                    assert_eq!(a.terrain(), b.terrain());
                    assert_eq!(a.normal(), b.normal());
                }

                let atmosphere_changed = SurfaceDefinition::new(
                    definition.identity(),
                    definition.seed(),
                    *definition.shape(),
                    definition.terrain(),
                    definition.material(),
                    SurfaceAtmosphere::Descriptor {
                        pressure_pa: 12_000.0,
                        scale_height_m: 900.0,
                    },
                )
                .unwrap();
                let atmosphere_generator =
                    SurfaceGenerator::new(&atmosphere_changed, radius_m).unwrap();
                for point in &points {
                    assert_eq!(
                        g.geological_controls(*point).unwrap(),
                        atmosphere_generator.geological_controls(*point).unwrap()
                    );
                    assert_eq!(
                        g.evaluate_point(*point).unwrap(),
                        atmosphere_generator.evaluate_point(*point).unwrap()
                    );
                }
            }
        }
    }
}

#[test]
fn director_and_height_gradients_match_independent_local_differences() {
    let control_step = 2e-7;
    let radius_m = 109_000.0;
    // A sub-millimetre perturbation resolves the finest RockyV4 primitive.
    let height_step = 2e-4 / radius_m;
    for algorithm in ALGORITHMS {
        let g = generator(algorithm, 0xcafe_babe, radius_m);
        for point in directions(24) {
            let n = point.direction().unit();
            let helper = if n.x.abs() < 0.8 { DVec3::X } else { DVec3::Y };
            let tangent = n.cross(helper).normalize();
            let center = g.geological_controls(point).unwrap().unwrap();
            let plus = g
                .geological_controls(location(n + tangent * control_step))
                .unwrap()
                .unwrap();
            let minus = g
                .geological_controls(location(n - tangent * control_step))
                .unwrap()
                .unwrap();
            assert_control_valid(algorithm, center);
            let plus_values = control_values(plus);
            let minus_values = control_values(minus);
            for index in 0..16 {
                assert!(
                    (plus_values[index] - minus_values[index]).abs() < 0.1,
                    "{algorithm:?} director has a discontinuity in component {index}"
                );
            }

            let sample = g.evaluate_point(point).unwrap();
            let analytic = sample
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .dot(tangent);
            let height = |direction| {
                g.evaluate_point(location(direction))
                    .unwrap()
                    .terrain()
                    .height_m()
            };
            let numeric = (height(n + tangent * height_step) - height(n - tangent * height_step))
                / (2.0 * height_step);
            let tolerance = 0.01 + analytic.abs() * 0.002;
            assert!(
                (numeric - analytic).abs() < tolerance,
                "{algorithm:?} at {n:?} along {tangent:?}: {numeric} vs {analytic}"
            );
        }
    }
}

#[test]
fn province_diagnostic_locations_have_smooth_local_geometry() {
    for algorithm in ALGORITHMS {
        let g = generator(algorithm, 0x7654_3210, 109_000.0);
        let near = location(DVec3::new(0.23, 0.91, -0.34));
        let probes = g.diagnostic_province_probes(near).unwrap();
        assert_eq!(probes.len(), 4, "{algorithm:?} missing a province target");
        let boundary_probes = g.diagnostic_boundary_probes(near).unwrap();
        assert!(
            !boundary_probes.is_empty(),
            "{algorithm:?} missing director diagnostics"
        );
        assert!(
            boundary_probes
                .iter()
                .any(|probe| probe.label == "province-transition"),
            "{algorithm:?} missing province transition probes"
        );
        assert!(
            boundary_probes
                .iter()
                .any(|probe| probe.label == "province-feature-support"),
            "{algorithm:?} missing feature support probes"
        );
        let step = 1e-8;
        for probe in probes.into_iter().chain(boundary_probes) {
            let direction = probe.location.direction().unit();
            let helper = if direction.x.abs() < 0.8 {
                DVec3::X
            } else {
                DVec3::Y
            };
            let tangent = direction.cross(helper).normalize();
            let repeated = g.evaluate_point(probe.location).unwrap();
            assert_eq!(repeated, g.evaluate_point(probe.location).unwrap());
            let controls = g.geological_controls(probe.location).unwrap().unwrap();
            assert_control_valid(algorithm, controls);
            let plus_location = location(direction + tangent * step);
            let minus_location = location(direction - tangent * step);
            let plus = g.evaluate_point(plus_location).unwrap();
            let minus = g.evaluate_point(minus_location).unwrap();
            let numeric = (plus.terrain().height_m() - minus.terrain().height_m()) / (2.0 * step);
            let analytic = repeated
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .dot(tangent);
            assert!(numeric.is_finite() && analytic.is_finite());
            assert!(
                (numeric - analytic).abs() < 0.02 + analytic.abs() * 0.003,
                "{algorithm:?} {}: {numeric} vs {analytic}",
                probe.label
            );
            assert!((plus.terrain().height_m() - minus.terrain().height_m()).abs() < 1.0);
        }
    }
}

#[test]
fn process_controls_respond_to_activity_and_resurfacing_parameter_edits() {
    let points = directions(256);
    for algorithm in ALGORITHMS {
        let seed = TerrainSeed(0x6611_92aa);
        let base = GeologicalParameters::generated(algorithm, seed);
        let base_definition =
            SurfaceDefinition::generated(TerrainIdentity(0x51_1b_01), seed, algorithm);
        let make = |parameters| {
            let definition = SurfaceDefinition::new(
                TerrainIdentity(0x51_1b_01),
                seed,
                mundaris_world::terrain::ShapeDefinition::sphere(),
                SurfaceTerrainDefinition::new(algorithm, parameters).unwrap(),
                base_definition.material(),
                base_definition.atmosphere(),
            )
            .unwrap();
            SurfaceGenerator::new(&definition, 109_000.0).unwrap()
        };
        let low_activity = make(GeologicalParameters {
            activity: 0.05,
            ..base
        });
        let high_activity = make(GeologicalParameters {
            activity: 0.95,
            ..base
        });
        let low_resurfacing = make(GeologicalParameters {
            resurfacing_fraction: 0.05,
            ..base
        });
        let high_resurfacing = make(GeologicalParameters {
            resurfacing_fraction: 0.95,
            ..base
        });
        let mean_processes = |g: &SurfaceGenerator| {
            let mut means = [0.0; 4];
            for point in &points {
                let controls = g.geological_controls(*point).unwrap().unwrap();
                for (mean, strength) in means.iter_mut().zip(controls.process_strengths) {
                    *mean += strength / points.len() as f64;
                }
            }
            means
        };
        let low_activity_mean = mean_processes(&low_activity);
        let high_activity_mean = mean_processes(&high_activity);
        assert!(
            high_activity_mean[1] > low_activity_mean[1],
            "{algorithm:?} activity did not strengthen its active geological process"
        );
        let low_resurfacing_mean = mean_processes(&low_resurfacing);
        let high_resurfacing_mean = mean_processes(&high_resurfacing);
        assert!(
            high_resurfacing_mean[2] > low_resurfacing_mean[2],
            "{algorithm:?} resurfacing did not strengthen resurfacing"
        );
        let mean_retention = |g: &SurfaceGenerator| {
            points
                .iter()
                .map(|point| {
                    g.geological_controls(*point)
                        .unwrap()
                        .unwrap()
                        .impact_retention
                })
                .sum::<f64>()
                / points.len() as f64
        };
        assert!(
            mean_retention(&high_resurfacing) < mean_retention(&low_resurfacing),
            "{algorithm:?} resurfacing did not reduce retained old impact relief"
        );
        let low_heights: Vec<_> = points
            .iter()
            .map(|point| {
                low_resurfacing
                    .evaluate_point(*point)
                    .unwrap()
                    .terrain()
                    .height_m()
            })
            .collect();
        let high_heights: Vec<_> = points
            .iter()
            .map(|point| {
                high_resurfacing
                    .evaluate_point(*point)
                    .unwrap()
                    .terrain()
                    .height_m()
            })
            .collect();
        let low_bound = low_resurfacing.conservative_absolute_height_bound_m();
        let high_bound = high_resurfacing.conservative_absolute_height_bound_m();
        assert!(
            low_heights.iter().zip(&high_heights).any(|(a, b)| a != b),
            "{algorithm:?} resurfacing did not alter the height field"
        );
        assert!(
            low_activity
                .evaluate_point(points[0])
                .unwrap()
                .terrain()
                .height_m()
                != high_activity
                    .evaluate_point(points[0])
                    .unwrap()
                    .terrain()
                    .height_m(),
            "{algorithm:?} activity did not alter the height field"
        );
        assert!(low_heights.iter().all(|height| height.abs() <= low_bound));
        assert!(high_heights.iter().all(|height| height.abs() <= high_bound));
    }
}
