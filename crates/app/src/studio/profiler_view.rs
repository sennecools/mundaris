//! CPU timeline and span statistics derived from an engine profile snapshot
//! (`engine_profile::ProfileSnapshot` serialized as JSON). Toolkit-free.

use serde_json::Value;

use crate::developer_snapshot::GpuScopeSnapshot;

/// Main-thread spans of Studio UI layout, tessellation and encoding. They are
/// shown in their own style and excluded from the engine-work window.
pub const UI_SPANS: &[&str] = &["UI draw"];
/// Main-thread spans that block on presentation (the vsync wait).
pub const WAIT_SPANS: &[&str] = &["Surface acquire", "Present"];
/// CPU span whose end is the queue submit; the GPU lane starts there.
pub const SUBMIT_SPAN: &str = "GPU submission";
pub const GPU_LANE: &str = "GPU (from submit)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    Work,
    Ui,
    Wait,
    Gpu,
}

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
    pub kind: SpanKind,
}

/// Spans of one frame across every lane, relative to the frame start.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameTimeline {
    pub frame_id: u64,
    pub duration_ms: f64,
    /// End of the last engine span (CPU or GPU): the engine-work window.
    pub work_end_ms: f64,
    /// Top-level main-thread engine work, UI drawing, presentation waits, and
    /// the remaining untracked main-thread time (event loop).
    pub work_ms: f64,
    pub ui_ms: f64,
    pub wait_ms: f64,
    pub untracked_ms: f64,
    /// GPU frame time when this frame's submission was timestamp-sampled.
    pub gpu_ms: Option<f64>,
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
/// `gpu` scopes, when this frame was sampled, form a final lane placed from the
/// end of the CPU submit span; that is the earliest the GPU can start, so the
/// lane shows GPU durations and order, not measured CPU/GPU clock alignment.
pub fn frame_timeline(
    profile: &Value,
    frame_id: u64,
    gpu: Option<&[GpuScopeSnapshot]>,
) -> Option<FrameTimeline> {
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
                kind: if UI_SPANS.contains(&event.name) {
                    SpanKind::Ui
                } else if WAIT_SPANS.contains(&event.name) {
                    SpanKind::Wait
                } else {
                    SpanKind::Work
                },
            });
        }
        timeline_lanes.push(TimelineLane {
            name: lane_name(lane),
            first_row: next_row,
            rows,
        });
        next_row += rows;
    }
    let duration_ms = ms(end_ns - start_ns);
    let main_top = |kind| {
        spans
            .iter()
            .filter(|s| s.lane == 0 && s.depth == 0 && s.kind == kind)
            .map(|s| s.duration_ms)
            .sum::<f64>()
    };
    let (work_ms, ui_ms, wait_ms) = (
        main_top(SpanKind::Work),
        main_top(SpanKind::Ui),
        main_top(SpanKind::Wait),
    );
    let untracked_ms = (duration_ms - work_ms - ui_ms - wait_ms).max(0.0);

    let mut timeline = FrameTimeline {
        frame_id,
        duration_ms,
        work_end_ms: 0.0,
        work_ms,
        ui_ms,
        wait_ms,
        untracked_ms,
        gpu_ms: None,
        lanes: timeline_lanes,
        spans,
        rows: next_row,
    };
    timeline.work_end_ms = work_end(&timeline.spans);
    if let Some(scopes) = gpu {
        add_gpu_lane(&mut timeline, scopes);
    }
    Some(timeline)
}

/// End of the last main-thread engine span or GPU span.
fn work_end(spans: &[TimelineSpan]) -> f64 {
    spans
        .iter()
        .filter(|s| (s.lane == 0 && s.kind == SpanKind::Work) || s.kind == SpanKind::Gpu)
        .map(|s| s.start_ms + s.duration_ms)
        .fold(0.0, f64::max)
}

/// Append the GPU lane for a sampled frame, placed from the end of the CPU
/// submit span. No-op without scopes or without a submit span.
pub fn add_gpu_lane(timeline: &mut FrameTimeline, scopes: &[GpuScopeSnapshot]) {
    let Some(anchor) = timeline
        .spans
        .iter()
        .find(|s| s.lane == 0 && s.name == SUBMIT_SPAN)
        .map(|s| s.start_ms + s.duration_ms)
    else {
        return;
    };
    if scopes.is_empty() {
        return;
    }
    let lane = timeline.lanes.len();
    let first_row = timeline.rows;
    let rows = scopes
        .iter()
        .map(|s| usize::from(s.depth) + 1)
        .max()
        .unwrap_or(1);
    for scope in scopes {
        timeline.spans.push(TimelineSpan {
            lane,
            row: first_row + usize::from(scope.depth),
            start_ms: anchor + scope.start_ms,
            duration_ms: (scope.end_ms - scope.start_ms).max(0.0),
            name: scope.name.clone(),
            depth: usize::from(scope.depth),
            kind: SpanKind::Gpu,
        });
        timeline.duration_ms = timeline.duration_ms.max(anchor + scope.end_ms);
    }
    timeline.gpu_ms = scopes
        .iter()
        .find(|s| s.depth == 0)
        .map(|s| s.end_ms - s.start_ms);
    timeline.lanes.push(TimelineLane {
        name: GPU_LANE.into(),
        first_row,
        rows,
    });
    timeline.rows += rows;
    timeline.work_end_ms = work_end(&timeline.spans);
}
/// What limited a frame, judged from its own timings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bottleneck {
    /// Main-thread CPU and GPU both finished well inside the presented interval.
    Vsync,
    Cpu,
    Gpu,
    Unknown,
}

impl Bottleneck {
    pub fn label(self) -> &'static str {
        match self {
            Self::Vsync => "Vsync (headroom)",
            Self::Cpu => "CPU",
            Self::Gpu => "GPU",
            Self::Unknown => "—",
        }
    }
}

/// A frame is CPU- or GPU-bound when that side takes most of the interval;
/// otherwise presentation (vsync) set the pace. `cpu_ms` should include UI
/// drawing on the main thread. The GPU figure covers scene work only: the UI
/// toolkit's own GPU composition is not timestamped.
pub fn bottleneck(
    interval_ms: Option<f64>,
    cpu_ms: Option<f64>,
    gpu_ms: Option<f64>,
) -> Bottleneck {
    const BOUND_SHARE: f64 = 0.8;
    let Some(interval) = interval_ms.filter(|v| *v > 0.0) else {
        return Bottleneck::Unknown;
    };
    let cpu = cpu_ms.unwrap_or(0.0);
    let gpu = gpu_ms.unwrap_or(0.0);
    if cpu.max(gpu) < BOUND_SHARE * interval {
        if cpu_ms.is_none() && gpu_ms.is_none() {
            Bottleneck::Unknown
        } else {
            Bottleneck::Vsync
        }
    } else if gpu > cpu {
        Bottleneck::Gpu
    } else {
        Bottleneck::Cpu
    }
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
                    {"name": "UI draw", "frame_id": 7, "start_ns": 3_100_000, "end_ns": 3_600_000, "nesting_depth": 0},
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
        let timeline = frame_timeline(&profile(), 7, None).unwrap();
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
        assert!(frame_timeline(&profile(), 99, None).is_none());
    }

    #[test]
    fn ui_draw_is_separated_and_gpu_lane_starts_at_submit() {
        let profile = json!({"lanes": [{"worker": {"kind": "main"}, "events": [
            {"name": "Frame", "frame_id": 3, "start_ns": 0, "end_ns": 600_000, "nesting_depth": 0},
            {"name": "GPU submission", "frame_id": 3, "start_ns": 400_000, "end_ns": 500_000, "nesting_depth": 1},
            {"name": "Surface acquire", "frame_id": 3, "start_ns": 700_000, "end_ns": 1_100_000, "nesting_depth": 0},
            {"name": "UI draw", "frame_id": 3, "start_ns": 2_000_000, "end_ns": 10_000_000, "nesting_depth": 0}
        ]}]});
        let gpu = [
            GpuScopeSnapshot {
                name: "GPU frame".into(),
                depth: 0,
                start_ms: 0.0,
                end_ms: 0.3,
            },
            GpuScopeSnapshot {
                name: "Scene pass".into(),
                depth: 1,
                start_ms: 0.05,
                end_ms: 0.25,
            },
        ];
        let timeline = frame_timeline(&profile, 3, Some(&gpu)).unwrap();
        assert!((timeline.work_ms - 0.6).abs() < 1e-9);
        assert!((timeline.ui_ms - 8.0).abs() < 1e-9);
        assert!((timeline.wait_ms - 0.4).abs() < 1e-9);
        assert!((timeline.untracked_ms - 1.0).abs() < 1e-9);
        let ui = timeline
            .spans
            .iter()
            .find(|s| s.kind == SpanKind::Ui)
            .unwrap();
        assert_eq!(ui.name, "UI draw");
        assert_eq!(timeline.lanes.last().unwrap().name, GPU_LANE);
        let scene = timeline
            .spans
            .iter()
            .find(|s| s.name == "Scene pass")
            .unwrap();
        assert!(
            (scene.start_ms - 0.55).abs() < 1e-9,
            "submit end 0.5 + 0.05"
        );
        assert_eq!(scene.row, timeline.lanes.last().unwrap().first_row + 1);
        assert!(
            (timeline.work_end_ms - 0.8).abs() < 1e-9,
            "GPU frame ends at 0.5 + 0.3"
        );
        assert_eq!(timeline.gpu_ms.map(|v| (v * 1e3).round()), Some(300.0));

        let unsampled = frame_timeline(&profile, 3, None).unwrap();
        assert!(unsampled.gpu_ms.is_none());
        assert!((unsampled.work_end_ms - 0.6).abs() < 1e-9);
    }

    #[test]
    fn bottleneck_compares_each_side_with_the_interval() {
        assert_eq!(
            bottleneck(Some(8.3), Some(0.6), Some(0.3)),
            Bottleneck::Vsync
        );
        assert_eq!(
            bottleneck(Some(20.0), Some(18.0), Some(2.0)),
            Bottleneck::Cpu
        );
        assert_eq!(
            bottleneck(Some(20.0), Some(3.0), Some(19.0)),
            Bottleneck::Gpu
        );
        assert_eq!(bottleneck(Some(8.3), None, None), Bottleneck::Unknown);
        assert_eq!(bottleneck(None, Some(1.0), None), Bottleneck::Unknown);
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
