//! Compact, read-only performance and terrain diagnostics for developer builds.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use egui::{Align, Color32, Context, Pos2, RichText, Sense, Stroke, Vec2};
use serde_json::Value;

use crate::developer_snapshot::{DeveloperSnapshot, PerformanceSnapshot};

const HISTORY_CAPACITY: usize = 600;
const DEFAULT_FRAME_TARGET_MS: f64 = 1000.0 / 60.0;
const HOST_STALL_BOUND_MS: f64 = 100.0;
const PUBLICATION_BUDGET_MS: f64 = 2.0;
const PANEL_WIDTH: f32 = 900.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Overview,
    Timeline,
    Terrain,
    Jobs,
    Gpu,
    Captures,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metric {
    HostFrame,
    RenderPresent,
    Update,
    TerrainUpdate,
    TerrainPreparation,
    Diagnostics,
    GpuPreparation,
    GpuTerrain,
    GpuFrame,
    Publication,
    PublicationAdmission,
}

impl Metric {
    const ALL: [Self; 11] = [
        Self::HostFrame,
        Self::RenderPresent,
        Self::Update,
        Self::TerrainUpdate,
        Self::TerrainPreparation,
        Self::Diagnostics,
        Self::GpuPreparation,
        Self::GpuTerrain,
        Self::GpuFrame,
        Self::Publication,
        Self::PublicationAdmission,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::HostFrame => "Host frame",
            Self::RenderPresent => "Render + present",
            Self::Update => "Simulation update",
            Self::TerrainUpdate => "Terrain update",
            Self::TerrainPreparation => "App / terrain preparation (CPU)",
            Self::Diagnostics => "Diagnostic publication",
            Self::GpuPreparation => "GPU terrain preparation (CPU)",
            Self::GpuTerrain => "GPU terrain execution",
            Self::GpuFrame => "GPU frame execution",
            Self::Publication => "Terrain publication (combined)",
            Self::PublicationAdmission => "Publication admission (CPU)",
        }
    }

    fn budget_ms(self, frame_target_ms: f64) -> Option<f64> {
        match self {
            Self::HostFrame => Some(frame_target_ms),
            Self::Publication => Some(PUBLICATION_BUDGET_MS),
            _ => None,
        }
    }

    fn value(self, snapshot: &DeveloperSnapshot) -> Option<f64> {
        self.value_from(&snapshot.performance, resident_value(snapshot))
    }

    fn value_from(
        self,
        performance: &PerformanceSnapshot,
        resident: Option<&Value>,
    ) -> Option<f64> {
        match self {
            Self::HostFrame => performance
                .host_frame_ms
                .or_else(|| resident_latest_sample_number(resident, "host_frame_ms")),
            Self::RenderPresent => performance
                .render_present_ms
                .or_else(|| resident_latest_sample_number(resident, "render_present_ms")),
            Self::Update => performance.update_ms,
            Self::TerrainUpdate => resident_latest_sample_number(resident, "regional_advance_ms")
                .or_else(|| resident_number(resident, "/regional_advance_ms"))
                .or(performance.terrain_update_ms),
            Self::TerrainPreparation => resident_latest_sample_number(resident, "preparation_ms")
                .or_else(|| resident_number(resident, "/preparation_ms"))
                .or(performance.preparation_ms)
                .or_else(|| resident_latest_sample_number(resident, "gpu_preparation_ms"))
                .or_else(|| resident_number(resident, "/gpu_preparation_ms"))
                .or(performance.terrain_preparation_ms),
            Self::Diagnostics => performance
                .diagnostics_ms
                .or_else(|| resident_latest_sample_number(resident, "diagnostics_ms")),
            Self::GpuPreparation => performance
                .gpu_preparation_cpu_ms
                .or_else(|| resident_latest_sample_number(resident, "gpu_preparation_ms")),
            Self::GpuTerrain => performance.gpu_terrain_ms,
            Self::GpuFrame => performance.gpu_frame_ms,
            Self::Publication => resident_number(resident, "/publication_ms"),
            Self::PublicationAdmission => resident_number(resident, "/publication_admission_ms")
                .or_else(|| {
                    resident_number(resident, "/publication_pipeline/publication_admission_ms")
                }),
        }
        .filter(|value| value.is_finite() && *value >= 0.0)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct FrameSample {
    frame: u64,
    values: [Option<f64>; 11],
}

#[derive(Debug, Clone, Copy)]
struct TimelineRange {
    start_ns: u64,
    end_ns: u64,
}

struct TimelineLaneIndex {
    max_depth: usize,
}

struct IndexedTimelineProfile<'a> {
    data: &'a Value,
    index: &'a TimelineIndex,
}

struct TimelineIndex {
    source: usize,
    capture_id: Option<String>,
    generated_at_ns: u64,
    lanes: Vec<TimelineLaneIndex>,
    retained_bounds: Option<TimelineRange>,
    frame_bounds: HashMap<u64, TimelineRange>,
    frame_starts: Vec<(u64, u64)>,
}

impl TimelineIndex {
    fn new(profile: &Value) -> Self {
        let source = profile as *const Value as usize;
        let capture_id = profile
            .get("capture_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let generated_at_ns = json_u64(profile, "generated_at_ns").unwrap_or(0);
        let lanes_value = profile
            .get("lanes")
            .or_else(|| profile.pointer("/profile/lanes"));
        let lane_values: Vec<(&str, &Value)> = match lanes_value {
            Some(Value::Array(values)) => values.iter().map(|value| ("", value)).collect(),
            Some(Value::Object(values)) => values
                .iter()
                .map(|(name, value)| (name.as_str(), value))
                .collect(),
            _ => Vec::new(),
        };
        let mut lanes = Vec::with_capacity(lane_values.len());
        let mut retained = None;
        let mut frame_bounds = HashMap::<u64, (u64, u64)>::new();
        let mut frame_starts = Vec::new();
        for (_, lane) in lane_values {
            let events = lane["events"].as_array();
            let mut max_depth = 0usize;
            if let Some(events) = events {
                let by_id = events
                    .iter()
                    .filter_map(|event| Some((json_u64(event, "event_id")?, event)))
                    .collect::<HashMap<_, _>>();
                for event in events {
                    let depth = json_u64(event, "nesting_depth")
                        .map(|depth| depth.min(63) as usize)
                        .unwrap_or_else(|| nested_depth(event, &by_id).0);
                    max_depth = max_depth.max(depth);
                    let Some((start, end)) =
                        json_u64(event, "start_ns").zip(json_u64(event, "end_ns"))
                    else {
                        continue;
                    };
                    if end < start {
                        continue;
                    }
                    retained = Some(retained.map_or((start, end), |(min, max): (u64, u64)| {
                        (min.min(start), max.max(end))
                    }));
                    if field_text(event, &["name"]) == Some("Frame")
                        && let Some(frame) = json_u64(event, "frame_id")
                    {
                        frame_starts.push((start, frame));
                        frame_bounds
                            .entry(frame)
                            .and_modify(|bounds| {
                                bounds.0 = bounds.0.min(start);
                                bounds.1 = bounds.1.max(end);
                            })
                            .or_insert((start, end));
                    }
                }
            }
            lanes.push(TimelineLaneIndex { max_depth });
        }
        frame_starts.sort_unstable();
        Self {
            source,
            capture_id,
            generated_at_ns,
            lanes,
            retained_bounds: retained.map(|(start, end)| TimelineRange::from_bounds(start, end)),
            frame_bounds: frame_bounds
                .into_iter()
                .map(|(frame, (start, end))| (frame, TimelineRange::from_bounds(start, end)))
                .collect(),
            frame_starts,
        }
    }

    fn matches(&self, profile: &Value) -> bool {
        self.source == profile as *const Value as usize
            && self.capture_id.as_deref() == profile.get("capture_id").and_then(Value::as_str)
            && self.generated_at_ns == json_u64(profile, "generated_at_ns").unwrap_or(0)
    }
}

impl TimelineRange {
    fn from_bounds(start_ns: u64, end_ns: u64) -> Self {
        let start_ns = start_ns.min(u64::MAX - 1);
        let end_ns = end_ns.max(start_ns.saturating_add(1));
        Self { start_ns, end_ns }
    }

    fn span_ns(self) -> u64 {
        self.end_ns.saturating_sub(self.start_ns).max(1)
    }

    fn clip(self, start_ns: u64, end_ns: u64) -> Option<(u64, u64)> {
        if end_ns < start_ns || end_ns < self.start_ns || start_ns > self.end_ns {
            return None;
        }
        Some((start_ns.max(self.start_ns), end_ns.min(self.end_ns)))
    }

    fn zoom(self, factor: f64) -> Self {
        self.zoom_at(factor, 0.5)
    }

    fn zoom_at(self, factor: f64, fraction: f64) -> Self {
        let fraction = fraction.clamp(0.0, 1.0);
        let span =
            (self.span_ns() as f64 * factor.clamp(0.1, 10.0)).clamp(100_000.0, 60_000_000_000.0);
        let anchor = self.start_ns as f64 + self.span_ns() as f64 * fraction;
        let start = (anchor - span * fraction).max(0.0) as u64;
        Self::from_bounds(start, start.saturating_add(span as u64))
    }

    fn with_span_ns(self, span: f64) -> Self {
        let new_span = span.clamp(100_000.0, 60_000_000_000.0);
        let center = self.start_ns as f64 + self.span_ns() as f64 * 0.5;
        Self::from_bounds(
            (center - new_span * 0.5).max(0.0) as u64,
            (center + new_span * 0.5).min(u64::MAX as f64) as u64,
        )
    }

    fn resize_for_width(self, previous: f64, current: f64) -> Self {
        if !previous.is_finite() || !current.is_finite() || previous < 1.0 || current < 1.0 {
            return self;
        }
        self.with_span_ns(self.span_ns() as f64 * current / previous)
    }

    fn pan(self, fraction: f64) -> Self {
        let offset =
            (self.span_ns() as f64 * fraction).clamp(-(u64::MAX as f64), u64::MAX as f64) as i128;
        let start = (self.start_ns as i128 + offset).clamp(0, u64::MAX as i128) as u64;
        let end = (start as u128 + self.span_ns() as u128).min(u64::MAX as u128) as u64;
        Self::from_bounds(start, end)
    }
}

impl FrameSample {
    fn from_snapshot(snapshot: &DeveloperSnapshot) -> Self {
        Self {
            frame: snapshot.general.frame_number,
            values: Metric::ALL.map(|metric| metric.value(snapshot)),
        }
    }

    fn get(self, metric: Metric) -> Option<f64> {
        self.values[Metric::ALL.iter().position(|item| *item == metric)?]
    }
}

/// Small diagnostic command shared by UI and native developer controls.
#[derive(Default)]
pub struct ProfilerControls<'a> {
    pub enabled: Option<bool>,
    pub freeze: Option<bool>,
    pub view: Option<&'a str>,
    pub frame: Option<u64>,
    pub event: Option<u64>,
    pub export: bool,
    pub zoom: Option<f64>,
    pub pan: Option<f64>,
    pub fill_window: Option<bool>,
}

/// State for the developer Performance Lab window.
///
/// `ingest` stores bounded samples only. It performs no I/O, capture, benchmark,
/// renderer control, or terrain query; action flags are consumed by the caller.
pub struct PerformanceLab {
    pub open: bool,
    pub enabled: bool,
    pub paused: bool,
    pub capture_requested: bool,
    pub capture_stop_requested: bool,
    history: VecDeque<FrameSample>,
    maxima: [Option<f64>; 11],
    selected_frame: Option<u64>,
    selected_job: Option<usize>,
    selected_job_key: Option<String>,
    tab: Tab,
    latest_frame: Option<u64>,
    latest_profile: Option<Arc<Value>>,
    frozen_snapshot: Option<Arc<DeveloperSnapshot>>,
    timeline_range: Option<TimelineRange>,
    timeline_index: Option<Arc<TimelineIndex>>,
    selected_event_id: Option<u64>,
    exporter: crate::profile_export::TimelineExporter,
    export_status: Option<String>,
    filter_frame: bool,
    fill_window: bool,
    last_axis_width: Option<f32>,
    frame_target_ms: f64,
}

impl Default for PerformanceLab {
    fn default() -> Self {
        Self {
            open: true,
            enabled: false,
            paused: false,
            capture_requested: false,
            capture_stop_requested: false,
            history: VecDeque::with_capacity(HISTORY_CAPACITY),
            maxima: [None; 11],
            selected_frame: None,
            selected_job: None,
            selected_job_key: None,
            tab: Tab::Overview,
            latest_frame: None,
            latest_profile: None,
            frozen_snapshot: None,
            timeline_range: None,
            timeline_index: None,
            selected_event_id: None,
            exporter: crate::profile_export::TimelineExporter::default(),
            export_status: None,
            filter_frame: false,
            fill_window: false,
            last_axis_width: None,
            frame_target_ms: DEFAULT_FRAME_TARGET_MS,
        }
    }
}

impl PerformanceLab {
    fn timeline_index_for(&mut self, profile: &Value) -> Arc<TimelineIndex> {
        if self
            .timeline_index
            .as_ref()
            .is_none_or(|index| !index.matches(profile))
        {
            self.timeline_index = Some(Arc::new(TimelineIndex::new(profile)));
        }
        Arc::clone(
            self.timeline_index
                .as_ref()
                .expect("timeline index initialized"),
        )
    }

    /// Retain cheap frame metrics. The host calls this for each completed frame;
    /// duplicate frame identifiers are ignored so the 4 Hz `ingest` path can
    /// safely call it as a fallback.
    pub fn observe_frame(&mut self, snapshot: &DeveloperSnapshot) {
        if !self.enabled || self.paused {
            return;
        }
        if self.latest_frame == Some(snapshot.general.frame_number) {
            return;
        }
        let sample = FrameSample::from_snapshot(snapshot);
        for (index, value) in sample.values.iter().enumerate() {
            if let Some(value) = value {
                self.maxima[index] = Some(self.maxima[index].map_or(*value, |old| old.max(*value)));
            }
        }
        if self.history.len() == HISTORY_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(sample);
        self.latest_frame = Some(sample.frame);
    }

    /// Retain the latest profiler payload at the caller's <=4 Hz cadence.
    /// Frame samples are taken separately by `observe_frame` on every host
    /// frame, so short spikes are not lost to profiler snapshot cadence.
    pub fn ingest(&mut self, snapshot: &DeveloperSnapshot, profile: Option<&Value>) {
        self.observe_frame(snapshot);
        if !self.enabled || self.paused {
            return;
        }
        self.latest_profile = profile
            .map(|value| Arc::new(value.clone()))
            .or_else(|| snapshot.engine_profile.clone());
    }

    /// Draw the compact panel. `profile` is JSON to keep this view independent
    /// of the profiler's Rust API while that API evolves.
    pub fn draw(
        &mut self,
        ctx: &Context,
        snapshot: Option<&DeveloperSnapshot>,
        profile: Option<&Value>,
    ) {
        if !self.open {
            return;
        }

        if let Some(result) = self.exporter.poll() {
            self.export_status = Some(result);
        }
        let mut open = self.open;
        let frozen_snapshot = self.frozen_snapshot.clone();
        let latest_profile = self.latest_profile.clone();
        let mut window = egui::Window::new("Performance Lab")
            .open(&mut open)
            .default_width(PANEL_WIDTH)
            .default_height(720.0)
            .resizable(true);
        if self.fill_window {
            let available = ctx.content_rect().shrink(8.0);
            window = window.fixed_pos(available.min).fixed_size(available.size());
        }
        window.show(ctx, |ui| {
            self.draw_header(ui, snapshot);
            ui.separator();
            self.draw_tabs(ui);
            ui.separator();
            let shown_snapshot = if self.paused {
                frozen_snapshot.as_deref().or(snapshot)
            } else {
                snapshot
            };
            let shown_profile = if self.paused {
                shown_snapshot
                    .and_then(|current| current.engine_profile.as_deref())
                    .or(latest_profile.as_deref())
            } else if self.enabled {
                shown_snapshot
                    .and_then(|current| current.engine_profile.as_deref())
                    .or(profile)
                    .or(latest_profile.as_deref())
            } else {
                None
            };
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match self.tab {
                    Tab::Overview => self.draw_overview(ui, shown_snapshot),
                    Tab::Timeline => self.draw_timeline(ui, shown_profile, shown_snapshot),
                    Tab::Terrain => self.draw_terrain(ui, shown_snapshot),
                    Tab::Jobs => self.draw_jobs(ui, shown_snapshot),
                    Tab::Gpu => self.draw_gpu(ui, shown_snapshot),
                    Tab::Captures => self.draw_captures(ui, shown_snapshot),
                });
        });
        self.open = open;
    }

    fn freeze(&self, snapshot: Option<&DeveloperSnapshot>) -> Option<Arc<DeveloperSnapshot>> {
        snapshot.cloned().map(|mut snapshot| {
            if snapshot.engine_profile.is_none() {
                snapshot.engine_profile = self.latest_profile.clone();
            }
            Arc::new(snapshot)
        })
    }

    /// Diagnostic-only controls used by the native UI and developer protocol.
    pub fn configure(
        &mut self,
        snapshot: Option<&DeveloperSnapshot>,
        controls: ProfilerControls<'_>,
    ) -> Result<(), &'static str> {
        let ProfilerControls {
            enabled,
            freeze,
            view,
            frame,
            event,
            export,
            zoom,
            pan,
            fill_window,
        } = controls;
        if zoom.is_some_and(|value| !value.is_finite() || !(0.1..=10.0).contains(&value))
            || pan.is_some_and(|value| !value.is_finite() || value.abs() > 10.0)
        {
            return Err("invalid_timeline_transform");
        }
        let tab = match view {
            None => None,
            Some("timeline") => Some(Tab::Timeline),
            Some("jobs") => Some(Tab::Jobs),
            Some("gpu") => Some(Tab::Gpu),
            Some("overview") => Some(Tab::Overview),
            _ => return Err("invalid_profiler_view"),
        };
        if let Some(fill) = fill_window {
            self.fill_window = fill;
        }
        if let Some(enabled) = enabled {
            self.enabled = enabled;
        }
        if let Some(freeze) = freeze {
            self.frozen_snapshot = if freeze { self.freeze(snapshot) } else { None };
            self.paused = freeze;
        }
        if let Some(tab) = tab {
            self.tab = tab;
            self.open = true;
        }
        if let Some(frame) = frame {
            self.selected_frame = Some(frame);
            self.timeline_range = None;
        }
        if let Some(event_id) = event {
            self.selected_event_id = Some(event_id);
            let current = self.frozen_snapshot.as_deref().or(snapshot);
            let selected_identity = current
                .and_then(|snapshot| snapshot.engine_profile.as_deref())
                .and_then(|profile| profile["lanes"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|lane| lane["events"].as_array())
                .flatten()
                .find(|event| json_u64(event, "event_id") == Some(event_id))
                .and_then(|event| event.get("job_identity"));
            if let (Some(identity), Some(snapshot)) = (selected_identity, current) {
                self.selected_job_key = resident_trace(snapshot)
                    .and_then(find_jobs)
                    .and_then(|jobs| jobs.iter().find(|job| profile_job_matches(job, identity)))
                    .map(job_identity);
            }
        }
        if let Some(range) = self.timeline_range {
            let range = zoom.map_or(range, |factor| range.zoom(factor));
            self.timeline_range = Some(pan.map_or(range, |fraction| range.pan(fraction)));
        }
        if export {
            let capture = if self.paused {
                self.frozen_snapshot.clone()
            } else {
                self.freeze(snapshot)
            };
            self.exporter
                .request(capture.ok_or("snapshot_unavailable")?)?;
            self.export_status = Some("Export queued; up to 16 MiB, background write".into());
        }
        Ok(())
    }

    fn draw_header(&mut self, ui: &mut egui::Ui, snapshot: Option<&DeveloperSnapshot>) {
        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.enabled, "Collect profiler").changed() && !self.enabled {
                self.latest_profile = None;
            }
            if ui
                .checkbox(&mut self.paused, "Freeze snapshot + history")
                .changed()
            {
                self.frozen_snapshot = if self.paused {
                    self.freeze(snapshot)
                } else {
                    None
                };
            }
            ui.checkbox(&mut self.fill_window, "Fill window");
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Reset maxima").clicked() {
                    self.maxima = [None; 11];
                }
                if ui.small_button("Clear history").clicked() {
                    self.history.clear();
                    self.selected_frame = None;
                }
            });
        });
        ui.small(format!(
            "{} frame samples · profile {} · frame {}{}",
            self.history.len(),
            if self.enabled { "on" } else { "off" },
            self.latest_frame
                .map_or_else(|| "—".into(), |frame| frame.to_string()),
            if self.paused { " · frozen" } else { "" }
        ));
    }

    fn draw_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
                (Tab::Overview, "Overview"),
                (Tab::Timeline, "Timeline"),
                (Tab::Terrain, "Terrain"),
                (Tab::Jobs, "Jobs"),
                (Tab::Gpu, "GPU"),
                (Tab::Captures, "Captures"),
            ] {
                ui.selectable_value(&mut self.tab, tab, label);
            }
        });
    }

    fn draw_overview(&mut self, ui: &mut egui::Ui, snapshot: Option<&DeveloperSnapshot>) {
        let Some(snapshot) = snapshot else {
            ui.label("Waiting for a developer snapshot.");
            return;
        };
        let current = FrameSample::from_snapshot(snapshot);
        ui.horizontal(|ui| {
            ui.strong("Active backend");
            ui.colored_label(
                Color32::from_rgb(105, 190, 235),
                display_or_unknown(&snapshot.terrain.backend),
            );
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.frame_target_ms)
                        .range(1.0..=100.0)
                        .speed(0.25)
                        .suffix(" ms frame target"),
                );
            });
        });
        ui.small("Frame target is configurable. The separate 100 ms host-frame stall bound is not a frame-rate target.");
        if let Some(frame) = resident_value(snapshot)
            .and_then(|resident| resident.pointer("/latest_native_frame/frame"))
            .and_then(Value::as_u64)
        {
            ui.small(format!(
                "Latest completed CPU sample: frame {frame}. GPU query results arrive asynchronously."
            ));
        }
        metric_table(
            ui,
            &self.history,
            current,
            &self.maxima,
            self.frame_target_ms,
        );
        ui.separator();
        egui::Grid::new("performance-lab-overview")
            .num_columns(2)
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                value_row(ui, "Frame", snapshot.general.frame_number.to_string());
                value_row(
                    ui,
                    "Terrain backend",
                    display_or_unknown(&snapshot.terrain.backend),
                );
                value_row(ui, "Terrain state", snapshot.terrain.status().to_owned());
                value_row(
                    ui,
                    "Worker lanes",
                    format!(
                        "{} busy / {}",
                        snapshot.work.worker_busy_count, snapshot.work.worker_count
                    ),
                );
                value_row(
                    ui,
                    "Pending requests",
                    snapshot.work.pending_requests.to_string(),
                );
                value_row(
                    ui,
                    "Terrain CPU memory",
                    format_bytes(snapshot.memory.used_bytes),
                );
                value_row(
                    ui,
                    "Terrain memory cap",
                    format_bytes(snapshot.memory.cap_bytes),
                );
                value_row(
                    ui,
                    "Content upload",
                    snapshot
                        .performance
                        .upload_bytes
                        .map(format_bytes)
                        .unwrap_or_else(|| "Unavailable".into()),
                );
            });
        if snapshot.memory.near_cap() {
            ui.colored_label(
                Color32::from_rgb(239, 179, 79),
                "Terrain memory accounting is near its configured cap.",
            );
        }
        if !snapshot.warnings.is_empty() {
            egui::CollapsingHeader::new(format!(
                "Diagnostics · {} warnings",
                snapshot.warnings.len()
            ))
            .show(ui, |ui| {
                for warning in &snapshot.warnings {
                    ui.label(warning);
                }
            });
        }
    }

    fn draw_timeline(
        &mut self,
        ui: &mut egui::Ui,
        profile: Option<&Value>,
        snapshot: Option<&DeveloperSnapshot>,
    ) {
        ui.label("Host elapsed stages and completed GPU execution are distinct measurements.");
        let selected = self
            .selected_frame
            .and_then(|frame| {
                self.history
                    .iter()
                    .find(|sample| sample.frame == frame)
                    .copied()
            })
            .or_else(|| self.history.back().copied());
        if let Some(sample) = selected {
            ui.horizontal(|ui| {
                ui.label(format!("Selected frame {}", sample.frame));
                if ui.small_button("Latest").clicked() {
                    self.selected_frame = None;
                }
            });
            egui::CollapsingHeader::new("Selected frame measurements").show(ui, |ui| {
                for metric in Metric::ALL {
                    let value = sample.get(metric);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(metric.label()).strong());
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            ui.label(value.map(format_ms).unwrap_or_else(|| "Unavailable".into()));
                        });
                    });
                }
            });
        } else {
            ui.label("No samples retained yet.");
        }
        if let Some(performance) = snapshot.map(|snapshot| &snapshot.performance) {
            ui.small(format!(
                "GPU measurements: frame {} · terrain {} · source submission {} · scope {}. GPU durations are separate from this CPU clock axis.",
                performance.gpu_frame_ms.map(format_ms).unwrap_or_else(|| "Unavailable".into()),
                performance.gpu_terrain_ms.map(format_ms).unwrap_or_else(|| "Unavailable".into()),
                performance.gpu_source_frame.map_or_else(|| "Unavailable".into(), |frame| frame.to_string()),
                display_or_unknown(&performance.gpu_timing_scope),
            ));
        }
        ui.horizontal(|ui| {
            if ui.small_button("Select slowest retained frame").clicked() {
                self.selected_frame = self
                    .history
                    .iter()
                    .filter_map(|sample| {
                        sample
                            .get(Metric::HostFrame)
                            .map(|value| (sample.frame, value))
                    })
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(frame, _)| frame);
                self.timeline_range = None;
            }
            ui.checkbox(&mut self.filter_frame, "Only selected frame + parents");
            if ui.small_button("Export timeline + AI summary").clicked() {
                let _ = self
                    .configure(
                        snapshot,
                        ProfilerControls {
                            export: true,
                            ..Default::default()
                        },
                    )
                    .map_err(|error| self.export_status = Some(error.into()));
            }
        });
        if let Some(status) = &self.export_status {
            ui.small(status);
        }
        self.draw_chart(ui);
        // Rolling aggregates remain available without pushing worker lanes below the window.
        draw_profile_stats(ui, profile);
        let timeline_index = profile.map(|profile| self.timeline_index_for(profile));
        let indexed_profile = profile
            .zip(timeline_index.as_deref())
            .map(|(data, index)| IndexedTimelineProfile { data, index });
        let selected_frame = self.selected_frame;
        egui::CollapsingHeader::new("Worker lanes")
            .default_open(true)
            .show(ui, |ui| {
                draw_lanes(
                    ui,
                    indexed_profile,
                    selected_frame,
                    self.filter_frame,
                    &mut self.timeline_range,
                    &mut self.selected_event_id,
                    &mut self.last_axis_width,
                )
            });
        if let (Some(profile), Some(snapshot), Some(id)) =
            (profile, snapshot, self.selected_event_id)
        {
            let event = profile["lanes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|lane| lane["events"].as_array())
                .flatten()
                .find(|event| json_u64(event, "event_id") == Some(id));
            if let Some(identity) = event
                .and_then(|event| event.get("job_identity"))
                .filter(|identity| !identity.is_null())
            {
                draw_correlated_job(ui, identity, snapshot);
            }
        }
        if !self.history.is_empty() {
            let mut index = self
                .selected_frame
                .and_then(|frame| self.history.iter().position(|sample| sample.frame == frame))
                .unwrap_or(self.history.len() - 1);
            let response =
                ui.add(egui::Slider::new(&mut index, 0..=self.history.len() - 1).text("Frame"));
            if response.changed() {
                self.selected_frame = self.history.get(index).map(|sample| sample.frame);
            }
        }
    }

    fn draw_chart(&mut self, ui: &mut egui::Ui) {
        let desired = Vec2::new(ui.available_width().max(240.0), 156.0);
        let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, Color32::from_rgb(22, 27, 34));
        let chart = rect.shrink2(Vec2::new(36.0, 14.0));
        let max_value = self
            .history
            .iter()
            .flat_map(|sample| sample.values.iter().flatten().copied())
            .fold(self.frame_target_ms * 1.2, f64::max)
            .max(1.0);
        let y_at = |value: f64| {
            chart.bottom() - (value / max_value).clamp(0.0, 1.0) as f32 * chart.height()
        };
        for fraction in [0.0, 0.5, 1.0] {
            let y = chart.bottom() - chart.height() * fraction;
            painter.line_segment(
                [Pos2::new(chart.left(), y), Pos2::new(chart.right(), y)],
                Stroke::new(1.0_f32, Color32::from_gray(55)),
            );
        }
        let budget_y = y_at(self.frame_target_ms);
        painter.line_segment(
            [
                Pos2::new(chart.left(), budget_y),
                Pos2::new(chart.right(), budget_y),
            ],
            Stroke::new(1.0_f32, Color32::from_rgb(233, 164, 69)),
        );
        painter.text(
            Pos2::new(rect.left() + 5.0, budget_y - 8.0),
            egui::Align2::LEFT_BOTTOM,
            format!("target {:.2} ms", self.frame_target_ms),
            egui::FontId::monospace(9.0),
            Color32::from_rgb(233, 164, 69),
        );
        if HOST_STALL_BOUND_MS <= max_value {
            let bound_y = y_at(HOST_STALL_BOUND_MS);
            painter.line_segment(
                [
                    Pos2::new(chart.left(), bound_y),
                    Pos2::new(chart.right(), bound_y),
                ],
                Stroke::new(1.0_f32, Color32::from_rgb(196, 76, 77)),
            );
            painter.text(
                Pos2::new(rect.right() - 4.0, bound_y - 7.0),
                egui::Align2::RIGHT_BOTTOM,
                "100 ms host stall bound",
                egui::FontId::monospace(9.0),
                Color32::from_rgb(230, 120, 112),
            );
        } else {
            painter.text(
                Pos2::new(rect.right() - 4.0, chart.top() + 2.0),
                egui::Align2::RIGHT_TOP,
                "100 ms host bound · off scale",
                egui::FontId::monospace(9.0),
                Color32::from_rgb(230, 120, 112),
            );
        }
        let series = [
            (Metric::HostFrame, Color32::from_rgb(229, 91, 83)),
            (Metric::TerrainUpdate, Color32::from_rgb(88, 174, 239)),
            (Metric::Publication, Color32::from_rgb(135, 202, 133)),
            (Metric::GpuFrame, Color32::from_rgb(190, 139, 230)),
        ];
        for (metric, color) in series {
            let mut segment = Vec::new();
            for (index, sample) in self.history.iter().enumerate() {
                if let Some(value) = sample.get(metric) {
                    let x = chart.left()
                        + chart.width() * index as f32 / (self.history.len() - 1).max(1) as f32;
                    segment.push(Pos2::new(x, y_at(value)));
                } else {
                    paint_metric_segment(&painter, &mut segment, color);
                }
            }
            paint_metric_segment(&painter, &mut segment, color);
        }
        if response.clicked()
            && !self.history.is_empty()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let fraction = ((pointer.x - chart.left()) / chart.width()).clamp(0.0, 1.0);
            let index = (fraction * (self.history.len() - 1) as f32).round() as usize;
            self.selected_frame = self.history.get(index).map(|sample| sample.frame);
        }
        ui.horizontal(|ui| {
            for (metric, color) in series {
                ui.colored_label(color, metric.label());
            }
        });
    }

    fn draw_terrain(&self, ui: &mut egui::Ui, snapshot: Option<&DeveloperSnapshot>) {
        let Some(snapshot) = snapshot else {
            ui.label("Waiting for a developer snapshot.");
            return;
        };
        let terrain = &snapshot.terrain;
        ui.horizontal(|ui| {
            ui.strong("Active backend");
            ui.colored_label(
                Color32::from_rgb(105, 190, 235),
                display_or_unknown(&terrain.backend),
            );
        });
        draw_terrain_pipeline(ui, snapshot);
        if let Some(hotspots) = resident_value(snapshot)
            .and_then(|value| value.get("visible_drawable_proxy_hotspots"))
            .and_then(Value::as_array)
        {
            egui::CollapsingHeader::new("Visible patches with largest projected error").show(
                ui,
                |ui| {
                    ui.small(
                        "Approximate selector error; inspect a patch to follow its refinement.",
                    );
                    for hotspot in hotspots.iter().take(8) {
                        let address = hotspot
                            .get("address")
                            .and_then(Value::as_str)
                            .unwrap_or("Unknown patch");
                        let error = hotspot.get("error_px").and_then(Value::as_f64);
                        let short_address = address
                            .trim_start_matches("CubePatchAddress { face: ")
                            .trim_end_matches(" }")
                            .replace("level: ", "LOD ");
                        let label = format!(
                            "{} · {} px",
                            short_address,
                            error.map_or_else(|| "—".into(), |value| format!("{value:.3}"))
                        );
                        egui::CollapsingHeader::new(label).show(ui, |ui| {
                            egui::Grid::new(("visible-patch-inspector", address))
                                .num_columns(2)
                                .show(ui, |ui| {
                                    for (label, field) in [
                                        ("Parent", "parent"),
                                        ("Desired leaf", "desired"),
                                        ("Resident", "resident"),
                                        ("CPU cached", "cpu_cached"),
                                        ("Upload queued", "upload_queued"),
                                        ("Build in flight", "in_flight"),
                                        ("Local blocker", "blocker_status"),
                                        ("Exact tile identity", "key"),
                                    ] {
                                        let value = hotspot.get(field).map_or_else(
                                            || "Unavailable".into(),
                                            |value| {
                                                if field == "key" {
                                                    format!(
                                                        "Body {} · surface {} · material {} · {} cells",
                                                        display_u64(value, "body_identity"),
                                                        display_u64(value, "surface_revision"),
                                                        display_u64(value, "material_revision"),
                                                        display_u64(value, "cells")
                                                    )
                                                } else {
                                                    value.to_string()
                                                }
                                            },
                                        );
                                        value_row(ui, label, value);
                                    }
                                });
                        });
                    }
                },
            );
        }
        egui::Grid::new("performance-lab-terrain")
            .num_columns(2)
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                value_row(ui, "Backend", display_or_unknown(&terrain.backend));
                value_row(
                    ui,
                    "Active body",
                    terrain
                        .active_body
                        .as_ref()
                        .map(|body| body.name.clone())
                        .unwrap_or_else(|| "None".into()),
                );
                value_row(
                    ui,
                    "Generator",
                    terrain
                        .generator_algorithm
                        .as_deref()
                        .unwrap_or("Unavailable")
                        .into(),
                );
                value_row(
                    ui,
                    "Certificate",
                    terrain
                        .certificate_kind
                        .as_deref()
                        .unwrap_or("Unavailable")
                        .into(),
                );
                value_row(
                    ui,
                    "Radial LOD",
                    format!(
                        "source {:?} · ready {:?} · desired {:?}",
                        terrain.source_radial_lod,
                        terrain.ready_radial_lod,
                        terrain.desired_radial_lod
                    ),
                );
                value_row(
                    ui,
                    "Leaves",
                    format!(
                        "{} visible / {} source",
                        terrain.visible_leaf_count, terrain.source_leaf_count
                    ),
                );
                value_row(ui, "State", terrain.status().into());
                value_row(ui, "Settled", option_bool(terrain.settled).into());
                value_row(
                    ui,
                    "Quality pending",
                    option_bool(terrain.quality_pending).into(),
                );
                value_row(
                    ui,
                    "Construction",
                    bool_word(terrain.construction_pending).into(),
                );
                value_row(
                    ui,
                    "Budget constrained",
                    bool_word(terrain.budget_constrained).into(),
                );
                value_row(
                    ui,
                    "Transition deferred",
                    bool_word(terrain.transition_deferred).into(),
                );
                value_row(
                    ui,
                    "Resident patches",
                    snapshot.work.raw_resident_patches.to_string(),
                );
                value_row(
                    ui,
                    "Accounted memory",
                    format!(
                        "{} / {}",
                        format_bytes(snapshot.memory.used_bytes),
                        format_bytes(snapshot.memory.cap_bytes)
                    ),
                );
                value_row(
                    ui,
                    "Upload this frame",
                    snapshot
                        .performance
                        .upload_bytes
                        .map(format_bytes)
                        .unwrap_or_else(|| "Unavailable".into()),
                );
            });
        egui::CollapsingHeader::new("Raw backend telemetry · advanced").show(ui, |ui| {
            for (name, value) in [
                ("Planetary resident", snapshot.resident_planetary.as_deref()),
                ("Regional resident", snapshot.resident_regional.as_ref()),
                ("Hierarchy resident", snapshot.resident_hierarchy.as_ref()),
                ("Tile fixture", snapshot.resident_tile.as_ref()),
            ] {
                if let Some(value) = value {
                    ui.label(RichText::new(name).strong());
                    ui.monospace(truncate(&value.to_string(), 900));
                }
            }
        });
    }

    fn draw_jobs(&mut self, ui: &mut egui::Ui, snapshot: Option<&DeveloperSnapshot>) {
        let Some(snapshot) = snapshot else {
            ui.label("Waiting for terrain job telemetry.");
            return;
        };
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} busy lanes · {} queued requests",
                snapshot.work.worker_busy_count, snapshot.work.pending_requests
            ));
            ui.label(format!(
                "{} raw resident patches",
                snapshot.work.raw_resident_patches
            ));
        });
        let trace = resident_trace(snapshot);
        let jobs = trace.and_then(find_jobs);
        if let Some(jobs) = jobs {
            if jobs.is_empty() {
                ui.label("No terrain jobs in the current telemetry window.");
            } else {
                if let Some(key) = self.selected_job_key.clone() {
                    self.selected_job = jobs.iter().position(|job| job_identity(job) == key);
                }
                let trace_time_us = trace.and_then(trace_generated_time_us);
                egui::CollapsingHeader::new(format!("Retained job table · {} jobs", jobs.len()))
                    .show(ui, |ui| {
                        egui::Grid::new("performance-lab-jobs")
                            .striped(true)
                            .num_columns(5)
                            .show(ui, |ui| {
                                for heading in [
                                    "Stage",
                                    "Tile address",
                                    "Surface rev",
                                    "Material rev",
                                    "Age at snapshot / blockers",
                                ] {
                                    ui.strong(heading);
                                }
                                ui.end_row();
                                for (index, job) in jobs.iter().enumerate() {
                                    let state = job_stage(job).unwrap_or("Requested");
                                    let address = format_address(job.get("address"));
                                    let surface_revision = display_u64(job, "surface_revision");
                                    let material_revision = display_u64(job, "material_revision");
                                    let age = format_optional_ms(job_age_ms(job, trace_time_us));
                                    let blockers = format_blockers(job.get("blocked_by"));
                                    if ui
                                        .selectable_label(self.selected_job == Some(index), state)
                                        .clicked()
                                    {
                                        self.selected_job = Some(index);
                                        self.selected_job_key = Some(job_identity(job));
                                    }
                                    ui.label(address);
                                    ui.label(surface_revision);
                                    ui.label(material_revision);
                                    ui.label(truncate(&format!("{age} · {blockers}"), 160));
                                    ui.end_row();
                                }
                            });
                    });
                if let Some(index) = self
                    .selected_job_key
                    .as_ref()
                    .and_then(|key| jobs.iter().find(|job| job_identity(job) == *key))
                {
                    egui::CollapsingHeader::new("Selected job fields")
                        .default_open(true)
                        .show(ui, |ui| {
                            draw_job_inspector(ui, index, trace_time_us);
                        });
                }
            }
        } else {
            ui.label("Detailed jobs are not available in this snapshot.");
        }
        if let Some(queues) = trace.and_then(|trace| trace.get("queues")) {
            egui::CollapsingHeader::new("Queues")
                .default_open(true)
                .show(ui, |ui| {
                    if let Some(items) = queues.as_array() {
                        egui::Grid::new("performance-lab-job-queues")
                            .striped(true)
                            .num_columns(4)
                            .show(ui, |ui| {
                                for heading in ["Stage", "Count", "Oldest", "Throughput"] {
                                    ui.strong(heading);
                                }
                                ui.end_row();
                                for item in items {
                                    ui.label(
                                        field_text(item, &["queue", "stage", "name"])
                                            .unwrap_or("—"),
                                    );
                                    ui.label(display_count(item));
                                    ui.label(display_age(item));
                                    ui.label(display_rate(item));
                                    ui.end_row();
                                }
                            });
                    } else {
                        ui.label("Queue records use an unsupported shape.");
                    }
                });
        }
        if let Some(events) = trace.and_then(|trace| trace.get("events")) {
            egui::CollapsingHeader::new("Recent terrain events").show(ui, |ui| {
                if let Some(events) = events.as_array() {
                    egui::Grid::new("performance-lab-job-events")
                        .striped(true)
                        .num_columns(4)
                        .show(ui, |ui| {
                            for heading in ["Time", "Stage / blocker", "Tile address", "Change"] {
                                ui.strong(heading);
                            }
                            ui.end_row();
                            for event in events.iter().rev().take(24).rev() {
                                ui.label(
                                    json_u64(event, "time_us")
                                        .map(|us| format_optional_ms(Some(us as f64 / 1000.0)))
                                        .unwrap_or_else(|| "—".into()),
                                );
                                let stage = field_text(event, &["stage"]).unwrap_or("—");
                                let blocker = field_text(event, &["block"]).unwrap_or("");
                                ui.label(if blocker.is_empty() {
                                    stage.into()
                                } else {
                                    format!("{stage} · {blocker}")
                                });
                                ui.label(format_address(event.get("address")));
                                ui.label(if event["unblocked"].as_bool() == Some(true) {
                                    "unblocked"
                                } else if !blocker.is_empty() {
                                    "blocked"
                                } else {
                                    "milestone"
                                });
                                ui.end_row();
                            }
                        });
                }
            });
        }
    }

    fn draw_gpu(&self, ui: &mut egui::Ui, snapshot: Option<&DeveloperSnapshot>) {
        let Some(snapshot) = snapshot else {
            ui.label("Waiting for a GPU timing snapshot.");
            return;
        };
        let performance = &snapshot.performance;
        egui::Grid::new("performance-lab-gpu")
            .num_columns(2)
            .spacing([18.0, 4.0])
            .show(ui, |ui| {
                value_row(
                    ui,
                    "GPU frame execution",
                    performance
                        .gpu_frame_ms
                        .map(format_ms)
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "GPU terrain execution",
                    performance
                        .gpu_terrain_ms
                        .map(format_ms)
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "GPU fallback execution",
                    performance
                        .gpu_transition_fallback_ms
                        .map(format_ms)
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "GPU source submission",
                    performance
                        .gpu_source_frame
                        .map(|frame| frame.to_string())
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "GPU timing scope",
                    display_or_unknown(&performance.gpu_timing_scope),
                );
                value_row(
                    ui,
                    "GPU terrain prep (CPU)",
                    performance
                        .gpu_preparation_cpu_ms
                        .map(format_ms)
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "Render + present (host)",
                    performance
                        .render_present_ms
                        .map(format_ms)
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "Upload this frame",
                    performance
                        .upload_bytes
                        .map(format_bytes)
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "Terrain backend",
                    display_or_unknown(&snapshot.terrain.backend),
                );
            });
        ui.small(format!("GPU measurement age: {} submissions; readback arrival timestamp unavailable. CPU/GPU clocks are uncalibrated.",
            performance.gpu_source_frame.and_then(|frame| performance.native_submission_id.and_then(|current| current.checked_sub(frame)))
                .map_or_else(|| "Unavailable".into(), |age| age.to_string())));
        ui.separator();
        ui.label("GPU timings are reported only when the renderer provides a completed measurement. Host preparation and render/present elapsed time are not GPU execution time.");
        ui.colored_label(Color32::from_rgb(151, 166, 183), "Adapter memory and VRAM budget: unavailable unless a separate renderer telemetry source is added.");
    }

    fn draw_captures(&mut self, ui: &mut egui::Ui, snapshot: Option<&DeveloperSnapshot>) {
        ui.label("Capture requests are handed to the application; this panel writes no files.");
        if ui.button("Request paired snapshot + PNG").clicked() {
            self.capture_requested = true;
        }
        if ui.button("Stop automatic capture").clicked() {
            self.capture_stop_requested = true;
        }
        if let Some(snapshot) = snapshot {
            if let Some(capture) = &snapshot.capture {
                egui::Grid::new("performance-lab-capture")
                    .num_columns(2)
                    .show(ui, |ui| {
                        value_row(ui, "Scene", capture.scene.clone());
                        value_row(ui, "Image", capture.image.clone());
                        value_row(
                            ui,
                            "Dimensions",
                            format!("{} × {}", capture.width, capture.height),
                        );
                        value_row(ui, "Terrain updates", capture.terrain_updates.to_string());
                        value_row(ui, "Workers", capture.worker_count.to_string());
                        value_row(ui, "Adapter", capture.adapter.clone());
                        value_row(ui, "Backend", capture.backend.clone());
                    });
            } else {
                ui.label("No capture metadata in the current snapshot.");
            }
            if ui.button("Copy current diagnostic JSON").clicked()
                && let Ok(json) = serde_json::to_string_pretty(snapshot)
            {
                ui.ctx().copy_text(json);
            }
        } else {
            ui.label("Waiting for a diagnostic snapshot.");
        }
    }
}

fn paint_metric_segment(painter: &egui::Painter, points: &mut Vec<Pos2>, color: Color32) {
    if points.len() >= 2 {
        painter.add(egui::Shape::line(
            points.clone(),
            Stroke::new(1.7_f32, color),
        ));
    } else if let Some(point) = points.first() {
        painter.circle_filled(*point, 2.0, color);
    }
    points.clear();
}

fn draw_terrain_pipeline(ui: &mut egui::Ui, snapshot: &DeveloperSnapshot) {
    let Some(resident) = resident_value(snapshot) else {
        ui.label("Terrain pipeline queues unavailable.");
        return;
    };
    let trace = resident_trace(snapshot);
    let queues = trace.and_then(|trace| trace.get("queues"));
    let publication = resident.get("publication_pipeline");

    let generation = queue_measure(queues, &["generation_queued", "generation_active"]);
    let boundary = queue_measure(
        queues,
        &[
            "generated_awaiting_boundary",
            "boundary_queued",
            "boundary_active",
        ],
    );
    let prepared = queue_measure(queues, &["prepared"]);
    let generation_blocked = queue_measure(queues, &["generation_blocked"]);
    let publication_blocked = queue_measure(queues, &["publication_blocked"]);
    let publish = queue_measure(queues, &["publishable", "publication_active"]);

    let trace_generation_count = trace.and_then(|trace| {
        Some(
            json_u64(trace, "generation_queued")?
                .saturating_add(json_u64(trace, "generation_running").unwrap_or(0)),
        )
    });
    let generation = generation.with_fallback(
        trace_generation_count,
        trace
            .and_then(|trace| json_u64(trace, "oldest_pending_us"))
            .map(|micros| micros as f64 / 1000.0),
        trace
            .and_then(|trace| json_u64(trace, "generation_p95_us"))
            .map(|micros| micros as f64 / 1000.0),
        trace
            .and_then(|trace| trace.get("completed_rate_per_second"))
            .and_then(Value::as_f64),
    );

    let boundary_fallback_count = publication.and_then(|value| {
        Some(
            json_u64(value, "boundary_preparation_queued_batches")?
                .saturating_add(json_u64(value, "boundary_preparation_active_batches").unwrap_or(0))
                .saturating_add(
                    json_u64(
                        value,
                        "boundary_preparation_completed_batches_awaiting_drain",
                    )
                    .unwrap_or(0),
                ),
        )
    });
    let boundary = boundary.with_fallback(boundary_fallback_count, None, None, None);

    let prepared = prepared.with_fallback(
        publication.and_then(|value| json_u64(value, "fully_prepared_results")),
        None,
        None,
        None,
    );
    let publish = publish.with_fallback(
        publication
            .and_then(|value| json_u64(value, "immediately_publishable_results"))
            .or_else(|| publication.and_then(|value| json_u64(value, "publication_candidates"))),
        publication.and_then(|value| {
            json_number(
                value,
                &["oldest_backlog_age_ms", "publication_backlog_age_ms"],
            )
        }),
        publication.and_then(|value| json_number(value, &["publication_p95_ms", "p95_ms"])),
        publication.and_then(|value| {
            json_number(
                value,
                &["publication_rate_per_second", "throughput_per_second"],
            )
        }),
    );

    ui.label(RichText::new("Terrain pipeline").strong());
    ui.label("Queue age and duration are milliseconds; throughput is completed items per second.");
    egui::Grid::new("performance-lab-terrain-pipeline")
        .striped(true)
        .num_columns(5)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for heading in [
                "Queue stage",
                "Count",
                "Oldest age",
                "p95 age",
                "Completions / s",
            ] {
                ui.strong(heading);
            }
            ui.end_row();
            for (label, measure) in [
                ("Generation queues", generation),
                ("Boundary queues", boundary),
                ("Prepared queue", prepared),
                ("Generation blocked queue", generation_blocked),
                ("Publication blocked queue", publication_blocked),
                ("Publication queues", publish),
            ] {
                ui.label(label);
                ui.label(format_optional_count(measure.count));
                ui.label(format_optional_ms(measure.oldest_ms));
                ui.label(format_optional_ms(measure.p95_ms));
                ui.label(
                    measure
                        .throughput_per_second
                        .map(|rate| format!("{rate:.2} / s"))
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                ui.end_row();
            }
        });
}

#[derive(Clone, Copy, Default)]
struct QueueMeasure {
    count: Option<u64>,
    oldest_ms: Option<f64>,
    p95_ms: Option<f64>,
    throughput_per_second: Option<f64>,
}

impl QueueMeasure {
    fn with_fallback(
        self,
        count: Option<u64>,
        oldest_ms: Option<f64>,
        p95_ms: Option<f64>,
        throughput_per_second: Option<f64>,
    ) -> Self {
        Self {
            count: self.count.or(count),
            oldest_ms: self.oldest_ms.or(oldest_ms),
            p95_ms: self.p95_ms.or(p95_ms),
            throughput_per_second: self.throughput_per_second.or(throughput_per_second),
        }
    }
}

fn queue_measure(queues: Option<&Value>, stages: &[&str]) -> QueueMeasure {
    let Some(queues) = queues else {
        return QueueMeasure::default();
    };
    let matching: Vec<&Value> = match queues {
        Value::Array(items) => items
            .iter()
            .filter(|item| {
                field_text(item, &["queue", "stage", "name"])
                    .is_some_and(|name| stages.iter().any(|stage| name.eq_ignore_ascii_case(stage)))
            })
            .collect(),
        Value::Object(items) => stages
            .iter()
            .filter_map(|stage| items.get(*stage))
            .collect(),
        _ => Vec::new(),
    };
    if matching.is_empty() {
        return QueueMeasure::default();
    }
    let count = matching
        .iter()
        .filter_map(|queue| json_number(queue, &["count", "queued", "pending", "length", "items"]))
        .map(|count| count.max(0.0) as u64)
        .sum();
    let oldest_ms = matching
        .iter()
        .filter_map(|queue| {
            json_number(
                queue,
                &["oldest_ms", "oldest_age_ms", "oldest_backlog_age_ms"],
            )
            .or_else(|| {
                json_number(queue, &["oldest_us", "oldest_age_us"]).map(|value| value / 1000.0)
            })
        })
        .reduce(f64::max);
    let p95_ms = matching
        .iter()
        .filter_map(|queue| {
            json_number(queue, &["p95_ms", "p95_age_ms", "duration_p95_ms"]).or_else(|| {
                json_number(queue, &["p95_us", "duration_p95_us"]).map(|value| value / 1000.0)
            })
        })
        .reduce(f64::max);
    // Serial queue stages count the same work as it moves through the pipeline.
    // Show the exit-stage rate rather than adding those duplicate completions.
    let throughput_per_second = stages.iter().rev().find_map(|stage| {
        let queue = match queues {
            Value::Array(items) => items.iter().find(|item| {
                field_text(item, &["queue", "stage", "name"])
                    .is_some_and(|name| name.eq_ignore_ascii_case(stage))
            }),
            Value::Object(items) => items.get(*stage),
            _ => None,
        }?;
        json_number(
            queue,
            &[
                "throughput_per_second",
                "completed_per_second",
                "rate_per_second",
            ],
        )
    });
    QueueMeasure {
        count: Some(count),
        oldest_ms,
        p95_ms,
        throughput_per_second,
    }
}

fn metric_table(
    ui: &mut egui::Ui,
    history: &VecDeque<FrameSample>,
    current: FrameSample,
    maxima: &[Option<f64>; 11],
    frame_target_ms: f64,
) {
    egui::Grid::new("performance-lab-metrics")
        .striped(true)
        .num_columns(6)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for title in ["Stage", "Current", "p50", "p95", "Max", "Budget"] {
                ui.strong(title);
            }
            ui.end_row();
            for (index, metric) in Metric::ALL.iter().enumerate() {
                let values = history
                    .iter()
                    .filter_map(|sample| sample.get(*metric))
                    .collect::<Vec<_>>();
                ui.label(metric.label());
                ui.label(
                    current
                        .get(*metric)
                        .map(format_ms)
                        .unwrap_or_else(|| "—".into()),
                );
                ui.label(
                    quantile(&values, 0.50)
                        .map(format_ms)
                        .unwrap_or_else(|| "—".into()),
                );
                ui.label(
                    quantile(&values, 0.95)
                        .map(format_ms)
                        .unwrap_or_else(|| "—".into()),
                );
                ui.label(maxima[index].map(format_ms).unwrap_or_else(|| "—".into()));
                ui.label(
                    metric
                        .budget_ms(frame_target_ms)
                        .map(format_ms)
                        .unwrap_or_else(|| "—".into()),
                );
                ui.end_row();
            }
        });
}

fn draw_profile_stats(ui: &mut egui::Ui, profile: Option<&Value>) {
    let Some(stats) = profile
        .and_then(|profile| profile.get("stats"))
        .and_then(Value::as_array)
    else {
        ui.label("Profiler rolling statistics unavailable.");
        return;
    };
    if stats.is_empty() {
        ui.label("No completed profiler spans in the retained sample window.");
        return;
    }
    egui::CollapsingHeader::new("Profiler rolling spans · nanoseconds converted to milliseconds")
        .default_open(false)
        .show(ui, |ui| {
            egui::Grid::new("performance-lab-profile-stats")
                .striped(true)
                .num_columns(7)
                .show(ui, |ui| {
                    for heading in ["Span", "Current", "p50", "p95", "Max", "Budget", "Samples"] {
                        ui.strong(heading);
                    }
                    ui.end_row();
                    for stat in stats {
                        ui.label(field_text(stat, &["name"]).unwrap_or("unnamed"));
                        for field in ["current_ns", "p50_ns", "p95_ns", "max_ns", "budget_ns"] {
                            ui.label(
                                json_u64(stat, field)
                                    .map(|ns| format_ms(ns as f64 / 1_000_000.0))
                                    .unwrap_or_else(|| "—".into()),
                            );
                        }
                        ui.label(
                            json_u64(stat, "sample_count")
                                .map(|count| count.to_string())
                                .unwrap_or_else(|| "—".into()),
                        );
                        ui.end_row();
                    }
                });
        });
}

fn timeline_gesture(
    ui: &mut egui::Ui,
    response: &egui::Response,
    axis: egui::Rect,
    range: TimelineRange,
    timeline_range: &mut Option<TimelineRange>,
) {
    if response.dragged() {
        let delta = ui.input(|input| input.pointer.delta().x);
        *timeline_range = Some(range.pan(-delta as f64 / axis.width().max(1.0) as f64));
    }
    if let Some(pointer) = ui.input(|input| input.pointer.hover_pos())
        && axis.contains(pointer)
        && ui.clip_rect().contains(pointer)
    {
        let scroll = ui.input(|input| input.smooth_scroll_delta);
        if scroll.y.abs() > 0.0 || scroll.x.abs() > 0.0 {
            let fraction = ((pointer.x - axis.left()) / axis.width().max(1.0)) as f64;
            let next = range.zoom_at((-scroll.y as f64 * 0.01).exp(), fraction);
            *timeline_range = Some(next.pan(-scroll.x as f64 / axis.width().max(1.0) as f64));
            ui.input_mut(|input| {
                input.smooth_scroll_delta = Vec2::ZERO;
                input.raw_scroll_delta = Vec2::ZERO;
            });
        }
    }
}

fn draw_lanes(
    ui: &mut egui::Ui,
    profile: Option<IndexedTimelineProfile<'_>>,
    frame_id: Option<u64>,
    filter_frame: bool,
    timeline_range: &mut Option<TimelineRange>,
    selected_event_id: &mut Option<u64>,
    last_axis_width: &mut Option<f32>,
) {
    let Some(profile) = profile else {
        ui.label("Profiler lane data unavailable.");
        return;
    };
    let timeline_index = profile.index;
    let profile = profile.data;
    let lanes = profile
        .get("lanes")
        .or_else(|| profile.pointer("/profile/lanes"));
    let Some(lanes) = lanes else {
        ui.monospace(truncate(&profile.to_string(), 1200));
        return;
    };
    let iterable: Vec<(String, &Value)> = match lanes {
        Value::Array(values) => values
            .iter()
            .enumerate()
            .map(|(index, value)| (worker_label(value, index), value))
            .collect(),
        Value::Object(values) => values
            .iter()
            .map(|(name, value)| (name.clone(), value))
            .collect(),
        _ => Vec::new(),
    };
    if iterable.is_empty() {
        ui.label("No worker lanes in the current profile.");
        return;
    }
    let generated_at_ns = timeline_index.generated_at_ns;
    let retained_bounds = timeline_index.retained_bounds;
    let selected_bounds =
        frame_id.and_then(|frame| timeline_index.frame_bounds.get(&frame).copied());
    let fallback = TimelineRange::from_bounds(
        generated_at_ns.saturating_sub(3_000_000_000),
        generated_at_ns,
    );
    let initial = retained_bounds.map_or(fallback, |bounds| {
        TimelineRange::from_bounds(
            bounds
                .start_ns
                .max(generated_at_ns.saturating_sub(3_000_000_000)),
            bounds.end_ns,
        )
    });
    let mut range = *timeline_range.get_or_insert(selected_bounds.unwrap_or(initial));
    let axis_width = (ui.available_width().max(240.0) - 176.0).max(1.0);
    if let Some(previous) = last_axis_width.replace(axis_width)
        && (previous - axis_width).abs() >= 1.0
    {
        range = range.resize_for_width(previous as f64, axis_width as f64);
        *timeline_range = Some(range);
    }
    ui.horizontal(|ui| {
        if ui.small_button("Fit frame").clicked()
            && let Some(frame_range) = selected_bounds
        {
            *timeline_range = Some(frame_range.zoom(1.15));
        }
        if ui.small_button("Fit retained").clicked()
            && let Some(bounds) = retained_bounds
        {
            *timeline_range = Some(bounds);
        }
        if ui.small_button("−").clicked() {
            *timeline_range = Some(range.zoom(1.5));
        }
        if ui.small_button("+").clicked() {
            *timeline_range = Some(range.zoom(0.67));
        }
        if ui.small_button("Earlier").clicked() {
            *timeline_range = Some(range.pan(-0.5));
        }
        if ui.small_button("Later").clicked() {
            *timeline_range = Some(range.pan(0.5));
        }
        let mut visible_ms = range.span_ns() as f64 / 1_000_000.0;
        if ui
            .add(
                egui::DragValue::new(&mut visible_ms)
                    .speed(0.1)
                    .range(0.1..=60_000.0)
                    .suffix(" ms visible"),
            )
            .changed()
        {
            *timeline_range = Some(range.with_span_ns(visible_ms * 1_000_000.0));
        }
    });
    let range = timeline_range.unwrap_or(range);
    if let Some(retained) = retained_bounds {
        let (overview_rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width().max(240.0), 24.0),
            Sense::hover(),
        );
        let overview = egui::Rect::from_min_max(
            Pos2::new(overview_rect.left() + 170.0, overview_rect.top()),
            Pos2::new(overview_rect.right() - 6.0, overview_rect.bottom()),
        );
        let response = ui.interact(
            overview,
            egui::Id::new("performance-lab-overview"),
            Sense::click_and_drag(),
        );
        let painter = ui.painter_at(overview_rect);
        painter.text(
            overview_rect.left_center(),
            egui::Align2::LEFT_CENTER,
            "Retained history",
            egui::FontId::proportional(11.0),
            Color32::GRAY,
        );
        painter.rect_filled(overview, 2.0, Color32::from_rgb(38, 45, 55));
        let position = |ns: u64| {
            overview.left()
                + overview.width()
                    * ((ns as f64 - retained.start_ns as f64) / retained.span_ns() as f64)
                        .clamp(0.0, 1.0) as f32
        };
        let viewport = egui::Rect::from_min_max(
            Pos2::new(position(range.start_ns), overview.top()),
            Pos2::new(
                position(range.end_ns).max(position(range.start_ns) + 2.0),
                overview.bottom(),
            ),
        );
        painter.rect_filled(viewport, 2.0, Color32::from_rgb(55, 95, 125));
        painter.rect_stroke(
            viewport,
            2.0,
            Stroke::new(1.0_f32, Color32::LIGHT_BLUE),
            egui::StrokeKind::Inside,
        );
        if (response.clicked() || response.dragged())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let fraction =
                ((pointer.x - overview.left()) / overview.width()).clamp(0.0, 1.0) as f64;
            let center = retained.start_ns as f64 + retained.span_ns() as f64 * fraction;
            let start = (center - range.span_ns() as f64 * 0.5).max(0.0) as u64;
            *timeline_range = Some(TimelineRange::from_bounds(
                start,
                start.saturating_add(range.span_ns()),
            ));
        }
        response.on_hover_text("Click or drag to move through retained history. The blue window shows the visible interval.");
    }
    ui.small(format!(
        "Completed spans only · {} sampled omissions · {} nesting omissions · {} refused lanes",
        json_u64(profile, "snapshot_events_omitted").unwrap_or(0),
        json_u64(profile, "dropped_nesting").unwrap_or(0),
        json_u64(profile, "dropped_lane_registrations").unwrap_or(0)
    ));
    ui.small(format!(
        "Process-relative CPU time · {} · blank space is unobserved time, not proof of idle or a wait",
        frame_id.map_or_else(|| "all retained frames".into(), |frame| format!("frame {frame}"))
    ));
    let (ruler, ruler_response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(240.0), 22.0),
        Sense::click_and_drag(),
    );
    let ruler = egui::Rect::from_min_max(
        Pos2::new(ruler.left() + 170.0, ruler.top()),
        Pos2::new(ruler.right() - 6.0, ruler.bottom()),
    );
    timeline_gesture(ui, &ruler_response, ruler, range, timeline_range);
    let painter = ui.painter_at(ruler);
    for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let x = ruler.left() + ruler.width() * fraction;
        painter.line_segment(
            [Pos2::new(x, ruler.top()), Pos2::new(x, ruler.bottom())],
            Stroke::new(0.5_f32, Color32::from_gray(65)),
        );
        let time_ms =
            (range.start_ns as f64 + range.span_ns() as f64 * fraction as f64) / 1_000_000.0;
        painter.text(
            Pos2::new(x, ruler.center().y),
            egui::Align2::CENTER_CENTER,
            format!("{time_ms:.1} ms"),
            egui::FontId::monospace(9.0),
            Color32::from_gray(160),
        );
    }
    ui.small("Wheel over lanes to zoom at cursor · drag to pan · double-click a span to fit it");
    ui.small("Tiny spans are grouped into density cells; zoom in to inspect individual spans.");

    let frame_starts = &timeline_index.frame_starts;
    let mut selected_event = None;
    for (lane_index, (name, lane)) in iterable.into_iter().enumerate() {
        let events = lane.get("events").and_then(Value::as_array);
        let Some(lane_events) = events else {
            continue;
        };
        // The profiler records depth, so lane sizing does not need to build a
        // parent map for every off-screen lane on every repaint.
        let max_depth = timeline_index
            .lanes
            .get(lane_index)
            .map_or(0, |lane| lane.max_depth);
        let desired = Vec2::new(
            ui.available_width().max(240.0),
            20.0 + (max_depth + 1) as f32 * 16.0,
        );
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        // Keep layout stable in the outer scroll area, but skip painting,
        // hit-testing, and tooltip preparation for off-screen lanes.
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let by_id = lane_events
            .iter()
            .filter_map(|event| Some((json_u64(event, "event_id")?, event)))
            .collect::<std::collections::HashMap<_, _>>();
        let visible_ids = frame_id.filter(|_| filter_frame).map(|frame| {
            let mut visible = HashSet::new();
            for event in lane_events
                .iter()
                .filter(|event| json_u64(event, "frame_id") == Some(frame))
            {
                let mut event_id = json_u64(event, "event_id");
                while let Some(id) = event_id {
                    if !visible.insert(id) {
                        break;
                    }
                    event_id = by_id
                        .get(&id)
                        .and_then(|event| json_u64(event, "parent_event_id"));
                }
            }
            visible
        });
        let depth_of = |event: &Value| {
            json_u64(event, "nesting_depth")
                .map(|depth| depth.min(63) as usize)
                .unwrap_or_else(|| nested_depth(event, &by_id).0)
        };
        let lane_frame_events = lane_events.iter().filter(|event| {
            visible_ids.as_ref().is_none_or(|visible| {
                json_u64(event, "event_id").is_some_and(|id| visible.contains(&id))
            })
        });
        let painter = ui.painter_at(rect);
        painter.text(
            Pos2::new(rect.left(), rect.top() + 7.0),
            egui::Align2::LEFT_TOP,
            truncate(&name, 135),
            egui::FontId::proportional(11.0),
            Color32::from_gray(205),
        );
        let timeline = egui::Rect::from_min_max(
            Pos2::new(rect.left() + 170.0, rect.top() + 18.0),
            Pos2::new(rect.right() - 6.0, rect.bottom() - 2.0),
        );
        let lane_response = ui.interact(
            timeline,
            egui::Id::new(("performance-lab-lane", &name)),
            Sense::click_and_drag(),
        );
        timeline_gesture(ui, &lane_response, timeline, range, timeline_range);
        painter.rect_filled(timeline, 2.0, Color32::from_rgb(28, 34, 42));
        for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let x = timeline.left() + timeline.width() * fraction;
            painter.line_segment(
                [
                    Pos2::new(x, timeline.top()),
                    Pos2::new(x, timeline.bottom()),
                ],
                Stroke::new(0.5_f32, Color32::from_gray(60)),
            );
        }
        let mut previous_frame_x = f32::NEG_INFINITY;
        for (start_ns, frame) in frame_starts {
            if *start_ns < range.start_ns || *start_ns > range.end_ns {
                continue;
            }
            let x = timeline.left()
                + timeline.width()
                    * (start_ns.saturating_sub(range.start_ns) as f64 / range.span_ns() as f64)
                        as f32;
            if x - previous_frame_x < 60.0 {
                continue;
            }
            previous_frame_x = x;
            painter.line_segment(
                [
                    Pos2::new(x, timeline.top()),
                    Pos2::new(x, timeline.bottom()),
                ],
                Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(230, 230, 230, 75)),
            );
            painter.text(
                Pos2::new(x, timeline.top()),
                egui::Align2::LEFT_TOP,
                format!("F{frame}"),
                egui::FontId::monospace(8.0),
                Color32::from_gray(200),
            );
        }
        if json_u64(lane, "dropped_events").unwrap_or(0) > 0 {
            painter.text(
                Pos2::new(timeline.right(), rect.top() + 7.0),
                egui::Align2::RIGHT_TOP,
                format!(
                    "{} overwritten",
                    json_u64(lane, "dropped_events").unwrap_or(0)
                ),
                egui::FontId::monospace(9.0),
                Color32::from_rgb(235, 170, 90),
            );
        }
        let mut density = std::collections::HashMap::<(usize, u32), (usize, usize)>::new();
        for event in lane_frame_events {
            let (Some(start_ns), Some(end_ns), Some(duration_ns)) = (
                json_u64(event, "start_ns"),
                json_u64(event, "end_ns"),
                json_u64(event, "duration_ns"),
            ) else {
                continue;
            };
            if range.clip(start_ns, end_ns).is_none() {
                continue;
            }
            let start_fraction =
                start_ns.saturating_sub(range.start_ns) as f64 / range.span_ns() as f64;
            let end_fraction =
                end_ns.saturating_sub(range.start_ns) as f64 / range.span_ns() as f64;
            let x1 = timeline.left() + timeline.width() * start_fraction.clamp(0.0, 1.0) as f32;
            let x2 = (timeline.left() + timeline.width() * end_fraction.clamp(0.0, 1.0) as f32)
                .max(x1 + 1.5);
            let depth = depth_of(event);
            if x2 - x1 < 8.0 {
                let cell = (((x1 + x2) * 0.5 - timeline.left()) / 8.0).floor() as u32;
                let counts = density.entry((depth, cell)).or_default();
                if event
                    .get("wait_reason")
                    .is_some_and(|value| !value.is_null())
                {
                    counts.1 += 1;
                } else {
                    counts.0 += 1;
                }
                continue;
            }
            let row_top = timeline.top() + depth as f32 * 16.0;
            let event_rect = egui::Rect::from_min_max(
                Pos2::new(x1.max(timeline.left()), row_top + 1.0),
                Pos2::new(
                    x2.min(timeline.right()),
                    (row_top + 14.0).min(timeline.bottom()),
                ),
            );
            let event_name = field_text(event, &["name"]).unwrap_or("span");
            let color = if event
                .get("wait_reason")
                .is_some_and(|value| !value.is_null())
            {
                Color32::from_rgb(170, 90, 90)
            } else {
                lane_color(event_name)
            };
            painter.rect_filled(event_rect, 1.5, color);
            let clipped_start = start_ns < range.start_ns;
            let clipped_end = end_ns > range.end_ns;
            if clipped_start || clipped_end {
                painter.rect_stroke(
                    event_rect,
                    1.5,
                    Stroke::new(1.0_f32, Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
                if clipped_start {
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            Pos2::new(timeline.left(), event_rect.center().y),
                            Pos2::new(timeline.left() + 4.0, event_rect.top()),
                            Pos2::new(timeline.left() + 4.0, event_rect.bottom()),
                        ],
                        Color32::WHITE,
                        Stroke::NONE,
                    ));
                }
                if clipped_end {
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            Pos2::new(timeline.right(), event_rect.center().y),
                            Pos2::new(timeline.right() - 4.0, event_rect.top()),
                            Pos2::new(timeline.right() - 4.0, event_rect.bottom()),
                        ],
                        Color32::WHITE,
                        Stroke::NONE,
                    ));
                }
            }
            if *selected_event_id == json_u64(event, "event_id") {
                painter.rect_stroke(
                    event_rect,
                    1.5,
                    Stroke::new(1.3_f32, Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
            }
            let response = ui.interact(
                event_rect,
                egui::Id::new(("performance-lab-event", json_u64(event, "event_id"))),
                Sense::click_and_drag(),
            );
            if response.dragged() {
                let delta = ui.input(|input| input.pointer.delta().x);
                *timeline_range = Some(range.pan(-delta as f64 / timeline.width().max(1.0) as f64));
            }
            let parent_id = json_u64(event, "parent_event_id");
            if event_rect.width() > 65.0 {
                painter.text(
                    event_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    truncate(event_name, (event_rect.width() / 6.0) as usize),
                    egui::FontId::monospace(9.0),
                    Color32::BLACK,
                );
            }
            if response.clicked() {
                *selected_event_id = json_u64(event, "event_id");
            }
            if response.double_clicked() {
                *selected_event_id = json_u64(event, "event_id");
                *timeline_range = Some(TimelineRange::from_bounds(start_ns, end_ns).zoom(1.25));
            }
            if response.hovered() {
                let missing_parent = nested_depth(event, &by_id).1;
                let thread_cpu = json_u64(event, "thread_cpu_ns")
                    .map(|value| {
                        format!("{:.3} ms scheduled thread CPU", value as f64 / 1_000_000.0)
                    })
                    .unwrap_or_else(|| "scheduled thread CPU unavailable".into());
                let job_text = event
                    .get("job_identity")
                    .filter(|value| !value.is_null())
                    .map(Value::to_string)
                    .unwrap_or_else(|| "job identity unavailable".into());
                let wait_text = event
                    .get("wait_reason")
                    .filter(|value| !value.is_null())
                    .map(Value::to_string)
                    .map(|reason| format!("instrumented wait: {reason}"))
                    .unwrap_or_else(|| "measured CPU span".into());
                response.clone().on_hover_text(
                    format!(
                        "{} · frame {} · {:.3} ms · thread {}",
                        event_name,
                        json_u64(event, "frame_id").unwrap_or(0),
                        duration_ns as f64 / 1_000_000.0,
                        json_u64(event, "thread_sequence").unwrap_or(0),
                    ) + &format!(
                        "\n{thread_cpu}\n{wait_text}\njob: {job_text}\nparent: {}{}{}{}",
                        parent_id.map_or_else(|| "none".into(), |id| id.to_string()),
                        if missing_parent {
                            " (overwritten or outside retained view)"
                        } else {
                            ""
                        },
                        if clipped_start {
                            "\nstarts before visible range"
                        } else {
                            ""
                        },
                        if clipped_end {
                            "\nends after visible range"
                        } else {
                            ""
                        },
                    ),
                );
            }
            response.on_hover_cursor(egui::CursorIcon::PointingHand);
            if *selected_event_id == json_u64(event, "event_id") {
                selected_event = Some(event);
            }
        }
        for ((depth, cell), (work, waits)) in density {
            let x = timeline.left() + cell as f32 * 8.0;
            let y = timeline.top() + depth as f32 * 16.0;
            let cell_rect = egui::Rect::from_min_max(
                Pos2::new(x, y + 3.0),
                Pos2::new(
                    (x + 8.0).min(timeline.right()),
                    (y + 12.0).min(timeline.bottom()),
                ),
            );
            let color = if waits > work {
                Color32::from_rgb(130, 75, 80)
            } else {
                Color32::from_rgb(100, 110, 125)
            };
            painter.rect_filled(cell_rect, 0.0, color);
            let response = ui.interact(
                cell_rect,
                egui::Id::new(("performance-lab-density", &name, depth, cell)),
                Sense::click_and_drag(),
            );
            if response.dragged() {
                let delta = ui.input(|input| input.pointer.delta().x);
                *timeline_range = Some(range.pan(-delta as f64 / timeline.width().max(1.0) as f64));
            }
            if response.double_clicked() {
                let fraction = ((cell_rect.center().x - timeline.left()) / timeline.width()) as f64;
                *timeline_range = Some(range.zoom_at(0.2, fraction));
            }
            response.on_hover_text(format!(
                "{} tiny measured spans, {} typed waits in this time cell.\nCounts are not CPU utilization. Wheel or double-click to zoom in.", work, waits
            ));
        }
    }
    if let Some(event) =
        selected_event.or_else(|| selected_event_id.and_then(|id| find_profile_event(profile, id)))
    {
        ui.separator();
        ui.label(RichText::new("Selected span").strong());
        egui::Grid::new("performance-lab-selected-span")
            .num_columns(2)
            .spacing([10.0, 3.0])
            .show(ui, |ui| {
                value_row(
                    ui,
                    "Name",
                    field_text(event, &["name"]).unwrap_or("span").to_string(),
                );
                value_row(
                    ui,
                    "Duration",
                    json_u64(event, "duration_ns")
                        .map(|ns| format!("{:.3} ms", ns as f64 / 1_000_000.0))
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "Frame",
                    json_u64(event, "frame_id")
                        .map_or_else(|| "Unavailable".into(), |id| id.to_string()),
                );
                value_row(
                    ui,
                    "Thread sequence",
                    json_u64(event, "thread_sequence")
                        .map_or_else(|| "Unavailable".into(), |id| id.to_string()),
                );
                value_row(
                    ui,
                    "Parent event",
                    json_u64(event, "parent_event_id")
                        .map_or_else(|| "none".into(), |id| id.to_string()),
                );
                value_row(
                    ui,
                    "Job identity",
                    event
                        .get("job_identity")
                        .filter(|value| !value.is_null())
                        .map(|identity| {
                            format!(
                                "Job {} · capture {} · origin F{} · dispatch F{} · {}",
                                display_u64(identity, "job_id"),
                                display_u64(identity, "capture_id"),
                                display_u64(identity, "origin_frame_id"),
                                display_u64(identity, "dispatch_frame_id"),
                                identity["kind"]
                            )
                        })
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "Wait reason",
                    event
                        .get("wait_reason")
                        .filter(|value| !value.is_null())
                        .map(Value::to_string)
                        .unwrap_or_else(|| "No typed wait recorded".into()),
                );
            });
    }
}

fn nested_depth(event: &Value, events: &std::collections::HashMap<u64, &Value>) -> (usize, bool) {
    let mut depth = 0usize;
    let mut parent = json_u64(event, "parent_event_id");
    let mut seen = HashSet::new();
    while let Some(parent_id) = parent {
        if !seen.insert(parent_id) || depth >= 64 {
            return (depth, true);
        }
        let Some(parent_event) = events.get(&parent_id) else {
            return (depth, true);
        };
        depth += 1;
        parent = json_u64(parent_event, "parent_event_id");
    }
    (depth, false)
}

fn find_profile_event(profile: &Value, event_id: u64) -> Option<&Value> {
    let lanes = profile
        .get("lanes")
        .or_else(|| profile.pointer("/profile/lanes"))?;
    let lanes: Box<dyn Iterator<Item = &Value> + '_> = match lanes {
        Value::Array(values) => Box::new(values.iter()),
        Value::Object(values) => Box::new(values.values()),
        _ => return None,
    };
    lanes
        .filter_map(|lane| lane.get("events").and_then(Value::as_array))
        .flatten()
        .find(|event| json_u64(event, "event_id") == Some(event_id))
}

fn lane_color(event_name: &str) -> Color32 {
    if event_name.contains("generation") || event_name.contains("sampling") {
        Color32::from_rgb(78, 157, 212)
    } else if event_name.contains("publication") || event_name.contains("boundary") {
        Color32::from_rgb(180, 130, 222)
    } else if event_name.contains("snapshot") || event_name.contains("diagnostic") {
        Color32::from_rgb(114, 181, 130)
    } else {
        Color32::from_rgb(202, 157, 86)
    }
}

fn worker_label(lane: &Value, index: usize) -> String {
    let Some(worker) = lane.get("worker") else {
        return field_text(lane, &["name", "label", "id"])
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Lane {index}"));
    };
    let kind = field_text(worker, &["kind"]).unwrap_or("worker");
    let identity = field_text(worker, &["value"])
        .map(str::to_owned)
        .or_else(|| json_u64(worker, "value").map(|value| value.to_string()));
    if kind == "main" {
        return format!(
            "Main · lane {}",
            json_u64(lane, "lane_id").unwrap_or(index as u64)
        );
    }
    if kind == "numeric"
        && let Some(value) = json_u64(worker, "value")
        && value & !0xff == 0x5445_5252_4149_4e00
    {
        return format!("genWorker{}", value & 0xff);
    }
    match identity {
        Some(identity) if kind == "named" => format!(
            "{} · lane {}",
            identity,
            json_u64(lane, "lane_id").unwrap_or(index as u64)
        ),
        Some(identity) => format!(
            "{} {} · lane {}",
            kind,
            identity,
            json_u64(lane, "lane_id").unwrap_or(index as u64)
        ),
        None => format!(
            "{} · lane {}",
            kind,
            json_u64(lane, "lane_id").unwrap_or(index as u64)
        ),
    }
}

fn find_jobs(value: &Value) -> Option<&Vec<Value>> {
    value.get("jobs").and_then(Value::as_array)
}

fn resident_value(snapshot: &DeveloperSnapshot) -> Option<&Value> {
    snapshot
        .resident_planetary
        .as_deref()
        .or(snapshot.resident_regional.as_ref())
}

fn resident_number(resident: Option<&Value>, pointer: &str) -> Option<f64> {
    resident?
        .pointer(pointer)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn resident_trace(snapshot: &DeveloperSnapshot) -> Option<&Value> {
    snapshot
        .resident_planetary
        .as_ref()
        .and_then(|resident| resident.terrain_trace())
        .or_else(|| snapshot.resident_regional.as_ref()?.get("terrain_trace"))
}

fn resident_latest_sample_number(resident: Option<&Value>, field: &str) -> Option<f64> {
    let resident = resident?;
    resident
        .get("latest_native_frame")
        .or_else(|| resident.get("native_frame_samples")?.as_array()?.last())?
        .get(field)?
        .as_f64()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn json_u64(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(Value::as_u64)
}

fn json_number(value: &Value, fields: &[&str]) -> Option<f64> {
    fields.iter().find_map(|field| {
        value
            .get(*field)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
    })
}

fn format_optional_count(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "Unavailable".into())
}

fn format_optional_ms(value: Option<f64>) -> String {
    value.map(format_ms).unwrap_or_else(|| "Unavailable".into())
}

const TRACE_STAGES: [&str; 26] = [
    "requested",
    "desired",
    "generation_queued",
    "generation_started",
    "generation_completed",
    "generation_cancelled",
    "generation_failed",
    "stale_completion",
    "cpu_evicted",
    "resident",
    "drawable",
    "boundary_queued",
    "boundary_started",
    "boundary_finished",
    "boundary_failed",
    "fully_prepared",
    "publishable",
    "publication_started",
    "publication_finished",
    "publication_adopted",
    "transition_started",
    "transition_completed",
    "drawn",
    "superseded",
    "discarded_stale",
    "evicted",
];

fn job_stage(job: &Value) -> Option<&'static str> {
    if let Some(stage) = job.get("latest_stage").and_then(Value::as_str) {
        return TRACE_STAGES.iter().copied().find(|name| *name == stage);
    }
    let milestones = job.get("milestones_us")?.as_array()?;
    milestones
        .iter()
        .rposition(|milestone| !milestone.is_null())
        .and_then(|index| TRACE_STAGES.get(index).copied())
}

fn trace_generated_time_us(trace: &Value) -> Option<u64> {
    json_u64(trace, "generated_at_us").or_else(|| {
        trace
            .get("events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|event| json_u64(event, "time_us"))
            .max()
    })
}

fn job_age_ms(job: &Value, trace_time_us: Option<u64>) -> Option<f64> {
    job.get("age_ms")
        .and_then(Value::as_f64)
        .filter(|age| age.is_finite() && *age >= 0.0)
        .or_else(|| {
            json_u64(job, "requested_us")
                .zip(trace_time_us)
                .map(|(requested, now)| now.saturating_sub(requested) as f64 / 1000.0)
        })
}

fn format_address(address: Option<&Value>) -> String {
    let Some(address) = address else {
        return "—".into();
    };
    let Some(face) = json_u64(address, "face") else {
        return "—".into();
    };
    let face = match face {
        0 => "+X",
        1 => "−X",
        2 => "+Y",
        3 => "−Y",
        4 => "+Z",
        5 => "−Z",
        _ => "?",
    };
    format!(
        "{face} L{} ({}, {})",
        json_u64(address, "level").unwrap_or(0),
        json_u64(address, "x").unwrap_or(0),
        json_u64(address, "y").unwrap_or(0),
    )
}

fn job_identity(job: &Value) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        job.get("address")
            .map_or_else(|| "—".into(), Value::to_string),
        job.get("body_identity")
            .map_or_else(|| "—".into(), Value::to_string),
        display_u64(job, "surface_revision"),
        display_u64(job, "material_revision"),
        display_u64(job, "requested_us"),
    )
}

fn format_blockers(blockers: Option<&Value>) -> String {
    blockers
        .and_then(Value::as_array)
        .map(|blockers| {
            blockers
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "none".into())
}

fn draw_job_inspector(ui: &mut egui::Ui, job: &Value, trace_time_us: Option<u64>) {
    egui::Grid::new("performance-lab-job-inspector")
        .num_columns(2)
        .spacing([12.0, 3.0])
        .show(ui, |ui| {
            let requested = json_u64(job, "requested_us");
            value_row(
                ui,
                "Requested timestamp",
                requested
                    .map(|time| format!("{time} µs from trace origin"))
                    .unwrap_or_else(|| "Unavailable".into()),
            );
            value_row(
                ui,
                "Age at snapshot",
                job_age_ms(job, trace_time_us)
                    .map(|age| format_optional_ms(Some(age)))
                    .unwrap_or_else(|| "Unavailable".into()),
            );
            value_row(ui, "Blockers", format_blockers(job.get("blocked_by")));
            for (label, field) in [
                ("Requested -> execution", "requested_to_generation_start_us"),
                ("Generation execution", "generation_elapsed_us"),
                ("Generation -> boundary", "generation_to_boundary_start_us"),
                ("Boundary execution", "boundary_elapsed_us"),
                ("Boundary -> publishable", "boundary_to_publishable_us"),
                ("Publication wait", "publication_wait_us"),
                ("Publication execution", "publication_elapsed_us"),
                ("Prepared -> drawable", "prepared_to_drawable_us"),
                ("Requested -> drawable", "requested_to_drawable_us"),
            ] {
                let duration = json_u64(job, field)
                    .map(|micros| format_optional_ms(Some(micros as f64 / 1000.0)))
                    .unwrap_or_else(|| "Unavailable".into());
                value_row(ui, label, duration);
            }
            value_row(
                ui,
                "Open blocker time (overlapping)",
                json_u64(job, "active_blocked_elapsed_us")
                    .map(|micros| format_optional_ms(Some(micros as f64 / 1000.0)))
                    .unwrap_or_else(|| "Unavailable".into()),
            );
            value_row(
                ui,
                "Total blocker time (may overlap)",
                json_u64(job, "total_blocked_elapsed_us")
                    .map(|micros| format_optional_ms(Some(micros as f64 / 1000.0)))
                    .unwrap_or_else(|| "Unavailable".into()),
            );
        });
    egui::CollapsingHeader::new("Exact tile identity").show(ui, |ui| {
        egui::Grid::new("performance-lab-job-identity").show(ui, |ui| {
            for (label, field) in [
                ("Body identity", "body_identity"),
                ("Surface revision", "surface_revision"),
                ("Material revision", "material_revision"),
                ("Radius bits", "radius_bits"),
                ("Cell resolution", "cells"),
                ("Format version", "format_version"),
                ("Filter version", "filter_version"),
            ] {
                value_row(ui, label, display_u64(job, field));
            }
        });
    });
    ui.small("Queue and blocker intervals may overlap; do not add them to infer a critical path. Drawable means recorded publication, not visual quality acceptance.");
    draw_blocker_intervals(ui, job);
    if let Some(links) = job.get("profile_jobs") {
        ui.small(format!("Worker admissions/retries: {links}"));
    }
    ui.small(format!(
        "Omitted job links: {}",
        json_u64(job, "profile_jobs_omitted").unwrap_or(0)
    ));
    ui.label(RichText::new("Milestones · milliseconds from trace origin").strong());
    if let Some(milestones) = job.get("milestones").and_then(Value::as_array) {
        for milestone in milestones {
            ui.monospace(format!(
                "{}: {} µs",
                milestone["stage"], milestone["time_us"]
            ));
        }
    }
    if let Some(milestones) = job.get("milestones_us").and_then(Value::as_array) {
        egui::Grid::new("performance-lab-job-milestones")
            .num_columns(2)
            .show(ui, |ui| {
                for (index, time) in milestones.iter().enumerate() {
                    let Some(name) = TRACE_STAGES.get(index) else {
                        continue;
                    };
                    if time.is_null() {
                        continue;
                    }
                    ui.label(*name);
                    ui.label(
                        time.as_u64()
                            .map(|us| format_optional_ms(Some(us as f64 / 1000.0)))
                            .unwrap_or_else(|| "Unavailable".into()),
                    );
                    ui.end_row();
                }
            });
    }
}

fn profile_job_matches(job: &Value, identity: &Value) -> bool {
    let mut ids = vec![json_u64(identity, "job_id").unwrap_or(0)];
    if let Some(upstream) = identity["upstream_job_ids"].as_array() {
        ids.extend(
            upstream
                .iter()
                .filter_map(Value::as_u64)
                .filter(|id| *id != 0),
        );
    }
    job["profile_jobs"].as_array().is_some_and(|links| {
        links.iter().any(|link| {
            json_u64(link, "job_id").is_some_and(|id| ids.contains(&id))
                && link["capture_id"] == identity["capture_id"]
        })
    })
}

fn draw_correlated_job(ui: &mut egui::Ui, identity: &Value, snapshot: &DeveloperSnapshot) {
    let Some(trace) = resident_trace(snapshot) else {
        ui.label("Job trace unavailable at this snapshot.");
        return;
    };
    let Some(jobs) = find_jobs(trace) else {
        return;
    };
    let mut ids = vec![json_u64(identity, "job_id").unwrap_or(0)];
    if let Some(upstream) = identity["upstream_job_ids"].as_array() {
        ids.extend(
            upstream
                .iter()
                .filter_map(Value::as_u64)
                .filter(|id| *id != 0),
        );
    }
    let matched = jobs
        .iter()
        .filter(|job| {
            job["profile_jobs"].as_array().is_some_and(|links| {
                links.iter().any(|link| {
                    json_u64(link, "job_id").is_some_and(|id| ids.contains(&id))
                        && link["capture_id"] == identity["capture_id"]
                })
            })
        })
        .collect::<Vec<_>>();
    if matched.is_empty() {
        ui.label("Exact job link absent from retained trace; no tile match inferred.");
    }
    for job in matched {
        egui::CollapsingHeader::new(format!(
            "Recorded job waits · {}",
            format_address(job.get("address"))
        ))
        .id_salt(job_identity(job))
        .default_open(true)
        .show(ui, |ui| {
            draw_job_inspector(ui, job, trace_generated_time_us(trace));
        });
    }
    ui.small(format!(
        "Dependency links omitted: {} · trace jobs omitted: {} · trace events omitted: {}",
        json_u64(identity, "upstream_jobs_omitted").unwrap_or(0),
        json_u64(trace, "snapshot_jobs_omitted").unwrap_or(0),
        json_u64(trace, "snapshot_events_omitted").unwrap_or(0)
    ));
}

fn interval_union_us(intervals: &[(u64, u64)]) -> u64 {
    let mut intervals = intervals
        .iter()
        .copied()
        .filter(|(start, end)| end >= start)
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let mut total = 0u64;
    let mut current: Option<(u64, u64)> = None;
    for (start, end) in intervals {
        current = match current {
            Some((left, right)) if start <= right => Some((left, right.max(end))),
            Some((left, right)) => {
                total = total.saturating_add(right - left);
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    total.saturating_add(current.map_or(0, |(start, end)| end - start))
}

fn draw_blocker_intervals(ui: &mut egui::Ui, job: &Value) {
    let Some(intervals) = job["blocker_intervals"].as_array() else {
        ui.label("Recorded blocker intervals unavailable.");
        return;
    };
    let pairs = intervals
        .iter()
        .filter_map(|interval| json_u64(interval, "start_us").zip(json_u64(interval, "end_us")))
        .collect::<Vec<_>>();
    ui.small(format!(
        "Recorded blocker union {:.3} ms · {} intervals omitted; independent reasons overlap",
        interval_union_us(&pairs) as f64 / 1000.0,
        json_u64(job, "blocker_intervals_omitted").unwrap_or(0)
    ));
    for interval in intervals {
        ui.monospace(format!(
            "{} · {} -> {} µs{}",
            interval["reason"],
            interval["start_us"],
            interval["end_us"],
            if interval["open"].as_bool() == Some(true) {
                " (open at snapshot)"
            } else {
                ""
            }
        ));
    }
    ui.small(format!("Recorded dependencies: parent {} · children {} · neighbors {}. Association is not a measured wait unless a blocker was recorded.",
        job["parent"],job["children"],job["neighbors"]));
}

fn display_u64(value: &Value, field: &str) -> String {
    json_u64(value, field)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".into())
}

fn display_count(value: &Value) -> String {
    json_u64(value, "count")
        .or_else(|| json_u64(value, "queued"))
        .or_else(|| json_u64(value, "pending"))
        .map(|count| count.to_string())
        .unwrap_or_else(|| "Unavailable".into())
}

fn display_age(value: &Value) -> String {
    json_number(
        value,
        &["oldest_ms", "oldest_age_ms", "oldest_backlog_age_ms"],
    )
    .map(|age| format_optional_ms(Some(age)))
    .or_else(|| {
        json_number(value, &["oldest_us", "oldest_age_us"])
            .map(|micros| format_optional_ms(Some(micros / 1000.0)))
    })
    .unwrap_or_else(|| "Unavailable".into())
}

fn display_rate(value: &Value) -> String {
    json_number(
        value,
        &[
            "throughput_per_second",
            "completed_per_second",
            "rate_per_second",
        ],
    )
    .map(|rate| format!("{rate:.2} / s"))
    .unwrap_or_else(|| "Unavailable".into())
}

#[cfg(test)]
mod timeline_tests {
    use super::{TimelineIndex, TimelineRange, interval_union_us, nested_depth};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn timeline_index_caches_frame_and_lane_layout_for_repaints() {
        let profile = json!({
            "capture_id":"stable",
            "generated_at_ns":20,
            "lanes":[{"worker":{"kind":"main"},"events":[
                {"name":"Frame","start_ns":2,"end_ns":20,"frame_id":7,"nesting_depth":0},
                {"name":"detail","start_ns":5,"end_ns":9,"frame_id":7,"nesting_depth":2}
            ]}]
        });
        let index = TimelineIndex::new(&profile);
        assert!(index.matches(&profile));
        assert_eq!(index.retained_bounds.unwrap().start_ns, 2);
        assert_eq!(index.frame_bounds.get(&7).unwrap().end_ns, 20);
        assert_eq!(index.frame_starts, vec![(2, 7)]);
        assert_eq!(index.lanes[0].max_depth, 2);
        assert_eq!(index.frame_bounds.len(), 1);
    }

    #[test]
    fn timeline_zoom_and_pan_keep_a_positive_bounded_range() {
        let range = TimelineRange::from_bounds(1_000_000, 11_000_000);
        let zoomed = range.zoom(0.5);
        assert_eq!(zoomed.span_ns(), 5_000_000);
        assert_eq!(zoomed.start_ns, 3_500_000);
        assert_eq!(zoomed.end_ns, 8_500_000);

        let panned = zoomed.pan(0.5);
        assert_eq!(panned.start_ns, 6_000_000);
        assert_eq!(panned.span_ns(), zoomed.span_ns());
        assert!(TimelineRange::from_bounds(4, 4).span_ns() > 0);
        assert_eq!(range.clip(0, 2_000_000), Some((1_000_000, 2_000_000)));
        assert_eq!(range.clip(20_000_000, 30_000_000), None);
        assert_eq!(range.clip(4_000_000, 2_000_000), None);
        assert_eq!(TimelineRange::from_bounds(u64::MAX, u64::MAX).span_ns(), 1);
    }

    #[test]
    fn wider_timeline_reveals_more_time_at_the_same_pixel_scale() {
        let range = TimelineRange::from_bounds(100_000_000, 110_000_000);
        let wider = range.resize_for_width(500.0, 1_000.0);
        assert_eq!(wider.start_ns, 95_000_000);
        assert_eq!(wider.end_ns, 115_000_000);
        assert_eq!(wider.span_ns() / 1_000, range.span_ns() / 500);
        let narrower = wider.resize_for_width(1_000.0, 500.0);
        assert_eq!(narrower.start_ns, range.start_ns);
        assert_eq!(narrower.end_ns, range.end_ns);
        assert_eq!(
            range.resize_for_width(f64::NAN, 500.0).span_ns(),
            range.span_ns()
        );
        assert_eq!(
            range.resize_for_width(0.0, 500.0).span_ns(),
            range.span_ns()
        );
        assert_eq!(range.with_span_ns(90_000_000_000.0).end_ns, 30_105_000_000);
    }

    #[test]
    fn pointer_zoom_keeps_the_point_of_interest_under_the_cursor() {
        let range = TimelineRange::from_bounds(100_000_000, 120_000_000);
        let zoomed = range.zoom_at(0.5, 0.25);
        assert_eq!(zoomed.start_ns, 102_500_000);
        assert_eq!(zoomed.span_ns(), 10_000_000);
        assert_eq!(
            range.start_ns + range.span_ns() / 4,
            zoomed.start_ns + zoomed.span_ns() / 4
        );
        let near_origin = TimelineRange::from_bounds(0, 1_000_000).zoom_at(2.0, 0.5);
        assert_eq!(near_origin.start_ns, 0);
        assert_eq!(near_origin.span_ns(), 2_000_000);
    }

    #[test]
    fn freeze_keeps_frame_history_and_profile_stable_as_live_data_advances() {
        use super::*;
        use crate::developer_snapshot::{CameraSnapshot, GeneralSnapshot, SnapshotCameraMode};
        let mut snapshot = DeveloperSnapshot {
            schema_version: 6,
            general: GeneralSnapshot {
                frame_number: 5,
                simulation_time_s: 0.0,
                world_revision: 1,
                selected_body: None,
                focused_body: None,
                camera_mode: SnapshotCameraMode::BodyOrbit,
                paused: true,
                simulation_speed: 1.0,
            },
            camera: CameraSnapshot {
                position_m: [0.0; 3],
                orientation_xyzw: [0.0, 0.0, 0.0, 1.0],
                reference_frame: "synthetic".into(),
                frame_body: None,
                reference_body: None,
                body_distance_m: None,
                reference_altitude_m: None,
                terrain_clearance_m: None,
                drawn_mesh_clearance_m: None,
                fov_y_degrees: 60.0,
                near_plane_m: 0.1,
                viewport_origin_pixels: [0; 2],
                viewport_size_pixels: [100; 2],
                navigation: None,
            },
            terrain: Default::default(),
            work: Default::default(),
            memory: Default::default(),
            rendering: Default::default(),
            performance: Default::default(),
            warnings: vec![],
            capture: None,
            shared_scene: None,
            motion: None,
            development: None,
            resident_tile: None,
            resident_hierarchy: None,
            resident_regional: None,
            resident_planetary: None,
            terrain_atlas: None,
            engine_profile: Some(Arc::new(json!({"generated_at_ns":1,"lanes":[]}))),
        };
        let mut lab = PerformanceLab::default();
        lab.configure(
            Some(&snapshot),
            ProfilerControls {
                enabled: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        lab.ingest(&snapshot, None);
        lab.configure(
            Some(&snapshot),
            ProfilerControls {
                freeze: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        snapshot.general.frame_number = 6;
        snapshot.engine_profile = Some(Arc::new(json!({"generated_at_ns":2,"lanes":[]})));
        lab.ingest(&snapshot, None);
        assert_eq!(lab.history.len(), 1);
        assert_eq!(
            lab.frozen_snapshot.as_ref().unwrap().general.frame_number,
            5
        );
        assert_eq!(
            lab.frozen_snapshot
                .as_ref()
                .unwrap()
                .engine_profile
                .as_ref()
                .unwrap()["generated_at_ns"],
            1
        );
    }

    #[test]
    fn nested_depth_follows_parent_links_and_reports_missing_parent() {
        let root = json!({"event_id": 1});
        let middle = json!({"event_id": 2, "parent_event_id": 1});
        let leaf = json!({"event_id": 3, "parent_event_id": 2});
        let events = HashMap::from([(1, &root), (2, &middle)]);
        assert_eq!(nested_depth(&leaf, &events), (2, false));

        let orphan = json!({"event_id": 4, "parent_event_id": 99});
        assert_eq!(nested_depth(&orphan, &events), (0, true));
    }

    #[test]
    fn overlapping_blockers_count_elapsed_union_once_and_reject_reversed_intervals() {
        assert_eq!(
            interval_union_us(&[(10, 20), (15, 30), (30, 40), (50, 60), (90, 80)]),
            40
        );
        assert_eq!(interval_union_us(&[]), 0);
    }

    #[test]
    fn malformed_parent_cycle_is_explicitly_incomplete() {
        let root = json!({"event_id":1,"parent_event_id":2});
        let child = json!({"event_id":2,"parent_event_id":1});
        assert!(nested_depth(&child, &HashMap::from([(1, &root), (2, &child)])).1);
    }
}

fn field_text<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|name| value.get(*name).and_then(Value::as_str))
}

fn quantile(values: &[f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(f64::total_cmp);
    let index = ((ordered.len() - 1) as f64 * fraction.clamp(0.0, 1.0)).ceil() as usize;
    ordered.get(index).copied()
}

fn value_row(ui: &mut egui::Ui, label: &str, value: String) {
    ui.label(label);
    ui.label(value);
    ui.end_row();
}

fn display_or_unknown(value: &str) -> String {
    if value.is_empty() {
        "Unavailable".into()
    } else {
        value.into()
    }
}

fn format_ms(value: f64) -> String {
    format!("{value:.3} ms")
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut amount = bytes as f64;
    let mut unit = 0;
    while amount >= 1024.0 && unit + 1 < UNITS.len() {
        amount /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{amount:.2} {}", UNITS[unit])
    }
}

fn option_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "Unavailable",
    }
}

fn bool_word(value: bool) -> &'static str {
    if value { "Yes" } else { "No" }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let prefix = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::{Metric, queue_measure};
    use crate::developer_snapshot::PerformanceSnapshot;

    #[test]
    fn preparation_and_publication_metrics_keep_their_telemetry_scopes() {
        let performance = PerformanceSnapshot {
            preparation_ms: Some(0.25),
            terrain_preparation_ms: Some(0.1),
            ..Default::default()
        };
        let resident = serde_json::json!({
            "preparation_ms": 0.8,
            "gpu_preparation_ms": 1.1,
            "publication_ms": 3.2,
            "publication_admission_ms": 0.7,
            "native_frame_samples": [{
                "preparation_ms": 0.9,
                "gpu_preparation_ms": 1.2,
                "regional_advance_ms": 2.4
            }]
        });

        assert_eq!(
            Metric::TerrainPreparation.value_from(&performance, Some(&resident)),
            Some(0.9)
        );
        assert_eq!(
            Metric::TerrainUpdate.value_from(&performance, Some(&resident)),
            Some(2.4)
        );
        assert_eq!(
            Metric::Publication.value_from(&performance, Some(&resident)),
            Some(3.2)
        );
        assert_eq!(
            Metric::PublicationAdmission.value_from(&performance, Some(&resident)),
            Some(0.7)
        );
    }

    #[test]
    fn queue_measure_aggregates_active_and_queued_trace_stages() {
        let queues = serde_json::json!([
            {
                "queue": "generation_queued",
                "count": 2,
                "oldest_age_ms": 10,
                "p95_age_ms": 8,
                "completed_per_second": 1.0
            },
            {
                "queue": "generation_active",
                "count": 3,
                "oldest_age_ms": 20,
                "p95_age_ms": 15,
                "completed_per_second": 3.0
            }
        ]);

        let measure = queue_measure(Some(&queues), &["generation_queued", "generation_active"]);
        assert_eq!(measure.count, Some(5));
        assert_eq!(measure.oldest_ms, Some(20.0));
        assert_eq!(measure.p95_ms, Some(15.0));
        assert_eq!(measure.throughput_per_second, Some(3.0));
    }

    #[test]
    fn incomplete_ui_snapshot_uses_completed_cpu_sample_without_relabeling_frame_cpu() {
        let performance = PerformanceSnapshot {
            frame_cpu_ms: Some(4.0),
            ..Default::default()
        };
        assert_eq!(Metric::HostFrame.value_from(&performance, None), None);
        let resident = serde_json::json!({"latest_native_frame": {
            "host_frame_ms": 7.0,
            "render_present_ms": 2.0,
            "diagnostics_ms": 0.5,
            "gpu_preparation_ms": 1.0
        }});
        for (metric, expected) in [
            (Metric::HostFrame, 7.0),
            (Metric::RenderPresent, 2.0),
            (Metric::Diagnostics, 0.5),
            (Metric::GpuPreparation, 1.0),
        ] {
            assert_eq!(
                metric.value_from(&performance, Some(&resident)),
                Some(expected)
            );
        }
    }
}
