//! Performance workspace: frame strip, a zoomable per-frame timeline with
//! fixed CPU and GPU lanes, and stat columns, painted from the cached
//! [`ProfilerData`].
//!
//! The timeline keeps its own time window (milliseconds from the frame start),
//! so the scale stays put between frames: the wheel zooms around the pointer,
//! dragging pans, a double-click returns to the 60 Hz budget. Lanes keep their
//! order and the most rows they have needed, so they never jump.

use astrum_app::studio::{profiler_view::SpanKind, view::StudioAction};
use egui::{Align, Align2, CornerRadius, Layout, Rect, RichText, Sense, Stroke, Ui};

use super::{panels, theme::*};
use crate::profiler_ui::{ProfilerData, tick_step};

/// One profiler interaction for the frame owner.
pub enum ProfilerInput {
    Action(StudioAction),
    SelectFrame(usize),
    FollowLatest,
    SelectSpan(usize),
}

const ROW_HEIGHT: f32 = 20.0;
const LABEL_WIDTH: f32 = 132.0;
const AXIS_HEIGHT: f32 = 20.0;
const STRIP_HEIGHT: f32 = 72.0;
const STATS_WIDTH: f32 = 340.0;
const BUDGET_MS: f64 = 1000.0 / 60.0;
/// Zoom limits of the timeline window.
const MIN_WINDOW_MS: f64 = 0.02;
const MAX_WINDOW_MS: f64 = 250.0;
/// Nesting rows a lane may grow to before deeper spans share the last row.
const MAX_LANE_ROWS: i32 = 10;

/// UI-local timeline view kept across frames.
pub struct TimelineState {
    start_ms: f64,
    end_ms: f64,
    /// Lanes in first-seen order with the most rows each has needed.
    lanes: Vec<(String, i32)>,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            start_ms: 0.0,
            end_ms: BUDGET_MS,
            lanes: Vec::new(),
        }
    }
}

impl TimelineState {
    fn set_window(&mut self, start: f64, end: f64) {
        let span = (end - start).clamp(MIN_WINDOW_MS, MAX_WINDOW_MS);
        self.start_ms = start.max(-0.1 * span);
        self.end_ms = self.start_ms + span;
    }

    /// Adds lanes seen for the first time and grows lanes that needed more
    /// rows; never shrinks or reorders.
    fn remember_lanes(&mut self, data: &ProfilerData) {
        for lane in &data.lanes {
            let rows = lane.rows.clamp(1, MAX_LANE_ROWS);
            match self.lanes.iter_mut().find(|(name, _)| *name == lane.name) {
                Some((_, kept)) => *kept = (*kept).max(rows),
                None => self.lanes.push((lane.name.clone(), rows)),
            }
        }
    }
}

pub fn show(ui: &mut Ui, data: &ProfilerData, state: &mut TimelineState) -> Vec<ProfilerInput> {
    let mut out = Vec::new();
    state.remember_lanes(data);
    controls(ui, data, state, &mut out);
    ui.add_space(SPACE_1);
    ui.horizontal_top(|ui| {
        let main_width = (ui.available_width() - STATS_WIDTH - SPACE_3).max(240.0);
        ui.vertical(|ui| {
            ui.set_width(main_width);
            panels::section_header(
                ui,
                "FRAMES",
                "fixed 0–33 ms scale · line at 16.7 ms · click a bar to pin that frame",
            );
            strip(ui, data, &mut out);
            ui.add_space(SPACE_2);
            panels::section_header(ui, "TIMELINE", &data.timeline_title);
            timeline(ui, data, state, &mut out);
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("profiler-stats")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(STATS_WIDTH - SPACE_3);
                ui.with_layout(Layout::top_down(Align::Min), |ui| stats(ui, data));
            });
    });
    out
}

fn stats(ui: &mut Ui, data: &ProfilerData) {
    panels::section_header(ui, "FRAME", "");
    for stat in &data.frame_stats {
        panels::stat_row(ui, stat);
    }
    ui.add_space(SPACE_2);
    panels::section_header(ui, "SELECTED SPAN", "");
    if data.span_detail.is_empty() {
        ui.label(
            RichText::new("Click a span in the timeline.")
                .font(small())
                .color(TEXT_DISABLED),
        );
    }
    for stat in &data.span_detail {
        panels::stat_row(ui, stat);
    }
    ui.add_space(SPACE_2);
    panels::section_header(ui, "GPU SCOPES", "pinned or latest sampled frame");
    for stat in &data.gpu_stats {
        panels::stat_row(ui, stat);
    }
    ui.add_space(SPACE_2);
    panels::section_header(ui, "HOT SPANS", "last · p50 · p95 · max (ms)");
    for row in &data.hot {
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(RichText::new(&row.name).font(small()).color(TEXT_SECONDARY))
                    .truncate(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for value in [&row.max, &row.p95, &row.p50, &row.last] {
                    ui.add_sized(
                        [40.0, 16.0],
                        egui::Label::new(RichText::new(value).font(mono()).color(TEXT_PRIMARY)),
                    );
                }
            });
        });
    }
}

fn controls(
    ui: &mut Ui,
    data: &ProfilerData,
    state: &mut TimelineState,
    out: &mut Vec<ProfilerInput>,
) {
    ui.horizontal(|ui| {
        let text = if data.enabled {
            "CPU spans on"
        } else {
            "CPU spans off"
        };
        if panels::tool_button(ui, text, data.enabled).clicked() {
            out.push(ProfilerInput::Action(StudioAction::ProfilerEnabled(
                !data.enabled,
            )));
        }
        let text = if data.frozen { "Frozen" } else { "Freeze" };
        if panels::tool_button(ui, text, data.frozen).clicked() {
            out.push(ProfilerInput::Action(StudioAction::ProfilerFrozen(
                !data.frozen,
            )));
        }
        if panels::tool_button(ui, "Follow latest", data.following).clicked() {
            out.push(ProfilerInput::FollowLatest);
        }
        ui.separator();
        ui.label(RichText::new("Zoom").font(small()).color(TEXT_DISABLED));
        if panels::tool_button(ui, "16.7 ms budget", false).clicked() {
            state.set_window(0.0, BUDGET_MS);
        }
        if panels::tool_button(ui, "Engine work", false).clicked() && data.work_end_ms > 0.0 {
            state.set_window(0.0, data.work_end_ms * 1.05);
        }
        if panels::tool_button(ui, "Whole frame", false).clicked() && data.frame_ms > 0.0 {
            state.set_window(0.0, data.frame_ms * 1.02);
        }
        ui.separator();
        let text = if data.capturing {
            "Stop bad-frame capture"
        } else {
            "Capture bad frames"
        };
        if panels::tool_button(ui, text, data.capturing).clicked() {
            out.push(ProfilerInput::Action(StudioAction::CaptureBadFrames));
        }
        if panels::tool_button(ui, "Export trace", false).clicked() {
            out.push(ProfilerInput::Action(StudioAction::ProfilerExport));
        }
        ui.add(
            egui::Label::new(
                RichText::new(&data.export_status)
                    .font(small())
                    .color(TEXT_DISABLED),
            )
            .truncate(),
        );
    });
}

fn frame(ui: &mut Ui, height: f32, sense: Sense) -> (Rect, egui::Response) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), sense);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(RADIUS_SMALL), SURFACE_0);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(RADIUS_SMALL),
        Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    (rect, response)
}

fn strip(ui: &mut Ui, data: &ProfilerData, out: &mut Vec<ProfilerInput>) {
    let (rect, response) = frame(ui, STRIP_HEIGHT, Sense::click());
    let painter = ui.painter_at(rect.shrink(1.0));
    let count = data.bars.len().max(1);
    let width = rect.width() / count as f32;
    let height = rect.height();
    for (index, bar) in data.bars.iter().enumerate() {
        let x = rect.left() + index as f32 * width;
        let column = |fraction: f32| {
            Rect::from_min_max(
                egui::pos2(x, rect.bottom() - height * fraction),
                egui::pos2(x + (width - 1.0).max(1.0), rect.bottom()),
            )
        };
        let color = if bar.selected {
            TEXT_PRIMARY
        } else if bar.tone == 3 {
            ERROR
        } else if bar.tone == 2 {
            WARN
        } else {
            SERIES_INTERVAL.gamma_multiply(0.75)
        };
        painter.rect_filled(column(bar.height), CornerRadius::ZERO, color);
        painter.rect_filled(
            column(bar.cpu),
            CornerRadius::ZERO,
            SERIES_CPU.gamma_multiply(0.85),
        );
        if bar.gpu > 0.0 {
            let y = rect.bottom() - height * bar.gpu;
            painter.rect_filled(
                Rect::from_min_max(
                    egui::pos2(x, y - 1.0),
                    egui::pos2(x + (width - 1.0).max(1.0), y + 1.0),
                ),
                CornerRadius::ZERO,
                SERIES_GPU,
            );
        }
    }
    let budget = rect.bottom() - height * data.budget;
    painter.hline(
        rect.x_range(),
        budget,
        Stroke::new(1.0, WARN.gamma_multiply(0.6)),
    );
    if let Some(pos) = response
        .interact_pointer_pos()
        .filter(|_| response.clicked())
    {
        let index = ((pos.x - rect.left()) / width).floor().max(0.0) as usize;
        if index < data.bars.len() {
            out.push(ProfilerInput::SelectFrame(index));
        }
    }
}

fn timeline(
    ui: &mut Ui,
    data: &ProfilerData,
    state: &mut TimelineState,
    out: &mut Vec<ProfilerInput>,
) {
    let height = ui.available_height().max(AXIS_HEIGHT + 3.0 * ROW_HEIGHT);
    let (rect, response) = frame(ui, height, Sense::click_and_drag());
    let painter = ui.painter_at(rect.shrink(1.0));
    let track = Rect::from_min_max(egui::pos2(rect.left() + LABEL_WIDTH, rect.top()), rect.max);

    // Zoom around the pointer (wheel or pinch), pan by dragging, reset on
    // double-click.
    if response.hovered() {
        let (scroll, zoom) = ui.input(|input| (input.smooth_scroll_delta, input.zoom_delta()));
        let factor = f64::from(zoom) * (f64::from(scroll.y) * 0.004).exp();
        if (factor - 1.0).abs() > 1e-4
            && let Some(pointer) = response.hover_pos()
        {
            let window = state.end_ms - state.start_ms;
            let anchor =
                state.start_ms + f64::from((pointer.x - track.left()) / track.width()) * window;
            let span = (window / factor).clamp(MIN_WINDOW_MS, MAX_WINDOW_MS);
            let fraction = (anchor - state.start_ms) / window;
            state.set_window(anchor - fraction * span, anchor - fraction * span + span);
        }
    }
    if response.dragged() {
        let window = state.end_ms - state.start_ms;
        let shift = f64::from(response.drag_delta().x / track.width()) * window;
        state.set_window(state.start_ms - shift, state.end_ms - shift);
    }
    if response.double_clicked() {
        state.set_window(0.0, BUDGET_MS);
    }
    let window = state.end_ms - state.start_ms;
    let x_of = |ms: f64| track.left() + ((ms - state.start_ms) / window) as f32 * track.width();

    // Lanes: stable order and height.
    let mut lane_top = Vec::with_capacity(state.lanes.len());
    let mut top = rect.top() + AXIS_HEIGHT;
    for (index, (name, rows)) in state.lanes.iter().enumerate() {
        let lane_rect = Rect::from_min_size(
            egui::pos2(rect.left(), top),
            egui::vec2(rect.width(), *rows as f32 * ROW_HEIGHT),
        );
        painter.rect_filled(
            lane_rect,
            CornerRadius::ZERO,
            if index % 2 == 0 { SURFACE_1 } else { SURFACE_0 },
        );
        painter.text(
            egui::pos2(rect.left() + SPACE_2, top + ROW_HEIGHT / 2.0),
            Align2::LEFT_CENTER,
            name,
            small(),
            TEXT_SECONDARY,
        );
        lane_top.push(top);
        top += *rows as f32 * ROW_HEIGHT;
    }
    painter.vline(
        track.left(),
        rect.y_range(),
        Stroke::new(1.0, BORDER_STRONG),
    );

    // Axis ticks for the current window.
    let step = tick_step(window);
    let first = (state.start_ms / step).ceil() as i64;
    let last = (state.end_ms / step).floor() as i64;
    for k in first..=last {
        let t = k as f64 * step;
        let x = x_of(t);
        painter.vline(x, rect.y_range(), Stroke::new(1.0, BORDER));
        painter.text(
            egui::pos2(x + 3.0, rect.top() + 3.0),
            Align2::LEFT_TOP,
            if step < 1.0 {
                format!("{t:.2} ms")
            } else {
                format!("{t:.0} ms")
            },
            small(),
            TEXT_DISABLED,
        );
    }
    let marker = |ms: f64, color: egui::Color32| {
        if ms > state.start_ms && ms < state.end_ms {
            painter.vline(x_of(ms), rect.y_range(), Stroke::new(1.0, color));
        }
    };
    marker(BUDGET_MS, WARN.gamma_multiply(0.7));
    if data.frame_ms > 0.0 {
        marker(data.frame_ms, TEXT_SECONDARY.gamma_multiply(0.6));
    }

    if data.spans.is_empty() {
        painter.text(
            track.center(),
            Align2::CENTER_CENTER,
            &data.empty_hint,
            egui::FontId::proportional(FONT_UI),
            TEXT_DISABLED,
        );
        return;
    }
    let clicked = response
        .interact_pointer_pos()
        .filter(|_| response.clicked());
    let hover = response.hover_pos();
    let mut hovered_name = None;
    let lane_area = Rect::from_min_max(egui::pos2(track.left(), rect.top()), rect.max);
    for (index, span) in data.spans.iter().enumerate() {
        let end = span.start_ms + span.duration_ms;
        if end < state.start_ms || span.start_ms > state.end_ms {
            continue;
        }
        let Some(lane_index) = data
            .lanes
            .get(span.lane)
            .and_then(|lane| state.lanes.iter().position(|(name, _)| *name == lane.name))
        else {
            continue;
        };
        let rows = state.lanes[lane_index].1;
        let row = span.lane_row.min(rows - 1);
        let top = lane_top[lane_index] + row as f32 * ROW_HEIGHT;
        let left = x_of(span.start_ms).max(track.left());
        let right = x_of(end).min(track.right()).max(left + 1.0);
        let span_rect = Rect::from_min_max(
            egui::pos2(left, top + 1.0),
            egui::pos2(right, top + ROW_HEIGHT - 1.0),
        );
        let (fill, text) = match span.kind {
            SpanKind::Ui => (SURFACE_3, TEXT_SECONDARY),
            SpanKind::Wait => (SURFACE_2, TEXT_DISABLED),
            SpanKind::Gpu => (
                SERIES_GPU.gamma_multiply(0.6 + 0.15 * (row % 3) as f32),
                SURFACE_0,
            ),
            SpanKind::Work => (
                SPAN_COLORS[span.color as usize % SPAN_COLORS.len()].gamma_multiply(0.82),
                SURFACE_0,
            ),
        };
        let clip = painter.with_clip_rect(lane_area.intersect(rect));
        clip.rect_filled(span_rect, CornerRadius::same(2), fill);
        if span.selected || matches!(span.kind, SpanKind::Ui | SpanKind::Wait) {
            let stroke = if span.selected {
                TEXT_PRIMARY
            } else {
                BORDER_STRONG
            };
            clip.rect_stroke(
                span_rect,
                CornerRadius::same(2),
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );
        }
        if span_rect.width() > 36.0 {
            clip.with_clip_rect(span_rect).text(
                span_rect.left_center() + egui::vec2(4.0, 0.0),
                Align2::LEFT_CENTER,
                &span.name,
                small(),
                text,
            );
        }
        if hover.is_some_and(|pos| span_rect.contains(pos)) {
            hovered_name = Some(span.name.as_str());
        }
        if clicked.is_some_and(|pos| span_rect.contains(pos)) {
            out.push(ProfilerInput::SelectSpan(index));
        }
    }
    if let Some(name) = hovered_name {
        response.on_hover_text_at_pointer(name);
    }
}
