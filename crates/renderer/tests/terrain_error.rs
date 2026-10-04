use glam::{DVec2, DVec3};
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_renderer::{
    CelestialProjection,
    planet_surface::{PatchMetadata, SurfaceErrorContributions, SurfaceExtent, SurfaceTopology},
};

#[test]
fn terrain_total_projection_preserves_smooth_path_and_expanded_bounds() {
    let radius = 6.371e6;
    let topology = SurfaceTopology::new();
    let patch = CubePatchAddress::try_new(CubeFace::PositiveZ, 4, 8, 8).unwrap();
    let metadata = PatchMetadata::build(patch, &topology).unwrap();
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let (centre, ball) = metadata
        .ball(radius, SurfaceExtent::smooth(radius))
        .unwrap();
    let centre_view = centre - DVec3::Z * (radius + 1e7);
    let errors = SurfaceErrorContributions {
        sphere_m: metadata.error_unit() * radius,
        ..Default::default()
    };
    let smooth = metadata.projected_error(centre_view, ball, radius, projection);
    let complete = metadata
        .projected_total_error(errors, centre_view, ball, projection)
        .unwrap();
    assert!(complete >= smooth && complete - smooth < 1e-12);
    let extent = SurfaceExtent {
        min_height_m: -1000.0,
        max_height_m: 5000.0,
        guaranteed_opaque_radius_m: 0.0,
    };
    let (displaced_centre, displaced_ball) = metadata.ball(radius, extent).unwrap();
    assert!(displaced_ball > ball && displaced_ball < ball + 5000.0);
    for j in 0..=16 {
        for i in 0..=16 {
            let n = patch.sample_direction(i, j, 16).unwrap().unit();
            for height in [extent.min_height_m, 2000.0, extent.max_height_m] {
                assert!((n * (radius + height) - displaced_centre).length() <= displaced_ball);
            }
        }
    }
    assert!(
        metadata
            .projected_total_error(
                SurfaceErrorContributions {
                    unresolved_m: 5000.0,
                    ..errors
                },
                displaced_centre - DVec3::Z * (radius + 1e7),
                displaced_ball,
                projection
            )
            .unwrap()
            > complete
    );
    assert!(!metadata.horizon_reject(-DVec3::Z * (radius + 1e7), radius, extent));
}

#[test]
fn vector_interpolation_remainder_uses_actual_domain_barycentrics() {
    let vertices = [
        DVec2::new(-0.2, 0.1),
        DVec2::new(0.7, 0.2),
        DVec2::new(0.1, 0.9),
    ];
    let diameter = (vertices[0] - vertices[1])
        .length()
        .max((vertices[0] - vertices[2]).length())
        .max((vertices[1] - vertices[2]).length());
    let affine = |p: DVec2| DVec3::new(2.0 * p.x - p.y, p.x + 3.0 * p.y, 0.5 + p.y);
    // Each component is q=x²+y², giving vector Hessian norm 2sqrt(3).
    let quadratic = |p: DVec2| affine(p) + DVec3::splat(p.length_squared());
    let bound =
        SurfaceErrorContributions::interpolation_bound_m(2.0 * 3.0_f64.sqrt(), diameter).unwrap();
    for i in 0..=64 {
        for j in 0..=64 - i {
            let weights = [i as f64 / 64.0, j as f64 / 64.0, (64 - i - j) as f64 / 64.0];
            let point =
                vertices[0] * weights[0] + vertices[1] * weights[1] + vertices[2] * weights[2];
            let interpolate = |f: &dyn Fn(DVec2) -> DVec3| {
                f(vertices[0]) * weights[0]
                    + f(vertices[1]) * weights[1]
                    + f(vertices[2]) * weights[2]
            };
            assert!((interpolate(&affine) - affine(point)).length() < 1e-12);
            assert!((interpolate(&quadratic) - quadratic(point)).length() <= bound);
        }
    }
    let fine =
        SurfaceErrorContributions::interpolation_bound_m(2.0 * 3.0_f64.sqrt(), diameter / 2.0)
            .unwrap();
    assert!((fine / bound - 0.25).abs() < 1e-15);
}

#[test]
fn equilateral_centroid_requires_the_triangle_variance_bound() {
    let vertices = [
        DVec2::new(1.0, 0.0),
        DVec2::new(-0.5, 3.0_f64.sqrt() * 0.5),
        DVec2::new(-0.5, -3.0_f64.sqrt() * 0.5),
    ];
    let diameter = (vertices[0] - vertices[1])
        .length()
        .max((vertices[1] - vertices[2]).length());
    let interpolated = vertices.iter().map(|v| v.length_squared()).sum::<f64>() / 3.0;
    let bound = SurfaceErrorContributions::interpolation_bound_m(2.0, diameter).unwrap();
    assert!(bound >= interpolated);
    assert!(bound - interpolated < 1e-14);
    // D²/8 is NOT a valid arbitrary-triangle remainder bound.
    assert!(2.0 * diameter * diameter / 8.0 < interpolated);
}

#[test]
fn every_error_component_increases_the_complete_certificate() {
    let base = SurfaceErrorContributions {
        sphere_m: 1.0,
        filtered_interpolation_m: 2.0,
        unresolved_m: 3.0,
        boundary_constraint_m: 4.0,
        morph_remaining_m: 5.0,
        numeric_m: 1e-6,
    };
    let sum = base.total_m().unwrap();
    assert!(sum >= 15.000001);
    assert!(
        SurfaceErrorContributions {
            unresolved_m: 4.0,
            ..base
        }
        .total_m()
        .unwrap()
            > sum
    );
    assert!(
        SurfaceErrorContributions {
            boundary_constraint_m: 5.0,
            ..base
        }
        .total_m()
        .unwrap()
            > sum
    );
    assert!(
        SurfaceErrorContributions {
            morph_remaining_m: 6.0,
            ..base
        }
        .total_m()
        .unwrap()
            > sum
    );
    for x in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(
            SurfaceErrorContributions {
                unresolved_m: x,
                ..base
            }
            .total_m()
            .is_err()
        );
        assert!(SurfaceErrorContributions::interpolation_bound_m(x, 1.0).is_err());
    }
    assert!(
        SurfaceErrorContributions {
            sphere_m: f64::MAX,
            filtered_interpolation_m: f64::MAX,
            ..base
        }
        .total_m()
        .is_err()
    );
    let tiny = SurfaceErrorContributions::interpolation_bound_m(f64::from_bits(1), 1e-100).unwrap();
    assert!(tiny > 0.0);
    assert_eq!(
        SurfaceErrorContributions::interpolation_bound_m(0.0, 1.0).unwrap(),
        0.0
    );
}
