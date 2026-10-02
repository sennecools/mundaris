use glam::DVec3;
use mundaris_math::{
    AngularVelocity3, Direction3, LinearVelocity3, LocalPosition, UnitRotation,
    surface::{CubeFace, CubePatchAddress, PatchEdge, SurfaceLocation},
};
use mundaris_world::{BodyProperties, BodyState, CelestialSystem, SimulationInstant, terrain::*};
use std::num::NonZeroU64;

fn definition(seed: u64, amplitude: f64) -> TerrainDefinition {
    let band = TerrainBandConfig::new(
        amplitude,
        TerrainScale::Metres {
            longest_wavelength_m: 128.0,
        },
        4,
    )
    .unwrap();
    let controls = TerrainControls::new(0.0, 1.0, 0.5, 1.0, 0.1, 0.1).unwrap();
    TerrainDefinition::new(
        TerrainIdentity(42),
        TerrainSeed(seed),
        TerrainGeneratorVersion::V1,
        TerrainConfig::new([band; 5], controls).unwrap(),
    )
}
fn location(v: DVec3) -> SurfaceLocation {
    SurfaceLocation::new(Direction3::try_new(v).unwrap())
}
fn query(location: SurfaceLocation) -> TerrainQuery {
    TerrainQuery {
        location,
        footprint: TerrainFootprint::COMPLETE,
    }
}

#[test]
fn terrain_publication_is_narrow_and_radius_edits_are_atomic() {
    let mut world = CelestialSystem::new(NonZeroU64::new(123).unwrap(), SimulationInstant::ZERO);
    let state = BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    );
    let id = world
        .insert_body("planet", BodyProperties::new(1e20, 1e4).unwrap(), state)
        .unwrap();
    let revision = world.revision();
    let first = definition(1, 10.0);
    world.edit_terrain(id, Some(first.clone())).unwrap();
    assert_eq!(world.revision(), revision);
    assert_eq!(world.sample_time(), SimulationInstant::ZERO);
    assert_eq!(*world.body(id).unwrap().state(), state);
    assert_eq!(world.body(id).unwrap().terrain_revision().value(), 1);
    world.edit_terrain(id, Some(first.clone())).unwrap();
    assert_eq!(world.body(id).unwrap().terrain_revision().value(), 1);
    assert!(
        world
            .edit_body(id, "changed", BodyProperties::new(2e20, 100.0).unwrap())
            .is_err()
    );
    assert!(
        world
            .edit_properties(id, BodyProperties::new(2e20, 100.0).unwrap())
            .is_err()
    );
    assert!(world.edit_terrain(id, Some(definition(2, 1e4))).is_err());
    let body = world.body(id).unwrap();
    assert_eq!(body.name(), "planet");
    assert_eq!(body.properties(), &BodyProperties::new(1e20, 1e4).unwrap());
    assert_eq!(body.terrain(), Some(&first));
    assert_eq!(body.terrain_revision().value(), 1);
    assert_eq!(world.revision(), revision);
    world.edit_terrain(id, Some(definition(2, 10.0))).unwrap();
    world.edit_terrain(id, Some(first.clone())).unwrap();
    assert_eq!(world.body(id).unwrap().terrain(), Some(&first));
    assert_eq!(world.body(id).unwrap().terrain_revision().value(), 3);
    world
        .edit_body(id, "renamed", BodyProperties::new(2e20, 2e4).unwrap())
        .unwrap();
    assert_eq!(world.body(id).unwrap().terrain_revision().value(), 3);
    world.edit_state(id, state).unwrap();
    assert_eq!(world.body(id).unwrap().terrain(), Some(&first));
    world.edit_terrain(id, None).unwrap();
    assert_eq!(world.body(id).unwrap().terrain_revision().value(), 4);
}

#[test]
fn complete_definition_equality_and_input_validation() {
    assert_ne!(definition(1, 10.0), definition(2, 10.0));
    assert_ne!(definition(1, 10.0), definition(1, 11.0));
    assert_eq!(definition(1, -0.0), definition(1, 0.0));
    assert_eq!(
        TerrainGeneratorVersion::from_code(2).unwrap(),
        TerrainGeneratorVersion::V2
    );
    assert!(TerrainGeneratorVersion::from_code(3).is_err());
    for x in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(TerrainFootprint::new(x).is_err());
    }
    for x in [f64::NAN, f64::INFINITY, -1.0] {
        assert!(
            TerrainBandConfig::new(
                x,
                TerrainScale::Metres {
                    longest_wavelength_m: 64.0
                },
                4
            )
            .is_err()
        );
    }
    assert!(
        TerrainBandConfig::new(
            1.0,
            TerrainScale::Metres {
                longest_wavelength_m: 32.0
            },
            4
        )
        .is_err()
    );
    for r in [1e4, 3.74e5, 6.371e6, 1.2742e7] {
        definition(1, 10.0).validate_radius(r).unwrap();
    }
}

#[test]
fn footprint_taper_endpoints_monotonicity_and_smoothness() {
    for rho in [50_000.0, 2.0] {
        let footprint = TerrainFootprint::new(rho).unwrap();
        assert_eq!(footprint.octave_weight(4.0 * rho).unwrap(), 0.0);
        assert_eq!(footprint.octave_weight(8.0 * rho).unwrap(), 1.0);
        let mut last = 0.0;
        for i in 0..=1000 {
            let w = footprint
                .octave_weight(rho * (4.0 + 4.0 * i as f64 / 1000.0))
                .unwrap();
            assert!(w >= last && w <= 1.0);
            last = w;
        }
        let eps = 1e-5;
        assert!(footprint.octave_weight(rho * (4.0 + eps)).unwrap() / eps < 1e-4);
        assert!((1.0 - footprint.octave_weight(rho * (8.0 - eps)).unwrap()) / eps < 1e-4);
    }
    assert_eq!(
        TerrainFootprint::new(50_000.0)
            .unwrap()
            .octave_weight(200_000.0)
            .unwrap(),
        0.0
    );
    assert_eq!(
        TerrainFootprint::new(2.0)
            .unwrap()
            .octave_weight(8.0)
            .unwrap(),
        0.0
    );
    assert_eq!(
        TerrainFootprint::new(1.0)
            .unwrap()
            .octave_weight(8.0)
            .unwrap(),
        1.0
    );
    assert_eq!(TerrainFootprint::COMPLETE.octave_weight(8.0).unwrap(), 1.0);
}

#[test]
fn analytic_scalar_batch_normals_and_independent_derivatives() {
    let radius = 6.371e6;
    let axis = Direction3::try_new(DVec3::new(1.0, 2.0, -3.0)).unwrap();
    let terrain = AnalyticTerrain::linear(radius, 1000.0, axis).unwrap();
    let locations: Vec<_> = (0..4096)
        .map(|i| {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / 4096.0;
            let angle = i as f64 * 2.399963229728653;
            let r = (1.0 - z * z).sqrt();
            location(DVec3::new(r * angle.cos(), r * angle.sin(), z))
        })
        .collect();
    let mut output = vec![TerrainSample::default(); locations.len()];
    for (input, out) in locations.chunks(32).zip(output.chunks_mut(32)) {
        terrain
            .evaluate_batch(input, TerrainFootprint::COMPLETE, out)
            .unwrap();
    }
    let [min, max] = terrain.height_interval_m();
    for (&n, &sample) in locations.iter().zip(&output) {
        assert_eq!(sample, terrain.evaluate_point(query(n)));
        assert!((min..=max).contains(&sample.height_m()));
        let normal = sample.normal_body(n, radius).unwrap().unit();
        assert!(normal.is_finite() && (normal.length() - 1.0).abs() < 1e-12);
        let tangent = n.direction().unit().cross(DVec3::X).normalize();
        let h = 1e-5;
        let fd = (terrain
            .evaluate_point(query(location(n.direction().unit() + h * tangent)))
            .height_m()
            - terrain
                .evaluate_point(query(location(n.direction().unit() - h * tangent)))
                .height_m())
            / (2.0 * h);
        assert!((fd - sample.tangent_gradient_m_per_unit_direction().dot(tangent)).abs() < 1e-6);
        let expected = (n.direction().unit()
            - 1000.0
                * (axis.unit() - n.direction().unit() * n.direction().unit().dot(axis.unit()))
                / (radius + sample.height_m()))
        .normalize();
        assert!((normal - expected).length() < 1e-10);
        let second_tangent = n.direction().unit().cross(tangent);
        let position = |v| {
            let loc = location(v);
            loc.direction().unit() * (radius + terrain.evaluate_point(query(loc)).height_m())
        };
        let du = position(n.direction().unit() + h * tangent)
            - position(n.direction().unit() - h * tangent);
        let dv = position(n.direction().unit() + h * second_tangent)
            - position(n.direction().unit() - h * second_tangent);
        let numerical_normal = du.cross(dv).normalize();
        assert!((normal - numerical_normal).length() < 1e-7);
    }
    for height in [-1000.0, 0.0, 1000.0] {
        let field = AnalyticTerrain::constant(radius, height).unwrap();
        let n = locations[0];
        let sample = field.evaluate_point(query(n));
        assert_eq!(sample.height_m(), height);
        assert_eq!(sample.normal_body(n, radius).unwrap(), n.direction());
        assert_eq!(sample.slope_angle_rad(radius).unwrap(), 0.0);
    }
    let old = output[0];
    assert!(
        terrain
            .evaluate_batch(&locations, TerrainFootprint::COMPLETE, &mut output[..1])
            .is_err()
    );
    assert_eq!(output[0], old);
}

#[test]
fn canonical_faces_and_parent_child_share_query_bits() {
    let terrain = AnalyticTerrain::linear(
        6.371e6,
        1000.0,
        Direction3::try_new(DVec3::new(2.0, -1.0, 3.0)).unwrap(),
    )
    .unwrap();
    for level in [0, 1, 5, 16, 30] {
        for face in CubeFace::ALL {
            let count = 1u32 << level;
            for (x, y) in [(0, 0), (count - 1, count - 1)] {
                let address = CubePatchAddress::try_new(face, level, x, y).unwrap();
                for edge in PatchEdge::ALL {
                    let neighbor = address.neighbor(edge);
                    for k in 0..=16 {
                        let a = edge.grid(k, 16);
                        let b = neighbor
                            .edge
                            .grid(if neighbor.reversed { 16 - k } else { k }, 16);
                        let n = SurfaceLocation::new(
                            address.sample_key(a[0], a[1], 16).unwrap().direction(),
                        );
                        let m = SurfaceLocation::new(
                            neighbor
                                .address
                                .sample_key(b[0], b[1], 16)
                                .unwrap()
                                .direction(),
                        );
                        assert_eq!(
                            terrain.evaluate_point(query(n)),
                            terrain.evaluate_point(query(m))
                        );
                    }
                }
                if level < 30 {
                    for (child_index, child) in address.children().unwrap().into_iter().enumerate()
                    {
                        for j in 0..=8 {
                            for i in 0..=8 {
                                let n = SurfaceLocation::new(
                                    address
                                        .sample_key(
                                            i + child_index as u32 % 2 * 8,
                                            j + child_index as u32 / 2 * 8,
                                            16,
                                        )
                                        .unwrap()
                                        .direction(),
                                );
                                let m = SurfaceLocation::new(
                                    child.sample_key(i * 2, j * 2, 16).unwrap().direction(),
                                );
                                assert_eq!(
                                    terrain.evaluate_point(query(n)),
                                    terrain.evaluate_point(query(m))
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
