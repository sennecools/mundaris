use glam::DVec3;
use mundaris_simulation::*;

const G: f64 = 6.67430e-11;

#[test]
fn independent_axis_nonaxis_and_three_body_forces() {
    for positions in [
        vec![DVec3::ZERO, DVec3::X * 1e7],
        vec![DVec3::new(-2e6, 3e6, 5e6), DVec3::new(4e6, -7e6, 2e6)],
        vec![DVec3::ZERO, DVec3::X * 1e7, DVec3::new(2e7, 3e7, -4e7)],
    ] {
        let masses = [2e20, 3e20, 7e20];
        let masses = &masses[..positions.len()];
        let mut output = vec![DVec3::ZERO; positions.len()];
        let report = evaluate_accelerations(masses, &positions, &mut output).unwrap();
        assert_eq!(
            report.pair_evaluations,
            (positions.len() * (positions.len() - 1) / 2) as u64
        );
        for i in 0..positions.len() {
            // Independent ordered per-body oracle: squared norm / r^3, bounded fixture.
            let mut expected = DVec3::ZERO;
            for j in 0..positions.len() {
                if i != j {
                    let d = positions[j] - positions[i];
                    expected += d * (G * masses[j] / d.length().powi(3));
                }
            }
            assert!((output[i] - expected).length() / expected.length() <= 5e-14);
        }
        let force: DVec3 = output.iter().zip(masses).map(|(a, m)| *a * *m).sum();
        let scale: f64 = output.iter().zip(masses).map(|(a, m)| a.length() * m).sum();
        assert!(force.length() / scale <= 2e-14);
    }
    let mut output = [DVec3::ZERO; 2];
    evaluate_accelerations(&[2e20, 3e20], &[DVec3::ZERO, DVec3::X * 1e7], &mut output).unwrap();
    assert!((output[0].x - G * 3e20 / 1e14).abs() / (G * 3e20 / 1e14) <= 2e-14);
    assert!((output[1].x + G * 2e20 / 1e14).abs() / (G * 2e20 / 1e14) <= 2e-14);
}

#[test]
fn no_self_force_and_deterministic_order() {
    assert_eq!(
        evaluate_accelerations(&[], &[], &mut [])
            .unwrap()
            .pair_evaluations,
        0
    );
    let mut output = [DVec3::ONE];
    evaluate_accelerations(&[1.0], &[DVec3::ZERO], &mut output).unwrap();
    assert_eq!(output, [DVec3::ZERO]);
    for (n, pairs) in [(3, 3), (16, 120), (64, 2016), (256, 32640), (1024, 523776)] {
        assert_eq!(pair_count(n).unwrap(), pairs);
    }
}

#[test]
fn rejects_invalid_and_unrepresentable_pairs_without_clamps() {
    let positions = [DVec3::ZERO, DVec3::X];
    let mut output = [DVec3::ZERO; 2];
    assert_eq!(
        evaluate_accelerations(&[1.0], &positions, &mut output),
        Err(GravityError::LengthMismatch)
    );
    for mass in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            evaluate_accelerations(&[mass, 1.0], &positions, &mut output),
            Err(GravityError::InvalidMass(0))
        );
    }
    assert!(matches!(
        evaluate_accelerations(&[1.0; 2], &[DVec3::ZERO; 2], &mut output),
        Err(GravityError::CoincidentBodies {
            first: 0,
            second: 1
        })
    ));
    for separation in [f64::from_bits(1), 1e-200, 1e200] {
        assert!(matches!(
            evaluate_accelerations(
                &[1.0; 2],
                &[DVec3::ZERO, DVec3::X * separation],
                &mut output
            ),
            Err(GravityError::UnrepresentableGravity { .. })
        ));
    }
    assert!(
        evaluate_accelerations(
            &[1.0; 2],
            &[DVec3::X * f64::MAX, -DVec3::X * f64::MAX],
            &mut output
        )
        .is_err()
    );
    assert!(
        evaluate_accelerations(
            &[1.0; 2],
            &[DVec3::ZERO, DVec3::splat(f64::NAN)],
            &mut output
        )
        .is_err()
    );
    let masses = [1e24, 1e20];
    let positions = [DVec3::ZERO, DVec3::X * 1e7];
    assert!(evaluate_resolved_accelerations(&masses, &positions, &mut output, 10.0).is_ok());
    assert!(matches!(
        evaluate_resolved_accelerations(&masses, &positions, &mut output, 100.0),
        Err(GravityError::UnresolvedEncounter { .. })
    ));
}
