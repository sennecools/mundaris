#![cfg(feature = "developer-tools")]
//! Developer camera commands must visibly move the observer in the canonical
//! paused scene, whose startup surface pose is a developer fixture pose.
use astrum_app::{
    GravityOrbitsDemo, celestial_camera::CameraMode, developer_protocol::DevCommand,
};
use std::time::Duration;

const FRAME: Duration = Duration::from_millis(16);

fn canonical() -> GravityOrbitsDemo {
    let demo = GravityOrbitsDemo::shared_test_system().unwrap();
    assert_eq!(demo.camera().mode(), CameraMode::SurfaceInspection);
    demo
}

fn run(demo: &mut GravityOrbitsDemo, seconds: f64) {
    for _ in 0..(seconds / FRAME.as_secs_f64()).ceil() as usize {
        demo.update(FRAME);
    }
}

#[test]
fn idle_paused_fixture_pose_stays_put() {
    let mut demo = canonical();
    demo.set_paused(true).unwrap();
    let before = demo.camera().pose();
    run(&mut demo, 1.0);
    assert_eq!(demo.camera().mode(), CameraMode::SurfaceInspection);
    assert_eq!(demo.camera().pose(), before);
}

#[test]
fn overview_completes_while_paused() {
    let mut demo = canonical();
    demo.set_paused(true).unwrap();
    let surface_distance = demo.camera().distance_m();
    demo.developer_apply_command(&DevCommand::Overview).unwrap();
    assert!(demo.camera().transitioning());
    run(&mut demo, 1.5);
    assert!(
        !demo.camera().transitioning(),
        "overview transition stalled"
    );
    assert_eq!(demo.camera().mode(), CameraMode::SystemOrbit);
    assert!(
        demo.camera().distance_m() > 1e3 * surface_distance,
        "overview distance {} m",
        demo.camera().distance_m()
    );
}

#[test]
fn body_orbit_clearance_recedes_while_paused() {
    let mut demo = canonical();
    demo.set_paused(true).unwrap();
    let start = demo.camera().clearance_m();
    demo.developer_apply_command(&DevCommand::NavigationMode {
        mode: "body_orbit".into(),
    })
    .unwrap();
    assert_eq!(demo.camera().mode(), CameraMode::BodyOrbit);
    demo.developer_apply_command(&DevCommand::Clearance { meters: 4.0e7 })
        .unwrap();
    run(&mut demo, 3.0);
    let clearance = demo.camera().clearance_m();
    assert!(start < 1e5, "canonical start clearance {start} m");
    assert!(
        (clearance - 4.0e7).abs() < 1e-3 * 4.0e7,
        "body orbit clearance {clearance} m did not reach 40000 km"
    );
}

type CameraAction = fn(&mut GravityOrbitsDemo);

fn pose_position_m(demo: &GravityOrbitsDemo) -> [f64; 3] {
    demo.camera().pose().position().local().metres().to_array()
}

#[test]
fn every_camera_action_moves_the_pose_while_paused() {
    // Each action starts from a fresh canonical (paused, fixture-posed) scene.
    // The pose must leave the startup surface point, not just report "applied".
    let actions: [(&str, CameraAction); 4] = [
        ("overview", |demo: &mut GravityOrbitsDemo| {
            demo.developer_apply_command(&DevCommand::Overview).unwrap()
        }),
        (
            "focus (frame the selected body)",
            |demo: &mut GravityOrbitsDemo| {
                demo.developer_set_session("camera-paused");
                let body = demo
                    .developer_inventory("camera-paused")
                    .unwrap()
                    .into_iter()
                    .find(|body| body.name != "Moon")
                    .expect("canonical system has a second body")
                    .handle;
                demo.developer_apply_command(&DevCommand::Focus {
                    body,
                    body_fixed: false,
                })
                .unwrap();
            },
        ),
        ("body orbit", |demo: &mut GravityOrbitsDemo| {
            demo.developer_apply_command(&DevCommand::NavigationMode {
                mode: "body_orbit".into(),
            })
            .unwrap();
            demo.developer_apply_command(&DevCommand::Clearance { meters: 4.0e7 })
                .unwrap();
        }),
        (
            "free flight with timed translation",
            |demo: &mut GravityOrbitsDemo| {
                demo.developer_apply_command(&DevCommand::NavigationMode {
                    mode: "free_flight".into(),
                })
                .unwrap();
                demo.developer_apply_command(&DevCommand::Navigation {
                    drag: [0.0; 2],
                    scroll_notches: 0.0,
                    translation: [0.0, 0.0, -1.0],
                    speed_multiplier: 1.0,
                    boost_multiplier: 1.0,
                    duration_s: 1.0,
                })
                .unwrap();
            },
        ),
    ];
    for (name, act) in actions {
        let mut demo = canonical();
        demo.set_paused(true).unwrap();
        let start = pose_position_m(&demo);
        act(&mut demo);
        run(&mut demo, 3.0);
        let end = pose_position_m(&demo);
        let moved = (0..3)
            .map(|i| (end[i] - start[i]).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(
            moved > 1.0,
            "{name}: pose moved only {moved} m while paused"
        );
        assert!(!demo.camera().transitioning(), "{name}: transition stalled");
    }
}
