use glam::DVec3;
use mundaris_math::{
    Direction3,
    surface::{DirectionalCap, SurfaceLocation},
};
use mundaris_world::terrain::*;

#[test]
fn subnormal_amplitudes_have_outward_regional_envelopes() {
    let axis = Direction3::try_new(DVec3::X).unwrap();
    let centre = Direction3::try_new(DVec3::Y).unwrap();
    for amplitude in [f64::from_bits(1), -f64::from_bits(1), f64::MIN_POSITIVE] {
        let field = AnalyticTerrain::linear(1e4, amplitude, axis).unwrap();
        let bound = field.bounds_for_region(DirectionalCap::new(centre, 0.1).unwrap());
        let [lo, hi] = bound.height_interval_m();
        assert!(lo < 0.0 && hi > 0.0);
        assert!(bound.cartesian_height_gradient_bound_m() >= amplitude.abs());
        let full = field.height_interval_m();
        assert!(full[0] < -amplitude.abs() && full[1] > amplitude.abs());
    }
}

#[test]
fn analytic_regional_intervals_contain_dense_independent_samples() {
    let axis = Direction3::try_new(DVec3::new(1.0, -2.0, 3.0)).unwrap();
    for amplitude in [-1000.0, 0.0, 1000.0] {
        let field = AnalyticTerrain::linear(6.371e6, amplitude, axis).unwrap();
        for centre in [
            DVec3::X,
            DVec3::Y,
            -axis.unit(),
            axis.unit(),
            DVec3::new(0.1, 0.7, -0.3),
        ] {
            let n = Direction3::try_new(centre).unwrap();
            let e = n
                .unit()
                .cross(if n.unit().x.abs() < 0.9 {
                    DVec3::X
                } else {
                    DVec3::Y
                })
                .normalize();
            let f = n.unit().cross(e);
            for angle in [0.0, 1e-8, 0.01, 0.5, std::f64::consts::PI] {
                let cap = DirectionalCap::new(n, angle).unwrap();
                let certificate = field.bounds_for_region(cap);
                let [min, max] = certificate.height_interval_m();
                for i in 0..4096 {
                    let theta = angle * i as f64 / 4095.0;
                    let phi = i as f64 * 2.399963229728653;
                    let direction = Direction3::try_new(
                        n.unit() * theta.cos() + (e * phi.cos() + f * phi.sin()) * theta.sin(),
                    )
                    .unwrap();
                    let sample = field.evaluate_point(TerrainQuery {
                        location: SurfaceLocation::new(direction),
                        footprint: TerrainFootprint::new(50_000.0).unwrap(),
                    });
                    assert!(sample.height_m().is_finite());
                    assert!(sample.height_m() >= min - 1e-9 && sample.height_m() <= max + 1e-9);
                    assert!(
                        sample.tangent_gradient_m_per_unit_direction().length()
                            <= certificate.cartesian_height_gradient_bound_m() + 1e-9
                    );
                }
                assert_eq!(certificate.cartesian_height_hessian_bound_m(), 0.0);
                assert_eq!(certificate.unresolved_height_bound_m(), 0.0);
            }
        }
    }
    assert!(DirectionalCap::new(axis, f64::NAN).is_err());
    assert!(DirectionalCap::new(axis, -0.1).is_err());
    let constant = AnalyticTerrain::constant(6.371e6, -1000.0).unwrap();
    assert_eq!(
        constant
            .bounds_for_region(DirectionalCap::new(axis, 0.01).unwrap())
            .height_interval_m(),
        [-1000.0; 2]
    );
    let linear = AnalyticTerrain::linear(6.371e6, 1000.0, axis).unwrap();
    let fine = linear
        .bounds_for_region(DirectionalCap::new(axis, 0.01).unwrap())
        .height_interval_m();
    let broad = linear
        .bounds_for_region(DirectionalCap::new(axis, 0.5).unwrap())
        .height_interval_m();
    assert!(fine[0] >= broad[0] && fine[1] <= broad[1]);
}
