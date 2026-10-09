use glam::DVec3;
use astrum_math::{
    Direction3,
    surface::{CubeFace, CubePatchAddress, PatchEdge, SurfaceLocation},
};
use astrum_world::terrain::*;

const ALGORITHMS: [SurfaceAlgorithm; 3] = [
    SurfaceAlgorithm::RockyV3,
    SurfaceAlgorithm::IcyV1,
    SurfaceAlgorithm::VolcanicV1,
];
fn location(n: DVec3) -> SurfaceLocation {
    SurfaceLocation::new(Direction3::try_new(n).unwrap())
}
fn directions(count: usize) -> Vec<SurfaceLocation> {
    (0..count)
        .map(|i| {
            let z = 1.0 - 2.0 * (i as f64 + 0.5) / count as f64;
            let a = i as f64 * 2.399963229728653;
            let r = (1.0 - z * z).sqrt();
            location(DVec3::new(r * a.cos(), r * a.sin(), z))
        })
        .collect()
}
fn definition(algorithm: SurfaceAlgorithm, seed: u64) -> SurfaceDefinition {
    SurfaceDefinition::generated(TerrainIdentity(42), TerrainSeed(seed), algorithm)
}

#[test]
fn identities_separate_geometry_material_atmosphere_and_histories() {
    for algorithm in ALGORITHMS {
        let d = definition(algorithm, 2);
        let changed = definition(algorithm, 7);
        assert_ne!(d.terrain().parameters(), changed.terrain().parameters());
        assert_ne!(d.terrain_identity(), changed.terrain_identity());
        let m = SurfaceMaterialDefinition::new(d.material().version(), 0.8, 0.9).unwrap();
        let material_changed = SurfaceDefinition::new(
            d.identity(),
            d.seed(),
            *d.shape(),
            d.terrain(),
            m,
            d.atmosphere(),
        )
        .unwrap();
        assert_eq!(d.geometry_identity(), material_changed.geometry_identity());
        assert_ne!(d.material_identity(), material_changed.material_identity());
        let atmosphere_changed = SurfaceDefinition::new(
            d.identity(),
            d.seed(),
            *d.shape(),
            d.terrain(),
            d.material(),
            SurfaceAtmosphere::Descriptor {
                pressure_pa: 1000.0,
                scale_height_m: 800.0,
            },
        )
        .unwrap();
        assert_eq!(
            d.geometry_identity(),
            atmosphere_changed.geometry_identity()
        );
        assert_eq!(
            d.material_identity(),
            atmosphere_changed.material_identity()
        );
        assert_ne!(
            d.configuration_identity(),
            atmosphere_changed.configuration_identity()
        );
        let f = SurfaceGenerator::new(&d, 109000.0).unwrap();
        let g = SurfaceGenerator::new(&material_changed, 109000.0).unwrap();
        let atmosphere = SurfaceGenerator::new(&atmosphere_changed, 109000.0).unwrap();
        for p in directions(24) {
            let a = f.evaluate_point(p).unwrap();
            let b = g.evaluate_point(p).unwrap();
            assert_eq!(a.terrain(), b.terrain());
            assert_eq!(a.shape(), b.shape());
            assert_eq!(a.normal(), b.normal());
            assert_eq!(a, atmosphere.evaluate_point(p).unwrap());
        }
        let mut params = d.terrain().parameters();
        params.resurfacing_fraction *= 0.5;
        let terrain = SurfaceTerrainDefinition::new(algorithm, params).unwrap();
        let terrain_changed = SurfaceDefinition::new(
            d.identity(),
            d.seed(),
            *d.shape(),
            terrain,
            d.material(),
            d.atmosphere(),
        )
        .unwrap();
        assert_ne!(d.terrain_identity(), terrain_changed.terrain_identity());
    }
    let ids = ALGORITHMS.map(|a| definition(a, 2).configuration_identity());
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[0], ids[2]);
    assert_ne!(ids[1], ids[2]);
}

#[test]
fn complete_samples_are_bounded_normalized_and_scalar_batch_identical() {
    let points = directions(128);
    for algorithm in ALGORITHMS {
        for seed in [2, 7, 19, 31] {
            for radius in [5000.0, 109000.0, 1200000.0] {
                let d = definition(algorithm, seed);
                let g = SurfaceGenerator::new(&d, radius).unwrap();
                let [lo, hi] = g.conservative_radius_envelope_m();
                let mut batch = vec![g.evaluate_point(points[0]).unwrap(); points.len()];
                g.evaluate_batch(&points, &mut batch).unwrap();
                for (&p, &s) in points.iter().zip(&batch) {
                    assert_eq!(s, g.evaluate_point(p).unwrap());
                    assert!(s.radius_m() >= lo && s.radius_m() <= hi);
                    assert!(
                        s.terrain().height_m().abs() <= g.conservative_absolute_height_bound_m()
                    );
                    assert!(s.normal().is_finite());
                    assert!((s.normal().length() - 1.0).abs() < 1e-12);
                    assert!(s.normal().dot(p.direction().unit()) > 0.0);
                    let weights = s.material_weights();
                    assert!(
                        weights
                            .iter()
                            .all(|w| w.is_finite() && (0.0..=1.0).contains(w))
                    );
                    assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
                }
                let reverse: Vec<_> = points.iter().rev().copied().collect();
                g.evaluate_batch(&reverse, &mut batch).unwrap();
                for (&p, &s) in reverse.iter().zip(&batch) {
                    assert_eq!(s, g.evaluate_point(p).unwrap());
                }
                assert_eq!(
                    g.evaluate_batch(&points, &mut batch[..1]),
                    Err(TerrainError::LengthMismatch)
                );
            }
        }
    }
}

#[test]
fn complete_gradients_and_combined_shape_normals_match_independent_geometry() {
    let shape = ShapeDefinition::irregular([1.35, 0.92, 0.70], 0.16, 0.12, 17).unwrap();
    for algorithm in ALGORITHMS {
        let d = definition(algorithm, 7);
        let d = SurfaceDefinition::new(
            d.identity(),
            d.seed(),
            shape,
            d.terrain(),
            d.material(),
            d.atmosphere(),
        )
        .unwrap();
        let g = SurfaceGenerator::new(&d, 109000.0).unwrap();
        for p in directions(48) {
            let n = p.direction().unit();
            let u = n.cross(DVec3::X).normalize();
            let v = n.cross(u);
            let s = g.evaluate_point(p).unwrap();
            let expected = s.terrain().tangent_gradient_m_per_unit_direction().dot(u);
            let derivative = |step: f64| {
                (g.evaluate_point(location(n + u * step))
                    .unwrap()
                    .terrain()
                    .height_m()
                    - g.evaluate_point(location(n - u * step))
                        .unwrap()
                        .terrain()
                        .height_m())
                    / (2.0 * step)
            };
            let a = derivative(1e-9);
            let b = derivative(5e-10);
            let tolerance = 0.003 + expected.abs() * 0.00005;
            assert!(
                (a - expected).abs() < tolerance,
                "{} a={a} expected={expected} tolerance={tolerance}",
                algorithm.name()
            );
            assert!(
                (b - expected).abs() < tolerance,
                "{} b={b} expected={expected} tolerance={tolerance}",
                algorithm.name()
            );
            assert!((a - b).abs() < tolerance * 2.0);
            let h = 1e-9;
            let position = |n| {
                let p = location(n);
                g.evaluate_point(p).unwrap().position(p)
            };
            let du = position(n + u * h) - position(n - u * h);
            let dv = position(n + v * h) - position(n - v * h);
            let numerical = du.cross(dv).normalize();
            assert!((numerical - s.normal()).length() < 3e-5);
        }
    }
}

#[test]
fn canonical_edges_corners_and_cross_chart_reuse_are_bit_identical() {
    for algorithm in ALGORITHMS {
        let g = SurfaceGenerator::new(&definition(algorithm, 19), 109000.0).unwrap();
        for face in CubeFace::ALL {
            let address = CubePatchAddress::root(face);
            for edge in PatchEdge::ALL {
                let neighbor = address.neighbor(edge);
                for k in 0..=8 {
                    let a = edge.grid(k, 8);
                    let b = neighbor
                        .edge
                        .grid(if neighbor.reversed { 8 - k } else { k }, 8);
                    let p = SurfaceLocation::new(
                        address.sample_key(a[0], a[1], 8).unwrap().direction(),
                    );
                    let q = SurfaceLocation::new(
                        neighbor
                            .address
                            .sample_key(b[0], b[1], 8)
                            .unwrap()
                            .direction(),
                    );
                    assert_eq!(g.evaluate_point(p).unwrap(), g.evaluate_point(q).unwrap());
                }
            }
        }
    }
}

#[test]
fn invalid_history_radius_and_atmosphere_fail_without_silent_clamping() {
    let d = definition(SurfaceAlgorithm::IcyV1, 2);
    for radius in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e9] {
        assert!(SurfaceGenerator::new(&d, radius).is_err());
    }
    let mut p = d.terrain().parameters();
    p.resurfacing_fraction = 1.1;
    assert_eq!(
        SurfaceTerrainDefinition::new(SurfaceAlgorithm::IcyV1, p),
        Err(TerrainError::InvalidConfig)
    );
    assert!(
        SurfaceDefinition::new(
            d.identity(),
            d.seed(),
            *d.shape(),
            d.terrain(),
            d.material(),
            SurfaceAtmosphere::Descriptor {
                pressure_pa: -1.0,
                scale_height_m: 5.0
            }
        )
        .is_err()
    );
    assert!(
        SurfaceDefinition::new(
            d.identity(),
            d.seed(),
            *d.shape(),
            d.terrain(),
            SurfaceMaterialDefinition::new(SurfaceMaterialVersion::RockyV1, 0.5, 0.4).unwrap(),
            d.atmosphere()
        )
        .is_err()
    );
}

#[test]
fn each_geological_control_changes_the_complete_field() {
    let points = directions(64);
    for algorithm in ALGORITHMS {
        let d = definition(algorithm, 7);
        let g = SurfaceGenerator::new(&d, 109000.0).unwrap();
        let original: Vec<_> = points
            .iter()
            .map(|p| g.evaluate_point(*p).unwrap().terrain())
            .collect();
        for control in 0..7 {
            let mut p = d.terrain().parameters();
            match control {
                0 => p.age *= 0.6,
                1 => p.activity = 0.2 + 0.7 * p.activity,
                2 => p.resurfacing_fraction *= 0.6,
                3 => p.impact_retention *= 0.6,
                4 => p.relief_fraction *= 0.6,
                5 => p.feature_scale_fraction *= 0.7,
                _ => p.orientation_radians *= 0.7,
            }
            let changed = SurfaceDefinition::new(
                d.identity(),
                d.seed(),
                *d.shape(),
                SurfaceTerrainDefinition::new(algorithm, p).unwrap(),
                d.material(),
                d.atmosphere(),
            )
            .unwrap();
            assert_ne!(d.terrain_identity(), changed.terrain_identity());
            let altered = SurfaceGenerator::new(&changed, 109000.0).unwrap();
            assert!(
                points.iter().zip(&original).any(|(point, sample)| altered
                    .evaluate_point(*point)
                    .unwrap()
                    .terrain()
                    != *sample),
                "unused complete-field control {control} in {}",
                algorithm.name()
            );
        }
    }
}

#[test]
fn diagnostic_boundaries_reuse_complete_queries_deterministically() {
    for algorithm in ALGORITHMS {
        let g = SurfaceGenerator::new(&definition(algorithm, 7), 109000.0).unwrap();
        let mut count = 0;
        for near in directions(12) {
            let probes = g.diagnostic_boundary_probes(near).unwrap();
            let replay = g.diagnostic_boundary_probes(near).unwrap();
            assert_eq!(probes.len(), replay.len());
            for (probe, repeated) in probes.iter().zip(&replay) {
                assert_eq!(probe.label, repeated.label);
                assert_eq!(probe.location, repeated.location);
                let s = g.evaluate_point(probe.location).unwrap();
                assert_eq!(s, g.evaluate_point(repeated.location).unwrap());
                let n = probe.location.direction().unit();
                let tangent = n
                    .cross(if n.x.abs() < 0.8 { DVec3::X } else { DVec3::Y })
                    .normalize();
                let step = 1e-9;
                let fd = (g
                    .evaluate_point(location(n + tangent * step))
                    .unwrap()
                    .terrain()
                    .height_m()
                    - g.evaluate_point(location(n - tangent * step))
                        .unwrap()
                        .terrain()
                        .height_m())
                    / (2.0 * step);
                let analytic = s
                    .terrain()
                    .tangent_gradient_m_per_unit_direction()
                    .dot(tangent);
                assert!(
                    (fd - analytic).abs() < 0.005 + analytic.abs() * 5e-5,
                    "{} {} fd={fd} analytic={analytic}",
                    algorithm.name(),
                    probe.label
                );
                count += 1;
            }
        }
        assert!(
            count > 0,
            "missing supported boundary fixture for {}",
            algorithm.name()
        );
    }
}
