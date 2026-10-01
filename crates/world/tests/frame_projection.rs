use glam::DVec3;
use mundaris_math::*;
use mundaris_world::*;
use std::num::NonZeroU64;

fn ns(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}
fn state(x: f64, angle: f64) -> BodyState {
    BodyState::new(
        LocalPosition::try_metres(DVec3::new(x, 2.0, 3.0)).unwrap(),
        LinearVelocity3::try_metres_per_second(DVec3::new(0.0, 30_000.0, 0.0)).unwrap(),
        UnitRotation::from_axis_angle(
            Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap(),
            angle,
        )
        .unwrap(),
        AngularVelocity3::try_radians_per_second(DVec3::new(0.1, 0.2, 0.3)).unwrap(),
    )
}
fn insert(world: &mut CelestialSystem, x: f64, angle: f64) -> BodyId {
    world
        .insert_body(
            "body",
            BodyProperties::new(1.0, 1.0).unwrap(),
            state(x, angle),
        )
        .unwrap()
}

#[test]
fn topology_exact_values_extreme_positions_and_rebuild() {
    let mut world = CelestialSystem::new(
        ns(1),
        SimulationInstant::try_seconds_since_epoch(-3.0).unwrap(),
    );
    let ids: Vec<_> = [0.0, 6.371e6, 1.5e11, 1e16]
        .into_iter()
        .map(|x| insert(&mut world, x, 0.7))
        .collect();
    let projection = CelestialFrameProjection::build(&world, ns(10)).unwrap();
    let rebuilt = CelestialFrameProjection::build(&world, ns(11)).unwrap();
    let evaluation = projection.tree().evaluate();
    assert_eq!(
        evaluation.sample_time_s(),
        world.sample_time().seconds_since_epoch()
    );
    assert_eq!(projection.represented_revision(), world.revision());
    for id in ids {
        let body = world.body(id).unwrap();
        let frames = projection.frames_for(id).unwrap();
        assert_eq!(
            evaluation.parent(frames.translating).unwrap(),
            Some(evaluation.root())
        );
        assert_eq!(
            evaluation.parent(frames.body_fixed).unwrap(),
            Some(frames.translating)
        );
        let anchor = *evaluation.state(frames.translating).unwrap();
        let fixed = *evaluation.state(frames.body_fixed).unwrap();
        assert_eq!(
            anchor.parent_from_local().translation().metres(),
            body.state().center_in_system().metres()
        );
        assert_eq!(
            anchor.parent_from_local().rotation(),
            UnitRotation::identity()
        );
        assert_eq!(
            anchor.motion().unwrap().origin_velocity_in_parent(),
            body.state().center_velocity_in_system()
        );
        assert_eq!(
            anchor.motion().unwrap().angular_velocity_in_parent(),
            AngularVelocity3::zero()
        );
        assert_eq!(
            fixed.parent_from_local().translation(),
            Displacement3::zero()
        );
        assert_eq!(
            fixed.parent_from_local().rotation(),
            body.state().body_to_system()
        );
        assert_eq!(
            fixed.motion().unwrap().origin_velocity_in_parent(),
            LinearVelocity3::zero()
        );
        assert_eq!(
            fixed.motion().unwrap().angular_velocity_in_parent(),
            body.state().angular_velocity_in_system()
        );
        let other = rebuilt.frames_for(id).unwrap();
        assert_ne!(frames, other);
        assert_eq!(
            evaluation.transform_to_root(frames.body_fixed).unwrap(),
            rebuilt
                .tree()
                .evaluate()
                .transform_to_root(other.body_fixed)
                .unwrap()
        );
    }
}

#[test]
fn independent_spin_properties_coherent_publish_and_live_append() {
    let mut world = CelestialSystem::new(ns(1), SimulationInstant::ZERO);
    let a = insert(&mut world, 1.5e11, 0.0);
    let b = insert(&mut world, 1.5e11 + 384e6, 0.2);
    let mut projection = CelestialFrameProjection::build(&world, ns(10)).unwrap();
    let af = projection.frames_for(a).unwrap();
    let bf = projection.frames_for(b).unwrap();
    let before_a = projection
        .tree()
        .evaluate()
        .transform_to_root(af.translating)
        .unwrap();
    let before_b = projection
        .tree()
        .evaluate()
        .transform_to_root(bf.body_fixed)
        .unwrap();
    let point = FramePosition::new(
        bf.body_fixed,
        LocalPosition::try_metres(DVec3::new(10.0, 20.0, 30.0)).unwrap(),
    );
    let before_point = projection
        .tree()
        .evaluate()
        .convert_position(point, projection.tree().root())
        .unwrap();
    world.edit_state(a, state(1.5e11, 1.2)).unwrap();
    projection.publish(&world).unwrap();
    assert_eq!(
        projection
            .tree()
            .evaluate()
            .transform_to_root(af.translating)
            .unwrap(),
        before_a
    );
    assert_eq!(
        projection
            .tree()
            .evaluate()
            .transform_to_root(bf.body_fixed)
            .unwrap(),
        before_b
    );
    assert_eq!(
        projection
            .tree()
            .evaluate()
            .convert_position(point, projection.tree().root())
            .unwrap(),
        before_point
    );
    assert_eq!(
        projection
            .tree()
            .evaluate()
            .state(af.body_fixed)
            .unwrap()
            .parent_from_local()
            .rotation(),
        state(1.5e11, 1.2).body_to_system()
    );
    world
        .edit_body(b, "new name", BodyProperties::new(9.0, 8.0).unwrap())
        .unwrap();
    projection.publish(&world).unwrap();
    assert_eq!(
        projection
            .tree()
            .evaluate()
            .transform_to_root(bf.body_fixed)
            .unwrap(),
        before_b
    );
    let c = insert(&mut world, 0.0, 0.0);
    assert!(projection.frames_for(c).is_err());
    projection.publish(&world).unwrap();
    assert_eq!(projection.frames_for(a).unwrap(), af);
    assert_eq!(projection.frames_for(b).unwrap(), bf);
    assert_eq!(
        projection
            .tree()
            .evaluate()
            .depth(projection.frames_for(c).unwrap().body_fixed)
            .unwrap(),
        2
    );
    let updates: Vec<_> = world
        .bodies()
        .map(|(id, _)| BodyStateUpdate {
            body: id,
            state: state(10.0, -0.7),
        })
        .collect();
    world
        .update_states(
            SimulationInstant::try_seconds_since_epoch(-50.0).unwrap(),
            &updates,
        )
        .unwrap();
    projection.publish(&world).unwrap();
    assert_eq!(projection.tree().evaluate().sample_time_s(), -50.0);
    assert_eq!(projection.represented_revision(), world.revision());
    let represented = projection.represented_revision();
    let before = projection
        .tree()
        .evaluate()
        .transform_to_root(af.body_fixed)
        .unwrap();
    let wrong = CelestialSystem::new(ns(2), SimulationInstant::ZERO);
    assert_eq!(
        projection.publish(&wrong),
        Err(FrameProjectionError::FrameProjectionMismatch)
    );
    assert_eq!(projection.represented_revision(), represented);
    assert_eq!(
        projection
            .tree()
            .evaluate()
            .transform_to_root(af.body_fixed)
            .unwrap(),
        before
    );
    assert_eq!(world.sample_time().seconds_since_epoch(), -50.0);
}
