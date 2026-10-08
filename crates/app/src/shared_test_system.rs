//! The ordinary shared solar system's authored prepared-surface bindings.
use anyhow::{Context, Result, ensure};
use mundaris_world::terrain::PreparedSurface;
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    catalogue_name: String,
    definition: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    schema_version: u32,
    scene_identity: String,
    surfaces: Vec<Binding>,
    initial_view: InitialView,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InitialView {
    pub catalogue_name: String,
    pub direction_body: [f64; 3],
    pub orientation_xyzw: [f64; 4],
    pub clearance_m: f64,
    pub paused: bool,
}
fn definition() -> Result<(PathBuf, Definition)> {
    let path = std::env::var_os("MUNDARIS_TEST_SYSTEM")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/prepared/shared-test-system.json")
        });
    let path = path
        .canonicalize()
        .context("locating the shared test solar system")?;
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(&path)?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 16 * 1024,
        "shared test definition exceeds cap"
    );
    let definition: Definition = serde_json::from_slice(&bytes)?;
    ensure!(
        definition.schema_version == 1
            && definition.scene_identity == "mundaris.shared-test-system/1"
            && definition.surfaces.len() <= 10,
        "invalid shared test system contract"
    );
    let view = &definition.initial_view;
    ensure!(
        !view.catalogue_name.is_empty()
            && view.catalogue_name.len() <= 128
            && view.clearance_m.is_finite()
            && (10.0..=1e6).contains(&view.clearance_m),
        "shared initial view/clearance"
    );
    let direction = glam::DVec3::from_array(view.direction_body);
    let orientation = glam::DQuat::from_array(view.orientation_xyzw);
    ensure!(
        direction.is_finite()
            && (direction.length() - 1.).abs() < 1e-10
            && orientation.is_finite()
            && (orientation.length() - 1.).abs() < 1e-10,
        "shared initial view basis"
    );
    Ok((path, definition))
}
pub(crate) fn initial_view() -> Result<InitialView> {
    Ok(definition()?.1.initial_view)
}
pub(crate) struct SharedTestSystem {
    surfaces: Vec<(String, Arc<PreparedSurface>)>,
}
impl SharedTestSystem {
    pub(crate) fn load() -> Result<Self> {
        let (path, definition) = definition()?;
        let root = path.parent().context("shared test content root")?;
        let mut surfaces = Vec::new();
        for binding in definition.surfaces {
            ensure!(
                !binding.catalogue_name.is_empty()
                    && binding.catalogue_name.len() <= 128
                    && !surfaces
                        .iter()
                        .any(|(name, _)| name == &binding.catalogue_name),
                "duplicate/invalid shared surface binding"
            );
            let relative = Path::new(&binding.definition);
            ensure!(
                !relative.is_absolute()
                    && relative
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_))),
                "surface binding path must remain in shared content"
            );
            let source = root.join(relative).canonicalize()?;
            ensure!(
                source.starts_with(root),
                "surface binding escapes shared content"
            );
            let started = std::time::Instant::now();
            let surface =
                Arc::new(PreparedSurface::load(&source).with_context(|| {
                    format!("loading prepared surface {}", binding.catalogue_name)
                })?);
            tracing::info!(body=%binding.catalogue_name, source=%source.display(), load_ms=started.elapsed().as_secs_f64()*1000., retained_source_bytes=surface.resident_bytes(), content=?surface.content_identity(),"shared prepared surface loaded");
            surfaces.push((binding.catalogue_name, surface));
        }
        Ok(Self { surfaces })
    }
    pub(crate) fn surface(&self, name: &str) -> Option<Arc<PreparedSurface>> {
        self.surfaces
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, s)| Arc::clone(s))
    }
}
