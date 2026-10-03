use mundaris_math::surface::{CubeFace, CubePatchAddress, PatchEdge};
use mundaris_renderer::planet_surface::{
    GRID_SAMPLES, GeneratedSurfacePatch, StitchedSurface, SurfaceErrorContributions, SurfaceExtent,
    SurfaceGeometrySample, SurfaceTopology, active_surface_cover,
};

fn fixture(address: CubePatchAddress) -> GeneratedSurfacePatch {
    let samples = (0..GRID_SAMPLES)
        .map(|index| {
            let direction = address
                .sample_direction((index % 17) as u32, (index / 17) as u32, 16)
                .unwrap()
                .unit();
            // Different footprint profiles deliberately disagree at equal directions.
            let height = 12.0 * direction.x - 7.0 * direction.y
                + 5.0 * direction.z
                + address.level() as f64 * 3.0;
            let gradient = glam::DVec3::new(12.0, -7.0, 5.0);
            let normal =
                (direction - (gradient - direction * gradient.dot(direction)) / 1000.0).normalize();
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
            min_height_m: -30.0,
            max_height_m: 30.0,
            guaranteed_opaque_radius_m: 0.0,
        },
        SurfaceErrorContributions::default(),
    )
    .unwrap()
}

#[test]
fn full_root_cover_reconciles_shared_edges_deterministically() {
    let topology = SurfaceTopology::new();
    let addresses: Vec<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    let active = active_surface_cover(&addresses, &topology).unwrap();
    let geometry: Vec<_> = addresses.iter().map(|&address| fixture(address)).collect();
    let refs: Vec<_> = geometry.iter().collect();
    let stitched = StitchedSurface::build(&active, &refs, &topology).unwrap();
    assert_eq!(stitched.patches().len(), 6);
    assert!(
        stitched.resident_bytes()
            >= 6 * GRID_SAMPLES * std::mem::size_of::<SurfaceGeometrySample>()
    );
    for left in stitched.patches() {
        for right in stitched.patches() {
            if left.address() >= right.address() {
                continue;
            }
            for (i, a) in left.samples().iter().enumerate() {
                let key = left
                    .address()
                    .sample_key((i % 17) as u32, (i / 17) as u32, 16)
                    .unwrap();
                if let Some(j) = (0..GRID_SAMPLES).find(|&j| {
                    right
                        .address()
                        .sample_key((j % 17) as u32, (j / 17) as u32, 16)
                        .unwrap()
                        == key
                }) {
                    let b = right.samples()[j];
                    assert_eq!(
                        a.position_body_m.to_array().map(f64::to_bits),
                        b.position_body_m.to_array().map(f64::to_bits)
                    );
                    assert_eq!(
                        a.normal_body.to_array().map(f64::to_bits),
                        b.normal_body.to_array().map(f64::to_bits)
                    );
                }
            }
        }
    }
}

fn exercise_cover(addresses: Vec<CubePatchAddress>) {
    let topology = SurfaceTopology::new();
    let active = active_surface_cover(&addresses, &topology).unwrap();
    let geometry: Vec<_> = addresses.iter().copied().map(fixture).collect();
    let refs: Vec<_> = geometry.iter().collect();
    let stitched = StitchedSurface::build(&active, &refs, &topology).unwrap();
    let mut canonical = std::collections::BTreeMap::new();
    let mut incidence = std::collections::BTreeMap::<(u128, u128), (usize, i32)>::new();
    for patch in stitched.patches() {
        for (index, sample) in patch.samples().iter().enumerate() {
            let (i, j) = (index % 17, index / 17);
            let key = patch
                .address()
                .sample_key(i as u32, j as u32, 16)
                .unwrap()
                .compact_key();
            if i == 0 || i == 16 || j == 0 || j == 16 {
                let bits = (
                    sample.position_body_m.to_array().map(f64::to_bits),
                    sample.normal_body.to_array().map(f64::to_bits),
                );
                if let Some(previous) = canonical.insert(key, bits) {
                    assert_eq!(previous, bits);
                }
            }
        }
        for &index in topology.indices(
            active
                .iter()
                .find(|a| a.address == patch.address())
                .unwrap()
                .stitch_mask,
        ) {
            assert!((index as usize) < GRID_SAMPLES);
        }
        let mask = active
            .iter()
            .find(|a| a.address == patch.address())
            .unwrap()
            .stitch_mask;
        for triangle in topology.indices(mask).as_chunks::<3>().0 {
            let keys = triangle.map(|k| {
                patch
                    .address()
                    .sample_key(u32::from(k % 17), u32::from(k / 17), 16)
                    .unwrap()
                    .compact_key()
            });
            for (a, b) in [(keys[0], keys[1]), (keys[1], keys[2]), (keys[2], keys[0])] {
                let (edge, orientation) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                let entry = incidence.entry(edge).or_default();
                entry.0 += 1;
                entry.1 += orientation;
            }
        }
        for edge in PatchEdge::ALL {
            if mask & edge.bit() == 0 {
                continue;
            }
            for along in (1u32..16).step_by(2) {
                let [i, j] = edge.grid(along, 16);
                let index = (j * 17 + i) as u16;
                assert!(
                    !topology.indices(mask).contains(&index),
                    "collapsed odd vertex referenced: {:?} {edge:?} {along}",
                    patch.address()
                );
            }
        }
    }
    assert!(
        incidence
            .values()
            .all(|&(count, orientation)| count == 2 && orientation == 0),
        "complete stitched cube must be an oriented closed two-manifold"
    );
}

#[test]
fn balanced_mixed_covers_reconcile_boundaries_and_stitch_indices() {
    let mut seed = 0x9e37_79b9_u64;
    for round in 0..96 {
        let mut addresses = Vec::new();
        for face in CubeFace::ALL {
            for y in 0..2 {
                for x in 0..2 {
                    let parent = CubePatchAddress::try_new(face, 1, x, y).unwrap();
                    seed ^= seed << 7;
                    seed ^= seed >> 9;
                    seed ^= seed << 8;
                    if seed & 3 == 0 {
                        addresses.extend(parent.children().unwrap());
                    } else {
                        addresses.push(parent);
                    }
                }
            }
        }
        // Sorting is part of the StitchedSurface contract.
        addresses.sort_unstable();
        assert!(addresses.len() > 24);
        exercise_cover(addresses);
        assert!(round < 96);
    }
}

#[test]
fn stitched_surface_rejects_incomplete_overlapping_unbalanced_and_unsorted_covers() {
    let topology = SurfaceTopology::new();
    let mut roots: Vec<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    let attempt = |addresses: &[CubePatchAddress]| {
        let active = active_surface_cover(addresses, &topology);
        let geometry: Vec<_> = addresses.iter().copied().map(fixture).collect();
        let refs: Vec<_> = geometry.iter().collect();
        active.and_then(|active| StitchedSurface::build(&active, &refs, &topology).map(|_| active))
    };
    roots.pop();
    assert!(attempt(&roots).is_err());
    roots.push(CubePatchAddress::root(CubeFace::NegativeZ));
    roots.sort_unstable();
    let mut overlap = roots.clone();
    overlap.extend(
        CubePatchAddress::root(CubeFace::PositiveX)
            .children()
            .unwrap(),
    );
    overlap.sort_unstable();
    assert!(attempt(&overlap).is_err());
    let mut unsorted = roots.clone();
    unsorted.reverse();
    let geometry: Vec<_> = unsorted.iter().copied().map(fixture).collect();
    let refs: Vec<_> = geometry.iter().collect();
    assert!(
        StitchedSurface::build(
            &active_surface_cover(&roots, &topology).unwrap(),
            &refs,
            &topology
        )
        .is_err()
    );
    let mut delta_two = Vec::new();
    for face in CubeFace::ALL {
        let root = CubePatchAddress::root(face);
        if face == CubeFace::PositiveX {
            let children = root.children().unwrap();
            delta_two.extend(children[0].children().unwrap());
            delta_two.extend(children.into_iter().skip(1));
        } else {
            delta_two.push(root);
        }
    }
    delta_two.sort_unstable();
    assert!(attempt(&delta_two).is_err());
}
