#![cfg(feature = "developer-tools")]
use mundaris_app::{
    GravityOrbitsDemo,
    developer_protocol::{DevCommand, DevRequest},
};
#[test]
fn handles_are_session_scoped_and_inventory_is_observational() {
    let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
    demo.developer_set_session("first");
    let revision = demo.world().revision();
    let a = demo.developer_inventory("first").unwrap();
    let b = demo.developer_inventory("second").unwrap();
    assert_eq!(revision, demo.world().revision());
    assert_ne!(a[0].handle, b[0].handle);
    assert!(
        demo.developer_apply_command(&DevCommand::Select {
            body: b[0].handle.clone()
        })
        .is_err()
    );
    demo.developer_apply_command(&DevCommand::Select {
        body: a[0].handle.clone(),
    })
    .unwrap();
    assert_eq!(revision, demo.world().revision());
    assert_eq!(a[0].state_reference_frame, "system_inertial");
}
#[test]
fn malformed_nonfinite_and_unknown_operations_do_not_mutate_authority() {
    let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
    let revision = demo.world().revision();
    for command in [
        DevCommand::Seek { seconds: f64::NAN },
        DevCommand::Rate {
            multiplier: f64::INFINITY,
        },
        DevCommand::Clearance { meters: -1.0 },
        DevCommand::SkySetting {
            setting: "intensity".into(),
            value: 5.0,
        },
        DevCommand::RenderMode {
            mode: "invalid".into(),
        },
    ] {
        assert!(demo.developer_apply_command(&command).is_err());
        assert_eq!(revision, demo.world().revision());
    }
    assert!(serde_json::from_str::<DevRequest>(r#"{"protocol_version":1,"session_id":"a","request_id":"x","operation":{"op":"action","lease":"x","command":{"action":"seek","seconds":NaN}}}"#).is_err());
    assert!(
        serde_json::from_str::<DevCommand>(r#"{"action":"shell","command":"anything"}"#).is_err()
    );
    assert!(
        serde_json::from_str::<DevCommand>(r#"{"action":"pause","paused":true,"unexpected":42}"#)
            .is_err()
    );
}
