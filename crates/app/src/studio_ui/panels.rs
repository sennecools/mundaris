//! Studio chrome: top bar, outliner, tabbed inspector, status bar and log
//! (docs/STUDIO_UI.md §4). Panels read the toolkit-free [`StudioView`] and
//! report interactions as [`StudioAction`]s; they hold no engine state.
//!
//! Inspector rows follow one property-grid layout: a fixed label column and a
//! value column, at a fixed row height, so values never shift the layout.

use std::ops::RangeInclusive;

use astrum_app::render_settings::{SPECS, SettingKind, SettingValue};
use astrum_app::studio::view::{
    Overlay, ParamEdit, ParamItem, ParamTarget, PlanetView, RATE_PRESETS, StatItem, StudioAction,
    StudioView,
};
use astrum_core::params::{ParamKind, ParamValue};
use astrum_renderer::TerrainViewMode;
use egui::{Align, Color32, CornerRadius, Layout, RichText, Ui};

use super::theme::{self, *};
use super::{InspectorTab, Workspace};

const RATE_LABELS: [&str; RATE_PRESETS.len()] = ["1×", "10×", "100×", "1k×", "10k×", "100k×"];
const ALTITUDE_LABELS: [&str; 6] = ["100 km", "10 km", "1 km", "100 m", "10 m", "2 m"];
const OVERLAY_LABELS: [&str; Overlay::ALL.len()] = ["Markers", "Trails", "Orbit guides"];
/// Property-grid row height and label column share.
const ROW_HEIGHT: f32 = 24.0;
const LABEL_SHARE: f32 = 0.42;
/// Width of the value box beside a [`fill_slider`].
const VALUE_WIDTH: f32 = 72.0;

pub fn view_mode_name(mode: TerrainViewMode) -> &'static str {
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
        TerrainViewMode::Elevation => "Elevation",
        TerrainViewMode::OceanMask => "Ocean mask",
        TerrainViewMode::Temperature => "Temperature",
        TerrainViewMode::Moisture => "Moisture",
        TerrainViewMode::Wind => "Wind",
        TerrainViewMode::Biome => "Biome",
        TerrainViewMode::Uplift => "Uplift",
        TerrainViewMode::Hardness => "Rock hardness",
        TerrainViewMode::Sediment => "Sediment",
        TerrainViewMode::Flow => "Flow",
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
    .stroke(egui::Stroke::new(1.0, if active { ACCENT } else { BORDER }))
    .corner_radius(CornerRadius::same(RADIUS_SMALL));
    ui.add_enabled(enabled, button)
}

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

/// A tab strip; returns the clicked tab.
fn tabs(ui: &mut Ui, names: &[&str], selected: usize) -> Option<usize> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (index, name) in names.iter().enumerate() {
            let active = index == selected;
            let text =
                RichText::new(*name).color(if active { TEXT_PRIMARY } else { TEXT_SECONDARY });
            let button = egui::Button::new(text)
                .fill(if active {
                    SURFACE_3
                } else {
                    Color32::TRANSPARENT
                })
                .stroke(egui::Stroke::NONE)
                .min_size(egui::vec2(0.0, ROW_HEIGHT));
            if ui.add(button).clicked() {
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
                ui.add(
                    egui::Label::new(RichText::new(detail).font(small()).color(TEXT_DISABLED))
                        .truncate(),
                );
            });
        }
    });
}

/// Read-only property row: label left, value right.
pub fn stat_row(ui: &mut Ui, stat: &StatItem) {
    ui.horizontal(|ui| {
        ui.set_min_height(20.0);
        ui.label(RichText::new(&stat.label).color(TEXT_SECONDARY));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Long values (errors, cascade splits) truncate with a hover
            // tooltip instead of running past the panel edge.
            ui.add(
                egui::Label::new(
                    RichText::new(&stat.value)
                        .font(mono())
                        .color(theme::tone_color(stat.tone)),
                )
                .truncate(),
            );
        });
    });
}

/// Editable property row: a fixed label column, then `add` fills the rest.
fn prop_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.set_min_height(ROW_HEIGHT);
        let width = (ui.available_width() * LABEL_SHARE).floor();
        ui.allocate_ui_with_layout(
            egui::vec2(width, ROW_HEIGHT),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_width(width);
                ui.add(egui::Label::new(RichText::new(label).color(TEXT_SECONDARY)).truncate());
            },
        );
        add(ui)
    })
    .inner
}

/// A collapsible inspector section.
fn section(ui: &mut Ui, title: &str, open: bool, add: impl FnOnce(&mut Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong().color(TEXT_PRIMARY))
        .id_salt(("inspector-section", title))
        .default_open(open)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            add(ui);
        });
}

/// A slider that fills the rest of the row, with a fixed-width value box on
/// its right, so long values never widen the panel past its edge.
fn fill_slider<N: egui::emath::Numeric>(
    ui: &mut Ui,
    value: &mut N,
    range: RangeInclusive<N>,
    logarithmic: bool,
    suffix: &str,
) -> egui::Response {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let span = range.end().to_f64() - range.start().to_f64();
        let drag = ui.add_sized(
            [VALUE_WIDTH, ui.spacing().interact_size.y],
            egui::DragValue::new(&mut *value)
                .range(range.clone())
                .speed(span / 300.0)
                .max_decimals(if N::INTEGRAL { 0 } else { 3 })
                .suffix(suffix),
        );
        fill_row(ui);
        let slider = ui.add(
            egui::Slider::new(value, range)
                .logarithmic(logarithmic)
                .show_value(false),
        );
        slider.union(drag)
    })
    .inner
}

/// Sizes the next slider to the width left in the row.
fn fill_row(ui: &mut Ui) {
    ui.spacing_mut().slider_width = (ui.available_width() - ui.spacing().item_spacing.x).max(40.0);
}

/// Top bar: name, workspaces, playback, scene toggles.
pub fn toolbar(
    ui: &mut Ui,
    view: &StudioView,
    time_text: &str,
    workspace: &mut Workspace,
    panels: &mut PanelToggles,
    actions: &mut Vec<StudioAction>,
) {
    ui.horizontal_centered(|ui| {
        ui.label(
            RichText::new("Astrum")
                .size(FONT_HEADING)
                .strong()
                .color(TEXT_PRIMARY),
        );
        ui.add_space(SPACE_3);
        if let Some(index) = tabs(ui, &Workspace::NAMES, workspace.index()) {
            *workspace = Workspace::ALL[index];
        }
        ui.add_space(SPACE_1);
        panels_menu(ui, panels);
        ui.add_space(SPACE_3);
        ui.separator();
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
            if tool_button(ui, "Labels", view.labels_enabled).clicked() {
                actions.push(StudioAction::ToggleLabels);
            }
            if tool_button(ui, "Terrain", view.terrain_enabled).clicked() {
                actions.push(StudioAction::ToggleTerrain);
            }
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
                    egui::vec2(ui.available_width(), 28.0),
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

/// Tabbed inspector for the selected body. `panel` is the throttled copy used
/// for readable text; `view` is live state.
/// One inspector page (a dock tab since the docking amendment). The Body page
/// opens with the selected body's name.
pub fn inspector(
    ui: &mut Ui,
    view: &StudioView,
    panel: &StudioView,
    tab: InspectorTab,
    actions: &mut Vec<StudioAction>,
) {
    if tab == InspectorTab::Body {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(
                RichText::new(&panel.inspector_title)
                    .size(17.0)
                    .strong()
                    .color(TEXT_PRIMARY),
            );
            ui.label(RichText::new(&panel.inspector_subtitle).color(TEXT_SECONDARY));
        });
        ui.separator();
    }
    egui::ScrollArea::vertical()
        .id_salt(("inspector-scroll", tab.index()))
        .auto_shrink([false, false])
        .show(ui, |ui| match tab {
            InspectorTab::Body => body_tab(ui, view, panel, actions),
            InspectorTab::Planet => planet_tab(ui, view.planet.as_ref(), actions),
            InspectorTab::Render => render_tab(ui, view, panel, actions),
        });
}

fn body_tab(ui: &mut Ui, view: &StudioView, panel: &StudioView, actions: &mut Vec<StudioAction>) {
    ui.horizontal(|ui| {
        if tool_button(ui, "Frame", false).clicked() {
            actions.push(StudioAction::FrameSelected);
        }
        if tool_button_enabled(ui, "Walk surface", false, view.surface_available).clicked() {
            actions.push(StudioAction::SurfaceNavigation);
        }
    });
    section(ui, "Body", true, |ui| {
        for stat in &panel.body_stats {
            stat_row(ui, stat);
        }
    });
    section(ui, "Camera", true, |ui| {
        for stat in &panel.camera_stats {
            stat_row(ui, stat);
        }
        prop_row(ui, "Fly speed", |ui| {
            let mut exponent = view.speed_exponent;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // The current multiplier sits where the slider ends.
                ui.add_sized(
                    [48.0, ROW_HEIGHT],
                    egui::Label::new(
                        RichText::new(format!("{:.2}×", 10f32.powf(exponent)))
                            .font(mono())
                            .color(TEXT_PRIMARY),
                    ),
                );
                fill_row(ui);
                if ui
                    .add(egui::Slider::new(&mut exponent, -2.0..=2.0).show_value(false))
                    .changed()
                {
                    actions.push(StudioAction::SetSpeedExponent(exponent));
                }
            });
        });
        prop_row(ui, "Look speed", |ui| {
            let mut exponent = view.look_exponent;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_sized(
                    [48.0, ROW_HEIGHT],
                    egui::Label::new(
                        RichText::new(format!("{:.2}×", 10f32.powf(exponent)))
                            .font(mono())
                            .color(TEXT_PRIMARY),
                    ),
                );
                fill_row(ui);
                let response = ui
                    .add(egui::Slider::new(&mut exponent, -1.0..=1.0).show_value(false))
                    .on_hover_text("Drag look response; 1.00× = the scene follows the pointer");
                if response.changed() {
                    actions.push(StudioAction::SetLookSensitivity(exponent));
                }
            });
        });
        ui.label(
            RichText::new("Go to altitude")
                .font(small())
                .color(TEXT_DISABLED),
        );
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (index, label) in ALTITUDE_LABELS.iter().enumerate() {
                if tool_button(ui, label, false).clicked() {
                    actions.push(StudioAction::Approach(index));
                }
            }
        });
    });
    section(ui, "Terrain", true, |ui| {
        for stat in &panel.terrain_stats {
            stat_row(ui, stat);
        }
    });
    section(ui, "Overlays", false, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (index, overlay) in Overlay::ALL.iter().enumerate() {
                if tool_button(ui, OVERLAY_LABELS[index], view.overlays[index]).clicked() {
                    actions.push(StudioAction::ToggleOverlay(*overlay));
                }
            }
        });
    });
}

/// Generic parameter panel (docs/STUDIO_UI.md amendment 2026-10-10b): one
/// collapsible section per group, in item order, and one property row per
/// item, whatever content type the items describe. `open` picks the groups
/// that start expanded.
pub fn param_sections(
    ui: &mut Ui,
    items: &[ParamItem],
    open: impl Fn(&str) -> bool,
    edits: &mut Vec<ParamEdit>,
) {
    let mut groups: Vec<&str> = Vec::new();
    for item in items {
        if !groups.contains(&item.group) {
            groups.push(item.group);
        }
    }
    for group in groups {
        section(ui, group, open(group), |ui| {
            for (index, item) in items.iter().enumerate() {
                if item.group == group {
                    param_row(ui, index, item, edits);
                }
            }
        });
    }
}

/// One parameter row: label (bold when overridden, with a reset button) and
/// the kind's widget. Sliders apply on release, so a drag costs one rebuild.
fn param_row(ui: &mut Ui, index: usize, item: &ParamItem, edits: &mut Vec<ParamEdit>) {
    ui.horizontal(|ui| {
        ui.set_min_height(ROW_HEIGHT);
        let width = (ui.available_width() * LABEL_SHARE).floor();
        ui.allocate_ui_with_layout(
            egui::vec2(width, ROW_HEIGHT),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.x = 2.0;
                if item.overridden {
                    if ui
                        .small_button("↺")
                        .on_hover_text("Back to the default value")
                        .clicked()
                    {
                        edits.push(ParamEdit::Reset(index));
                    }
                } else {
                    // Same footprint as the reset button, so labels line up.
                    ui.add_space(18.0);
                }
                let label = if item.unit.is_empty() {
                    item.label.clone()
                } else {
                    format!("{} ({})", item.label, item.unit)
                };
                let text = RichText::new(label);
                let response = ui.add(
                    egui::Label::new(if item.overridden {
                        text.color(TEXT_PRIMARY).strong()
                    } else {
                        text.color(TEXT_SECONDARY)
                    })
                    .truncate(),
                );
                if !item.help.is_empty() {
                    response.on_hover_text(item.help);
                }
            },
        );
        let released = |r: &egui::Response| r.drag_stopped() || (r.changed() && !r.dragged());
        let edit = match (item.kind, item.value) {
            (ParamKind::Float { min, max, log }, ParamValue::Float(mut x)) => {
                let response = fill_slider(ui, &mut x, min..=max, log, "");
                released(&response).then_some(ParamValue::Float(x))
            }
            (ParamKind::Int { min, max }, ParamValue::Int(mut i)) => {
                let response = fill_slider(ui, &mut i, min..=max, false, "");
                released(&response).then_some(ParamValue::Int(i))
            }
            (ParamKind::Choice { options }, ParamValue::Choice(selected)) => {
                let mut chosen = None;
                egui::ComboBox::from_id_salt(("param", item.key))
                    .width(ui.available_width() - SPACE_1)
                    .selected_text(options.get(selected).copied().unwrap_or("—"))
                    .show_ui(ui, |ui| {
                        for (option_index, option) in options.iter().enumerate() {
                            if ui
                                .selectable_label(option_index == selected, *option)
                                .clicked()
                            {
                                chosen = Some(ParamValue::Choice(option_index));
                            }
                        }
                    });
                chosen
            }
            (ParamKind::Bool, ParamValue::Bool(mut on)) => ui
                .checkbox(&mut on, "")
                .changed()
                .then_some(ParamValue::Bool(on)),
            _ => None,
        };
        if let Some(value) = edit {
            edits.push(ParamEdit::Set(index, value));
        }
    });
}

/// Planet editor (pipeline §18.1): seed, grouped parameters, undo, save.
fn planet_tab(ui: &mut Ui, planet: Option<&PlanetView>, actions: &mut Vec<StudioAction>) {
    let Some(planet) = planet else {
        ui.add_space(SPACE_3);
        ui.label(
            RichText::new("This body has no editable world map. Select Rust in the scene list.")
                .color(TEXT_SECONDARY),
        );
        return;
    };
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(&planet.archetype)
                .strong()
                .color(TEXT_PRIMARY),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (text, color) = if planet.dirty {
                ("unsaved changes", WARN)
            } else {
                (planet.terrain_file.as_str(), TEXT_DISABLED)
            };
            ui.add(egui::Label::new(RichText::new(text).font(small()).color(color)).truncate());
        });
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if tool_button_enabled(ui, "Undo", false, planet.can_undo)
            .on_hover_text("Ctrl+Z")
            .clicked()
        {
            actions.push(StudioAction::PlanetUndo);
        }
        if tool_button_enabled(ui, "Redo", false, planet.can_redo)
            .on_hover_text("Ctrl+Shift+Z")
            .clicked()
        {
            actions.push(StudioAction::PlanetRedo);
        }
        ui.add_space(SPACE_2);
        if tool_button_enabled(ui, "Save", planet.dirty, planet.dirty).clicked() {
            actions.push(StudioAction::PlanetSave);
        }
        if tool_button_enabled(ui, "Revert", false, planet.dirty).clicked() {
            actions.push(StudioAction::PlanetRevert);
        }
    });
    // Fixed-height status line, so the rows below never move.
    ui.horizontal(|ui| {
        ui.set_min_height(20.0);
        if planet.baking {
            ui.add(egui::Spinner::new().size(12.0).color(WARN));
            ui.label(
                RichText::new("Rebuilding the world map…")
                    .font(small())
                    .color(WARN),
            );
        } else {
            ui.label(
                RichText::new("World map up to date")
                    .font(small())
                    .color(TEXT_DISABLED),
            );
        }
    });
    ui.separator();
    prop_row(ui, "Seed", |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if tool_button(ui, "Random", false).clicked() {
                actions.push(StudioAction::PlanetRandomSeed);
            }
            let mut seed = planet.seed;
            // The view shows the edit's seed, so drags accumulate; rebuilds
            // are throttled by the editor and a drag undoes as one step.
            let width = ui.available_width() - ui.spacing().item_spacing.x;
            if ui
                .add_sized(
                    [width.max(40.0), ui.spacing().interact_size.y],
                    egui::DragValue::new(&mut seed).speed(1.0),
                )
                .changed()
            {
                actions.push(StudioAction::PlanetSeed(seed));
            }
        });
    });
    // Generic descriptor panel (`archetype::PARAM_FIELDS`, registry order).
    let mut edits = Vec::new();
    param_sections(
        ui,
        &planet.params,
        |group| group == "Continents",
        &mut edits,
    );
    actions.extend(
        edits
            .into_iter()
            .map(|edit| StudioAction::EditParam(ParamTarget::Planet, edit)),
    );
    section(ui, "Status", false, |ui| {
        for stat in &planet.stats {
            stat_row(ui, stat);
        }
    });
}

/// Render settings generated from the registry, grouped, plus per-pass GPU
/// times. The view mode lives on the viewport.
fn render_tab(ui: &mut Ui, view: &StudioView, panel: &StudioView, actions: &mut Vec<StudioAction>) {
    section(ui, "GPU passes", true, |ui| {
        for stat in &panel.render_stats {
            stat_row(ui, stat);
        }
    });
    let mut groups: Vec<&str> = Vec::new();
    for spec in SPECS.iter().filter(|spec| spec.id != "render.view_mode") {
        if !groups.contains(&spec.group) {
            groups.push(spec.group);
        }
    }
    for group in groups {
        section(ui, group, false, |ui| {
            for (index, spec) in SPECS.iter().enumerate() {
                if spec.group != group || spec.id == "render.view_mode" {
                    continue;
                }
                let Some(value) = view.render_settings.get(index).copied() else {
                    continue;
                };
                if let Some(value) = setting_widget(ui, spec.id, spec.label, spec.kind, value) {
                    actions.push(StudioAction::SetSetting(index, value));
                }
            }
        });
    }
    ui.add_space(SPACE_2);
    if tool_button(ui, "Reset render settings", false).clicked() {
        actions.push(StudioAction::ResetRenderSettings);
    }
}

/// One registry widget as a property row; returns the new value when the
/// user changed it.
fn setting_widget(
    ui: &mut Ui,
    id: &str,
    label: &str,
    kind: SettingKind,
    value: SettingValue,
) -> Option<SettingValue> {
    match (kind, value) {
        (SettingKind::Bool, SettingValue::Bool(mut on)) => prop_row(ui, label, |ui| {
            ui.checkbox(&mut on, "")
                .changed()
                .then_some(SettingValue::Bool(on))
        }),
        (
            SettingKind::Float {
                min,
                max,
                logarithmic,
                unit,
            },
            SettingValue::Float(mut x),
        ) => {
            let suffix = if unit.is_empty() {
                String::new()
            } else {
                format!(" {unit}")
            };
            prop_row(ui, label, |ui| {
                fill_slider(ui, &mut x, min..=max, logarithmic, &suffix)
                    .changed()
                    .then_some(SettingValue::Float(x))
            })
        }
        (SettingKind::Choice(options), SettingValue::Choice(selected)) => {
            prop_row(ui, label, |ui| {
                let mut chosen = None;
                egui::ComboBox::from_id_salt(id)
                    .width(ui.available_width() - SPACE_1)
                    .selected_text(options.get(selected).copied().unwrap_or("—"))
                    .show_ui(ui, |ui| {
                        for (index, option) in options.iter().enumerate() {
                            if ui.selectable_label(index == selected, *option).clicked() {
                                chosen = Some(SettingValue::Choice(index));
                            }
                        }
                    });
                chosen
            })
        }
        _ => None,
    }
}

/// Status bar; returns true when the user stops an automation lease.
/// `log_open` toggles the Editor's log drawer.
pub fn status_bar(
    ui: &mut Ui,
    view: &StudioView,
    panel: &StudioView,
    log_open: Option<&mut bool>,
) -> bool {
    let mut stop = false;
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = SPACE_1;
        if let Some(log_open) = log_open {
            let warn = panel.log.iter().any(|line| line.tone != Default::default());
            let text = if warn { "Log •" } else { "Log" };
            if tool_button(ui, text, *log_open).clicked() {
                *log_open = !*log_open;
            }
            ui.add_space(SPACE_2);
        }
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

/// Open state of every dock tab ([`super::dock::Tab::ALL`] order) for the
/// Panels menu; the shell applies changes to the workspace's dock tree.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PanelToggles {
    pub open: [bool; super::dock::Tab::ALL.len()],
    pub reset: bool,
}

/// "Panels" menu: show or hide each panel, or restore the default layout.
fn panels_menu(ui: &mut Ui, panels: &mut PanelToggles) {
    ui.menu_button("Panels", |ui| {
        for (index, tab) in super::dock::Tab::ALL.iter().enumerate() {
            if *tab == super::dock::Tab::Viewport {
                continue;
            }
            ui.checkbox(&mut panels.open[index], tab.title());
        }
        ui.separator();
        if ui.button("Reset layout").clicked() {
            panels.reset = true;
            ui.close();
        }
    });
}
