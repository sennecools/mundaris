//! Studio chrome: toolbar, outliner, inspector, status bar and log
//! (docs/STUDIO_UI.md §4). Panels read the toolkit-free [`StudioView`] and
//! report interactions as [`StudioAction`]s; they hold no engine state.

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};
use mundaris_app::render_settings::{SPECS, SettingKind, SettingValue};
use mundaris_app::studio::view::{
    CAMERA_MODES, Overlay, RATE_PRESETS, StatItem, StudioAction, StudioView,
};
use mundaris_renderer::TerrainViewMode;

use super::theme::{self, *};

const RATE_LABELS: [&str; RATE_PRESETS.len()] = ["1×", "10×", "100×", "1k×", "10k×", "100k×"];
const ALTITUDE_LABELS: [&str; 6] = ["100 km", "10 km", "1 km", "100 m", "10 m", "2 m"];
const OVERLAY_LABELS: [&str; Overlay::ALL.len()] = ["Markers", "Trails", "Orbit guides"];

fn view_mode_name(mode: TerrainViewMode) -> &'static str {
    match mode {
        TerrainViewMode::Lit => "Lit",
        TerrainViewMode::Height => "Height",
        TerrainViewMode::Normals => "Normals",
        TerrainViewMode::Grid => "Grid",
        TerrainViewMode::Level => "LOD level",
        TerrainViewMode::MorphFade => "Morph / fade",
        TerrainViewMode::Unlit => "Unlit (albedo)",
        TerrainViewMode::AoOnly => "AO only",
        TerrainViewMode::Shadows => "Shadow cascades",
        TerrainViewMode::Luminance => "Luminance (stops)",
    }
}

/// A flat button that shows an "on" state with the accent outline.
pub fn tool_button(ui: &mut Ui, text: &str, active: bool) -> egui::Response {
    tool_button_enabled(ui, text, active, true)
}

pub fn tool_button_enabled(ui: &mut Ui, text: &str, active: bool, enabled: bool) -> egui::Response {
    let button = egui::Button::new(RichText::new(text).color(if enabled {
        TEXT_PRIMARY
    } else {
        TEXT_DISABLED
    }))
    .fill(if active { ACCENT_SOFT } else { SURFACE_2 })
    .stroke(Stroke::new(1.0, if active { ACCENT } else { BORDER }))
    .corner_radius(CornerRadius::same(RADIUS_SMALL));
    ui.add_enabled(enabled, button)
}

/// Joined option buttons; returns the chosen index when clicked.
fn segmented(ui: &mut Ui, options: &[&str], selected: Option<usize>) -> Option<usize> {
    let mut chosen = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (index, option) in options.iter().enumerate() {
            if tool_button(ui, option, selected == Some(index)).clicked() {
                chosen = Some(index);
            }
        }
    });
    chosen
}

pub fn section_header(ui: &mut Ui, title: &str, detail: &str) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .font(small())
                .strong()
                .color(TEXT_SECONDARY),
        );
        if !detail.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(detail).font(small()).color(TEXT_DISABLED));
            });
        }
    });
}

pub fn stat_row(ui: &mut Ui, stat: &StatItem) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(&stat.label).color(TEXT_SECONDARY));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(&stat.value)
                    .font(mono())
                    .color(theme::tone_color(stat.tone)),
            );
        });
    });
}

pub fn card(ui: &mut Ui, title: &str, add_contents: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(SURFACE_0)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(SPACE_3 as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = SPACE_1;
            section_header(ui, title, "");
            add_contents(ui);
        });
}

/// Top toolbar. `bottom_open` is UI-local (the Profiler/Log panel).
pub fn toolbar(
    ui: &mut Ui,
    view: &StudioView,
    time_text: &str,
    bottom_open: &mut bool,
    actions: &mut Vec<StudioAction>,
) {
    ui.horizontal_centered(|ui| {
        ui.label(
            RichText::new("Mundaris")
                .size(FONT_HEADING)
                .strong()
                .color(TEXT_PRIMARY),
        );
        ui.add_space(SPACE_2);
        let play = if view.paused {
            "▶  Play"
        } else {
            "⏸  Pause"
        };
        if tool_button(ui, play, !view.paused).clicked() {
            actions.push(StudioAction::TogglePause);
        }
        if let Some(index) = segmented(ui, &RATE_LABELS, view.rate_index) {
            actions.push(StudioAction::SetRate(index));
        }
        ui.add_sized(
            [120.0, 20.0],
            egui::Label::new(RichText::new(time_text).font(mono()).color(TEXT_SECONDARY)),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if tool_button(ui, "Capture", false).clicked() {
                actions.push(StudioAction::Capture);
            }
            if tool_button(ui, "Profiler", *bottom_open).clicked() {
                *bottom_open = !*bottom_open;
            }
            if tool_button(ui, "Labels", view.labels_enabled).clicked() {
                actions.push(StudioAction::ToggleLabels);
            }
            if tool_button(ui, "Terrain", view.terrain_enabled).clicked() {
                actions.push(StudioAction::ToggleTerrain);
            }
            let current = TerrainViewMode::ALL
                .get(view.view_index)
                .copied()
                .map_or("—", view_mode_name);
            egui::ComboBox::from_id_salt("view-mode")
                .width(130.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (index, mode) in TerrainViewMode::ALL.iter().enumerate() {
                        if ui
                            .selectable_label(index == view.view_index, view_mode_name(*mode))
                            .clicked()
                        {
                            actions.push(StudioAction::SetView(index));
                        }
                    }
                });
            ui.label(RichText::new("View").font(small()).color(TEXT_DISABLED));
            // Right-to-left: add the camera options reversed so they read left to right.
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for index in (0..CAMERA_MODES.len()).rev() {
                    if tool_button(ui, CAMERA_MODES[index], view.camera_index == index).clicked() {
                        actions.push(StudioAction::SetCamera(index));
                    }
                }
            });
            ui.label(RichText::new("Camera").font(small()).color(TEXT_DISABLED));
        });
    });
}

/// Scene outliner: click selects, double-click focuses.
pub fn outliner(ui: &mut Ui, view: &StudioView, actions: &mut Vec<StudioAction>) {
    section_header(ui, "SCENE", &format!("{} bodies", view.bodies.len()));
    ui.add_space(SPACE_1);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height(ui.available_height() - 24.0)
        .show(ui, |ui| {
            for (index, body) in view.bodies.iter().enumerate() {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 30.0),
                    egui::Sense::click(),
                );
                let fill = if body.selected {
                    ACCENT_SOFT
                } else if response.hovered() {
                    SURFACE_2
                } else {
                    Color32::TRANSPARENT
                };
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, CornerRadius::same(RADIUS_SMALL), fill);
                let x = rect.left() + SPACE_2 + body.depth as f32 * 14.0;
                let dot = if body.focused {
                    OK
                } else if body.selected {
                    ACCENT
                } else {
                    BORDER_STRONG
                };
                painter.circle_filled(egui::pos2(x + 4.0, rect.center().y), 4.0, dot);
                painter.text(
                    egui::pos2(x + 16.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &body.name,
                    egui::FontId::proportional(FONT_UI),
                    TEXT_PRIMARY,
                );
                painter.text(
                    egui::pos2(rect.right() - SPACE_2, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    &body.detail,
                    small(),
                    TEXT_DISABLED,
                );
                if response.double_clicked() {
                    actions.push(StudioAction::FocusBody(index));
                } else if response.clicked() {
                    actions.push(StudioAction::SelectBody(index));
                }
            }
        });
    ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
        ui.label(
            RichText::new("Click selects · double-click focuses")
                .font(small())
                .color(TEXT_DISABLED),
        );
    });
}

/// Inspector for the selected body, camera, terrain and overlays.
/// `panel` is the throttled copy used for readable text; `view` is live state.
pub fn inspector(
    ui: &mut Ui,
    view: &StudioView,
    panel: &StudioView,
    actions: &mut Vec<StudioAction>,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = SPACE_3;
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(
                    RichText::new(&panel.inspector_title)
                        .size(18.0)
                        .strong()
                        .color(TEXT_PRIMARY),
                );
                ui.label(RichText::new(&panel.inspector_subtitle).color(TEXT_SECONDARY));
            });
            ui.horizontal(|ui| {
                if tool_button(ui, "Frame", false).clicked() {
                    actions.push(StudioAction::FrameSelected);
                }
                if tool_button_enabled(ui, "Walk surface", false, view.surface_available).clicked()
                {
                    actions.push(StudioAction::SurfaceNavigation);
                }
            });
            card(ui, "BODY", |ui| {
                for stat in &panel.body_stats {
                    stat_row(ui, stat);
                }
            });
            card(ui, "CAMERA", |ui| {
                for stat in &panel.camera_stats {
                    stat_row(ui, stat);
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new("0.01×").font(small()).color(TEXT_DISABLED));
                    let mut exponent = view.speed_exponent;
                    ui.spacing_mut().slider_width = (ui.available_width() - 40.0).max(60.0);
                    if ui
                        .add(egui::Slider::new(&mut exponent, -2.0..=2.0).show_value(false))
                        .changed()
                    {
                        actions.push(StudioAction::SetSpeedExponent(exponent));
                    }
                    ui.label(RichText::new("100×").font(small()).color(TEXT_DISABLED));
                });
                section_header(ui, "GO TO ALTITUDE", "");
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for (index, label) in ALTITUDE_LABELS.iter().enumerate() {
                        if tool_button(ui, label, false).clicked() {
                            actions.push(StudioAction::Approach(index));
                        }
                    }
                });
            });
            card(ui, "TERRAIN", |ui| {
                for stat in &panel.terrain_stats {
                    stat_row(ui, stat);
                }
            });
            card(ui, "OVERLAYS", |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (index, overlay) in Overlay::ALL.iter().enumerate() {
                        if tool_button(ui, OVERLAY_LABELS[index], view.overlays[index]).clicked() {
                            actions.push(StudioAction::ToggleOverlay(*overlay));
                        }
                    }
                });
            });
            render_card(ui, view, panel, actions);
        });
}

/// Render settings generated from the registry, grouped, plus per-pass GPU
/// times. The view mode lives in the toolbar.
fn render_card(
    ui: &mut Ui,
    view: &StudioView,
    panel: &StudioView,
    actions: &mut Vec<StudioAction>,
) {
    card(ui, "RENDER", |ui| {
        for stat in &panel.render_stats {
            stat_row(ui, stat);
        }
        let mut groups: Vec<&str> = Vec::new();
        for spec in SPECS.iter().filter(|spec| spec.id != "render.view_mode") {
            if !groups.contains(&spec.group) {
                groups.push(spec.group);
            }
        }
        for group in groups {
            egui::CollapsingHeader::new(RichText::new(group).color(TEXT_SECONDARY))
                .id_salt(("render-group", group))
                .default_open(false)
                .show(ui, |ui| {
                    for (index, spec) in SPECS.iter().enumerate() {
                        if spec.group != group || spec.id == "render.view_mode" {
                            continue;
                        }
                        let Some(value) = view.render_settings.get(index).copied() else {
                            continue;
                        };
                        if let Some(value) =
                            setting_widget(ui, spec.id, spec.label, spec.kind, value)
                        {
                            actions.push(StudioAction::SetSetting(index, value));
                        }
                    }
                });
        }
        if tool_button(ui, "Reset render settings", false).clicked() {
            actions.push(StudioAction::ResetRenderSettings);
        }
    });
}

/// One registry widget; returns the new value when the user changed it.
fn setting_widget(
    ui: &mut Ui,
    id: &str,
    label: &str,
    kind: SettingKind,
    value: SettingValue,
) -> Option<SettingValue> {
    match (kind, value) {
        (SettingKind::Bool, SettingValue::Bool(mut on)) => ui
            .checkbox(&mut on, RichText::new(label).color(TEXT_PRIMARY))
            .changed()
            .then_some(SettingValue::Bool(on)),
        (
            SettingKind::Float {
                min,
                max,
                logarithmic,
                unit,
            },
            SettingValue::Float(mut x),
        ) => {
            ui.label(RichText::new(label).font(small()).color(TEXT_SECONDARY));
            ui.spacing_mut().slider_width = (ui.available_width() - 70.0).max(60.0);
            let slider = egui::Slider::new(&mut x, min..=max)
                .logarithmic(logarithmic)
                .suffix(if unit.is_empty() {
                    String::new()
                } else {
                    format!(" {unit}")
                });
            ui.add(slider).changed().then_some(SettingValue::Float(x))
        }
        (SettingKind::Choice(options), SettingValue::Choice(selected)) => {
            let mut chosen = None;
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(TEXT_SECONDARY));
                egui::ComboBox::from_id_salt(id)
                    .selected_text(options.get(selected).copied().unwrap_or("—"))
                    .show_ui(ui, |ui| {
                        for (index, option) in options.iter().enumerate() {
                            if ui.selectable_label(index == selected, *option).clicked() {
                                chosen = Some(SettingValue::Choice(index));
                            }
                        }
                    });
            });
            chosen
        }
        _ => None,
    }
}

/// Status bar; returns true when the user stops an automation lease.
pub fn status_bar(ui: &mut Ui, view: &StudioView, panel: &StudioView) -> bool {
    let mut stop = false;
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = SPACE_1;
        for item in &panel.status {
            ui.label(
                RichText::new(&item.label)
                    .font(small())
                    .color(TEXT_DISABLED),
            );
            ui.label(
                RichText::new(&item.value)
                    .font(mono())
                    .color(theme::tone_color(item.tone)),
            );
            ui.add_space(SPACE_3);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(&panel.build_text)
                    .font(small())
                    .color(TEXT_DISABLED),
            );
            if !view.automation_owner.is_empty() {
                if tool_button(ui, "Stop", false).clicked() {
                    stop = true;
                }
                ui.label(
                    RichText::new(format!("Automation: {}", view.automation_owner))
                        .font(small())
                        .color(WARN),
                );
            }
        });
    });
    stop
}

pub fn log(ui: &mut Ui, panel: &StudioView) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &panel.log {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&line.time).font(mono()).color(TEXT_DISABLED));
                    ui.label(RichText::new(&line.text).color(theme::tone_color(line.tone)));
                });
            }
        });
}
