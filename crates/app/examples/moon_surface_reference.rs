//! Temporary software reference captures for the Slice 1 moon terrain oracle.
//!
//! This is deliberately a CPU/f64 reference rasterizer, not a claim about the
//! production renderer, native presentation, or GPU performance.

use anyhow::{Context, Result, bail};
use glam::{DVec3, Vec3};
use mundaris_math::{
    Direction3,
    surface::{CubeFace, SurfaceLocation},
};
use mundaris_world::terrain::{
    MoonTerrainConfig, MoonTerrainDefinition, MoonTerrainGenerator, MoonTerrainVersion,
    TerrainIdentity, TerrainSeed,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    env, fs,
    io::BufWriter,
    path::{Path, PathBuf},
    time::Instant,
};
#[path = "moon_surface_reference/mesh_shadow.rs"]
mod mesh_shadow;
#[path = "moon_surface_reference/surface_adapter.rs"]
pub(crate) mod surface_adapter;
use surface_adapter::{ReferenceSample, SurfaceQuery};

const WIDTH: u32 = 960;
const HEIGHT: u32 = 640;
const ORBIT_CELLS: usize = 512;
const LOCAL_CELLS: usize = 384;
const AMBIENT_FRACTION: f64 = 0.015;
const REGOLITH_RGB: [f32; 3] = [0.62, 0.60, 0.56];
const ROCK_RGB: [f32; 3] = [0.47, 0.45, 0.42];
const BASALT_RGB: [f32; 3] = [0.10, 0.11, 0.12];
const RADIUS_M: f64 = 109_000.0;
const IDENTITY: u64 = 0x4d4f_4f4e_5f56_3101;
const SEEDS: [u64; 3] = [2, 7, 19];
const OUTPUTS: [&str; 13] = [
    "lit",
    "unshadowed_lit",
    "lighting",
    "shadow_factor",
    "height",
    "normal",
    "analytic_normal",
    "material",
    "raw_material",
    "material_4",
    "raw_material_4",
    "shape",
    "geometry",
];

#[derive(Clone, Copy, Debug)]
struct Camera {
    position: DVec3,
    target: DVec3,
    up_hint: DVec3,
    fov_y_radians: f64,
    surface_clearance_m: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
struct Vertex {
    position: DVec3,
    mesh_normal: DVec3,
    analytic_normal: DVec3,
    height_m: f64,
    material: [f64; 4],
    raw_material: [f64; 4],
    shape_radius_m: f64,
}

#[derive(Clone, Copy, Debug)]
struct ScreenVertex {
    x: f64,
    y: f64,
    depth: f64,
    position: DVec3,
    mesh_normal: DVec3,
    analytic_normal: DVec3,
    height_m: f64,
    material: [f64; 4],
    raw_material: [f64; 4],
    shape_radius_m: f64,
}

#[derive(Clone, Copy, Debug)]
struct View {
    name: &'static str,
    cells: usize,
    kind: ViewKind,
}

#[derive(Clone, Copy, Debug)]
enum ViewKind {
    Orbit,
    Local {
        side_m: f64,
        altitude_m: f64,
        fov_y_degrees: f64,
    },
}

const VIEWS: [View; 3] = [
    View {
        name: "orbit",
        cells: ORBIT_CELLS,
        kind: ViewKind::Orbit,
    },
    View {
        name: "regional",
        cells: LOCAL_CELLS,
        kind: ViewKind::Local {
            side_m: 20_000.0,
            altitude_m: 12_000.0,
            fov_y_degrees: 58.0,
        },
    },
    View {
        name: "near",
        cells: LOCAL_CELLS,
        kind: ViewKind::Local {
            side_m: 128.0,
            altitude_m: 4.0,
            fov_y_degrees: 82.0,
        },
    },
];

struct Mesh {
    vertices: Vec<Vertex>,
    triangles: Vec<[usize; 3]>,
    spacing_m: f64,
    coverage_description: String,
    material_filter_queries: usize,
    material_filter_wall_ms: f64,
    material_filter_samples_per_vertex: usize,
    #[allow(dead_code)] // Read by the family-package path sharing this mesh builder.
    query_work: [u64; 2],
    #[allow(dead_code)] // Read by the family-package path sharing this mesh builder.
    accepted_features: Option<u64>,
    #[allow(dead_code)] // Recorded by the family-package metadata path.
    gradient_norm_bounds: [f64; 2],
}

struct Images {
    lit: Vec<u8>,
    unshadowed_lit: Vec<u8>,
    lighting: Vec<u8>,
    shadow_factor: Vec<u8>,
    height: Vec<u8>,
    normal: Vec<u8>,
    analytic_normal: Vec<u8>,
    material: Vec<u8>,
    raw_material: Vec<u8>,
    material_4: Vec<u8>,
    raw_material_4: Vec<u8>,
    shape: Vec<u8>,
    geometry: Vec<u8>,
    depth: Vec<f64>,
    covered_pixels: usize,
}

struct ReferenceShadows {
    triangles: mesh_shadow::TriangleShadows,
    toward_light: DVec3,
    build_wall_ms: f64,
}

#[derive(Clone, Copy)]
struct RenderStyle {
    palette: [[f32; 3]; 4],
    toward_light: DVec3,
    ambient_fraction: f64,
    geometry_ambient_fraction: f64,
}

const MOON_PALETTE: [[f32; 3]; 4] = [REGOLITH_RGB, ROCK_RGB, BASALT_RGB, [0.5, 0.5, 0.5]];

#[allow(dead_code)] // Also compiled as a helper module by the family reference example.
fn main() -> Result<()> {
    let options = parse_options(env::args_os().skip(1))?;
    let output = options.output;
    if output.exists() {
        bail!(
            "refusing to overwrite existing output directory: {}",
            output.display()
        );
    }
    fs::create_dir_all(
        output
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?;
    fs::create_dir(&output).with_context(|| format!("creating {}", output.display()))?;

    let config = MoonTerrainConfig::default();
    let mut manifest_scenes = Vec::new();
    let seeds = options
        .seed
        .map_or_else(|| SEEDS.to_vec(), |seed| vec![seed]);
    let views: Vec<_> = VIEWS
        .iter()
        .copied()
        .filter(|view| options.view.as_deref().is_none_or(|name| name == view.name))
        .map(|mut view| {
            if view.name == "orbit" {
                view.cells = options.orbit_cells;
            }
            view
        })
        .collect();
    let mut actual_terrain_version = None;
    for &seed in &seeds {
        let definition = MoonTerrainDefinition::with_version(
            TerrainIdentity(IDENTITY),
            TerrainSeed(seed),
            config,
            options.terrain_version,
        );
        let generator = MoonTerrainGenerator::new(&definition, RADIUS_M)
            .with_context(|| format!("constructing moon terrain for seed {seed}"))?;
        actual_terrain_version = Some(generator.version());
        for view in &views {
            let view = *view;
            let scene_key = format!("seed-{seed}-{}", view.name);
            eprintln!("rendering {scene_key}...");
            let generation_start = Instant::now();
            let mesh = build_mesh(&generator, view)?;
            let query_build_ms = generation_start.elapsed().as_secs_f64() * 1000.0;
            let sampled_height_range = sampled_height_range(&mesh);
            let sampled_shape_range = sampled_shape_range(&mesh);
            let camera = camera_for(view, &generator)?;
            let shadow_start = Instant::now();
            let reference_shadows = build_reference_shadows(&mesh, light_for(view))?;
            let shadow_build_ms = shadow_start.elapsed().as_secs_f64() * 1000.0;
            let raster_start = Instant::now();
            let images = rasterize(
                &mesh,
                camera,
                sampled_height_range,
                sampled_shape_range,
                &reference_shadows,
                RenderStyle {
                    palette: MOON_PALETTE,
                    toward_light: light_for(view),
                    ambient_fraction: AMBIENT_FRACTION,
                    geometry_ambient_fraction: 0.30,
                },
            )?;
            let raster_wall_ms = raster_start.elapsed().as_secs_f64() * 1000.0;
            let residual = measure_residual(&generator, &mesh)?;
            let extra_camera_queries = match view.name {
                "near" => 2,
                "orbit" | "regional" => 0,
                _ => 0,
            };
            let oracle_query_count = mesh.vertices.len()
                + 1
                + residual["sample_count"].as_u64().unwrap_or(0) as usize
                + extra_camera_queries;
            let total_wall_ms = generation_start.elapsed().as_secs_f64() * 1000.0;
            let scene_dir = output.join(&scene_key);
            fs::create_dir(&scene_dir)?;
            for (name, pixels) in [
                ("lit", &images.lit),
                ("unshadowed_lit", &images.unshadowed_lit),
                ("lighting", &images.lighting),
                ("shadow_factor", &images.shadow_factor),
                ("height", &images.height),
                ("normal", &images.normal),
                ("analytic_normal", &images.analytic_normal),
                ("material", &images.material),
                ("raw_material", &images.raw_material),
                ("material_4", &images.material_4),
                ("raw_material_4", &images.raw_material_4),
                ("shape", &images.shape),
                ("geometry", &images.geometry),
            ] {
                write_png(&scene_dir.join(format!("{name}.png")), pixels)?;
            }

            let spacing = mesh.spacing_m;
            let metadata = json!({
                "schema_version": 1,
                "capture_stage": "reference prototype; visual acceptance remains open for user review",
                "renderer": "temporary_software_reference_rasterizer",
                "native_or_gpu_claim": false,
                "seed": seed,
                "terrain_identity": IDENTITY,
                "terrain_version": version_name(generator.version()),
                "radius_m": RADIUS_M,
                "config": config_json(&config),
                "view": view.name,
                "width": WIDTH,
                "height": HEIGHT,
                "mesh": {
                    "topology": match view.kind { ViewKind::Orbit => "six cube-face regular grids", ViewKind::Local{..} => "local tangent chart regular grid" },
                    "cells_per_grid_edge": view.cells,
                    "sample_count": mesh.vertices.len(),
                    "complete_oracle_query_count": oracle_query_count,
                    "derived_material_filter_oracle_query_count": mesh.material_filter_queries,
                    "derived_material_filter": {
                        "version": "MaterialFootprintAverageV2",
                        "samples_per_vertex": mesh.material_filter_samples_per_vertex,
                        "method": if mesh.material_filter_samples_per_vertex == 1 { "complete oracle material at each vertex; no filtering" } else { "2x2 stratified complete-oracle tangent-offset average over one mesh spacing" },
                        "measured_wall_ms": mesh.material_filter_wall_ms,
                        "certified_alias_bound": false,
                        "raw_material_diagnostic": "raw_material.png"
                    },
                    "triangle_count": mesh.triangles.len(),
                    "approximate_sample_spacing_m": spacing,
                    "coverage": mesh.coverage_description,
                    "coverage_fraction_of_pixels": images.covered_pixels as f64 / f64::from(WIDTH * HEIGHT),
                    "global_conservative_radius_envelope_m": generator.radial_envelope_m(),
                    "global_conservative_height_envelope_m": [-generator.absolute_height_bound_m(), generator.absolute_height_bound_m()],
                    "global_conservative_terrain_height_bound_m": generator.absolute_height_bound_m(),
                    "height_diagnostic_mapping": "per-view sampled mesh-vertex min/max, remapped to grayscale",
                    "sampled_vertex_height_range_m": [sampled_height_range.0, sampled_height_range.1],
                    "sampled_vertex_range_is_certified": false,
                    "height_quantization_m_per_8bit_step": (sampled_height_range.1-sampled_height_range.0) / 255.0,
                    "shading_normals": "area-weighted vertex normals reconstructed from actual displaced triangles",
                    "analytic_normal_diagnostic": "complete analytic oracle normals sampled at mesh vertices; coarse views may alias sub-grid detail",
                    "oracle_field": "complete unfiltered surface query at every mesh vertex",
                },
                "camera": camera_json(camera, &generator, None),
                "lighting": {
                    "direction_body_axes": light_for(view).to_array(),
                    "fixture": if matches!(view.kind, ViewKind::Orbit) { "grazing orbital sunlight for terminator relief" } else { "higher local sunlight for morphology inspection; fixed across seeds" },
                    "ambient_fraction": AMBIENT_FRACTION,
                    "diffuse_fraction": 1.0 - AMBIENT_FRACTION,
                    "shadows": true,
                    "reference_shadows": {
                        "type": "fixed_cpu_displaced_triangle_ray_reference",
                        "fit": if matches!(view.kind, ViewKind::Orbit) { "whole displaced body bounds" } else { "actual displaced mesh bounds" },
                        "bounds_body_axes_m": shadow_bounds_json(&reference_shadows),
                        "triangle_count": reference_shadows.triangles.triangle_count(),
                        "index_node_count": reference_shadows.triangles.node_count(),
                        "ray_origin_lightward_offset_m": 0.01,
                        "positive_hit_epsilon_m": 0.00001,
                        "comparison": "ray intersection with actual represented mesh; no shadow-map depth quantization",
                        "index_build_wall_ms": reference_shadows.build_wall_ms
                    },
                    "material_palette_linear_rgb": {"regolith": REGOLITH_RGB, "rock": ROCK_RGB, "basalt": BASALT_RGB},
                    "material_palette_purpose": "illustrative material contrast; not calibrated lunar reflectance or copied reference colors",
                    "geometry_diagnostic": {
                        "material_linear_rgb": [0.55, 0.55, 0.55],
                        "ambient_fraction": 0.30,
                        "independent_of_surface_material_weights": true,
                        "lighting": "same scene direction and represented-mesh shadow visibility"
                    }
                },
                "png_encoding": {"lit_rgb": "linear lighting converted to sRGB", "geometry_rgb": "uniform linear neutral albedo lit and converted to sRGB", "lighting": "linear ambient plus diffuse factor, without albedo", "height_normal_material_analytic_normal_raw_material": "direct normalized 8-bit diagnostic values"},
                "timing_ms": {
                    "terrain_mesh_build_including_material_filter": query_build_ms,
                    "terrain_geometry_and_complete_vertex_queries_excluding_filter": (query_build_ms - mesh.material_filter_wall_ms).max(0.0),
                    "derived_material_filter": mesh.material_filter_wall_ms,
                    "shadow_geometry_index_build": shadow_build_ms,
                    "software_rasterization": raster_wall_ms,
                    "total_scene_work": total_wall_ms
                },
                "representation_residual": residual,
                "limitations": [
                    "software f64 rasterizer; not production renderer, native interaction, GPU parity, settled LOD, convergence, or FPS evidence",
                    "residual is sampled at deterministic triangle centroids, not a conservative certificate",
                    "complete height and analytic normal samples are not footprint-filtered; coarse orbit/regional geometry can alias sub-grid relief",
                    "shadow rays intersect the fixed represented triangles, not the complete analytic surface",
                    "regional and near shadows use crop-only casters; terrain outside the crop cannot cast into it"
                ]
            });
            write_json(&scene_dir.join("metadata.json"), &metadata)?;
            manifest_scenes.push(json!({"key":scene_key,"seed":seed,"view":view.name,"directory":scene_key,"metadata":"metadata.json"}));
        }
    }
    let unique_queries = make_query_corpus(&config, &seeds, options.terrain_version)?;
    write_json(&output.join("oracle_queries.json"), &unique_queries)?;
    write_json(
        &output.join("manifest.json"),
        &json!({
            "schema_version":1,
            "capture_stage":"reference prototype; visual acceptance remains open for user review",
            "title":"Moon terrain Slice 1 software reference captures",
            "renderer":"temporary software f64 reference rasterizer",
            "width":WIDTH,"height":HEIGHT,"radius_m":RADIUS_M,"terrain_identity":IDENTITY,
            "terrain_version":version_name(actual_terrain_version.context("no scenes rendered")?), "seeds":seeds,
            "views":views.iter().map(|view| view.name).collect::<Vec<_>>(),
            "orbit_cells":options.orbit_cells,
            "scenes":manifest_scenes,
            "oracle_query_corpus":"oracle_queries.json",
            "outputs":OUTPUTS,
            "visual_acceptance":"open; user review required"
        }),
    )?;
    write_index(&output, &seeds, &views)?;
    eprintln!("wrote reference package to {}", output.display());
    Ok(())
}

struct Options {
    output: PathBuf,
    seed: Option<u64>,
    view: Option<String>,
    orbit_cells: usize,
    terrain_version: MoonTerrainVersion,
}

fn parse_options(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Options> {
    let mut args = args.peekable();
    let output = args
        .next()
        .map(PathBuf::from)
        .context("usage: moon_surface_reference <new-output-directory> [--seed N] [--view orbit|regional|near] [--orbit-cells N] [--terrain-version 1|2]")?;
    let mut seed = None;
    let mut view = None;
    let mut orbit_cells = ORBIT_CELLS;
    let mut terrain_version = MoonTerrainVersion::MoonLikeV2;
    while let Some(arg) = args.next() {
        let flag = arg.to_string_lossy();
        let value = args
            .next()
            .with_context(|| format!("missing value for {flag}"))?;
        match flag.as_ref() {
            "--terrain-version" => {
                terrain_version = match value.to_string_lossy().as_ref() {
                    "1" => MoonTerrainVersion::MoonLikeV1,
                    "2" => MoonTerrainVersion::MoonLikeV2,
                    _ => bail!("--terrain-version must be 1 or 2"),
                };
            }
            "--seed" => {
                seed = Some(value.to_string_lossy().parse().context("invalid --seed")?);
            }
            "--view" => {
                let name = value.to_string_lossy().into_owned();
                if !VIEWS.iter().any(|view| view.name == name) {
                    bail!("unknown view {name}; expected orbit, regional, or near");
                }
                view = Some(name);
            }
            "--orbit-cells" => {
                orbit_cells = value
                    .to_string_lossy()
                    .parse()
                    .context("invalid --orbit-cells")?;
                if !(64..=1024).contains(&orbit_cells) {
                    bail!("--orbit-cells must be between 64 and 1024");
                }
            }
            _ => bail!("unknown option {flag}"),
        }
    }
    Ok(Options {
        output,
        seed,
        view,
        orbit_cells,
        terrain_version,
    })
}

fn build_mesh(generator: &impl SurfaceQuery, view: View) -> Result<Mesh> {
    let n = view.cells;
    let mut locations = Vec::new();
    let mut triangles = Vec::new();
    let spacing_m;
    let coverage_description;
    match view.kind {
        ViewKind::Orbit => {
            spacing_m = 2.0 * generator.radius_m() / n as f64;
            coverage_description = "six complete normalized cube faces; back-facing surface is naturally depth-occluded".to_owned();
            for face in CubeFace::ALL {
                let base = locations.len();
                for y in 0..=n {
                    for x in 0..=n {
                        let uv = [
                            2.0 * x as f64 / n as f64 - 1.0,
                            2.0 * y as f64 / n as f64 - 1.0,
                        ];
                        locations.push(SurfaceLocation::new(face.direction(uv)?));
                    }
                }
                add_grid_triangles(&mut triangles, base, n);
            }
        }
        ViewKind::Local { side_m, .. } => {
            spacing_m = side_m / n as f64;
            coverage_description = if side_m >= 1000.0 {
                format!(
                    "square tangent chart centered at capture-chart +Z, {:.1} km per side",
                    side_m / 1000.0
                )
            } else {
                format!(
                    "finite square tangent crop centered at capture-chart +Z, {:.1} m per side",
                    side_m
                )
            };
            let center = DVec3::Z;
            let east = DVec3::X;
            let north = DVec3::Y;
            for y in 0..=n {
                for x in 0..=n {
                    let tx = (x as f64 / n as f64 - 0.5) * side_m;
                    let ty = (y as f64 / n as f64 - 0.5) * side_m;
                    locations.push(SurfaceLocation::new(Direction3::try_new(
                        center * generator.radius_m() + east * tx + north * ty,
                    )?));
                }
            }
            add_grid_triangles(&mut triangles, 0, n);
        }
    }
    let first = generator.evaluate_point(locations[0])?;
    let mut samples = vec![first; locations.len()];
    generator.evaluate_batch(&locations, &mut samples)?;
    let raw_materials: Vec<_> = samples
        .iter()
        .map(|sample| sample.material_weights)
        .collect();
    let filter_start = Instant::now();
    let (filtered_materials, material_filter_queries, material_filter_samples_per_vertex) =
        filter_materials(generator, &locations, spacing_m, view, &raw_materials)?;
    let material_filter_wall_ms = filter_start.elapsed().as_secs_f64() * 1000.0;
    let mut vertices = Vec::with_capacity(samples.len());
    let mut query_work = [0_u64; 2];
    let mut accepted_features = Some(0_u64);
    let mut gradient_norm_bounds = [0.0_f64; 2];
    let direction_keys: Vec<_> = locations
        .iter()
        .map(|location| direction_key(location.direction().unit()))
        .collect();
    for (index, (location, sample)) in locations.iter().copied().zip(samples).enumerate() {
        for (total, count) in query_work.iter_mut().zip(sample.work) {
            *total += u64::from(count);
        }
        accepted_features = accepted_features
            .zip(sample.accepted_features.map(u64::from))
            .map(|(total, count)| total + count);
        gradient_norm_bounds[0] =
            gradient_norm_bounds[0].max(sample.shape_gradient_m_per_unit_direction.length());
        gradient_norm_bounds[1] =
            gradient_norm_bounds[1].max(sample.terrain_gradient_m_per_unit_direction.length());
        let direction = location.direction().unit();
        vertices.push(Vertex {
            position: direction * sample.radius_m,
            mesh_normal: DVec3::ZERO,
            analytic_normal: sample.normal_body,
            height_m: sample.terrain_height_m,
            material: filtered_materials[index],
            raw_material: raw_materials[index],
            shape_radius_m: sample.shape_radius_m,
        });
    }
    let mut normal_sums = vec![DVec3::ZERO; vertices.len()];
    for triangle in &triangles {
        let a = vertices[triangle[0]].position;
        let b = vertices[triangle[1]].position;
        let c = vertices[triangle[2]].position;
        let mut area_normal = (b - a).cross(c - a);
        if area_normal.dot(a + b + c) < 0.0 {
            area_normal = -area_normal;
        }
        for &index in triangle {
            normal_sums[index] += area_normal;
        }
    }
    let reconciled_normals = reconcile_duplicate_normals(&direction_keys, &normal_sums);
    for (vertex, normal) in vertices.iter_mut().zip(reconciled_normals) {
        vertex.mesh_normal = normal;
    }
    Ok(Mesh {
        vertices,
        triangles,
        spacing_m,
        coverage_description,
        material_filter_queries,
        material_filter_wall_ms,
        material_filter_samples_per_vertex,
        query_work,
        accepted_features,
        gradient_norm_bounds,
    })
}

fn filter_materials(
    generator: &impl SurfaceQuery,
    locations: &[SurfaceLocation],
    spacing_m: f64,
    view: View,
    raw_materials: &[[f64; 4]],
) -> Result<(Vec<[f64; 4]>, usize, usize)> {
    if is_near_view(view) {
        return Ok((raw_materials.to_vec(), 0, 1));
    }
    const FILTER_SAMPLES_PER_AXIS: usize = 2;
    const SAMPLES_PER_VERTEX: usize = FILTER_SAMPLES_PER_AXIS * FILTER_SAMPLES_PER_AXIS;
    const VERTICES_PER_BATCH: usize = 256;
    let mut filtered = vec![[0.0; 4]; locations.len()];
    let mut query_count = 0;
    let half_spacing = spacing_m * 0.5;
    for base in (0..locations.len()).step_by(VERTICES_PER_BATCH) {
        let end = (base + VERTICES_PER_BATCH).min(locations.len());
        let mut footprint_locations = Vec::with_capacity((end - base) * SAMPLES_PER_VERTEX);
        for location in &locations[base..end] {
            let direction = location.direction().unit();
            let reference = if direction.z.abs() < 0.9 {
                DVec3::Z
            } else {
                DVec3::Y
            };
            let east = direction.cross(reference).normalize();
            let north = direction.cross(east).normalize();
            for sample_y in 0..FILTER_SAMPLES_PER_AXIS {
                for sample_x in 0..FILTER_SAMPLES_PER_AXIS {
                    let offset_x = ((sample_x as f64 + 0.5) / FILTER_SAMPLES_PER_AXIS as f64 * 2.0
                        - 1.0)
                        * half_spacing;
                    let offset_y = ((sample_y as f64 + 0.5) / FILTER_SAMPLES_PER_AXIS as f64 * 2.0
                        - 1.0)
                        * half_spacing;
                    let sample_direction = Direction3::try_new(
                        direction * generator.radius_m() + east * offset_x + north * offset_y,
                    )?;
                    footprint_locations.push(SurfaceLocation::new(sample_direction));
                }
            }
        }
        let first = generator.evaluate_point(footprint_locations[0])?;
        let mut footprint_samples = vec![first; footprint_locations.len()];
        generator.evaluate_batch(&footprint_locations, &mut footprint_samples)?;
        query_count += footprint_samples.len() + 1;
        for (local_index, group) in footprint_samples
            .as_chunks::<SAMPLES_PER_VERTEX>()
            .0
            .iter()
            .enumerate()
        {
            for sample in group {
                let weights = sample.material_weights;
                for (sum, weight) in filtered[base + local_index].iter_mut().zip(weights) {
                    *sum += weight / SAMPLES_PER_VERTEX as f64;
                }
            }
        }
    }
    Ok((filtered, query_count, SAMPLES_PER_VERTEX))
}

fn is_near_view(view: View) -> bool {
    matches!(view.kind, ViewKind::Local { side_m, .. } if side_m < 1_000.0)
}

fn family_camera_query_count(view: View) -> usize {
    if is_near_view(view) { 5 } else { 1 }
}

const DIRECTOR_CHANNELS: [&str; 13] = [
    "director-age",
    "director-activity",
    "director-resurfacing",
    "director-impact-retention",
    "director-relief-potential",
    "province-0",
    "province-1",
    "province-2",
    "province-3",
    "process-0",
    "process-1",
    "process-2",
    "process-3",
];
const DIRECTOR_LOCAL_SIZE: u32 = 256;
const DIRECTOR_ORBIT_WIDTH: u32 = 512;
const DIRECTOR_ORBIT_HEIGHT: u32 = 256;

struct DirectorDiagnostics {
    width: u32,
    height: u32,
    center_controls: Option<mundaris_world::terrain::GeologicalControls>,
    map_query_count: usize,
    center_query_count: usize,
    center_query_wall_ms: f64,
    map_query_wall_ms: f64,
    projection: &'static str,
}

fn write_director_diagnostics(
    scene_dir: &Path,
    generator: &impl SurfaceQuery,
    view: View,
) -> Result<DirectorDiagnostics> {
    let center = SurfaceLocation::new(Direction3::try_new(DVec3::Z)?);
    let center_start = Instant::now();
    let center_controls = generator.geological_controls(center)?;
    let center_query_wall_ms = center_start.elapsed().as_secs_f64() * 1000.0;
    let (width, height, projection) = match view.kind {
        ViewKind::Orbit => (
            DIRECTOR_ORBIT_WIDTH,
            DIRECTOR_ORBIT_HEIGHT,
            "body-fixed equirectangular longitude/latitude; not the orbit camera projection",
        ),
        ViewKind::Local { .. } => (
            DIRECTOR_LOCAL_SIZE,
            DIRECTOR_LOCAL_SIZE,
            "capture-chart local tangent square centered at +Z; not the perspective camera projection",
        ),
    };
    let Some(center_controls) = center_controls else {
        return Ok(DirectorDiagnostics {
            width: 0,
            height: 0,
            center_controls: None,
            map_query_count: 0,
            center_query_count: 1,
            center_query_wall_ms,
            map_query_wall_ms: 0.0,
            projection: "unavailable for this surface family",
        });
    };
    let start = Instant::now();
    let mut images: [Vec<u8>; DIRECTOR_CHANNELS.len()] =
        std::array::from_fn(|_| vec![0; (width * height * 4) as usize]);
    let mut map_query_count = 0;
    for y in 0..height {
        for x in 0..width {
            let chart_direction = match view.kind {
                ViewKind::Orbit => {
                    let longitude = (x as f64 + 0.5) / f64::from(width) * std::f64::consts::TAU
                        - std::f64::consts::PI;
                    let latitude = std::f64::consts::FRAC_PI_2
                        - (y as f64 + 0.5) / f64::from(height) * std::f64::consts::PI;
                    let body_direction = DVec3::new(
                        latitude.cos() * longitude.sin(),
                        latitude.sin(),
                        latitude.cos() * longitude.cos(),
                    );
                    generator.body_to_chart_direction(body_direction)
                }
                ViewKind::Local { side_m, .. } => {
                    let tx = (x as f64 + 0.5) / f64::from(width) * side_m - side_m * 0.5;
                    let ty = side_m * 0.5 - (y as f64 + 0.5) / f64::from(height) * side_m;
                    DVec3::Z * generator.radius_m() + DVec3::X * tx + DVec3::Y * ty
                }
            };
            let location = SurfaceLocation::new(Direction3::try_new(chart_direction)?);
            let controls = generator
                .geological_controls(location)?
                .context("geological controls disappeared while generating diagnostic map")?;
            map_query_count += 1;
            let values = [
                controls.age,
                controls.activity,
                controls.resurfacing,
                controls.impact_retention,
                controls.relief_potential / 0.01,
                controls.province_weights[0],
                controls.province_weights[1],
                controls.province_weights[2],
                controls.province_weights[3],
                controls.process_strengths[0],
                controls.process_strengths[1],
                controls.process_strengths[2],
                controls.process_strengths[3],
            ];
            let offset = (y as usize * width as usize + x as usize) * 4;
            for (image, value) in images.iter_mut().zip(values) {
                let pixel = gray(value.clamp(0.0, 1.0));
                image[offset..offset + 4].copy_from_slice(&[pixel, pixel, pixel, 255]);
            }
        }
    }
    let query_wall_ms = start.elapsed().as_secs_f64() * 1000.0;
    for (name, pixels) in DIRECTOR_CHANNELS.into_iter().zip(&images) {
        write_png_sized(
            &scene_dir.join(format!("{name}.png")),
            width,
            height,
            pixels,
        )?;
    }
    Ok(DirectorDiagnostics {
        width,
        height,
        center_controls: Some(center_controls),
        map_query_count,
        center_query_count: 1,
        center_query_wall_ms,
        map_query_wall_ms: query_wall_ms,
        projection,
    })
}

fn direction_key(direction: DVec3) -> [u64; 3] {
    direction
        .to_array()
        .map(|value| if value == 0.0 { 0 } else { value.to_bits() })
}

fn reconcile_duplicate_normals(keys: &[[u64; 3]], normal_sums: &[DVec3]) -> Vec<DVec3> {
    let mut shared_sums = HashMap::with_capacity(keys.len());
    for (&key, &normal_sum) in keys.iter().zip(normal_sums) {
        *shared_sums.entry(key).or_insert(DVec3::ZERO) += normal_sum;
    }
    keys.iter()
        .map(|key| shared_sums[key].normalize())
        .collect()
}

fn add_grid_triangles(triangles: &mut Vec<[usize; 3]>, base: usize, n: usize) {
    for y in 0..n {
        for x in 0..n {
            let a = base + y * (n + 1) + x;
            let b = a + 1;
            let c = a + n + 1;
            let d = c + 1;
            triangles.push([a, b, d]);
            triangles.push([a, d, c]);
        }
    }
}

fn light_direction() -> DVec3 {
    DVec3::new(-0.93, 0.22, 0.12).normalize()
}

fn light_for(view: View) -> DVec3 {
    if matches!(view.kind, ViewKind::Orbit) {
        light_direction()
    } else {
        DVec3::new(-0.65, 0.18, 0.74).normalize()
    }
}

fn version_name(version: MoonTerrainVersion) -> &'static str {
    match version {
        MoonTerrainVersion::MoonLikeV1 => "MoonLikeV1",
        MoonTerrainVersion::MoonLikeV2 => "MoonLikeV2",
    }
}

fn build_reference_shadows(mesh: &Mesh, toward_light: DVec3) -> Result<ReferenceShadows> {
    let start = Instant::now();
    let positions: Vec<_> = mesh.vertices.iter().map(|vertex| vertex.position).collect();
    let triangles = mesh_shadow::TriangleShadows::new(&positions, &mesh.triangles)?;
    Ok(ReferenceShadows {
        triangles,
        toward_light,
        build_wall_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

#[derive(Clone, Copy)]
pub(crate) struct FamilyCaptureStyle {
    pub(crate) palette: [[f32; 3]; 4],
    pub(crate) neutral_broad_light: bool,
}

fn province_view(view_name: &str, cells: usize, metadata: &Value) -> Result<View> {
    if !view_name.starts_with("province-") && !matches!(view_name, "unbiased-32m" | "unbiased-8m") {
        bail!("unknown family reference view {view_name}");
    }
    let scale = metadata
        .get("local_scale")
        .and_then(Value::as_object)
        .context("province view requires local_scale object")?;
    let side_m = scale
        .get("side_m")
        .and_then(Value::as_f64)
        .context("local_scale.side_m must be numeric")?;
    let altitude_m = scale
        .get("altitude_m")
        .and_then(Value::as_f64)
        .context("local_scale.altitude_m must be numeric")?;
    let fov_y_degrees = scale
        .get("fov_y_degrees")
        .and_then(Value::as_f64)
        .context("local_scale.fov_y_degrees must be numeric")?;
    if !side_m.is_finite()
        || side_m <= 0.0
        || !altitude_m.is_finite()
        || altitude_m <= 0.0
        || !fov_y_degrees.is_finite()
        || !(1.0..179.0).contains(&fov_y_degrees)
    {
        bail!("local_scale side and altitude must be positive; fov_y_degrees must be in (1, 179)");
    }
    Ok(View {
        name: "province",
        cells,
        kind: ViewKind::Local {
            side_m,
            altitude_m,
            fov_y_degrees,
        },
    })
}

/// Render a body from the compositional world surface oracle. The family driver
/// owns definition metadata and chooses its presentation palette; this renderer
/// consumes only complete geometry/material samples.
#[allow(dead_code)] // Called by the sibling surface_family_reference example.
pub(crate) fn capture_family_scene(
    output: &Path,
    body_key: &str,
    view_name: &str,
    cells: usize,
    generator: &impl SurfaceQuery,
    style: FamilyCaptureStyle,
    body_metadata: Value,
) -> Result<Value> {
    let FamilyCaptureStyle {
        palette,
        neutral_broad_light,
    } = style;
    let has_local_scale = body_metadata.get("local_scale").is_some();
    let view = match view_name {
        "orbit" | "orbit-neutral" => View {
            name: "orbit",
            cells,
            kind: ViewKind::Orbit,
        },
        "regional" | "regional-landmark" => View {
            name: "regional",
            cells,
            kind: ViewKind::Local {
                side_m: 20_000.0,
                altitude_m: 12_000.0,
                fov_y_degrees: 58.0,
            },
        },
        "near" | "near-landmark" => View {
            name: "near",
            cells,
            kind: ViewKind::Local {
                side_m: 128.0,
                altitude_m: 4.0,
                fov_y_degrees: 82.0,
            },
        },
        _ if has_local_scale => province_view(view_name, cells, &body_metadata)?,
        _ => bail!("unknown family reference view {view_name}"),
    };
    let scene_dir = output.join(body_key).join(view_name);
    fs::create_dir_all(&scene_dir)?;
    let start = Instant::now();
    let mesh = build_mesh(generator, view)?;
    let query_build_ms = start.elapsed().as_secs_f64() * 1000.0;
    let (camera, camera_normal_offset_m) = family_camera_for(view, generator)?;
    let (toward_light, body_toward_light) =
        capture_light_directions(generator, view, neutral_broad_light);
    let shadow_start = Instant::now();
    let shadows = build_reference_shadows(&mesh, toward_light)?;
    let shadow_build_ms = shadow_start.elapsed().as_secs_f64() * 1000.0;
    let raster_start = Instant::now();
    let images = rasterize(
        &mesh,
        camera,
        sampled_height_range(&mesh),
        sampled_shape_range(&mesh),
        &shadows,
        RenderStyle {
            palette: if neutral_broad_light {
                [[0.55, 0.55, 0.55]; 4]
            } else {
                palette
            },
            toward_light,
            ambient_fraction: if neutral_broad_light {
                0.58
            } else {
                AMBIENT_FRACTION
            },
            geometry_ambient_fraction: 0.30,
        },
    )?;
    let raster_wall_ms = raster_start.elapsed().as_secs_f64() * 1000.0;
    let residual = measure_residual(generator, &mesh)?;
    let director_diagnostics = write_director_diagnostics(&scene_dir, generator, view)?;
    let camera_query_count = family_camera_query_count(view);
    let complete_oracle_query_count = mesh.vertices.len()
        + 1
        + mesh.material_filter_queries
        + residual["sample_count"].as_u64().unwrap_or(0) as usize
        + camera_query_count;
    for (name, pixels) in [
        ("lit", &images.lit),
        ("unshadowed_lit", &images.unshadowed_lit),
        ("lighting", &images.lighting),
        ("shadow_factor", &images.shadow_factor),
        ("height", &images.height),
        ("normal", &images.normal),
        ("analytic_normal", &images.analytic_normal),
        ("material", &images.material),
        ("raw_material", &images.raw_material),
        ("material_4", &images.material_4),
        ("raw_material_4", &images.raw_material_4),
        ("shape", &images.shape),
        ("geometry", &images.geometry),
    ] {
        write_png(&scene_dir.join(format!("{name}.png")), pixels)?;
    }
    let mut metadata = json!({
        "schema_version": 1,
        "capture_stage": "reference prototype; user visual acceptance remains open",
        "renderer": "temporary_software_f64_reference_rasterizer",
        "native_or_gpu_claim": false,
        "body_key": body_key,
        "view": view_name,
        "regional_crop_center_direction_body_axes": generator.chart_to_body_direction(DVec3::Z).to_array(),
        "chart_to_body_matrix_columns_body_axes": [
            generator.chart_to_body_direction(DVec3::X).to_array(),
            generator.chart_to_body_direction(DVec3::Y).to_array(),
            generator.chart_to_body_direction(DVec3::Z).to_array()
        ],
        "local_crop_policy": if body_metadata.get("landmark_selection").is_some() {
            "crop is centered on the deterministic maximum-gradient body-fixed landmark recorded in landmark_selection"
        } else {
            "fixed body-fixed +Z chart, identical across bodies; not selected around a procedural feature"
        },
        "width": WIDTH,
        "height": HEIGHT,
        "mesh": {
            "cells_per_grid_edge": cells,
            "sample_count": mesh.vertices.len(),
            "complete_oracle_query_count": complete_oracle_query_count,
            "triangle_count": mesh.triangles.len(),
            "approximate_sample_spacing_m": mesh.spacing_m,
            "coverage": mesh.coverage_description,
            "mesh_vertex_query_work": {
                "scope": "sum of world query work counters for mesh vertex samples only; excludes the batch priming query, material footprint filtering, residual probes, camera queries, and landmark selection",
                "cells_visited": mesh.query_work[0],
                "candidate_features": mesh.query_work[1],
                "accepted_features": mesh.accepted_features
            },
            "sampled_gradient_norm_maxima_m_per_unit_direction": {
                "shape": mesh.gradient_norm_bounds[0],
                "terrain": mesh.gradient_norm_bounds[1]
            },
            "sample_sizes_bytes": {
                "reference_sample_inline": std::mem::size_of::<ReferenceSample>(),
                "world_surface_sample_inline": std::mem::size_of::<mundaris_world::terrain::SurfaceSample>(),
                "query_adapter_inline": std::mem::size_of_val(generator),
                "world_surface_generator_inline": std::mem::size_of::<mundaris_world::terrain::SurfaceGenerator>(),
                "surface_generator_owned_heap_and_query_scratch": "not exposed by world generator; unavailable"
            },
            "material_filter": {
                "method": if mesh.material_filter_samples_per_vertex == 1 { "complete oracle point samples" } else { "2x2 stratified complete oracle tangent-offset average" },
                "samples_per_vertex": mesh.material_filter_samples_per_vertex,
                "query_count": mesh.material_filter_queries,
                "measured_wall_ms": mesh.material_filter_wall_ms,
                "certified_alias_bound": false
            },
            "radial_envelope_m": generator.radial_envelope_m(),
            "terrain_height_bound_m": generator.absolute_height_bound_m(),
            "shape_diagnostic": "sampled complete shape radius at mesh vertices; per-scene min/max mapped to grayscale",
            "terrain_height_diagnostic": "complete terrain height at mesh vertices; per-scene min/max mapped to grayscale"
        },
        "camera": camera_json(camera, generator, camera_normal_offset_m),
        "geological_director": {
            "center_query_location_chart_axes": [0.0, 0.0, 1.0],
            "center_query_count": director_diagnostics.center_query_count,
            "center_controls": director_diagnostics.center_controls.map(|controls| json!({
                "age": controls.age,
                "activity": controls.activity,
                "resurfacing": controls.resurfacing,
                "impact_retention": controls.impact_retention,
                "relief_potential": controls.relief_potential,
                "province_weights": controls.province_weights,
                "process_strengths": controls.process_strengths,
                "structural_direction_chart_axes": controls.structural_direction.to_array()
            })),
            "images": if director_diagnostics.center_controls.is_some() { DIRECTOR_CHANNELS.to_vec() } else { Vec::<&str>::new() },
            "map_projection": director_diagnostics.projection,
            "map_dimensions": [director_diagnostics.width, director_diagnostics.height],
            "map_query_count": director_diagnostics.map_query_count,
            "center_query_wall_ms": director_diagnostics.center_query_wall_ms,
            "map_query_wall_ms": director_diagnostics.map_query_wall_ms,
            "scalar_encoding": "each channel mapped to [0,1] and written as linear 8-bit grayscale",
            "channel_value_mappings": {
                "director-age": "age",
                "director-activity": "activity",
                "director-resurfacing": "resurfacing",
                "director-impact-retention": "impact_retention",
                "director-relief-potential": "relief_potential / 0.01; full white is the 1% maximum",
                "province-0..3": "matching province_weights element",
                "process-0..3": "matching process_strengths element"
            },
            "values_are_authoritative": true
        },
        "local_scale": body_metadata.get("local_scale"),
        "capture_coordinate_basis": {
            "chart_axes_in_body_axes": [
                generator.chart_to_body_direction(DVec3::X).to_array(),
                generator.chart_to_body_direction(DVec3::Y).to_array(),
                generator.chart_to_body_direction(DVec3::Z).to_array()
            ],
            "geometry_positions_and_diagnostic_normals": "capture chart axes",
            "material_diagnostics": "body-fixed material weights sampled through the chart-to-body query adapter"
        },
        "lighting": {
            "direction_body_axes": body_toward_light.to_array(),
            "direction_chart_axes": toward_light.to_array(),
            "policy": if neutral_broad_light { "neutral broad light fixed in capture-chart axes" } else { "per-view fixture from light_for(view), fixed in capture-chart axes across rotated landmarks" },
            "direction_basis_conversion": "capture-chart fixture transformed to body axes for metadata; rasterization and shadows use capture-chart axes",
            "neutral_broad_light": neutral_broad_light,
            "ambient_fraction": if neutral_broad_light { 0.58 } else { AMBIENT_FRACTION },
            "shadows": true,
            "shadow_mesh": "represented displaced triangles; local views use crop-only casters",
            "palette_linear_rgb": if neutral_broad_light { "neutral grayscale" } else { "family presentation palette supplied by the capture definition" },
            "geometry_diagnostic": {
                "material": "uniform neutral linear RGB [0.55, 0.55, 0.55], independent of world material weights",
                "ambient_fraction": 0.30,
                "direction_chart_axes": toward_light.to_array(),
                "shadow_geometry": "same represented mesh and shadow visibility as the scene render"
            }
        },
        "timing_ms": {
            "mesh_and_oracle_queries": query_build_ms,
            "shadow_geometry_index_build": shadow_build_ms,
            "software_rasterization": raster_wall_ms
        },
        "representation_residual": residual,
        "limitations": [
            "software reference rendering is not production renderer or native performance evidence",
            "sampled triangle-centroid residual is not a conservative certificate",
            "shadow rays intersect represented triangles; local crops omit outside casters",
            "star-shaped radial representation does not include overhangs or cavities"
        ]
    });
    if let (Some(dst), Some(src)) = (metadata.as_object_mut(), body_metadata.as_object()) {
        for (key, value) in src {
            dst.insert(key.clone(), value.clone());
        }
    }
    write_json(&scene_dir.join("metadata.json"), &metadata)?;
    Ok(metadata)
}

fn capture_light_directions(
    generator: &impl SurfaceQuery,
    view: View,
    neutral_broad_light: bool,
) -> (DVec3, DVec3) {
    let chart_direction = if neutral_broad_light {
        DVec3::new(0.18, 0.31, 0.93).normalize()
    } else {
        light_for(view)
    };
    let chart_direction = chart_direction.normalize();
    let body_direction = generator
        .chart_to_body_direction(chart_direction)
        .normalize();
    (chart_direction, body_direction)
}
fn shadow_visibility(position: DVec3, shadows: &ReferenceShadows) -> f64 {
    shadows.triangles.visibility(position, shadows.toward_light)
}
fn shadow_bounds_json(shadows: &ReferenceShadows) -> Value {
    let (minimum, maximum) = shadows.triangles.bounds();
    json!({"minimum_body_axes_m":minimum.to_array(),"maximum_body_axes_m":maximum.to_array()})
}

fn camera_for(view: View, generator: &impl SurfaceQuery) -> Result<Camera> {
    match view.kind {
        ViewKind::Orbit => {
            let location = SurfaceLocation::new(Direction3::try_new(DVec3::Z)?);
            let target_radius = generator.view_target_radius_m(location, false)?;
            let envelope = generator.radial_envelope_m();
            let fov_y_radians = 38f64.to_radians();
            let fit_distance = envelope[1] / (fov_y_radians * 0.5).sin() * 1.08;
            let nominal_distance = generator.radius_m() * 3.45;
            let distance = if envelope[1] > generator.radius_m() * 1.2 {
                nominal_distance.max(target_radius + fit_distance)
            } else {
                nominal_distance
            };
            Ok(Camera {
                position: DVec3::Z * distance,
                target: DVec3::Z * target_radius,
                up_hint: DVec3::Y,
                fov_y_radians,
                surface_clearance_m: None,
            })
        }
        ViewKind::Local {
            side_m,
            altitude_m,
            fov_y_degrees,
        } if is_near_view(view) => {
            let direction =
                Direction3::try_new(DVec3::Z * generator.radius_m() + DVec3::X * (side_m * 0.06))?;
            let eye_radius = generator
                .evaluate_point(SurfaceLocation::new(direction))?
                .radius_m;
            let target_radius = generator.view_target_radius_m(
                SurfaceLocation::new(Direction3::try_new(DVec3::Z)?),
                false,
            )?;
            Ok(Camera {
                position: direction.unit() * (eye_radius + altitude_m),
                target: DVec3::Z * target_radius,
                up_hint: DVec3::Z,
                fov_y_radians: fov_y_degrees.to_radians(),
                surface_clearance_m: Some(altitude_m),
            })
        }
        ViewKind::Local {
            altitude_m,
            fov_y_degrees,
            ..
        } => {
            let target = DVec3::Z
                * generator.view_target_radius_m(
                    SurfaceLocation::new(Direction3::try_new(DVec3::Z)?),
                    false,
                )?;
            Ok(Camera {
                position: target + DVec3::Z * altitude_m + DVec3::X * altitude_m * 0.23,
                target,
                up_hint: DVec3::Y,
                fov_y_radians: fov_y_degrees.to_radians(),
                surface_clearance_m: None,
            })
        }
    }
}

fn family_camera_for(view: View, generator: &impl SurfaceQuery) -> Result<(Camera, Option<f64>)> {
    let mut camera = camera_for(view, generator)?;
    if !is_near_view(view) {
        return Ok((camera, None));
    }
    let ViewKind::Local {
        side_m, altitude_m, ..
    } = view.kind
    else {
        unreachable!()
    };
    let eye_direction = Direction3::try_new(
        DVec3::Z * generator.radius_m()
            + DVec3::X * (side_m * if view.name == "province" { 0.45 } else { 0.06 }),
    )?;
    let eye_location = SurfaceLocation::new(eye_direction);
    let eye = generator.evaluate_point(eye_location)?;
    let target_location = SurfaceLocation::new(Direction3::try_new(DVec3::Z)?);
    let target_radius = generator.view_target_radius_m(target_location, false)?;
    camera.position = eye_direction.unit() * eye.radius_m + eye.normal_body * altitude_m;
    camera.target = DVec3::Z * target_radius;
    camera.up_hint = eye.normal_body;
    let final_eye_location = SurfaceLocation::new(Direction3::try_new(camera.position)?);
    let final_eye_surface_radius = generator.evaluate_point(final_eye_location)?.radius_m;
    camera.surface_clearance_m = Some(camera.position.length() - final_eye_surface_radius);
    Ok((camera, Some(altitude_m)))
}

fn sampled_height_range(mesh: &Mesh) -> (f64, f64) {
    let mut low = f64::INFINITY;
    let mut high = f64::NEG_INFINITY;
    for vertex in &mesh.vertices {
        low = low.min(vertex.height_m);
        high = high.max(vertex.height_m);
    }
    if high - low < 1e-12 {
        (low - 0.5, high + 0.5)
    } else {
        (low, high)
    }
}

fn sampled_shape_range(mesh: &Mesh) -> (f64, f64) {
    mesh.vertices
        .iter()
        .map(|vertex| vertex.shape_radius_m)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
            (lo.min(value), hi.max(value))
        })
}

fn rasterize(
    mesh: &Mesh,
    camera: Camera,
    height_range_m: (f64, f64),
    shape_range_m: (f64, f64),
    reference_shadows: &ReferenceShadows,
    style: RenderStyle,
) -> Result<Images> {
    let pixels = (WIDTH * HEIGHT) as usize;
    let mut images = Images {
        lit: vec![0; pixels * 4],
        unshadowed_lit: vec![0; pixels * 4],
        lighting: vec![0; pixels * 4],
        shadow_factor: vec![0; pixels * 4],
        height: vec![0; pixels * 4],
        normal: vec![0; pixels * 4],
        analytic_normal: vec![0; pixels * 4],
        material: vec![0; pixels * 4],
        raw_material: vec![0; pixels * 4],
        material_4: vec![0; pixels * 4],
        raw_material_4: vec![0; pixels * 4],
        shape: vec![0; pixels * 4],
        geometry: vec![0; pixels * 4],
        depth: vec![f64::INFINITY; pixels],
        covered_pixels: 0,
    };
    let forward = (camera.target - camera.position).normalize();
    let up_hint = (camera.up_hint - forward * camera.up_hint.dot(forward)).normalize();
    let right = forward.cross(up_hint).normalize();
    let up = right.cross(forward).normalize();
    let focal = f64::from(HEIGHT) * 0.5 / (camera.fov_y_radians * 0.5).tan();
    for tri in &mesh.triangles {
        let polygon =
            clip_triangle_near(tri.map(|i| mesh.vertices[i]), camera.position, forward, 1.0);
        if polygon.len() < 3 {
            continue;
        }
        let project = |v: Vertex| {
            let relative = v.position - camera.position;
            let depth = relative.dot(forward);
            ScreenVertex {
                x: f64::from(WIDTH) * 0.5 + focal * relative.dot(right) / depth,
                y: f64::from(HEIGHT) * 0.5 - focal * relative.dot(up) / depth,
                depth,
                position: v.position,
                mesh_normal: v.mesh_normal,
                analytic_normal: v.analytic_normal,
                height_m: v.height_m,
                material: v.material,
                raw_material: v.raw_material,
                shape_radius_m: v.shape_radius_m,
            }
        };
        for i in 1..polygon.len() - 1 {
            raster_triangle(
                &mut images,
                [
                    project(polygon[0]),
                    project(polygon[i]),
                    project(polygon[i + 1]),
                ],
                height_range_m,
                shape_range_m,
                style,
                reference_shadows,
            );
        }
    }
    images.covered_pixels = images.depth.iter().filter(|d| d.is_finite()).count();
    Ok(images)
}

fn raster_triangle(
    images: &mut Images,
    v: [ScreenVertex; 3],
    height_range_m: (f64, f64),
    shape_range_m: (f64, f64),
    style: RenderStyle,
    reference_shadows: &ReferenceShadows,
) {
    let raw_min_x = v.iter().map(|p| p.x.floor() as i32).min().unwrap();
    let raw_max_x = v.iter().map(|p| p.x.ceil() as i32).max().unwrap();
    let raw_min_y = v.iter().map(|p| p.y.floor() as i32).min().unwrap();
    let raw_max_y = v.iter().map(|p| p.y.ceil() as i32).max().unwrap();
    if raw_max_x < 0 || raw_min_x >= WIDTH as i32 || raw_max_y < 0 || raw_min_y >= HEIGHT as i32 {
        return;
    }
    let min_x = raw_min_x.clamp(0, WIDTH as i32 - 1);
    let max_x = raw_max_x.clamp(0, WIDTH as i32 - 1);
    let min_y = raw_min_y.clamp(0, HEIGHT as i32 - 1);
    let max_y = raw_max_y.clamp(0, HEIGHT as i32 - 1);
    let area = edge((v[0].x, v[0].y), (v[1].x, v[1].y), (v[2].x, v[2].y));
    if area.abs() < 1e-12 {
        return;
    }
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let p = (x as f64 + 0.5, y as f64 + 0.5);
            let b0 = edge((v[1].x, v[1].y), (v[2].x, v[2].y), p) / area;
            let b1 = edge((v[2].x, v[2].y), (v[0].x, v[0].y), p) / area;
            let b2 = 1.0 - b0 - b1;
            if b0 < -1e-10 || b1 < -1e-10 || b2 < -1e-10 {
                continue;
            }
            let reciprocal_depth = b0 / v[0].depth + b1 / v[1].depth + b2 / v[2].depth;
            let depth = reciprocal_depth.recip();
            let idx = y as usize * WIDTH as usize + x as usize;
            if depth >= images.depth[idx] {
                continue;
            }
            let w = [
                b0 / v[0].depth / reciprocal_depth,
                b1 / v[1].depth / reciprocal_depth,
                b2 / v[2].depth / reciprocal_depth,
            ];
            let interp =
                |f: fn(&ScreenVertex) -> f64| w[0] * f(&v[0]) + w[1] * f(&v[1]) + w[2] * f(&v[2]);
            let mesh_normal =
                (v[0].mesh_normal * w[0] + v[1].mesh_normal * w[1] + v[2].mesh_normal * w[2])
                    .normalize();
            let analytic_normal = (v[0].analytic_normal * w[0]
                + v[1].analytic_normal * w[1]
                + v[2].analytic_normal * w[2])
                .normalize();
            let height = interp(|p| p.height_m);
            let material: [f64; 4] = std::array::from_fn(|i| {
                w[0] * v[0].material[i] + w[1] * v[1].material[i] + w[2] * v[2].material[i]
            });
            let raw_material: [f64; 4] = std::array::from_fn(|i| {
                w[0] * v[0].raw_material[i]
                    + w[1] * v[1].raw_material[i]
                    + w[2] * v[2].raw_material[i]
            });
            images.depth[idx] = depth;
            let height_gray = gray(
                ((height - height_range_m.0) / (height_range_m.1 - height_range_m.0))
                    .clamp(0.0, 1.0),
            );
            put_rgba(
                &mut images.height,
                idx,
                [height_gray, height_gray, height_gray, 255],
            );
            let rgb = [mesh_normal.x, mesh_normal.y, mesh_normal.z]
                .map(|c| gray((c * 0.5 + 0.5).clamp(0.0, 1.0)));
            put_rgba(&mut images.normal, idx, [rgb[0], rgb[1], rgb[2], 255]);
            let analytic_rgb = [analytic_normal.x, analytic_normal.y, analytic_normal.z]
                .map(|c| gray((c * 0.5 + 0.5).clamp(0.0, 1.0)));
            put_rgba(
                &mut images.analytic_normal,
                idx,
                [analytic_rgb[0], analytic_rgb[1], analytic_rgb[2], 255],
            );
            put_rgba(
                &mut images.material,
                idx,
                [gray(material[0]), gray(material[1]), gray(material[2]), 255],
            );
            let fourth_material = gray(material[3]);
            put_rgba(
                &mut images.material_4,
                idx,
                [fourth_material, fourth_material, fourth_material, 255],
            );
            put_rgba(
                &mut images.raw_material,
                idx,
                [
                    gray(raw_material[0]),
                    gray(raw_material[1]),
                    gray(raw_material[2]),
                    255,
                ],
            );
            put_rgba(
                &mut images.raw_material_4,
                idx,
                [
                    gray(raw_material[3]),
                    gray(raw_material[3]),
                    gray(raw_material[3]),
                    255,
                ],
            );
            let shape_radius = v[0].shape_radius_m * w[0]
                + v[1].shape_radius_m * w[1]
                + v[2].shape_radius_m * w[2];
            let shape_span = shape_range_m.1 - shape_range_m.0;
            let shape_fraction = if shape_span > 0.0 {
                (shape_radius - shape_range_m.0) / shape_span
            } else {
                0.5
            };
            let shape_gray = gray(shape_fraction.clamp(0.0, 1.0));
            put_rgba(
                &mut images.shape,
                idx,
                [shape_gray, shape_gray, shape_gray, 255],
            );
            let base = (0..4).fold(Vec3::ZERO, |color, channel| {
                color + Vec3::from_array(style.palette[channel]) * material[channel] as f32
            });
            let diffuse = mesh_normal.dot(style.toward_light).max(0.0) as f32;
            let position = v[0].position * w[0] + v[1].position * w[1] + v[2].position * w[2];
            let shadowed = shadow_visibility(position, reference_shadows) as f32;
            let unshadowed =
                (style.ambient_fraction as f32) + (1.0 - style.ambient_fraction as f32) * diffuse;
            let lighting_factor = (style.ambient_fraction as f32)
                + (1.0 - style.ambient_fraction as f32) * diffuse * shadowed;
            let geometry_factor = (style.geometry_ambient_fraction as f32)
                + (1.0 - style.geometry_ambient_fraction as f32) * diffuse * shadowed;
            let shaded = base * lighting_factor;
            let unshadowed_shaded = base * unshadowed;
            let geometry_linear = 0.55 * geometry_factor;
            let geometry_srgb = to_byte(linear_to_srgb(geometry_linear));
            put_rgba(
                &mut images.geometry,
                idx,
                [geometry_srgb, geometry_srgb, geometry_srgb, 255],
            );
            let lighting = gray(f64::from(unshadowed));
            put_rgba(
                &mut images.lighting,
                idx,
                [lighting, lighting, lighting, 255],
            );
            let shadow = gray(f64::from(shadowed));
            put_rgba(
                &mut images.shadow_factor,
                idx,
                [shadow, shadow, shadow, 255],
            );
            put_rgba(
                &mut images.lit,
                idx,
                [
                    to_byte(linear_to_srgb(shaded.x)),
                    to_byte(linear_to_srgb(shaded.y)),
                    to_byte(linear_to_srgb(shaded.z)),
                    255,
                ],
            );
            put_rgba(
                &mut images.unshadowed_lit,
                idx,
                [
                    to_byte(linear_to_srgb(unshadowed_shaded.x)),
                    to_byte(linear_to_srgb(unshadowed_shaded.y)),
                    to_byte(linear_to_srgb(unshadowed_shaded.z)),
                    255,
                ],
            );
        }
    }
}

fn edge(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> f64 {
    (p.0 - a.0) * (b.1 - a.1) - (p.1 - a.1) * (b.0 - a.0)
}
fn gray(x: f64) -> u8 {
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}
fn to_byte(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}
fn linear_to_srgb(x: f32) -> f32 {
    if x <= 0.0031308 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}
fn put_rgba(image: &mut [u8], pixel: usize, color: [u8; 4]) {
    image[pixel * 4..pixel * 4 + 4].copy_from_slice(&color);
}

fn measure_residual(generator: &impl SurfaceQuery, mesh: &Mesh) -> Result<Value> {
    let count = mesh.triangles.len().min(512);
    let mut errors = Vec::with_capacity(count);
    for i in 0..count {
        let ti = ((i * 104729) % mesh.triangles.len()).min(mesh.triangles.len() - 1);
        let tri = mesh.triangles[ti];
        let p = (mesh.vertices[tri[0]].position
            + mesh.vertices[tri[1]].position
            + mesh.vertices[tri[2]].position)
            / 3.0;
        let direction = Direction3::try_new(p)?;
        let exact = generator
            .evaluate_point(SurfaceLocation::new(direction))?
            .radius_m;
        let represented = p.length();
        errors.push((exact - represented).abs());
    }
    let rms = (errors.iter().map(|e| e * e).sum::<f64>() / errors.len() as f64).sqrt();
    Ok(
        json!({"method":"deterministic evenly-strided triangle centroids; exact complete oracle query vs radial height of 3D triangle centroid", "sample_count":errors.len(),"rms_abs_height_error_m":rms,"max_sampled_abs_height_error_m":errors.iter().copied().fold(0.0,f64::max),"certified":false}),
    )
}

fn make_query_corpus(
    config: &MoonTerrainConfig,
    seeds: &[u64],
    version: MoonTerrainVersion,
) -> Result<Value> {
    let mut cases = Vec::new();
    let mut directions = Vec::new();
    for face in CubeFace::ALL {
        let boundary_uvs: [[f64; 2]; 8] = [
            [-1.0, -1.0],
            [-1.0, 0.0],
            [-1.0, 1.0],
            [0.0, -1.0],
            [0.0, 1.0],
            [1.0, -1.0],
            [1.0, 0.0],
            [1.0, 1.0],
        ];
        for uv in boundary_uvs {
            let label = if uv[0].abs() == 1.0 && uv[1].abs() == 1.0 {
                "canonical_cube_corner"
            } else {
                "canonical_cube_edge"
            };
            directions.push((
                format!("{label}_{face:?}_{:.0}_{:.0}", uv[0], uv[1]),
                face.direction(uv)?.unit(),
            ));
        }
    }
    for (name, d) in [
        ("reference_center", DVec3::Z),
        ("reference_east", DVec3::X),
        ("reference_west", -DVec3::X),
        ("reference_north", DVec3::Y),
        ("reference_south", -DVec3::Y),
        ("oblique_1", DVec3::new(1.0, 2.0, 3.0)),
        ("oblique_2", DVec3::new(-2.0, 1.0, 0.5)),
    ] {
        directions.push((name.to_owned(), d));
    }
    for &seed in seeds {
        let definition = MoonTerrainDefinition::with_version(
            TerrainIdentity(IDENTITY),
            TerrainSeed(seed),
            *config,
            version,
        );
        for radius_m in [80_000.0, RADIUS_M, 1_200_000.0] {
            let generator = MoonTerrainGenerator::new(&definition, radius_m)?;
            for (label, d) in &directions {
                let location = SurfaceLocation::new(Direction3::try_new(*d)?);
                let sample = generator.evaluate_point(location)?;
                let terrain = sample.terrain();
                let normal = terrain.normal_body(location, radius_m)?.unit();
                let material = sample.material();
                let g = terrain.tangent_gradient_m_per_unit_direction();
                cases.push(json!({"seed":seed,"radius_m":radius_m,"terrain_identity":IDENTITY,"terrain_version":version_name(generator.version()),"config":config_json(config),"label":label,"direction_body_axes":location.direction().unit().to_array(),"height_m":terrain.height_m(),"analytic_tangent_gradient_m_per_unit_direction":g.to_array(),"analytic_normal_body_axes":normal.to_array(),"material":{"regolith":material.regolith_weight(),"rock":material.rock_weight(),"basalt":material.basalt_weight()}}));
            }
        }
    }
    Ok(
        json!({"schema_version":1,"purpose":"unchanged complete oracle reference corpus for Slice 2 reuse","query_mode":"complete unfiltered point query","cases":cases}),
    )
}

fn config_json(c: &MoonTerrainConfig) -> Value {
    json!({"basin_relief_fraction":c.basin_relief_fraction(),"highland_relief_fraction":c.highland_relief_fraction(),"impact_relief_fractions":c.impact_relief_fractions(),"impact_scale_fractions":c.impact_scale_fractions()})
}

fn clip_triangle_near(
    triangle: [Vertex; 3],
    camera: DVec3,
    forward: DVec3,
    near: f64,
) -> Vec<Vertex> {
    let polygon = triangle.to_vec();
    let mut clipped = Vec::with_capacity(4);
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let da = (a.position - camera).dot(forward) - near;
        let db = (b.position - camera).dot(forward) - near;
        let ina = da >= 0.0;
        let inb = db >= 0.0;
        if ina {
            clipped.push(a);
        }
        if ina != inb {
            let t = da / (da - db);
            clipped.push(interpolate_vertex(a, b, t));
        }
    }
    clipped
}
fn interpolate_vertex(a: Vertex, b: Vertex, t: f64) -> Vertex {
    Vertex {
        position: a.position.lerp(b.position, t),
        mesh_normal: a.mesh_normal.lerp(b.mesh_normal, t).normalize(),
        analytic_normal: a.analytic_normal.lerp(b.analytic_normal, t).normalize(),
        height_m: a.height_m + (b.height_m - a.height_m) * t,
        material: std::array::from_fn(|i| a.material[i] + (b.material[i] - a.material[i]) * t),
        raw_material: std::array::from_fn(|i| {
            a.raw_material[i] + (b.raw_material[i] - a.raw_material[i]) * t
        }),
        shape_radius_m: a.shape_radius_m + (b.shape_radius_m - a.shape_radius_m) * t,
    }
}

fn camera_json(
    camera: Camera,
    generator: &impl SurfaceQuery,
    normal_offset_m: Option<f64>,
) -> Value {
    let position_body = generator.chart_to_body_direction(camera.position);
    let target_body = generator.chart_to_body_direction(camera.target);
    let up_body = generator.chart_to_body_direction(camera.up_hint);
    json!({
        "position_chart_axes_m":camera.position.to_array(),
        "target_chart_axes_m":camera.target.to_array(),
        "up_hint_chart_axes":camera.up_hint.to_array(),
        "position_body_axes_m":position_body.to_array(),
        "target_body_axes_m":target_body.to_array(),
        "up_hint_body_axes":up_body.to_array(),
        "coordinate_basis":"camera vectors are reported in both capture-chart and body-fixed axes",
        "vertical_fov_radians":camera.fov_y_radians,
        "normal_offset_m":normal_offset_m,
        "clearance_above_complete_surface_m":camera.surface_clearance_m,
        "clearance_reference":if camera.surface_clearance_m.is_some() { "complete oracle sampled along the final camera radial direction" } else { "not explicitly controlled" }
    })
}
fn write_json(path: &Path, value: &Value) -> Result<()> {
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
fn write_png(path: &Path, pixels: &[u8]) -> Result<()> {
    write_png_sized(path, WIDTH, HEIGHT, pixels)
}

fn write_png_sized(path: &Path, width: u32, height: u32, pixels: &[u8]) -> Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(fs::File::create(path)?), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(pixels)?;
    Ok(())
}

fn write_index(output: &Path, seeds: &[u64], views: &[View]) -> Result<()> {
    let html = r##"<!doctype html><meta charset="utf-8"><title>Moon terrain Slice 1 reference</title>
<style>body{font:15px system-ui;background:#17191c;color:#eee;margin:24px}h1{font-size:1.5rem}.note{color:#c9c1ac;max-width:900px}select{font:inherit;padding:6px;background:#262a30;color:#fff}.grid{display:grid;grid-template-columns:110px repeat(3,minmax(250px,1fr));gap:10px;align-items:start;margin-top:18px}.head,.seed{font-weight:650;padding:8px}.cell{background:#22262b;padding:8px;border-radius:5px}.cell img{width:100%;display:block;background:#090a0b}.links{display:flex;gap:8px;margin-top:6px;flex-wrap:wrap}.links a{color:#b8d5ff}.meta{font-size:12px;color:#aaa;margin-top:5px}@media(max-width:850px){.grid{grid-template-columns:80px 1fr}}</style>
<h1>Moon terrain reference · temporary software rasterizer</h1><p class="note">Fixed composition across selected seeds and views. Lighting uses normals reconstructed from displaced triangles; CPU rays test the represented displaced mesh; local crops omit casters outside that mesh. unshadowed_lit.png and shadow_factor.png separate shadowing from terrain and albedo. analytic_normal.png and raw_material.png preserve complete oracle samples and can show aliasing in coarse views. Orbit/regional material uses a 2×2 complete-oracle footprint average; near remains unfiltered. These captures are not native/GPU performance or visual acceptance evidence.</p>
<label>Diagnostic <select id="kind"><option>lit</option><option>unshadowed_lit</option><option>lighting</option><option>shadow_factor</option><option>height</option><option>normal</option><option>analytic_normal</option><option>material</option><option>raw_material</option></select></label><div class="grid" id="grid"></div>
<script>const scenes=__VIEWS__,seeds=__SEEDS__,kind=document.querySelector("#kind"),grid=document.querySelector("#grid");function draw(){grid.innerHTML="<div></div>"+scenes.map(x=>`<div class=head>${x}</div>`).join("");for(const s of seeds){grid.insertAdjacentHTML("beforeend",`<div class=seed>Seed ${s}</div>`);for(const v of scenes){const d=`seed-${s}-${v}`,src=`${d}/${kind.value}.png`;grid.insertAdjacentHTML("beforeend",`<div class=cell><a href="${src}"><img loading=lazy src="${src}" alt="${d} ${kind.value}"></a><div class=links>${["lit","unshadowed_lit","lighting","shadow_factor","height","normal","analytic_normal","material","raw_material"].map(k=>`<a href="${d}/${k}.png">${k}</a>`).join("")}<a href="${d}/metadata.json">metadata</a></div></div>`)}}}kind.onchange=draw;draw();</script>"##;
    let html = html
        .replace(
            "__VIEWS__",
            &serde_json::to_string(&views.iter().map(|view| view.name).collect::<Vec<_>>())?,
        )
        .replace("__SEEDS__", &serde_json::to_string(seeds)?);
    fs::write(output.join("index.html"), html)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RotatedLightFixture(glam::DQuat);

    impl SurfaceQuery for RotatedLightFixture {
        fn radius_m(&self) -> f64 {
            RADIUS_M
        }
        fn radial_envelope_m(&self) -> [f64; 2] {
            [RADIUS_M, RADIUS_M]
        }
        fn absolute_height_bound_m(&self) -> f64 {
            0.0
        }
        fn evaluate_point(&self, _location: SurfaceLocation) -> anyhow::Result<ReferenceSample> {
            unreachable!("lighting fixture only tests coordinate conversion")
        }
        fn chart_to_body_direction(&self, chart_direction: DVec3) -> DVec3 {
            self.0 * chart_direction
        }
    }

    #[test]
    fn rotated_landmark_keeps_view_light_in_capture_chart_axes() {
        let rotation = glam::DQuat::from_rotation_y(0.73) * glam::DQuat::from_rotation_x(-0.31);
        let fixture = RotatedLightFixture(rotation);
        let view = View {
            name: "near",
            cells: 256,
            kind: ViewKind::Local {
                side_m: 128.0,
                altitude_m: 4.0,
                fov_y_degrees: 82.0,
            },
        };
        let (chart, body) = capture_light_directions(&fixture, view, false);
        assert!((chart - light_for(view)).length() < 1e-15);
        assert!((rotation.conjugate() * body - chart).length() < 1e-12);
    }

    fn sv(x: f64, y: f64, z: f64, height: f64) -> ScreenVertex {
        ScreenVertex {
            x,
            y,
            depth: z,
            position: DVec3::ZERO,
            mesh_normal: DVec3::Z,
            analytic_normal: DVec3::Z,
            height_m: height,
            material: [1.0, 0.0, 0.0, 0.0],
            raw_material: [1.0, 0.0, 0.0, 0.0],
            shape_radius_m: 109_000.0,
        }
    }

    fn empty_reference_shadows() -> ReferenceShadows {
        ReferenceShadows {
            triangles: mesh_shadow::TriangleShadows::new(&[], &[]).unwrap(),
            toward_light: DVec3::Z,
            build_wall_ms: 0.0,
        }
    }

    fn vertex(position: DVec3) -> Vertex {
        Vertex {
            position,
            mesh_normal: DVec3::Z,
            analytic_normal: DVec3::Z,
            height_m: position.z,
            material: [1.0, 0.0, 0.0, 0.0],
            raw_material: [1.0, 0.0, 0.0, 0.0],
            shape_radius_m: 109_000.0,
        }
    }

    #[test]
    fn reference_cli_selects_seed_view_and_orbit_density() {
        let options = parse_options(
            [
                "temporary-output",
                "--seed",
                "7",
                "--view",
                "orbit",
                "--orbit-cells",
                "256",
            ]
            .into_iter()
            .map(std::ffi::OsString::from),
        )
        .unwrap();
        assert_eq!(options.output, PathBuf::from("temporary-output"));
        assert_eq!(options.seed, Some(7));
        assert_eq!(options.view.as_deref(), Some("orbit"));
        assert_eq!(options.orbit_cells, 256);
    }

    #[test]
    fn province_local_scale_selects_the_camera_regime_and_drives_its_dimensions() {
        let close = province_view(
            "province-2",
            128,
            &json!({"local_scale":{"side_m":320.0,"altitude_m":7.5,"fov_y_degrees":68.0}}),
        )
        .unwrap();
        assert!(is_near_view(close));
        assert_eq!(family_camera_query_count(close), 5);
        assert!(matches!(
            close.kind,
            ViewKind::Local {
                side_m: 320.0,
                altitude_m: 7.5,
                fov_y_degrees: 68.0
            }
        ));

        let broad = province_view(
            "province-2",
            128,
            &json!({"local_scale":{"side_m":18_000.0,"altitude_m":9_000.0,"fov_y_degrees":54.0}}),
        )
        .unwrap();
        assert!(!is_near_view(broad));
        assert_eq!(family_camera_query_count(broad), 1);
        assert!(matches!(
            broad.kind,
            ViewKind::Local {
                side_m: 18_000.0,
                altitude_m: 9_000.0,
                fov_y_degrees: 54.0
            }
        ));
        assert!(
            province_view(
                "province-0",
                8,
                &json!({"local_scale":{"side_m":0,"altitude_m":1,"fov_y_degrees":45}})
            )
            .is_err()
        );
    }

    #[test]
    fn rasterizer_uses_perspective_correct_attributes_and_nearest_depth() {
        let mut images = Images {
            lit: vec![0; (WIDTH * HEIGHT * 4) as usize],
            unshadowed_lit: vec![0; (WIDTH * HEIGHT * 4) as usize],
            lighting: vec![0; (WIDTH * HEIGHT * 4) as usize],
            shadow_factor: vec![0; (WIDTH * HEIGHT * 4) as usize],
            height: vec![0; (WIDTH * HEIGHT * 4) as usize],
            normal: vec![0; (WIDTH * HEIGHT * 4) as usize],
            analytic_normal: vec![0; (WIDTH * HEIGHT * 4) as usize],
            material: vec![0; (WIDTH * HEIGHT * 4) as usize],
            raw_material: vec![0; (WIDTH * HEIGHT * 4) as usize],
            material_4: vec![0; (WIDTH * HEIGHT * 4) as usize],
            raw_material_4: vec![0; (WIDTH * HEIGHT * 4) as usize],
            shape: vec![0; (WIDTH * HEIGHT * 4) as usize],
            geometry: vec![0; (WIDTH * HEIGHT * 4) as usize],
            depth: vec![f64::INFINITY; (WIDTH * HEIGHT) as usize],
            covered_pixels: 0,
        };
        let style = RenderStyle {
            palette: MOON_PALETTE,
            toward_light: DVec3::Z,
            ambient_fraction: AMBIENT_FRACTION,
            geometry_ambient_fraction: 0.30,
        };
        // At pixel (0.5,0.5), screen barycentrics are (0.5,0.25,0.25).
        // Reciprocal-depth interpolation gives 1000/6, not the affine 250.
        raster_triangle(
            &mut images,
            [
                sv(0.0, 0.0, 1.0, 0.0),
                sv(2.0, 0.0, 2.0, 1000.0),
                sv(0.0, 2.0, 2.0, 0.0),
            ],
            (-2000.0, 2000.0),
            (109_000.0, 109_000.0),
            style,
            &empty_reference_shadows(),
        );
        let pixel = 0;
        let encoded = images.height[pixel * 4];
        let recovered = (f64::from(encoded) / 255.0 * 2.0 - 1.0) * 2000.0;
        assert!((recovered - 1000.0 / 6.0).abs() < 20.0);
        raster_triangle(
            &mut images,
            [
                sv(0.0, 0.0, 8.0, 1800.0),
                sv(2.0, 0.0, 8.0, 1800.0),
                sv(0.0, 2.0, 8.0, 1800.0),
            ],
            (-2000.0, 2000.0),
            (109_000.0, 109_000.0),
            style,
            &empty_reference_shadows(),
        );
        assert!(images.depth[pixel] < 8.0);

        let geometry_before = images.geometry.clone();
        images.depth.fill(f64::INFINITY);
        let mut material_variant = [
            sv(0.0, 0.0, 1.0, 0.0),
            sv(2.0, 0.0, 2.0, 1000.0),
            sv(0.0, 2.0, 2.0, 0.0),
        ];
        for vertex in &mut material_variant {
            vertex.material = [0.0, 0.0, 0.0, 1.0];
            vertex.raw_material = vertex.material;
        }
        let mut family_style = style;
        family_style.palette = [[0.1, 0.8, 0.3]; 4];
        raster_triangle(
            &mut images,
            material_variant,
            (-2000.0, 2000.0),
            (109_000.0, 109_000.0),
            family_style,
            &empty_reference_shadows(),
        );
        assert_eq!(images.geometry, geometry_before);
    }

    #[test]
    fn canonical_cube_face_duplicates_share_triangle_derived_normals() {
        let edge_x = CubeFace::PositiveX.direction([-1.0, 0.25]).unwrap().unit();
        let edge_z = CubeFace::PositiveZ.direction([1.0, 0.25]).unwrap().unit();
        let corner_x = CubeFace::PositiveX.direction([-1.0, 1.0]).unwrap().unit();
        let corner_y = CubeFace::PositiveY.direction([1.0, -1.0]).unwrap().unit();
        let corner_z = CubeFace::PositiveZ.direction([1.0, 1.0]).unwrap().unit();
        let edge_keys = [direction_key(edge_x), direction_key(edge_z)];
        let edge_normals = reconcile_duplicate_normals(&edge_keys, &[DVec3::X, DVec3::Y]);
        assert_eq!(edge_normals[0], edge_normals[1]);
        let corner_keys = [
            direction_key(corner_x),
            direction_key(corner_y),
            direction_key(corner_z),
        ];
        let corner_normals =
            reconcile_duplicate_normals(&corner_keys, &[DVec3::X, DVec3::Y, DVec3::Z]);
        assert_eq!(corner_normals[0], corner_normals[1]);
        assert_eq!(corner_normals[1], corner_normals[2]);
    }

    #[test]
    fn material_footprint_filter_is_bounded_and_near_stays_complete() {
        let config = MoonTerrainConfig::default();
        let definition =
            MoonTerrainDefinition::new(TerrainIdentity(IDENTITY), TerrainSeed(2), config);
        let generator = MoonTerrainGenerator::new(&definition, RADIUS_M).unwrap();
        let locations = [
            SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap()),
            SurfaceLocation::new(Direction3::try_new(DVec3::new(1.0, 0.0, 1.0)).unwrap()),
        ];
        let raw = locations
            .iter()
            .map(|location| {
                let material = generator.evaluate_point(*location).unwrap().material();
                [
                    material.regolith_weight(),
                    material.rock_weight(),
                    material.basalt_weight(),
                    0.0,
                ]
            })
            .collect::<Vec<_>>();
        let (filtered, queries, samples_per_vertex) =
            filter_materials(&generator, &locations, 52.0, VIEWS[1], &raw).unwrap();
        assert_eq!(samples_per_vertex, 4);
        assert_eq!(queries, locations.len() * 4 + 1);
        for weights in &filtered {
            assert!(
                weights
                    .iter()
                    .all(|weight| weight.is_finite() && (0.0..=1.0).contains(weight))
            );
            assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-10);
        }
        let (near, near_queries, near_samples) =
            filter_materials(&generator, &locations, 0.333, VIEWS[2], &raw).unwrap();
        assert_eq!(near, raw);
        assert_eq!(near_queries, 0);
        assert_eq!(near_samples, 1);

        let mut near_landmark = VIEWS[2];
        near_landmark.name = "near-landmark";
        let (near_landmark_material, landmark_queries, landmark_samples) =
            filter_materials(&generator, &locations, 0.333, near_landmark, &raw).unwrap();
        assert_eq!(near_landmark_material, raw);
        assert_eq!(landmark_queries, 0);
        assert_eq!(landmark_samples, 1);
        assert_eq!(family_camera_query_count(VIEWS[2]), 5);
    }

    #[test]
    fn legacy_moon_camera_targets_keep_the_reference_sphere() {
        let config = MoonTerrainConfig::default();
        for version in [
            MoonTerrainVersion::MoonLikeV1,
            MoonTerrainVersion::MoonLikeV2,
        ] {
            let definition = MoonTerrainDefinition::with_version(
                TerrainIdentity(IDENTITY),
                TerrainSeed(7),
                config,
                version,
            );
            let generator = MoonTerrainGenerator::new(&definition, RADIUS_M).unwrap();
            for view in VIEWS {
                let camera = camera_for(view, &generator).unwrap();
                assert_eq!(camera.target, DVec3::Z * RADIUS_M);
            }
        }
    }

    #[test]
    fn compositional_camera_targets_use_the_complete_surface() {
        use mundaris_world::terrain::{
            SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
        };

        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x4341_4d45_5241),
            TerrainSeed(7),
            SurfaceAlgorithm::RockyV3,
        );
        let generator = SurfaceGenerator::new(&definition, RADIUS_M).unwrap();
        let location = SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap());
        let complete_radius = generator.evaluate_point(location).unwrap().radius_m();
        let camera = camera_for(VIEWS[1], &generator).unwrap();
        assert_eq!(camera.target, DVec3::Z * complete_radius);
    }

    #[test]
    fn family_camera_applies_requested_local_altitude_and_fov() {
        use mundaris_world::terrain::{
            SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
        };

        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x4341_4d45_5241),
            TerrainSeed(7),
            SurfaceAlgorithm::RockyV3,
        );
        let generator = SurfaceGenerator::new(&definition, RADIUS_M).unwrap();
        let close = province_view(
            "province-0-scale-320",
            32,
            &json!({"local_scale":{"side_m":320.0,"altitude_m":7.5,"fov_y_degrees":68.0}}),
        )
        .unwrap();
        let (close_camera, close_offset) = family_camera_for(close, &generator).unwrap();
        assert_eq!(close_offset, Some(7.5));
        assert!((close_camera.fov_y_radians.to_degrees() - 68.0).abs() < 1e-12);
        assert!(close_camera.surface_clearance_m.unwrap() > 0.0);

        let regional = province_view(
            "province-0-scale-20000",
            32,
            &json!({"local_scale":{"side_m":20_000.0,"altitude_m":12_000.0,"fov_y_degrees":54.0}}),
        )
        .unwrap();
        let (regional_camera, regional_offset) = family_camera_for(regional, &generator).unwrap();
        assert_eq!(regional_offset, None);
        assert_eq!(regional_camera.surface_clearance_m, None);
        assert!((regional_camera.fov_y_radians.to_degrees() - 54.0).abs() < 1e-12);
        let target_radius = generator
            .view_target_radius_m(
                SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap()),
                false,
            )
            .unwrap();
        assert_eq!(regional_camera.position.z, target_radius + 12_000.0);
    }

    #[test]
    fn directional_reference_shadows_fits_mesh_and_blocks_receiver_behind_raised_geometry() {
        let mesh = Mesh {
            vertices: vec![
                vertex(DVec3::new(-2.0, -2.0, 0.0)),
                vertex(DVec3::new(2.0, -2.0, 0.0)),
                vertex(DVec3::new(0.0, 2.0, 0.0)),
                vertex(DVec3::new(-0.5, -0.5, 10.0)),
                vertex(DVec3::new(0.5, -0.5, 10.0)),
                vertex(DVec3::new(0.0, 0.5, 10.0)),
            ],
            triangles: vec![[0, 1, 2], [3, 4, 5]],
            spacing_m: 1.0,
            coverage_description: "shadow test".to_owned(),
            material_filter_queries: 0,
            material_filter_wall_ms: 0.0,
            material_filter_samples_per_vertex: 1,
            query_work: [0; 2],
            accepted_features: Some(0),
            gradient_norm_bounds: [0.0; 2],
        };
        let shadow = build_reference_shadows(&mesh, DVec3::Z).unwrap();
        let (minimum, maximum) = shadow.triangles.bounds();
        assert!(minimum.x <= -2.0 && minimum.y <= -2.0);
        assert!(maximum.z >= 10.0);
        assert_eq!(shadow.triangles.triangle_count(), 2);
        assert_eq!(shadow_visibility(DVec3::ZERO, &shadow), 0.0);
        assert_eq!(shadow_visibility(DVec3::new(0.0, 1.5, 0.0), &shadow), 1.0);
    }

    #[test]
    fn shadow_rays_keep_a_sloped_surface_unshadowed() {
        let plane = |x, y| DVec3::new(x, y, 7.0 * x - 3.0 * y);
        let mesh = Mesh {
            vertices: vec![
                vertex(plane(-1000.0, -1000.0)),
                vertex(plane(1000.0, -1000.0)),
                vertex(plane(1000.0, 1000.0)),
                vertex(plane(-1000.0, 1000.0)),
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            spacing_m: 100.0,
            coverage_description: "sloped receiver".to_owned(),
            material_filter_queries: 0,
            material_filter_wall_ms: 0.0,
            material_filter_samples_per_vertex: 1,
            query_work: [0; 2],
            accepted_features: Some(0),
            gradient_norm_bounds: [0.0; 2],
        };
        let map = build_reference_shadows(&mesh, DVec3::Z).unwrap();
        for i in 0..97 {
            let x = -900.0 + i as f64 * 18.312;
            let y = 700.0 - i as f64 * 13.791;
            assert_eq!(shadow_visibility(plane(x, y), &map), 1.0);
        }
    }

    #[test]
    fn near_plane_clipping_keeps_the_visible_part_of_a_crossing_triangle() {
        let v = [
            Vertex {
                position: DVec3::new(0.0, 0.0, 0.0),
                mesh_normal: DVec3::Z,
                analytic_normal: DVec3::Z,
                height_m: 0.0,
                material: [1.0, 0.0, 0.0, 0.0],
                raw_material: [1.0, 0.0, 0.0, 0.0],
                shape_radius_m: 109_000.0,
            },
            Vertex {
                position: DVec3::new(1.0, 0.0, 2.0),
                mesh_normal: DVec3::Z,
                analytic_normal: DVec3::Z,
                height_m: 2.0,
                material: [1.0, 0.0, 0.0, 0.0],
                raw_material: [1.0, 0.0, 0.0, 0.0],
                shape_radius_m: 109_000.0,
            },
            Vertex {
                position: DVec3::new(0.0, 1.0, 2.0),
                mesh_normal: DVec3::Z,
                analytic_normal: DVec3::Z,
                height_m: 2.0,
                material: [1.0, 0.0, 0.0, 0.0],
                raw_material: [1.0, 0.0, 0.0, 0.0],
                shape_radius_m: 109_000.0,
            },
        ];
        let clipped = clip_triangle_near(v, DVec3::ZERO, DVec3::Z, 1.0);
        assert_eq!(clipped.len(), 4);
        assert!(clipped.iter().all(|p| p.position.z >= 1.0));
    }
}
