use glam::DVec3;
use mundaris_math::{Direction3, surface::*};

#[test]
fn poles_handedness_and_transport() {
    for p in [
        DVec3::Y,
        -DVec3::Y,
        DVec3::X,
        DVec3::Z,
        DVec3::new(1e-8, 1.0, 0.0),
        DVec3::new(1.0, 2.0, 3.0),
    ] {
        let up = Direction3::try_new(p).unwrap();
        let t = SurfaceTangentBasis::new(up);
        let [e, n, u] = [t.east().unit(), t.north().unit(), t.up().unit()];
        for v in [e, n, u] {
            assert!((v.length() - 1.0).abs() <= 1e-12);
        }
        assert!(e.dot(n).abs() <= 1e-12 && e.dot(u).abs() <= 1e-12 && n.dot(u).abs() <= 1e-12);
        assert!((e.cross(n) - u).length() <= 1e-12);
        assert!((e.cross(u).dot(-n) - 1.0).abs() <= 1e-12);
        assert_eq!(SurfaceTangentBasis::new(up).east(), t.east());
    }
    let mut t = SurfaceTangentBasis::new(Direction3::try_new(DVec3::new(0.01, 1.0, 0.0)).unwrap());
    for i in 0..1000 {
        let up = Direction3::try_new(DVec3::new(0.01 - i as f64 * 0.00002, 1.0, 0.000001)).unwrap();
        let next = t.transported(up);
        assert!((next.east().unit() - t.east().unit()).length() < 0.0001);
        t = next;
    }
    assert!(Direction3::try_new(DVec3::ZERO).is_err());
}
