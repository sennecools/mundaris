use glam::DVec3;
use mundaris_math::{
    Direction3,
    surface::{CubeFace, CubePatchAddress, PatchEdge, SurfaceLocation},
};
use mundaris_world::terrain::{
    MoonTerrainConfig, MoonTerrainDefinition, MoonTerrainGenerator, MoonTerrainVersion,
    TerrainError, TerrainIdentity, TerrainSeed,
};

fn location(vector: DVec3) -> SurfaceLocation {
    SurfaceLocation::new(Direction3::try_new(vector).unwrap())
}

fn generator(identity: u64, seed: u64, radius_m: f64) -> MoonTerrainGenerator {
    let definition = MoonTerrainDefinition::new(
        TerrainIdentity(identity),
        TerrainSeed(seed),
        MoonTerrainConfig::default(),
    );
    MoonTerrainGenerator::new(&definition, radius_m).unwrap()
}

#[test]
fn definition_is_separate_versioned_and_complete_queries_repeat() {
    let definition = MoonTerrainDefinition::new(
        TerrainIdentity(42),
        TerrainSeed(7),
        MoonTerrainConfig::default(),
    );
    assert_eq!(definition.version(), MoonTerrainVersion::MoonLikeV1);
    assert_eq!(definition.version().code(), 1);
    let field = MoonTerrainGenerator::new(&definition, 1_737_000.0).unwrap();
    let point = location(DVec3::new(0.4, -0.7, 0.3));
    assert_eq!(
        field.evaluate_point(point).unwrap(),
        field.evaluate_point(point).unwrap()
    );

    let changed_identity = generator(43, 7, field.radius_m());
    let changed_seed = generator(42, 8, field.radius_m());
    assert_ne!(
        field
            .evaluate_point(point)
            .unwrap()
            .terrain()
            .height_m()
            .to_bits(),
        changed_identity
            .evaluate_point(point)
            .unwrap()
            .terrain()
            .height_m()
            .to_bits()
    );
    assert_ne!(
        field
            .evaluate_point(point)
            .unwrap()
            .terrain()
            .height_m()
            .to_bits(),
        changed_seed
            .evaluate_point(point)
            .unwrap()
            .terrain()
            .height_m()
            .to_bits()
    );
}

#[test]
fn batch_reordering_and_materials_are_deterministic_and_normalized() {
    let field = generator(9, 123, 109_000.0);
    let vectors = [
        DVec3::X,
        DVec3::Y,
        DVec3::Z,
        DVec3::new(1.0, 1.0, 1.0),
        DVec3::new(-1.0, 1.0, 0.0),
    ];
    let points: Vec<_> = vectors.into_iter().map(location).collect();
    let mut samples = [field.evaluate_point(points[0]).unwrap(); 5];
    field.evaluate_batch(&points, &mut samples).unwrap();
    for (index, point) in points.iter().enumerate() {
        assert_eq!(samples[index], field.evaluate_point(*point).unwrap());
        let material = samples[index].material();
        let weights = [
            material.regolith_weight(),
            material.rock_weight(),
            material.basalt_weight(),
        ];
        assert!(
            weights
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        );
        assert!((weights.iter().sum::<f64>() - 1.0).abs() < 2.0e-15);
    }
    let reordered = [points[4], points[2], points[0], points[3], points[1]];
    let mut reordered_samples = [samples[0]; 5];
    field
        .evaluate_batch(&reordered, &mut reordered_samples)
        .unwrap();
    for (index, point) in reordered.iter().enumerate() {
        assert_eq!(
            reordered_samples[index],
            field.evaluate_point(*point).unwrap()
        );
    }
    let mut short = [samples[0]; 4];
    assert_eq!(
        field.evaluate_batch(&points, &mut short),
        Err(TerrainError::LengthMismatch)
    );
}

#[test]
fn analytic_gradient_matches_finite_differences_at_edges_and_overlaps() {
    let field = generator(0x1337, 0x91, 109_000.0);
    let points = [
        DVec3::new(1.0, 1.0, 1.0).normalize(),
        DVec3::new(1.0, 1.0, 0.0).normalize(),
        DVec3::new(-1.0, 0.0, 1.0).normalize(),
        DVec3::new(0.2, -0.9, 0.3).normalize(),
        DVec3::new(-0.45, 0.1, 0.89).normalize(),
    ];
    let epsilon = 1.0e-8;
    for normal in points {
        let direction = location(normal);
        let sample = field.evaluate_point(direction).unwrap().terrain();
        let mut tangent = normal.cross(DVec3::new(0.31, -0.72, 0.55)).normalize();
        if !tangent.is_finite() {
            tangent = normal.cross(DVec3::Y).normalize();
        }
        let plus = location((normal + epsilon * tangent).normalize());
        let minus = location((normal - epsilon * tangent).normalize());
        let fd = (field.evaluate_point(plus).unwrap().terrain().height_m()
            - field.evaluate_point(minus).unwrap().terrain().height_m())
            / (2.0 * epsilon);
        let analytic = sample.tangent_gradient_m_per_unit_direction().dot(tangent);
        let tolerance = 0.08 + analytic.abs() * 0.015;
        assert!(
            (fd - analytic).abs() <= tolerance,
            "gradient mismatch at {normal:?}: fd={fd}, analytic={analytic}, tolerance={tolerance}"
        );
    }
}

#[test]
fn canonical_cube_edges_and_corners_agree_and_heights_stay_bounded() {
    let field = generator(11, 99, 1_737_000.0);
    let samples = [
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::new(1.0, 1.0, 1.0),
        DVec3::new(0.0, -1.0, 1.0),
    ];
    for vector in samples {
        let canonical = location(vector);
        let duplicate = location(vector.normalize() * 7.0);
        assert_eq!(
            field.evaluate_point(canonical).unwrap(),
            field.evaluate_point(duplicate).unwrap()
        );
    }
    let bound = field.conservative_absolute_height_bound_m();
    for index in 0..96 {
        let angle = index as f64 * 0.6180339887498948;
        let z = 1.0 - 2.0 * (index as f64 + 0.5) / 96.0;
        let radial = (1.0 - z * z).sqrt();
        let point = location(DVec3::new(radial * angle.cos(), radial * angle.sin(), z));
        let height = field.evaluate_point(point).unwrap().terrain().height_m();
        assert!(height.abs() <= bound, "height {height} exceeded {bound}");
    }
}

#[test]
fn canonical_cube_patch_edges_and_corners_reuse_identical_samples() {
    let field = generator(88, 1001, 1_737_000.0);
    for level in [0, 3] {
        let count = 1u32 << level;
        for face in CubeFace::ALL {
            for (x, y) in [(0, 0), (count - 1, count - 1)] {
                let address = CubePatchAddress::try_new(face, level, x, y).unwrap();
                for edge in PatchEdge::ALL {
                    let neighbor = address.neighbor(edge);
                    for k in [0, 8, 16] {
                        let a = edge.grid(k, 16);
                        let b = neighbor
                            .edge
                            .grid(if neighbor.reversed { 16 - k } else { k }, 16);
                        let here = location(
                            address
                                .sample_key(a[0], a[1], 16)
                                .unwrap()
                                .direction()
                                .unit(),
                        );
                        let there = location(
                            neighbor
                                .address
                                .sample_key(b[0], b[1], 16)
                                .unwrap()
                                .direction()
                                .unit(),
                        );
                        assert_eq!(field.evaluate_point(here), field.evaluate_point(there));
                    }
                }
            }
        }
    }
}

#[test]
fn configuration_and_radius_validation_cover_extreme_scales() {
    assert!(MoonTerrainConfig::new(0.0, 0.0, [1.0e308, 1.0e307, 1.0e306], [0.0; 3],).is_err());
    assert!(MoonTerrainConfig::new(0.2, 0.0, [0.8, 0.4, 0.1], [0.0; 3]).is_err());
    let definition = MoonTerrainDefinition::new(
        TerrainIdentity(1),
        TerrainSeed(2),
        MoonTerrainConfig::default(),
    );
    assert!(matches!(
        MoonTerrainGenerator::new(&definition, f64::NAN),
        Err(TerrainError::InvalidRadius)
    ));
    assert!(MoonTerrainGenerator::new(&definition, 1.0).is_err());
    assert!(MoonTerrainGenerator::new(&definition, 20.0).is_ok());
    for radius in [109_000.0, 1_737_000.0, 100_000_000.0] {
        let field = MoonTerrainGenerator::new(&definition, radius).unwrap();
        let query = field
            .evaluate_point(location(DVec3::new(1.0, 1.0, 0.5)))
            .unwrap();
        assert!(query.terrain().height_m().abs() <= field.conservative_absolute_height_bound_m());
    }
}

#[test]
fn ancient_version_queries_are_bounded_repeatable_and_have_analytic_gradients() {
    for seed in [2, 7, 19] {
        for radius in [109_000.0, 1_737_000.0] {
            let definition = MoonTerrainDefinition::with_version(
                TerrainIdentity(0x1337),
                TerrainSeed(seed),
                MoonTerrainConfig::default(),
                MoonTerrainVersion::MoonLikeV2,
            );
            assert_eq!(definition.version().code(), 2);
            let field = MoonTerrainGenerator::new(&definition, radius).unwrap();
            assert_eq!(field.version(), MoonTerrainVersion::MoonLikeV2);
            let points: Vec<_> = (0..64)
                .map(|index| {
                    let z = 1.0 - 2.0 * (index as f64 + 0.5) / 64.0;
                    let a = index as f64 * 2.399963229728653;
                    let r = (1.0 - z * z).sqrt();
                    location(DVec3::new(r * a.cos(), r * a.sin(), z))
                })
                .collect();
            let mut samples = vec![field.evaluate_point(points[0]).unwrap(); points.len()];
            field.evaluate_batch(&points, &mut samples).unwrap();
            for (point, value) in points.iter().zip(samples) {
                assert_eq!(value, field.evaluate_point(*point).unwrap());
                assert!(
                    value.terrain().height_m().abs()
                        <= field.conservative_absolute_height_bound_m()
                );
                let material = value.material();
                let weights = [
                    material.regolith_weight(),
                    material.rock_weight(),
                    material.basalt_weight(),
                ];
                assert!(weights.iter().all(|x| (0.0..=1.0).contains(x)));
                assert!((weights.iter().sum::<f64>() - 1.0).abs() < 2e-15);
                let n = point.direction().unit();
                let tangent = n.cross(DVec3::new(0.31, -0.55, 0.77)).normalize();
                let analytic = value
                    .terrain()
                    .tangent_gradient_m_per_unit_direction()
                    .dot(tangent);
                // Resolve the narrow corrugated walls even at the large radius.
                // Two independent step sizes guard against a coincidental fit;
                // the derivative tolerance is unchanged.
                let tolerance = 0.3 + analytic.abs() * 0.001;
                let mut previous: Option<f64> = None;
                for e in [1e-9, 5e-10] {
                    let plus = field
                        .evaluate_point(location(n + tangent * e))
                        .unwrap()
                        .terrain()
                        .height_m();
                    let minus = field
                        .evaluate_point(location(n - tangent * e))
                        .unwrap()
                        .terrain()
                        .height_m();
                    let fd = (plus - minus) / (2.0 * e);
                    assert!(
                        (fd - analytic).abs() < tolerance,
                        "seed{seed} radius{radius} direction={n:?} step={e} fd={fd} analytic={analytic}"
                    );
                    if let Some(coarser) = previous {
                        assert!((fd - coarser).abs() < tolerance);
                    }
                    previous = Some(fd);
                }
            }
        }
    }
}

#[test]
fn ancient_envelope_and_canonical_patch_samples_are_checked_separately() {
    let too_large =
        MoonTerrainConfig::new(0.0, 0.0, [0.24, 0.018, 0.00003], [0.03, 0.0, 0.0]).unwrap();
    let bad = MoonTerrainDefinition::with_version(
        TerrainIdentity(1),
        TerrainSeed(2),
        too_large,
        MoonTerrainVersion::MoonLikeV2,
    );
    assert!(matches!(
        MoonTerrainGenerator::new(&bad, 109_000.0),
        Err(TerrainError::InvalidConfig)
    ));
    let definition = MoonTerrainDefinition::with_version(
        TerrainIdentity(77),
        TerrainSeed(19),
        MoonTerrainConfig::default(),
        MoonTerrainVersion::MoonLikeV2,
    );
    let field = MoonTerrainGenerator::new(&definition, 109_000.0).unwrap();
    for face in CubeFace::ALL {
        let address = CubePatchAddress::try_new(face, 0, 0, 0).unwrap();
        for edge in PatchEdge::ALL {
            let neighbor = address.neighbor(edge);
            for k in [0, 8, 16] {
                let a = edge.grid(k, 16);
                let b = neighbor
                    .edge
                    .grid(if neighbor.reversed { 16 - k } else { k }, 16);
                let here = location(
                    address
                        .sample_key(a[0], a[1], 16)
                        .unwrap()
                        .direction()
                        .unit(),
                );
                let there = location(
                    neighbor
                        .address
                        .sample_key(b[0], b[1], 16)
                        .unwrap()
                        .direction()
                        .unit(),
                );
                assert_eq!(field.evaluate_point(here), field.evaluate_point(there));
            }
        }
    }
}
