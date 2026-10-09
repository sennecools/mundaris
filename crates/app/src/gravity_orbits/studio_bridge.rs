//! Studio view publication and UI actions over the existing command path.
use super::*;
use crate::studio::view::{
    ALTITUDE_PRESETS_M, BodyItem, LabelItem, LabelState, LogItem, Overlay, RATE_PRESETS, StatItem,
    StudioAction, StudioView, Tone, rate_preset_index,
};

const LOG_CAPACITY: usize = 200;
/// Label text is measured with this approximation (12 px UI font) so layout
/// stays toolkit-independent; labels size to their text in the UI.
const LABEL_CHAR_WIDTH: f64 = 7.0;
const LABEL_HEIGHT: f64 = 20.0;
const LABEL_PADDING: f64 = 12.0;

impl GravityOrbitsDemo {
    /// View model of the latest frame for the Studio UI.
    pub fn studio_view(&self) -> &StudioView {
        &self.view
    }

    /// Host report of how long the UI toolkit took to render the last frame.
    pub fn set_ui_render_ms(&mut self, ms: f64) {
        self.ui_render_ms = ms.is_finite().then_some(ms);
    }

    /// Whether the bad-frame capture writer is running.
    pub fn bad_frame_capture_active(&self) -> bool {
        self.performance_capture.is_active()
    }

    pub fn profiler(&mut self) -> &mut crate::profiler::Profiler {
        &mut self.controls.profiler
    }

    /// Applies one UI interaction through the ordinary command queue.
    pub fn studio_action(&mut self, action: StudioAction) {
        let selected = self.ids[self.selected_index()];
        let push = |controls: &mut Controls, command| controls.pending.push_back(command);
        match action {
            StudioAction::TogglePause => {
                let paused = self.motion.paused();
                push(&mut self.controls, Command::Pause(!paused));
            }
            StudioAction::SetRate(index) => {
                if let Some(rate) = RATE_PRESETS.get(index) {
                    push(&mut self.controls, Command::Rate(*rate));
                }
            }
            StudioAction::SetCamera(index) => {
                let command = match index {
                    0 => Command::Overview,
                    1 => Command::Focus {
                        fixed: false,
                        fit: true,
                    },
                    2 => Command::SurfaceInspection,
                    _ => Command::FreeFlight,
                };
                push(&mut self.controls, command);
            }
            StudioAction::SetSetting(index, value) => {
                if let Err(error) = self.apply_render_setting(index, value) {
                    self.log(Tone::Warn, format!("Setting rejected: {error:#}"));
                }
            }
            StudioAction::ResetRenderSettings => {
                self.controls.render_settings = Default::default();
                self.controls.terrain_view = TerrainViewMode::Lit;
                self.log(Tone::Normal, "Render settings reset to defaults".into());
            }
            StudioAction::SetView(index) => {
                if let Some(mode) = TerrainViewMode::ALL.get(index) {
                    push(
                        &mut self.controls,
                        Command::Visual(visual_controls::VisualCommand::RenderMode(*mode)),
                    );
                }
            }
            StudioAction::ToggleTerrain => {
                let enabled = !self.controls.terrain_preview;
                push(
                    &mut self.controls,
                    Command::Visual(visual_controls::VisualCommand::Layer(
                        visual_controls::Layer::Terrain,
                        enabled,
                    )),
                );
            }
            StudioAction::ToggleLabels => {
                let enabled = !self.controls.labels;
                push(
                    &mut self.controls,
                    Command::Visual(visual_controls::VisualCommand::Layer(
                        visual_controls::Layer::Labels,
                        enabled,
                    )),
                );
            }
            StudioAction::ToggleOverlay(overlay) => {
                let (layer, enabled) = match overlay {
                    Overlay::Markers => (visual_controls::Layer::Markers, !self.controls.markers),
                    Overlay::Trails => (visual_controls::Layer::Trails, !self.controls.trails),
                    Overlay::Guides => {
                        (visual_controls::Layer::Guides, !self.controls.guide_visible)
                    }
                };
                push(
                    &mut self.controls,
                    Command::Visual(visual_controls::VisualCommand::Layer(layer, enabled)),
                );
            }
            StudioAction::SelectBody(index) => {
                if let Some(&body) = self.ids.get(index) {
                    push(&mut self.controls, Command::Select(body));
                }
            }
            StudioAction::FocusBody(index) => {
                if let Some(&body) = self.ids.get(index) {
                    push(&mut self.controls, Command::Select(body));
                    push(
                        &mut self.controls,
                        Command::Focus {
                            fixed: false,
                            fit: true,
                        },
                    );
                }
            }
            StudioAction::SetSpeedExponent(exponent) => {
                if exponent.is_finite() {
                    self.controls.manual_speed = 10f64.powf(f64::from(exponent.clamp(-2.0, 2.0)));
                }
            }
            StudioAction::Approach(index) => {
                if let Some(&clearance) = ALTITUDE_PRESETS_M.get(index) {
                    if self.camera.mode() == CameraMode::SystemOrbit
                        || self.camera.focused_body() != Some(selected)
                    {
                        push(
                            &mut self.controls,
                            Command::Focus {
                                fixed: false,
                                fit: false,
                            },
                        );
                    }
                    push(&mut self.controls, Command::Clearance(clearance));
                }
            }
            StudioAction::SurfaceNavigation => {
                push(&mut self.controls, Command::SurfaceInspection);
            }
            StudioAction::FrameSelected => push(
                &mut self.controls,
                Command::Focus {
                    fixed: false,
                    fit: true,
                },
            ),
            StudioAction::ProfilerEnabled(enabled) => {
                let _ = self.controls.profiler.configure(
                    None,
                    crate::profiler::ProfilerControls {
                        enabled: Some(enabled),
                        ..Default::default()
                    },
                );
            }
            StudioAction::ProfilerFrozen(frozen) => {
                #[cfg(feature = "developer-tools")]
                let snapshot = self.developer_snapshot.as_ref();
                #[cfg(not(feature = "developer-tools"))]
                let snapshot = None;
                let _ = self.controls.profiler.configure(
                    snapshot,
                    crate::profiler::ProfilerControls {
                        freeze: Some(frozen),
                        ..Default::default()
                    },
                );
            }
            StudioAction::ProfilerExport => {
                #[cfg(feature = "developer-tools")]
                let snapshot = self.developer_snapshot.as_ref();
                #[cfg(not(feature = "developer-tools"))]
                let snapshot = None;
                if let Err(error) = self.controls.profiler.configure(
                    snapshot,
                    crate::profiler::ProfilerControls {
                        export: true,
                        ..Default::default()
                    },
                ) {
                    self.log(Tone::Warn, format!("Profile export unavailable: {error}"));
                }
            }
            StudioAction::CaptureBadFrames => {
                if self.performance_capture.is_active() {
                    self.controls.profiler.capture_stop_requested = true;
                    self.log(Tone::Normal, "Bad-frame capture stopped".into());
                } else {
                    self.controls.profiler.capture_requested = true;
                    self.log(Tone::Normal, "Bad-frame capture armed".into());
                }
            }
            StudioAction::Capture => {
                #[cfg(feature = "developer-tools")]
                if self.developer_session.is_some() {
                    self.diagnostic_capture_request =
                        Some(format!("studio-{}", self.developer_frame_number));
                    self.log(Tone::Normal, "Capture requested".into());
                    return;
                }
                self.log(
                    Tone::Warn,
                    "Captures need the developer interface (--dev-interface)".into(),
                );
            }
            StudioAction::PlanetSeed(_)
            | StudioAction::PlanetRandomSeed
            | StudioAction::PlanetParam(..)
            | StudioAction::PlanetResetParam(_)
            | StudioAction::PlanetRevert
            | StudioAction::PlanetSave => self.planet_action(action),
        }
    }

    pub(super) fn log(&mut self, tone: Tone, text: String) {
        if self.controls.log.len() == LOG_CAPACITY {
            self.controls.log.pop_front();
        }
        let seconds = self.system.sample_time().seconds_since_epoch();
        self.controls.log.push_back(LogItem {
            time: format!("{:>8.1} s", seconds),
            text,
            tone,
        });
    }

    /// Builds the view model for the frame just submitted. `markers` are that
    /// frame's body markers in physical viewport pixels.
    pub(super) fn publish_view(
        &mut self,
        snapshot: Option<&DeveloperSnapshot>,
        markers: &[CelestialMarker],
    ) {
        self.last_markers.clear();
        self.last_markers.extend_from_slice(markers);
        if self.diagnostic != self.controls.logged_diagnostic {
            self.controls.logged_diagnostic = self.diagnostic.clone();
            if let Some(message) = self.diagnostic.clone() {
                self.log(Tone::Warn, message);
            }
        }
        let scale = self.controls.pixels_per_point.max(0.1);
        let selected_index = self.selected_index();
        let selected = self.ids[selected_index];
        let focused = self.camera.focused_body();
        let labels = self.layout_labels(markers, selected_index, focused, scale);

        let bodies = self
            .ids
            .iter()
            .enumerate()
            .filter_map(|(index, &id)| {
                let body = self.system.body(id).ok()?;
                Some(BodyItem {
                    name: body.name().into(),
                    detail: compact_distance(body.properties().reference_radius_m()),
                    depth: u32::from(index > 0),
                    selected: index == selected_index,
                    focused: focused == Some(id),
                })
            })
            .collect();

        let body = self.system.body(selected).ok();
        let marker = markers
            .iter()
            .find(|marker| marker.request_index == selected_index);
        let radius = body.map_or(0.0, |body| body.properties().reference_radius_m());
        let mut body_stats = vec![
            StatItem::new("Radius", compact_distance(radius)),
            StatItem::new(
                "Mass",
                body.map_or("—".into(), |body| {
                    format!("{:.3e} kg", body.properties().mass_kg())
                }),
            ),
        ];
        if let Some(marker) = marker {
            body_stats.push(StatItem::new(
                "Distance",
                compact_distance(marker.distance_m),
            ));
            body_stats.push(StatItem::new(
                "Altitude",
                compact_distance(marker.distance_m - radius),
            ));
        }
        let surface_available = body.is_some_and(|body| body.has_surface());
        body_stats.push(StatItem::new(
            "Surface",
            if surface_available {
                "Atlas terrain"
            } else {
                "Sphere only"
            },
        ));

        let mut camera_stats = vec![StatItem::new(
            "Mode",
            crate::studio::view::CAMERA_MODES[camera_index(self.camera.mode())],
        )];
        if let Some(clearance) = self.terrain_clearance {
            camera_stats.push(
                StatItem::new("Terrain clearance", compact_distance(clearance.clearance_m)).tone(
                    if clearance.clearance_m < 0.0 {
                        Tone::Error
                    } else {
                        Tone::Normal
                    },
                ),
            );
        }
        if let Some(snapshot) = snapshot {
            camera_stats.push(StatItem::new(
                "Near plane",
                compact_distance(snapshot.camera.near_plane_m),
            ));
        }
        camera_stats.push(StatItem::new(
            "Fly speed",
            format!("{:.2}×", self.controls.manual_speed),
        ));

        let mut terrain_stats = Vec::new();
        let mut hud = Vec::new();
        if let Some(snapshot) = snapshot {
            let terrain = &snapshot.terrain;
            terrain_stats.push(StatItem::new("Status", terrain.status()).tone(
                match terrain.status() {
                    "Settled" => Tone::Ok,
                    "Not active" => Tone::Normal,
                    _ => Tone::Warn,
                },
            ));
            if let Some(level) = terrain.desired_radial_lod {
                terrain_stats.push(StatItem::new("Finest level", level.to_string()));
                hud.push(StatItem::new("LOD", level.to_string()));
            }
            terrain_stats.push(StatItem::new(
                "Drawn nodes",
                terrain.visible_leaf_count.to_string(),
            ));
            terrain_stats.push(StatItem::new(
                "Resident tiles",
                terrain.source_leaf_count.to_string(),
            ));
            if let Some(jobs) = snapshot
                .terrain_atlas
                .as_ref()
                .and_then(|atlas| atlas["jobs_total"].as_u64())
            {
                terrain_stats.push(StatItem::new("Tiles produced", jobs.to_string()));
            }
            terrain_stats.push(StatItem::new("Backend", terrain.backend.clone()));
            let performance = &snapshot.performance;
            if let Some(host) = performance.host_frame_ms {
                hud.insert(0, StatItem::new("Frame", format!("{host:.2} ms")));
            }
            if let Some(gpu) = performance.gpu_frame_ms {
                hud.insert(
                    1.min(hud.len()),
                    StatItem::new("GPU", format!("{gpu:.2} ms")),
                );
            }
            hud.push(StatItem::new(
                "Nodes",
                terrain.visible_leaf_count.to_string(),
            ));
        }

        let interval_p50 = self.controls.profiler.interval_percentile(0.5);
        let mut status = vec![StatItem::new(
            "FPS",
            interval_p50.map_or("—".into(), |ms| format!("{:.0}", 1000.0 / ms.max(0.001))),
        )];
        if let Some(snapshot) = snapshot {
            status.push(StatItem::new(
                "CPU",
                snapshot
                    .performance
                    .frame_cpu_ms
                    .map_or("—".into(), |ms| format!("{ms:.2} ms")),
            ));
            status.push(StatItem::new(
                "GPU",
                snapshot
                    .performance
                    .gpu_frame_ms
                    .map_or("—".into(), |ms| format!("{ms:.2} ms")),
            ));
            if !snapshot.warnings.is_empty() {
                status.push(
                    StatItem::new("Warnings", snapshot.warnings.len().to_string()).tone(Tone::Warn),
                );
            }
        }
        status.push(StatItem::new("View", self.controls.terrain_view.name()));

        #[cfg(feature = "developer-tools")]
        let automation_owner = self.controls.automation_owner.clone().unwrap_or_default();
        #[cfg(not(feature = "developer-tools"))]
        let automation_owner = String::new();

        self.view = StudioView {
            paused: self.motion.paused(),
            time_text: format!(
                "T+{:.3} d",
                self.system.sample_time().seconds_since_epoch() / 86_400.0
            ),
            rate_index: rate_preset_index(self.motion.rate().multiplier()),
            camera_index: camera_index(self.camera.mode()),
            view_index: TerrainViewMode::ALL
                .iter()
                .position(|mode| *mode == self.controls.terrain_view)
                .unwrap_or(0),
            terrain_enabled: self.controls.terrain_preview,
            labels_enabled: self.controls.labels,
            overlays: [
                self.controls.markers,
                self.controls.trails,
                self.controls.guide_visible,
            ],
            bodies,
            labels,
            hud,
            inspector_title: body.map_or("—".into(), |body| body.name().into()),
            inspector_subtitle: if surface_available {
                "Celestial body · terrain".into()
            } else {
                "Celestial body".into()
            },
            body_stats,
            camera_stats,
            terrain_stats,
            speed_exponent: self.controls.manual_speed.log10() as f32,
            surface_available,
            status,
            automation_owner,
            build_text: format!(
                "{} · scene {}",
                crate::shared_system::SCENE_NAME,
                self.scene_sha256.get(..8).unwrap_or_default()
            ),
            log: self.controls.log.iter().rev().cloned().collect(),
            render_settings: {
                let state = self.render_state();
                (0..crate::render_settings::SPECS.len())
                    .map(|index| crate::render_settings::get(&state, index))
                    .collect()
            },
            render_stats: snapshot.map_or_else(Vec::new, |snapshot| {
                let mut stats: Vec<StatItem> = self
                    .sun_elevation
                    .map(|(_, elevation)| {
                        StatItem::new(
                            "Sun elevation",
                            if elevation < 0.0 {
                                format!("{elevation:.1}° (night)")
                            } else {
                                format!("{elevation:.1}°")
                            },
                        )
                        .tone(if elevation < 0.0 {
                            Tone::Warn
                        } else {
                            Tone::Normal
                        })
                    })
                    .into_iter()
                    .collect();
                stats.extend(
                    snapshot
                        .performance
                        .gpu_scopes
                        .iter()
                        .filter(|scope| scope.depth > 0)
                        .map(|scope| {
                            StatItem::new(
                                &scope.name,
                                format!("{:.2} ms", scope.end_ms - scope.start_ms),
                            )
                        }),
                );
                if let Some(shadows) = snapshot.render_settings.as_ref().map(|r| &r["shadows"])
                    && let Some(count) = shadows["cascades"].as_u64().filter(|c| *c > 0)
                {
                    {
                        let splits: Vec<String> = shadows["splits_m"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .take(count as usize)
                            .filter_map(|v| v.as_f64())
                            .map(compact_distance)
                            .collect();
                        stats.push(StatItem::new("Cascades", splits.join(" / ")));
                        let casters: u64 = shadows["casters"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_u64())
                            .sum();
                        stats.push(StatItem::new("Caster draws", casters.to_string()));
                    }
                }
                stats
            }),
            gpu_passes: snapshot.map_or([None; 3], |snapshot| {
                [
                    snapshot.performance.gpu_main_pass_ms,
                    snapshot.performance.gpu_terrain_ms,
                    snapshot.performance.gpu_overlay_ms,
                ]
            }),
            planet: self.planet_view(),
        };
    }

    pub(super) fn render_state(&self) -> crate::render_settings::RenderState {
        crate::render_settings::RenderState {
            settings: self.controls.render_settings,
            view_mode: self.controls.terrain_view,
        }
    }

    /// Applies one registry value to the session render state.
    pub(super) fn apply_render_setting(
        &mut self,
        index: usize,
        value: crate::render_settings::SettingValue,
    ) -> Result<()> {
        let mut state = self.render_state();
        crate::render_settings::set(&mut state, index, value)?;
        self.controls.render_settings = state.settings;
        self.controls.terrain_view = state.view_mode;
        Ok(())
    }

    /// Places body labels with the shared collision-avoiding layout and returns
    /// them in viewport logical pixels; `controls.placed` keeps physical rects
    /// for picking, in the same order.
    fn layout_labels(
        &mut self,
        markers: &[CelestialMarker],
        selected_index: usize,
        focused: Option<BodyId>,
        scale: f64,
    ) -> Vec<LabelItem> {
        let Some((origin, size)) = self.viewport else {
            return Vec::new();
        };
        let viewport = ScreenRect {
            min: origin.map(f64::from),
            max: [
                f64::from(origin[0] + size[0]),
                f64::from(origin[1] + size[1]),
            ],
        };
        let mut inputs = Vec::new();
        let mut texts = Vec::new();
        let mut rings = Vec::new();
        for marker in markers {
            let Some(&id) = self.ids.get(marker.request_index) else {
                continue;
            };
            let Some([x, y]) = marker.screen_pixels else {
                continue;
            };
            let selected = marker.request_index == selected_index;
            let is_focused = focused == Some(id);
            let state = if selected {
                LabelState::Selected
            } else if is_focused {
                LabelState::Focused
            } else {
                LabelState::Normal
            };
            let ring = if (selected || is_focused)
                && self.camera.mode() != CameraMode::SurfaceInspection
            {
                ((marker.apparent_diameter_pixels * 0.5 + 5.0).clamp(8.0, 2000.0) / scale) as f32
            } else {
                0.0
            };
            rings.push((id, [x / scale as f32, y / scale as f32], state, ring));
            if !self.controls.labels {
                continue;
            }
            let Ok(body) = self.system.body(id) else {
                continue;
            };
            let duplicate = self
                .system
                .bodies()
                .filter(|(_, other)| other.name() == body.name())
                .count()
                > 1;
            let name = if duplicate {
                format!("{} #{}", body.name(), marker.request_index)
            } else {
                body.name().into()
            };
            let text = format!(
                "{} · {}{}",
                name,
                compact_distance(marker.distance_m),
                if marker.occluded { " (overlay)" } else { "" }
            );
            let width = (text.chars().count() as f64 * LABEL_CHAR_WIDTH + LABEL_PADDING) * scale;
            inputs.push(LabelInput {
                body: id,
                marker: [f64::from(x), f64::from(y)],
                size: [width, LABEL_HEIGHT * scale],
                selected,
                focused: is_focused,
                hovered: false,
                diameter: marker.apparent_diameter_pixels,
                distance_m: marker.distance_m,
            });
            texts.push((id, text));
        }
        self.controls
            .layout
            .layout(&inputs, viewport, &[], &mut self.controls.placed);
        if !self.controls.labels {
            self.controls.placed.clear();
        }
        let show_marker = self.controls.markers;
        let mut items: Vec<LabelItem> = self
            .controls
            .placed
            .iter()
            .filter_map(|label| {
                let (_, text) = texts.iter().find(|(id, _)| *id == label.body)?;
                let (_, marker, state, ring) = rings.iter().find(|r| r.0 == label.body)?;
                Some(LabelItem {
                    rect: [
                        (label.rect.min[0] / scale) as f32,
                        (label.rect.min[1] / scale) as f32,
                        ((label.rect.max[0] - label.rect.min[0]) / scale) as f32,
                        ((label.rect.max[1] - label.rect.min[1]) / scale) as f32,
                    ],
                    marker: *marker,
                    text: text.clone(),
                    state: *state,
                    leader: label.leader,
                    show_marker,
                    ring: *ring,
                })
            })
            .collect();
        // Bodies without a placed label still show their marker and ring.
        for (id, marker, state, ring) in &rings {
            if self.controls.placed.iter().any(|label| label.body == *id) {
                continue;
            }
            if !show_marker && *ring == 0.0 {
                continue;
            }
            items.push(LabelItem {
                rect: [marker[0], marker[1], 0.0, 0.0],
                marker: *marker,
                text: String::new(),
                state: *state,
                leader: false,
                show_marker,
                ring: *ring,
            });
        }
        items
    }
}

fn camera_index(mode: CameraMode) -> usize {
    match mode {
        CameraMode::SystemOrbit => 0,
        CameraMode::BodyOrbit => 1,
        CameraMode::SurfaceInspection => 2,
        CameraMode::FreeFlight => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_actions_reach_the_command_queue() {
        let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
        let paused = demo.motion.paused();
        demo.studio_action(StudioAction::TogglePause);
        demo.studio_action(StudioAction::SetRate(2));
        demo.studio_action(StudioAction::SetView(2));
        demo.update(Duration::ZERO);
        assert_eq!(demo.motion.paused(), !paused);
        assert_eq!(demo.motion.rate().multiplier(), RATE_PRESETS[2]);
        assert_eq!(demo.controls.terrain_view, TerrainViewMode::ALL[2]);
        demo.studio_action(StudioAction::SetSpeedExponent(1.0));
        assert!((demo.controls.manual_speed - 10.0).abs() < 1e-12);
    }
}
