#![cfg(feature = "developer-tools")]
//! Developer camera commands must visibly move the observer in the canonical
//! paused scene, whose startup surface pose is a developer fixture pose.
use mundaris_app::{
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
