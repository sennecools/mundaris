//! Rank deterministic body-local Moon viewpoints against the exact profile source.
//!
//! This is an offline composition aid. Its PNG is a software projection of a
//! finite exact-source mesh, not a native renderer capture or terrain guarantee.
//!
//! Usage: cargo run --locked -p mundaris_app --features developer-tools --example phase2g_vista -- <root.r16> <new-output-directory> [--detail-profile <r16> <footprint_m> <amplitude_m>] [--radius <m>]

use anyhow::{Context, Result, ensure};
use glam::{DMat3, DQuat, DVec3};
use mundaris_app::terrain_profile::load_height_profile;
use mundaris_math::surface::{CubeFace, SurfaceLocation};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};

const DEFAULT_RADIUS_M: f64 = mundaris_app::solar_system::SOLAR_SYSTEM_CONTENT[4]
    .real_mean_radius_m
    * mundaris_app::solar_system::SolarSystemPreset::gameplay().body_radius_scale;
const ROOT_FOOTPRINT_M: f64 = 4_000.0;
const ROOT_AMPLITUDE_M: f64 = 600.0;
const SEARCH_HALF_EXTENT_M: f64 = 2_000.0;
const ANCHOR_STEPS: usize = 17;
const HEADING_COUNT: usize = 16;
const CAMERA_PITCH_DEGREES: f64 = 3.0;
const CLEARANCE_M: f64 = 2.0;
const RAY_ANGLES_DEGREES: [f64; 9] = [-48.0, -36.0, -24.0, -12.0, 0.0, 12.0, 24.0, 36.0, 48.0];
const RAY_DISTANCES_M: [f64; 19] = [
    10.0, 20.0, 35.0, 50.0, 75.0, 100.0, 150.0, 250.0, 400.0, 650.0, 950.0, 1_300.0, 1_700.0,
    2_000.0, 3_000.0, 4_000.0, 6_000.0, 8_000.0, 12_000.0,
];
const PREVIEW_CELLS: usize = 256;
const PREVIEW_WIDTH: usize = 1_580;
const PREVIEW_HEIGHT: usize = 826;
const PREVIEW_VERTICAL_FOV_DEGREES: f64 = 60.0;
const PREVIEW_HALF_EXTENT_M: f64 = 12_000.0;

#[derive(Debug)]
struct DetailInput {
    path: PathBuf,
    footprint_m: f64,
    amplitude_m: f64,
}

#[derive(Debug, Clone, Copy)]
struct SurfacePoint {
    position_body_m: DVec3,
    radial_up: DVec3,
    surface_normal: DVec3,
}

#[derive(Debug)]
struct Candidate {
    grid_x: usize,
    grid_y: usize,
    tangent_x_m: f64,
    tangent_y_m: f64,
    heading_index: usize,
    heading_radians: f64,
    score: f64,
    foreground_open_fraction: f64,
    foreground_open_margin_mean_m: f64,
    ridge_ray_fraction: f64,
    connected_ridge_fraction: f64,
    distant_skyline_variation_radians: f64,
    surface_slope_radians: f64,
    slope_suitability: f64,
    skyline_suitability: f64,
    skyline_radians_by_ray: Vec<f64>,
    skyline_distance_m_by_ray: Vec<f64>,
    position_body_m: DVec3,
    radial_up: DVec3,
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let root_path = args
        .next()
        .map(PathBuf::from)
        .context("usage: phase2g_vista <root.r16> <new-output-directory> [--detail-profile <r16> <footprint_m> <amplitude_m>] [--radius <m>]")?;
    let output_dir = args
        .next()
        .map(PathBuf::from)
        .context("missing new output directory")?;
    let mut detail_inputs = Vec::new();
    let mut radius_m = DEFAULT_RADIUS_M;
    while let Some(flag) = args.next() {
        if flag == "--radius" {
            radius_m = parse_f64(args.next(), "missing radius in metres")?;
            ensure!(
                radius_m.is_finite() && radius_m > 0.0,
                "radius must be finite and positive"
            );
        } else {
            ensure!(
                flag == "--detail-profile",
                "expected --detail-profile <r16> <footprint_m> <amplitude_m> or --radius <m>"
            );
            let path = args
                .next()
                .map(PathBuf::from)
                .context("missing detail profile path")?;
            let footprint_m = parse_f64(args.next(), "missing detail footprint in metres")?;
            let amplitude_m = parse_f64(args.next(), "missing detail amplitude in metres")?;
            ensure!(
                footprint_m.is_finite() && footprint_m > 0.0,
                "detail footprint must be finite and positive"
            );
            ensure!(amplitude_m.is_finite(), "detail amplitude must be finite");
            detail_inputs.push(DetailInput {
                path,
                footprint_m,
                amplitude_m,
            });
        }
    }
    ensure!(
        !output_dir.exists(),
        "refusing to replace existing output directory {}",
        output_dir.display()
    );

    let mut profile = load_height_profile(&root_path)?
        .with_cubic_bspline()
        .with_terrain_scale(ROOT_FOOTPRINT_M, ROOT_AMPLITUDE_M)?;
    let root_profile_dimensions = [profile.width(), profile.height()];
    let root_profile_sample_range = profile.sample_range();
    for input in &detail_inputs {
        let detail = load_height_profile(&input.path)?.with_cubic_bspline();
        profile = profile.with_detail_layer(detail, input.footprint_m, input.amplitude_m)?;
    }
    let composed_profile_identity_words = profile.identity_words();
    let composed_profile_dimensions = [profile.width(), profile.height()];
    let composed_profile_sample_range = profile.sample_range();
    let profile_kernel = profile.kernel_name();
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(0x4d4f_4f4e),
        TerrainSeed(2),
        SurfaceAlgorithm::MoonProfileV1,
    )
    .with_height_profile(profile)?;
    let generator = SurfaceGenerator::new(&definition, radius_m)?;
    let mut point_cache = BTreeMap::<(u64, u64), SurfacePoint>::new();
    let mut candidates = Vec::with_capacity(ANCHOR_STEPS * ANCHOR_STEPS * HEADING_COUNT);

    for grid_y in 0..ANCHOR_STEPS {
        for grid_x in 0..ANCHOR_STEPS {
            let tangent_x_m = grid_coordinate(grid_x);
            let tangent_y_m = grid_coordinate(grid_y);
            let origin = sample_tangent_point(
                &generator,
                radius_m,
                tangent_x_m,
                tangent_y_m,
                &mut point_cache,
            )?;
            for heading_index in 0..HEADING_COUNT {
                let heading_radians =
                    std::f64::consts::TAU * heading_index as f64 / HEADING_COUNT as f64;
                candidates.push(rank_candidate(
                    &generator,
                    radius_m,
                    &mut point_cache,
                    grid_x,
                    grid_y,
                    tangent_x_m,
                    tangent_y_m,
                    origin,
                    heading_index,
                    heading_radians,
                )?);
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.grid_y.cmp(&b.grid_y))
            .then_with(|| a.grid_x.cmp(&b.grid_x))
            .then_with(|| a.heading_index.cmp(&b.heading_index))
    });
    let selected = candidates
        .first()
        .context("candidate search produced no poses")?;
    fs::create_dir_all(&output_dir)?;

    let position_body_m = selected.position_body_m + selected.radial_up * CLEARANCE_M;
    let (right, camera_up, forward) = camera_basis(
        selected.radial_up,
        selected.heading_radians,
        CAMERA_PITCH_DEGREES.to_radians(),
    );
    let orientation = DQuat::from_mat3(&DMat3::from_cols(right, camera_up, -forward)).normalize();
    let pose = json!({
        "schema": 1,
        "radius_m": radius_m,
        "position_body_m": position_body_m.to_array(),
        "orientation_xyzw": [orientation.x, orientation.y, orientation.z, orientation.w],
        "clearance_m": CLEARANCE_M,
        "source": {
            "algorithm": "MoonProfileV1", "seed": 2, "kernel": "bspline",
            "sampler": profile_kernel,
            "root_sha256": sha256_file(&root_path)?,
            "root_footprint_m": ROOT_FOOTPRINT_M, "root_amplitude_m": ROOT_AMPLITUDE_M,
            "profile_identity_words": composed_profile_identity_words,
            "detail_layers": detail_inputs.iter().map(|input| Ok(json!({
                "sha256": sha256_file(&input.path)?,
                "footprint_m": input.footprint_m, "amplitude_m": input.amplitude_m
            }))).collect::<Result<Vec<Value>>>()?
        }
    });
    fs::write(
        output_dir.join("pose.json"),
        serde_json::to_vec_pretty(&pose)?,
    )?;

    let preview_name = "vista_source_preview.png";
    write_perspective_preview(
        &output_dir.join(preview_name),
        &generator,
        radius_m,
        selected,
        position_body_m,
        [right, camera_up, forward],
        &mut point_cache,
    )?;

    let ranked_json = json!({
        "schema": 1,
        "experiment": "offline deterministic exact-source Moon vista selection",
        "status": "finite source samples used for viewpoint ranking; no global terrain or quality guarantee",
        "source_profile": {
            "algorithm": "MoonProfileV1",
            "seed": 2,
            "radius_m": radius_m,
            "kernel": profile_kernel,
            "root_profile": {
                "identity_source": "SHA-256 of raw big-endian u16 input bytes plus canonical composed profile identity below",
                "path": root_path,
                "sha256_raw_file": sha256_file(&root_path)?,
                "dimensions": root_profile_dimensions,
                "sample_range": root_profile_sample_range,
                "footprint_m": ROOT_FOOTPRINT_M,
                "amplitude_m": ROOT_AMPLITUDE_M,
                "physical_scale": true
            },
            "detail_profiles": detail_inputs.iter().map(|input| Ok(json!({
                "path": input.path,
                "sha256_raw_file": sha256_file(&input.path)?,
                "footprint_m": input.footprint_m,
                "amplitude_m": input.amplitude_m,
                "dimensions": [load_height_profile(&input.path)?.width(), load_height_profile(&input.path)?.height()],
                "physical_scale": true
            }))).collect::<Result<Vec<Value>>>()?,
            "composed_profile": {
                "identity_words": composed_profile_identity_words,
                "dimensions": composed_profile_dimensions,
                "sample_range": composed_profile_sample_range,
                "identity_note": "world content identity after canonical little-endian sample loading, cubic B-spline filtering, physical root calibration, and ordered detail composition"
            },
            "definition": {"terrain_identity": "0x4d4f4f4e", "surface_algorithm": "MoonProfileV1", "terrain_seed": 2}
        },
        "selection": {
            "region": "PositiveZ tangent chart centered at [0,0,1], anchors in +/-2000 m chart coordinates",
            "anchor_grid": [ANCHOR_STEPS, ANCHOR_STEPS],
            "heading_count": HEADING_COUNT,
            "camera_pitch_down_degrees": CAMERA_PITCH_DEGREES,
            "camera_clearance_m": CLEARANCE_M,
            "ray_fan_angles_degrees": RAY_ANGLES_DEGREES,
            "ray_distances_m": RAY_DISTANCES_M,
            "score_weights": {"foreground_open_fraction": 0.40, "connected_ridge_fraction": 0.28, "ridge_ray_fraction": 0.14, "distant_skyline_variation_normalized": 0.08, "slope_suitability": 0.10},
            "skyline_weight": "multiply score by 0.25+0.75*skyline_suitability; prefer an open distant vista over a nearby wall",
            "candidate_count": candidates.len(),
            "source_sample_cache_unique_points": point_cache.len(),
            "selected": candidate_json(selected),
            "candidates_ranked": candidates.iter().enumerate().map(|(rank, candidate)| {
                let mut item = candidate_json(candidate);
                item["rank"] = json!(rank + 1);
                item
            }).collect::<Vec<_>>()
        },
        "preview": {
            "file": preview_name,
            "source": "exact SurfaceGenerator::evaluate_point samples triangulated into one body-local mesh",
            "sample_grid": [PREVIEW_CELLS + 1, PREVIEW_CELLS + 1],
            "grid_spacing": "cubic chart offsets concentrate vertices near the observer; spacing varies across the footprint",
            "physical_extent_m": [PREVIEW_HALF_EXTENT_M * 2.0, PREVIEW_HALF_EXTENT_M * 2.0],
            "projection": "perspective depth-tested CPU triangle rasterizer",
            "viewport_pixels": [PREVIEW_WIDTH, PREVIEW_HEIGHT],
            "vertical_fov_degrees": PREVIEW_VERTICAL_FOV_DEGREES,
            "limitation": "offline source-mesh preview only; finite mesh, simple neutral grayscale light, no native LOD, materials, shadows, GPU path, runtime interaction, or visual acceptance"
        },
        "units": {"distance": "metres", "angle_in_json": "radians unless field name says degrees", "position": "body-local metres", "quaternion": "xyzw; camera-local +X right, +Y up, -Z forward"},
        "limitations": [
            "finite sampled rays rank the supplied source only and do not certify unsampled terrain",
            "vista score is a deterministic heuristic, not an acceptance criterion",
            "the preview omits native renderer LOD, materials, lighting, temporal behavior, and ordinary-Moon integration"
        ]
    });
    fs::write(
        output_dir.join("candidate_metrics.json"),
        serde_json::to_vec_pretty(&ranked_json)?,
    )?;
    Ok(())
}

fn parse_f64(value: Option<std::ffi::OsString>, message: &'static str) -> Result<f64> {
    Ok(value.context(message)?.to_string_lossy().parse::<f64>()?)
}

fn grid_coordinate(index: usize) -> f64 {
    -SEARCH_HALF_EXTENT_M + 2.0 * SEARCH_HALF_EXTENT_M * index as f64 / (ANCHOR_STEPS - 1) as f64
}

fn sample_tangent_point(
    generator: &SurfaceGenerator,
    radius_m: f64,
    tangent_x_m: f64,
    tangent_y_m: f64,
    cache: &mut BTreeMap<(u64, u64), SurfacePoint>,
) -> Result<SurfacePoint> {
    let key = (tangent_x_m.to_bits(), tangent_y_m.to_bits());
    if let Some(point) = cache.get(&key) {
        return Ok(*point);
    }
    let direction =
        CubeFace::PositiveZ.direction([tangent_x_m / radius_m, tangent_y_m / radius_m])?;
    let location = SurfaceLocation::new(direction);
    let sample = generator.evaluate_point(location)?;
    let point = SurfacePoint {
        position_body_m: sample.position(location),
        radial_up: direction.unit(),
        surface_normal: sample.normal(),
    };
    cache.insert(key, point);
    Ok(point)
}

#[allow(clippy::too_many_arguments)]
fn rank_candidate(
    generator: &SurfaceGenerator,
    radius_m: f64,
    cache: &mut BTreeMap<(u64, u64), SurfacePoint>,
    grid_x: usize,
    grid_y: usize,
    tangent_x_m: f64,
    tangent_y_m: f64,
    origin: SurfacePoint,
    heading_index: usize,
    heading_radians: f64,
) -> Result<Candidate> {
    let (right, _, _) = camera_basis(origin.radial_up, heading_radians, 0.0);
    let (_, _, horizontal_forward) = camera_basis(origin.radial_up, heading_radians, 0.0);
    let mut open_samples = 0usize;
    let mut open_count = 0usize;
    let mut open_margin_sum = 0.0;
    let mut skyline_radians_by_ray = Vec::with_capacity(RAY_ANGLES_DEGREES.len());
    let mut skyline_distance_m_by_ray = Vec::with_capacity(RAY_ANGLES_DEGREES.len());
    let mut ridge_flags = Vec::with_capacity(RAY_ANGLES_DEGREES.len());
    for ray_angle_degrees in RAY_ANGLES_DEGREES {
        let ray_angle = ray_angle_degrees.to_radians();
        let ray_forward =
            (horizontal_forward * ray_angle.cos() + right * ray_angle.sin()).normalize();
        let mut skyline_angle = f64::NEG_INFINITY;
        let mut skyline_distance = 0.0;
        for distance_m in RAY_DISTANCES_M {
            let chart_x = tangent_x_m + ray_forward.x * distance_m;
            let chart_y = tangent_y_m + ray_forward.y * distance_m;
            let point = sample_tangent_point(generator, radius_m, chart_x, chart_y, cache)?;
            let delta = point.position_body_m - origin.position_body_m;
            let forward_distance_m = delta.dot(ray_forward).max(0.0);
            let elevation_m = delta.dot(origin.radial_up) - CLEARANCE_M;
            let elevation_angle = elevation_m.atan2(forward_distance_m.max(1.0e-6));
            if distance_m >= 100.0 && elevation_angle > skyline_angle {
                skyline_angle = elevation_angle;
                skyline_distance = distance_m;
            }
            if distance_m <= 50.0 {
                let tangent_ray_height_m = -CAMERA_PITCH_DEGREES.to_radians().tan() * distance_m;
                let margin_m = tangent_ray_height_m - elevation_m;
                open_samples += 1;
                open_margin_sum += margin_m.max(0.0);
                if margin_m > 0.0 {
                    open_count += 1;
                }
            }
        }
        skyline_radians_by_ray.push(if skyline_angle.is_finite() {
            skyline_angle
        } else {
            0.0
        });
        skyline_distance_m_by_ray.push(skyline_distance);
        ridge_flags.push(skyline_angle > 0.5_f64.to_radians());
    }
    let foreground_open_fraction = open_count as f64 / open_samples.max(1) as f64;
    let foreground_open_margin_mean_m = open_margin_sum / open_samples.max(1) as f64;
    let ridge_count = ridge_flags.iter().filter(|flag| **flag).count();
    let ridge_ray_fraction = ridge_count as f64 / ridge_flags.len() as f64;
    let mut connected_edges = 0usize;
    let mut connected_pairs = 0usize;
    for pair in 0..ridge_flags.len() - 1 {
        if ridge_flags[pair] && ridge_flags[pair + 1] {
            connected_pairs += 1;
            if (skyline_radians_by_ray[pair] - skyline_radians_by_ray[pair + 1]).abs()
                <= 6.0_f64.to_radians()
            {
                connected_edges += 1;
            }
        }
    }
    let connected_ridge_fraction = if connected_pairs == 0 {
        0.0
    } else {
        connected_edges as f64 / connected_pairs as f64
    };
    let distal: Vec<f64> = skyline_radians_by_ray
        .iter()
        .zip(&skyline_distance_m_by_ray)
        .filter_map(|(angle, distance)| (*distance >= 300.0).then_some(*angle))
        .collect();
    let distant_skyline_variation_radians = standard_deviation(&distal);
    let variation_score = (distant_skyline_variation_radians / 0.12).clamp(0.0, 1.0);
    let surface_slope_radians = origin
        .surface_normal
        .dot(origin.radial_up)
        .clamp(-1.0, 1.0)
        .acos();
    let slope_suitability = (1.0 - surface_slope_radians / 35.0_f64.to_radians()).clamp(0.0, 1.0);
    let maximum_skyline = skyline_radians_by_ray
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let skyline_suitability = (1.0
        - (maximum_skyline - 6.0_f64.to_radians()).max(0.0) / 12.0_f64.to_radians())
    .clamp(0.0, 1.0);
    let score = (0.40 * foreground_open_fraction
        + 0.28 * connected_ridge_fraction
        + 0.14 * ridge_ray_fraction
        + 0.08 * variation_score
        + 0.10 * slope_suitability)
        * (0.25 + 0.75 * skyline_suitability);
    Ok(Candidate {
        grid_x,
        grid_y,
        tangent_x_m,
        tangent_y_m,
        heading_index,
        heading_radians,
        score,
        foreground_open_fraction,
        foreground_open_margin_mean_m,
        ridge_ray_fraction,
        connected_ridge_fraction,
        distant_skyline_variation_radians,
        surface_slope_radians,
        slope_suitability,
        skyline_suitability,
        skyline_radians_by_ray,
        skyline_distance_m_by_ray,
        position_body_m: origin.position_body_m,
        radial_up: origin.radial_up,
    })
}

fn standard_deviation(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    (values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64)
        .sqrt()
}

fn camera_basis(
    radial_up: DVec3,
    heading_radians: f64,
    pitch_down_radians: f64,
) -> (DVec3, DVec3, DVec3) {
    let world_heading = DVec3::new(heading_radians.cos(), heading_radians.sin(), 0.0);
    let horizontal_forward = (world_heading - radial_up * world_heading.dot(radial_up)).normalize();
    let right = horizontal_forward.cross(radial_up).normalize();
    let forward = (horizontal_forward * pitch_down_radians.cos()
        - radial_up * pitch_down_radians.sin())
    .normalize();
    let camera_up = right.cross(forward).normalize();
    (right, camera_up, forward)
}

fn candidate_json(candidate: &Candidate) -> Value {
    json!({
        "grid_index": [candidate.grid_x, candidate.grid_y],
        "tangent_chart_anchor_m": [candidate.tangent_x_m, candidate.tangent_y_m],
        "heading_index": candidate.heading_index,
        "heading_radians": candidate.heading_radians,
        "heading_degrees": candidate.heading_radians.to_degrees(),
        "score": candidate.score,
        "metrics": {
            "foreground_open_fraction_first_50m": candidate.foreground_open_fraction,
            "foreground_open_positive_margin_mean_m": candidate.foreground_open_margin_mean_m,
            "ridge_ray_fraction_beyond_100m": candidate.ridge_ray_fraction,
            "connected_ridge_fraction_adjacent_rays": candidate.connected_ridge_fraction,
            "distant_skyline_variation_radians": candidate.distant_skyline_variation_radians,
            "surface_slope_radians": candidate.surface_slope_radians,
            "surface_slope_degrees": candidate.surface_slope_radians.to_degrees(),
            "slope_suitability": candidate.slope_suitability,
            "skyline_suitability": candidate.skyline_suitability,
            "skyline_max_elevation_radians_by_ray": candidate.skyline_radians_by_ray,
            "skyline_distance_m_by_ray": candidate.skyline_distance_m_by_ray
        },
        "surface_anchor_position_body_m": candidate.position_body_m.to_array(),
        "radial_up_body": candidate.radial_up.to_array()
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).with_context(|| format!("reading input for SHA-256 {}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn write_perspective_preview(
    path: &Path,
    generator: &SurfaceGenerator,
    radius_m: f64,
    candidate: &Candidate,
    eye_body_m: DVec3,
    camera_axes: [DVec3; 3],
    cache: &mut BTreeMap<(u64, u64), SurfacePoint>,
) -> Result<()> {
    let [right, camera_up, forward] = camera_axes;
    let mut camera_positions = Vec::with_capacity((PREVIEW_CELLS + 1) * (PREVIEW_CELLS + 1));
    let mut body_positions = Vec::with_capacity((PREVIEW_CELLS + 1) * (PREVIEW_CELLS + 1));
    for y in 0..=PREVIEW_CELLS {
        for x in 0..=PREVIEW_CELLS {
            let offset_x =
                PREVIEW_HALF_EXTENT_M * (2.0 * x as f64 / PREVIEW_CELLS as f64 - 1.0).powi(3);
            let offset_y =
                PREVIEW_HALF_EXTENT_M * (2.0 * y as f64 / PREVIEW_CELLS as f64 - 1.0).powi(3);
            let chart_x = candidate.tangent_x_m + offset_x;
            let chart_y = candidate.tangent_y_m + offset_y;
            let point = sample_tangent_point(generator, radius_m, chart_x, chart_y, cache)?;
            let relative = point.position_body_m - eye_body_m;
            body_positions.push(point.position_body_m);
            camera_positions.push(DVec3::new(
                relative.dot(right),
                relative.dot(camera_up),
                relative.dot(forward),
            ));
        }
    }
    let f = (PREVIEW_HEIGHT as f64 * 0.5) / (PREVIEW_VERTICAL_FOV_DEGREES.to_radians() * 0.5).tan();
    let mut image = vec![0u8; PREVIEW_WIDTH * PREVIEW_HEIGHT * 4];
    let mut depth = vec![f64::INFINITY; PREVIEW_WIDTH * PREVIEW_HEIGHT];
    let stride = PREVIEW_CELLS + 1;
    let light = (DVec3::new(-0.4, 0.65, 0.64)).normalize();
    for y in 0..PREVIEW_CELLS {
        for x in 0..PREVIEW_CELLS {
            let a = y * stride + x;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            for indices in [[a, b, c], [b, d, c]] {
                rasterize_triangle(
                    &mut image,
                    &mut depth,
                    &camera_positions,
                    &body_positions,
                    indices,
                    f,
                    light,
                );
            }
        }
    }
    let file =
        File::create(path).with_context(|| format!("creating preview {}", path.display()))?;
    let mut encoder = png::Encoder::new(
        BufWriter::new(file),
        PREVIEW_WIDTH as u32,
        PREVIEW_HEIGHT as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image)?;
    Ok(())
}

fn rasterize_triangle(
    image: &mut [u8],
    depth_buffer: &mut [f64],
    camera_positions: &[DVec3],
    body_positions: &[DVec3],
    indices: [usize; 3],
    focal_length_px: f64,
    light: DVec3,
) {
    let points = indices.map(|index| camera_positions[index]);
    if points
        .iter()
        .any(|point| point.z <= 0.1 || !point.is_finite())
    {
        return;
    }
    let screen = points.map(|point| {
        [
            PREVIEW_WIDTH as f64 * 0.5 + point.x * focal_length_px / point.z,
            PREVIEW_HEIGHT as f64 * 0.5 - point.y * focal_length_px / point.z,
        ]
    });
    let area = edge(screen[0], screen[1], screen[2]);
    if area.abs() < 1.0e-9 {
        return;
    }
    let min_x = screen
        .iter()
        .map(|p| p[0].floor() as isize)
        .min()
        .unwrap_or(0)
        .max(0) as usize;
    let max_x = screen
        .iter()
        .map(|p| p[0].ceil() as isize)
        .max()
        .unwrap_or(-1)
        .min(PREVIEW_WIDTH as isize - 1);
    let min_y = screen
        .iter()
        .map(|p| p[1].floor() as isize)
        .min()
        .unwrap_or(0)
        .max(0) as usize;
    let max_y = screen
        .iter()
        .map(|p| p[1].ceil() as isize)
        .max()
        .unwrap_or(-1)
        .min(PREVIEW_HEIGHT as isize - 1);
    if max_x < 0 || max_y < 0 || min_x >= PREVIEW_WIDTH || min_y >= PREVIEW_HEIGHT {
        return;
    }
    let mut normal = (body_positions[indices[1]] - body_positions[indices[0]])
        .cross(body_positions[indices[2]] - body_positions[indices[0]])
        .normalize_or_zero();
    if normal == DVec3::ZERO {
        return;
    }
    if normal.dot(body_positions[indices[0]]) < 0.0 {
        normal = -normal;
    }
    let diffuse = normal.dot(light).clamp(0.0, 1.0);
    let shade = (65.0 + diffuse * 155.0) as u8;
    for py in min_y..=max_y as usize {
        for px in min_x..=max_x as usize {
            let sample = [px as f64 + 0.5, py as f64 + 0.5];
            let w0 = edge(screen[1], screen[2], sample) / area;
            let w1 = edge(screen[2], screen[0], sample) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let inverse_z = w0 / points[0].z + w1 / points[1].z + w2 / points[2].z;
            if inverse_z <= 0.0 {
                continue;
            }
            let distance = 1.0 / inverse_z;
            let pixel = py * PREVIEW_WIDTH + px;
            if distance >= depth_buffer[pixel] {
                continue;
            }
            depth_buffer[pixel] = distance;
            let offset = pixel * 4;
            image[offset] = shade;
            image[offset + 1] = shade;
            image[offset + 2] = shade;
            image[offset + 3] = 255;
        }
    }
}

fn edge(a: [f64; 2], b: [f64; 2], point: [f64; 2]) -> f64 {
    (point[0] - a[0]) * (b[1] - a[1]) - (point[1] - a[1]) * (b[0] - a[0])
}
