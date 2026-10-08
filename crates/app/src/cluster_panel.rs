//! Compact, presentation-only controls and counters for the cluster renderer.
use egui::{Color32, RichText};
use mundaris_renderer::{ClusterDebug, ClusterMode, ClusterReport, ClusterSettings};

const SELECTED: Color32 = Color32::from_rgb(71, 203, 221);
const FALLBACK: Color32 = Color32::from_rgb(238, 174, 83);
const PENDING: Color32 = Color32::from_rgb(166, 131, 255);
const UNAVAILABLE: Color32 = Color32::from_rgb(232, 104, 111);

/// Draws the cluster prototype's derived presentation controls and latest report.
/// It changes only `settings`; it never reaches terrain authority or resource policy.
pub fn show(ui: &mut egui::Ui, settings: &mut ClusterSettings, report: &ClusterReport) {
    ui.group(|ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.horizontal_wrapped(|ui| {
            ui.heading("Cluster view");
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    let (label, color) = if report.enabled {
                        ("ACTIVE", SELECTED)
                    } else {
                        ("REFERENCE", FALLBACK)
                    };
                    ui.label(RichText::new(label).strong().color(color));
                },
            );
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("Backend");
            egui::ComboBox::from_id_salt("cluster-render-mode")
                .selected_text(mode_label(&settings.mode))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.mode, ClusterMode::Reference, "Reference")
                        .on_hover_text("Use the existing resident terrain renderer.");
                    ui.selectable_value(&mut settings.mode, ClusterMode::Culling, "Culling")
                        .on_hover_text("Use cluster bounds and visibility selection without detail replacement.");
                    ui.selectable_value(&mut settings.mode, ClusterMode::Lod, "Cluster LOD")
                        .on_hover_text("Select a legal cluster cut by projected detail.");
                });
            ui.small(format!("Active: {}", mode_label(&report.active_mode)));
        });

        ui.horizontal_wrapped(|ui| {
            ui.label("Debug view");
            egui::ComboBox::from_id_salt("cluster-debug-view")
                .selected_text(debug_label(&settings.debug))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut settings.debug, ClusterDebug::Lit, "Lit");
                    ui.selectable_value(&mut settings.debug, ClusterDebug::Clusters, "Clusters");
                    ui.selectable_value(&mut settings.debug, ClusterDebug::Lod, "LOD");
                    ui.selectable_value(&mut settings.debug, ClusterDebug::Residency, "Residency");
                });
            ui.add_enabled(report.resident_regions>0 && settings.mode!=ClusterMode::Reference,egui::Checkbox::new(&mut settings.freeze, "Freeze"));
            if report.freeze_active {
                ui.colored_label(SELECTED, RichText::new("FROZEN").strong());
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut settings.cluster_edges, "Cluster boundaries")
                .on_hover_text("Outline cluster boundaries while retaining normal triangle shading.");
            ui.checkbox(&mut settings.triangle_edges, "Triangle wireframe")
                .on_hover_text("Show triangle edges; this is distinct from cluster boundaries.");
        });

        legend(ui, &settings.debug);

        egui::Grid::new("cluster-report-counters")
            .num_columns(2)
            .min_col_width(120.0)
            .max_col_width(130.0)
            .spacing([14.0, 2.0])
            .show(ui, |ui| {
                value_row(ui, "Predicted selected", format_count(report.selected_clusters));
                value_row(ui, "Fine / coarse / transition", format!("{} / {} / {}",report.selected_fine_clusters,report.selected_coarse_clusters,report.selected_transition_clusters));
                value_row(ui, "Resident clusters", format_count(report.resident_clusters));
                value_row(ui, "Resident regions", format_count(report.resident_regions));
                value_row(ui, "Pending regions", format_count(report.pending_regions));
                value_row(ui, "Fallback regions", format_count(report.fallback_regions));
                value_row(ui, "Submitted triangles", format_count(report.submitted_triangles));
                value_row(ui, "Draw commands", format_count(report.draw_commands));
                value_row(ui, "Geometry CPU", format_bytes(report.cpu_bytes as u64));
                value_row(ui, "Geometry GPU", format_bytes(report.gpu_bytes));
                value_row(ui, "Upload", format_bytes(report.upload_bytes));
            });
        ui.small(report.counters_scope);

        ui.horizontal_wrapped(|ui| {
            ui.small(format!("Build {}", format_micros(report.build_micros)));
            ui.small("·");
            ui.small(format!(
                "Selection CPU {}",
                format_micros(report.selection_cpu_micros)
            ));
            ui.small("·");
            ui.small(format!("GPU select {}", format_optional_ms(report.gpu_selection_ms)));
            ui.small("·");
            ui.small(format!("GPU render {}", format_optional_ms(report.gpu_render_ms)));
        });

        ui.horizontal_wrapped(|ui| {
            ui.small(format!("Cache hits {}", format_count(report.cache_hits)));
            ui.small("·");
            ui.small(format!("Rebuilds {}", format_count(report.rebuilds)));
            ui.small("·");
            ui.small(format!("Cancelled {}", format_count(report.cancelled)));
        });

        if settings.freeze && !report.freeze_active {
            ui.colored_label(
                FALLBACK,
                "Freeze requested; retained selection is not active yet.",
            );
        }
        if let Some(reason) = report.reason.as_deref().filter(|reason| !reason.is_empty()) {
            ui.colored_label(UNAVAILABLE, format!("Status: {}", concise(reason, 140)));
        }
    });
}

fn mode_label(mode: &ClusterMode) -> &'static str {
    match mode {
        ClusterMode::Reference => "Reference",
        ClusterMode::Culling => "Culling",
        ClusterMode::Lod => "Cluster LOD",
    }
}

fn debug_label(debug: &ClusterDebug) -> &'static str {
    match debug {
        ClusterDebug::Lit => "Lit",
        ClusterDebug::Clusters => "Clusters",
        ClusterDebug::Lod => "LOD",
        ClusterDebug::Residency => "Residency",
    }
}

fn legend(ui: &mut egui::Ui, debug: &ClusterDebug) {
    let (title, items): (&str, &[(&str, Color32)]) = match debug {
        ClusterDebug::Lit => ("Terrain shading", &[]),
        ClusterDebug::Clusters => ("Stable cluster ID hash colors", &[]),
        ClusterDebug::Lod => ("Drawn detail", &[("Coarse", FALLBACK), ("Fine", SELECTED)]),
        ClusterDebug::Residency => (
            "Residency",
            &[
                ("Selected detail", SELECTED),
                ("Drawn reference fallback", FALLBACK),
                ("Reference while pending", PENDING),
                ("Unavailable cluster / reference retained", UNAVAILABLE),
            ],
        ),
    };
    ui.vertical(|ui| {
        ui.small(RichText::new(title).strong());
        if matches!(debug, ClusterDebug::Clusters) {
            ui.small("same ID keeps its color; edges show actual cluster boundaries");
        } else if items.is_empty() {
            ui.small("debug colors off");
        } else {
            for (label, color) in items {
                ui.horizontal_wrapped(|ui| {
                    ui.small(RichText::new("●").color(*color));
                    ui.small(*label);
                });
            }
        }
    });
}

fn value_row(ui: &mut egui::Ui, label: &str, value: String) {
    ui.small(label);
    ui.small(RichText::new(value).strong());
    ui.end_row();
}

fn format_count(value: impl std::fmt::Display) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

fn format_bytes(value: u64) -> String {
    if value >= 1024 * 1024 {
        format!("{:.1} MiB", value as f64 / (1024.0 * 1024.0))
    } else if value >= 1024 {
        format!("{:.1} KiB", value as f64 / 1024.0)
    } else {
        format!("{value} B")
    }
}

fn format_micros(value: impl std::fmt::Display) -> String {
    format!("{} µs", format_count(value))
}

fn format_optional_ms(value: Option<f64>) -> String {
    value.map_or_else(
        || "unavailable".to_owned(),
        |ms| {
            if ms.is_finite() {
                format!("{ms:.2} ms")
            } else {
                "unavailable".to_owned()
            }
        },
    )
}

fn concise(value: &str, max_chars: usize) -> String {
    let mut result: String = value.chars().take(max_chars).collect();
    if value.chars().count() > max_chars {
        result.push('…');
    }
    result
}
