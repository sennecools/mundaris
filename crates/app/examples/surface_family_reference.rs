//! Fixed-resolution reference package for compositional body-surface families.

use anyhow::{Context, Result, bail};
use glam::{DMat3, DQuat, DVec3};
use mundaris_math::{
    Direction3,
    surface::{CubeFace, SurfaceLocation},
};
use mundaris_world::terrain::{
    ShapeDefinition, SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity,
    TerrainSeed,
};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[path = "surface_family_reference/provinces.rs"]
mod provinces;
#[path = "moon_surface_reference.rs"]
mod reference;

const FAMILY_SEEDS: [u64; 4] = [0, 1, 2, 3];
const BODY_RADII_M: [f64; 4] = [80_000.0, 109_000.0, 180_000.0, 260_000.0];
const DEFAULT_ORBIT_CELLS: usize = 192;
const DEFAULT_LOCAL_CELLS: usize = 256;
const SHAPE_STRESS_SEED: u64 = 0x5348_4150_455f_315b;
const FEATURE_SCAN_SAMPLES: usize = 384;
const LANDMARK_ANCHOR_CANDIDATES: usize = 64;
const LANDMARK_FALLBACK_DIRECTION_CANDIDATES: usize = 1024;

#[derive(Clone, Copy)]
struct Family {
    slug: &'static str,
    algorithm: SurfaceAlgorithm,
    identity: u64,
    palette: [[f32; 3]; 4],
}

const FAMILIES: [Family; 3] = [
    Family {
        slug: "rocky",
        algorithm: SurfaceAlgorithm::RockyV3,
        identity: 0x524f_434b_595f_0001,
        palette: [
            [0.49, 0.43, 0.34],
            [0.66, 0.60, 0.50],
            [0.21, 0.19, 0.17],
            [0.72, 0.54, 0.38],
        ],
    },
    Family {
        slug: "icy",
        algorithm: SurfaceAlgorithm::IcyV1,
        identity: 0x4943_595f_0000_0001,
        palette: [
            [0.65, 0.78, 0.88],
            [0.40, 0.52, 0.63],
            [0.80, 0.87, 0.91],
            [0.43, 0.39, 0.35],
        ],
    },
    Family {
        slug: "volcanic",
        algorithm: SurfaceAlgorithm::VolcanicV1,
        identity: 0x564f_4c43_0000_0001,
        palette: [
            [0.43, 0.18, 0.12],
            [0.29, 0.27, 0.25],
            [0.13, 0.12, 0.12],
            [0.61, 0.38, 0.23],
        ],
    },
];

const DIRECTED_FAMILIES: [Family; 3] = [
    Family {
        algorithm: SurfaceAlgorithm::RockyV4,
        ..FAMILIES[0]
    },
    Family {
        algorithm: SurfaceAlgorithm::IcyV2,
        ..FAMILIES[1]
    },
    Family {
        algorithm: SurfaceAlgorithm::VolcanicV2,
        ..FAMILIES[2]
    },
];

const HIERARCHICAL_FAMILIES: [Family; 3] = [
    Family {
        algorithm: SurfaceAlgorithm::RockyV5,
        ..FAMILIES[0]
    },
    Family {
        algorithm: SurfaceAlgorithm::IcyV3,
        ..FAMILIES[1]
    },
    Family {
        algorithm: SurfaceAlgorithm::VolcanicV3,
        ..FAMILIES[2]
    },
];

fn main() -> Result<()> {
    let options = parse_options(env::args_os().skip(1))?;
    if options.output.exists() {
        bail!(
            "refusing to overwrite existing output directory: {}",
            options.output.display()
        );
    }
    fs::create_dir_all(&options.output)?;

    let families = match options.hierarchy {
        true => HIERARCHICAL_FAMILIES,
        false if options.provinces => DIRECTED_FAMILIES,
        false => FAMILIES,
    };
    let local_cells = if options.hierarchy {
        384
    } else {
        options.local_cells
    };
    let corpus_fixtures = build_fixtures(families)?;
    if options.body.as_ref().is_some_and(|key| {
        !corpus_fixtures[..12]
            .iter()
            .any(|(body, _, _, _)| body == key)
    }) {
        bail!("--body must name one of the twelve family fixtures");
    }
    if options.corpus_only {
        let corpus_wall_ms = write_query_corpus(&options.output, &corpus_fixtures)?;
        eprintln!(
            "wrote query-only family corpus to {} in {:.3} ms",
            options.output.display(),
            corpus_wall_ms
        );
        return Ok(());
    }

    let mut bodies = Vec::new();
    let mut scene_order = Vec::new();
    let mut fixture_index = 0;
    for family in families {
        for (index, seed) in FAMILY_SEEDS.iter().copied().enumerate() {
            let (body_key, definition, radius_m, metadata) = &corpus_fixtures[fixture_index];
            fixture_index += 1;
            if options
                .body
                .as_ref()
                .is_some_and(|selected| selected != body_key)
            {
                continue;
            }
            let generator = SurfaceGenerator::new(definition, *radius_m)?;
            let mut scenes = Vec::new();
            if index == 0 && options.hierarchy {
                provinces::capture_provinces(
                    &options.output,
                    body_key,
                    &generator,
                    family,
                    metadata,
                    local_cells,
                    &mut scenes,
                )?;
                provinces::capture_unbiased_fine_views(
                    &options.output,
                    body_key,
                    &generator,
                    family,
                    metadata,
                    local_cells,
                    &mut scenes,
                )?;
            }
            for (view, cells, neutral) in [
                ("orbit", options.orbit_cells, false),
                ("regional", local_cells, false),
                ("near", local_cells, false),
            ] {
                let scene_metadata = reference::capture_family_scene(
                    &options.output,
                    body_key,
                    view,
                    cells,
                    &generator,
                    reference::FamilyCaptureStyle {
                        palette: family.palette,
                        neutral_broad_light: neutral,
                    },
                    metadata.clone(),
                )?;
                scenes.push(json!({"view":view,"path":format!("{body_key}/{view}"),"metadata":scene_metadata}));
                scene_order.push(json!({"body_key":body_key,"view":view,"path":format!("{body_key}/{view}/lit.png")}));
            }
            if index == 0 && !options.provinces && !options.hierarchy {
                let view = "orbit-neutral";
                let scene_metadata = reference::capture_family_scene(
                    &options.output,
                    body_key,
                    view,
                    options.orbit_cells,
                    &generator,
                    reference::FamilyCaptureStyle {
                        palette: family.palette,
                        neutral_broad_light: true,
                    },
                    metadata.clone(),
                )?;
                scenes.push(json!({"view":view,"path":format!("{body_key}/{view}"),"metadata":scene_metadata}));
            }
            if index == 0 && options.provinces {
                provinces::capture_provinces(
                    &options.output,
                    body_key,
                    &generator,
                    family,
                    metadata,
                    local_cells,
                    &mut scenes,
                )?;
            }
            if index == 0 && !options.provinces && !options.hierarchy {
                let selection = select_landmark(&generator)?;
                let chart_to_body = chart_to_body_rotation(selection.direction_body_axes);
                let rotated =
                    reference::surface_adapter::RotatedSurfaceQuery::new(&generator, chart_to_body);
                let chart_to_body_axes = rotated.chart_to_body_axes();
                let landmark_selection = json!({
                    "selection_policy":selection.selection_policy,
                    "candidate_distribution":selection.candidate_distribution,
                    "anchor_candidate_limit":LANDMARK_ANCHOR_CANDIDATES,
                    "anchor_probe_query_count":selection.anchor_probe_query_count,
                    "selected_anchor_index":selection.anchor_index,
                    "selected_anchor_direction_body_axes":selection.anchor_direction_body_axes.map(|direction| direction.to_array()),
                    "feature_probes_returned":selection.feature_probe_count,
                    "selected_feature_probe_label":selection.feature_probe_label,
                    "feature_probe_gradient_query_count":selection.feature_gradient_query_count,
                    "fallback_used":selection.fallback_used,
                    "fallback_direction_candidate_count":selection.fallback_direction_candidate_count,
                    "selected_fallback_candidate_index":selection.fallback_candidate_index,
                    "selected_candidate_index":selection.candidate_index,
                    "selected_crop_direction_body_axes":rotated.body_center_direction().to_array(),
                    "selected_terrain_gradient_norm_m_per_unit_direction":selection.gradient_norm_m_per_unit_direction,
                    "selection_oracle_query_count":selection.oracle_query_count,
                    "selection_query_wall_ms":selection.query_wall_ms,
                    "selection_reused_for_views":["regional-landmark","near-landmark"],
                    "chart_to_body_matrix_columns_body_axes":chart_to_body_axes.map(|axis| axis.to_array())
                });
                let mut landmark_fixture_metadata = metadata.clone();
                landmark_fixture_metadata
                    .as_object_mut()
                    .expect("fixture metadata is an object")
                    .insert("landmark_selection".to_owned(), landmark_selection);
                for (view, cells) in [
                    ("regional-landmark", local_cells),
                    ("near-landmark", local_cells),
                ] {
                    let scene_metadata = reference::capture_family_scene(
                        &options.output,
                        body_key,
                        view,
                        cells,
                        &rotated,
                        reference::FamilyCaptureStyle {
                            palette: family.palette,
                            neutral_broad_light: false,
                        },
                        landmark_fixture_metadata.clone(),
                    )?;
                    scenes.push(json!({"view":view,"path":format!("{body_key}/{view}"),"metadata":scene_metadata}));
                }
            }
            bodies.push(json!({"key":body_key,"family":family.slug,"seed":seed,"radius_m":radius_m,"definition":metadata,"director_statistics":provinces::summary(&generator)?,"scenes":scenes}));
        }
    }

    let (stress_key, irregular, irregular_radius_m, stress_metadata) = corpus_fixtures
        .last()
        .context("missing irregular stress fixture")?;
    let irregular_generator = SurfaceGenerator::new(irregular, *irregular_radius_m)?;
    let rocky_palette = FAMILIES[0].palette;
    let mut stress_scenes = Vec::new();
    for (view, cells) in [
        ("orbit", options.orbit_cells),
        ("regional", local_cells),
        ("near", local_cells),
        ("orbit-neutral", options.orbit_cells),
    ] {
        if options.body.is_some() {
            break;
        }
        let neutral = view == "orbit-neutral";
        let scene_metadata = reference::capture_family_scene(
            &options.output,
            stress_key,
            view,
            cells,
            &irregular_generator,
            reference::FamilyCaptureStyle {
                palette: rocky_palette,
                neutral_broad_light: neutral,
            },
            stress_metadata.clone(),
        )?;
        stress_scenes.push(
            json!({"view":view,"path":format!("{stress_key}/{view}"),"metadata":scene_metadata}),
        );
    }
    write_query_corpus(&options.output, &corpus_fixtures)?;
    let mut diagnostic_outputs = vec![
        "lit",
        "geometry",
        "unshadowed_lit",
        "lighting",
        "shadow_factor",
        "height",
        "shape",
        "normal",
        "analytic_normal",
        "material",
        "raw_material",
        "material_4",
        "raw_material_4",
    ];
    if options.hierarchy {
        diagnostic_outputs.extend([
            "detail-parent-morphology",
            "detail-inherited-height",
            "detail-regional-height",
            "detail-local-height",
            "detail-fine-height",
            "detail-parent-process-0..3",
        ]);
    }
    let mut manifest = json!({
        "schema_version":1,
        "title":"Compositional surface family reference package",
        "capture_stage":"software reference package; user visual acceptance remains open",
        "renderer":"temporary software f64 reference rasterizer",
        "body_seed_selection":"fixed consecutive seeds 0, 1, 2, 3 for each family; no seed rejected or selected by rendered appearance",
        "orbit_cells":options.orbit_cells,
        "local_cells":options.local_cells,
        "geological_director_enabled":options.provinces || options.hierarchy,
        "body_filter":options.body,
        "diagnostics":diagnostic_outputs,
        "bodies":bodies,
        "irregular_shape_stress":{"key":stress_key,"scenes":stress_scenes},
        "orbital_contact_order":scene_order,
        "surface_query_corpus":"surface_queries.json",
        "representation_boundary":"radial graph is star-shaped about the body origin; this package does not claim arbitrary concave geometry, caves, overhangs or contact binaries"
    });
    if options.hierarchy {
        manifest["hierarchy_local_cells"] = json!(384);
        manifest["hierarchical_detail_enabled"] = json!(true);
        manifest["hierarchy_anchor_policy"] = json!(
            "select a nonzero parent-morphology rim/shoulder/front process probe per province; reuse its exact body-fixed direction at all five scales; record nearby fine probes as linked context"
        );
        manifest["detail_map_policy"] = json!(
            "256x256 authoritative detail maps are emitted only for 256 m, 32 m and 8 m province crops; each channel maps its observed local min/max independently; aggregate per-band query work and sampling time are recorded"
        );
    }
    fs::write(
        options.output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    write_html_index(&options.output, &bodies, stress_key)?;
    eprintln!(
        "wrote family reference package to {}",
        options.output.display()
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LandmarkSelection {
    direction_body_axes: DVec3,
    selection_policy: &'static str,
    candidate_distribution: &'static str,
    anchor_index: Option<usize>,
    anchor_direction_body_axes: Option<DVec3>,
    anchor_probe_query_count: usize,
    feature_probe_count: usize,
    feature_probe_label: Option<&'static str>,
    feature_gradient_query_count: usize,
    fallback_used: bool,
    fallback_direction_candidate_count: usize,
    fallback_candidate_index: Option<usize>,
    candidate_index: Option<usize>,
    oracle_query_count: usize,
    gradient_norm_m_per_unit_direction: f64,
    query_wall_ms: f64,
}

fn select_landmark(generator: &SurfaceGenerator) -> Result<LandmarkSelection> {
    let query_start = Instant::now();
    let mut anchor_probe_query_count = 0;
    for anchor_index in 0..LANDMARK_ANCHOR_CANDIDATES {
        let anchor_direction = fibonacci_direction(anchor_index, LANDMARK_ANCHOR_CANDIDATES);
        let anchor = SurfaceLocation::new(Direction3::try_new(anchor_direction)?);
        let probes = generator.diagnostic_landmark_probes(anchor)?;
        anchor_probe_query_count += 1;
        if probes.is_empty() {
            continue;
        }

        let mut best: Option<(usize, &mundaris_world::terrain::SurfaceBoundaryProbe, f64)> = None;
        for (probe_index, probe) in probes.iter().enumerate() {
            let sample = generator.evaluate_point(probe.location)?;
            let gradient_norm_m_per_unit_direction = sample
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .length();
            if best.as_ref().is_none_or(|(_, _, best_gradient)| {
                gradient_norm_m_per_unit_direction > *best_gradient
            }) {
                best = Some((probe_index, probe, gradient_norm_m_per_unit_direction));
            }
        }
        let (probe_index, probe, gradient_norm_m_per_unit_direction) =
            best.context("world landmark probe set was unexpectedly empty")?;
        let direction_body_axes = probe.location.direction().unit();
        return Ok(LandmarkSelection {
            direction_body_axes,
            selection_policy: "scan fixed Fibonacci anchors in order; stop at the first non-empty world feature-probe set and choose its first maximum terrain-gradient probe",
            candidate_distribution: "64 Fibonacci-sphere anchors with golden-angle azimuth and midpoint-spaced Y coordinate",
            anchor_index: Some(anchor_index),
            anchor_direction_body_axes: Some(anchor_direction),
            anchor_probe_query_count,
            feature_probe_count: probes.len(),
            feature_probe_label: Some(probe.label),
            feature_gradient_query_count: probes.len(),
            fallback_used: false,
            fallback_direction_candidate_count: 0,
            fallback_candidate_index: None,
            candidate_index: Some(probe_index),
            oracle_query_count: anchor_probe_query_count + probes.len(),
            gradient_norm_m_per_unit_direction,
            query_wall_ms: query_start.elapsed().as_secs_f64() * 1000.0,
        });
    }

    let mut best: Option<(usize, DVec3, f64)> = None;
    for candidate_index in 0..LANDMARK_FALLBACK_DIRECTION_CANDIDATES {
        let direction =
            fibonacci_direction(candidate_index, LANDMARK_FALLBACK_DIRECTION_CANDIDATES);
        let location = SurfaceLocation::new(Direction3::try_new(direction)?);
        let sample = generator.evaluate_point(location)?;
        let gradient_norm_m_per_unit_direction = sample
            .terrain()
            .tangent_gradient_m_per_unit_direction()
            .length();
        if best
            .as_ref()
            .is_none_or(|(_, _, best_gradient)| gradient_norm_m_per_unit_direction > *best_gradient)
        {
            best = Some((
                candidate_index,
                direction,
                gradient_norm_m_per_unit_direction,
            ));
        }
    }
    let (candidate_index, direction_body_axes, gradient_norm_m_per_unit_direction) =
        best.context("fallback direction scan returned no candidates")?;
    Ok(LandmarkSelection {
        direction_body_axes,
        selection_policy: "scan 64 fixed Fibonacci anchors for world feature probes; because all are empty, use the first maximum-gradient point from 1024 fixed Fibonacci directions",
        candidate_distribution: "64 Fibonacci anchors followed by 1024 Fibonacci fallback directions, both with golden-angle azimuth and midpoint-spaced Y coordinate",
        anchor_index: None,
        anchor_direction_body_axes: None,
        anchor_probe_query_count,
        feature_probe_count: 0,
        feature_probe_label: None,
        feature_gradient_query_count: 0,
        fallback_used: true,
        fallback_direction_candidate_count: LANDMARK_FALLBACK_DIRECTION_CANDIDATES,
        fallback_candidate_index: Some(candidate_index),
        candidate_index: None,
        oracle_query_count: anchor_probe_query_count + LANDMARK_FALLBACK_DIRECTION_CANDIDATES,
        gradient_norm_m_per_unit_direction,
        query_wall_ms: query_start.elapsed().as_secs_f64() * 1000.0,
    })
}

fn fibonacci_direction(index: usize, count: usize) -> DVec3 {
    let midpoint = index as f64 + 0.5;
    let y = 1.0 - 2.0 * midpoint / count as f64;
    let radial = (1.0 - y * y).sqrt();
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let azimuth = index as f64 * golden_angle;
    DVec3::new(radial * azimuth.cos(), y, radial * azimuth.sin())
}

fn chart_to_body_rotation(center_body_axes: DVec3) -> DQuat {
    let center = center_body_axes.normalize();
    let reference = if center.z.abs() < 0.9 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let east = center.cross(reference).normalize();
    let north = center.cross(east).normalize();
    DQuat::from_mat3(&DMat3::from_cols(east, north, center))
}

struct Options {
    output: PathBuf,
    orbit_cells: usize,
    local_cells: usize,
    corpus_only: bool,
    provinces: bool,
    hierarchy: bool,
    body: Option<String>,
}

fn parse_options(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Options> {
    let mut args = args.peekable();
    let output = args.next().map(PathBuf::from).context(
        "usage: surface_family_reference <new-output-directory> [--orbit-cells N] [--local-cells N] [--provinces | --hierarchy] [--body KEY] [--corpus-only]",
    )?;
    let (mut orbit_cells, mut local_cells) = (DEFAULT_ORBIT_CELLS, DEFAULT_LOCAL_CELLS);
    let mut corpus_only = false;
    let mut provinces = false;
    let mut hierarchy = false;
    let mut body = None;
    let mut local_cells_explicit = false;
    while let Some(flag) = args.next() {
        if flag == "--corpus-only" {
            corpus_only = true;
            continue;
        }
        if flag == "--provinces" {
            provinces = true;
            continue;
        }
        if flag == "--hierarchy" {
            hierarchy = true;
            continue;
        }
        let value = args
            .next()
            .with_context(|| format!("missing value for {}", flag.to_string_lossy()))?;
        if flag == "--body" {
            body = Some(value.to_string_lossy().into_owned());
            continue;
        }
        let count: usize = value
            .to_string_lossy()
            .parse()
            .context("invalid cell count")?;
        match flag.to_string_lossy().as_ref() {
            "--orbit-cells" if (64..=768).contains(&count) => orbit_cells = count,
            "--local-cells" if (64..=512).contains(&count) => {
                local_cells = count;
                local_cells_explicit = true;
            }
            "--orbit-cells" => bail!("--orbit-cells must be in 64..=768"),
            "--local-cells" => bail!("--local-cells must be in 64..=512"),
            _ => bail!("unknown option {}", flag.to_string_lossy()),
        }
    }
    if provinces && hierarchy {
        bail!(
            "--provinces and --hierarchy select different terrain versions and cannot be combined"
        );
    }
    if hierarchy && local_cells_explicit && local_cells != 384 {
        bail!("--hierarchy requires the fixed 384 local mesh cells");
    }
    Ok(Options {
        output,
        orbit_cells,
        local_cells,
        corpus_only,
        provinces,
        hierarchy,
        body,
    })
}

#[cfg(test)]
fn build_query_corpus_fixtures() -> Result<Vec<(String, SurfaceDefinition, f64, Value)>> {
    build_fixtures(FAMILIES)
}

fn build_fixtures(families: [Family; 3]) -> Result<Vec<(String, SurfaceDefinition, f64, Value)>> {
    let mut fixtures = Vec::with_capacity(FAMILIES.len() * FAMILY_SEEDS.len() + 1);
    for family in families {
        for (index, seed) in FAMILY_SEEDS.iter().copied().enumerate() {
            let radius_m = BODY_RADII_M[index];
            let definition = make_definition(family, seed, radius_m, ShapeDefinition::sphere())?;
            let body_key = format!("{}-seed-{seed}", family.slug);
            let metadata = fixture_metadata(&definition, family, seed, radius_m, false);
            fixtures.push((body_key, definition, radius_m, metadata));
        }
    }
    let irregular_radius_m = 109_000.0;
    let shape = ShapeDefinition::irregular([1.35, 0.92, 0.70], 0.16, 0.12, SHAPE_STRESS_SEED)?;
    let irregular = make_definition(families[0], SHAPE_STRESS_SEED, irregular_radius_m, shape)?;
    let metadata = fixture_metadata(
        &irregular,
        families[0],
        SHAPE_STRESS_SEED,
        irregular_radius_m,
        true,
    );
    fixtures.push((
        "irregular-shape-stress".to_owned(),
        irregular,
        irregular_radius_m,
        metadata,
    ));
    Ok(fixtures)
}

fn write_query_corpus(
    output: &Path,
    fixtures: &[(String, SurfaceDefinition, f64, Value)],
) -> Result<f64> {
    let corpus_start = Instant::now();
    let mut corpus = query_corpus(fixtures)?;
    let hierarchy_enabled = fixtures.iter().any(|(_, definition, _, _)| {
        matches!(
            definition.terrain().algorithm(),
            SurfaceAlgorithm::RockyV5 | SurfaceAlgorithm::IcyV3 | SurfaceAlgorithm::VolcanicV3
        )
    });
    if hierarchy_enabled {
        corpus["hierarchy_diagnostics"] = json!({
            "fields":"inherited/regional/local/fine height and gradient, three band work records, four parent process strengths",
            "record_policy":"attached to every complete query for hierarchy fixtures across all radii and seeds",
            "deterministic_fields_exclude_wall_time":true
        });
    }
    let corpus_wall_ms = corpus_start.elapsed().as_secs_f64() * 1000.0;
    fs::write(
        output.join("surface_queries.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version":1,
            "purpose":"complete world-owned compositional surface oracle for later representation comparisons",
            "query_mode":"complete unfiltered point query",
            "probe_policy":"all canonical face edges/corners and face interiors; actual world family boundary/support and feature-local landmark probes; deterministic 384-direction scan also records sampled shape and relief extrema",
            "wall_ms":corpus_wall_ms,
            "corpus":corpus
        }))?,
    )?;
    Ok(corpus_wall_ms)
}

fn make_definition(
    family: Family,
    seed: u64,
    _radius_m: f64,
    shape: ShapeDefinition,
) -> Result<SurfaceDefinition> {
    let identity = TerrainIdentity(family.identity ^ seed.rotate_left(17));
    let base = SurfaceDefinition::generated(identity, TerrainSeed(seed), family.algorithm);
    if shape == ShapeDefinition::sphere() {
        return Ok(base);
    }
    SurfaceDefinition::new(
        identity,
        TerrainSeed(seed),
        shape,
        base.terrain(),
        base.material(),
        base.atmosphere(),
    )
    .map_err(Into::into)
}

#[cfg(test)]
fn irregular_definition() -> Result<SurfaceDefinition> {
    let shape = ShapeDefinition::irregular([1.35, 0.92, 0.70], 0.16, 0.12, SHAPE_STRESS_SEED)?;
    make_definition(FAMILIES[0], SHAPE_STRESS_SEED, 109_000.0, shape)
}

fn fixture_metadata(
    definition: &SurfaceDefinition,
    family: Family,
    seed: u64,
    radius_m: f64,
    stress: bool,
) -> Value {
    let parameters = definition.terrain().parameters();
    let material_version = definition.material().version();
    let mut metadata = json!({
        "fixture_kind":if stress {"irregular_shape_stress_test"} else {"family_body"},
        "family":family.slug,
        "body_identity":definition.identity().0,
        "seed":seed,
        "radius_m":radius_m,
        "shape":shape_metadata(definition.shape()),
        "shape_identity":definition.shape().configuration_identity(),
        "terrain_algorithm":definition.terrain().algorithm().name(),
        "terrain_identity":definition.terrain_identity(),
        "terrain_configuration_identity":definition.terrain().configuration_identity(),
        "geological_phenotype":{
            "age":parameters.age,"activity":parameters.activity,
            "resurfacing_fraction":parameters.resurfacing_fraction,
            "impact_retention":parameters.impact_retention,
            "relief_fraction":parameters.relief_fraction,
            "feature_scale_fraction":parameters.feature_scale_fraction,
            "orientation_radians":parameters.orientation_radians
        },
        "material_version":format!("{material_version:?}"),
        "material_channels":material_version.channels(),
        "material_composition":definition.material().composition(),
        "material_regional_contrast":definition.material().regional_contrast(),
        "material_identity":definition.material_identity(),
        "atmosphere":{"state":"airless_descriptor_only"},
        "surface_configuration_identity":definition.configuration_identity(),
        "radial_representation":"one positive radius per body-local direction"
    });
    if matches!(
        family.algorithm,
        SurfaceAlgorithm::RockyV4
            | SurfaceAlgorithm::IcyV2
            | SurfaceAlgorithm::VolcanicV2
            | SurfaceAlgorithm::RockyV5
            | SurfaceAlgorithm::IcyV3
            | SurfaceAlgorithm::VolcanicV3
    ) {
        metadata["province_names"] = json!(family.algorithm.province_names());
        metadata["process_names"] = json!(family.algorithm.process_names());
    }
    metadata
}

fn shape_metadata(shape: &ShapeDefinition) -> Value {
    json!({"algorithm":shape.algorithm_name(),"axes_fractions":shape.axes_fractions(),
        "irregular_amplitudes":shape.irregular_amplitudes(),"orientation_seed":shape.seed(),
        "configuration_identity":shape.configuration_identity()})
}

fn query_corpus(fixtures: &[(String, SurfaceDefinition, f64, Value)]) -> Result<Value> {
    let mut records = Vec::new();
    let mut boundary_probe_audit = Vec::new();
    let mut landmark_probe_audit = Vec::new();
    let fixture_table = fixtures.iter().map(|(key, definition, radius, metadata)| json!({
        "body_key":key,"reference_radius_m":radius,"seed":definition.seed().0,
        "identity":definition.identity().0,"surface_configuration_identity":definition.configuration_identity(),
        "shape":metadata["shape"],"terrain_algorithm":metadata["terrain_algorithm"],
        "terrain_identity":metadata["terrain_identity"],"terrain_configuration_identity":metadata["terrain_configuration_identity"],
        "geological_phenotype":metadata["geological_phenotype"],"material_version":metadata["material_version"],
        "material_channels":metadata["material_channels"],"material_composition":metadata["material_composition"],
        "material_regional_contrast":metadata["material_regional_contrast"],"material_identity":metadata["material_identity"],
        "atmosphere":metadata["atmosphere"]
    })).collect::<Vec<_>>();
    let mut directions = Vec::new();
    for face in CubeFace::ALL {
        for [u, v] in [
            [-1.0_f64, -1.0],
            [-1.0, 0.0],
            [-1.0, 1.0],
            [0.0, -1.0],
            [0.0, 0.0],
            [0.0, 1.0],
            [1.0, -1.0],
            [1.0, 0.0],
            [1.0, 1.0],
        ] {
            let label = if u.abs() == 1.0 && v.abs() == 1.0 {
                "canonical_cube_corner"
            } else if u.abs() == 1.0 || v.abs() == 1.0 {
                "canonical_cube_edge"
            } else {
                "face_interior"
            };
            directions.push((format!("{label}_{face:?}_{u}_{v}"), face.direction([u, v])?));
        }
    }
    for (name, direction) in [
        ("axis_x", DVec3::X),
        ("axis_y", DVec3::Y),
        ("axis_z", DVec3::Z),
        ("oblique_a", DVec3::new(1.0, 2.0, 3.0)),
        ("oblique_b", DVec3::new(-2.0, 1.0, 0.5)),
    ] {
        directions.push((name.to_owned(), Direction3::try_new(direction)?));
    }

    for (body_key, definition, radius_m, fixture_metadata) in fixtures {
        for query_radius_m in [80_000.0, *radius_m, 1_200_000.0] {
            let generator = SurfaceGenerator::new(definition, query_radius_m)?;
            for (label, direction) in &directions {
                let location = SurfaceLocation::new(*direction);
                let sample = generator.evaluate_point(location)?;
                records.push(sample_record(
                    body_key,
                    *radius_m,
                    query_radius_m,
                    label,
                    location,
                    sample,
                    fixture_metadata,
                ));
            }
            if query_radius_m == *radius_m {
                let mut attempts = 0;
                let mut selected_anchor = None;
                let mut selected_probes = Vec::new();
                for anchor_index in 0..LANDMARK_ANCHOR_CANDIDATES {
                    attempts += 1;
                    let anchor_direction =
                        fibonacci_direction(anchor_index, LANDMARK_ANCHOR_CANDIDATES);
                    let anchor = SurfaceLocation::new(Direction3::try_new(anchor_direction)?);
                    let probes = generator.diagnostic_landmark_probes(anchor)?;
                    if !probes.is_empty() {
                        selected_anchor = Some((anchor_index, anchor_direction.to_array()));
                        selected_probes = probes;
                        break;
                    }
                }
                let probe_count = selected_probes.len();
                for probe in selected_probes {
                    let sample = generator.evaluate_point(probe.location)?;
                    records.push(sample_record(
                        body_key,
                        *radius_m,
                        query_radius_m,
                        probe.label,
                        probe.location,
                        sample,
                        fixture_metadata,
                    ));
                }
                landmark_probe_audit.push(json!({
                    "body_key":body_key,
                    "query_radius_m":query_radius_m,
                    "anchor_policy":"first non-empty world landmark probe set across fixed 64-point Fibonacci anchors",
                    "anchor_candidate_limit":LANDMARK_ANCHOR_CANDIDATES,
                    "anchors_tested":attempts,
                    "selected_anchor_index":selected_anchor.map(|selected| selected.0),
                    "selected_anchor_direction_body_axes":selected_anchor.map(|selected| selected.1),
                    "world_landmark_probe_count":probe_count,
                    "found":probe_count>0
                }));

                let anchors = [
                    DVec3::Z,
                    DVec3::X,
                    DVec3::Y,
                    DVec3::new(1.0, 2.0, 3.0),
                    DVec3::new(-2.0, 1.0, 0.5),
                    -DVec3::Z,
                    -DVec3::X,
                    -DVec3::Y,
                ];
                let mut attempts = 0;
                let mut count = 0;
                let mut selected_anchor = None;
                for anchor in anchors {
                    attempts += 1;
                    let near = SurfaceLocation::new(Direction3::try_new(anchor)?);
                    let probes = generator.diagnostic_boundary_probes(near)?;
                    if !probes.is_empty() {
                        selected_anchor = Some(near.direction().unit().to_array());
                        count = probes.len();
                        for probe in probes {
                            let sample = generator.evaluate_point(probe.location)?;
                            records.push(sample_record(
                                body_key,
                                *radius_m,
                                query_radius_m,
                                probe.label,
                                probe.location,
                                sample,
                                fixture_metadata,
                            ));
                        }
                        break;
                    }
                }
                boundary_probe_audit.push(
                    json!({"body_key":body_key,"query_radius_m":query_radius_m,
                    "nearpoint_attempts":attempts,"selected_anchor_direction":selected_anchor,
                    "world_boundary_probe_count":count,"found":count>0}),
                );
            }
            let mut shape_min: Option<(
                String,
                SurfaceLocation,
                mundaris_world::terrain::SurfaceSample,
            )> = None;
            let mut shape_max = None;
            let mut relief_min = None;
            let mut relief_max = None;
            for index in 0..FEATURE_SCAN_SAMPLES {
                let direction = fibonacci_direction(index, FEATURE_SCAN_SAMPLES);
                let location = SurfaceLocation::new(Direction3::try_new(direction)?);
                let sample = generator.evaluate_point(location)?;
                keep_extreme(&mut shape_min, "sampled_shape_min", location, sample, |s| {
                    s.shape().radius_m()
                });
                keep_extreme(&mut shape_max, "sampled_shape_max", location, sample, |s| {
                    s.shape().radius_m()
                });
                keep_extreme(
                    &mut relief_min,
                    "sampled_relief_min",
                    location,
                    sample,
                    |s| s.terrain().height_m(),
                );
                keep_extreme(
                    &mut relief_max,
                    "sampled_relief_max",
                    location,
                    sample,
                    |s| s.terrain().height_m(),
                );
            }
            for candidate in [shape_min, shape_max, relief_min, relief_max]
                .into_iter()
                .flatten()
            {
                records.push(sample_record(
                    body_key,
                    *radius_m,
                    query_radius_m,
                    &candidate.0,
                    candidate.1,
                    candidate.2,
                    fixture_metadata,
                ));
            }
        }
    }
    provinces::extend_corpus(fixtures, &mut records)?;
    for (body_key, definition, radius_m, _) in fixtures.iter().filter(|(_, definition, _, _)| {
        matches!(
            definition.terrain().algorithm(),
            SurfaceAlgorithm::RockyV5 | SurfaceAlgorithm::IcyV3 | SurfaceAlgorithm::VolcanicV3
        )
    }) {
        for query_radius_m in [80_000.0, *radius_m, 1_200_000.0] {
            let generator = SurfaceGenerator::new(definition, query_radius_m)?;
            for record in records.iter_mut().filter(|record| {
                record["body_key"] == *body_key && record["query_radius_m"] == query_radius_m
            }) {
                let values = record["direction_body_axes"]
                    .as_array()
                    .context("hierarchy corpus direction array")?;
                let direction = DVec3::new(
                    values[0].as_f64().context("hierarchy x")?,
                    values[1].as_f64().context("hierarchy y")?,
                    values[2].as_f64().context("hierarchy z")?,
                );
                let location = SurfaceLocation::new(Direction3::try_new(direction)?);
                if let Some(diagnostics) = generator.detail_diagnostics(location)? {
                    record["detail_diagnostics"] = provinces::detail_json(diagnostics);
                }
            }
        }
    }
    Ok(
        json!({"fixtures":fixture_table,"family_boundary_probe_audit":boundary_probe_audit,"family_landmark_probe_audit":landmark_probe_audit,"queries":records}),
    )
}

fn keep_extreme(
    target: &mut Option<(
        String,
        SurfaceLocation,
        mundaris_world::terrain::SurfaceSample,
    )>,
    label: &str,
    location: SurfaceLocation,
    sample: mundaris_world::terrain::SurfaceSample,
    value: impl Fn(mundaris_world::terrain::SurfaceSample) -> f64,
) {
    let replace = target.as_ref().is_none_or(|(_, _, old)| {
        if label.ends_with("min") {
            value(sample) < value(*old)
        } else {
            value(sample) > value(*old)
        }
    });
    if replace {
        *target = Some((label.to_owned(), location, sample));
    }
}

fn sample_record(
    body_key: &str,
    reference_radius_m: f64,
    query_radius_m: f64,
    label: &str,
    location: SurfaceLocation,
    sample: mundaris_world::terrain::SurfaceSample,
    fixture_metadata: &Value,
) -> Value {
    json!({"body_key":body_key,"reference_radius_m":reference_radius_m,"query_radius_m":query_radius_m,
        "label":label,"direction_body_axes":location.direction().unit().to_array(),
        "shape_radius_m":sample.shape().radius_m(),"shape_gradient_m_per_unit_direction":sample.shape().gradient_m().to_array(),
        "terrain_height_m":sample.terrain().height_m(),"terrain_gradient_m_per_unit_direction":sample.terrain().tangent_gradient_m_per_unit_direction().to_array(),
        "combined_radius_m":sample.radius_m(),"combined_normal_body_axes":sample.normal().to_array(),
        "shape_identity":fixture_metadata["shape"]["configuration_identity"],
        "terrain_algorithm":fixture_metadata["terrain_algorithm"],
        "terrain_identity":fixture_metadata["terrain_identity"],
        "terrain_configuration_identity":fixture_metadata["terrain_configuration_identity"],
        "material_version":fixture_metadata["material_version"],
        "material_identity":fixture_metadata["material_identity"],
        "material_composition":fixture_metadata["material_composition"],
        "material_regional_contrast":fixture_metadata["material_regional_contrast"],
        "atmosphere":fixture_metadata["atmosphere"],
        "material_weights":sample.material_weights(),"work":{"cells_visited":sample.work().cells_visited,
            "candidate_features":sample.work().candidate_features,
            "accepted_features":if definition_algorithm_is_rocky(fixture_metadata) { Value::Null } else { json!(sample.work().accepted_features) }}})
}

fn definition_algorithm_is_rocky(metadata: &Value) -> bool {
    metadata["terrain_algorithm"] == "RockyV3"
}

fn write_html_index(output: &Path, _bodies: &[Value], stress_key: &str) -> Result<()> {
    let html = format!(
        r#"<!doctype html><meta charset="utf-8"><title>Surface family reference</title>
<style>body{{font:15px system-ui;background:#17191c;color:#eee;margin:24px}}a{{color:#b8d5ff}}img{{width:min(90vw,1100px);display:block;margin:12px 0}}.grid{{display:grid;grid-template-columns:repeat(3,1fr);gap:12px}}</style>
<h1>Compositional surface family reference</h1><p>Software reference captures for visual review. Neutral orbital views use grayscale material and broad ambient fill.</p>
<h2>Contact sheets</h2><p>Run <code>scripts/surface-family-sheets.py</code> on this package to build labelled and unlabelled orbital sheets.</p>
<h2>Irregular shape stress test</h2><div class="grid"><a href="{stress_key}/orbit/lit.png"><img src="{stress_key}/orbit/lit.png">orbit</a><a href="{stress_key}/regional/shape.png"><img src="{stress_key}/regional/shape.png">regional shape</a><a href="{stress_key}/near/analytic_normal.png"><img src="{stress_key}/near/analytic_normal.png">near normal</a></div>
<p>Every scene directory contains lit, unshadowed-lit, lighting, shadow, height, shape, normal, analytic-normal and four material channel diagnostics.</p>"#
    );
    fs::write(output.join("index.html"), html)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landmark_scan_is_fixed_deterministic_and_uses_rotated_world_queries() {
        let definition =
            make_definition(FAMILIES[1], 0, 80_000.0, ShapeDefinition::sphere()).unwrap();
        let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
        let first = select_landmark(&generator).unwrap();
        let repeated = select_landmark(&generator).unwrap();
        assert_eq!(first.candidate_index, repeated.candidate_index);
        assert_eq!(first.feature_probe_label, repeated.feature_probe_label);
        assert_eq!(first.direction_body_axes, repeated.direction_body_axes);
        assert_eq!(
            first.gradient_norm_m_per_unit_direction,
            repeated.gradient_norm_m_per_unit_direction
        );
        assert!(!first.fallback_used);
        assert_eq!(
            first.anchor_probe_query_count,
            first.anchor_index.unwrap() + 1
        );
        assert!(first.feature_probe_count > 0);
        assert!(first.candidate_index.unwrap() < first.feature_probe_count);
        assert!(first.gradient_norm_m_per_unit_direction.is_finite());

        let chart_to_body = chart_to_body_rotation(first.direction_body_axes);
        assert!((chart_to_body * DVec3::Z).distance(first.direction_body_axes) < 1e-12);
        let rotated =
            reference::surface_adapter::RotatedSurfaceQuery::new(&generator, chart_to_body);
        let chart_location = SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap());
        let body_location =
            SurfaceLocation::new(Direction3::try_new(first.direction_body_axes).unwrap());
        let chart_sample =
            reference::surface_adapter::SurfaceQuery::evaluate_point(&rotated, chart_location)
                .unwrap();
        let body_sample = generator.evaluate_point(body_location).unwrap();
        assert!((chart_sample.radius_m - body_sample.radius_m()).abs() < 1e-8);
        assert!((chart_sample.terrain_height_m - body_sample.terrain().height_m()).abs() < 1e-8);
        assert!(
            chart_sample
                .normal_body
                .distance(chart_to_body.conjugate() * body_sample.normal())
                < 1e-9
        );
    }

    #[test]
    fn rocky_landmark_selection_records_bounded_gradient_fallback() {
        let definition =
            make_definition(FAMILIES[0], 0, 80_000.0, ShapeDefinition::sphere()).unwrap();
        let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
        let selection = select_landmark(&generator).unwrap();
        assert!(selection.fallback_used);
        assert_eq!(
            selection.anchor_probe_query_count,
            LANDMARK_ANCHOR_CANDIDATES
        );
        assert_eq!(
            selection.fallback_direction_candidate_count,
            LANDMARK_FALLBACK_DIRECTION_CANDIDATES
        );
        assert!(
            selection.fallback_candidate_index.unwrap() < LANDMARK_FALLBACK_DIRECTION_CANDIDATES
        );
    }

    #[test]
    fn family_cli_accepts_reference_densities_and_rejects_unknown_options() {
        let options = parse_options(
            ["capture", "--orbit-cells", "768", "--local-cells", "512"]
                .into_iter()
                .map(std::ffi::OsString::from),
        )
        .unwrap();
        assert_eq!(options.orbit_cells, 768);
        assert_eq!(options.local_cells, 512);
        assert!(
            parse_options(
                ["capture", "--bogus", "128"]
                    .into_iter()
                    .map(std::ffi::OsString::from),
            )
            .is_err()
        );
    }

    #[test]
    fn hierarchy_cli_selects_successor_versions_without_changing_legacy_modes() {
        let options = parse_options(
            ["capture", "--hierarchy"]
                .into_iter()
                .map(std::ffi::OsString::from),
        )
        .unwrap();
        assert!(options.hierarchy);
        assert!(!options.provinces);
        assert_eq!(
            parse_options(
                ["capture", "--hierarchy", "--local-cells", "384"]
                    .into_iter()
                    .map(std::ffi::OsString::from),
            )
            .unwrap()
            .local_cells,
            384
        );
        assert!(
            parse_options(
                ["capture", "--hierarchy", "--local-cells", "256"]
                    .into_iter()
                    .map(std::ffi::OsString::from),
            )
            .is_err()
        );
        assert_eq!(
            HIERARCHICAL_FAMILIES.map(|family| family.algorithm),
            [
                SurfaceAlgorithm::RockyV5,
                SurfaceAlgorithm::IcyV3,
                SurfaceAlgorithm::VolcanicV3
            ]
        );
        let legacy = parse_options(
            ["capture", "--provinces"]
                .into_iter()
                .map(std::ffi::OsString::from),
        )
        .unwrap();
        assert!(legacy.provinces);
        assert!(!legacy.hierarchy);
        assert!(
            parse_options(
                ["capture", "--hierarchy", "--provinces"]
                    .into_iter()
                    .map(std::ffi::OsString::from),
            )
            .is_err()
        );
    }

    #[test]
    fn corpus_only_is_valueless_and_uses_all_thirteen_shared_fixtures() {
        let options = parse_options(
            ["corpus", "--corpus-only"]
                .into_iter()
                .map(std::ffi::OsString::from),
        )
        .unwrap();
        assert!(options.corpus_only);
        assert_eq!(options.orbit_cells, DEFAULT_ORBIT_CELLS);
        assert_eq!(options.local_cells, DEFAULT_LOCAL_CELLS);
        let fixtures = build_query_corpus_fixtures().unwrap();
        assert_eq!(fixtures.len(), 13);
        assert_eq!(fixtures[0].0, "rocky-seed-0");
        assert_eq!(fixtures[12].0, "irregular-shape-stress");
    }

    #[test]
    fn all_family_samples_share_composed_shape_geometry_and_four_weights() {
        let direction =
            SurfaceLocation::new(Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap());
        for family in FAMILIES {
            let definition = make_definition(
                family,
                17,
                180_000.0,
                ShapeDefinition::ellipsoid([1.1, 0.95, 0.9]).unwrap(),
            )
            .unwrap();
            let generator = SurfaceGenerator::new(&definition, 180_000.0).unwrap();
            let sample = generator.evaluate_point(direction).unwrap();
            assert!(
                (sample.radius_m() - (sample.shape().radius_m() + sample.terrain().height_m()))
                    .abs()
                    < 1e-8
            );
            assert!(sample.normal().is_finite());
            let weights = sample.material_weights();
            assert!(
                weights
                    .iter()
                    .all(|weight| weight.is_finite() && (0.0..=1.0).contains(weight))
            );
            assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-10);
            let adapted =
                <SurfaceGenerator as reference::surface_adapter::SurfaceQuery>::evaluate_point(
                    &generator, direction,
                )
                .unwrap();
            assert_eq!(adapted.material_weights, weights);
            assert_eq!(
                adapted.shape_gradient_m_per_unit_direction,
                sample.shape().gradient_m()
            );
            assert_eq!(
                adapted.terrain_gradient_m_per_unit_direction,
                sample.terrain().tangent_gradient_m_per_unit_direction()
            );
            for probe in generator.diagnostic_boundary_probes(direction).unwrap() {
                assert!(
                    generator
                        .evaluate_point(probe.location)
                        .unwrap()
                        .normal()
                        .is_finite()
                );
            }
        }
    }

    #[test]
    fn irregular_stress_fixture_keeps_positive_radial_envelope() {
        let definition = irregular_definition().unwrap();
        let generator = SurfaceGenerator::new(&definition, 109_000.0).unwrap();
        let envelope = generator.conservative_radius_envelope_m();
        assert!(envelope[0] > 0.0 && envelope[1] > envelope[0]);
        assert_eq!(definition.shape().axes_fractions(), [1.35, 0.92, 0.70]);
        assert_eq!(definition.shape().irregular_amplitudes(), [0.16, 0.12]);
    }

    #[test]
    fn irregular_corpus_fixture_records_the_complete_definition() {
        let definition = irregular_definition().unwrap();
        let metadata =
            fixture_metadata(&definition, FAMILIES[0], SHAPE_STRESS_SEED, 109_000.0, true);
        let corpus = query_corpus(&[(
            "irregular-shape-stress".to_owned(),
            definition.clone(),
            109_000.0,
            metadata,
        )])
        .unwrap();
        let fixture = &corpus["fixtures"][0];
        assert_eq!(fixture["identity"], json!(definition.identity().0));
        assert_eq!(
            fixture["surface_configuration_identity"],
            json!(definition.configuration_identity())
        );
        assert_eq!(
            fixture["terrain_configuration_identity"],
            json!(definition.terrain().configuration_identity())
        );
        assert_eq!(
            fixture["geological_phenotype"]["age"],
            json!(definition.terrain().parameters().age)
        );
        assert_eq!(
            fixture["shape"]["axes_fractions"],
            json!([1.35, 0.92, 0.70])
        );
        for query in corpus["queries"].as_array().unwrap() {
            assert_eq!(
                query["terrain_configuration_identity"],
                fixture["terrain_configuration_identity"]
            );
            assert_eq!(query["material_identity"], fixture["material_identity"]);
        }
    }
}
