use glam::DVec3;
use mundaris_math::{Direction3, surface::*};

#[test]
fn checked_addresses_and_nested_canonical_samples() {
    for face in CubeFace::ALL {
        let root = CubePatchAddress::root(face);
        assert!(root.parent().is_none());
        assert!(CubePatchAddress::try_new(face, 31, 0, 0).is_err());
        for level in [0, 1, 5, 16, 30] {
            let count = 1 << level;
            assert!(CubePatchAddress::try_new(face, level, count, 0).is_err());
            let patch = CubePatchAddress::try_new(face, level, count - 1, count - 1).unwrap();
            for i in 0..=16 {
                for j in 0..=16 {
                    let d = patch.sample_direction(i, j, 16).unwrap().unit();
                    assert!((d.length() - 1.0).abs() <= 2e-14);
                    assert!(((d * 6.4e6).length() - 6.4e6).abs() <= 32.0 * f64::EPSILON * 6.4e6);
                }
            }
            if level == 30 {
                assert!(patch.children().is_err());
                continue;
            }
            for (child_index, child) in patch.children().unwrap().into_iter().enumerate() {
                assert_eq!(child.parent(), Some(patch));
                assert_eq!(
                    child.coordinates(),
                    [
                        2 * (count - 1) + child_index as u32 % 2,
                        2 * (count - 1) + child_index as u32 / 2
                    ]
                );
                for i in (0..=16).step_by(2) {
                    for j in (0..=16).step_by(2) {
                        assert_eq!(
                            child.sample_key(i, j, 16).unwrap(),
                            patch
                                .sample_key(
                                    i / 2 + child_index as u32 % 2 * 8,
                                    j / 2 + child_index as u32 / 2 * 8,
                                    16
                                )
                                .unwrap()
                        );
                    }
                }
            }
        }
        assert!(root.sample_direction(17, 0, 16).is_err());
        assert!(root.sample_direction(0, 0, 15).is_err());
    }
}

#[test]
fn independent_twenty_four_transitions_and_bit_equal_edges() {
    use CubeFace::*;
    use PatchEdge::*;
    // Independently authored expected cube-edge permutations, columns U-/U+/V-/V+.
    let expected = [
        [
            (PositiveZ, UMax, false),
            (NegativeZ, UMin, false),
            (NegativeY, UMax, true),
            (PositiveY, UMax, false),
        ],
        [
            (NegativeZ, UMax, false),
            (PositiveZ, UMin, false),
            (NegativeY, UMin, false),
            (PositiveY, UMin, true),
        ],
        [
            (NegativeX, VMax, true),
            (PositiveX, VMax, false),
            (PositiveZ, VMax, false),
            (NegativeZ, VMax, true),
        ],
        [
            (NegativeX, VMin, false),
            (PositiveX, VMin, true),
            (NegativeZ, VMin, true),
            (PositiveZ, VMin, false),
        ],
        [
            (NegativeX, UMax, false),
            (PositiveX, UMin, false),
            (NegativeY, VMax, false),
            (PositiveY, VMin, false),
        ],
        [
            (PositiveX, UMax, false),
            (NegativeX, UMin, false),
            (NegativeY, VMin, true),
            (PositiveY, VMax, true),
        ],
    ];
    for (f, face) in CubeFace::ALL.into_iter().enumerate() {
        for (e, edge) in PatchEdge::ALL.into_iter().enumerate() {
            for level in [0, 1, 5, 16, 30] {
                let count = 1u32 << level;
                for k in [0, count / 2, count - 1] {
                    let [x, y] = edge.grid(k, count - 1);
                    let patch = CubePatchAddress::try_new(face, level, x, y).unwrap();
                    let neighbor = patch.neighbor(edge);
                    assert_eq!(
                        (neighbor.address.face(), neighbor.edge, neighbor.reversed),
                        expected[f][e]
                    );
                    let reverse = neighbor.address.neighbor(neighbor.edge);
                    assert_eq!(reverse.address, patch);
                    assert_eq!(reverse.edge, edge);
                    assert_eq!(reverse.reversed, neighbor.reversed);
                    for i in 0..=16 {
                        let [a, b] = edge.grid(i, 16);
                        let [c, d] = neighbor
                            .edge
                            .grid(if neighbor.reversed { 16 - i } else { i }, 16);
                        assert_eq!(
                            patch.sample_key(a, b, 16).unwrap(),
                            neighbor.address.sample_key(c, d, 16).unwrap()
                        );
                        assert_eq!(
                            patch
                                .sample_direction(a, b, 16)
                                .unwrap()
                                .unit()
                                .to_array()
                                .map(f64::to_bits),
                            neighbor
                                .address
                                .sample_direction(c, d, 16)
                                .unwrap()
                                .unit()
                                .to_array()
                                .map(f64::to_bits)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn eight_corners_mapping_winding_and_location_identity() {
    for x in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for z in [-1.0, 1.0] {
                let cube = DVec3::new(x, y, z);
                let expected = cube / 3.0_f64.sqrt();
                let mut bits = None;
                let mut incident = 0;
                for face in CubeFace::ALL {
                    let [n, u, v] = face.basis();
                    assert_eq!(u.cross(v), n);
                    if cube.dot(n) != 1.0 {
                        continue;
                    }
                    incident += 1;
                    let uv = [cube.dot(u), cube.dot(v)];
                    let i = if uv[0] > 0.0 { 16 } else { 0 };
                    let j = if uv[1] > 0.0 { 16 } else { 0 };
                    for level in [0, 1, 5, 16, 30] {
                        let count = 1u32 << level;
                        let p = CubePatchAddress::try_new(
                            face,
                            level,
                            if i == 16 { count - 1 } else { 0 },
                            if j == 16 { count - 1 } else { 0 },
                        )
                        .unwrap();
                        let d = p.sample_direction(i, j, 16).unwrap().unit();
                        assert!((d - expected).length() <= 2e-14);
                        let actual = d.to_array().map(f64::to_bits);
                        if let Some(b) = bits {
                            assert_eq!(actual, b);
                        } else {
                            bits = Some(actual);
                        }
                        // Independent evaluated domain winding, including corner child convergence.
                        let a = p.sample_direction(0, 0, 16).unwrap().unit();
                        let b = p.sample_direction(16, 0, 16).unwrap().unit();
                        let c = p.sample_direction(16, 16, 16).unwrap().unit();
                        assert!((b - a).cross(c - a).dot(a) > 0.0);
                    }
                }
                assert_eq!(incident, 3);
                let location = SurfaceLocation::new(Direction3::try_new(cube).unwrap());
                assert_eq!(
                    location.face_uv().0,
                    if x > 0.0 {
                        CubeFace::PositiveX
                    } else {
                        CubeFace::NegativeX
                    }
                );
            }
        }
    }
    for face in CubeFace::ALL {
        assert_eq!(face.direction([0.0, 0.0]).unwrap().unit(), face.basis()[0]);
        for i in 0..=50 {
            for j in 0..=50 {
                let uv = [-1.0 + 2.0 * i as f64 / 50.0, -1.0 + 2.0 * j as f64 / 50.0];
                let d = face.direction(uv).unwrap();
                assert!((d.unit().length() - 1.0).abs() <= 2e-14);
                let (chart, coords) = SurfaceLocation::new(d).face_uv();
                assert!((chart.direction(coords).unwrap().unit() - d.unit()).length() <= 2e-14);
            }
        }
    }
}
