use glam::DVec3;
use astrum_math::noise::{GLOBAL_BOUNDS, gradient_noise, gradient_noise_value, lattice_hash};

#[test]
fn fixed_hash_vectors_and_signed_indices() {
    assert_eq!(lattice_hash(0, 0, 0, 0), 3_746_585_686_858_627_171);
    assert_eq!(lattice_hash(7, -1, 2, -3), 1_186_183_337_883_307_218);
    assert_eq!(
        lattice_hash(u64::MAX, i64::MIN, 0, i64::MAX),
        13_871_856_802_698_682_116
    );
    assert_eq!(
        lattice_hash(0xdead_beef, 123_456, -654_321, 42),
        2_325_461_484_154_693_503
    );
}

#[test]
fn analytic_derivatives_match_independent_finite_difference_step_sweep() {
    for seed in [0, 1, 0xdead_beef, u64::MAX] {
        for p in [
            DVec3::new(0.13, 0.29, 0.71),
            DVec3::new(-8.2, 4.3, 1.1),
            DVec3::new(0.5, 0.5, 0.5),
        ] {
            let s = gradient_noise(seed, p).unwrap();
            for axis in 0..3 {
                for h in [1e-3, 1e-4, 1e-5, 1e-6] {
                    let centered = |step: f64| {
                        let delta = [DVec3::X, DVec3::Y, DVec3::Z][axis] * step;
                        (gradient_noise(seed, p + delta).unwrap().value
                            - gradient_noise(seed, p - delta).unwrap().value)
                            / (2. * step)
                    };
                    let fd = (4.0 * centered(h * 0.5) - centered(h)) / 3.0;
                    assert!(
                        (fd - s.gradient[axis]).abs() <= 1e-6 * s.gradient[axis].abs().max(1.0),
                        "{seed} {p} {axis} {h}: {fd} vs {}",
                        s.gradient[axis]
                    );
                }
            }
            assert!(s.value.abs() <= GLOBAL_BOUNDS.value_abs);
            assert!(s.gradient.abs().max_element() <= GLOBAL_BOUNDS.gradient_component_abs);
            for axis in 0..3 {
                let h = 1e-5;
                let d = [DVec3::X, DVec3::Y, DVec3::Z][axis] * h;
                let numerical = (gradient_noise(seed, p + d).unwrap().gradient
                    - gradient_noise(seed, p - d).unwrap().gradient)
                    / (2.0 * h);
                assert!(numerical.abs().max_element() <= GLOBAL_BOUNDS.hessian_component_abs);
            }
        }
    }
}

#[test]
fn deterministic_seeded_ensemble_stays_inside_global_envelopes() {
    let mut state = 0x1234_5678_9abc_def0u64;
    for seed in [0, 1, 0xdead_beef, u64::MAX] {
        for sample_index in 0..4096 {
            let mut coordinate = || {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 11) as f64 / ((1u64 << 53) as f64)) * 200.0 - 100.0
            };
            let p = DVec3::new(coordinate(), coordinate(), coordinate());
            let a = gradient_noise(seed, p).unwrap();
            let b = gradient_noise(seed, p).unwrap();
            assert_eq!(a.value.to_bits(), b.value.to_bits());
            assert_eq!(
                a.value.to_bits(),
                gradient_noise_value(seed, p).unwrap().to_bits()
            );
            assert_eq!(
                a.gradient.to_array().map(f64::to_bits),
                b.gradient.to_array().map(f64::to_bits)
            );
            assert!(a.value.abs() <= GLOBAL_BOUNDS.value_abs);
            assert!(a.gradient.abs().max_element() <= GLOBAL_BOUNDS.gradient_component_abs);
            if sample_index < 256 {
                for axis in 0..3 {
                    let offset = [DVec3::X, DVec3::Y, DVec3::Z][axis] * 1e-5;
                    let gradient_difference = (gradient_noise(seed, p + offset).unwrap().gradient
                        - gradient_noise(seed, p - offset).unwrap().gradient)
                        / 2e-5;
                    assert!(
                        gradient_difference.abs().max_element()
                            <= GLOBAL_BOUNDS.hessian_component_abs
                    );
                }
            }
        }
    }
}

#[test]
fn lattice_boundaries_are_c2_and_domain_is_checked() {
    for axis in 0..3 {
        for k in -4..=4 {
            let mut a = [0.31, 0.47, 0.79];
            let mut b = a;
            a[axis] = k as f64 - 1e-8;
            b[axis] = k as f64 + 1e-8;
            let left = gradient_noise(42, DVec3::from_array(a)).unwrap();
            let right = gradient_noise(42, DVec3::from_array(b)).unwrap();
            assert!((left.value - right.value).abs() < 1e-6);
            assert!((left.gradient - right.gradient).length() < 1e-5);
        }
    }
    assert!(gradient_noise(0, DVec3::splat(f64::NAN)).is_err());
    assert!(gradient_noise(0, DVec3::splat(2.2e9)).is_err());
}

#[test]
fn seeds_separate_a_common_sample_ensemble_and_arbitrary_positions_are_continuous() {
    let mut differences = 0;
    for i in 0..4096 {
        let p = DVec3::new(
            i as f64 * 0.0317 - 16.0,
            i as f64 * 0.0123 + 0.3,
            i as f64 * 0.0731 - 7.4,
        );
        let a = gradient_noise(1, p).unwrap();
        let b = gradient_noise(2, p).unwrap();
        differences += usize::from(a.value.to_bits() != b.value.to_bits());
        let delta = DVec3::new(1e-8, -2e-8, 3e-8);
        let nearby = gradient_noise(1, p + delta).unwrap();
        let lipschitz = 3.0_f64.sqrt() * GLOBAL_BOUNDS.gradient_component_abs;
        assert!((a.value - nearby.value).abs() <= lipschitz * delta.length() + 1e-12);
        assert!(a.value.is_finite() && a.gradient.is_finite());
    }
    assert!(differences > 4000);
}
