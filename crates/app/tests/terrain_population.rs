use glam::{DQuat, DVec3};
use mundaris_app::{planet_surface::*, solar_system::*, terrain_population::TerrainPopulation};
use mundaris_math::*;
use mundaris_renderer::*;
use mundaris_world::terrain::*;
use mundaris_world::*;
use std::{num::NonZeroU64, time::Duration};

const NAMESPACE: u64 = 580;
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;

fn system() -> (
    CelestialSystem,
    CelestialFrameProjection,
    Vec<PlanetSurfaceSession>,
    Vec<CelestialRenderBody>,
) {
    let world = SolarSystemPreset::gameplay()
        .create(NonZeroU64::new(NAMESPACE).unwrap())
        .unwrap();
    let frames =
        CelestialFrameProjection::build(&world, NonZeroU64::new(NAMESPACE).unwrap()).unwrap();
    let mut sessions = Vec::new();
    let mut requests = Vec::new();
    for (id, body) in world.bodies() {
        let fixed = frames.frames_for(id).unwrap().body_fixed;
        let color = SOLAR_SYSTEM_CONTENT
            .iter()
            .find(|content| content.name == body.name())
            .unwrap()
            .color;
        requests.push(CelestialRenderBody {
            body_fixed_frame: fixed,
            reference_radius_m: body.properties().reference_radius_m(),
            color,
            unlit: false,
            selected: false,
        });
        if body.has_surface() {
            sessions.push(PlanetSurfaceSession::new(id, 4096).unwrap());
        }
    }
    (world, frames, sessions, requests)
}

fn observer<'a>(
    pair: &CoherentCelestialView<'a>,
    frame: FrameId,
    local: DVec3,
) -> PreparedView<'a> {
    PreparedView::new(
        &pair.evaluation(),
        FramePose::new(
            FramePosition::new(frame, LocalPosition::try_metres(local).unwrap()),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn population_update(
    population: &mut TerrainPopulation,
    world: &CelestialSystem,
    frames: &CelestialFrameProjection,
    sessions: &mut [PlanetSurfaceSession],
    requests: &[CelestialRenderBody],
    target: SolarBody,
    eye: DVec3,
    vertex_budget: usize,
) -> (Vec<bool>, mundaris_world::BodyId) {
    let pair = frames.coherent_view(world).unwrap();
    let index = SOLAR_SYSTEM_CONTENT
        .iter()
        .position(|body| body.identity == target)
        .unwrap();
    let view = observer(&pair, requests[index].body_fixed_frame, eye);
    let projection =
        CelestialProjection::try_new(WIDTH, HEIGHT, 60.0_f64.to_radians(), 0.1).unwrap();
    let mut owners = vec![false; requests.len()];
    let sphere = Icosphere::new();
    population
        .update(
            &pair,
            &view,
            projection,
            requests,
            sessions,
            &mut owners,
            &sphere,
            true,
            Duration::ZERO,
            vertex_budget,
            None,
            Duration::from_millis(16),
        )
        .unwrap();
    (owners, pair.system().bodies().nth(index).unwrap().0)
}

#[test]
fn distant_system_is_far_only_without_generation_or_queued_work() {
    let (world, frames, mut sessions, requests) = system();
    let mut population = TerrainPopulation::new().unwrap();
    let (owners, _) = population_update(
        &mut population,
        &world,
        &frames,
        &mut sessions,
        &requests,
        SolarBody::Earth,
        DVec3::new(0.0, 0.0, 6.0e11),
        1156,
    );
    assert_eq!(population.active_body(), None);
    assert!(owners.iter().all(|owner| !owner));
    assert_eq!(population.work.vertices_generated, 0);
    assert_eq!(population.cache.pending(), 0);
}

#[test]
fn near_earth_roots_exclusively_own_surface_and_teleports_reuse_unpinned_cache() {
    let (world, frames, mut sessions, requests) = system();
    let mut population = TerrainPopulation::new().unwrap();
    let earth_index = SolarBody::Earth as usize;
    let earth_radius = requests[earth_index].reference_radius_m;
    let (earth_owners, earth_id) = population_update(
        &mut population,
        &world,
        &frames,
        &mut sessions,
        &requests,
        SolarBody::Earth,
        DVec3::Z * (earth_radius + 600_000.0),
        1734,
    );
    assert_eq!(
        population.active_body(),
        Some(earth_id),
        "active={:?}, errors={:?}",
        population.active_body(),
        sessions
            .iter()
            .map(|session| (session.body(), session.far_error_pixels))
            .collect::<Vec<_>>()
    );
    assert_eq!(earth_owners.iter().filter(|owner| **owner).count(), 1);
    assert!(
        population.cover.ready(),
        "six terrain roots should become ready"
    );
    assert_eq!(population.cover.active().len(), 6);
    let resident_before = population.cache.report().resident_patches;
    assert!(population.cache.report().pinned_patches >= 6);

    for target in [SolarBody::Moon, SolarBody::Mars, SolarBody::Earth] {
        let index = target as usize;
        let radius = requests[index].reference_radius_m;
        let (owners, active) = population_update(
            &mut population,
            &world,
            &frames,
            &mut sessions,
            &requests,
            target,
            DVec3::Z * (radius + 600_000.0),
            if target == SolarBody::Earth { 0 } else { 1734 },
        );
        assert_eq!(active, world.bodies().nth(index).unwrap().0);
        assert!(owners.iter().filter(|owner| **owner).count() <= 1);
        assert_eq!(population.active_body(), Some(active));
        assert!(population.cover.ready());
        for (id, _) in world.bodies().filter(|(id, _)| *id != active) {
            assert_eq!(
                population.cache.pending_for_body(id),
                0,
                "inactive work must be cancelled"
            );
        }
        for session in &sessions {
            if session.body() != active {
                assert_eq!(session.state(), SurfaceRepresentationState::Far);
            }
        }
        assert!(
            population.cache.report().pinned_patches <= 6,
            "only active-body root dependencies may be pinned"
        );
    }
    assert!(
        population.cache.report().resident_patches >= resident_before,
        "unpinned terrain geometry should remain reusable"
    );
    assert_eq!(population.active_body(), Some(earth_id));
    assert_eq!(
        population.work.vertices_generated, 0,
        "returning to Earth should reuse its retained root geometry"
    );
    let (owners, _) = population_update(
        &mut population,
        &world,
        &frames,
        &mut sessions,
        &requests,
        SolarBody::Earth,
        DVec3::Z * 6.0e11,
        1156,
    );
    assert!(owners.iter().all(|&owner| !owner));
    assert_eq!(population.active_body(), None);
    assert_eq!(population.cache.pending(), 0);
    assert_eq!(population.cache.report().pinned_patches, 0);
    assert_eq!(population.work.vertices_generated, 0);
    assert!(!population.cover.ready());
}

#[test]
fn surface_binding_refreshes_after_definition_and_radius_changes() {
    let (mut world, mut frames, mut sessions, mut requests) = system();
    let mut population = TerrainPopulation::new().unwrap();
    let index = SolarBody::Moon as usize;
    let moon = world.bodies().nth(index).unwrap().0;
    let mut radius = requests[index].reference_radius_m;

    let (owners, active) = population_update(
        &mut population,
        &world,
        &frames,
        &mut sessions,
        &requests,
        SolarBody::Moon,
        DVec3::Z * (radius + 600_000.0),
        1734,
    );
    assert_eq!(active, moon);
    assert!(owners[index]);
    assert!(population.work.vertices_generated > 0);

    let definition = SurfaceDefinition::generated(
        TerrainIdentity(0x4d4f4f4e),
        TerrainSeed(0x4d4f4f4e),
        SurfaceAlgorithm::RockyV5,
    );
    world
        .edit_surface_definition(moon, Some(definition))
        .unwrap();
    let (owners, active) = population_update(
        &mut population,
        &world,
        &frames,
        &mut sessions,
        &requests,
        SolarBody::Moon,
        DVec3::Z * (radius + 600_000.0),
        1734,
    );
    assert_eq!(active, moon);
    assert!(owners[index]);
    assert!(population.work.vertices_generated > 0);

    radius += 10_000.0;
    let body = world.body(moon).unwrap();
    world
        .edit_properties(
            moon,
            BodyProperties::new(body.properties().mass_kg(), radius).unwrap(),
        )
        .unwrap();
    frames.publish(&world).unwrap();
    requests[index].reference_radius_m = radius;
    let (owners, active) = population_update(
        &mut population,
        &world,
        &frames,
        &mut sessions,
        &requests,
        SolarBody::Moon,
        DVec3::Z * (radius + 600_000.0),
        1734,
    );
    assert_eq!(active, moon);
    assert!(owners[index]);
    assert_eq!(population.work.vertices_generated, 1734);
}

#[test]
fn precision_marker_keeps_ready_terrain_owner_until_far_representation_is_ready() {
    check_deferred_far_handoff(true);
}

#[test]
fn disabling_terrain_defers_handoff_until_far_representation_is_ready() {
    check_deferred_far_handoff(false);
}

fn check_deferred_far_handoff(terrain_enabled: bool) {
    let (world, frames, mut sessions, requests) = system();
    let mut population = TerrainPopulation::new().unwrap();
    let earth_index = SolarBody::Earth as usize;
    let earth = requests[earth_index];
    let earth_id = world.bodies().nth(earth_index).unwrap().0;
    let radius = earth.reference_radius_m;
    let projection =
        CelestialProjection::try_new(WIDTH, HEIGHT, 60.0_f64.to_radians(), 0.1).unwrap();
    let sphere = Icosphere::new();

    // Build and publish a complete ready surface at the near orbital observer.
    let pair = frames.coherent_view(&world).unwrap();
    let view = observer(
        &pair,
        earth.body_fixed_frame,
        DVec3::Z * (radius + 600_000.0),
    );
    let mut owners = vec![false; requests.len()];
    population
        .update(
            &pair,
            &view,
            projection,
            &requests,
            &mut sessions,
            &mut owners,
            &sphere,
            true,
            Duration::ZERO,
            1734,
            None,
            Duration::from_millis(16),
        )
        .unwrap();
    assert_eq!(population.active_body(), Some(earth_id));
    assert!(population.cover.ready());
    assert!(owners[earth_index]);
    let storage = population.cover.surface().unwrap() as *const _;

    // Turn the camera sideways. The large far sphere crosses the near plane and
    // cannot be prepared, regardless of the displaced cover's visible patches.
    let pair = frames.coherent_view(&world).unwrap();
    let orientation =
        UnitRotation::try_from_quaternion(DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2))
            .unwrap();
    let view = PreparedView::new(
        &pair.evaluation(),
        FramePose::new(
            FramePosition::new(
                earth.body_fixed_frame,
                LocalPosition::try_metres(DVec3::Z * (radius + 2_000.0)).unwrap(),
            ),
            orientation,
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    let mut far = CelestialStaging::default();
    let mut probe = CelestialFrame::new(&view, &mut far, projection, &sphere);
    probe.append_bodies(&[earth]).unwrap();
    assert_eq!(
        probe.markers()[0].representation,
        SphereRepresentation::PrecisionMarker
    );

    let mut owners = vec![false; requests.len()];
    population
        .update(
            &pair,
            &view,
            projection,
            &requests,
            &mut sessions,
            &mut owners,
            &sphere,
            terrain_enabled,
            Duration::ZERO,
            0,
            None,
            Duration::from_millis(16),
        )
        .unwrap();
    assert_eq!(population.active_body(), Some(earth_id));
    assert!(population.cover.ready());
    assert_eq!(population.cover.surface().unwrap() as *const _, storage);
    assert!(
        owners[earth_index],
        "ready surface remains the exclusive owner"
    );
    let mut observations = CelestialStaging::default();
    let mut frame = CelestialFrame::new(&view, &mut observations, projection, &sphere);
    frame
        .append_body_observations(&[earth], &[owners[earth_index]])
        .unwrap();
    assert_eq!(
        frame.markers()[0].representation,
        SphereRepresentation::Surface
    );
    assert_eq!(
        frame.report().triangles,
        0,
        "Earth's far sphere is suppressed"
    );

    // Once a far representation is ready, coverage is relinquished.
    let pair = frames.coherent_view(&world).unwrap();
    let view = observer(&pair, earth.body_fixed_frame, DVec3::Z * 6.0e11);
    let mut owners = vec![false; requests.len()];
    population
        .update(
            &pair,
            &view,
            projection,
            &requests,
            &mut sessions,
            &mut owners,
            &sphere,
            terrain_enabled,
            Duration::ZERO,
            0,
            None,
            Duration::from_millis(16),
        )
        .unwrap();
    assert_eq!(population.active_body(), None);
    assert!(owners.iter().all(|&owner| !owner));
}

#[test]
fn body_fixed_precision_retains_centimetre_delta_at_astronomical_centres() {
    for offset in [0.0, 1e16] {
        let (mut world, _, _, _) = system();
        let updates: Vec<_> = world
            .bodies()
            .map(|(body, b)| BodyStateUpdate {
                body,
                state: BodyState::new(
                    LocalPosition::try_metres(
                        b.state().center_in_system().metres() + DVec3::splat(offset),
                    )
                    .unwrap(),
                    b.state().center_velocity_in_system(),
                    b.state().body_to_system(),
                    b.state().angular_velocity_in_system(),
                ),
            })
            .collect();
        world
            .update_states(SimulationInstant::ZERO, &updates)
            .unwrap();
        let frames =
            CelestialFrameProjection::build(&world, NonZeroU64::new(581).unwrap()).unwrap();
        let pair = frames.coherent_view(&world).unwrap();
        for target in [
            SolarBody::Earth,
            SolarBody::Moon,
            SolarBody::Mars,
            SolarBody::Neptune,
        ] {
            let (id, body) = world.bodies().nth(target as usize).unwrap();
            let fixed = frames.frames_for(id).unwrap().body_fixed;
            let eye = DVec3::Z * (body.properties().reference_radius_m() + 20.0);
            let view = observer(&pair, fixed, eye);
            let source = view.prepare_source(fixed).unwrap();
            let displacement = source
                .view_displacement(FramePosition::new(
                    fixed,
                    LocalPosition::try_metres(eye + DVec3::X * 0.01).unwrap(),
                ))
                .unwrap()
                .metres();
            assert!(
                (displacement - DVec3::X * 0.01).length() < 1e-12,
                "{target:?}, offset={offset}"
            );
        }
    }
}
