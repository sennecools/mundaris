//! Builds the Studio profiler panel from the profiler history and CPU profile.

use mundaris_app::{
    GravityOrbitsDemo,
    profiler::FrameSample,
    studio::{
        profiler_view::{self, FrameTimeline, SpanStatRow},
        view::StatItem,
    },
};
use slint::{ModelRc, VecModel};

use std::{
    cell::{Ref, RefCell},
    rc::Rc,
    sync::Arc,
    sync::mpsc,
};

use serde_json::Value;

use crate::{
    FrameBar, HotSpan, LaneRow, ProfilerData, SpanBox, Stat, Tick,
    ui_sync::{stat, sync},
};

/// Frames shown in the strip.
pub const STRIP_FRAMES: usize = 240;
const BUDGET_MS: f64 = 1000.0 / 60.0;
/// Strip full height in milliseconds (two 60 Hz budgets).
const STRIP_MS: f64 = 2.0 * BUDGET_MS;
const SPAN_COLORS: u32 = 8;
const HOT_ROWS: usize = 12;

/// Persistent list models for the profiler panel. Rows are synced in place so
/// Slint keeps its element instances; replacing lists made every refresh
/// rebuild ~300 elements and measurably missed vsync.
#[derive(Default)]
pub struct ProfilerModels {
    bars: Rc<VecModel<FrameBar>>,
    lanes: Rc<VecModel<LaneRow>>,
    spans: Rc<VecModel<SpanBox>>,
    ticks: Rc<VecModel<Tick>>,
    frame_stats: Rc<VecModel<Stat>>,
    span_detail: Rc<VecModel<Stat>>,
    hot: Rc<VecModel<HotSpan>>,
    gpu_stats: Rc<VecModel<Stat>>,
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
    let timeline = selected_frame.and_then(|frame| profiler_view::frame_timeline(&profile, frame));
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

fn bind<T: Clone + PartialEq + 'static>(model: &Rc<VecModel<T>>, rows: Vec<T>) -> ModelRc<T> {
    sync(model, rows);
    ModelRc::from(model.clone())
}

fn ms(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_string(), |ms| format!("{ms:.2} ms"))
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

    let timeline = &derived.timeline;
    let selected_span = profiler.selected_span;
    let (lanes, spans, rows, ticks, title) = match timeline {
        Some(timeline) => {
            let duration = timeline.duration_ms.max(1e-6);
            let spans = timeline
                .spans
                .iter()
                .enumerate()
                .map(|(index, span)| SpanBox {
                    x: (span.start_ms / duration) as f32,
                    width: (span.duration_ms / duration) as f32,
                    row: span.row as i32,
                    name: span.name.as_str().into(),
                    color: profiler_view::color_slot(&span.name, SPAN_COLORS) as i32,
                    selected: Some(index) == selected_span,
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
            let step = tick_step(duration);
            let ticks = (0..)
                .map(|k| f64::from(k) * step)
                .take_while(|t| *t < duration)
                .map(|t| Tick {
                    x: (t / duration) as f32,
                    text: if step < 1.0 {
                        format!("{t:.2} ms")
                    } else {
                        format!("{t:.0} ms")
                    }
                    .into(),
                })
                .collect();
            (
                lanes,
                spans,
                timeline.rows as i32,
                ticks,
                format!(
                    "frame {} · {:.2} ms of main-thread work{}",
                    timeline.frame_id,
                    timeline.duration_ms,
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
    let engine_limit = [profiler.host_percentile(0.5), sample.gpu_ms]
        .into_iter()
        .flatten()
        .fold(None, |max: Option<f64>, v| {
            Some(max.map_or(v, |m| m.max(v)))
        });
    let frame_stats = [
        StatItem::new("Frame", sample.frame.to_string()),
        StatItem::new("Interval", ms(sample.interval_ms)),
        StatItem::new("Engine CPU", ms(sample.host_ms)),
        StatItem::new("GPU", ms(sample.gpu_ms)),
        StatItem::new("UI render + vsync wait", ms(sample.ui_render_ms)),
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
            Some(vec![
                StatItem::new("Name", span.name.clone()),
                StatItem::new("Duration", format!("{:.3} ms", span.duration_ms)),
                StatItem::new("Starts at", format!("+{:.3} ms", span.start_ms)),
                StatItem::new("Thread", timeline.lanes[span.lane].name.clone()),
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
            last: format!("{:.2}", row.last_ms).into(),
            p50: format!("{:.2}", row.p50_ms).into(),
            p95: format!("{:.2}", row.p95_ms).into(),
            max: format!("{:.2}", row.max_ms).into(),
        })
        .collect();
    let gpu_stats = [
        StatItem::new("Scene pass", ms(gpu_passes[0])),
        StatItem::new("Terrain draw", ms(gpu_passes[1])),
        StatItem::new("Overlays", ms(gpu_passes[2])),
        StatItem::new("Frame total", ms(sample.gpu_ms)),
    ];
    let enabled = profiler.enabled;
    let frozen = profiler.paused;
    let export_status = profiler.export_status().unwrap_or_default().to_string();
    let data = ProfilerData {
        enabled,
        frozen,
        capturing,
        following,
        bars: bind(&models.bars, bars),
        budget: (BUDGET_MS / STRIP_MS) as f32,
        lanes: bind(&models.lanes, lanes),
        spans: bind(&models.spans, spans),
        rows,
        ticks: bind(&models.ticks, ticks),
        timeline_title: title.into(),
        empty_hint: empty_hint.into(),
        frame_stats: bind(&models.frame_stats, frame_stats.iter().map(stat).collect()),
        span_detail: bind(&models.span_detail, span_detail.iter().map(stat).collect()),
        hot: bind(&models.hot, hot),
        gpu_stats: bind(&models.gpu_stats, gpu_stats.iter().map(stat).collect()),
        export_status: export_status.into(),
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
