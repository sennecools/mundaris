//! Real filtered fields complement the renderer's analytic overlay fixtures.
use glam::DVec3;
use mundaris_app::planet_terrain::*;
use mundaris_math::{surface::*, *};
use mundaris_renderer::planet_surface::*;
use mundaris_world::{terrain::*, *};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
};

fn refine(cover: &mut BTreeSet<CubePatchAddress>, address: CubePatchAddress) {
    assert!(cover.remove(&address));
    cover.extend(address.children().unwrap());
}
fn balance(cover: &mut BTreeSet<CubePatchAddress>) {
    loop {
        let mut split = BTreeSet::new();
        for address in cover.iter() {
            for edge in PatchEdge::ALL {
                let mut neighbor = Some(address.neighbor(edge).address);
                while let Some(n) = neighbor {
                    if cover.contains(&n) {
                        if address.level() > n.level() + 1 {
                            split.insert(n);
                        }
                        break;
                    }
                    neighbor = n.parent();
                }
            }
        }
        if split.is_empty() {
            break;
        }
        for address in split {
            refine(cover, address);
        }
    }
}
fn generate(
    cache: &mut TerrainPatchCache,
    identity: &TerrainGeometryIdentity,
    addresses: &BTreeSet<CubePatchAddress>,
    topology: &SurfaceTopology,
) -> (Vec<ActiveSurfacePatch>, StitchedSurface) {
    for &address in addresses {
        assert!(cache.request(identity, address));
        while cache.peek(identity, address).is_none() {
            assert!(
                cache
                    .generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)
                    .unwrap()
                    .vertices_generated
                    > 0
            );
        }
    }
    let addresses = addresses.iter().copied().collect::<Vec<_>>();
    let cover = active_surface_cover(&addresses, topology).unwrap();
    let geometry = addresses
        .iter()
        .map(|&a| cache.peek(identity, a).unwrap())
        .collect::<Vec<_>>();
    let surface = StitchedSurface::build(&cover, &geometry, topology).unwrap();
    (cover, surface)
}

#[test]
fn seeded_v1_v2_filtered_corner_morphs_reproduce_endpoints_and_close() {
    for (case, (radius, level, seed, version)) in [
        (10_000.0, 7, 1, TerrainGeneratorVersion::V1),
        (10_000.0, 7, 17, TerrainGeneratorVersion::V2),
        (10_000.0, 7, 0x8b37_41f1, TerrainGeneratorVersion::V2),
        (6_371_000.0, 13, 113, TerrainGeneratorVersion::V2),
    ]
    .into_iter()
    .enumerate()
    {
        let mut world = CelestialSystem::new(
            NonZeroU64::new(90 + case as u64).unwrap(),
            SimulationInstant::ZERO,
        );
        let body = world
            .insert_body(
                "real transition fixture",
                BodyProperties::new(1.0, radius).unwrap(),
                BodyState::new(
                    LocalPosition::origin(),
                    LinearVelocity3::zero(),
                    UnitRotation::identity(),
                    AngularVelocity3::zero(),
                ),
            )
            .unwrap();
        let preset = checkpoint_terrain_definition_version(radius, version).unwrap();
        let definition = TerrainDefinition::new(
            preset.identity(),
            TerrainSeed(seed),
            version,
            preset.config().clone(),
        );
        world.edit_terrain(body, Some(definition.clone())).unwrap();
        let identity = TerrainGeometryIdentity::new(
            body,
            definition,
            world.body(body).unwrap().terrain_revision(),
            radius,
        )
        .unwrap();
        let target = CubePatchAddress::try_new(
            CubeFace::PositiveZ,
            level,
            (1u32 << level) - 1,
            (1u32 << level) - 1,
        )
        .unwrap();
        let mut old_addresses = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect::<BTreeSet<_>>();
        while !old_addresses.contains(&target) {
            let leaf = *old_addresses.iter().find(|a| a.contains(target)).unwrap();
            refine(&mut old_addresses, leaf);
        }
        balance(&mut old_addresses);
        let mut new_addresses = old_addresses.clone();
        refine(&mut new_addresses, target);
        balance(&mut new_addresses);
        let topology = SurfaceTopology::new();
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, MAX_TERRAIN_PATCHES).unwrap();
        let (old_cover, old) = generate(&mut cache, &identity, &old_addresses, &topology);
        let (new_cover, new) = generate(&mut cache, &identity, &new_addresses, &topology);
        let transition = SurfaceTransition::build(
            &old_cover,
            &old,
            &new_cover,
            &new,
            &topology,
            16 * 1024 * 1024,
        )
        .unwrap();
        assert!(transition.resident_bytes() <= 16 * 1024 * 1024);
        let tolerance = (256.0 * f64::EPSILON * radius).max(1e-9);
        for triangle in transition.triangles() {
            for vertex in triangle {
                for (surface, reference, sample) in [
                    (&old, vertex.old_reference, vertex.old),
                    (&new, vertex.new_reference, vertex.new),
                ] {
                    let patch = &surface.patches()[surface
                        .patches()
                        .binary_search_by_key(&reference.address, GeneratedSurfacePatch::address)
                        .unwrap()];
                    let samples = reference.indices.map(|i| patch.samples()[usize::from(i)]);
                    let position = samples
                        .iter()
                        .zip(reference.weights)
                        .map(|(s, w)| s.position_body_m * w)
                        .sum::<DVec3>();
                    let normal = samples
                        .iter()
                        .zip(reference.weights)
                        .map(|(s, w)| s.normal_body * w)
                        .sum::<DVec3>();
                    assert!(
                        (position - sample.position_body_m).length() <= tolerance,
                        "case={case}"
                    );
                    assert!(
                        (normal - sample.normal_body).length() <= 1e-9,
                        "case={case}"
                    );
                }
                for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
                    let sample = vertex.sample(fraction).unwrap();
                    assert!(sample.position_body_m.dot(sample.normal_body) > 0.0);
                }
            }
        }
        let mut edges = BTreeMap::<([i64; 3], [i64; 3]), (usize, i32)>::new();
        let mut append = |points: [DVec3; 3]| {
            assert!(
                (points[1] - points[0])
                    .cross(points[2] - points[0])
                    .dot(points[0] + points[1] + points[2])
                    > 0.0
            );
            let keys = points.map(|p| p.to_array().map(|x| (x * 1e7).round() as i64));
            for (a, b) in [(keys[0], keys[1]), (keys[1], keys[2]), (keys[2], keys[0])] {
                assert_ne!(a, b);
                let (key, sign) = if a < b { ((a, b), 1) } else { ((b, a), -1) };
                let entry = edges.entry(key).or_default();
                entry.0 += 1;
                entry.1 += sign;
            }
        };
        for (patch, geometry) in old_cover.iter().zip(old.patches()) {
            if transition
                .affected_old()
                .binary_search(&patch.address)
                .is_ok()
            {
                continue;
            }
            for triangle in topology.indices(patch.stitch_mask).as_chunks::<3>().0 {
                append(triangle.map(|i| geometry.samples()[usize::from(i)].position_body_m));
            }
        }
        for triangle in transition.triangles() {
            append(triangle.map(|v| v.sample(0.5).unwrap().position_body_m));
        }
        assert!(
            edges.values().all(|&incidence| incidence == (2, 0)),
            "case={case}: real-field intermediate cover is open"
        );
    }
}
