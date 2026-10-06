use glam::DVec3;
use mundaris_math::{
    Direction3,
    surface::{CubeFace, SurfaceLocation},
};
use mundaris_world::terrain::*;

const ALGORITHMS: [SurfaceAlgorithm; 3] = [
    SurfaceAlgorithm::RockyV5,
    SurfaceAlgorithm::IcyV3,
    SurfaceAlgorithm::VolcanicV3,
];

fn location(direction: DVec3) -> SurfaceLocation {
    SurfaceLocation::new(Direction3::try_new(direction).unwrap())
}

fn directions(count: usize) -> Vec<SurfaceLocation> {
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    (0..count)
        .map(|index| {
            let y = 1.0 - 2.0 * (index as f64 + 0.5) / count as f64;
            let radial = (1.0 - y * y).sqrt();
            let angle = index as f64 * golden_angle;
            location(DVec3::new(radial * angle.cos(), y, radial * angle.sin()))
        })
        .collect()
}

fn definition(algorithm: SurfaceAlgorithm, seed: u64) -> SurfaceDefinition {
    SurfaceDefinition::generated(TerrainIdentity(0x51_1b_02), TerrainSeed(seed), algorithm)
}

fn generator(algorithm: SurfaceAlgorithm, seed: u64, radius_m: f64) -> SurfaceGenerator {
    SurfaceGenerator::new(&definition(algorithm, seed), radius_m).unwrap()
}

fn hierarchy_sum(details: SurfaceDetailDiagnostics) -> f64 {
    details.inherited_height_m
        + details.regional_height_m
        + details.local_height_m
        + details.fine_height_m
}

fn hierarchy_gradient(details: SurfaceDetailDiagnostics) -> DVec3 {
    details.inherited_gradient_m
        + details.regional_gradient_m
        + details.local_gradient_m
        + details.fine_gradient_m
}

#[test]
fn complete_hierarchical_queries_repeat_across_batch_order_and_threads() {
    let points = directions(36);
    for algorithm in ALGORITHMS {
        let generator = generator(algorithm, 0x8a2c_5d91, 109_000.0);
        let expected: Vec<_> = points
            .iter()
            .map(|&point| {
                (
                    generator.evaluate_point(point).unwrap(),
                    generator.detail_diagnostics(point).unwrap().unwrap(),
                )
            })
            .collect();

        for (&point, &(sample, diagnostics)) in points.iter().zip(&expected) {
            assert_eq!(generator.evaluate_point(point).unwrap(), sample);
            assert_eq!(
                generator.detail_diagnostics(point).unwrap(),
                Some(diagnostics)
            );
        }

        let reversed_points: Vec<_> = points.iter().rev().copied().collect();
        let mut reversed_samples = vec![expected[0].0; reversed_points.len()];
        generator
            .evaluate_batch(&reversed_points, &mut reversed_samples)
            .unwrap();
        for ((&point, sample), &(expected_sample, _)) in reversed_points
            .iter()
            .zip(&reversed_samples)
            .zip(expected.iter().rev())
        {
            assert_eq!(*sample, expected_sample);
            assert_eq!(*sample, generator.evaluate_point(point).unwrap());
        }

        let parallel = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|lane| {
                    let generator = &generator;
                    let points = points.as_slice();
                    scope.spawn(move || {
                        (lane..points.len())
                            .step_by(4)
                            .map(|index| {
                                let point = points[index];
                                (
                                    generator.evaluate_point(point).unwrap(),
                                    generator.detail_diagnostics(point).unwrap().unwrap(),
                                )
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
            for (offset, (sample, diagnostics)) in parallel[lane].iter().enumerate() {
                assert_eq!(*sample, expected[lane + offset * 4].0);
                assert_eq!(*diagnostics, expected[lane + offset * 4].1);
            }
        }
    }
}

#[test]
fn hierarchical_samples_are_identical_on_canonical_cube_edges_and_corners() {
    for algorithm in ALGORITHMS {
        let generator = generator(algorithm, 0x7654_3210, 109_000.0);
        let mut seen = Vec::new();
        for face in CubeFace::ALL {
            for t in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                for uv in [[-1.0, t], [1.0, t], [t, -1.0], [t, 1.0]] {
                    let point = SurfaceLocation::new(face.direction(uv).unwrap());
                    let sample = generator.evaluate_point(point).unwrap();
                    let diagnostics = generator.detail_diagnostics(point).unwrap().unwrap();
                    if let Some((_, prior_sample, prior_diagnostics)) = seen
                        .iter()
                        .find(|(known_direction, _, _)| *known_direction == point.direction())
                    {
                        assert_eq!(*prior_sample, sample, "{algorithm:?} at {point:?}");
                        assert_eq!(
                            *prior_diagnostics, diagnostics,
                            "{algorithm:?} at {point:?}"
                        );
                    } else {
                        seen.push((point.direction(), sample, diagnostics));
                    }
                }
            }
        }
    }
}

#[test]
fn detail_diagnostics_recompose_the_authoritative_height_and_gradient() {
    let points = directions(48);
    for algorithm in ALGORITHMS {
        let generator = generator(algorithm, 0xcafe_babe, 109_000.0);
        for point in &points {
            let sample = generator.evaluate_point(*point).unwrap();
            let details = generator.detail_diagnostics(*point).unwrap().unwrap();
            let actual_gradient = sample.terrain().tangent_gradient_m_per_unit_direction();
            assert!(
                (hierarchy_sum(details) - sample.terrain().height_m()).abs() < 1.0e-10,
                "{algorithm:?} at {point:?}: components do not recompose height"
            );
            assert!(
                (hierarchy_gradient(details) - actual_gradient).length() < 1.0e-7,
                "{algorithm:?} at {point:?}: components do not recompose gradient"
            );
            assert!(details.band_work.iter().all(|work| work.cells_visited > 0));
            assert!(
                details
                    .parent_process_strengths
                    .iter()
                    .all(|strength| strength.is_finite() && (0.0..=1.0).contains(strength))
            );
        }
    }
}

#[test]
fn complete_geometry_and_materials_are_independent_of_material_and_atmosphere() {
    let points = directions(32);
    for algorithm in ALGORITHMS {
        let original = definition(algorithm, 0x1122_3344);
        let material_changed = SurfaceDefinition::new(
            original.identity(),
            original.seed(),
            *original.shape(),
            original.terrain(),
            SurfaceMaterialDefinition::new(original.material().version(), 0.91, 0.87).unwrap(),
            original.atmosphere(),
        )
        .unwrap();
        let atmosphere_changed = SurfaceDefinition::new(
            original.identity(),
            original.seed(),
            *original.shape(),
            original.terrain(),
            original.material(),
            SurfaceAtmosphere::Descriptor {
                pressure_pa: 12_000.0,
                scale_height_m: 900.0,
            },
        )
        .unwrap();
        let base = SurfaceGenerator::new(&original, 109_000.0).unwrap();
        let changed_material = SurfaceGenerator::new(&material_changed, 109_000.0).unwrap();
        let changed_atmosphere = SurfaceGenerator::new(&atmosphere_changed, 109_000.0).unwrap();
        for point in &points {
            let sample = base.evaluate_point(*point).unwrap();
            let material_sample = changed_material.evaluate_point(*point).unwrap();
            let atmosphere_sample = changed_atmosphere.evaluate_point(*point).unwrap();
            assert_eq!(sample.shape(), material_sample.shape());
            assert_eq!(sample.terrain(), material_sample.terrain());
            assert_eq!(sample.normal(), material_sample.normal());
            assert_eq!(sample, atmosphere_sample);
            assert_eq!(
                base.detail_diagnostics(*point).unwrap(),
                changed_material.detail_diagnostics(*point).unwrap()
            );
            assert_eq!(
                base.detail_diagnostics(*point).unwrap(),
                changed_atmosphere.detail_diagnostics(*point).unwrap()
            );
        }
    }
}

#[test]
fn zero_relief_disables_added_bands_while_envelopes_remain_positive() {
    for algorithm in ALGORITHMS {
        for seed in [7, 31] {
            for radius_m in [5_000.0, 109_000.0, 1_200_000.0] {
                let generator = generator(algorithm, seed, radius_m);
                let [minimum_radius, maximum_radius] = generator.conservative_radius_envelope_m();
                assert!(minimum_radius > 0.0 && maximum_radius >= minimum_radius);
                for point in directions(24) {
                    let sample = generator.evaluate_point(point).unwrap();
                    assert!(sample.radius_m().is_finite() && sample.radius_m() > 0.0);
                    assert!((minimum_radius..=maximum_radius).contains(&sample.radius_m()));
                    assert!(sample.normal().is_finite());
                    assert!((sample.normal().length() - 1.0).abs() < 1.0e-12);
                    assert!(
                        sample
                            .terrain()
                            .tangent_gradient_m_per_unit_direction()
                            .is_finite()
                    );
                }
            }

            let base = definition(algorithm, seed);
            let mut flat_parameters = base.terrain().parameters();
            flat_parameters.relief_fraction = 0.0;
            let flat_definition = SurfaceDefinition::new(
                base.identity(),
                base.seed(),
                ShapeDefinition::sphere(),
                SurfaceTerrainDefinition::new(algorithm, flat_parameters).unwrap(),
                base.material(),
                base.atmosphere(),
            )
            .unwrap();
            let flat = SurfaceGenerator::new(&flat_definition, 109_000.0).unwrap();
            for point in directions(24) {
                let sample = flat.evaluate_point(point).unwrap();
                let details = flat.detail_diagnostics(point).unwrap().unwrap();
                // RockyV5 preserves RockyV4's fixed-amplitude regolith grain
                // (REGOLITH_RELIEF_BOUND_M in the inherited MoonLikeV2 oracle).
                // Zero adjustable relief must disable only the successor bands.
                assert_eq!(details.regional_height_m, 0.0, "{algorithm:?}");
                assert_eq!(details.local_height_m, 0.0, "{algorithm:?}");
                assert_eq!(details.fine_height_m, 0.0, "{algorithm:?}");
                assert_eq!(details.regional_gradient_m, DVec3::ZERO, "{algorithm:?}");
                assert_eq!(details.local_gradient_m, DVec3::ZERO, "{algorithm:?}");
                assert_eq!(details.fine_gradient_m, DVec3::ZERO, "{algorithm:?}");
                assert_eq!(sample.terrain().height_m(), details.inherited_height_m);
                assert!(
                    (sample.terrain().tangent_gradient_m_per_unit_direction()
                        - details.inherited_gradient_m)
                        .length()
                        < 1.0e-7
                );
                assert!(sample.radius_m() > 0.0);
            }
        }
    }
}

#[test]
fn hierarchical_gradients_match_small_physical_tangent_differences() {
    let radius_m = 109_000.0;
    let physical_step_m = 2.0e-4;
    for algorithm in ALGORITHMS {
        let generator = generator(algorithm, 0xcafe_babe, radius_m);
        for point in directions(16) {
            let n = point.direction().unit();
            let helper = if n.x.abs() < 0.8 { DVec3::X } else { DVec3::Y };
            let tangent = n.cross(helper).normalize();
            let sample = generator.evaluate_point(point).unwrap();
            let analytic = sample
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .dot(tangent);
            let step = physical_step_m / radius_m;
            let plus = generator
                .evaluate_point(location(n + tangent * step))
                .unwrap()
                .terrain()
                .height_m();
            let minus = generator
                .evaluate_point(location(n - tangent * step))
                .unwrap()
                .terrain()
                .height_m();
            let numeric = (plus - minus) / (2.0 * step);
            assert!(numeric.is_finite() && analytic.is_finite());
            assert!(
                (numeric - analytic).abs() < 0.08 + analytic.abs() * 0.004,
                "{algorithm:?} at {n:?}: {numeric} vs {analytic}"
            );
        }
    }
}

#[test]
fn fine_detail_support_probes_remain_continuous_and_have_finite_derivatives() {
    let radius_m = 109_000.0;
    for algorithm in ALGORITHMS {
        let generator = generator(algorithm, 0x7654_3210, radius_m);
        let probes = generator
            .diagnostic_boundary_probes(location(DVec3::new(0.23, 0.91, -0.34)))
            .unwrap()
            .into_iter()
            .filter(|probe| probe.label == "hierarchical-detail-support")
            .collect::<Vec<_>>();
        assert!(
            !probes.is_empty(),
            "{algorithm:?} has no detail support probes"
        );
        for probe in probes {
            let center = generator.evaluate_point(probe.location).unwrap();
            let n = probe.location.direction().unit();
            let helper = if n.x.abs() < 0.8 { DVec3::X } else { DVec3::Y };
            let tangent = n.cross(helper).normalize();
            let step = 2.0e-4 / radius_m;
            let plus = generator
                .evaluate_point(location(n + tangent * step))
                .unwrap();
            let minus = generator
                .evaluate_point(location(n - tangent * step))
                .unwrap();
            let numeric = (plus.terrain().height_m() - minus.terrain().height_m()) / (2.0 * step);
            let analytic = center
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .dot(tangent);
            assert!(numeric.is_finite() && analytic.is_finite());
            assert!(
                (numeric - analytic).abs() < 0.1 + analytic.abs() * 0.01,
                "{algorithm:?} {}: {numeric} vs {analytic}",
                probe.label
            );
            assert!(
                (plus.terrain().height_m() - minus.terrain().height_m()).abs() < 0.1,
                "{algorithm:?} has a height jump at a compact detail support"
            );
        }
    }
}

#[test]
fn each_family_adds_local_structure_and_resurfacing_reduces_old_detail() {
    let points = directions(192);
    let mut family_band_energies = Vec::new();
    for algorithm in ALGORITHMS {
        let base_definition = definition(algorithm, 0x5eed_cafe);
        let base = SurfaceGenerator::new(&base_definition, 109_000.0).unwrap();
        let mut young_parameters = base_definition.terrain().parameters();
        young_parameters.resurfacing_fraction = 0.95;
        let young_definition = SurfaceDefinition::new(
            base_definition.identity(),
            base_definition.seed(),
            *base_definition.shape(),
            SurfaceTerrainDefinition::new(algorithm, young_parameters).unwrap(),
            base_definition.material(),
            base_definition.atmosphere(),
        )
        .unwrap();
        let young = SurfaceGenerator::new(&young_definition, 109_000.0).unwrap();
        let mut band_energy = [0.0; 3];
        let mut old_energy = 0.0;
        let mut young_energy = 0.0;
        let mut old_impact_strength = 0.0;
        let mut young_impact_strength = 0.0;
        for point in &points {
            let detail = base.detail_diagnostics(*point).unwrap().unwrap();
            let reworked = young.detail_diagnostics(*point).unwrap().unwrap();
            band_energy[0] += detail.regional_height_m.abs();
            band_energy[1] += detail.local_height_m.abs();
            band_energy[2] += detail.fine_height_m.abs();
            old_energy += detail.local_height_m.abs() + detail.fine_height_m.abs();
            young_energy += reworked.local_height_m.abs() + reworked.fine_height_m.abs();
            old_impact_strength += detail.parent_process_strengths[0];
            young_impact_strength += reworked.parent_process_strengths[0];
        }
        assert!(
            band_energy.iter().all(|energy| *energy > 0.0),
            "{algorithm:?} has a hierarchy band with no contribution: {band_energy:?}"
        );
        family_band_energies.push(band_energy);
        match algorithm {
            SurfaceAlgorithm::RockyV5 | SurfaceAlgorithm::IcyV3 => {
                assert!(old_energy > 0.0, "{algorithm:?} has no local/fine detail");
                assert!(
                    young_energy < old_energy,
                    "{algorithm:?} resurfacing did not suppress local/fine old detail: {young_energy} >= {old_energy}"
                );
            }
            SurfaceAlgorithm::VolcanicV3 => {
                assert!(
                    young_impact_strength < old_impact_strength,
                    "VolcanicV3 resurfacing did not reduce retained old impacts"
                );
            }
            _ => unreachable!(),
        }
    }
    assert_ne!(family_band_energies[0], family_band_energies[1]);
    assert_ne!(family_band_energies[0], family_band_energies[2]);
    assert_ne!(family_band_energies[1], family_band_energies[2]);
}
