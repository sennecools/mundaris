#![cfg(feature = "developer-tools")]

use mundaris_app::{GravityOrbitsDemo, developer_protocol::DevCommand};
use serde_json::json;

#[test]
fn regional_command_defaults_match_the_slice_fixture() {
    let command: DevCommand = serde_json::from_value(json!({
        "action": "gpu_regional",
        "enabled": true,
    }))
    .unwrap();
    match command {
        DevCommand::GpuRegional {
            enabled,
            max_depth,
            gpu_slots,
            cpu_tiles,
            worker_count,
            worker_delay_ms,
            upload_tiles_per_frame,
            upload_bytes_per_frame,
            publication_groups_per_frame,
            transition_limit,
            morph_duration_ms,
            split_error_px,
            merge_error_px,
        } => {
            assert!(enabled);
            assert_eq!(max_depth, 3);
            assert_eq!(gpu_slots, 128);
            assert_eq!(cpu_tiles, 256);
            assert_eq!(worker_count, 4);
            assert_eq!(worker_delay_ms, 0);
            assert_eq!(upload_tiles_per_frame, 2);
            assert_eq!(upload_bytes_per_frame, 1_048_576);
            assert_eq!(publication_groups_per_frame, 2);
            assert_eq!(transition_limit, 4);
            assert_eq!(morph_duration_ms, 150);
            assert_eq!(split_error_px, 2.0);
            assert_eq!(merge_error_px, 1.0);
        }
        _ => panic!("expected GPU regional command"),
    }
}

#[test]
fn regional_runtime_rejects_prototype_limit_and_hysteresis_violations() {
    let invalid = [
        json!({"max_depth": 5}),
        json!({"gpu_slots": 4}),
        json!({"cpu_tiles": 4}),
        json!({"worker_count": 0}),
        json!({"worker_delay_ms": 5001}),
        json!({"upload_tiles_per_frame": 0}),
        json!({"upload_bytes_per_frame": 0}),
        json!({"publication_groups_per_frame": 0}),
        json!({"transition_limit": 0}),
        json!({"morph_duration_ms": 10001}),
        json!({"split_error_px": 1.0, "merge_error_px": 1.0}),
    ];

    let mut demo = GravityOrbitsDemo::solar_system(true).unwrap();
    for overrides in invalid {
        let mut value = json!({"action": "gpu_regional", "enabled": false});
        for (key, parameter) in overrides.as_object().unwrap() {
            value[key] = parameter.clone();
        }
        let command: DevCommand = serde_json::from_value(value).unwrap();
        assert!(
            demo.developer_apply_command(&command).is_err(),
            "expected regional parameter rejection for {overrides}"
        );
    }
}

#[test]
fn regional_route_fixture_uses_snapshot_contract_and_valid_scenario_shape() {
    use mundaris_app::developer_protocol::DevCommand;
    use mundaris_app::developer_scenarios::Scenario;

    let scenario: Scenario = serde_json::from_str(include_str!(
        "../../../scenarios/developer/gpu-regional-route.json"
    ))
    .unwrap();
    scenario.validate().unwrap();
    assert!(!scenario.deterministic);

    let root = scenario
        .initial_settings
        .iter()
        .find_map(|command| match command {
            DevCommand::GpuTile { level, cells, .. } => Some((*level, *cells)),
            _ => None,
        })
        .expect("route configures its canonical root tile");
    assert_eq!(root, (9, 32));

    let cancellation_probe_seeds: Vec<_> = scenario
        .steps
        .iter()
        .filter_map(|step| match step.action.as_ref()? {
            DevCommand::GpuTile {
                enabled: true,
                seed,
                ..
            } => Some(*seed),
            _ => None,
        })
        .collect();
    assert_eq!(cancellation_probe_seeds, [1, 0]);

    let enabled_regional: Vec<_> = scenario
        .steps
        .iter()
        .filter_map(|step| match step.action.as_ref()? {
            DevCommand::GpuRegional {
                enabled: true,
                max_depth,
                worker_delay_ms,
                worker_count,
                gpu_slots,
                cpu_tiles,
                upload_tiles_per_frame,
                upload_bytes_per_frame,
                ..
            } => Some((
                *max_depth,
                *worker_delay_ms,
                *worker_count,
                *gpu_slots,
                *cpu_tiles,
                *upload_tiles_per_frame,
                *upload_bytes_per_frame,
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        enabled_regional,
        [
            (4, 0, 4, 384, 512, 2, 1_048_576),
            (4, 50, 4, 384, 512, 2, 1_048_576),
            (4, 250, 4, 384, 512, 2, 1_048_576),
            (4, 500, 4, 384, 512, 2, 1_048_576),
            (4, 500, 2, 384, 512, 2, 1_048_576),
            (4, 0, 4, 12, 512, 1, 39_200),
        ]
    );
    assert_eq!(
        enabled_regional.last(),
        Some(&(4, 0, 4, 12, 512, 1, 39_200))
    );
    let thresholds: Vec<_> = scenario
        .steps
        .iter()
        .filter_map(|step| match step.action.as_ref()? {
            DevCommand::GpuRegional {
                enabled: true,
                split_error_px,
                merge_error_px,
                ..
            } => Some((*split_error_px, *merge_error_px)),
            _ => None,
        })
        .collect();
    assert_eq!(thresholds, [(0.15, 0.075); 6]);

    let regional_offsets: Vec<_> = scenario
        .steps
        .iter()
        .filter_map(|step| match step.action.as_ref()? {
            DevCommand::GpuTileView {
                camera_offset_m, ..
            } => Some(*camera_offset_m),
            _ => None,
        })
        .collect();
    assert_eq!(regional_offsets.len(), 129);
    for (endpoint, expected_count) in [
        ([0.0, 0.0, 3000.0], 12),
        ([80.0, 0.0, -170.0], 12),
        ([-80.0, 0.0, -170.0], 5),
    ] {
        assert_eq!(
            regional_offsets
                .iter()
                .filter(|offset| **offset == endpoint)
                .count(),
            expected_count,
            "route omitted an exact stop at {endpoint:?}"
        );
    }
    let cancellation_probe_jump = |from: [f64; 3], to: [f64; 3]| {
        (from == [80.0, 0.0, -170.0] && to == [0.0, 0.0, 3000.0])
            || (from == [0.0, 0.0, 3000.0] && to == [80.0, 0.0, -170.0])
    };
    let probe_jumps = regional_offsets
        .windows(2)
        .filter(|pair| cancellation_probe_jump(pair[0], pair[1]))
        .count();
    assert_eq!(probe_jumps, 2, "missing cancellation-probe reversals");
    assert!(regional_offsets.windows(2).all(|pair| {
        let distance = pair[0]
            .iter()
            .zip(pair[1])
            .map(|(from, to)| (from - to).powi(2))
            .sum::<f64>()
            .sqrt();
        distance <= 800.0 || cancellation_probe_jump(pair[0], pair[1])
    }));

    let checkpoints: Vec<_> = scenario
        .steps
        .iter()
        .filter_map(|step| {
            step.checkpoint
                .as_ref()
                .map(|item| item.predicate.path.as_str())
        })
        .collect();
    let valid_paths = [
        "resident_regional.ready",
        "resident_regional.quality_pending",
        "resident_regional.refinement_debt",
        "resident_regional.slot_pressure",
        "resident_regional.upload_pressure",
        "resident_regional.gpu_admission_pressure",
        "resident_regional.gpu_minimum_peak_split_slots",
        "resident_regional.upload_tile_capacity",
        "resident_regional.upload_byte_capacity",
        "resident_regional.worker_queued",
        "resident_regional.worker_running",
        "resident_regional.completion_backlog",
        "resident_regional.cache_hits",
        "resident_regional.cancelled_before_start",
        "resident_regional.cancelled_during_work",
        "resident_regional.desired_count",
        "resident_regional.drawable_count",
    ];
    for path in &checkpoints {
        assert!(
            valid_paths.contains(path),
            "scenario checkpoint has invalid or unsupported snapshot path {path:?}"
        );
    }
    for path in [
        "resident_regional.ready",
        "resident_regional.quality_pending",
        "resident_regional.refinement_debt",
        "resident_regional.gpu_admission_pressure",
        "resident_regional.gpu_minimum_peak_split_slots",
        "resident_regional.upload_tile_capacity",
        "resident_regional.upload_byte_capacity",
        "resident_regional.worker_queued",
        "resident_regional.worker_running",
        "resident_regional.completion_backlog",
        "resident_regional.cache_hits",
        "resident_regional.cancelled_before_start",
        "resident_regional.cancelled_during_work",
        "resident_regional.desired_count",
        "resident_regional.drawable_count",
    ] {
        assert!(checkpoints.contains(&path), "route omits checkpoint {path}");
    }
    let checkpoint = |name: &str| {
        scenario
            .steps
            .iter()
            .filter_map(|step| step.checkpoint.as_ref())
            .find(|item| item.name == name)
            .unwrap_or_else(|| panic!("route omits checkpoint {name}"))
    };
    let no_frontier_fits = checkpoint("route_pressure_no_split_frontier_fits");
    assert_eq!(
        no_frontier_fits.predicate.path,
        "resident_regional.gpu_minimum_peak_split_slots"
    );
    assert_eq!(no_frontier_fits.predicate.at_least, Some(13.0));
    let upload_tile_cap = checkpoint("route_pressure_upload_tile_cap");
    assert_eq!(
        upload_tile_cap.predicate.path,
        "resident_regional.upload_tile_capacity"
    );
    assert_eq!(upload_tile_cap.predicate.equals, Some(serde_json::json!(1)));
    let upload_byte_cap = checkpoint("route_pressure_upload_byte_cap");
    assert_eq!(
        upload_byte_cap.predicate.path,
        "resident_regional.upload_byte_capacity"
    );
    assert_eq!(
        upload_byte_cap.predicate.equals,
        Some(serde_json::json!(39_200))
    );
    assert!(
        checkpoints.contains(&"resident_regional.ready"),
        "pressure route omits its retained-cover readiness checkpoint"
    );
}
