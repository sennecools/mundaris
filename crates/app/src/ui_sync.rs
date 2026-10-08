//! Pushes the toolkit-free view model into the Slint window.
//!
//! List models are created once and updated row by row, so Slint keeps its
//! item instances and only re-lays out rows whose content changed. Panel text
//! refreshes at a readable rate; the viewport overlay tracks every frame.

use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use mundaris_app::studio::view::{LabelState, StatItem, StudioView, Tone};
use slint::{Model, ModelRc, VecModel};

use crate::{BodyLabel, BodyRow, LogLine, Stat, StudioWindow};

/// Refresh period for panel text (inspector, status bar, profiler readouts).
pub const PANEL_PERIOD: Duration = Duration::from_millis(100);

pub fn tone(tone: Tone) -> i32 {
    match tone {
        Tone::Normal => 0,
        Tone::Ok => 1,
        Tone::Warn => 2,
        Tone::Error => 3,
    }
}

pub fn stat(item: &StatItem) -> Stat {
    Stat {
        label: item.label.as_str().into(),
        value: item.value.as_str().into(),
        tone: tone(item.tone),
    }
}

/// Updates `model` to `next`, touching only rows that changed.
pub fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, next: Vec<T>) {
    if model.row_count() != next.len() {
        model.set_vec(next);
        return;
    }
    for (index, row) in next.into_iter().enumerate() {
        if model.row_data(index).as_ref() != Some(&row) {
            model.set_row_data(index, row);
        }
    }
}

fn new_model<T: Clone + 'static>() -> Rc<VecModel<T>> {
    Rc::new(VecModel::default())
}

/// Persistent list models bound to the window once.
pub struct UiModels {
    pub bodies: Rc<VecModel<BodyRow>>,
    pub labels: Rc<VecModel<BodyLabel>>,
    pub hud: Rc<VecModel<Stat>>,
    pub body_stats: Rc<VecModel<Stat>>,
    pub camera_stats: Rc<VecModel<Stat>>,
    pub terrain_stats: Rc<VecModel<Stat>>,
    pub status: Rc<VecModel<Stat>>,
    pub log: Rc<VecModel<LogLine>>,
    pub overlays: Rc<VecModel<bool>>,
    last_panels: Option<Instant>,
}

impl UiModels {
    pub fn bind(ui: &StudioWindow) -> Self {
        let models = Self {
            bodies: new_model(),
            labels: new_model(),
            hud: new_model(),
            body_stats: new_model(),
            camera_stats: new_model(),
            terrain_stats: new_model(),
            status: new_model(),
            log: new_model(),
            overlays: new_model(),
            last_panels: None,
        };
        ui.set_bodies(ModelRc::from(models.bodies.clone()));
        ui.set_labels(ModelRc::from(models.labels.clone()));
        ui.set_hud(ModelRc::from(models.hud.clone()));
        ui.set_body_stats(ModelRc::from(models.body_stats.clone()));
        ui.set_camera_stats(ModelRc::from(models.camera_stats.clone()));
        ui.set_terrain_stats(ModelRc::from(models.terrain_stats.clone()));
        ui.set_status_items(ModelRc::from(models.status.clone()));
        ui.set_log_lines(ModelRc::from(models.log.clone()));
        ui.set_overlays(ModelRc::from(models.overlays.clone()));
        models
    }

    /// True once per [`PANEL_PERIOD`]; panels update only then.
    pub fn panels_due(&mut self) -> bool {
        let now = Instant::now();
        if self
            .last_panels
            .is_some_and(|last| now.duration_since(last) < PANEL_PERIOD)
        {
            return false;
        }
        self.last_panels = Some(now);
        true
    }

    pub fn apply(&mut self, ui: &StudioWindow, view: &StudioView, panels_due: bool) {
        // Viewport overlay and toolbar state follow every frame.
        sync(
            &self.labels,
            view.labels
                .iter()
                .map(|label| BodyLabel {
                    x: label.rect[0],
                    y: label.rect[1],
                    width: label.rect[2],
                    height: label.rect[3],
                    marker_x: label.marker[0],
                    marker_y: label.marker[1],
                    text: label.text.as_str().into(),
                    state: match label.state {
                        LabelState::Normal => 0,
                        LabelState::Selected => 1,
                        LabelState::Focused => 2,
                    },
                    leader: label.leader,
                    show_marker: label.show_marker,
                    ring: label.ring,
                })
                .collect(),
        );
        ui.set_paused(view.paused);
        ui.set_rate_index(view.rate_index.map_or(-1, |index| index as i32));
        ui.set_camera_index(view.camera_index as i32);
        ui.set_view_index(view.view_index as i32);
        ui.set_terrain_enabled(view.terrain_enabled);
        ui.set_labels_enabled(view.labels_enabled);
        sync(&self.overlays, view.overlays.to_vec());
        ui.set_speed_exponent(view.speed_exponent);
        ui.set_surface_available(view.surface_available);
        ui.set_automation_owner(view.automation_owner.as_str().into());
        sync(
            &self.bodies,
            view.bodies
                .iter()
                .map(|body| BodyRow {
                    name: body.name.as_str().into(),
                    detail: body.detail.as_str().into(),
                    depth: body.depth as i32,
                    selected: body.selected,
                    focused: body.focused,
                })
                .collect(),
        );
        if !panels_due {
            return;
        }
        ui.set_time_text(view.time_text.as_str().into());
        sync(&self.hud, view.hud.iter().map(stat).collect());
        ui.set_inspector_title(view.inspector_title.as_str().into());
        ui.set_inspector_subtitle(view.inspector_subtitle.as_str().into());
        sync(&self.body_stats, view.body_stats.iter().map(stat).collect());
        sync(
            &self.camera_stats,
            view.camera_stats.iter().map(stat).collect(),
        );
        sync(
            &self.terrain_stats,
            view.terrain_stats.iter().map(stat).collect(),
        );
        sync(&self.status, view.status.iter().map(stat).collect());
        ui.set_build_text(view.build_text.as_str().into());
        sync(
            &self.log,
            view.log
                .iter()
                .map(|line| LogLine {
                    time: line.time.as_str().into(),
                    text: line.text.as_str().into(),
                    tone: tone(line.tone),
                })
                .collect(),
        );
    }
}
