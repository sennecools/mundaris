//! The single authored test solar system shared by interactive and automated runs.
//! Parsing, asset verification and generator validation happen before worker demand.
use anyhow::{Context, Result, ensure};
use astrum_math::*;
use astrum_world::{terrain::*, *};
use glam::{DQuat, DVec3};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    num::NonZeroU64,
    path::{Path, PathBuf},
};

pub const SCENE_NAME: &str = "test-solar-system";
const MAX_DEFINITION_BYTES: u64 = 256 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemContent {
    schema: u32,
    id: String,
    initial_body: String,
    camera_state: String,
    /// Atlas terrain LOD policy (ADR 0016).
    lod: crate::planet_lod::LodPolicy,
    /// Read-back collision policy (pipeline §15.1, ADR 0023).
    collision: crate::planet_lod::collision::CollisionPolicy,
    bodies: Vec<BodyContent>,
}

/// Check a profile's declared provenance and convert its u16 samples to the
/// canonical little-endian order world authority consumes.
fn canonical_profile_bytes(
    provenance: &str,
    byte_order: &str,
    mut bytes: Vec<u8>,
) -> Result<Vec<u8>> {
    ensure!(
        matches!(
            provenance,
            "original-bake" | "public-domain-derived" | "temporary-reference-input"
        ),
        "profile provenance must be original-bake, public-domain-derived or temporary-reference-input"
    );
    match byte_order {
        "little-endian" => {}
        "big-endian" => {
            for pair in bytes.as_chunks_mut::<2>().0 {
                pair.swap(0, 1);
            }
        }
        _ => anyhow::bail!("profile byte_order must be little-endian or big-endian"),
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_bytes_honour_declared_provenance_and_byte_order() {
        let le = vec![0x34, 0x12, 0x78, 0x56];
        let be = vec![0x12, 0x34, 0x56, 0x78];
        assert_eq!(
            canonical_profile_bytes("original-bake", "little-endian", le.clone()).unwrap(),
            le
        );
        assert_eq!(
            canonical_profile_bytes("temporary-reference-input", "big-endian", be).unwrap(),
            le
        );
        assert_eq!(
            canonical_profile_bytes("public-domain-derived", "little-endian", le.clone()).unwrap(),
            le
        );
        assert!(canonical_profile_bytes("downloaded", "little-endian", le.clone()).is_err());
        assert!(canonical_profile_bytes("original-bake", "middle-endian", le).is_err());
    }

    #[test]
    fn shared_content_loads_two_distinct_authorities_and_complete_camera() {
        let loaded = SharedTestSystem::load_canonical(NonZeroU64::new(71).unwrap()).unwrap();
        assert_eq!(loaded.presentation.len(), 3);
        assert_eq!(loaded.scene_sha256.len(), 64);
        assert_eq!(loaded.camera_sha256.len(), 64);
        let surfaces: Vec<_> = loaded
            .system
            .bodies()
            .filter(|(_, body)| body.has_surface())
            .collect();
        assert_eq!(surfaces.len(), 2);
        assert_ne!(
            surfaces[0].1.surface_definition(),
            surfaces[1].1.surface_definition()
        );
        let (id, body) = surfaces[0];
        let position = DVec3::from_array(loaded.camera.position_body_m);
        let clearance = crate::terrain_inspection::clearance_at_body_position(body, position, id)
            .unwrap()
            .unwrap();
        assert!(clearance.clearance_m >= 1.0);
        assert!(
            clearance.clearance_m < 10.0,
            "canonical clearance {clearance:?}"
        );
        assert!(
            loaded
                .presentation
                .iter()
                .filter(|p| p.definition_sha256.is_some())
                .all(|p| p.definition_revision.is_some())
        );
    }

    #[test]
    fn authored_paths_cannot_escape_the_content_root() {
        let content_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let root = content_root.as_path();
        for invalid in [
            "../secret",
            "terrain/../../secret",
            "C:/outside",
            "/outside",
        ] {
            assert!(local_path(root, invalid).is_err(), "{invalid}");
        }
        assert!(local_path(root, "terrain/moon.ron").is_ok());
    }
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BodyContent {
    id: String,
    name: String,
    identity: u64,
    mass_kg: f64,
    radius_m: f64,
    color: [f32; 4],
    unlit: bool,
    translation: TranslationContent,
    spin_axis: [f64; 3],
    spin_orientation_xyzw: [f64; 4],
    spin_radians_per_second: f64,
    terrain: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum TranslationContent {
    Stationary {
        position_m: [f64; 3],
    },
    Elliptic {
        reference: String,
        semi_major_axis_m: f64,
        eccentricity: f64,
        period_s: f64,
        inclination_radians: f64,
        phase_radians: f64,
    },
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainContent {
    schema: u32,
    id: String,
    revision: u32,
    identity: u64,
    seed: u64,
    algorithm: String,
    geology: GeologicalDistribution,
    procedural: Option<MoonFieldDefinition>,
    profile: Option<ProfileContent>,
    /// Optional band-limited fBm detail layer (pipeline §9.3).
    #[serde(default)]
    detail_noise: Option<astrum_world::terrain::noise::DetailNoiseDefinition>,
    /// Tier A world map of a `WorldV1` surface (pipeline §6, M1).
    #[serde(default)]
    world: Option<WorldContent>,
    material_composition: [f64; 2],
    material_contrast: f64,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorldContent {
    /// Archetype RON file relative to the content root.
    archetype: String,
    /// Named `PlanetParams` overrides (planet editor saves).
    #[serde(default)]
    overrides: Vec<(String, f64)>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileContent {
    asset: String,
    sha256: String,
    /// `original-bake` or `public-domain-derived` (a `mundaris.terrain-bundle.v1`
    /// height channel; the latter synthesized from credited public-domain DEMs,
    /// `docs/PLANET_DATA_PIPELINE.md` §3) or `temporary-reference-input` (test
    /// data that must not ship).
    provenance: String,
    /// `little-endian` or `big-endian` u16 samples.
    byte_order: String,
    width: u32,
    height: u32,
    kernel: String,
    footprint_m: f64,
    amplitude_m: f64,
    detail_layers: Vec<DetailContent>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DetailContent {
    footprint_m: f64,
    amplitude_m: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedCameraState {
    pub schema: u32,
    pub body: String,
    pub radius_m: f64,
    pub position_body_m: [f64; 3],
    pub orientation_xyzw: [f64; 4],
    pub paused: bool,
    pub rate: f64,
}
pub struct BodyPresentation {
    pub identity: u64,
    pub semantic_id: String,
    pub color: [f32; 4],
    pub unlit: bool,
    pub definition_sha256: Option<String>,
    pub definition_revision: Option<u32>,
    /// Editable world-map source (planet editor), if any.
    pub world: Option<WorldSource>,
}
pub struct SharedTestSystem {
    pub system: CelestialSystem,
    pub lod: crate::planet_lod::LodPolicy,
    pub collision: crate::planet_lod::collision::CollisionPolicy,
    pub motion: CelestialMotionDefinition,
    pub presentation: Vec<BodyPresentation>,
    pub initial_body_index: usize,
    pub camera: SharedCameraState,
    pub scene_sha256: String,
    pub camera_sha256: String,
}

fn bounded_bytes(path: &Path) -> Result<Vec<u8>> {
    let file = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    ensure!(
        file.metadata()?.len() <= MAX_DEFINITION_BYTES,
        "authored definition exceeds 256 KiB"
    );
    let mut bytes = Vec::new();
    file.take(MAX_DEFINITION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_DEFINITION_BYTES,
        "authored definition grew beyond limit"
    );
    Ok(bytes)
}
/// Parse an authored definition: RON for `.ron` files (hand-authored data with
/// comments), JSON otherwise. The hash covers the exact bytes read.
fn parse<T: serde::de::DeserializeOwned>(path: &Path) -> Result<(T, String)> {
    let bytes = bounded_bytes(path)?;
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let value = if path.extension().is_some_and(|e| e == "ron") {
        ron::de::from_bytes(&bytes).with_context(|| format!("parsing {}", path.display()))?
    } else {
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?
    };
    Ok((value, hash))
}
/// Resolve an archetype's colour references: biome LUT metadata and image,
/// snow material.
fn world_look(
    root: &Path,
    archetype: &astrum_world::terrain::archetype::PlanetArchetype,
) -> Result<(astrum_world::terrain::world_field::WorldLook, Vec<PathBuf>)> {
    use astrum_world::terrain::{
        archetype::MaterialAsset,
        biome_lut::{BiomeLut, LutMetadata},
    };
    let metadata_path = local_path(root, &archetype.biome_lut)?;
    let (metadata, _): (LutMetadata, _) = parse(&metadata_path)?;
    let image_path = local_path(root, &metadata.image)?;
    let image = bounded_bytes(&image_path)?;
    let lut = BiomeLut::from_png(&metadata, &image)
        .with_context(|| format!("invalid biome LUT {}", metadata.image))?;
    let snow_path = local_path(root, &archetype.snow.material)?;
    let (snow, _): (MaterialAsset, _) = parse(&snow_path)?;
    let look = astrum_world::terrain::world_field::WorldLook::new(archetype, lut, &snow)
        .context("invalid world colour")?;
    Ok((look, vec![metadata_path, image_path, snow_path]))
}

fn local_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    ensure!(
        !path.is_absolute()
            && path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "content path must stay beneath content directory"
    );
    let root = root.canonicalize()?;
    let result = root
        .join(path)
        .canonicalize()
        .with_context(|| format!("resolving content {relative}"))?;
    ensure!(
        result.starts_with(&root),
        "content link escapes content directory"
    );
    Ok(result)
}

/// Body-fixed rotation axis: the spin axis expressed in the body's rest frame.
fn body_pole(body: &BodyContent) -> Result<glam::DVec3> {
    let q = glam::DQuat::from_array(body.spin_orientation_xyzw);
    let axis = glam::DVec3::from_array(body.spin_axis);
    ensure!(
        q.is_finite() && q.length() > 0.0 && axis.is_finite() && axis.length() > 0.0,
        "invalid spin axis or orientation"
    );
    Ok((q.normalize().inverse() * axis).normalize())
}

/// A terrain definition loaded from content, with the editable world-map
/// source when the body has one.
pub(crate) struct LoadedTerrain {
    pub definition: SurfaceDefinition,
    pub sha256: String,
    pub revision: u32,
    pub world: Option<WorldSource>,
}

/// Planet editor state of a world-map body (M1 Step 6): the body seed and
/// named `PlanetParams` overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldEdit {
    pub seed: u64,
    pub overrides: Vec<(String, f64)>,
}

/// Where a world-map body's terrain comes from, for regeneration, hot reload
/// and saving.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldSource {
    /// Content root (absolute) and terrain definition path relative to it.
    pub root: PathBuf,
    pub terrain: String,
    pub radius_m: f64,
    pub pole: glam::DVec3,
    /// Seed and overrides as authored in the terrain file.
    pub saved: WorldEdit,
    /// Files whose change requires a rebuild: terrain, archetype, LUT
    /// metadata and image, snow material.
    pub dependencies: Vec<PathBuf>,
}

impl WorldSource {
    /// Rebuild the body's surface from current files with `edit` applied.
    pub fn rebuild(&self, edit: &WorldEdit) -> Result<(SurfaceDefinition, WorldSource)> {
        let loaded = terrain_definition(&self.root, &self.terrain, self.radius_m, self.pole, Some(edit))?;
        let world = loaded.world.context("terrain is no longer a world map")?;
        Ok((loaded.definition, world))
    }

    /// Write `edit` into the terrain file: the `seed` and the `overrides` list.
    /// Everything else (comments included) is kept; the revision is bumped.
    pub fn save(&self, edit: &WorldEdit) -> Result<()> {
        let path = local_path(&self.root, &self.terrain)?;
        let text = String::from_utf8(bounded_bytes(&path)?)?;
        let saved = rewrite_world_edit(&text, edit)?;
        let check: TerrainContent = ron::from_str(&saved).context("saved terrain does not parse")?;
        ensure!(
            check.seed == edit.seed && check.world.is_some_and(|w| w.overrides == edit.overrides),
            "saved terrain does not round-trip"
        );
        fs::write(&path, saved).with_context(|| format!("writing {}", path.display()))
    }
}

/// Editable source of a world-map terrain file on its own (pole along +Y);
/// for tools and tests that need no scene.
pub fn load_world_source(root: &Path, terrain: &str, radius_m: f64) -> Result<WorldSource> {
    terrain_definition(root, terrain, radius_m, glam::DVec3::Y, None)?
        .world
        .context("terrain is not a world map")
}

/// Replace the first `seed: N,` and the `overrides: [...]` list of a terrain
/// RON file and bump `revision`.
fn rewrite_world_edit(text: &str, edit: &WorldEdit) -> Result<String> {
    let field = |text: &str, name: &str| -> Result<(usize, usize)> {
        let key = format!("
    {name}:");
        let start = text.find(&key).with_context(|| format!("terrain file has no top-level {name}"))? + key.len();
        let end = start + text[start..].find(",
").context("unterminated field")?;
        Ok((start, end))
    };
    let mut out = text.to_string();
    let (start, end) = field(&out, "revision")?;
    let revision: u32 = out[start..end].trim().parse().context("revision is not a number")?;
    out.replace_range(start..end, &format!(" {}", revision + 1));
    let (start, end) = field(&out, "seed")?;
    out.replace_range(start..end, &format!(" {}", edit.seed));
    let key = "overrides:";
    let start = out.find(key).context("terrain file has no world overrides")? + key.len();
    let open = start + out[start..].find('[').context("overrides is not a list")?;
    let close = open + out[open..].find(']').context("unterminated overrides")?;
    let list = if edit.overrides.is_empty() {
        "[]".to_string()
    } else {
        let items: String = edit
            .overrides
            .iter()
            .map(|(name, value)| format!("
            (\"{name}\", {value:?}),"))
            .collect();
        format!("[{items}
        ]")
    };
    out.replace_range(open..=close, &list);
    Ok(out)
}

fn terrain_definition(
    root: &Path,
    name: &str,
    radius_m: f64,
    pole: glam::DVec3,
    edit: Option<&WorldEdit>,
) -> Result<LoadedTerrain> {
    let terrain_path = local_path(root, name)?;
    let (mut content, hash): (TerrainContent, _) = parse(&terrain_path)?;
    let saved = content.world.as_ref().map(|w| WorldEdit {
        seed: content.seed,
        overrides: w.overrides.clone(),
    });
    if let (Some(edit), Some(world)) = (edit, content.world.as_mut()) {
        content.seed = edit.seed;
        world.overrides = edit.overrides.clone();
    }
    ensure!(
        content.schema == 1 && content.revision > 0 && !content.id.is_empty(),
        "invalid terrain schema, identity or revision"
    );
    let algorithm = match content.algorithm.as_str() {
        "MoonProfileV1" => SurfaceAlgorithm::MoonProfileV1,
        "MoonFieldsV1" => SurfaceAlgorithm::MoonFieldsV1,
        "WorldV1" => SurfaceAlgorithm::WorldV1,
        _ => anyhow::bail!(
            "unsupported authored terrain algorithm {}",
            content.algorithm
        ),
    };
    let seed = TerrainSeed(content.seed);
    let parameters = content.geology.generate(algorithm, seed)?;
    let generated =
        SurfaceDefinition::generated(TerrainIdentity(content.identity), seed, algorithm);
    let composition = content.material_composition[0]
        + content.material_composition[1] * generated.material().composition();
    ensure!(
        content.material_composition.iter().all(|v| v.is_finite())
            && content.material_composition[0] >= 0.0
            && content.material_composition[1] >= 0.0
            && content.material_composition.iter().sum::<f64>() <= 1.0,
        "invalid material composition distribution"
    );
    let mut definition = SurfaceDefinition::new(
        TerrainIdentity(content.identity),
        seed,
        ShapeDefinition::sphere(),
        SurfaceTerrainDefinition::new(algorithm, parameters)?,
        SurfaceMaterialDefinition::new(
            generated.material().version(),
            composition,
            content.material_contrast,
        )?,
        SurfaceAtmosphere::Airless,
    )?
    .with_geological_distribution(content.geology)?;
    if let Some(procedural) = content.procedural {
        definition = definition.with_moon_fields(procedural)?;
    }
    if let Some(detail) = content.detail_noise {
        definition = definition.with_detail_noise(detail)?;
    }
    let mut world_source = None;
    if let Some(world) = content.world {
        let archetype_path = local_path(root, &world.archetype)?;
        let (archetype, _): (astrum_world::terrain::archetype::PlanetArchetype, _) =
            parse(&archetype_path)?;
        let (look, mut dependencies) = world_look(root, &archetype)?;
        dependencies.extend([terrain_path.clone(), archetype_path]);
        world_source = Some(WorldSource {
            root: root.canonicalize()?,
            terrain: name.to_string(),
            radius_m,
            pole,
            saved: saved.expect("world content has a saved edit"),
            dependencies,
        });
        let world = astrum_world::terrain::world_field::WorldDefinition::new(
            archetype,
            look,
            content.seed,
            &world.overrides,
            pole,
        )
        .context("invalid world map")?;
        definition = definition.with_world(world)?;
    }
    if let Some(profile) = content.profile {
        let asset = local_path(root, &profile.asset)?;
        ensure!(
            fs::metadata(&asset)?.len() <= crate::terrain_profile::MOON_PROFILE_BYTES,
            "profile exceeds bounded input size"
        );
        ensure!(
            (2..=2048).contains(&profile.width) && (2..=2048).contains(&profile.height),
            "profile dimensions outside 2..=2048"
        );
        let bytes = fs::read(&asset)?;
        ensure!(
            format!("{:x}", Sha256::digest(&bytes)) == profile.sha256,
            "profile hash differs from authored content"
        );
        // Feed exactly the verified bytes into world authority; do not reopen a mutable asset.
        ensure!(
            bytes.len() as u64 == u64::from(profile.width) * u64::from(profile.height) * 2,
            "profile size does not match dimensions"
        );
        let canonical = canonical_profile_bytes(&profile.provenance, &profile.byte_order, bytes)?;
        let source = TerrainHeightProfile::from_u16_le(profile.width, profile.height, &canonical)?;
        let source = match profile.kernel.as_str() {
            "bspline" => source.with_cubic_bspline(),
            "smoothstep" => source,
            _ => anyhow::bail!("invalid profile kernel"),
        };
        let mut root_profile = source
            .clone()
            .with_terrain_scale(profile.footprint_m, profile.amplitude_m)?;
        ensure!(profile.detail_layers.len() <= 2, "too many detail layers");
        for layer in profile.detail_layers {
            root_profile = root_profile.with_detail_layer(
                source.clone(),
                layer.footprint_m,
                layer.amplitude_m,
            )?;
        }
        definition = definition.with_height_profile(root_profile)?;
    }
    definition.validate_radius(radius_m)?;
    Ok(LoadedTerrain {
        definition,
        sha256: hash,
        revision: content.revision,
        world: world_source,
    })
}

impl SharedTestSystem {
    /// Canonical content belongs to this compiled checkout, never an unrelated working directory.
    pub fn load_canonical(namespace: NonZeroU64) -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        Self::load(&root, namespace)
    }
    pub fn load(root: &Path, namespace: NonZeroU64) -> Result<Self> {
        let (content, scene_sha256): (SystemContent, _) =
            parse(&root.join("test-solar-system.json"))?;
        ensure!(
            content.schema == 1 && content.id == SCENE_NAME,
            "only the shared test solar system is supported"
        );
        content.lod.validate().context("invalid lod policy")?;
        content
            .collision
            .validate()
            .context("invalid collision policy")?;
        ensure!(
            (3..=32).contains(&content.bodies.len()),
            "test system must have 3..=32 bodies"
        );
        let mut identities = HashSet::new();
        let mut names = HashSet::new();
        let mut system = CelestialSystem::new(namespace, SimulationInstant::ZERO);
        let mut ids = Vec::new();
        let mut presentation = Vec::new();
        for body in &content.bodies {
            ensure!(
                identities.insert(body.identity)
                    && names.insert(body.id.as_str())
                    && !body.id.is_empty(),
                "duplicate or missing persistent body identity"
            );
            ensure!(
                body.color
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                "invalid body color"
            );
            let id = system.insert_body(
                &body.name,
                BodyProperties::new(body.mass_kg, body.radius_m)?,
                BodyState::new(
                    LocalPosition::origin(),
                    LinearVelocity3::zero(),
                    UnitRotation::identity(),
                    AngularVelocity3::zero(),
                ),
            )?;
            let mut style = BodyPresentation {
                identity: body.identity,
                semantic_id: body.id.clone(),
                color: body.color,
                unlit: body.unlit,
                definition_sha256: None,
                definition_revision: None,
                world: None,
            };
            if let Some(terrain) = &body.terrain {
                let loaded =
                    terrain_definition(root, terrain, body.radius_m, body_pole(body)?, None)?;
                system.edit_surface_definition(id, Some(loaded.definition))?;
                style.definition_sha256 = Some(loaded.sha256);
                style.definition_revision = Some(loaded.revision);
                style.world = loaded.world;
            }
            presentation.push(style);
            ids.push(id);
        }
        let mut definitions = Vec::new();
        for (index, body) in content.bodies.iter().enumerate() {
            let translation = match &body.translation {
                TranslationContent::Stationary { position_m } => CelestialTranslation::Stationary(
                    LocalPosition::try_metres(DVec3::from_array(*position_m))?,
                ),
                TranslationContent::Elliptic {
                    reference,
                    semi_major_axis_m,
                    eccentricity,
                    period_s,
                    inclination_radians,
                    phase_radians,
                } => {
                    let parent = content
                        .bodies
                        .iter()
                        .position(|b| &b.id == reference)
                        .context("unknown orbital reference")?;
                    CelestialTranslation::Elliptic(EllipticOrbit::new(
                        ids[parent],
                        *semi_major_axis_m,
                        *eccentricity,
                        UnitRotation::from_axis_angle(
                            Direction3::try_new(DVec3::X)?,
                            *inclination_radians,
                        )?,
                        *period_s,
                        *phase_radians,
                        SimulationInstant::ZERO,
                    )?)
                }
            };
            definitions.push(BodyMotion {
                body: ids[index],
                translation,
                spin: AxialSpin::new(
                    UnitRotation::try_from_quaternion(DQuat::from_array(
                        body.spin_orientation_xyzw,
                    ))?,
                    Direction3::try_new(DVec3::from_array(body.spin_axis))?,
                    body.spin_radians_per_second,
                    SimulationInstant::ZERO,
                )?,
            });
        }
        let motion = CelestialMotionDefinition::new(&system, &definitions)?;
        let initial_body_index = content
            .bodies
            .iter()
            .position(|b| b.id == content.initial_body)
            .context("initial body missing")?;
        let (camera, camera_sha256): (SharedCameraState, _) =
            parse(&local_path(root, &content.camera_state)?)?;
        ensure!(
            camera.schema == 1
                && camera.body == content.initial_body
                && camera.radius_m.to_bits()
                    == content.bodies[initial_body_index].radius_m.to_bits()
                && camera.rate.is_finite(),
            "camera state does not match canonical body"
        );
        LocalPosition::try_metres(DVec3::from_array(camera.position_body_m))?;
        UnitRotation::try_from_quaternion(DQuat::from_array(camera.orientation_xyzw))?;
        ensure!(
            system
                .bodies()
                .filter(|(_, body)| body.has_surface())
                .count()
                >= 2,
            "shared system needs two authored surfaces"
        );
        Ok(Self {
            system,
            lod: content.lod,
            collision: content.collision,
            motion,
            presentation,
            initial_body_index,
            camera,
            scene_sha256,
            camera_sha256,
        })
    }
}
