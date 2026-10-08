#![cfg(feature = "developer-tools")]

use anyhow::Result;
use mundaris_app::{GravityOrbitsDemo, developer_protocol::DevCommand};

#[test]
fn retired_resident_tile_fixture_is_rejected_in_shared_scene_without_mutation() -> Result<()> {
    let session = "resident-tile-runtime-test";
    let mut demo = GravityOrbitsDemo::shared_test_system()?;
    demo.developer_set_session(session);
    let before = demo.developer_inventory(session)?;
    let revision = demo.world().revision();
    let before_authority = before
        .iter()
        .map(|body| {
            (
                body.handle.as_str(),
                body.name.as_str(),
                body.reference_radius_m,
                body.position_m,
                body.surface_available,
            )
        })
        .collect::<Vec<_>>();

    let error = demo
        .developer_apply_command(&DevCommand::GpuTile {
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
        })
        .expect_err("the standalone resident-tile fixture is retired");

    assert!(
        error
            .to_string()
            .contains("standalone fixture scenes are retired")
    );
    assert_eq!(demo.world().revision(), revision);
    let after = demo.developer_inventory(session)?;
    let after_authority = after
        .iter()
        .map(|body| {
            (
                body.handle.as_str(),
                body.name.as_str(),
                body.reference_radius_m,
                body.position_m,
                body.surface_available,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(after_authority, before_authority);
    Ok(())
}
