#![cfg(feature = "developer-tools")]

use mundaris_app::developer_scenarios::{Predicate, Scenario, run_offscreen};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[test]
fn agreed_scenario_fixtures_parse_and_validate() {
    for source in scenario_sources() {
        let scenario: Scenario = serde_json::from_str(source).unwrap();
        scenario.validate().unwrap();
        assert!(scenario.deterministic);
        assert!(!scenario.initial_settings.is_empty());
    }
}

fn scenario_sources() -> [&'static str; 5] {
    [
        include_str!("../../../scenarios/developer/navigation.json"),
        include_str!("../../../scenarios/developer/rendering.json"),
        include_str!("../../../scenarios/developer/analytic-playback.json"),
        include_str!("../../../scenarios/developer/convergence-1-2-5s.json"),
        include_str!("../../../scenarios/developer/gpu-tile-cold-warm.json"),
    ]
}

#[test]
fn scenario_actions_require_finite_bounded_durations() {
    let scenario: Scenario = serde_json::from_str(
        r#"{"schema":1,"preset":"solar-system","steps":[{"action":{"action":"pause","paused":true},"duration_s":1e99}]}"#,
    )
    .unwrap();
    assert!(scenario.validate().is_err());
}

#[test]
#[ignore = "runs all production offscreen scenarios twice and requires the renderer/GPU stack"]
fn deterministic_scenarios_repeat_semantic_checkpoints_and_rgba_pixels() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/developer-scenario-repeat/{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    for (scenario_index, source) in scenario_sources().iter().enumerate() {
        let scenario: Scenario = serde_json::from_str(source).unwrap();
        scenario.validate().unwrap();
        assert!(scenario.deterministic);
        let first_dir = base.join(format!("{scenario_index}-first"));
        let second_dir = base.join(format!("{scenario_index}-second"));
        let first = run_offscreen(&scenario, &first_dir).unwrap();
        let second = run_offscreen(&scenario, &second_dir).unwrap();

        let first_steps = first["steps"].as_array().unwrap();
        let second_steps = second["steps"].as_array().unwrap();
        for (index, step) in scenario.steps.iter().enumerate() {
            let Some(checkpoint) = &step.checkpoint else {
                continue;
            };
            let first_snapshot = checkpoint_snapshot(first_steps, index);
            let second_snapshot = checkpoint_snapshot(second_steps, index);
            assert_eq!(
                predicate_value(first_snapshot, &checkpoint.predicate),
                predicate_value(second_snapshot, &checkpoint.predicate),
                "{} checkpoint {:?} semantic value differs between repeats",
                scenario.preset,
                checkpoint.name
            );
        }
        for step in &scenario.steps {
            let Some(capture) = &step.capture else {
                continue;
            };
            assert_eq!(
                decode_rgba(&first_dir.join(format!("{}.png", capture.name))),
                decode_rgba(&second_dir.join(format!("{}.png", capture.name))),
                "{} capture {:?} pixels differ between repeats",
                scenario.preset,
                capture.name
            );
        }
    }
}

fn checkpoint_snapshot(steps: &[Value], index: usize) -> &Value {
    steps
        .iter()
        .find(|record| record["index"].as_u64() == Some(index as u64))
        .and_then(|record| record.get("snapshot"))
        .expect("checkpoint result snapshot")
}

fn predicate_value<'a>(snapshot: &'a Value, predicate: &Predicate) -> &'a Value {
    predicate
        .path
        .split('.')
        .try_fold(snapshot, |value, part| value.get(part))
        .expect("checkpoint semantic predicate value")
}

fn decode_rgba(path: &PathBuf) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
    let frame = reader.next_frame(&mut decoded).unwrap();
    assert_eq!(frame.color_type, png::ColorType::Rgba);
    assert_eq!(frame.bit_depth, png::BitDepth::Eight);
    (
        frame.width,
        frame.height,
        decoded[..frame.buffer_size()].to_vec(),
    )
}
