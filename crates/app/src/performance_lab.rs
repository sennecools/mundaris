//! Compact, read-only performance and terrain diagnostics for developer builds.

use std::{collections::VecDeque, sync::Arc};

use egui::{Align, Color32, Context, Pos2, RichText, Sense, Stroke, Vec2};
use serde_json::Value;

use crate::developer_snapshot::{DeveloperSnapshot, PerformanceSnapshot};

const HISTORY_CAPACITY: usize = 600;
const DEFAULT_FRAME_TARGET_MS: f64 = 1000.0 / 60.0;
const HOST_STALL_BOUND_MS: f64 = 100.0;
const PUBLICATION_BUDGET_MS: f64 = 2.0;
const PANEL_WIDTH: f32 = 570.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Overview,
    Timeline,
    Terrain,
    Jobs,
    Gpu,
    Captures,
    Benchmarks,
}

/// Deterministic real-Moon routes. The application owns camera control,
/// checkpoint timing, native captures, and evidence output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchmarkScenario {
    Moon100Km,
    MoonCloseInspection,
    MoonSweep,
}

impl BenchmarkScenario {
    pub const ALL: [Self; 3] = [Self::Moon100Km, Self::MoonCloseInspection, Self::MoonSweep];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Moon100Km => "Moon · 100 km checkpoint",
            Self::MoonCloseInspection => "Moon · close inspection",
            Self::MoonSweep => "Moon · approach / retreat sweep",
        }
    }

    pub const fn route_id(self) -> &'static str {
        match self {
            Self::Moon100Km => "moon-100km",
            Self::MoonCloseInspection => "moon-close-inspection",
            Self::MoonSweep => "moon-approach-retreat-sweep",
        }
    }

    /// Named radial checkpoints for the standard Moon sweep.
    /// Close-inspection details remain with the native route controller.
    pub const fn clearance_checkpoints_km(self) -> &'static [f64] {
        match self {
            Self::Moon100Km => &[100.0],
            Self::MoonCloseInspection => &[],
            Self::MoonSweep => &[
                1000.0, 500.0, 250.0, 100.0, 25.0, 5.0, 1.0, 5.0, 25.0, 100.0, 250.0, 500.0, 1000.0,
            ],
        }
    }
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
    pub benchmark_requested: Option<BenchmarkScenario>,
    pub stop_requested: bool,
    history: VecDeque<FrameSample>,
    maxima: [Option<f64>; 11],
    selected_frame: Option<u64>,
    selected_job: Option<usize>,
    selected_job_key: Option<String>,
    tab: Tab,
    latest_frame: Option<u64>,
    latest_profile: Option<Arc<Value>>,
    frozen_snapshot: Option<Arc<DeveloperSnapshot>>,
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
            benchmark_requested: None,
            stop_requested: false,
            history: VecDeque::with_capacity(HISTORY_CAPACITY),
            maxima: [None; 11],
            selected_frame: None,
            selected_job: None,
            selected_job_key: None,
            tab: Tab::Overview,
            latest_frame: None,
            latest_profile: None,
            frozen_snapshot: None,
            frame_target_ms: DEFAULT_FRAME_TARGET_MS,
        }
    }
}

impl PerformanceLab {
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

        let mut open = self.open;
        let frozen_snapshot = self.frozen_snapshot.clone();
        let latest_profile = self.latest_profile.clone();
        egui::Window::new("Performance Lab")
            .open(&mut open)
            .default_width(PANEL_WIDTH)
            .resizable(true)
            .show(ctx, |ui| {
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
                    shown_snapshot.and_then(|current| current.engine_profile.as_deref())
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
                        Tab::Timeline => self.draw_timeline(ui, shown_profile),
                        Tab::Terrain => self.draw_terrain(ui, shown_snapshot),
                        Tab::Jobs => self.draw_jobs(ui, shown_snapshot),
                        Tab::Gpu => self.draw_gpu(ui, shown_snapshot),
                        Tab::Captures => self.draw_captures(ui, shown_snapshot),
                        Tab::Benchmarks => self.draw_benchmarks(ui),
                    });
            });
        self.open = open;
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
                    snapshot.cloned().map(Arc::new)
                } else {
                    None
                };
            }
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
                (Tab::Benchmarks, "Benchmarks"),
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

    fn draw_timeline(&mut self, ui: &mut egui::Ui, profile: Option<&Value>) {
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
            for metric in Metric::ALL {
                let value = sample.get(metric);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(metric.label()).strong());
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        ui.label(value.map(format_ms).unwrap_or_else(|| "Unavailable".into()));
                    });
                });
            }
        } else {
            ui.label("No samples retained yet.");
        }
        self.draw_chart(ui);
        draw_profile_stats(ui, profile);
        let selected_frame = self
            .selected_frame
            .filter(|frame| self.history.iter().any(|sample| sample.frame == *frame));
        egui::CollapsingHeader::new("Worker lanes")
            .default_open(true)
            .show(ui, |ui| draw_lanes(ui, profile, selected_frame));
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
                    "GPU clouds execution",
                    performance
                        .gpu_clouds_ms
                        .map(format_ms)
                        .unwrap_or_else(|| "Unavailable".into()),
                );
                value_row(
                    ui,
                    "GPU atmosphere execution",
                    performance
                        .gpu_atmosphere_ms
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
                    "GPU source frame",
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

    fn draw_benchmarks(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "Queue a real-Moon route. The application owns native camera control and paired evidence capture.",
        );
        ui.label(
            RichText::new("These routes measure the ordinary planetary terrain path.")
                .small()
                .color(Color32::from_gray(160)),
        );
        for scenario in BenchmarkScenario::ALL {
            ui.horizontal(|ui| {
                if ui.button(format!("Run · {}", scenario.label())).clicked() {
                    self.benchmark_requested = Some(scenario);
                }
                ui.monospace(scenario.route_id());
            });
        }
        if ui.button("Stop active route").clicked() {
            self.stop_requested = true;
        }
        ui.separator();
        if let Some(route) = self.benchmark_requested {
            ui.label(format!("Pending route request: {}", route.route_id()));
        }
        ui.label(
            "The route controller records checkpoint state and owns native PNG + JSON output.",
        );
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
        .default_open(true)
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

fn draw_lanes(ui: &mut egui::Ui, profile: Option<&Value>, frame_id: Option<u64>) {
    let Some(profile) = profile else {
        ui.label("Profiler lane data unavailable.");
        return;
    };
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
    let generated_at_ns = json_u64(profile, "generated_at_ns").unwrap_or(0);
    let window_start_ns = generated_at_ns.saturating_sub(3_000_000_000);
    let window_span_ns = generated_at_ns.saturating_sub(window_start_ns).max(1);
    let selected_frame = frame_id;
    ui.small(format!(
        "3 s profile window · {} · blank rows mean no retained matching spans",
        selected_frame.map_or_else(
            || "all retained frames".into(),
            |frame| format!("frame {frame}")
        )
    ));
    if let Some(frame) = selected_frame {
        let frame_events = iterable
            .iter()
            .filter_map(|(_, lane)| lane.get("events").and_then(Value::as_array));
        let mut matching_events = false;
        let mut only_older = true;
        for event in frame_events
            .flatten()
            .filter(|event| json_u64(event, "frame_id") == Some(frame))
        {
            matching_events = true;
            if json_u64(event, "end_ns").is_some_and(|end| end >= window_start_ns) {
                only_older = false;
            }
        }
        if matching_events && only_older {
            ui.small("Matching lane events are older than the displayed 3 s window.");
        } else if !matching_events {
            ui.small(
                "No matching lane events remain in the bounded profiler rings for this frame.",
            );
        }
    }
    let (ruler, _) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(240.0), 16.0),
        Sense::hover(),
    );
    for (second, label) in ["0 s", "−1 s", "−2 s", "−3 s"].into_iter().enumerate() {
        let fraction = second as f32 / 3.0;
        let x = ruler.left() + 170.0 + (ruler.width() - 176.0) * (1.0 - fraction);
        ui.painter().text(
            Pos2::new(x, ruler.center().y),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::monospace(9.0),
            Color32::from_gray(150),
        );
    }
    for (name, lane) in iterable {
        let events = lane.get("events").and_then(Value::as_array);
        let desired = Vec2::new(ui.available_width().max(240.0), 25.0);
        let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.text(
            Pos2::new(rect.left(), rect.center().y),
            egui::Align2::LEFT_CENTER,
            truncate(&name, 135),
            egui::FontId::proportional(11.0),
            Color32::from_gray(205),
        );
        let timeline = egui::Rect::from_min_max(
            Pos2::new(rect.left() + 170.0, rect.top() + 3.0),
            Pos2::new(rect.right() - 6.0, rect.bottom() - 3.0),
        );
        painter.rect_filled(timeline, 2.0, Color32::from_rgb(28, 34, 42));
        for second in 0..=3 {
            let x = timeline.right() - timeline.width() * second as f32 / 3.0;
            painter.line_segment(
                [
                    Pos2::new(x, timeline.top()),
                    Pos2::new(x, timeline.bottom()),
                ],
                Stroke::new(0.5_f32, Color32::from_gray(60)),
            );
        }
        for event in events.into_iter().flatten() {
            if selected_frame.is_some_and(|frame| json_u64(event, "frame_id") != Some(frame)) {
                continue;
            }
            let (Some(start_ns), Some(end_ns), Some(duration_ns)) = (
                json_u64(event, "start_ns"),
                json_u64(event, "end_ns"),
                json_u64(event, "duration_ns"),
            ) else {
                continue;
            };
            if end_ns < window_start_ns || start_ns > generated_at_ns {
                continue;
            }
            let start_fraction =
                start_ns.saturating_sub(window_start_ns) as f32 / window_span_ns as f32;
            let end_fraction =
                end_ns.saturating_sub(window_start_ns) as f32 / window_span_ns as f32;
            let x1 = timeline.left() + timeline.width() * start_fraction.clamp(0.0, 1.0);
            let x2 =
                (timeline.left() + timeline.width() * end_fraction.clamp(0.0, 1.0)).max(x1 + 1.5);
            let event_rect = egui::Rect::from_min_max(
                Pos2::new(x1, timeline.top() + 1.0),
                Pos2::new(x2.min(timeline.right()), timeline.bottom() - 1.0),
            );
            let event_name = field_text(event, &["name"]).unwrap_or("span");
            let color = lane_color(event_name);
            painter.rect_filled(event_rect, 1.5, color);
            ui.interact(
                event_rect,
                egui::Id::new(("performance-lab-event", json_u64(event, "event_id"))),
                Sense::hover(),
            )
            .on_hover_text(format!(
                "{} · frame {} · {:.3} ms · thread {}",
                event_name,
                json_u64(event, "frame_id").unwrap_or(0),
                duration_ns as f64 / 1_000_000.0,
                json_u64(event, "thread_sequence").unwrap_or(0),
            ));
        }
    }
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

const TRACE_STAGES: [&str; 24] = [
    "requested",
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
        });
    ui.label(RichText::new("Milestones · milliseconds from trace origin").strong());
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
