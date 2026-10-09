//! Planet editor v1 over the demo (M1 Step 6): publishes rebuilt world-map
//! definitions, applies Studio planet actions and builds the panel view.
use super::*;
use crate::studio::view::{PlanetParamItem, PlanetView, StatItem, StudioAction, Tone};
use astrum_world::terrain::archetype::PARAM_FIELDS;

impl GravityOrbitsDemo {
    /// Publish definitions the planet editor rebuilt this frame. The terrain
    /// revision bump makes the atlas rebind and re-bake Tier A on the GPU.
    pub(super) fn publish_planet_edits(&mut self, now: Instant) {
        for (index, definition) in self.planet_editor.update(now) {
            let Some(&id) = self.ids.get(index) else {
                continue;
            };
            let name = self
                .system
                .body(id)
                .map_or_else(|_| "?".to_string(), |b| b.name().to_string());
            match self.system.edit_surface_definition(id, Some(definition)) {
                Ok(()) => {
                    let reason = self
                        .planet_editor
                        .world(index)
                        .and_then(|w| w.published)
                        .map_or("edited", |(_, reason)| reason);
                    if reason != "edited" {
                        self.log(Tone::Normal, format!("{name}: terrain reloaded ({reason})"));
                    }
                }
                Err(error) => self.log(Tone::Error, format!("{name}: {error}")),
            }
        }
    }

    /// Apply a planet editor action to the selected body.
    pub(super) fn planet_action(&mut self, action: StudioAction) {
        let index = self.selected_index();
        let name = |i: usize| PARAM_FIELDS.get(i).map(|f| f.name);
        match action {
            StudioAction::PlanetSeed(seed) => self.planet_editor.set_seed(index, seed),
            StudioAction::PlanetRandomSeed => {
                // Wall-clock entropy is fine for an authoring tool; the chosen
                // seed is shown and saved, so results stay reproducible.
                let nanos = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos() as u64);
                let seed = (nanos ^ (nanos >> 29)).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 33;
                self.planet_editor.set_seed(index, seed);
            }
            StudioAction::PlanetParam(field, value) => {
                if let Some(name) = name(field) {
                    self.planet_editor.set_param(index, name, value);
                }
            }
            StudioAction::PlanetResetParam(field) => {
                if let Some(name) = name(field) {
                    self.planet_editor.reset_param(index, name);
                }
            }
            StudioAction::PlanetRevert => self.planet_editor.revert(index),
            StudioAction::PlanetSave => match self.planet_editor.save(index) {
                Ok(()) => {
                    let file = self
                        .planet_editor
                        .world(index)
                        .map_or_else(String::new, |w| w.source.terrain.clone());
                    self.log(Tone::Ok, format!("Saved planet to {file}"));
                }
                Err(error) => self.log(Tone::Error, format!("Save failed: {error:#}")),
            },
            _ => {}
        }
    }

    /// Developer `PlanetEdit`: apply an edit to a world-map body.
    #[cfg(feature = "developer-tools")]
    pub(super) fn developer_planet_edit(
        &mut self,
        body: BodyId,
        seed: Option<u64>,
        params: &[(String, f64)],
        reset: bool,
    ) -> Result<()> {
        let index = self
            .ids
            .iter()
            .position(|id| *id == body)
            .context("unknown body")?;
        anyhow::ensure!(
            self.planet_editor.world(index).is_some(),
            "body is not an editable world map"
        );
        for (name, _) in params {
            anyhow::ensure!(
                PARAM_FIELDS.iter().any(|f| f.name == name),
                "unknown planet parameter {name}"
            );
        }
        if reset {
            for field in PARAM_FIELDS {
                self.planet_editor.reset_param(index, field.name);
            }
        }
        if let Some(seed) = seed {
            self.planet_editor.set_seed(index, seed);
        }
        for (name, value) in params {
            self.planet_editor.set_param(index, name, *value);
        }
        Ok(())
    }

    /// Planet editor panel of the selected body, if it is an editable world.
    pub(super) fn planet_view(&self) -> Option<PlanetView> {
        let index = self.selected_index();
        let editable = self.planet_editor.world(index)?;
        let body = self.system.body(self.ids[index]).ok()?;
        let world = body.surface_definition()?.world()?;
        let params = PARAM_FIELDS
            .iter()
            .map(|field| {
                let value = world.params.get(field.name).unwrap_or_default();
                let (low, high) = world
                    .archetype
                    .editor_range(field.name, value)
                    .unwrap_or((field.min, field.max));
                PlanetParamItem {
                    name: field.name.to_string(),
                    value,
                    range: [low.min(value), high.max(value)],
                    overridden: editable.override_of(field.name).is_some(),
                }
            })
            .collect();
        let mut stats = vec![StatItem::new(
            "Tier A",
            format!(
                "{}² per face",
                world
                    .archetype
                    .face_cells(body.properties().reference_radius_m())
            ),
        )];
        if let Some(seconds) = self.atlas.world_bake_seconds(self.ids[index]) {
            stats.push(StatItem::new("Bake", format!("{:.0} ms", seconds * 1000.0)));
        } else {
            stats.push(StatItem::new("Bake", "running").tone(Tone::Warn));
        }
        stats.push(StatItem::new(
            "Overrides",
            editable.edit.overrides.len().to_string(),
        ));
        if let Some(error) = &editable.error {
            stats.push(StatItem::new("Error", error.clone()).tone(Tone::Error));
        }
        Some(PlanetView {
            archetype: world.archetype.name.clone(),
            terrain_file: editable.source.terrain.clone(),
            seed: editable.edit.seed,
            dirty: editable.dirty(),
            params,
            stats,
        })
    }
}
