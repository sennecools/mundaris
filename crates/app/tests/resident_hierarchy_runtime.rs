#![cfg(feature = "developer-tools")]

use anyhow::Result;
use mundaris_app::GravityOrbitsDemo;
use mundaris_app::developer_protocol::DevCommand;
use serde_json::json;

#[test]
fn hierarchy_command_defaults_are_fixed_and_bounded() -> Result<()> {
    let command: DevCommand = serde_json::from_value(json!({
        "action": "gpu_hierarchy",
        "enabled": true,
    }))?;
    match command {
        DevCommand::GpuHierarchy {
            enabled,
            refine,
            morph_duration_ms,
            child_delays_ms,
            request_mask,
            cancel_pending,
            diagnostic_validate,
        } => {
            assert!(enabled);
            assert!(refine);
            assert_eq!(morph_duration_ms, 150);
            assert_eq!(child_delays_ms, [0; 4]);
            assert_eq!(request_mask, 0x0f);
            assert!(!cancel_pending);
            assert!(!diagnostic_validate);
        }
        _ => panic!("expected GPU hierarchy command"),
    }

    let session = "resident-hierarchy-runtime-test";
    let mut demo = GravityOrbitsDemo::solar_system(true)?;
    demo.developer_set_session(session);
    let error = demo
        .developer_apply_command(&DevCommand::GpuHierarchy {
            enabled: true,
            refine: true,
            morph_duration_ms: 10_001,
            child_delays_ms: [0; 4],
            request_mask: 0x0f,
            cancel_pending: false,
            diagnostic_validate: false,
        })
        .expect_err("hierarchy duration over its fixture bound must be rejected");
    assert!(error.to_string().contains("active GpuTile parent"));

    let mut demo = GravityOrbitsDemo::solar_system(true)?;
    demo.developer_set_session(session);
    let error = demo
        .developer_apply_command(&DevCommand::GpuHierarchy {
            enabled: false,
            refine: false,
            morph_duration_ms: 10_001,
            child_delays_ms: [0; 4],
            request_mask: 0x0f,
            cancel_pending: false,
            diagnostic_validate: false,
        })
        .expect_err("hierarchy duration over its fixture bound must be rejected");
    assert!(error.to_string().contains("prototype limit"));
    Ok(())
}
