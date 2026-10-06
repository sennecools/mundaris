use mundaris_app::{planet_terrain::*, solar_system::*};
use mundaris_math::surface::*;
use mundaris_renderer::planet_surface::*;
use mundaris_world::*;
use std::{
    num::NonZeroU64,
    thread,
    time::{Duration, Instant},
};

fn fixture() -> (CelestialSystem, Vec<TerrainGeometryIdentity>) {
    let world = SolarSystemPreset::gameplay()
        .create(NonZeroU64::new(714).unwrap())
        .unwrap();
    let identities = [SolarBody::Earth, SolarBody::Moon, SolarBody::Mars]
        .into_iter()
        .map(|target| {
            let (body, state) = world.bodies().nth(target as usize).unwrap();
            if state.surface_definition().is_some() {
                TerrainGeometryIdentity::from_body(body, state).unwrap()
            } else {
                TerrainGeometryIdentity::new(
                    body,
                    state.terrain().unwrap().clone(),
                    state.terrain_revision(),
                    state.properties().reference_radius_m(),
                )
                .unwrap()
            }
        })
        .collect();
    (world, identities)
}

fn address(level: u8) -> CubePatchAddress {
    let side = 1u32 << level;
    CubePatchAddress::try_new(CubeFace::PositiveZ, level, side / 2, side / 2).unwrap()
}

fn ready(
    cache: &mut TerrainPatchCache,
    id: &TerrainGeometryIdentity,
    a: CubePatchAddress,
) -> TerrainWorkReport {
    assert!(cache.request(id, a));
    let deadline = Instant::now() + Duration::from_secs(10);
    while cache.peek(id, a).is_none() {
        let work = cache
            .generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)
            .unwrap();
        if cache.peek(id, a).is_some() {
            return work;
        }
        assert!(
            Instant::now() < deadline,
            "worker patch did not publish before deadline"
        );
        thread::sleep(Duration::from_millis(1));
    }
    unreachable!("ready loop returns on publication")
}

fn assert_same(a: &GeneratedSurfacePatch, b: &GeneratedSurfacePatch) {
    assert_eq!(a.footprint_m().to_bits(), b.footprint_m().to_bits());
    for (x, y) in [
        (a.extent().min_height_m, b.extent().min_height_m),
        (a.extent().max_height_m, b.extent().max_height_m),
        (
            a.extent().guaranteed_opaque_radius_m,
            b.extent().guaranteed_opaque_radius_m,
        ),
        (a.error().sphere_m, b.error().sphere_m),
        (
            a.error().filtered_interpolation_m,
            b.error().filtered_interpolation_m,
        ),
        (a.error().unresolved_m, b.error().unresolved_m),
        (
            a.error().boundary_constraint_m,
            b.error().boundary_constraint_m,
        ),
        (a.error().morph_remaining_m, b.error().morph_remaining_m),
        (a.error().numeric_m, b.error().numeric_m),
    ] {
        assert_eq!(x.to_bits(), y.to_bits());
    }
    for (left, right) in a.samples().iter().zip(b.samples()) {
        for (x, y) in left
            .position_body_m
            .to_array()
            .into_iter()
            .zip(right.position_body_m.to_array())
        {
            assert_eq!(x.to_bits(), y.to_bits());
        }
        for (x, y) in left
            .normal_body
            .to_array()
            .into_iter()
            .zip(right.normal_body.to_array())
        {
            assert_eq!(x.to_bits(), y.to_bits());
        }
    }
}

#[test]
fn workers_match_serial_patch_bits_across_bodies_and_scales() {
    let (_, identities) = fixture();
    for id in &identities {
        for level in [0, 6, 18] {
            let a = address(level);
            let mut serial =
                TerrainPatchCache::new_with_workers(TERRAIN_CPU_CAP_BYTES, 2, 0).unwrap();
            ready(&mut serial, id, a);
            for count in [1, 2, 4] {
                let mut workers =
                    TerrainPatchCache::new_with_workers(TERRAIN_CPU_CAP_BYTES, 2, count).unwrap();
                let work = ready(&mut workers, id, a);
                assert_same(serial.peek(id, a).unwrap(), workers.peek(id, a).unwrap());
                assert_eq!(work.worker_samples_completed, GRID_SAMPLES);
                assert!(work.worker_cpu > Duration::ZERO);
            }
        }
    }
}

#[test]
fn worker_cancellation_revision_switch_and_aggregate_reservations_stay_bounded() {
    let (mut world, identities) = fixture();
    let cap = 4 * 1024 * 1024;
    let mut cache = TerrainPatchCache::new_with_workers(cap, 64, 4).unwrap();
    let old = &identities[0];
    for level in 0..12 {
        assert!(cache.request(old, address(level)));
    }
    cache.generate(1, GENERATION_MICROBATCH, None).unwrap();
    thread::sleep(Duration::from_millis(1));
    world
        .edit_terrain(old.body, Some(old.definition.legacy().unwrap().clone()))
        .unwrap();
    let revised = TerrainGeometryIdentity::new(
        old.body,
        world.body(old.body).unwrap().terrain().unwrap().clone(),
        world.body(old.body).unwrap().terrain_revision(),
        old.radius_m,
    )
    .unwrap();
    cache.invalidate_body(&revised);
    assert!(
        cache.peek(old, address(0)).is_none(),
        "invalidation retained old revision"
    );
    cache.cancel_body_work(old.body);
    let deadline = Instant::now() + Duration::from_secs(10);
    while cache.pending_for_body(old.body) != 0 {
        cache.generate(1, GENERATION_MICROBATCH, None).unwrap();
        assert!(Instant::now() < deadline, "cancelled work did not drain");
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(cache.pending_for_body(old.body), 0);
    for id in [&identities[1], &identities[2], &revised] {
        assert!(cache.request(id, address(0)));
        cache.generate(1, GENERATION_MICROBATCH, None).unwrap();
        assert!(cache.pending() <= MAX_PENDING_PATCHES);
        assert!(cache.report().resident_bytes + cache.report().external_bytes <= cap);
        cache.reserve_external(1024);
        assert!(cache.report().resident_bytes + cache.report().external_bytes <= cap);
        cache.reserve_external(0);
        ready(&mut cache, id, address(0));
    }
    assert!(cache.report().worker_count <= 4);
    assert!(cache.report().cancellations > 0);
}
