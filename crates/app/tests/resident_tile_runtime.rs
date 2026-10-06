#![cfg(feature = "developer-tools")]

use anyhow::Result;
use mundaris_app::{GravityOrbitsDemo, developer_protocol::DevCommand};

#[test]
fn resident_tile_enable_changes_authority_and_disable_restores_body_state() -> Result<()> {
    let session = "resident-tile-runtime-test";
    let mut demo = GravityOrbitsDemo::solar_system(true)?;
    demo.developer_set_session(session);
    let moon = demo
        .developer_inventory(session)?
        .into_iter()
        .find(|body| body.name == "Moon")
        .expect("real Solar System has a Moon");
    demo.developer_apply_command(&DevCommand::Select {
        body: moon.handle.clone(),
    })?;

    let original_radius = moon.reference_radius_m;
    let original_surface = moon.surface_available;
    demo.developer_apply_command(&DevCommand::GpuTile {
        enabled: true,
        family: "rocky_v5".into(),
        seed: 0,
        radius_m: 80_000.0,
        face: "positive_z".into(),
        level: 9,
        x: 157,
        y: 39,
        cells: 64,
        revision: 0,
    })?;
    let active = demo
        .developer_inventory(session)?
        .into_iter()
        .find(|body| body.name == "Moon")
        .unwrap();
    assert_eq!(active.reference_radius_m, 80_000.0);
    assert!(active.surface_available);

    demo.developer_apply_command(&DevCommand::GpuTile {
        enabled: false,
        family: "rocky_v5".into(),
        seed: 0,
        radius_m: 80_000.0,
        face: "positive_z".into(),
        level: 9,
        x: 157,
        y: 39,
        cells: 64,
        revision: 0,
    })?;
    let restored = demo
        .developer_inventory(session)?
        .into_iter()
        .find(|body| body.name == "Moon")
        .unwrap();
    assert_eq!(restored.reference_radius_m, original_radius);
    assert_eq!(restored.surface_available, original_surface);
    assert!(demo.developer_last_snapshot().is_none());
    Ok(())
}
