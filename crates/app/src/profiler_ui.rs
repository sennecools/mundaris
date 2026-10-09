//! Builds the Studio profiler panel from the profiler history and CPU profile.

use astrum_app::{
    GravityOrbitsDemo,
    profiler::FrameSample,
    studio::{
        profiler_view::{self, FrameTimeline, SpanKind, SpanStatRow},
        view::StatItem,
    },
};
use std::{
    cell::{Ref, RefCell},
    sync::Arc,
    sync::mpsc,
};

use serde_json::Value;

/// One frame-strip bar; heights are fractions of the strip (0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct FrameBar {
    pub height: f32,
    /// Engine CPU share of the strip height.
    pub cpu: f32,
    /// Sampled GPU frame time share; 0 when not sampled.
    pub gpu: f32,
    /// 0 within budget, 2 over 16.7 ms, 3 over 33.3 ms.
    pub tone: i32,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaneRow {
    pub name: String,
    pub first_row: i32,
    pub rows: i32,
}

/// A span placed in the shown timeline window (x and width are 0..1).
#[derive(Debug, Clone, PartialEq)]
pub struct SpanBox {
    pub x: f32,
    pub width: f32,
    pub row: i32,
    pub name: String,
    pub color: i32,
    pub kind: SpanKind,
    /// Clipped at the right edge of the shown window.
    pub clipped: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub x: f32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HotSpan {
    pub name: String,
    pub last: String,
    pub p50: String,
    pub p95: String,
    pub max: String,
}

/// Everything the profiler panel draws, rebuilt at the panel refresh rate.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProfilerData {
    pub enabled: bool,
    pub frozen: bool,
    pub capturing: bool,
    pub following: bool,
    pub full_frame: bool,
    pub bars: Vec<FrameBar>,
    /// 16.7 ms budget line as a fraction of the strip height.
    pub budget: f32,
    pub lanes: Vec<LaneRow>,
    pub spans: Vec<SpanBox>,
    pub rows: i32,
    pub ticks: Vec<Tick>,
    pub timeline_title: String,
    pub empty_hint: String,
    pub frame_stats: Vec<StatItem>,
    pub span_detail: Vec<StatItem>,
    pub hot: Vec<HotSpan>,
    pub gpu_stats: Vec<StatItem>,
    pub export_status: String,
}

/// Frames shown in the strip.
pub const STRIP_FRAMES: usize = 240;
const BUDGET_MS: f64 = 1000.0 / 60.0;
/// Strip full height in milliseconds (two 60 Hz budgets).
const STRIP_MS: f64 = 2.0 * BUDGET_MS;
const SPAN_COLORS: u32 = 8;
const HOT_ROWS: usize = 12;

/// Off-thread derivation state for the profiler panel.
#[derive(Default)]
pub struct ProfilerModels {
    cache: RefCell<Derived>,
    worker: RefCell<Option<DeriveWorker>>,
}

/// Profile-derived data. Parsing the ~1 MB profile on the UI thread cost
/// ~5-8 ms per new sample and missed vsync, so a worker derives it and the
/// panel shows the result on the next refresh.
#[derive(Default)]
pub struct Derived {
    profile: Option<Arc<Value>>,
    requested: Option<u64>,
    selected_frame: Option<u64>,
    timeline: Option<FrameTimeline>,
    hot: Vec<SpanStatRow>,
}

enum DeriveRequest {
    Derive(Arc<Value>, Option<u64>),
    /// Retired data freed on the worker: dropping the last reference to a
    /// ~1 MB profile took ~3-4 ms on the UI thread.
    Retire(Derived),
}

fn derive(profile: Arc<Value>, requested: Option<u64>) -> Derived {
    let frames = profiler_view::frame_ids(&profile);
    // Follow the newest frame whose spans are complete (the last one may
    // still be missing its UI render span).
    let selected_frame = requested.or_else(|| {
        frames
            .len()
            .checked_sub(2)
            .map(|index| frames[index])
            .or(frames.last().copied())
    });
    let timeline =
        selected_frame.and_then(|frame| profiler_view::frame_timeline(&profile, frame, None));
    let hot = profiler_view::span_stats(&profile);
    Derived {
        profile: Some(profile),
        requested,
        selected_frame,
        timeline,
        hot,
    }
}

struct DeriveWorker {
    requests: mpsc::Sender<DeriveRequest>,
    results: mpsc::Receiver<Derived>,
    pending: bool,
}

impl DeriveWorker {
    fn spawn() -> Option<Self> {
        let (requests, request_rx) = mpsc::channel::<DeriveRequest>();
        let (result_tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("profiler-derive".into())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    match request {
                        DeriveRequest::Derive(profile, requested) => {
                            if result_tx.send(derive(profile, requested)).is_err() {
                                break;
                            }
                        }
                        DeriveRequest::Retire(retired) => drop(retired),
                    }
                }
            })
            .ok()?;
        Some(Self {
            requests,
            results,
            pending: false,
        })
    }
}

impl ProfilerModels {
    /// Latest derived data; requests a new derivation when the profile sample
    /// or requested frame changed and none is in flight.
    fn derived(&self, profile: Option<&Arc<Value>>, requested: Option<u64>) -> Ref<'_, Derived> {
        let mut worker = self.worker.borrow_mut();
        if worker.is_none() {
            *worker = DeriveWorker::spawn();
        }
        if let Some(worker) = worker.as_mut() {
            while let Ok(result) = worker.results.try_recv() {
                worker.pending = false;
                let retired = std::mem::replace(&mut *self.cache.borrow_mut(), result);
                let _ = worker.requests.send(DeriveRequest::Retire(retired));
            }
            let cache = self.cache.borrow();
            let stale = cache.requested != requested
                || match (&cache.profile, profile) {
                    (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                    (None, None) => false,
                    _ => true,
                };
            drop(cache);
            if stale
                && !worker.pending
                && let Some(profile) = profile
                && worker
                    .requests
                    .send(DeriveRequest::Derive(Arc::clone(profile), requested))
                    .is_ok()
            {
                worker.pending = true;
            }
        }
        self.cache.borrow()
    }
}

fn ms(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_string(), |ms| format!("{ms:.2} ms"))
}

/// Main-thread CPU of a frame: engine work plus UI drawing.
fn main_thread_ms(host_ms: Option<f64>, ui_ms: Option<f64>) -> Option<f64> {
    match (host_ms, ui_ms) {
        (None, None) => None,
        (host, ui) => Some(host.unwrap_or(0.0) + ui.unwrap_or(0.0)),
    }
}

/// Tick spacing that yields roughly 4–8 labelled divisions.
fn tick_step(duration_ms: f64) -> f64 {
    [0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0]
        .into_iter()
        .find(|step| duration_ms / step <= 8.0)
        .unwrap_or(200.0)
}

/// Builds the panel data; returns the engine frame number of each strip bar so
/// clicks can be mapped back to frames.
pub fn build(demo: &mut GravityOrbitsDemo, models: &ProfilerModels) -> (ProfilerData, Vec<u64>) {
    let gpu_passes = demo.studio_view().gpu_passes;
    let capturing = demo.bad_frame_capture_active();
    let profiler = demo.profiler();
    let history: Vec<FrameSample> = profiler.history().copied().collect();
    let window = &history[history.len().saturating_sub(STRIP_FRAMES)..];
    let profile = profiler.profile();
    let following = profiler.selected_frame.is_none();
    let derived = models.derived(profile.as_ref(), profiler.selected_frame);
    let selected_frame = derived.selected_frame;

    let bar_frames: Vec<u64> = window.iter().map(|sample| sample.frame).collect();
    let bars = window
        .iter()
        .map(|sample| {
            let interval = sample.interval_ms.unwrap_or(0.0);
            FrameBar {
                height: (interval / STRIP_MS).clamp(0.0, 1.0) as f32,
                cpu: (sample.host_ms.unwrap_or(0.0) / STRIP_MS).clamp(0.0, 1.0) as f32,
                gpu: sample
                    .gpu_ms
                    .map_or(0.0, |gpu| (gpu / STRIP_MS).clamp(0.005, 1.0))
                    as f32,
                tone: if interval > 2.0 * BUDGET_MS {
                    3
                } else if interval > BUDGET_MS {
                    2
                } else {
                    0
                },
                selected: Some(sample.frame) == selected_frame,
            }
        })
        .collect();

    // The worker derives the CPU timeline; the GPU lane of a sampled frame is
    // added here from the profiler's per-frame GPU samples.
    let mut timeline = derived.timeline.clone();
    if let Some(timeline) = &mut timeline
        && let Some(gpu) = profiler.gpu_frame(timeline.frame_id)
    {
        profiler_view::add_gpu_lane(timeline, &gpu.scopes);
    }
    let full_frame = profiler.full_frame;
    let selected_span = profiler.selected_span;
    let (lanes, spans, rows, ticks, title) = match &timeline {
        Some(timeline) => {
            // "Engine work" fits the window to engine CPU work and GPU scopes;
            // UI drawing and presentation waits are clipped at the right edge.
            let view = if full_frame {
                timeline.duration_ms
            } else {
                timeline.work_end_ms * 1.04
            }
            .max(1e-3);
            let spans = timeline
                .spans
                .iter()
                .enumerate()
                .filter(|(_, span)| span.start_ms < view)
                .map(|(index, span)| {
                    let end = span.start_ms + span.duration_ms;
                    let clipped = end > view;
                    let precision = if span.duration_ms < 1.0 { 2 } else { 1 };
                    SpanBox {
                        x: (span.start_ms / view) as f32,
                        width: ((end.min(view) - span.start_ms) / view) as f32,
                        row: span.row as i32,
                        name: format!(
                            "{} {:.precision$} ms{}",
                            span.name,
                            span.duration_ms,
                            if clipped { " →" } else { "" }
                        ),
                        color: profiler_view::color_slot(&span.name, SPAN_COLORS) as i32,
                        kind: span.kind,
                        clipped,
                        selected: Some(index) == selected_span,
                    }
                })
                .collect();
            let lanes = timeline
                .lanes
                .iter()
                .map(|lane| LaneRow {
                    name: lane.name.as_str().into(),
                    first_row: lane.first_row as i32,
                    rows: lane.rows as i32,
                })
                .collect();
            let step = tick_step(view);
            let ticks = (0..)
                .map(|k| f64::from(k) * step)
                .take_while(|t| *t < view)
                .map(|t| Tick {
                    x: (t / view) as f32,
                    text: if step < 1.0 {
                        format!("{t:.2} ms")
                    } else {
                        format!("{t:.0} ms")
                    },
                })
                .collect();
            let gpu = timeline.gpu_ms.map_or_else(
                || "GPU not sampled".to_string(),
                |gpu| format!("GPU {gpu:.2} ms"),
            );
            (
                lanes,
                spans,
                timeline.rows as i32,
                ticks,
                format!(
                    "frame {} · engine {:.2} ms · UI draw {:.2} ms · vsync wait {:.2} ms · other {:.2} ms · {gpu}{}",
                    timeline.frame_id,
                    timeline.work_ms,
                    timeline.ui_ms,
                    timeline.wait_ms,
                    timeline.untracked_ms,
                    if following {
                        " · following latest"
                    } else {
                        ""
                    }
                ),
            )
        }
        None => (Vec::new(), Vec::new(), 0, Vec::new(), String::new()),
    };
    let empty_hint = if !profiler.enabled && profile.is_none() {
        "Turn on CPU spans to record a per-frame timeline."
    } else if profile.is_none() {
        "Waiting for the first CPU profile sample…"
    } else {
        "This frame is no longer in the span buffer. Pick a recent frame or press Latest frame."
    };

    let sample = selected_frame
        .and_then(|frame| history.iter().find(|s| s.frame == frame))
        .or(history.last())
        .copied()
        .unwrap_or_default();
    let interval_p50 = profiler.interval_percentile(0.5);
    let engine_limit = [profiler.host_percentile(0.5), profiler.gpu_percentile(0.5)]
        .into_iter()
        .flatten()
        .fold(None, |max: Option<f64>, v| {
            Some(max.map_or(v, |m| m.max(v)))
        });
    let frame_stats = [
        StatItem::new("Frame", sample.frame.to_string()),
        StatItem::new("Interval", ms(sample.interval_ms)),
        StatItem::new("Engine CPU", ms(sample.host_ms)),
        StatItem::new(
            "GPU (scene)",
            sample
                .gpu_ms
                .map_or_else(|| "not sampled".to_string(), |gpu| format!("{gpu:.2} ms")),
        ),
        StatItem::new("UI draw", ms(sample.ui_render_ms)),
        StatItem::new(
            "Bottleneck",
            profiler_view::bottleneck(
                sample.interval_ms,
                main_thread_ms(sample.host_ms, sample.ui_render_ms),
                sample.gpu_ms,
            )
            .label(),
        ),
        StatItem::new(
            "Tiles produced",
            sample
                .terrain_jobs
                .map_or_else(|| "—".to_string(), |jobs| jobs.to_string()),
        ),
        StatItem::new(
            "Presented FPS (p50)",
            interval_p50.map_or_else(|| "—".to_string(), |ms| format!("{:.0}", 1000.0 / ms)),
        ),
        StatItem::new(
            "Engine-only limit",
            engine_limit.map_or_else(
                || "—".to_string(),
                |ms| format!("{:.0} FPS", 1000.0 / ms.max(1e-3)),
            ),
        ),
    ];
    let span_detail: Vec<StatItem> = timeline
        .as_ref()
        .zip(selected_span)
        .and_then(|(timeline, index)| {
            let span = timeline.spans.get(index)?;
            let lane = timeline.lanes[span.lane].name.clone();
            Some(vec![
                StatItem::new("Name", span.name.clone()),
                StatItem::new("Duration", format!("{:.3} ms", span.duration_ms)),
                StatItem::new(
                    if span.kind == SpanKind::Gpu {
                        "After submit"
                    } else {
                        "Starts at"
                    },
                    format!("+{:.3} ms", span.start_ms),
                ),
                StatItem::new(
                    if span.kind == SpanKind::Gpu {
                        "Queue"
                    } else {
                        "Thread"
                    },
                    lane,
                ),
                StatItem::new("Depth", span.depth.to_string()),
            ])
        })
        .unwrap_or_default();
    let hot = derived
        .hot
        .iter()
        .take(HOT_ROWS)
        .map(|row| HotSpan {
            name: row.name.as_str().into(),
            last: format!("{:.2}", row.last_ms),
            p50: format!("{:.2}", row.p50_ms),
            p95: format!("{:.2}", row.p95_ms),
            max: format!("{:.2}", row.max_ms),
        })
        .collect();
    // Scopes of the selected frame when sampled, else of the latest sample.
    let gpu_frame = selected_frame
        .and_then(|frame| profiler.gpu_frame(frame))
        .or_else(|| profiler.latest_gpu_frame());
    let gpu_stats: Vec<StatItem> = match gpu_frame {
        Some(gpu) => std::iter::once(StatItem::new("Sampled frame", gpu.frame.to_string()))
            .chain(gpu.scopes.iter().map(|scope| {
                StatItem::new(
                    &format!("{}{}", "  ".repeat(usize::from(scope.depth)), scope.name),
                    format!("{:.2} ms", scope.end_ms - scope.start_ms),
                )
            }))
            .collect(),
        None => vec![
            StatItem::new("Scene pass", ms(gpu_passes[0])),
            StatItem::new("Terrain draw", ms(gpu_passes[1])),
            StatItem::new("Overlays", ms(gpu_passes[2])),
        ],
    };
    let enabled = profiler.enabled;
    let frozen = profiler.paused;
    let export_status = profiler.export_status().unwrap_or_default().to_string();
    let data = ProfilerData {
        enabled,
        frozen,
        capturing,
        following,
        full_frame,
        bars,
        budget: (BUDGET_MS / STRIP_MS) as f32,
        lanes,
        spans,
        rows,
        ticks,
        timeline_title: title,
        empty_hint: empty_hint.into(),
        frame_stats: frame_stats.to_vec(),
        span_detail,
        hot,
        gpu_stats,
        export_status,
    };
    (data, bar_frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_steps_keep_divisions_readable() {
        assert_eq!(tick_step(0.3), 0.05);
        assert_eq!(tick_step(4.0), 0.5);
        assert_eq!(tick_step(16.7), 5.0);
        assert!(tick_step(5000.0) >= 200.0);
    }
}
