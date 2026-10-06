//! Human-facing summaries consume the same snapshot as machine tooling.
use super::*;

fn row(ui: &mut egui::Ui, label: &str, value: impl Into<egui::RichText>) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.strong(value);
        });
    });
}

pub(super) fn left(ui: &mut egui::Ui, controls: &mut Controls, info: &UiInfo<'_>) {
    ui.heading("Scene");
    #[cfg(feature = "developer-tools")]
    if let Some(owner) = &controls.automation_owner {
        ui.colored_label(egui::Color32::YELLOW, format!("Automation active: {owner}"));
        if ui.button("Stop automation").clicked() {
            controls.automation_stop = true;
        }
    }
    if let Some(s) = info.snapshot {
        row(
            ui,
            "Selected",
            s.general
                .selected_body
                .as_ref()
                .map_or("None", |b| b.name.as_str()),
        );
        row(
            ui,
            "Focused",
            s.general
                .focused_body
                .as_ref()
                .map_or("None", |b| b.name.as_str()),
        );
    }
    ui.horizontal(|ui| {
        if ui
            .button("System overview")
            .on_hover_text("Fit the whole system. Home is the keyboard shortcut.")
            .clicked()
        {
            controls.pending.push_back(Command::Overview);
        }
        if ui.button("Focus selected").clicked() {
            controls.pending.push_back(Command::Focus {
                fixed: false,
                fit: false,
            });
        }
    });
    ui.horizontal(|ui| {
        for (label, offset) in [("Previous body", info.ids.len() - 1), ("Next body", 1)] {
            if ui.button(label).clicked() {
                controls.pending.push_back(Command::Select(
                    info.ids[(info.selected + offset) % info.ids.len()],
                ));
            }
        }
    });
    egui::ComboBox::from_id_salt("scene body")
        .width(260.0)
        .selected_text(
            info.system
                .body(info.ids[info.selected])
                .map_or("None", |b| b.name()),
        )
        .show_ui(ui, |ui| {
            for &id in info.ids {
                if let Ok(body) = info.system.body(id)
                    && ui
                        .selectable_label(id == info.ids[info.selected], body.name())
                        .clicked()
                {
                    controls.pending.push_back(Command::Select(id));
                }
            }
        });
    ui.add_space(8.0);
    ui.separator();
    ui.heading("Camera");
    if let Some(s) = info.snapshot {
        row(ui, "Mode", s.general.camera_mode.label());
        if let Some(body) = &s.camera.reference_body {
            ui.small(format!("Distances relative to {}", body.name));
        }
        row(
            ui,
            "Distance to body",
            format_distance(s.camera.body_distance_m, false),
        );
        row(
            ui,
            "Reference altitude",
            format_distance(s.camera.reference_altitude_m, false),
        );
        row(
            ui,
            "Terrain clearance",
            format_distance(s.camera.terrain_clearance_m, true),
        );
        row(
            ui,
            "Mesh clearance",
            format_distance(s.camera.drawn_mesh_clearance_m, true),
        );
        if s.camera.terrain_clearance_m.is_some_and(|c| c < 0.0)
            || s.camera.drawn_mesh_clearance_m.is_some_and(|c| c < 0.0)
        {
            ui.colored_label(egui::Color32::LIGHT_RED, "Inside terrain or drawn mesh");
        }
    } else {
        ui.label("Current frame diagnostics unavailable");
    }
    ui.horizontal(|ui| {
        if ui.button("Advanced Free Flight").clicked() {
            controls.pending.push_back(Command::FreeFlight);
        }
        if ui
            .add_enabled(
                info.surfaces
                    .iter()
                    .any(|s| s.body() == info.ids[info.selected]),
                egui::Button::new("Surface Navigation"),
            )
            .clicked()
        {
            controls.pending.push_back(Command::SurfaceInspection);
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Body orbit").clicked() {
            controls.pending.push_back(Command::BodyOrbit);
        }
        if ui.button("Frame Selected").clicked() {
            controls.pending.push_back(Command::Focus {
                fixed: false,
                fit: true,
            });
        }
    });
    ui.small("Orbit: drag / scroll. Navigation: tangent WASD, local-up Q/E, wheel forward; right drag looks around.");
    ui.add(
        egui::Slider::new(&mut controls.manual_speed, 1e-3..=1e3)
            .logarithmic(true)
            .text("User speed multiplier (×)"),
    );
    ui.small("Dimensionless multiplier; wheel zoom/forward input is separate.");
    if let Some(navigation) = info.snapshot.and_then(|s| s.camera.navigation.as_ref()) {
        ui.small(format!(
            "Base {:.2} m/s × User {:.2} × Boost {:.0} = {:.2} m/s",
            navigation.base_speed_m_s,
            navigation.user_multiplier,
            navigation.boost_multiplier,
            navigation.effective_speed_m_s
        ));
        ui.small(format!("Source: {}", navigation.base_source));
        ui.collapsing("Navigation diagnostics", |ui| {
            row(
                ui,
                "Base speed",
                format!("{:.3e} m/s", navigation.base_speed_m_s),
            );
            row(ui, "Base source", navigation.base_source.as_str());
            row(
                ui,
                "User multiplier",
                format!("{:.3}×", navigation.user_multiplier),
            );
            row(
                ui,
                "Boost multiplier",
                format!("{:.3}×", navigation.boost_multiplier),
            );
            row(
                ui,
                "Effective speed",
                format!("{:.3e} m/s", navigation.effective_speed_m_s),
            );
            row(ui, "Safeguard", navigation.safeguard.as_str());
            row(
                ui,
                "Minimum radial clearance",
                format_distance(navigation.safeguard_minimum_clearance_m, false),
            );
        });
    }
    if matches!(
        info.camera.mode(),
        CameraMode::BodyOrbit | CameraMode::SurfaceInspection
    ) && !info.camera.transitioning()
    {
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut controls.clearance_target)
                    .speed(10.0)
                    .suffix(" m"),
            );
            if ui
                .button("Approach (debug)")
                .on_hover_text("Debug-only terrain/reference-clearance target; distinct from ordinary navigation.")
                .clicked()
            {
                controls
                    .pending
                    .push_back(Command::Clearance(controls.clearance_target));
            }
        });
        if info.camera.mode() == CameraMode::SurfaceInspection
            && ui.button("Look toward horizon").clicked()
        {
            controls.pending.push_back(Command::SurfaceHorizon);
        }
    }
    ui.add_space(8.0);
    ui.separator();
    ui.heading("Time");
    let motion = info.motion.snapshot(info.system);
    row(ui, "Motion", motion.mode.as_str());
    row(ui, "Requested", format!("{:.6} s", motion.requested_time_s));
    row(ui, "Published", format!("{:.6} s", motion.published_time_s));
    row(ui, "Sampling", motion.sampling_status.as_str());
    if let Some(failure) = &motion.latest_failure {
        ui.colored_label(egui::Color32::LIGHT_RED, failure);
    }
    if let Some(count) = motion.analytic_body_count {
        ui.small(format!(
            "{count} analytic bodies; {:?} solver iterations",
            motion.solver_iterations
        ));
        ui.small(format!(
            "Complete sample/world commit {} · frame publication {}",
            format_milliseconds(motion.sampling_ms),
            format_milliseconds(motion.publication_ms)
        ));
    }
    if let Some(s) = info.snapshot {
        row(
            ui,
            "Simulation",
            if s.general.paused {
                "Paused"
            } else {
                "Running"
            },
        );
        row(ui, "Speed", format!("{}×", s.general.simulation_speed));
        row(
            ui,
            "Time",
            format!("{:.2} days", s.general.simulation_time_s / 86_400.0),
        );
    }
    ui.horizontal(|ui| {
        if ui
            .button(if info.motion.paused() {
                "Resume"
            } else {
                "Pause"
            })
            .clicked()
        {
            controls
                .pending
                .push_back(Command::Pause(!info.motion.paused()));
        }
        if ui.button("Step +").clicked() {
            controls.pending.push_back(Command::Single(true));
        }
        if ui.button("Step −").clicked() {
            controls.pending.push_back(Command::Single(false));
        }
    });
    ui.horizontal_wrapped(|ui| {
        for rate in [-1000.0, -1.0, 1.0, 100.0, 1000.0, 10000.0] {
            if ui.button(format!("{rate}×")).clicked() {
                controls.pending.push_back(Command::Rate(rate));
            }
        }
    });
    ui.horizontal(|ui| {
        ui.add(
            egui::DragValue::new(&mut controls.custom_rate)
                .speed(100.0)
                .prefix("Speed "),
        );
        if ui.button("Set").clicked() {
            controls
                .pending
                .push_back(Command::Rate(controls.custom_rate));
        }
    });
    if info
        .advance
        .is_some_and(|a| a.status == PlaybackStatus::DemandHaltedOverload)
    {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            "Time demand halted; admitted work is draining",
        );
        if ui.button("Resume admission").clicked() {
            controls.pending.push_back(Command::ResumeAdmission);
        }
    }
    if info.motion.is_analytic() {
        ui.small("Steps sample ±60 s; no integration. Signed fractional seeks are direct.");
        ui.add(
            egui::DragValue::new(&mut controls.seek_seconds)
                .speed(1.0)
                .suffix(" seek s"),
        );
        if ui.button("Seek directly (paused)").clicked() {
            controls
                .pending
                .push_back(Command::SeekSeconds(controls.seek_seconds));
        }
        if ui.button("Reset authored epoch").clicked() {
            controls.pending.push_back(Command::Reset);
        }
    }
    ui.add_space(8.0);
    ui.separator();
    ui.heading("Selected body");
    if let Ok(body) = info.system.body(info.ids[info.selected]) {
        row(ui, "Name", body.name());
        row(
            ui,
            "Radius",
            format_distance(Some(body.properties().reference_radius_m()), false),
        );
        row(
            ui,
            "Mass",
            format!("{:.3e} kg", body.properties().mass_kg()),
        );
    }
    ui.collapsing("Scene overlays", |ui| {
        visual_controls::checkbox(
            ui,
            controls,
            visual_controls::Layer::Markers,
            "Navigation markers",
        );
        visual_controls::checkbox(ui, controls, visual_controls::Layer::Labels, "Body labels");
        visual_controls::checkbox(ui, controls, visual_controls::Layer::Guides, "Orbit guides")
            .on_hover_text(if info.motion.is_analytic() {
                "Authored ellipses about their reference's published center; not history."
            } else {
                "Instantaneous two-body guides, not predictions or historical paths."
            });
        visual_controls::checkbox(
            ui,
            controls,
            visual_controls::Layer::Trails,
            "Recorded history",
        )
        .on_hover_text("Actual committed simulation samples.");
    });
}

pub(super) fn right(ui: &mut egui::Ui, controls: &mut Controls, info: &UiInfo<'_>) {
    ui.heading("Planet");
    if let Some(s) = info.snapshot {
        if let Some(body) = &s.terrain.active_body {
            ui.strong(body.name.to_uppercase());
            if s.general.focused_body.as_ref() != Some(body) {
                ui.small("Active terrain body differs from camera focus");
            }
        } else {
            ui.strong("No active terrain body");
        }
        row(ui, "Terrain", s.terrain.status());
        row(
            ui,
            "Displayed LOD",
            s.terrain
                .source_radial_lod
                .map_or("—".into(), |l| l.to_string()),
        );
        ui.small("Camera-radial, not whole-view quality.")
            .on_hover_text("The level currently contributing to the rendered source surface.");
        row(
            ui,
            "Target LOD",
            s.terrain
                .desired_radial_lod
                .map_or("—".into(), |l| l.to_string()),
        );
        if s.terrain.quality_pending == Some(true) {
            ui.small("Quality pending — not settled visual quality");
        }
    }
    ui.add_space(8.0);
    ui.separator();
    ui.heading("Rendering");
    ui.strong("Planetary layers");
    let mut natural = controls.terrain_lighting.mode() == TerrainRenderMode::Natural;
    if ui
        .checkbox(&mut natural, "Natural terrain")
        .on_hover_text(
            "Switch presentation between Natural and Lit, without changing terrain geometry.",
        )
        .changed()
    {
        controls
            .pending
            .push_back(Command::Visual(visual_controls::VisualCommand::RenderMode(
                if natural {
                    TerrainRenderMode::Natural
                } else {
                    TerrainRenderMode::Lit
                },
            )));
    }
    visual_controls::checkbox(ui, controls, visual_controls::Layer::Ocean, "Ocean");
    visual_controls::checkbox(ui, controls, visual_controls::Layer::Clouds, "Clouds");
    visual_controls::checkbox(
        ui,
        controls,
        visual_controls::Layer::Atmosphere,
        "Atmosphere",
    );
    ui.small("Layers render only where defined, in Natural mode.");
    if let Some(s) = info.snapshot {
        ui.small(format!(
            "This frame: ocean {} · clouds {} · atmosphere {}",
            on_off(s.rendering.ocean_drawn),
            on_off(s.rendering.clouds_drawn),
            on_off(s.rendering.atmosphere_drawn)
        ));
    }
    ui.collapsing("Diagnostic rendering", |ui| {
        visual_controls::checkbox(
            ui,
            controls,
            visual_controls::Layer::Borders,
            "Patch borders",
        )
        .on_hover_text("Draw chart patch boundaries, not additional geometry.");
        visual_controls::checkbox(
            ui,
            controls,
            visual_controls::Layer::LodColors,
            "LOD colors",
        )
        .on_hover_text("Cyclic diagnostic palette, not a whole-screen quality measure.");
        ui.checkbox(&mut controls.surface_style.face_colors, "Face colors");
        egui::ComboBox::from_label("Shading")
            .selected_text(render_mode_name(controls.terrain_lighting.mode()))
            .show_ui(ui, |ui| {
                for mode in TerrainRenderMode::ALL {
                    if ui
                        .selectable_label(
                            controls.terrain_lighting.mode() == mode,
                            render_mode_name(mode),
                        )
                        .clicked()
                    {
                        controls.pending.push_back(Command::Visual(
                            visual_controls::VisualCommand::RenderMode(mode),
                        ));
                    }
                }
            });
    });
    let mut sky_settings = controls.sky;
    ui.collapsing("Distant sky", |ui| {
        ui.small("Fictional decorative content · not travel destinations");
        ui.checkbox(&mut sky_settings.enabled, "Enabled");
        ui.add(egui::Slider::new(&mut sky_settings.intensity, 0.0..=3.0).text("Intensity"));
        ui.add(
            egui::Slider::new(
                &mut sky_settings.galactic_yaw_rad,
                -std::f64::consts::PI..=std::f64::consts::PI,
            )
            .text("Galactic yaw"),
        );
        ui.add(
            egui::Slider::new(
                &mut sky_settings.galactic_roll_rad,
                -std::f64::consts::PI..=std::f64::consts::PI,
            )
            .text("Galactic roll"),
        );
        ui.add(egui::Slider::new(&mut sky_settings.star_intensity, 0.0..=3.0).text("Stars"));
        ui.add(
            egui::Slider::new(&mut sky_settings.background_intensity, 0.0..=3.0)
                .text("Galactic band"),
        );
        ui.add(
            egui::Slider::new(&mut sky_settings.halo_strength, 0.0..=0.5).text("Bright-star halos"),
        );
        if let Some(sky) = info.snapshot.and_then(|s| s.sky.as_ref()) {
            ui.small(format!(
                "{} v{} · {} finite stars",
                sky.preset, sky.version, sky.star_count
            ));
            if sky.outside_envelope {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Sky unavailable outside supported observer envelope",
                );
            }
        }
    });
    if sky_settings != controls.sky {
        controls
            .pending
            .push_back(Command::Visual(visual_controls::VisualCommand::Sky(
                sky_settings,
            )));
    }
    ui.add_space(8.0);
    ui.separator();
    ui.heading("Terrain");
    let mut enabled = controls.terrain_preview;
    if ui.checkbox(&mut enabled, "Procedural terrain").on_hover_text("Use existing observer-local terrain admission; disabling requests the smooth/far path.").changed() { controls.pending.push_back(Command::TerrainPreview(enabled)); }
    if let Some(s) = info.snapshot {
        row(ui, "Source leaves", s.terrain.source_leaf_count.to_string());
        row(
            ui,
            "Visible regular leaves",
            s.terrain.visible_leaf_count.to_string(),
        );
    }
    ui.add_space(8.0);
    ui.separator();
    ui.heading("Performance");
    if let Some(s) = info.snapshot {
        row(
            ui,
            "Frame CPU",
            format_milliseconds(s.performance.frame_cpu_ms),
        );
        ui.small("Update + preparation; excludes UI and presentation.");
        row(
            ui,
            "Terrain update",
            format_milliseconds(s.performance.terrain_update_ms),
        );
        row(
            ui,
            "Terrain prepare",
            format_milliseconds(s.performance.terrain_preparation_ms),
        );
        row(
            ui,
            "GPU terrain",
            format_milliseconds(s.performance.gpu_terrain_ms),
        );
        ui.small("Latest completed regular-terrain scope, may be older.");
        row(
            ui,
            "Terrain memory",
            format!(
                "{} / {}",
                format_bytes(s.memory.used_bytes),
                format_bytes(s.memory.cap_bytes)
            ),
        );
        if s.memory.near_cap() {
            ui.colored_label(egui::Color32::YELLOW, "NEAR CAP (>95% accounted budget)");
        }
        ui.small("Accounted CPU bytes + reservations, not RSS or VRAM.");
        ui.collapsing("Snapshot / warnings", |ui| {
            for warning in &s.warnings {
                ui.monospace(warning);
            }
            ui.small(format!(
                "Schema {} · frame {}",
                s.schema_version, s.general.frame_number
            ));
            if ui.button("Export current frame JSON").clicked() {
                let directory = std::path::Path::new("target/mundaris-diagnostics");
                let result = std::fs::create_dir_all(directory)
                    .map_err(anyhow::Error::new)
                    .and_then(|()| s.write_json(&directory.join("frame.json")));
                controls.snapshot_export_status = Some(match result {
                    Ok(()) => "Wrote target/mundaris-diagnostics/frame.json".into(),
                    Err(error) => format!("Export failed: {error}"),
                });
            }
            if let Some(status) = &controls.snapshot_export_status {
                ui.small(status);
            }
        });
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "On" } else { "Off" }
}
