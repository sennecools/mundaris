//! Planet editor v1 (`docs/ASTRUM_TERRAIN_PIPELINE.md` §18.1, M1 Step 6):
//! live seed and parameter overrides of world-map bodies, hot reload of their
//! authored files and saving back to the terrain definition.
//!
//! The editor owns only editable state. Each change rebuilds the body's
//! `SurfaceDefinition` from the current files plus the edit; the frame owner
//! publishes it to the celestial system, whose terrain revision makes the
//! atlas rebind and the GPU re-bake Tier A.
use crate::shared_system::{BodyPresentation, WorldEdit, WorldSource};
use anyhow::Result;
use astrum_world::terrain::SurfaceDefinition;
use std::time::{Duration, Instant, SystemTime};

/// Edits are applied at most this often while a slider is dragged.
const APPLY_INTERVAL: Duration = Duration::from_millis(100);
/// Authored files are checked for changes this often.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Same-kind changes closer together than this form one undo step.
const COALESCE: Duration = Duration::from_millis(600);
/// Undo steps kept per body.
const UNDO_LIMIT: usize = 200;

/// One editable world-map body.
#[derive(Debug, Clone)]
pub struct EditableWorld {
    /// Index of the body in presentation (and body id) order.
    pub body_index: usize,
    pub source: WorldSource,
    /// Current (possibly unsaved) edit.
    pub edit: WorldEdit,
    /// Edit of the definition last published to the system.
    applied: WorldEdit,
    stamps: Vec<Option<SystemTime>>,
    /// Last rebuild or save failure; the previous definition stays live.
    pub error: Option<String>,
    /// When the last definition was published, and why.
    pub published: Option<(Instant, &'static str)>,
    /// Earlier edits for undo (newest last) and undone edits for redo.
    undo: Vec<WorldEdit>,
    redo: Vec<WorldEdit>,
    /// Kind and time of the last recorded change, so a seed drag undoes as
    /// one step.
    last_change: Option<(Instant, &'static str)>,
}

impl EditableWorld {
    /// The edit differs from the terrain file.
    pub fn dirty(&self) -> bool {
        self.edit != self.source.saved
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn override_of(&self, name: &str) -> Option<f64> {
        self.edit
            .overrides
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
    }
}

fn stamps(paths: &[std::path::PathBuf]) -> Vec<Option<SystemTime>> {
    paths
        .iter()
        .map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .collect()
}

/// Editable state of every world-map body in a loaded system.
#[derive(Debug, Default)]
pub struct PlanetEditor {
    worlds: Vec<EditableWorld>,
    last_apply: Option<Instant>,
    last_poll: Option<Instant>,
}

impl PlanetEditor {
    pub fn new(presentation: &[BodyPresentation]) -> Self {
        let worlds = presentation
            .iter()
            .enumerate()
            .filter_map(|(body_index, p)| {
                let source = p.world.clone()?;
                Some(EditableWorld {
                    body_index,
                    edit: source.saved.clone(),
                    applied: source.saved.clone(),
                    stamps: stamps(&source.dependencies),
                    source,
                    error: None,
                    published: None,
                    undo: Vec::new(),
                    redo: Vec::new(),
                    last_change: None,
                })
            })
            .collect();
        Self {
            worlds,
            last_apply: None,
            last_poll: None,
        }
    }

    pub fn world(&self, body_index: usize) -> Option<&EditableWorld> {
        self.worlds.iter().find(|w| w.body_index == body_index)
    }

    fn world_mut(&mut self, body_index: usize) -> Option<&mut EditableWorld> {
        self.worlds.iter_mut().find(|w| w.body_index == body_index)
    }

    /// Apply `change` to the edit and record the previous edit for undo.
    /// Consecutive changes of the same `kind` within [`COALESCE`] merge into
    /// one undo step (seed drags, slider nudges of one parameter).
    fn change(
        &mut self,
        body_index: usize,
        kind: &'static str,
        change: impl FnOnce(&mut WorldEdit),
    ) {
        let Some(world) = self.world_mut(body_index) else {
            return;
        };
        let before = world.edit.clone();
        change(&mut world.edit);
        if world.edit == before {
            return;
        }
        let now = Instant::now();
        let merge = world
            .last_change
            .is_some_and(|(at, last)| last == kind && now.duration_since(at) < COALESCE);
        if !merge {
            world.undo.push(before);
            if world.undo.len() > UNDO_LIMIT {
                world.undo.remove(0);
            }
        }
        world.redo.clear();
        world.last_change = Some((now, kind));
    }

    /// Step back to the edit before the last change.
    pub fn undo(&mut self, body_index: usize) {
        if let Some(world) = self.world_mut(body_index)
            && let Some(previous) = world.undo.pop()
        {
            world.redo.push(std::mem::replace(&mut world.edit, previous));
            world.last_change = None;
        }
    }

    /// Re-apply the last undone change.
    pub fn redo(&mut self, body_index: usize) {
        if let Some(world) = self.world_mut(body_index)
            && let Some(next) = world.redo.pop()
        {
            world.undo.push(std::mem::replace(&mut world.edit, next));
            world.last_change = None;
        }
    }

    pub fn set_seed(&mut self, body_index: usize, seed: u64) {
        self.change(body_index, "seed", |edit| edit.seed = seed);
    }

    /// Override parameter `name`; validation happens on rebuild.
    pub fn set_param(&mut self, body_index: usize, name: &str, value: f64) {
        self.change(body_index, "param", |edit| {
            match edit.overrides.iter_mut().find(|(n, _)| n == name) {
                Some(entry) => entry.1 = value,
                None => edit.overrides.push((name.to_string(), value)),
            }
        });
    }

    /// Drop the override of `name`, returning to the sampled value.
    pub fn reset_param(&mut self, body_index: usize, name: &str) {
        self.change(body_index, "reset", |edit| {
            edit.overrides.retain(|(n, _)| n != name);
        });
    }

    /// Discard unsaved changes.
    pub fn revert(&mut self, body_index: usize) {
        let Some(saved) = self.world(body_index).map(|w| w.source.saved.clone()) else {
            return;
        };
        self.change(body_index, "revert", |edit| *edit = saved);
    }

    /// Write the current edit into the body's terrain file.
    pub fn save(&mut self, body_index: usize) -> Result<()> {
        let world = self
            .world_mut(body_index)
            .ok_or_else(|| anyhow::anyhow!("body is not a world map"))?;
        let result = world.source.save(&world.edit);
        match &result {
            Ok(()) => {
                world.source.saved = world.edit.clone();
                // The terrain file changed under us; do not treat it as an
                // external edit on the next poll.
                world.stamps = stamps(&world.source.dependencies);
                world.error = None;
            }
            Err(error) => world.error = Some(format!("{error:#}")),
        }
        result
    }

    /// Per frame: rebuild bodies whose edit changed (throttled) or whose
    /// authored files changed on disk (polled). Returns the definitions to
    /// publish, by body index. Failures keep the previous definition live and
    /// are reported through `EditableWorld::error`.
    pub fn update(&mut self, now: Instant) -> Vec<(usize, SurfaceDefinition)> {
        let apply_due = self
            .last_apply
            .is_none_or(|at| now.duration_since(at) >= APPLY_INTERVAL);
        let poll_due = self
            .last_poll
            .is_none_or(|at| now.duration_since(at) >= POLL_INTERVAL);
        if poll_due {
            self.last_poll = Some(now);
        }
        let mut out = Vec::new();
        let mut applied_edit = false;
        for world in &mut self.worlds {
            let files_changed = poll_due && stamps(&world.source.dependencies) != world.stamps;
            let edited = apply_due && world.edit != world.applied;
            if !files_changed && !edited {
                continue;
            }
            // Stamps are taken before the files are read, so a write that
            // lands during the rebuild is seen on the next poll.
            let new_stamps = stamps(&world.source.dependencies);
            applied_edit |= edited;
            let mut used = world.edit.clone();
            let mut result = world.source.rebuild(&used);
            // An external change of the terrain file replaces the saved edit;
            // adopt it unless there are unsaved changes, and build with it
            // now so one change publishes once.
            if let Ok((_, source)) = &result
                && source.saved != world.source.saved
                && world.edit == world.source.saved
            {
                world.edit = source.saved.clone();
                used = world.edit.clone();
                result = world.source.rebuild(&used);
            }
            world.stamps = new_stamps;
            match result {
                Ok((definition, source)) => {
                    world.source = source;
                    world.applied = used;
                    world.error = None;
                    world.published = Some((
                        now,
                        if files_changed {
                            "files changed"
                        } else {
                            "edited"
                        },
                    ));
                    out.push((world.body_index, definition));
                }
                Err(error) => {
                    // Do not retry the same failing edit every frame.
                    world.applied = used;
                    world.error = Some(format!("{error:#}"));
                }
            }
        }
        if applied_edit {
            self.last_apply = Some(now);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A scratch copy of the canonical content (only what Rust's terrain
    /// needs), so saving and hot reload never touch the real files.
    fn scratch_content(tag: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let root =
            std::env::temp_dir().join(format!("astrum-planet-editor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for file in [
            "terrain/rust.ron",
            "archetypes/terra.ron",
            "lut/terra_whittaker.ron",
            "lut/terra_whittaker.png",
            "materials/snow/material.ron",
        ] {
            let to = root.join(file);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(source.join(file), to).unwrap();
        }
        root
    }

    fn editor(root: &Path) -> PlanetEditor {
        let world =
            crate::shared_system::load_world_source(root, "terrain/rust.ron", 338_950.0).unwrap();
        let presentation = BodyPresentation {
            identity: 1,
            semantic_id: "rust".into(),
            color: [1.0; 4],
            unlit: false,
            definition_sha256: None,
            definition_revision: None,
            world: Some(world),
        };
        PlanetEditor::new(&[presentation])
    }

    #[test]
    fn undo_and_redo_step_through_edits_and_merge_a_seed_drag() {
        let root = scratch_content("undo");
        let mut editor = editor(&root);
        let original = editor.world(0).unwrap().edit.clone();
        editor.set_seed(0, 11);
        editor.set_seed(0, 12);
        editor.set_seed(0, 13);
        editor.world_mut(0).unwrap().last_change = None;
        editor.set_param(0, "warp_strength", 0.5);
        assert!(editor.world(0).unwrap().can_undo());
        editor.undo(0);
        assert_eq!(editor.world(0).unwrap().edit.seed, 13);
        assert!(editor.world(0).unwrap().override_of("warp_strength").is_none());
        editor.undo(0);
        assert_eq!(editor.world(0).unwrap().edit, original);
        assert!(!editor.world(0).unwrap().can_undo());
        editor.redo(0);
        editor.redo(0);
        assert_eq!(editor.world(0).unwrap().override_of("warp_strength"), Some(0.5));
        editor.undo(0);
        editor.set_seed(0, 99);
        assert!(!editor.world(0).unwrap().can_redo());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn edits_rebuild_once_per_interval_and_bad_values_keep_the_old_surface() {
        let root = scratch_content("edit");
        let mut editor = editor(&root);
        let t0 = Instant::now();
        assert!(editor.update(t0).is_empty(), "nothing changed yet");
        editor.set_param(0, "rain", 0.03);
        let published = editor.update(t0 + Duration::from_millis(1));
        assert_eq!(published.len(), 1);
        let world = published[0].1.world().unwrap();
        assert_eq!(world.params.rain, 0.03);
        assert!(editor.world(0).unwrap().dirty());
        // Within the apply interval a further edit waits.
        editor.set_param(0, "rain", 0.035);
        assert!(editor.update(t0 + Duration::from_millis(20)).is_empty());
        assert_eq!(editor.update(t0 + Duration::from_millis(200)).len(), 1);
        // Out-of-bounds values fail validation: nothing is published.
        editor.set_param(0, "ocean_coverage", 5.0);
        assert!(editor.update(t0 + Duration::from_millis(400)).is_empty());
        assert!(editor.world(0).unwrap().error.is_some());
        editor.reset_param(0, "ocean_coverage");
        editor.set_seed(0, 99);
        let published = editor.update(t0 + Duration::from_millis(600));
        assert_eq!(published.len(), 1);
        assert!(editor.world(0).unwrap().error.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn save_round_trips_and_keeps_comments_and_hot_reload_sees_file_changes() {
        let root = scratch_content("save");
        let mut editor = editor(&root);
        let before = std::fs::read_to_string(root.join("terrain/rust.ron")).unwrap();
        let revision = |text: &str| -> u32 {
            let line = text.lines().find(|l| l.trim_start().starts_with("revision:")).unwrap();
            line.trim_start()["revision:".len()..].trim().trim_end_matches(',').parse().unwrap()
        };
        editor.set_seed(0, 1234);
        editor.set_param(0, "equator_c", 33.5);
        editor.set_param(0, "rain", 0.031);
        editor.save(0).unwrap();
        assert!(!editor.world(0).unwrap().dirty());
        let after = std::fs::read_to_string(root.join("terrain/rust.ron")).unwrap();
        assert!(after.contains("seed: 1234,"));
        assert!(after.contains("(\"equator_c\", 33.5)"));
        assert_eq!(revision(&after), revision(&before) + 1, "revision bumped");
        let comments = |text: &str| {
            text.lines()
                .filter(|l| l.trim_start().starts_with("//"))
                .count()
        };
        assert_eq!(comments(&before), comments(&after));
        // A fresh load sees the saved edit.
        let reloaded = editor_world_saved(&root);
        assert_eq!(reloaded.seed, 1234);
        assert_eq!(reloaded.overrides.len(), 2);
        // The save itself is not treated as an external change.
        let t0 = Instant::now();
        editor.update(t0);
        assert!(editor.update(t0 + Duration::from_secs(1)).is_empty());
        // An external archetype edit is picked up on the next poll.
        let archetype = root.join("archetypes/terra.ron");
        let text = std::fs::read_to_string(&archetype).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(
            &archetype,
            text.replace("lapse_c_per_km: 6.5", "lapse_c_per_km: 7.0"),
        )
        .unwrap();
        let published = editor.update(t0 + Duration::from_secs(2));
        assert_eq!(published.len(), 1, "hot reload rebuilt the body");
        assert_eq!(published[0].1.world().unwrap().params.lapse_c_per_km, 7.0);
        // A broken file keeps the previous surface and reports the error.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(&archetype, "(broken").unwrap();
        assert!(editor.update(t0 + Duration::from_secs(3)).is_empty());
        assert!(editor.world(0).unwrap().error.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    fn editor_world_saved(root: &Path) -> WorldEdit {
        crate::shared_system::load_world_source(root, "terrain/rust.ron", 338_950.0)
            .unwrap()
            .saved
    }
}
