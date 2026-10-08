#![cfg(feature = "developer-tools")]

use anyhow::Result;
use mundaris_app::GravityOrbitsDemo;
use mundaris_app::developer_protocol::DevCommand;
use serde_json::json;

#[test]
fn hierarchy_defaults_are_fixed_and_standalone_fixtures_are_retired() -> Result<()> {
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
    let mut demo = GravityOrbitsDemo::shared_test_system()?;
    demo.developer_set_session(session);
    for enabled in [true, false] {
        let error = demo
            .developer_apply_command(&DevCommand::GpuHierarchy {
                enabled,
                refine: true,
                morph_duration_ms: 150,
                child_delays_ms: [0; 4],
                request_mask: 0x0f,
                cancel_pending: false,
                diagnostic_validate: false,
            })
            .expect_err("standalone hierarchy fixtures are retired");
        assert!(
            error
                .to_string()
                .contains("standalone fixture scenes are retired")
        );
    }
    Ok(())
}
