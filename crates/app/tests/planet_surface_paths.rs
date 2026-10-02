use glam::{DQuat, DVec3};
use mundaris_app::gravity_fixtures::*;
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_world::*;
use std::num::NonZeroU64;

#[test]
fn fast_lateral_orbit_rapid_zoom_and_reversal_keep_complete_coverage() {
    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    let id = world.bodies().nth(1).unwrap().0;
    let source = frames.frames_for(id).unwrap().body_fixed;
    let radius = world.body(id).unwrap().properties().reference_radius_m();
    let before: Vec<_> = world.bodies().map(|(id, b)| (id, b.clone())).collect();
    let revision = world.revision();
    let mut session = SurfaceLodSession::new(2048).unwrap();
    for sample in 0..24 {
        let angle = sample as f64 * std::f64::consts::TAU / 24.0;
        let direction = DVec3::new(angle.sin(), 0.3 * (angle * 2.0).sin(), angle.cos()).normalize();
        let clearance = match sample % 6 {
            0 => 1e11,
            1 => 1e5,
            2 => 1e4,
            3 => 2.0,
            4 => 100.0,
            _ => 1e11,
        };
        let observer = FramePose::new(
            FramePosition::new(
                source,
                LocalPosition::try_metres(direction * (radius + clearance)).unwrap(),
            ),
            UnitRotation::try_from_quaternion(DQuat::from_rotation_arc(DVec3::Z, direction))
                .unwrap(),
        );
        let view = PreparedView::new(
            &frames.tree().evaluate(),
            observer,
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection =
            CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: source,
            reference_radius_m: radius,
            projection,
        };
        let mut report = LodReport::default();
        let mut churn = 0;
        for update in 0..200 {
            let settings = LodSettings::default()
                .with_work_limit(if update < 3 { 0 } else { 32 })
                .unwrap();
            report = session.update(&input, &settings).unwrap();
            churn += report.cache_evictions;
            let leaves: Vec<_> = session.covering_leaves().collect();
            for face in mundaris_math::surface::CubeFace::ALL {
                let area: u128 = leaves
                    .iter()
                    .filter(|p| p.face() == face)
                    .map(|p| 1u128 << (2 * (30 - p.level())))
                    .sum();
                assert_eq!(area, 1u128 << 60);
            }
            assert!(report.visible_patches <= 4096);
            assert!(report.cache_bytes <= 1024 * 1024);
            assert!(report.scratch_bytes <= 8 * 1024 * 1024);
            if report.settled {
                break;
            }
        }
        assert!(report.settled, "sample={sample} {report:?}");
        eprintln!(
            "path sample={sample} h={clearance} cover={} visible={} max_level={} error={} cache={} churn={}",
            report.active_patches,
            report.visible_patches,
            report.max_level,
            report.max_error_pixels,
            report.cache_records,
            churn
        );
    }
    assert_eq!(world.revision(), revision);
    assert_eq!(
        world
            .bodies()
            .map(|(id, b)| (id, b.clone()))
            .collect::<Vec<_>>(),
        before
    );
}
