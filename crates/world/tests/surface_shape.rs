use glam::DVec3;
use mundaris_math::{Direction3, surface::CubeFace};
use mundaris_world::terrain::{ShapeDefinition, TerrainError};

fn direction(vector: DVec3) -> Direction3 {
    Direction3::try_new(vector).unwrap()
}

fn irregular(seed: u64) -> ShapeDefinition {
    ShapeDefinition::irregular([1.35, 0.92, 0.70], 0.16, 0.12, seed).unwrap()
}

#[test]
fn ellipsoid_and_irregular_gradients_match_tangent_finite_differences() {
    let shapes = [
        ShapeDefinition::ellipsoid([1.35, 0.92, 0.70]).unwrap(),
        irregular(0x8137_49ab),
    ];
    let directions = [
        DVec3::X,
        DVec3::Y,
        DVec3::Z,
        DVec3::new(1.0, 1.0, 1.0).normalize(),
        DVec3::new(-1.0, 1.0, 0.0).normalize(),
        DVec3::new(0.31, -0.72, 0.55).normalize(),
    ];
    let radius_m = 8.0e7;
    let epsilon = 2.0e-6;

    for shape in shapes {
        for normal in directions {
            let mut tangent = normal.cross(DVec3::new(0.17, -0.61, 0.77)).normalize();
            if !tangent.is_finite() {
                tangent = normal.cross(DVec3::Y).normalize();
            }
            let sample = shape.query(direction(normal), radius_m).unwrap();
            let plus = shape
                .query(direction(normal + epsilon * tangent), radius_m)
                .unwrap()
                .radius_m();
            let minus = shape
                .query(direction(normal - epsilon * tangent), radius_m)
                .unwrap()
                .radius_m();
            let finite_difference = (plus - minus) / (2.0 * epsilon);
            let analytic = sample.gradient_m().dot(tangent);
            let tolerance = radius_m * 8.0e-7;
            assert!(
                (finite_difference - analytic).abs() <= tolerance,
                "shape={shape:?}, direction={normal:?}, finite_difference={finite_difference}, analytic={analytic}"
            );
            assert!(sample.gradient_m().dot(normal).abs() <= tolerance);
            assert!((sample.normal().length() - 1.0).abs() < 2.0e-15);
            assert!(sample.normal().dot(normal) > 0.0);
        }
    }
}

#[test]
fn sampled_shape_radii_stay_within_the_conservative_envelope() {
    let shapes = [
        ShapeDefinition::sphere(),
        ShapeDefinition::ellipsoid([1.35, 0.92, 0.70]).unwrap(),
        irregular(0x8137_49ab),
    ];
    let radius_m = 1.0e8;
    for shape in shapes {
        let envelope = shape.conservative_radius_envelope_m(radius_m).unwrap();
        for face in CubeFace::ALL {
            for u in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                for v in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                    let sample = shape
                        .query(face.direction([u, v]).unwrap(), radius_m)
                        .unwrap();
                    assert!(sample.radius_m() >= envelope[0]);
                    assert!(sample.radius_m() <= envelope[1]);
                }
            }
        }

        let mut state = 0x5ae1_7492_d3c8_0b6fu64;
        for _ in 0..256 {
            let mut components = [0.0; 3];
            for component in &mut components {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *component = ((state >> 11) as f64 / ((1u64 << 53) as f64)) * 2.0 - 1.0;
            }
            if components.iter().all(|component| *component == 0.0) {
                components[0] = 1.0;
            }
            let sample = shape
                .query(direction(DVec3::from_array(components)), radius_m)
                .unwrap();
            assert!(sample.radius_m() >= envelope[0]);
            assert!(sample.radius_m() <= envelope[1]);
        }
    }
}

#[test]
fn sphere_returns_exact_radius_and_radial_normal() {
    let shape = ShapeDefinition::sphere();
    let radial = DVec3::new(0.27, -0.86, 0.43);
    let direction = direction(radial);
    let sample = shape.query(direction, 7.3e7).unwrap();
    assert_eq!(sample.radius_m().to_bits(), 7.3e7f64.to_bits());
    assert_eq!(sample.gradient_m(), DVec3::ZERO);
    assert_eq!(sample.normal(), direction.unit());
}

#[test]
fn canonical_cube_face_edges_and_corners_share_identical_shape_samples() {
    let shape = irregular(0x4521_9ac3);
    let radius_m = 6.0e6;
    let mut canonical = Vec::new();
    for face in CubeFace::ALL {
        for t in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            for uv in [[-1.0, t], [1.0, t], [t, -1.0], [t, 1.0]] {
                let direction = face.direction(uv).unwrap();
                let sample = shape.query(direction, radius_m).unwrap();
                if let Some((_, previous)) = canonical
                    .iter()
                    .find(|(known_direction, _)| *known_direction == direction)
                {
                    assert_eq!(*previous, sample);
                } else {
                    canonical.push((direction, sample));
                }
            }
        }
    }
}

#[test]
fn configuration_identity_is_versioned_seeded_and_canonicalizes_signed_zero() {
    let sphere = ShapeDefinition::sphere();
    let ellipsoid = ShapeDefinition::ellipsoid([1.0, 1.0, 1.0]).unwrap();
    assert_ne!(
        sphere.configuration_identity(),
        ellipsoid.configuration_identity()
    );
    assert_eq!(sphere.algorithm_name(), "SphereV1");
    assert_eq!(sphere.axes_fractions(), [1.0; 3]);
    assert_eq!(sphere.irregular_amplitudes(), [0.0; 2]);
    assert_eq!(sphere.seed(), None);

    let first = irregular(77);
    assert_eq!(first, irregular(77));
    assert_eq!(first.algorithm_name(), "IrregularV1");
    assert_eq!(first.axes_fractions(), [1.35, 0.92, 0.70]);
    assert_eq!(first.irregular_amplitudes(), [0.16, 0.12]);
    assert_eq!(first.seed(), Some(77));
    assert_ne!(
        first.configuration_identity(),
        irregular(78).configuration_identity()
    );
    let positive_zero = ShapeDefinition::irregular([1.0, 1.0, 1.0], 0.0, 0.12, 9).unwrap();
    let negative_zero = ShapeDefinition::irregular([1.0, 1.0, 1.0], -0.0, 0.12, 9).unwrap();
    assert_eq!(
        positive_zero.configuration_identity(),
        negative_zero.configuration_identity()
    );
}

#[test]
fn invalid_shape_controls_and_reference_radii_fail_explicitly() {
    assert_eq!(
        ShapeDefinition::ellipsoid([1.0, f64::NAN, 1.0]),
        Err(TerrainError::InvalidConfig)
    );
    assert_eq!(
        ShapeDefinition::ellipsoid([0.0, 1.0, 1.0]),
        Err(TerrainError::InvalidConfig)
    );
    assert!(ShapeDefinition::irregular([1.0; 3], f64::INFINITY, 0.1, 1).is_err());
    assert!(ShapeDefinition::irregular([1.0; 3], 0.8, 0.1, 1).is_err());

    let shape = irregular(3);
    assert_eq!(
        shape.query(direction(DVec3::X), 0.0),
        Err(TerrainError::InvalidRadius)
    );
    assert_eq!(
        shape.conservative_radius_envelope_m(1.0e8 + 1.0),
        Err(TerrainError::InvalidRadius)
    );
    assert!(shape.query(direction(DVec3::X), 1.0e8).is_ok());
}
