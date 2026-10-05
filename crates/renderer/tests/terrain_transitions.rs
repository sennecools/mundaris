use glam::DVec3;
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_renderer::planet_surface::{
    GRID_SAMPLES, GeneratedSurfacePatch, StitchedSurface, SurfaceErrorContributions, SurfaceExtent,
    SurfaceGeometrySample, SurfaceTopology, SurfaceTransition, TransitionVertex,
    active_surface_cover,
};

fn fixture(address: CubePatchAddress, profile: f64) -> GeneratedSurfacePatch {
    let gradient = DVec3::new(12.0, -7.0, 5.0);
    let samples = (0..GRID_SAMPLES)
        .map(|index| {
            let direction = address
                .sample_direction((index % 17) as u32, (index / 17) as u32, 16)
                .unwrap()
                .unit();
            let height = gradient.dot(direction) + profile * (address.level() as f64 / 100.0);
            let normal = (direction
                - (gradient - direction * gradient.dot(direction)) / (1000.0 + height))
                .normalize();
            SurfaceGeometrySample {
                position_body_m: direction * (1000.0 + height),
                normal_body: normal,
            }
        })
        .collect();
    GeneratedSurfacePatch::new(
        address,
        1000.0,
        1.0,
        samples,
        SurfaceExtent {
            min_height_m: -100.0,
            max_height_m: 100.0,
            guaranteed_opaque_radius_m: 0.0,
        },
        SurfaceErrorContributions::default(),
    )
    .unwrap()
}

fn surface(
    addresses: &[CubePatchAddress],
    topology: &SurfaceTopology,
    profile: f64,
) -> (
    Vec<mundaris_renderer::planet_surface::ActiveSurfacePatch>,
    StitchedSurface,
) {
    let mut addresses = addresses.to_vec();
    addresses.sort_unstable();
    let cover = active_surface_cover(&addresses, topology).unwrap();
    let geometry: Vec<_> = addresses
        .iter()
        .copied()
        .map(|address| fixture(address, profile))
        .collect();
    let refs: Vec<_> = geometry.iter().collect();
    let stitched = StitchedSurface::build(&cover, &refs, topology).unwrap();
    (cover, stitched)
}

fn referenced(
    samples: &[SurfaceGeometrySample],
    vertex: &TransitionVertex,
    old: bool,
) -> SurfaceGeometrySample {
    let reference = if old {
        vertex.old_reference
    } else {
        vertex.new_reference
    };
    let points = reference.indices.map(|index| samples[usize::from(index)]);
    let combine = |get: fn(SurfaceGeometrySample) -> DVec3| {
        get(points[0]) * reference.weights[0]
            + get(points[1]) * reference.weights[1]
            + get(points[2]) * reference.weights[2]
    };
    SurfaceGeometrySample {
        position_body_m: combine(|sample| sample.position_body_m),
        normal_body: combine(|sample| sample.normal_body),
    }
}

fn assert_close(a: DVec3, b: DVec3) {
    assert!((a - b).length() <= 1.0e-9, "{a:?} != {b:?}");
}

fn check_transition(old_addresses: Vec<CubePatchAddress>, new_addresses: Vec<CubePatchAddress>) {
    let topology = SurfaceTopology::new();
    let (old_cover, old_surface) = surface(&old_addresses, &topology, 18.0);
    let (new_cover, new_surface) = surface(&new_addresses, &topology, 18.0);
    let transition = SurfaceTransition::build(
        &old_cover,
        &old_surface,
        &new_cover,
        &new_surface,
        &topology,
        16 * 1024 * 1024,
    )
    .unwrap();
    assert!(!transition.triangles().is_empty());
    #[cfg(feature = "surface-profile")]
    {
        let profile = transition.profile();
        assert!(profile.overlapping_patch_pairs > 0);
        assert!(profile.destination_triangles_precomputed > 0);
        assert_eq!(profile.emitted_triangles, transition.triangles().len());
        assert!(profile.triangle_pairs_considered >= profile.bbox_rejected_pairs);
        assert!(profile.triangle_pairs_considered < profile.potential_triangle_pairs);
        assert!(profile.clipped_pairs > 0);
    }
    for fraction in [0.0, 0.125, 0.5, 0.875, 1.0] {
        let mut edges = std::collections::BTreeMap::<([i64; 3], [i64; 3]), (usize, i32)>::new();
        let mut append = |points: [DVec3; 3]| {
            assert!(
                (points[1] - points[0])
                    .cross(points[2] - points[0])
                    .dot(points[0] + points[1] + points[2])
                    > 0.0,
                "overlay winding inverted at t={fraction}"
            );
            let keys = points.map(|p| p.to_array().map(|v| (v * 1e7).round() as i64));
            for (a, b) in [(keys[0], keys[1]), (keys[1], keys[2]), (keys[2], keys[0])] {
                assert_ne!(a, b, "degenerate transition edge");
                let (edge, orientation) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                let item = edges.entry(edge).or_default();
                item.0 += 1;
                item.1 += orientation;
            }
        };
        for (p, g) in old_cover.iter().zip(old_surface.patches()) {
            if transition.affected_old().binary_search(&p.address).is_ok() {
                continue;
            }
            for triangle in topology.indices(p.stitch_mask).as_chunks::<3>().0 {
                append(triangle.map(|i| g.samples()[usize::from(i)].position_body_m));
            }
        }
        for triangle in transition.triangles() {
            append(triangle.map(|v| v.sample(fraction).unwrap().position_body_m));
        }
        let bad: Vec<_> = edges
            .iter()
            .filter(|(_, v)| **v != (2, 0))
            .take(4)
            .collect();
        assert!(
            bad.is_empty(),
            "transition+unchanged surface is not closed at t={fraction}: {bad:?}"
        );
    }
    for triangle in transition.triangles() {
        for vertex in triangle {
            let old_patch = old_surface
                .patches()
                .iter()
                .find(|patch| patch.address() == vertex.old_reference.address)
                .unwrap();
            let new_patch = new_surface
                .patches()
                .iter()
                .find(|patch| patch.address() == vertex.new_reference.address)
                .unwrap();
            let old = referenced(old_patch.samples(), vertex, true);
            let new = referenced(new_patch.samples(), vertex, false);
            assert_close(old.position_body_m, vertex.old.position_body_m);
            assert_close(old.normal_body, vertex.old.normal_body);
            assert_close(new.position_body_m, vertex.new.position_body_m);
            assert_close(new.normal_body, vertex.new.normal_body);
            for fraction in [0.0, 0.125, 0.5, 0.875, 1.0] {
                let sample = vertex.sample(fraction).unwrap();
                assert!(sample.position_body_m.is_finite());
                assert!(sample.normal_body.is_finite());
                assert!(sample.normal_body.length_squared() > 1.0e-20);
                assert!(sample.position_body_m.dot(sample.normal_body) > 0.0);
                assert_close(
                    sample.position_body_m,
                    vertex
                        .old
                        .position_body_m
                        .lerp(vertex.new.position_body_m, fraction),
                );
            }
            for fraction in [-f64::EPSILON, 1.0 + f64::EPSILON, f64::NAN] {
                assert!(vertex.sample(fraction).is_err());
            }
        }
    }
    for fraction in [0.0, 0.125, 0.5, 0.875, 1.0] {
        let mut normals = std::collections::BTreeMap::<[i64; 3], DVec3>::new();
        for vertex in transition.triangles().iter().flatten() {
            let sample = vertex.sample(fraction).unwrap();
            let key = sample
                .position_body_m
                .to_array()
                .map(|component| (component * 1e7).round() as i64);
            if let Some(previous) = normals.insert(key, sample.normal_body) {
                assert_close(previous, sample.normal_body);
            }
        }
    }
    let vertex = transition.triangles()[0][0];
    let mut invalid = vertex;
    invalid.old.normal_body = DVec3::ZERO;
    invalid.new.normal_body = DVec3::ZERO;
    assert!(invalid.sample(0.5).is_err(), "degenerate normal accepted");
    assert!(
        invalid.sample(0.0).is_err(),
        "degenerate old endpoint accepted"
    );
    assert!(
        invalid.sample(1.0).is_err(),
        "degenerate new endpoint accepted"
    );
    invalid.old.normal_body = -vertex.old.position_body_m.normalize();
    invalid.new.normal_body = -vertex.new.position_body_m.normalize();
    assert!(invalid.sample(0.5).is_err(), "inward normal accepted");
    assert!(
        SurfaceTransition::build(
            &old_cover,
            &old_surface,
            &new_cover,
            &new_surface,
            &topology,
            1,
        )
        .is_err()
    );
}

#[test]
fn mixed_corner_refinements_preserve_balanced_transition_covers() {
    let mut level_one = Vec::new();
    for face in CubeFace::ALL {
        for child in CubePatchAddress::root(face).children().unwrap() {
            level_one.push(child);
        }
    }
    let mut corners = std::collections::BTreeMap::<[i64; 3], Vec<CubePatchAddress>>::new();
    for address in &level_one {
        for u in [0, 16] {
            for v in [0, 16] {
                let direction = address.sample_direction(u, v, 16).unwrap().unit();
                let key = direction
                    .to_array()
                    .map(|component| (component * 1e9).round() as i64);
                let entries = corners.entry(key).or_default();
                if !entries.contains(address) {
                    entries.push(*address);
                }
            }
        }
    }
    let mut checked = 0;
    for incident in corners.values().filter(|addresses| addresses.len() == 3) {
        // Each cube corner involves three distinct faces. Refine one patch, then
        // an incident patch on another face to exercise changing corner masks.
        for first_index in 0..incident.len() {
            for second_index in first_index + 1..incident.len() {
                let first = incident[first_index];
                let second = incident[second_index];
                assert_ne!(first.face(), second.face());
                let refine = |addresses: &[CubePatchAddress], patch| {
                    addresses
                        .iter()
                        .flat_map(|address| {
                            if *address == patch {
                                address.children().unwrap().to_vec()
                            } else {
                                vec![*address]
                            }
                        })
                        .collect::<Vec<_>>()
                };
                let once = refine(&level_one, first);
                let twice = refine(&once, second);
                check_transition(once.clone(), twice.clone());
                check_transition(twice, once);
                checked += 1;
            }
        }
    }
    assert_eq!(
        checked,
        8 * 3,
        "expected three face-pairs at each cube corner"
    );
}

#[test]
fn transition_rejects_two_level_replacement_jumps_for_balanced_covers() {
    let topology = SurfaceTopology::new();
    let roots: Vec<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    let mut deep = Vec::new();
    for root in &roots {
        for child in root.children().unwrap() {
            deep.extend(child.children().unwrap());
        }
    }
    deep.sort_unstable();
    let (root_cover, root_surface) = surface(&roots, &topology, 0.0);
    let (deep_cover, deep_surface) = surface(&deep, &topology, 0.0);
    assert!(
        SurfaceTransition::build(
            &root_cover,
            &root_surface,
            &deep_cover,
            &deep_surface,
            &topology,
            16 * 1024 * 1024
        )
        .is_err()
    );
    assert!(
        SurfaceTransition::build(
            &deep_cover,
            &deep_surface,
            &root_cover,
            &root_surface,
            &topology,
            16 * 1024 * 1024
        )
        .is_err()
    );
}

#[test]
fn split_and_reverse_transitions_reconstruct_captured_stitched_triangles() {
    let roots: Vec<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    for face in CubeFace::ALL {
        let mut split = Vec::new();
        for root in &roots {
            if root.face() == face {
                split.extend(root.children().unwrap());
            } else {
                split.push(*root);
            }
        }
        split.sort_unstable();
        check_transition(roots.clone(), split.clone());
        check_transition(split, roots.clone());
    }
}

#[test]
fn multi_face_and_child_refinement_transitions_are_bounded_and_finite() {
    let roots: Vec<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    let mut refined = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        if index < 3 {
            refined.extend(root.children().unwrap());
        } else {
            refined.push(*root);
        }
    }
    refined.sort_unstable();
    check_transition(roots.clone(), refined);
    let split_faces = |faces: &[CubeFace]| {
        roots
            .iter()
            .flat_map(|r| {
                if faces.contains(&r.face()) {
                    r.children().unwrap().to_vec()
                } else {
                    vec![*r]
                }
            })
            .collect::<Vec<_>>()
    };
    let one = split_faces(&[CubeFace::PositiveX]);
    let two = split_faces(&[CubeFace::PositiveX, CubeFace::PositiveY]);
    check_transition(one.clone(), two.clone());
    check_transition(two, one);
}

#[test]
fn projected_displacement_bound_is_view_dependent_and_near_plane_conservative() {
    let topology = SurfaceTopology::new();
    let roots: Vec<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    let mut split = Vec::new();
    for root in &roots {
        if root.face() == CubeFace::PositiveX {
            split.extend(root.children().unwrap());
        } else {
            split.push(*root);
        }
    }
    let (old_cover, old) = surface(&roots, &topology, 0.0);
    let (new_cover, new) = surface(&split, &topology, 2.0);
    let transition = SurfaceTransition::build(
        &old_cover,
        &old,
        &new_cover,
        &new,
        &topology,
        16 * 1024 * 1024,
    )
    .unwrap();
    let projection = mundaris_renderer::CelestialProjection::try_new(640, 480, 1.0, 0.1).unwrap();
    let identity = glam::DQuat::IDENTITY;
    let near_view = transition.projected_displacement_pixels(
        projection,
        DVec3::new(1012.1, 0.0, 0.0),
        glam::DQuat::from_rotation_y(-std::f64::consts::FRAC_PI_2),
    );
    assert!(near_view.is_infinite());
    let far =
        transition.projected_displacement_pixels(projection, DVec3::new(0.0, 0.0, 1.0e7), identity);
    let closer = transition.projected_displacement_pixels(
        projection,
        DVec3::new(0.0, 0.0, 2000.0),
        identity,
    );
    assert!(far.is_finite() && closer.is_finite());
    assert!(closer > far);
    for (observer, rotation) in [
        (DVec3::Z * 2000.0, identity),
        (DVec3::Z * 2000.0, glam::DQuat::from_rotation_y(0.25)),
        (
            DVec3::new(1800.0, 100.0, 400.0),
            glam::DQuat::from_rotation_y(-1.2),
        ),
    ] {
        let bound = transition.projected_displacement_pixels(projection, observer, rotation);
        assert!(bound.is_finite());
        for triangle in transition.triangles() {
            for weights in [DVec3::X, DVec3::Y, DVec3::Z, DVec3::splat(1.0 / 3.0)] {
                let old = triangle
                    .iter()
                    .zip(weights.to_array())
                    .fold(DVec3::ZERO, |sum, (v, w)| sum + w * v.old.position_body_m);
                let new = triangle
                    .iter()
                    .zip(weights.to_array())
                    .fold(DVec3::ZERO, |sum, (v, w)| sum + w * v.new.position_body_m);
                let start = rotation * (old - observer);
                for t in [0.0, 0.1, 0.5, 0.9, 1.0] {
                    let point = rotation * (old.lerp(new, t) - observer);
                    if projection
                        .frustum_planes()
                        .iter()
                        .any(|(n, o)| n.dot(point) + o < 0.0)
                    {
                        continue;
                    }
                    let focal = projection.focal_pixels();
                    let delta = glam::DVec2::new(
                        focal * point.x / -point.z - focal * start.x / -start.z,
                        focal * point.y / -point.z - focal * start.y / -start.z,
                    )
                    .length();
                    assert!(
                        delta <= bound,
                        "sampled affine displacement {delta} exceeds {bound}"
                    );
                }
            }
        }
    }
}
