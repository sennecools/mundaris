//! Species browser panels (docs/STUDIO_UI.md amendment 2026-10-10e): the
//! species list, the genome sliders (generic descriptor panel) and the
//! line-up of grown variants with their metrics.

use astrum_app::studio::species::{
    LineUp, MAX_VARIANTS, SpeciesAction, SpeciesView, THUMB_W, Variant,
};
use astrum_app::studio::view::{ParamTarget, StudioAction, Tone};
use egui::{Align, Layout, RichText, TextureHandle, Ui};

use super::panels::{self, tool_button_enabled};
use super::theme::{self, *};

/// Line-up thumbnails uploaded to egui, re-made when the line-up changes.
#[derive(Default)]
pub struct Thumbnails {
    generation: u64,
    textures: Vec<TextureHandle>,
}

impl Thumbnails {
    fn sync(&mut self, ctx: &egui::Context, lineup: &LineUp) {
        if self.generation == lineup.generation && self.textures.len() == lineup.variants.len() {
            return;
        }
        self.generation = lineup.generation;
        self.textures = lineup
            .variants
            .iter()
            .map(|variant| {
                let image = egui::ColorImage::from_rgb(
                    [variant.thumbnail.width, variant.thumbnail.height],
                    &variant.thumbnail.rgb,
                );
                ctx.load_texture(
                    format!("species-variant-{}", variant.index),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            })
            .collect();
    }
}

/// Species list with save and revert of the selected species.
pub fn species_list(ui: &mut Ui, view: &SpeciesView, actions: &mut Vec<StudioAction>) {
    panels::section_header(ui, "SPECIES", &format!("{} files", view.species.len()));
    ui.add_space(SPACE_1);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if tool_button_enabled(ui, "Save", view.dirty, view.dirty).clicked() {
            actions.push(StudioAction::Species(SpeciesAction::Save));
        }
        if tool_button_enabled(ui, "Revert", false, view.dirty).clicked() {
            actions.push(StudioAction::Species(SpeciesAction::Revert));
        }
    });
    ui.add_space(SPACE_1);
    egui::ScrollArea::vertical()
        .id_salt("species-list")
        .auto_shrink([false, false])
        .max_height((ui.available_height() - 44.0).max(60.0))
        .show(ui, |ui| {
            for (index, (name, dirty)) in view.species.iter().enumerate() {
                let text = if *dirty {
                    format!("{name}  •")
                } else {
                    name.clone()
                };
                let selected = index == view.selected;
                let label = RichText::new(text).color(if selected {
                    TEXT_PRIMARY
                } else {
                    TEXT_SECONDARY
                });
                if ui
                    .add_sized(
                        [ui.available_width(), 26.0],
                        egui::Button::selectable(selected, label),
                    )
                    .clicked()
                {
                    actions.push(StudioAction::Species(SpeciesAction::Select(index)));
                }
            }
        });
    // Fixed footer: file and the last problem, if any.
    ui.add(
        egui::Label::new(RichText::new(&view.file).font(small()).color(TEXT_DISABLED)).truncate(),
    );
    let (text, color) = match &view.error {
        Some(error) => (error.as_str(), ERROR),
        None => ("", TEXT_DISABLED),
    };
    ui.add(egui::Label::new(RichText::new(text).font(small()).color(color)).truncate());
}

/// Genome (and later niche) sliders through the generic descriptor panel.
pub fn genome(ui: &mut Ui, view: &SpeciesView, actions: &mut Vec<StudioAction>) {
    if view.params.is_empty() {
        ui.label(RichText::new("No species loaded.").color(TEXT_SECONDARY));
        return;
    }
    let first = view.params[0].group;
    egui::ScrollArea::vertical()
        .id_salt("species-genome")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut edits = Vec::new();
            panels::param_sections(ui, &view.params, |group| group == first, &mut edits);
            actions.extend(
                edits
                    .into_iter()
                    .map(|edit| StudioAction::EditParam(ParamTarget::Species, edit)),
            );
        });
}

/// Line-up of grown variants: thumbnails with captions, then a metrics table.
pub fn line_up(
    ui: &mut Ui,
    view: &SpeciesView,
    thumbnails: &mut Thumbnails,
    actions: &mut Vec<StudioAction>,
) {
    let name = view
        .species
        .get(view.selected)
        .map_or("", |(name, _)| name.as_str());
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        ui.label(RichText::new(name).strong().color(TEXT_PRIMARY));
        ui.add_space(SPACE_2);
        ui.label(RichText::new("Variants").color(TEXT_SECONDARY));
        ui.spacing_mut().item_spacing.x = 2.0;
        if tool_button_enabled(ui, "−", false, view.variants > 1).clicked() {
            actions.push(StudioAction::Species(SpeciesAction::Variants(
                view.variants - 1,
            )));
        }
        ui.label(
            RichText::new(view.variants.to_string())
                .font(mono())
                .color(TEXT_PRIMARY),
        );
        if tool_button_enabled(ui, "+", false, view.variants < MAX_VARIANTS).clicked() {
            actions.push(StudioAction::Species(SpeciesAction::Variants(
                view.variants + 1,
            )));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if view.growing {
                ui.label(RichText::new("Growing…").font(small()).color(WARN));
                ui.add(egui::Spinner::new().size(12.0).color(WARN));
            } else if let Some(lineup) = &view.lineup {
                ui.label(
                    RichText::new(format!(
                        "{} variants · {:.0} ms",
                        lineup.variants.len(),
                        lineup.total_ms
                    ))
                    .font(small())
                    .color(TEXT_DISABLED),
                );
            }
        });
    });
    ui.separator();
    let Some(lineup) = &view.lineup else {
        return;
    };
    thumbnails.sync(ui.ctx(), lineup);
    egui::ScrollArea::vertical()
        .id_salt("species-lineup")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Rows of whole thumbnails, as many columns as fit.
            let cell = THUMB_W as f32 + SPACE_2;
            let columns = (((ui.available_width() + SPACE_2) / cell).floor() as usize).max(1);
            let cells: Vec<_> = lineup.variants.iter().zip(&thumbnails.textures).collect();
            for row in cells.chunks(columns) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = SPACE_2;
                    for (variant, texture) in row {
                        thumbnail(ui, variant, texture);
                    }
                });
                ui.add_space(SPACE_2);
            }
            ui.add_space(SPACE_2);
            metrics_table(ui, lineup);
        });
}

/// One line-up cell: the thumbnail, a caption, and the red flags (one line).
fn thumbnail(ui: &mut Ui, variant: &Variant, texture: &TextureHandle) {
    let size = egui::vec2(
        variant.thumbnail.width as f32,
        variant.thumbnail.height as f32,
    );
    ui.vertical(|ui| {
        ui.set_width(size.x);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.image((texture.id(), size));
        ui.label(
            RichText::new(format!(
                "#{}  {} tris  {:.0} ms",
                variant.index, variant.metrics.triangles[0], variant.grow_ms
            ))
            .font(small())
            .color(TEXT_SECONDARY),
        );
        // Always one line, so captions keep their height.
        let (text, color) = if variant.failures.is_empty() {
            ("ok".to_string(), OK)
        } else {
            (variant.failures.join(", "), ERROR)
        };
        ui.add(egui::Label::new(RichText::new(text).font(small()).color(color)).truncate())
            .on_hover_text(variant.failures.join("\n"));
    });
}

/// Line-up summary: ranges over the variants.
fn metrics_table(ui: &mut Ui, lineup: &LineUp) {
    let variants = &lineup.variants;
    if variants.is_empty() {
        return;
    }
    let range = |f: &dyn Fn(&astrum_app::studio::species::Variant) -> f64, digits: usize| {
        let lo = variants.iter().map(f).fold(f64::INFINITY, f64::min);
        let hi = variants.iter().map(f).fold(f64::NEG_INFINITY, f64::max);
        if (hi - lo).abs() < 10f64.powi(-(digits as i32)) {
            format!("{lo:.digits$}")
        } else {
            format!("{lo:.digits$} – {hi:.digits$}")
        }
    };
    let failing = variants.iter().filter(|v| !v.failures.is_empty()).count();
    let rows = [
        (
            "Triangles L0 / L1 / L2",
            format!(
                "{} / {} / {}",
                range(&|v| v.metrics.triangles[0] as f64, 0),
                range(&|v| v.metrics.triangles[1] as f64, 0),
                range(&|v| v.metrics.triangles[2] as f64, 0)
            ),
            Tone::Normal,
        ),
        (
            "Grow",
            format!("{} ms", range(&|v| v.grow_ms, 0)),
            Tone::Normal,
        ),
        (
            "Height",
            format!("{} m", range(&|v| v.metrics.structure.height_m, 1)),
            Tone::Normal,
        ),
        (
            "Fractal dimension",
            range(&|v| v.metrics.silhouette.fractal_dimension, 2),
            Tone::Normal,
        ),
        (
            "Crown fill",
            range(&|v| v.metrics.silhouette.crown_fill, 2),
            Tone::Normal,
        ),
        (
            "Failing variants",
            format!("{failing} of {}", variants.len()),
            if failing > 0 { Tone::Error } else { Tone::Ok },
        ),
    ];
    panels::section_header(ui, "METRICS", "");
    ui.label(
        RichText::new("Ranges over the line-up; red flags use the hero bands.")
            .font(small())
            .color(TEXT_DISABLED),
    );
    egui::Grid::new("species-metrics")
        .num_columns(2)
        .spacing(egui::vec2(SPACE_3 * 2.0, SPACE_1))
        .show(ui, |ui| {
            for (label, value, tone) in rows {
                ui.label(RichText::new(label).color(TEXT_SECONDARY));
                ui.label(
                    RichText::new(value)
                        .font(mono())
                        .color(theme::tone_color(tone)),
                );
                ui.end_row();
            }
        });
}
