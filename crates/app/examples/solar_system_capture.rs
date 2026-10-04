//! Headless gameplay-scale content captures through the native terrain admission and
//! production celestial renderer. Labels are recorded in manifests; crosshairs are
//! navigational markers with explicit pixel width, never physically scaled bodies.
use anyhow::{Result, ensure};
use glam::{DMat3, DQuat, DVec3};
use mundaris_app::{
    celestial_labels::{LabelInput, LabelLayout, ScreenRect},
    planet_surface::PlanetSurfaceSession,
    planet_terrain::{MAX_PENDING_PATCHES, MAX_TERRAIN_PATCHES},
    solar_system::{SOLAR_SYSTEM_CONTENT, SolarBody, SolarSystemPreset},
    system_view::{OverviewScope, SystemViewBounds},
    terrain_population::TerrainPopulation,
};
use mundaris_math::{surface::SurfaceLocation, *};
use mundaris_renderer::{
    CelestialFrame, CelestialLineStyle, CelestialPolyline, CelestialProjection,
    CelestialRenderBody, CelestialStaging, Icosphere, PlanetaryConfig, PreparedView,
    RenderPrecisionBudget,
    planet_surface::{SurfaceStyle, TerrainLighting, TerrainRenderMode},
    terrain_capture::TerrainCaptureRenderer,
};
use mundaris_world::{terrain::*, *};
use std::{
    collections::BTreeMap,
    fmt::Write,
    fs,
    num::NonZeroU64,
    path::Path,
    time::{Duration, Instant},
};

const WIDTH: u32 = 1152;
const HEIGHT: u32 = 768;
const DEFAULT_MAX_UPDATES: usize = 800;
const VERTEX_BUDGET: usize = 1156;
const SCENES: &[&str] = &[
    "system",
    "inner",
    "earth-moon",
    "adaptive",
    "high-orbit",
    "low-orbit",
    "high-altitude",
    "mountain",
    "near-ground",
    "mars",
    "jupiter",
    "saturn",
    "neptune",
    "morph",
    "compare600",
    "compare800",
    "compare1000",
    "phase59-100000",
    "phase59-10000",
    "phase59-1000",
    "phase59-100",
    "phase59-10",
    "phase59-2",
    "phase59-100-legacy",
    "phase59-10-legacy",
    "phase59-2-legacy",
    "phase59-100-guarded",
    "phase59-10-guarded",
    "phase59-2-guarded",
    "phase59-high-orbit",
    "phase59-coastline",
    "phase59-mars-10",
    "phase59-moon-10",
    "phase59-land-mountain",
    "phase59-land-plain",
    "phase59-land-ridge",
    "phase59-land-gully",
    "phase59-shoreline",
    "planetary-terminator",
    "planetary-grazing",
    "planetary-real-orbit",
    "planetary-moon",
    "planetary-mars",
];

fn bitmap(path: &Path, rgba: &[u8]) -> Result<()> {
    ensure!(
        rgba.len() == (WIDTH * HEIGHT * 4) as usize,
        "unexpected capture size"
    );
    let row = (WIDTH * 3).div_ceil(4) * 4;
    let mut bytes = vec![0u8; (54 + row * HEIGHT) as usize];
    let size = bytes.len() as u32;
    bytes[..2].copy_from_slice(b"BM");
    bytes[2..6].copy_from_slice(&size.to_le_bytes());
    bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
    bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
    bytes[18..22].copy_from_slice(&(WIDTH as i32).to_le_bytes());
    bytes[22..26].copy_from_slice(&(-(HEIGHT as i32)).to_le_bytes());
    bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
    bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let source = (y * WIDTH as usize + x) * 4;
            let target = 54 + y * row as usize + x * 3;
            bytes[target..target + 3].copy_from_slice(&[
                rgba[source + 2],
                rgba[source + 1],
                rgba[source],
            ]);
        }
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn body_index(body: SolarBody) -> usize {
    body as usize
}

fn camera_rotation(back: DVec3) -> Result<UnitRotation> {
    let back = back.normalize();
    let right = if back.y.abs() < 0.9 {
        DVec3::Y.cross(back)
    } else {
        DVec3::X.cross(back)
    }
    .normalize();
    let up = back.cross(right);
    Ok(UnitRotation::try_from_quaternion(DQuat::from_mat3(
        &DMat3::from_cols(right, up, back),
    ))?)
}

#[allow(clippy::too_many_arguments)]
fn marker_segments(
    frame: &mut CelestialFrame<'_, '_, '_>,
    pair: &CoherentCelestialView<'_>,
    root: FrameId,
    observer: DVec3,
    requests: &[CelestialRenderBody],
    names: &[String],
    pixels: f64,
) -> Result<()> {
    let eval = pair.evaluation();
    let half_factor = 2.0 * (60.0_f64.to_radians() * 0.5).tan() * pixels / HEIGHT as f64;
    for (index, (_, body)) in pair.system().bodies().enumerate() {
        if !names.iter().any(|name| name == body.name()) {
            continue;
        }
        let center = eval
            .convert_position(
                FramePosition::new(requests[index].body_fixed_frame, LocalPosition::origin()),
                root,
            )?
            .local()
            .metres();
        let half = (center - observer).length() * half_factor;
        let color = SOLAR_SYSTEM_CONTENT[index].color;
        for axis in [DVec3::X, DVec3::Y] {
            let points = [
                FramePosition::new(root, LocalPosition::try_metres(center - axis * half)?),
                FramePosition::new(root, LocalPosition::try_metres(center + axis * half)?),
            ];
            frame.append_polylines(&[CelestialPolyline {
                points: &points,
                colors: &[color, color],
                width_pixels: 1.5,
                style: CelestialLineStyle::Solid,
            }])?;
        }
    }
    Ok(())
}

fn body_id(world: &CelestialSystem, body: SolarBody) -> Result<BodyId> {
    world
        .bodies()
        .nth(body_index(body))
        .map(|(id, _)| id)
        .ok_or_else(|| anyhow::anyhow!("missing authored body {body:?}"))
}

fn scope_for(scene: &str, world: &CelestialSystem) -> Result<Option<(Vec<BodyId>, &'static str)>> {
    let bodies = match scene {
        "system" => world.bodies().map(|(id, _)| id).collect(),
        "inner" => [
            SolarBody::Sun,
            SolarBody::Mercury,
            SolarBody::Venus,
            SolarBody::Earth,
            SolarBody::Moon,
            SolarBody::Mars,
        ]
        .into_iter()
        .map(|b| body_id(world, b))
        .collect::<Result<Vec<_>>>()?,
        "earth-moon" => [SolarBody::Earth, SolarBody::Moon]
            .into_iter()
            .map(|b| body_id(world, b))
            .collect::<Result<Vec<_>>>()?,
        _ => return Ok(None),
    };
    Ok(Some((
        bodies,
        match scene {
            "system" => "whole-system",
            "inner" => "inner-system",
            _ => "Earth-Moon",
        },
    )))
}

type SceneObserver = (FramePose, Option<SolarBody>, String, Vec<String>);

fn fibonacci_direction(index: usize, count: usize) -> DVec3 {
    let z = 1.0 - 2.0 * (index as f64 + 0.5) / count as f64;
    let angle = index as f64 * (std::f64::consts::PI * (3.0 - 5.0_f64.sqrt()));
    DVec3::new(
        (1.0 - z * z).sqrt() * angle.cos(),
        (1.0 - z * z).sqrt() * angle.sin(),
        z,
    )
}

/// Fixed-order content selection, not geometry tuning. Names describe inspection
/// targets; ridge/branching visual acceptance still requires reading the images.
fn landform_direction(
    scene: &str,
    generator: &TerrainGenerator,
    radius: f64,
) -> Result<(DVec3, String)> {
    let sea = mundaris_app::solar_system::GAMEPLAY_EARTH_SEA_LEVEL_M;
    let mut best: Option<(f64, DVec3, String)> = None;
    for index in 0..4096 {
        let direction = fibonacci_direction(index, 4096);
        let location = SurfaceLocation::new(Direction3::try_new(direction)?);
        let query = TerrainQuery {
            location,
            footprint: TerrainFootprint::COMPLETE,
        };
        let sample = generator.evaluate_point(query)?;
        let height = sample.height_m();
        let slope = sample.slope_angle_rad(radius)?.to_degrees();
        if height < sea + 60.0 {
            continue;
        }
        let erosion = generator.erosion_diagnostics(query)?;
        let score = match scene {
            "phase59-land-mountain" => height,
            "phase59-land-plain" => -slope,
            "phase59-land-ridge" => slope,
            "phase59-land-gully" => -erosion.contribution_m,
            _ => anyhow::bail!("unknown landform scene"),
        };
        if best
            .as_ref()
            .is_none_or(|(previous, _, _)| score > *previous)
        {
            best = Some((
                score,
                direction,
                format!(
                    "landform_selection candidates=4096 order=fibonacci index={index} predicate=height_above_sea_plus_60m score={score} complete_height_m={height} complete_slope_degrees={slope} erosion={erosion:?}"
                ),
            ));
        }
    }
    let (_, direction, description) =
        best.ok_or_else(|| anyhow::anyhow!("no landform candidate"))?;
    Ok((direction, description))
}

fn shoreline_direction(generator: &TerrainGenerator) -> Result<(DVec3, String)> {
    let sea = mundaris_app::solar_system::GAMEPLAY_EARTH_SEA_LEVEL_M;
    let footprint = TerrainFootprint::new(781.25)?;
    let mut best: Option<(f64, DVec3, String)> = None;
    for index in 0..4096 {
        let direction = fibonacci_direction(index, 4096);
        let sample = generator.evaluate_point(TerrainQuery {
            location: SurfaceLocation::new(Direction3::try_new(direction)?),
            footprint,
        })?;
        let gradient = sample.tangent_gradient_m_per_unit_direction();
        if gradient.length_squared() == 0.0 {
            continue;
        }
        let tangent = gradient.normalize();
        let endpoints = [
            (direction - tangent * 0.04).normalize(),
            (direction + tangent * 0.04).normalize(),
        ];
        let mut complete = [0.0; 2];
        let mut filtered = [0.0; 2];
        for (end, point) in endpoints.iter().enumerate() {
            let location = SurfaceLocation::new(Direction3::try_new(*point)?);
            complete[end] = generator
                .evaluate_point(TerrainQuery {
                    location,
                    footprint: TerrainFootprint::COMPLETE,
                })?
                .height_m();
            filtered[end] = generator
                .evaluate_point(TerrainQuery {
                    location,
                    footprint,
                })?
                .height_m();
        }
        if complete[0] >= sea - 5.0
            || complete[1] <= sea + 5.0
            || filtered[0] >= sea - 5.0
            || filtered[1] <= sea + 5.0
        {
            continue;
        }
        let score = (sample.height_m() - sea).abs();
        if best
            .as_ref()
            .is_none_or(|(previous, _, _)| score < *previous)
        {
            best = Some((
                score,
                direction,
                format!(
                    "shoreline_selection candidates=4096 order=fibonacci index={index} filtered_footprint_m=781.25 angular_half_span=0.04 center_filtered_height_m={} endpoints_body_fixed={endpoints:?} complete_endpoint_heights_m={complete:?} filtered_endpoint_heights_m={filtered:?} sea_level_m={sea} opposite_sides_margin_m=5",
                    sample.height_m()
                ),
            ));
        }
    }
    let (_, direction, description) =
        best.ok_or_else(|| anyhow::anyhow!("no shoreline crossing candidate"))?;
    Ok((direction, description))
}

fn observer_pose(
    scene: &str,
    pair: &CoherentCelestialView<'_>,
    frames: &CelestialFrameProjection,
    projection: CelestialProjection,
    requests: &[CelestialRenderBody],
) -> Result<SceneObserver> {
    let root = frames.tree().root();
    if let Some((ids, label)) = scope_for(scene, pair.system())? {
        let bounds = SystemViewBounds::calculate(
            pair.system(),
            &OverviewScope::ExplicitBodies(ids.clone()),
            &[],
            &[],
            false,
        )?;
        let center = bounds.center_m();
        let distance = bounds.fit_distance_m(projection)?;
        let position = center + DVec3::Z * distance;
        let pose = FramePose::new(
            FramePosition::new(root, LocalPosition::try_metres(position)?),
            camera_rotation(DVec3::Z)?,
        );
        let visible_names = ids
            .iter()
            .map(|id| pair.system().body(*id).map(|b| b.name().to_owned()))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok((
            pose,
            None,
            format!("scope={label} fit_distance_m={distance} bounds_center_root_m={center:?}"),
            visible_names,
        ));
    }

    let target = match scene {
        "mars" | "phase59-mars-10" | "planetary-mars" => SolarBody::Mars,
        "phase59-moon-10" | "planetary-moon" => SolarBody::Moon,
        "jupiter" => SolarBody::Jupiter,
        "saturn" => SolarBody::Saturn,
        "neptune" => SolarBody::Neptune,
        _ => SolarBody::Earth,
    };
    let index = body_index(target);
    let body = pair
        .system()
        .bodies()
        .nth(index)
        .ok_or_else(|| anyhow::anyhow!("missing target body"))?
        .1;
    let radius = body.properties().reference_radius_m();
    let fixed = requests[index].body_fixed_frame;
    let star = pair
        .system()
        .bodies()
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing Sun"))?
        .1;
    let to_star =
        star.state().center_in_system().metres() - body.state().center_in_system().metres();
    let star_direction = pair
        .evaluation()
        .convert_direction(
            FrameDirection::new(root, Direction3::try_new(to_star)?),
            fixed,
        )?
        .local()
        .unit();
    let (mut direction, clearance, horizon) = match scene {
        "high-orbit" | "phase59-high-orbit" => (star_direction, 600_000.0, false),
        "planetary-real-orbit" => (star_direction, radius * 1.5, false),
        "planetary-moon" | "planetary-mars" => (star_direction, radius * 1.5, false),
        "planetary-terminator" | "planetary-grazing" => {
            let tangent = star_direction.cross(DVec3::Y).normalize();
            let angle = if scene == "planetary-terminator" {
                90.0_f64
            } else {
                75.0_f64
            }
            .to_radians();
            (
                star_direction * angle.cos() + tangent * angle.sin(),
                600_000.0,
                false,
            )
        }
        "adaptive" => (star_direction, 200_000.0, false),
        "low-orbit" => (star_direction, 40_000.0, false),
        "high-altitude" => (star_direction, 10_000.0, false),
        "mountain" | "morph" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            3_000.0,
            false,
        ),
        "near-ground" => (star_direction, 20.0, true),
        "phase59-100000" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            100_000.0,
            false,
        ),
        "phase59-10000" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            10_000.0,
            false,
        ),
        "phase59-1000" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            1_000.0,
            false,
        ),
        "phase59-100" | "phase59-100-legacy" | "phase59-100-guarded" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            100.0,
            false,
        ),
        "phase59-10" | "phase59-10-legacy" | "phase59-10-guarded" | "phase59-mars-10"
        | "phase59-moon-10" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            10.0,
            true,
        ),
        "phase59-2" | "phase59-2-legacy" | "phase59-2-guarded" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            2.0,
            true,
        ),
        "phase59-coastline" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            30_000.0,
            false,
        ),
        "phase59-land-mountain" => (star_direction, 1_000.0, false),
        "phase59-land-plain" => (star_direction, 100.0, false),
        "phase59-land-ridge" => (star_direction, 30.0, false),
        "phase59-land-gully" => (star_direction, 100.0, false),
        "phase59-shoreline" => (star_direction, 60_000.0, false),
        "compare600" | "compare800" | "compare1000" => (star_direction, 600_000.0, false),
        "mars" | "jupiter" | "saturn" | "neptune" => (star_direction, radius * 2.0, false),
        _ => anyhow::bail!("unknown scene {scene}"),
    };
    let mut selection_description = String::new();
    if scene.starts_with("phase59-land-") || scene == "phase59-shoreline" {
        let generator = TerrainGenerator::new(
            body.terrain()
                .ok_or_else(|| anyhow::anyhow!("landform body has no terrain"))?,
            radius,
        )?;
        (direction, selection_description) = if scene == "phase59-shoreline" {
            shoreline_direction(&generator)?
        } else {
            landform_direction(scene, &generator, radius)?
        };
    }
    if scene == "phase59-coastline" {
        let generator = TerrainGenerator::new(
            body.terrain()
                .ok_or_else(|| anyhow::anyhow!("coastline body has no terrain"))?,
            radius,
        )?;
        let mut closest = (f64::INFINITY, direction);
        for index in 0..256 {
            let z = 1.0 - 2.0 * (index as f64 + 0.5) / 256.0;
            let angle = index as f64 * (std::f64::consts::PI * (3.0 - 5.0_f64.sqrt()));
            let candidate = DVec3::new(
                (1.0 - z * z).sqrt() * angle.cos(),
                (1.0 - z * z).sqrt() * angle.sin(),
                z,
            );
            let height = generator
                .evaluate_point(TerrainQuery {
                    location: SurfaceLocation::new(Direction3::try_new(candidate)?),
                    footprint: TerrainFootprint::COMPLETE,
                })?
                .height_m();
            if (height - mundaris_app::solar_system::GAMEPLAY_EARTH_SEA_LEVEL_M).abs() < closest.0 {
                closest = (
                    (height - mundaris_app::solar_system::GAMEPLAY_EARTH_SEA_LEVEL_M).abs(),
                    candidate,
                );
            }
        }
        direction = closest.1;
    }
    let mut height_m = 0.0;
    if let Some(definition) = body.terrain() {
        let generator = TerrainGenerator::new(definition, radius)?;
        height_m = generator
            .evaluate_point(TerrainQuery {
                location: SurfaceLocation::new(Direction3::try_new(direction)?),
                footprint: TerrainFootprint::COMPLETE,
            })?
            .height_m();
    }
    let legacy_sphere_pose = scene.ends_with("-legacy");
    let target_point = direction * (radius + if legacy_sphere_pose { 0.0 } else { height_m });
    let eye = target_point + direction * clearance;
    let off_nadir_degrees: f64 = match scene {
        "low-orbit" => 55.0,
        "high-altitude" => 70.0,
        "mountain" | "morph" => 78.0,
        "phase59-land-mountain" | "phase59-land-plain" => 45.0,
        "near-ground" | "phase59-10" | "phase59-2" | "phase59-10-legacy" | "phase59-2-legacy"
        | "phase59-10-guarded" | "phase59-2-guarded" | "phase59-mars-10" | "phase59-moon-10" => {
            90.0
        }
        _ => 0.0,
    };
    let back = if off_nadir_degrees > 0.0 {
        let east = direction
            .cross(if direction.y.abs() < 0.9 {
                DVec3::Y
            } else {
                DVec3::X
            })
            .normalize();
        let angle = off_nadir_degrees.to_radians();
        direction * angle.cos() - east * angle.sin()
    } else {
        direction
    };
    let orientation = if off_nadir_degrees > 0.0 {
        let right = direction.cross(back).normalize();
        let up = back.cross(right).normalize();
        UnitRotation::try_from_quaternion(DQuat::from_mat3(&DMat3::from_cols(right, up, back)))?
    } else {
        camera_rotation(back)?
    };
    let pose = FramePose::new(
        FramePosition::new(fixed, LocalPosition::try_metres(eye)?),
        orientation,
    );
    Ok((
        pose,
        Some(target),
        format!(
            "target={target:?} clearance_above_sampled_terrain_m={clearance} sampled_complete_height_m={height_m} eye_body_fixed_m={eye:?} camera_policy={} look_horizon={horizon} off_nadir_degrees={off_nadir_degrees} configured_diameter_km={}\n{selection_description}",
            if legacy_sphere_pose {
                "legacy-sphere-relative-unadjusted"
            } else {
                "analytic-terrain-relative-unadjusted"
            },
            2.0 * radius / 1000.0
        ),
        vec![SOLAR_SYSTEM_CONTENT[index].name.to_owned()],
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_frame<'view, 'tree, 'storage>(
    view: &'view PreparedView<'tree>,
    staging: &'storage mut CelestialStaging,
    projection: CelestialProjection,
    sphere: &'view Icosphere,
    requests: &[CelestialRenderBody],
    owners: &[bool],
    population: &TerrainPopulation,
    lighting: TerrainLighting,
    style: SurfaceStyle,
    environment: Option<PlanetaryConfig>,
) -> Result<CelestialFrame<'view, 'tree, 'storage>> {
    let mut frame = CelestialFrame::new(view, staging, projection, sphere);
    frame.set_terrain_lighting(lighting);
    if let Some(index) = owners.iter().position(|&owner| owner)
        && population.cover.ready()
    {
        if lighting.mode() == TerrainRenderMode::Natural
            && let Some(config) = environment
        {
            frame.set_planetary_environment(requests[index], config)?;
        }
        let (surface, topology) = (
            population
                .cover
                .surface()
                .ok_or_else(|| anyhow::anyhow!("missing ready terrain surface"))?,
            population
                .cover
                .topology()
                .ok_or_else(|| anyhow::anyhow!("missing terrain topology"))?,
        );
        frame.append_stitched_surface(
            requests[index],
            population.cover.visible(),
            surface,
            topology,
            style,
        )?;
        if let Some((mesh, fraction)) = population.cover.transition() {
            frame.append_surface_transition(requests[index], mesh, fraction, style)?;
        }
    }
    frame.append_body_observations(requests, owners)?;
    Ok(frame)
}

fn distribution(levels: impl Iterator<Item = u8>) -> BTreeMap<u8, usize> {
    let mut counts = BTreeMap::new();
    for level in levels {
        *counts.entry(level).or_insert(0) += 1;
    }
    counts
}

/// Directed inspection only: an unresolved filtered mesh can sit above the
/// complete field. Place this capture above the actual ready triangles, without
/// pretending the native reference-sphere guard is terrain collision/navigation.
fn drawn_surface_radius(population: &TerrainPopulation, direction: DVec3) -> Result<f64> {
    let (face, uv) = SurfaceLocation::new(Direction3::try_new(direction)?).face_uv();
    let surface = population
        .cover
        .surface()
        .ok_or_else(|| anyhow::anyhow!("ground capture lacks ready surface"))?;
    let topology = population
        .cover
        .topology()
        .ok_or_else(|| anyhow::anyhow!("ground capture lacks topology"))?;
    let mut radius: Option<f64> = None;
    for patch in population
        .cover
        .active()
        .iter()
        .filter(|p| p.address.face() == face && p.address.patch_local(uv).is_ok())
    {
        let index = surface
            .patches()
            .binary_search_by_key(&patch.address, |p| p.address())
            .map_err(|_| anyhow::anyhow!("ground patch missing"))?;
        let samples = surface.patches()[index].samples();
        for triangle in topology.indices(patch.stitch_mask).as_chunks::<3>().0 {
            let [a, b, c] = [
                samples[usize::from(triangle[0])].position_body_m,
                samples[usize::from(triangle[1])].position_body_m,
                samples[usize::from(triangle[2])].position_body_m,
            ];
            let ab = b - a;
            let ac = c - a;
            let normal = ab.cross(ac);
            let denominator = normal.dot(direction);
            if denominator <= normal.length() * 1e-12 {
                continue;
            }
            let distance = normal.dot(a) / denominator;
            if distance <= 0.0 || !distance.is_finite() {
                continue;
            }
            let displacement = direction * distance - a;
            let d00 = ab.dot(ab);
            let d01 = ab.dot(ac);
            let d11 = ac.dot(ac);
            let determinant = d00 * d11 - d01 * d01;
            if determinant <= 0.0 {
                continue;
            }
            let u = (d11 * displacement.dot(ab) - d01 * displacement.dot(ac)) / determinant;
            let v = (d00 * displacement.dot(ac) - d01 * displacement.dot(ab)) / determinant;
            if u >= -1e-8 && v >= -1e-8 && u + v <= 1.0 + 1e-8 {
                radius = Some(radius.map_or(distance, |old| old.max(distance)));
            }
        }
    }
    radius.ok_or_else(|| anyhow::anyhow!("ground inspection radial ray missed ready triangles"))
}

fn run_scene(
    scene: &str,
    output: &Path,
    max_updates: usize,
    capture: &mut TerrainCaptureRenderer,
) -> Result<()> {
    let namespace = NonZeroU64::new(580).ok_or_else(|| anyhow::anyhow!("namespace"))?;
    let mut preset = SolarSystemPreset::gameplay();
    if scene == "planetary-real-orbit" {
        preset = SolarSystemPreset::real_scale();
    }
    if let Some(diameter) = scene.strip_prefix("compare") {
        let diameter_km = diameter.parse::<f64>()?;
        preset.body_radius_scale = diameter_km * 1000.0 / (2.0 * 6_371_000.0);
    }
    let world = preset.create(namespace)?;
    let frames = CelestialFrameProjection::build(&world, namespace)?;
    let pair = frames.coherent_view(&world)?;
    let projection = CelestialProjection::try_new(WIDTH, HEIGHT, 60.0_f64.to_radians(), 0.1)?;
    let requests: Vec<_> = world
        .bodies()
        .map(|(id, body)| {
            let index = SOLAR_SYSTEM_CONTENT
                .iter()
                .position(|content| content.name == body.name())
                .ok_or_else(|| anyhow::anyhow!("unknown authored body"))?;
            Ok(CelestialRenderBody {
                body_fixed_frame: frames.frames_for(id)?.body_fixed,
                reference_radius_m: body.properties().reference_radius_m(),
                color: SOLAR_SYSTEM_CONTENT[index].color,
                unlit: SOLAR_SYSTEM_CONTENT[index].identity == SolarBody::Sun,
                selected: false,
            })
        })
        .collect::<Result<_>>()?;
    let (mut pose, target, mut scene_description, marker_names) =
        observer_pose(scene, &pair, &frames, projection, &requests)?;
    let mut view = PreparedView::new(
        &pair.evaluation(),
        pose,
        RenderPrecisionBudget::near_debug(),
    )?;
    let mut sessions = Vec::new();
    for (id, body) in world.bodies() {
        if body.terrain().is_some() {
            sessions.push(PlanetSurfaceSession::new(id, MAX_TERRAIN_PATCHES)?);
        }
    }
    let mut population = TerrainPopulation::new()?;
    let morph = scene == "morph";
    let morph_duration = if morph {
        Duration::from_millis(150)
    } else {
        Duration::ZERO
    };
    let mut owners = vec![false; requests.len()];
    let sphere = Icosphere::new();
    let mut update_cpu_ms = Vec::new();
    let mut generated_total = 0usize;
    let mut pending_max = 0usize;
    let mut vertices_max = 0usize;
    let mut idle = 0usize;
    let mut updates = 0usize;
    let mut morph_active_updates = 0usize;
    let mut morph_builds = 0usize;
    let mut morph_build_ms = Vec::new();
    let mut stitch_build_ms = Vec::new();
    let mut morph_sample = false;
    let mut morph_fraction = None;
    for update_index in 0..max_updates {
        let start = Instant::now();
        population.update(
            &pair,
            &view,
            projection,
            &requests,
            &mut sessions,
            &mut owners,
            &sphere,
            true,
            morph_duration,
            VERTEX_BUDGET,
            None,
            Duration::from_millis(16),
        )?;
        let elapsed = start.elapsed();
        update_cpu_ms.push(elapsed.as_secs_f64() * 1000.0);
        updates += 1;
        generated_total += population.work.vertices_generated;
        pending_max = pending_max.max(population.work.pending_patches);
        vertices_max = vertices_max.max(population.work.vertices_generated);
        if population.cover.transition().is_some() {
            morph_active_updates += 1;
        }
        if !population.cover.morph_preparation.is_zero() {
            morph_builds += 1;
            morph_build_ms.push(population.cover.morph_preparation.as_secs_f64() * 1000.0);
        }
        if !population.cover.stitch_preparation.is_zero() {
            stitch_build_ms.push(population.cover.stitch_preparation.as_secs_f64() * 1000.0);
        }
        if morph
            && population
                .cover
                .active()
                .iter()
                .any(|p| p.address.level() > 0)
            && let Some((_, fraction)) = population.cover.transition()
            && (0.35..=0.65).contains(&fraction)
        {
            morph_sample = true;
            morph_fraction = Some(fraction);
            break;
        }
        if population.work.vertices_generated == 0
            && population.work.pending_patches == 0
            && population.cover.transition().is_none()
        {
            idle += 1;
        } else {
            idle = 0;
        }
        if idle >= 8 {
            break;
        }
        if update_index + 1 == max_updates {
            break;
        }
    }

    let guarded_pose = scene.ends_with("-guarded");
    if scene == "near-ground" || guarded_pose {
        let direction = pose.position().local().metres().normalize();
        let drawn_radius = drawn_surface_radius(&population, direction)?;
        let old_eye_radius = pose.position().local().metres().length();
        let requested_clearance = if guarded_pose {
            match scene {
                "phase59-100-guarded" => 100.0,
                "phase59-10-guarded" => 10.0,
                _ => 2.0,
            }
        } else {
            20.0
        };
        let target_index =
            body_index(target.ok_or_else(|| anyhow::anyhow!("guarded capture target missing"))?);
        let target_body = world
            .bodies()
            .nth(target_index)
            .ok_or_else(|| anyhow::anyhow!("guarded target body missing"))?
            .1;
        let full_radius = requests[target_index].reference_radius_m
            + if let Some(definition) = target_body.terrain() {
                TerrainGenerator::new(definition, requests[target_index].reference_radius_m)?
                    .evaluate_point(TerrainQuery {
                        location: SurfaceLocation::new(Direction3::try_new(direction)?),
                        footprint: TerrainFootprint::COMPLETE,
                    })?
                    .height_m()
            } else {
                0.0
            };
        let eye_radius = if guarded_pose {
            old_eye_radius.max(full_radius.max(drawn_radius) + requested_clearance)
        } else {
            old_eye_radius.max(drawn_radius + requested_clearance)
        };
        pose = FramePose::new(
            FramePosition::new(
                pose.position().frame(),
                LocalPosition::try_metres(direction * eye_radius)?,
            ),
            pose.orientation(),
        );
        view = PreparedView::new(
            &pair.evaluation(),
            pose,
            RenderPrecisionBudget::near_debug(),
        )?;
        writeln!(
            scene_description,
            "\ninspection_only_radial_adjustment_m={} drawn_surface_radius_m={drawn_radius} full_terrain_radius_m={full_radius} requested_clearance_m={requested_clearance} clearance_above_ready_mesh_m={}",
            eye_radius - old_eye_radius,
            eye_radius - drawn_radius
        )?;
    }
    let mut steady_update_cpu_ms = Vec::new();
    for _ in 0..8 {
        let started = Instant::now();
        population.update(
            &pair,
            &view,
            projection,
            &requests,
            &mut sessions,
            &mut owners,
            &sphere,
            true,
            morph_duration,
            0,
            None,
            Duration::ZERO,
        )?;
        steady_update_cpu_ms.push(started.elapsed().as_secs_f64() * 1000.0);
        ensure!(
            population.work.vertices_generated == 0,
            "steady zero-budget probe generated terrain"
        );
    }
    let population_body = population
        .active_body()
        .and_then(|id| world.bodies().position(|(candidate, _)| candidate == id))
        .map(|index| SOLAR_SYSTEM_CONTENT[index].identity);
    let levels = distribution(population.cover.active().iter().map(|p| p.address.level()));
    let visible_levels = distribution(population.cover.visible().iter().map(|p| p.address.level()));
    let cache = population.cache.report();
    let mut manifest = format!(
        "Production terrain capture — static operation-budget fixture\nscene={scene}\npreset_radius_scale={} orbital_distance_scale={} viewport={WIDTH}x{HEIGHT} fov_degrees=60\nscene_geometry={scene_description}\nlabels=manifest-only; screen-space identity markers are navigational aids and do not change physical scales\nadapter={} gpu_timestamp_capability={:?}\nupdates={updates} update_limit={max_updates} update_vertex_budget={VERTEX_BUDGET} elapsed_step_ms=16 morph_ms={}\nrequested_target={target:?} active_body={population_body:?} active_body_id={:?} surface_owner_count={}\nactive_cover_patches={} visible_cover_patches={} levels={levels:?} visible_levels={visible_levels:?} ready={} quality_pending={} settled={} budget_constrained={} max_level={} max_error_px={}\ncache_resident_patches={} cache_resident_bytes={} cache_pinned_patches={} cache_external_bytes={} cache_peak_aggregate_bytes={} cache_hits={} cache_misses={} cache_evictions={} pending_final={} pending_max={} vertices_per_update_max={} generated_samples_total={}\nmorph_active_updates={} morph_build_count={} morph_build_ms_total={} morph_build_ms_worst={} morph_transition_active={} morph_fraction={:?} morph_capture={}\n",
        preset.body_radius_scale,
        preset.orbital_distance_scale,
        capture.adapter_name(),
        capture.timestamp_availability(),
        morph_duration.as_millis(),
        population.active_body(),
        owners.iter().filter(|&&owner| owner).count(),
        population.cover.active().len(),
        population.cover.visible().len(),
        population.cover.ready(),
        population.cover.report.quality_pending,
        population.cover.report.settled,
        population.cover.report.budget_constrained,
        population.cover.report.max_level,
        population.cover.report.max_error_pixels,
        cache.resident_patches,
        cache.resident_bytes,
        cache.pinned_patches,
        cache.external_bytes,
        cache.peak_aggregate_bytes,
        cache.hits,
        cache.misses,
        cache.evictions,
        population.cache.pending(),
        pending_max,
        vertices_max,
        generated_total,
        morph_active_updates,
        morph_builds,
        morph_build_ms.iter().sum::<f64>(),
        morph_build_ms.iter().copied().fold(0.0_f64, f64::max),
        population.cover.transition().is_some(),
        morph_fraction,
        morph_sample
    );
    for args in [vec!["rev-parse", "HEAD"], vec!["status", "--short"]] {
        let value = std::process::Command::new("git")
            .args(&args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_else(|| "unavailable".into());
        writeln!(manifest, "repository_command={args:?}\n{value}")?;
    }
    writeln!(
        manifest,
        "capture_label={} adapter_backend={}",
        std::env::var("MUNDARIS_CAPTURE_LABEL").unwrap_or_else(|_| "current-working-tree".into()),
        capture.adapter_backend()
    )?;
    ensure!(
        population.cache.pending() <= MAX_PENDING_PATCHES,
        "capture exceeded pending-work cap"
    );
    ensure!(
        population.cache.report().peak_aggregate_bytes <= 128 * 1024 * 1024,
        "aggregate terrain ceiling exceeded"
    );
    writeln!(
        manifest,
        "body_count={} far_owner_count={} terrain_active_body_count={} steady_generated_samples={} pending_patch_count={} shared_cache_quota_bytes={} cpu_cap_bytes={} persistent_world_body_struct_bytes={} surface_session_count={}",
        world.body_count(),
        world.body_count() - owners.iter().filter(|&&owner| owner).count(),
        usize::from(population.active_body().is_some()),
        population.work.vertices_generated,
        population.cache.pending(),
        112 * 1024 * 1024,
        128 * 1024 * 1024,
        world.body_count() * size_of::<CelestialBody>(),
        sessions.len()
    )?;
    stitch_build_ms.sort_by(f64::total_cmp);
    morph_build_ms.sort_by(f64::total_cmp);
    writeln!(
        manifest,
        "stitch_build_count={} stitch_cpu_ms_median={} worst={} morph_cpu_ms_median={}",
        stitch_build_ms.len(),
        stitch_build_ms
            .get(stitch_build_ms.len() / 2)
            .copied()
            .unwrap_or(0.0),
        stitch_build_ms.last().copied().unwrap_or(0.0),
        morph_build_ms
            .get(morph_build_ms.len() / 2)
            .copied()
            .unwrap_or(0.0)
    )?;
    writeln!(
        manifest,
        "mixed_lod_active_cover={} generated_samples_new_during_capture={generated_total} morph_active_updates={morph_active_updates} morph_build_count={morph_builds}",
        levels.len() > 1
    )?;
    writeln!(
        manifest,
        "terrain_lod_diagnostics={:?} active_patch_addresses={:?} visible_patch_addresses={:?}",
        population.cover.report,
        population
            .cover
            .active()
            .iter()
            .map(|patch| patch.address)
            .collect::<Vec<_>>(),
        population
            .cover
            .visible()
            .iter()
            .map(|patch| patch.address)
            .collect::<Vec<_>>()
    )?;
    let mut staging = CelestialStaging::default();
    let mut render_preparation_ms = Vec::new();
    let mut render_encode_ms = Vec::new();
    let mut image = None;
    let mut initial_rgba = None;
    let mut marker_counts = BTreeMap::<String, usize>::new();
    let default_lighting = TerrainLighting::default();
    let lighting = if let Some(active) = population.active_body() {
        let body = pair.system().body(active)?;
        let star = pair
            .system()
            .bodies()
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing Sun"))?
            .1;
        let direction = body
            .state()
            .body_to_system()
            .inverse()
            .rotate_direction(Direction3::try_new(
                star.state().center_in_system().metres() - body.state().center_in_system().metres(),
            )?)?
            .unit();
        TerrainLighting::try_new(
            direction,
            default_lighting.ambient_strength(),
            default_lighting.diffuse_strength(),
            TerrainRenderMode::Lit,
        )?
    } else {
        default_lighting
    };
    let diagnostic_light = scene.starts_with("phase59-land-") || scene == "phase59-shoreline";
    let lighting = if diagnostic_light {
        let radial = pose.position().local().metres().normalize();
        let tangent = radial
            .cross(if radial.y.abs() < 0.9 {
                DVec3::Y
            } else {
                DVec3::X
            })
            .normalize();
        TerrainLighting::try_new(
            radial * 0.55 + tangent * 0.8,
            default_lighting.ambient_strength(),
            default_lighting.diffuse_strength(),
            TerrainRenderMode::Lit,
        )?
    } else {
        lighting
    };
    let environment = population_body
        .map(|body| {
            mundaris_app::solar_system::planetary_config(
                body,
                requests[body_index(body)].reference_radius_m,
            )
        })
        .transpose()?
        .flatten();
    let lighting = if std::env::var_os("MUNDARIS_CAPTURE_PLANETARY").is_some()
        || scene.starts_with("planetary-")
    {
        lighting.with_mode(TerrainRenderMode::Natural)
    } else {
        lighting
    };
    writeln!(
        manifest,
        "planetary_environment={environment:?} camera_pose={pose:?} radial_convergence={:?}",
        population.cover.convergence
    )?;
    if let Some(id) = population.active_body() {
        let body = world.body(id)?;
        writeln!(
            manifest,
            "terrain_definition={:?} terrain_revision={:?} identity_radius_m={}",
            body.terrain(),
            body.terrain_revision(),
            body.properties().reference_radius_m()
        )?;
    }
    let lighting = if let Some(active) = population.active_body() {
        let index = world
            .bodies()
            .position(|(id, _)| id == active)
            .ok_or_else(|| anyhow::anyhow!("active terrain body missing"))?;
        if let Some(config) = mundaris_app::solar_system::terrain_readability_config(
            SOLAR_SYSTEM_CONTENT[index].identity,
            requests[index].reference_radius_m,
        )? {
            lighting.with_readability(config)
        } else {
            lighting
        }
    } else {
        lighting
    };
    writeln!(
        manifest,
        "terrain_lighting_sun_body={:?} mode={:?} ambient={} diffuse={} lighting_source={}",
        lighting.sun_direction_body(),
        lighting.mode(),
        lighting.ambient_strength(),
        lighting.diffuse_strength(),
        if diagnostic_light {
            "diagnostic-local-side"
        } else {
            "authored-star"
        }
    )?;
    for repetition in 0..8 {
        let start = Instant::now();
        let mut frame = build_frame(
            &view,
            &mut staging,
            projection,
            &sphere,
            &requests,
            &owners,
            &population,
            lighting,
            SurfaceStyle {
                elevation_colors: population_body.is_some_and(|body| {
                    SOLAR_SYSTEM_CONTENT[body as usize].terrain_elevation_diagnostic
                }),
                ..Default::default()
            },
            environment,
        )?;
        if matches!(scene, "system" | "inner" | "earth-moon") {
            let root = frames.tree().root();
            marker_segments(
                &mut frame,
                &pair,
                root,
                pose.position().local().metres(),
                &requests,
                &marker_names,
                7.0,
            )?;
        }
        let preparation = start.elapsed();
        let rgba = capture.render(&frame)?;
        if let Some(initial) = &initial_rgba {
            ensure!(
                initial == &rgba,
                "unchanged production frame was not pixel deterministic"
            );
        } else {
            initial_rgba = Some(rgba.clone());
        }
        if repetition > 0 {
            render_preparation_ms.push(preparation.as_secs_f64() * 1000.0);
            render_encode_ms.push(capture.last_cpu_encode().as_secs_f64() * 1000.0);
        }
        if repetition == 7 {
            image = Some(rgba);
        }
        if repetition == 7 {
            for marker in frame.markers() {
                let key = format!("{:?}", marker.representation);
                *marker_counts.entry(key).or_default() += 1;
            }
            writeln!(
                manifest,
                "render_report={:?} marker_representations={marker_counts:?}",
                frame.report()
            )?;
            if matches!(scene, "system" | "inner" | "earth-moon") {
                let mut labels = Vec::new();
                for marker in frame.markers() {
                    let (id, body) = world
                        .bodies()
                        .nth(marker.request_index)
                        .ok_or_else(|| anyhow::anyhow!("label body association"))?;
                    if marker_names.iter().any(|name| name == body.name())
                        && let Some(p) = marker.screen_pixels
                    {
                        labels.push(LabelInput {
                            body: id,
                            marker: p.map(f64::from),
                            size: [body.name().len() as f64 * 8.0 + 8.0, 20.0],
                            selected: false,
                            focused: false,
                            hovered: false,
                            diameter: marker.apparent_diameter_pixels,
                            distance_m: marker.distance_m,
                        });
                    }
                }
                let mut placed = Vec::new();
                LabelLayout::default().layout(
                    &labels,
                    ScreenRect {
                        min: [0.0, 0.0],
                        max: [f64::from(WIDTH), f64::from(HEIGHT)],
                    },
                    &[],
                    &mut placed,
                );
                for label in placed {
                    writeln!(
                        manifest,
                        "navigation_label={} rect={},{},{},{} marker={},{}",
                        world.body(label.body)?.name(),
                        label.rect.min[0],
                        label.rect.min[1],
                        label.rect.max[0] - label.rect.min[0],
                        label.rect.max[1] - label.rect.min[1],
                        label.marker[0],
                        label.marker[1]
                    )?;
                }
            }
            let report = frame.report();
            writeln!(
                manifest,
                "draw_count_total={} sphere_draws={} terrain_draws={} navigation_polyline_draws={} visual_layer_draws={}",
                marker_counts.get("PhysicalSphere").copied().unwrap_or(0)
                    + report.surface.draws
                    + usize::from(report.polyline_segments > 0)
                    + report.planetary_ocean_draws
                    + report.planetary_cloud_draws
                    + report.planetary_atmosphere_draws,
                marker_counts.get("PhysicalSphere").copied().unwrap_or(0),
                report.surface.draws,
                usize::from(report.polyline_segments > 0),
                report.planetary_ocean_draws
                    + report.planetary_cloud_draws
                    + report.planetary_atmosphere_draws
            )?;
        }
    }
    update_cpu_ms.sort_by(f64::total_cmp);
    steady_update_cpu_ms.sort_by(f64::total_cmp);
    render_preparation_ms.sort_by(f64::total_cmp);
    render_encode_ms.sort_by(f64::total_cmp);
    let quantiles = |values: &[f64]| -> (f64, f64) {
        if values.is_empty() {
            (0.0, 0.0)
        } else {
            (values[values.len() / 2], values[values.len() - 1])
        }
    };
    let (update_median, update_worst) = quantiles(&update_cpu_ms);
    let (steady_update_median, steady_update_worst) = quantiles(&steady_update_cpu_ms);
    let (prep_median, prep_worst) = quantiles(&render_preparation_ms);
    let (encode_median, encode_worst) = quantiles(&render_encode_ms);
    writeln!(
        manifest,
        "host_timing_convergence_update_cpu_ms_median={update_median} worst={update_worst} samples={}\nhost_timing_steady_update_cpu_ms_median={steady_update_median} worst={steady_update_worst} samples=8 generated_samples=0\nhost_timing_render_prepare_cpu_ms_median={prep_median} worst={prep_worst} steady_samples={}\nhost_timing_render_upload_encode_cpu_ms_median={encode_median} worst={encode_worst}; draw/GPU execution excluded\n",
        update_cpu_ms.len(),
        render_preparation_ms.len()
    )?;
    writeln!(
        manifest,
        "unchanged_frame_gpu_readback_repetitions=8 pixel_identical=true color_target=Rgba8UnormSrgb"
    )?;
    for (index, (id, body)) in world.bodies().enumerate() {
        writeln!(
            manifest,
            "identity index={index} body_id={id:?} name={} color_rgba={:?} radius_m={} terrain_authored={} owner={} state={:?}",
            body.name(),
            SOLAR_SYSTEM_CONTENT[index].color,
            body.properties().reference_radius_m(),
            body.terrain().is_some(),
            owners[index],
            sessions
                .iter()
                .find(|session| session.body() == id)
                .map(PlanetSurfaceSession::state)
        )?;
    }
    let far_representations = marker_counts.get("PhysicalSphere").copied().unwrap_or(0)
        + marker_counts.get("SubpixelMarker").copied().unwrap_or(0)
        + marker_counts.get("RangeMarker").copied().unwrap_or(0)
        + marker_counts.get("PrecisionMarker").copied().unwrap_or(0);
    writeln!(
        manifest,
        "visible_far_representations={far_representations} visible_surface_representations={} visible_culled_representations={} body_count={} terrain_body_count={} active_body_pending_work={}",
        marker_counts.get("Surface").copied().unwrap_or(0),
        marker_counts.get("Culled").copied().unwrap_or(0),
        world.body_count(),
        sessions.len(),
        population.cache.pending()
    )?;
    let filename = format!("{scene}.bmp");
    bitmap(
        &output.join(&filename),
        &image.ok_or_else(|| anyhow::anyhow!("capture was not rendered"))?,
    )?;
    if matches!(scene, "adaptive" | "morph") || scene.starts_with("phase59-") {
        let frame = build_frame(
            &view,
            &mut staging,
            projection,
            &sphere,
            &requests,
            &owners,
            &population,
            lighting.with_mode(TerrainRenderMode::Elevation),
            SurfaceStyle {
                lod_colors: true,
                borders: true,
                ..Default::default()
            },
            environment,
        )?;
        bitmap(
            &output.join(format!("{scene}-lod.bmp")),
            &capture.render(&frame)?,
        )?;
    }
    if scene.starts_with("phase59-") || scene.starts_with("planetary-") {
        let cache_before_modes = population.cache.report();
        let mut mode_specs = vec![
            (
                "planetary",
                TerrainRenderMode::Natural,
                SurfaceStyle::default(),
            ),
            (
                "elevation",
                TerrainRenderMode::Elevation,
                SurfaceStyle {
                    elevation_colors: true,
                    ..Default::default()
                },
            ),
            (
                "normals",
                TerrainRenderMode::Normals,
                SurfaceStyle::default(),
            ),
            ("slope", TerrainRenderMode::Slope, SurfaceStyle::default()),
            (
                "rock-weight",
                TerrainRenderMode::RockWeight,
                SurfaceStyle::default(),
            ),
            (
                "sea-mask",
                TerrainRenderMode::SeaMask,
                SurfaceStyle::default(),
            ),
            ("lit", TerrainRenderMode::Lit, SurfaceStyle::default()),
            (
                "readability",
                TerrainRenderMode::Readability,
                SurfaceStyle::default(),
            ),
            (
                "diffuse",
                TerrainRenderMode::Diffuse,
                SurfaceStyle::default(),
            ),
            (
                "lod",
                TerrainRenderMode::Elevation,
                SurfaceStyle {
                    lod_colors: true,
                    borders: true,
                    ..Default::default()
                },
            ),
        ];
        if scene.starts_with("phase59-land-") || scene == "phase59-shoreline" {
            mode_specs.extend([
                ("slope", TerrainRenderMode::Slope, SurfaceStyle::default()),
                (
                    "sea-mask",
                    TerrainRenderMode::SeaMask,
                    SurfaceStyle::default(),
                ),
                (
                    "rock-weight",
                    TerrainRenderMode::RockWeight,
                    SurfaceStyle::default(),
                ),
            ]);
        }
        let cache_generation_snapshot = (
            cache_before_modes.misses,
            cache_before_modes.evictions,
            cache_before_modes.resident_patches,
            cache_before_modes.resident_bytes,
        );
        for (suffix, mode, style) in mode_specs {
            let mut prepare_samples_ms = Vec::with_capacity(20);
            for _ in 0..20 {
                let started = Instant::now();
                let frame = build_frame(
                    &view,
                    &mut staging,
                    projection,
                    &sphere,
                    &requests,
                    &owners,
                    &population,
                    lighting.with_mode(mode),
                    style,
                    environment,
                )?;
                std::hint::black_box(frame.report());
                prepare_samples_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            prepare_samples_ms.sort_by(f64::total_cmp);
            writeln!(
                manifest,
                "matched_mode_prepare mode={suffix} repeats=20 median_cpu_ms={} mean_cpu_ms={} worst_cpu_ms={} gpu_capture_excluded=true",
                prepare_samples_ms[10],
                prepare_samples_ms.iter().sum::<f64>() / 20.0,
                prepare_samples_ms[19]
            )?;
            let frame = build_frame(
                &view,
                &mut staging,
                projection,
                &sphere,
                &requests,
                &owners,
                &population,
                lighting.with_mode(mode),
                style,
                environment,
            )?;
            bitmap(
                &output.join(format!("{scene}-{suffix}.bmp")),
                &capture.render(&frame)?,
            )?;
        }
        // Exact-camera backface intervention: neither readiness, geometry, near
        // plane, nor ownership changes when the underside diagnostic is enabled.
        let frame = build_frame(
            &view,
            &mut staging,
            projection,
            &sphere,
            &requests,
            &owners,
            &population,
            lighting.with_mode(TerrainRenderMode::Readability),
            SurfaceStyle {
                underside: true,
                ..Default::default()
            },
            environment,
        )?;
        bitmap(
            &output.join(format!("{scene}-no-cull.bmp")),
            &capture.render(&frame)?,
        )?;
        if let Some(config) = environment {
            for (suffix, layers) in [
                ("all-layers", config),
                (
                    "no-atmosphere",
                    PlanetaryConfig {
                        atmosphere_enabled: false,
                        ..config
                    },
                ),
                (
                    "no-ocean",
                    PlanetaryConfig {
                        ocean_enabled: false,
                        ..config
                    },
                ),
                (
                    "no-clouds",
                    PlanetaryConfig {
                        clouds_enabled: false,
                        ..config
                    },
                ),
                (
                    "land-only",
                    PlanetaryConfig {
                        atmosphere_enabled: false,
                        ocean_enabled: false,
                        clouds_enabled: false,
                        ..config
                    },
                ),
            ] {
                let start = Instant::now();
                let frame = build_frame(
                    &view,
                    &mut staging,
                    projection,
                    &sphere,
                    &requests,
                    &owners,
                    &population,
                    lighting.with_mode(TerrainRenderMode::Natural),
                    SurfaceStyle::default(),
                    Some(layers),
                )?;
                let prepare_ms = start.elapsed().as_secs_f64() * 1000.0;
                let mut encode_samples = Vec::with_capacity(8);
                let mut readback_samples = Vec::with_capacity(8);
                let mut first_image = None;
                for _ in 0..8 {
                    let start = Instant::now();
                    let rgba = capture.render(&frame)?;
                    readback_samples.push(start.elapsed().as_secs_f64() * 1000.0);
                    encode_samples.push(capture.last_cpu_encode().as_secs_f64() * 1000.0);
                    if let Some(first) = &first_image {
                        ensure!(
                            first == &rgba,
                            "unchanged layer capture was not deterministic"
                        );
                    } else {
                        bitmap(&output.join(format!("{scene}-{suffix}.bmp")), &rgba)?;
                        first_image = Some(rgba);
                    }
                }
                encode_samples.sort_by(f64::total_cmp);
                readback_samples.sort_by(f64::total_cmp);
                let (encode_median, encode_worst) = quantiles(&encode_samples);
                let (readback_median, readback_worst) = quantiles(&readback_samples);
                writeln!(
                    manifest,
                    "layer_variant={suffix} config={layers:?} prepare_ms={prepare_ms} encode_ms_median={encode_median} encode_ms_worst={encode_worst} render_wait_readback_ms_median={readback_median} render_wait_readback_ms_worst={readback_worst} repeats=8 pixel_identical=true report={:?} gpu_timestamp_last={:?} upload_last={:?}",
                    frame.report(),
                    capture.last_gpu_profile(),
                    capture.last_terrain_upload_profile()
                )?;
            }
        }
        writeln!(
            manifest,
            "exact_camera_no_cull_intervention=true unchanged_near_m={} unchanged_geometry=true unchanged_ownership=true",
            projection.near_m()
        )?;
        let cache_after_modes = population.cache.report();
        ensure!(
            cache_generation_snapshot
                == (
                    cache_after_modes.misses,
                    cache_after_modes.evictions,
                    cache_after_modes.resident_patches,
                    cache_after_modes.resident_bytes
                ),
            "mode-only frame preparation changed terrain cache geometry state"
        );
        writeln!(
            manifest,
            "matched_mode_cover_identical=true mode_capture_cache_before={cache_before_modes:?} mode_capture_cache_after={cache_after_modes:?} mode_capture_cache_miss_delta={} mode_capture_eviction_delta={}",
            cache_after_modes
                .misses
                .saturating_sub(cache_before_modes.misses),
            cache_after_modes
                .evictions
                .saturating_sub(cache_before_modes.evictions)
        )?;
        if let Some(active) = population.active_body() {
            let index = world
                .bodies()
                .position(|(id, _)| id == active)
                .ok_or_else(|| anyhow::anyhow!("active body missing"))?;
            let body = world.body(active)?;
            let radius = body.properties().reference_radius_m();
            let eye = pose.position().local().metres();
            let direction = eye.normalize();
            let generator = TerrainGenerator::new(
                body.terrain()
                    .ok_or_else(|| anyhow::anyhow!("active terrain definition missing"))?,
                radius,
            )?;
            let sample = generator.evaluate_point(TerrainQuery {
                location: SurfaceLocation::new(Direction3::try_new(direction)?),
                footprint: TerrainFootprint::COMPLETE,
            })?;
            let ready_radius = drawn_surface_radius(&population, direction)?;
            let center_distance = eye.length();
            let location = SurfaceLocation::new(Direction3::try_new(direction)?);
            let ready_probe =
                mundaris_app::surface_probe::ready_mesh_probe(&population.cover, location)?;
            let local_patch =
                mundaris_app::surface_probe::patch_at_location(&population.cover, location);
            for footprint_m in [
                1562.5,
                781.25,
                195.3125,
                24.4140625,
                3.0517578125,
                0.3814697265625,
                0.0,
            ] {
                let footprint = TerrainFootprint::new(footprint_m)?;
                let query = TerrainQuery {
                    location,
                    footprint,
                };
                let represented = generator.evaluate_point(query)?;
                let erosion = generator.erosion_diagnostics(query)?;
                writeln!(
                    manifest,
                    "filtered_query footprint_m={footprint_m} height_m={} slope_degrees={} erosion={erosion:?}",
                    represented.height_m(),
                    represented.slope_angle_rad(radius)?.to_degrees()
                )?;
            }
            let query_directions: Vec<_> = (0..1024)
                .map(|index| {
                    let z = 1.0 - 2.0 * (index as f64 + 0.5) / 1024.0;
                    let angle = index as f64 * (std::f64::consts::PI * (3.0 - 5.0_f64.sqrt()));
                    DVec3::new(
                        (1.0 - z * z).sqrt() * angle.cos(),
                        (1.0 - z * z).sqrt() * angle.sin(),
                        z,
                    )
                })
                .collect();
            let query_locations: Vec<_> = query_directions
                .iter()
                .map(|d| {
                    SurfaceLocation::new(Direction3::try_new(*d).expect("unit Fibonacci direction"))
                })
                .collect();
            let query_started = Instant::now();
            for location in &query_locations {
                std::hint::black_box(generator.evaluate_point(TerrainQuery {
                    location: *location,
                    footprint: TerrainFootprint::COMPLETE,
                })?);
            }
            let direct_query_ms = query_started.elapsed().as_secs_f64() * 1000.0;
            let construction_started = Instant::now();
            for location in &query_locations {
                let fresh = TerrainGenerator::new(
                    body.terrain()
                        .ok_or_else(|| anyhow::anyhow!("terrain missing"))?,
                    radius,
                )?;
                std::hint::black_box(fresh.evaluate_point(TerrainQuery {
                    location: *location,
                    footprint: TerrainFootprint::COMPLETE,
                })?);
            }
            let construct_query_ms = construction_started.elapsed().as_secs_f64() * 1000.0;
            let mut heights = Vec::with_capacity(query_locations.len());
            let mut slopes = Vec::with_capacity(query_locations.len());
            let mut below_sea = 0usize;
            let mut steep_12 = 0usize;
            let mut steep_35 = 0usize;
            let mut steep_8 = 0usize;
            let mut steep_16 = 0usize;
            for location in &query_locations {
                let s = generator.evaluate_point(TerrainQuery {
                    location: *location,
                    footprint: TerrainFootprint::COMPLETE,
                })?;
                heights.push(s.height_m());
                let slope = s.slope_angle_rad(radius)?.to_degrees();
                slopes.push(slope);
                below_sea += usize::from(
                    s.height_m()
                        < mundaris_app::solar_system::reference_sea_level_m(
                            SOLAR_SYSTEM_CONTENT[index].identity,
                        )
                        .unwrap_or(0.0),
                );
                steep_12 += usize::from(slope >= 12.0);
                steep_35 += usize::from(slope >= 35.0);
                steep_8 += usize::from(slope >= 8.0);
                steep_16 += usize::from(slope >= 16.0);
            }
            heights.sort_by(f64::total_cmp);
            slopes.sort_by(f64::total_cmp);
            writeln!(
                manifest,
                "terrain_query_cost_direct_1024_ms={direct_query_ms} including_generator_construction_1024_ms={construct_query_ms} morphology_samples=1024 height_quantiles_m={:?} slope_quantiles_degrees={:?} water_fraction_at_sea_datum={} slope_fraction_ge_12={} slope_fraction_ge_35={}",
                [
                    heights[25],
                    heights[256],
                    heights[512],
                    heights[768],
                    heights[998]
                ],
                [
                    slopes[25],
                    slopes[256],
                    slopes[512],
                    slopes[768],
                    slopes[998]
                ],
                below_sea as f64 / 1024.0,
                steep_12 as f64 / 1024.0,
                steep_35 as f64 / 1024.0
            )?;
            writeln!(
                manifest,
                "content_palette sea_level_m={} slope_blend_degrees=8..16 fraction_ge_8={} fraction_ge_16={}",
                mundaris_app::solar_system::reference_sea_level_m(
                    SOLAR_SYSTEM_CONTENT[index].identity
                )
                .unwrap_or(0.0),
                steep_8 as f64 / 1024.0,
                steep_16 as f64 / 1024.0
            )?;
            let (body_index, body_owner) = owners
                .iter()
                .copied()
                .enumerate()
                .find(|(_, owns)| *owns)
                .unwrap_or((index, false));
            writeln!(
                manifest,
                "phase59_clearance center_distance_m={center_distance} sphere_altitude_m={} terrain_height_m={} terrain_slope_degrees={} analytic_clearance_m={} ready_mesh_clearance_m={} inside_analytic_terrain={} inside_ready_mesh={} ready_mesh_radius_m={ready_radius} direction_body_fixed={direction:?} cover={} visible={} ready={} pending={} morph_active={} levels={levels:?} visible_levels={visible_levels:?} quality_pending={} settled={} all_finite={}",
                eye.length() - radius,
                sample.height_m(),
                sample.slope_angle_rad(radius)?.to_degrees(),
                eye.length() - radius - sample.height_m(),
                eye.length() - ready_radius,
                eye.length() < radius + sample.height_m(),
                eye.length() < ready_radius,
                population.cover.active().len(),
                population.cover.visible().len(),
                population.cover.ready(),
                population.cache.pending(),
                population.cover.transition().is_some(),
                population.cover.report.quality_pending,
                population.cover.report.settled,
                center_distance.is_finite()
                    && eye.is_finite()
                    && radius.is_finite()
                    && sample.height_m().is_finite()
                    && ready_radius.is_finite()
            )?;
            writeln!(
                manifest,
                "terrain_under_camera patch={local_patch:?} ready_probe={ready_probe:?} ownership_body_index={body_index} owns_surface={body_owner} min_visible_level={:?} max_visible_level={:?} frustum_culled={} horizon_culled={} max_projected_error_pixels={} ready_mesh_clearance_m={} near_plane_m={}",
                visible_levels.keys().next(),
                visible_levels.keys().next_back(),
                population.cover.report.frustum_culled,
                population.cover.report.horizon_culled,
                population.cover.report.max_error_pixels,
                eye.length() - ready_radius,
                projection.near_m()
            )?;
        }
    }
    fs::write(output.join(format!("{scene}-manifest.txt")), &manifest)?;
    println!("{scene}: captured {filename}\n{manifest}");
    Ok(())
}

fn parse_max_updates(arg: Option<String>) -> Result<usize> {
    let configured = arg.or_else(|| std::env::var("MUNDARIS_PHASE58_MAX_UPDATES").ok());
    let value = configured.map_or(Ok(DEFAULT_MAX_UPDATES), |v| v.parse::<usize>())?;
    ensure!(
        value > 0 && value <= 10_000,
        "max updates must be in 1..=10000"
    );
    Ok(value)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let output = Path::new(&args.next().unwrap_or_else(|| "target/phase58".into())).to_path_buf();
    let selected = args.next().unwrap_or_else(|| "all".into());
    let max_updates = parse_max_updates(args.next())?;
    ensure!(
        args.next().is_none(),
        "usage: solar_system_capture [output] [scene|all] [max_updates]"
    );
    fs::create_dir_all(&output)?;
    let selected_scenes: Vec<_> = if selected == "all" {
        SCENES.to_vec()
    } else if selected == "phase59" {
        vec![
            "phase59-100000",
            "phase59-10000",
            "phase59-1000",
            "phase59-100",
            "phase59-10",
            "phase59-2",
            "phase59-100-legacy",
            "phase59-10-legacy",
            "phase59-2-legacy",
            "phase59-100-guarded",
            "phase59-10-guarded",
            "phase59-2-guarded",
            "phase59-mars-10",
            "phase59-moon-10",
            "phase59-high-orbit",
            "phase59-coastline",
            "phase59-land-mountain",
            "phase59-land-plain",
            "phase59-land-ridge",
            "phase59-land-gully",
            "phase59-shoreline",
        ]
    } else if selected == "phase59-landforms" {
        vec![
            "phase59-land-mountain",
            "phase59-land-plain",
            "phase59-land-ridge",
            "phase59-land-gully",
            "phase59-shoreline",
        ]
    } else {
        ensure!(
            SCENES.contains(&selected.as_str()),
            "unknown scene {selected}; available: {SCENES:?}"
        );
        vec![selected.as_str()]
    };
    let mut capture = TerrainCaptureRenderer::new(WIDTH, HEIGHT)?;
    for scene in selected_scenes {
        run_scene(scene, &output, max_updates, &mut capture)?;
    }
    Ok(())
}
