use glam::DVec3;
use mundaris_app::{celestial_selection::*, gravity_fixtures::*, orbit_guides::*, system_view::*};
use mundaris_math::*;
use mundaris_world::*;
use std::num::NonZeroU64;
#[test]
fn empty_single_local_and_outlier_policy() {
    let empty = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
    let bounds =
        SystemViewBounds::calculate(&empty, &OverviewScope::WholeSystem, &[], &[], false).unwrap();
    assert!(bounds.radius_m().is_finite());
    assert!(bounds.included().is_empty());
    let mut world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(2).unwrap())
        .unwrap();
    let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
    let mut guides = OrbitGuides::default();
    guides.update(&world);
    let moon = SystemViewBounds::calculate(
        &world,
        &OverviewScope::SelectedSubsystem(ids[2]),
        guides.guides(),
        &[],
        false,
    )
    .unwrap();
    assert_eq!(
        moon.radius_m(),
        world
            .body(ids[2])
            .unwrap()
            .properties()
            .reference_radius_m()
    );
    let planet = SystemViewBounds::calculate(
        &world,
        &OverviewScope::SelectedSubsystem(ids[1]),
        guides.guides(),
        &[],
        false,
    )
    .unwrap();
    assert_eq!(planet.included(), [ids[1], ids[2]]);
    assert!(planet.radius_m() < 2e8);
    let body = world
        .insert_body(
            "Outlier",
            BodyProperties::new(1.0, 1.0).unwrap(),
            BodyState::new(
                LocalPosition::try_metres(DVec3::X * 1e14).unwrap(),
                LinearVelocity3::zero(),
                UnitRotation::identity(),
                AngularVelocity3::zero(),
            ),
        )
        .unwrap();
    let full =
        SystemViewBounds::calculate(&world, &OverviewScope::WholeSystem, &[], &[], false).unwrap();
    assert!(full.included().contains(&body));
    assert!(full.radius_m() > 4e13);
    let core =
        SystemViewBounds::calculate(&world, &OverviewScope::ExplicitBodies(ids), &[], &[], false)
            .unwrap();
    assert!(core.radius_m() < 1e12);
    let mut selected = BodySelection::default();
    selected.select(&world, body).unwrap();
    let foreign = GravityFixture::Hierarchy
        .create(NonZeroU64::new(3).unwrap())
        .unwrap()
        .bodies()
        .next()
        .unwrap()
        .0;
    assert!(selected.select(&world, foreign).is_err());
    assert_eq!(selected.selected(), Some(body));
}
