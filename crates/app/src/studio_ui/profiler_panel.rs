//! Profiler panel: frame strip, per-frame timeline with CPU lanes and the GPU
//! lane, and stat cards, painted from the cached [`ProfilerData`].

use egui::{Align, Align2, CornerRadius, Layout, Rect, RichText, Sense, Stroke, Ui};
use mundaris_app::studio::{profiler_view::SpanKind, view::StudioAction};

use super::{panels, theme::*};
use crate::profiler_ui::ProfilerData;

/// One profiler interaction for the frame owner.
pub enum ProfilerInput {
    Action(StudioAction),
    SelectFrame(usize),
    FollowLatest,
    SelectSpan(usize),
    FullFrame(bool),
}

const ROW_HEIGHT: f32 = 18.0;
const LABEL_WIDTH: f32 = 120.0;
const AXIS_HEIGHT: f32 = 18.0;

pub fn show(ui: &mut Ui, data: &ProfilerData) -> Vec<ProfilerInput> {
    let mut out = Vec::new();
    ui.horizontal_top(|ui| {
        let stats_width = 330.0;
        let main_width = (ui.available_width() - stats_width - SPACE_3).max(200.0);
        ui.vertical(|ui| {
            ui.set_width(main_width);
            controls(ui, data, &mut out);
            panels::section_header(
                ui,
                "FRAMES",
                "interval (bar) · engine CPU (green) · GPU (purple mark, sampled frames) · 16.7 ms line · click to inspect",
            );
            strip(ui, data, &mut out);
            panels::section_header(ui, "TIMELINE", &data.timeline_title);
            timeline(ui, data, &mut out);
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("profiler-stats")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(stats_width - SPACE_3);
                // The scroll area inherits the row layout; stats stack vertically.
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
    if !data.span_detail.is_empty() {
        panels::section_header(ui, "SELECTED SPAN", "");
        for stat in &data.span_detail {
            panels::stat_row(ui, stat);
        }
    }
    panels::section_header(ui, "GPU SCOPES", "selected or latest sampled frame");
    for stat in &data.gpu_stats {
        panels::stat_row(ui, stat);
    }
    panels::section_header(ui, "HOT SPANS", "last · p50 · p95 · max (ms)");
    for row in &data.hot {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&row.name).font(small()).color(TEXT_SECONDARY));
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

fn controls(ui: &mut Ui, data: &ProfilerData, out: &mut Vec<ProfilerInput>) {
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
        if panels::tool_button(ui, "Latest frame", data.following).clicked() {
            out.push(ProfilerInput::FollowLatest);
        }
        let text = if data.full_frame {
            "Full frame"
        } else {
            "Engine work"
        };
        if panels::tool_button(ui, text, !data.full_frame).clicked() {
            out.push(ProfilerInput::FullFrame(!data.full_frame));
        }
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
        ui.label(
            RichText::new(&data.export_status)
                .font(small())
                .color(TEXT_DISABLED),
        );
    });
}

fn frame(ui: &mut Ui, height: f32) -> (Rect, egui::Response) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
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
    let (rect, response) = frame(ui, 56.0);
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

fn timeline(ui: &mut Ui, data: &ProfilerData, out: &mut Vec<ProfilerInput>) {
    let wanted = AXIS_HEIGHT + data.rows.max(1) as f32 * ROW_HEIGHT + 4.0;
    let height = wanted
        .max(ui.available_height() - 4.0)
        .min(ui.available_height().max(60.0));
    let (rect, response) = frame(ui, height.max(60.0));
    let painter = ui.painter_at(rect.shrink(1.0));
    if data.spans.is_empty() {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            &data.empty_hint,
            egui::FontId::proportional(FONT_UI),
            TEXT_DISABLED,
        );
        return;
    }
    let track = Rect::from_min_max(egui::pos2(rect.left() + LABEL_WIDTH, rect.top()), rect.max);
    let row_rect = |row: i32| {
        let top = rect.top() + AXIS_HEIGHT + row as f32 * ROW_HEIGHT;
        (top, top + ROW_HEIGHT)
    };
    for (index, lane) in data.lanes.iter().enumerate() {
        let (top, _) = row_rect(lane.first_row);
        let lane_rect = Rect::from_min_size(
            egui::pos2(rect.left(), top),
            egui::vec2(rect.width(), lane.rows as f32 * ROW_HEIGHT),
        );
        painter.rect_filled(
            lane_rect,
            CornerRadius::ZERO,
            if index % 2 == 0 { SURFACE_1 } else { SURFACE_0 },
        );
        painter.text(
            egui::pos2(rect.left() + SPACE_2, top + ROW_HEIGHT / 2.0),
            Align2::LEFT_CENTER,
            &lane.name,
            small(),
            TEXT_SECONDARY,
        );
    }
    for tick in &data.ticks {
        let x = track.left() + tick.x * track.width();
        painter.vline(x, rect.y_range(), Stroke::new(1.0, BORDER));
        painter.text(
            egui::pos2(x + 3.0, rect.top() + 2.0),
            Align2::LEFT_TOP,
            &tick.text,
            small(),
            TEXT_DISABLED,
        );
    }
    let clicked = response
        .interact_pointer_pos()
        .filter(|_| response.clicked());
    for (index, span) in data.spans.iter().enumerate() {
        let (top, bottom) = row_rect(span.row);
        let x = track.left() + span.x * track.width();
        let span_rect = Rect::from_min_max(
            egui::pos2(x, top + 1.0),
            egui::pos2(x + (span.width * track.width()).max(1.0), bottom - 1.0),
        );
        let (fill, text) = match span.kind {
            SpanKind::Ui => (SURFACE_3, TEXT_SECONDARY),
            SpanKind::Wait => (SURFACE_2, TEXT_DISABLED),
            SpanKind::Gpu => (
                SERIES_GPU.gamma_multiply(0.6 + 0.15 * (span.row % 3) as f32),
                SURFACE_0,
            ),
            SpanKind::Work => (
                SPAN_COLORS[span.color as usize % SPAN_COLORS.len()].gamma_multiply(0.82),
                SURFACE_0,
            ),
        };
        painter.rect_filled(span_rect, CornerRadius::same(2), fill);
        if span.selected || matches!(span.kind, SpanKind::Ui | SpanKind::Wait) {
            let stroke = if span.selected {
                TEXT_PRIMARY
            } else {
                BORDER_STRONG
            };
            painter.rect_stroke(
                span_rect,
                CornerRadius::same(2),
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );
        }
        if span_rect.width() > 36.0 {
            let (anchor, pos) = if span.clipped {
                (
                    Align2::RIGHT_CENTER,
                    span_rect.right_center() - egui::vec2(4.0, 0.0),
                )
            } else {
                (
                    Align2::LEFT_CENTER,
                    span_rect.left_center() + egui::vec2(4.0, 0.0),
                )
            };
            painter.with_clip_rect(span_rect.intersect(rect)).text(
                pos,
                anchor,
                &span.name,
                small(),
                text,
            );
        }
        if clicked.is_some_and(|pos| span_rect.contains(pos)) {
            out.push(ProfilerInput::SelectSpan(index));
        }
    }
}
