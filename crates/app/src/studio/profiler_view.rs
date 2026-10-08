//! CPU timeline and span statistics derived from an engine profile snapshot
//! (`engine_profile::ProfileSnapshot` serialized as JSON). Toolkit-free.

use serde_json::Value;

/// One thread lane of the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineLane {
    pub name: String,
    /// First row of this lane in the stacked timeline.
    pub first_row: usize,
    /// Nesting rows used by this lane (at least one).
    pub rows: usize,
}

/// One completed span placed on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineSpan {
    pub lane: usize,
    /// Absolute row: lane first row plus nesting depth.
    pub row: usize,
    pub start_ms: f64,
    pub duration_ms: f64,
    pub name: String,
    pub depth: usize,
}

/// Spans of one frame across every lane, relative to the frame start.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameTimeline {
    pub frame_id: u64,
    pub duration_ms: f64,
    pub lanes: Vec<TimelineLane>,
    pub spans: Vec<TimelineSpan>,
    pub rows: usize,
}

/// Rolling statistics for one span name.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanStatRow {
    pub name: String,
    pub last_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
    pub samples: u64,
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1.0e6
}

/// Lane identity as serialized by `engine_profile::WorkerIdentity`
/// (`{"kind": "main"}`, `{"kind": "named", "value": ..}`, `{"kind": "numeric", "value": ..}`).
fn lane_name(lane: &Value) -> String {
    let worker = &lane["worker"];
    match worker["kind"].as_str() {
        Some("main") => "Main thread".into(),
        Some("named") => worker["value"].as_str().unwrap_or("Worker").to_string(),
        Some("numeric") => format!("Worker {}", worker["value"].as_u64().unwrap_or(0)),
        _ => "Worker".into(),
    }
}

fn is_main(lane: &Value) -> bool {
    lane["worker"]["kind"].as_str() == Some("main")
}

struct Event<'a> {
    name: &'a str,
    frame_id: u64,
    start_ns: u64,
    end_ns: u64,
    depth: usize,
}

fn events(lane: &Value) -> impl Iterator<Item = Event<'_>> {
    lane["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|event| {
            let start_ns = event["start_ns"].as_u64()?;
            let end_ns = event["end_ns"].as_u64()?;
            (end_ns >= start_ns).then(|| Event {
                name: event["name"].as_str().unwrap_or("?"),
                frame_id: event["frame_id"].as_u64().unwrap_or(0),
                start_ns,
                end_ns,
                depth: event["nesting_depth"].as_u64().unwrap_or(0) as usize,
            })
        })
}

/// Frame identifiers with main-thread spans in `profile`, ascending.
pub fn frame_ids(profile: &Value) -> Vec<u64> {
    let mut ids: Vec<u64> = profile["lanes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|lane| is_main(lane))
        .flat_map(|lane| events(lane).map(|event| event.frame_id).collect::<Vec<_>>())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Timeline of `frame_id`: the frame window spans every main-thread event of
/// that frame; worker spans overlapping the window are included and clipped.
pub fn frame_timeline(profile: &Value, frame_id: u64) -> Option<FrameTimeline> {
    let lanes = profile["lanes"].as_array()?;
    let (start_ns, end_ns) = lanes
        .iter()
        .filter(|lane| is_main(lane))
        .flat_map(|lane| events(lane).collect::<Vec<_>>())
        .filter(|event| event.frame_id == frame_id)
        .fold(None, |window: Option<(u64, u64)>, event| {
            Some(
                window.map_or((event.start_ns, event.end_ns), |(start, end)| {
                    (start.min(event.start_ns), end.max(event.end_ns))
                }),
            )
        })?;
    let mut timeline_lanes = Vec::new();
    let mut spans = Vec::new();
    let mut next_row = 0;
    // Main thread first, then workers in their snapshot order.
    let mut ordered: Vec<&Value> = lanes.iter().filter(|lane| is_main(lane)).collect();
    ordered.extend(lanes.iter().filter(|lane| !is_main(lane)));
    for lane in ordered {
        let lane_spans: Vec<Event<'_>> = events(lane)
            .filter(|event| event.end_ns > start_ns && event.start_ns < end_ns)
            .collect();
        if lane_spans.is_empty() {
            continue;
        }
        let rows = lane_spans
            .iter()
            .map(|event| event.depth + 1)
            .max()
            .unwrap_or(1);
        let index = timeline_lanes.len();
        for event in lane_spans {
            let clipped_start = event.start_ns.max(start_ns);
            let clipped_end = event.end_ns.min(end_ns);
            spans.push(TimelineSpan {
                lane: index,
                row: next_row + event.depth,
                start_ms: ms(clipped_start - start_ns),
                duration_ms: ms(clipped_end - clipped_start),
                name: event.name.to_string(),
                depth: event.depth,
            });
        }
        timeline_lanes.push(TimelineLane {
            name: lane_name(lane),
            first_row: next_row,
            rows,
        });
        next_row += rows;
    }
    Some(FrameTimeline {
        frame_id,
        duration_ms: ms(end_ns - start_ns),
        lanes: timeline_lanes,
        spans,
        rows: next_row,
    })
}

/// Rolling per-name statistics, slowest p95 first.
pub fn span_stats(profile: &Value) -> Vec<SpanStatRow> {
    let mut rows: Vec<SpanStatRow> = profile["stats"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|stat| {
            Some(SpanStatRow {
                name: stat["name"].as_str()?.to_string(),
                last_ms: ms(stat["current_ns"].as_u64()?),
                p50_ms: ms(stat["p50_ns"].as_u64()?),
                p95_ms: ms(stat["p95_ns"].as_u64()?),
                max_ms: ms(stat["max_ns"].as_u64()?),
                samples: stat["sample_count"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    rows.sort_by(|a, b| b.p95_ms.total_cmp(&a.p95_ms));
    rows
}

/// Stable colour slot for a span name, so a span keeps its colour across frames.
pub fn color_slot(name: &str, slots: u32) -> u32 {
    let hash = name.bytes().fold(2_166_136_261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16_777_619)
    });
    hash % slots.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile() -> Value {
        json!({
            "lanes": [
                {"worker": {"kind": "named", "value": "Terrain"}, "events": [
                    {"name": "Bake", "frame_id": 7, "start_ns": 1_500_000, "end_ns": 4_000_000, "nesting_depth": 0}
                ]},
                {"worker": {"kind": "main"}, "events": [
                    {"name": "Frame", "frame_id": 7, "start_ns": 1_000_000, "end_ns": 3_000_000, "nesting_depth": 0},
                    {"name": "Simulation", "frame_id": 7, "start_ns": 1_200_000, "end_ns": 1_700_000, "nesting_depth": 1},
                    {"name": "Slint UI render", "frame_id": 7, "start_ns": 3_100_000, "end_ns": 3_600_000, "nesting_depth": 0},
                    {"name": "Frame", "frame_id": 8, "start_ns": 9_000_000, "end_ns": 9_500_000, "nesting_depth": 0}
                ]}
            ],
            "stats": [
                {"name": "Frame", "current_ns": 2_000_000, "p50_ns": 1_000_000, "p95_ns": 3_000_000, "max_ns": 5_000_000, "sample_count": 10},
                {"name": "Simulation", "current_ns": 500_000, "p50_ns": 400_000, "p95_ns": 600_000, "max_ns": 900_000, "sample_count": 10}
            ]
        })
    }

    #[test]
    fn frame_window_covers_main_events_and_clips_workers() {
        let timeline = frame_timeline(&profile(), 7).unwrap();
        assert!((timeline.duration_ms - 2.6).abs() < 1e-9);
        assert_eq!(timeline.lanes[0].name, "Main thread");
        assert_eq!(timeline.lanes[0].rows, 2);
        assert_eq!(timeline.lanes[1].name, "Terrain");
        assert_eq!(timeline.lanes[1].first_row, 2);
        assert_eq!(timeline.rows, 3);
        let bake = timeline.spans.iter().find(|s| s.name == "Bake").unwrap();
        assert!((bake.start_ms - 0.5).abs() < 1e-9);
        assert!(
            (bake.duration_ms - 2.1).abs() < 1e-9,
            "clipped to the frame window"
        );
        let simulation = timeline
            .spans
            .iter()
            .find(|s| s.name == "Simulation")
            .unwrap();
        assert_eq!(simulation.row, 1);
        assert!(frame_timeline(&profile(), 99).is_none());
    }

    #[test]
    fn real_worker_identity_serialization_is_recognised() {
        let main = serde_json::to_value(crate::engine_profile::WorkerIdentity::Main).unwrap();
        assert!(is_main(&json!({"worker": main})));
        let named =
            serde_json::to_value(crate::engine_profile::WorkerIdentity::Named("Atlas bind"))
                .unwrap();
        assert_eq!(lane_name(&json!({"worker": named})), "Atlas bind");
    }

    #[test]
    fn frames_and_stats_are_ordered() {
        assert_eq!(frame_ids(&profile()), vec![7, 8]);
        let stats = span_stats(&profile());
        assert_eq!(stats[0].name, "Frame");
        assert!((stats[0].p95_ms - 3.0).abs() < 1e-9);
        assert_eq!(color_slot("Frame", 8), color_slot("Frame", 8));
    }
}
