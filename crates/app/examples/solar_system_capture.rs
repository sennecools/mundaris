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
    CelestialRenderBody, CelestialStaging, Icosphere, PreparedView, RenderPrecisionBudget,
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
        "mars" => SolarBody::Mars,
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
    let (direction, clearance, horizon) = match scene {
        "high-orbit" => (star_direction, 600_000.0, false),
        "adaptive" => (star_direction, 200_000.0, false),
        "low-orbit" => (star_direction, 40_000.0, false),
        "high-altitude" => (star_direction, 10_000.0, false),
        "mountain" | "morph" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            3_000.0,
            false,
        ),
        "near-ground" => (star_direction, 20.0, true),
        "compare600" | "compare800" | "compare1000" => (star_direction, 600_000.0, false),
        "mars" | "jupiter" | "saturn" | "neptune" => (star_direction, radius * 2.0, false),
        _ => anyhow::bail!("unknown scene {scene}"),
    };
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
    let target_point = direction * (radius + height_m);
    let eye = target_point + direction * clearance;
    let off_nadir_degrees: f64 = match scene {
        "low-orbit" => 55.0,
        "high-altitude" => 70.0,
        "mountain" | "morph" => 78.0,
        "near-ground" => 90.0,
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
            "target={target:?} clearance_above_sampled_terrain_m={clearance} sampled_complete_height_m={height_m} eye_body_fixed_m={eye:?} look_horizon={horizon} off_nadir_degrees={off_nadir_degrees} configured_diameter_km={}",
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
) -> Result<CelestialFrame<'view, 'tree, 'storage>> {
    let mut frame = CelestialFrame::new(view, staging, projection, sphere);
    frame.set_terrain_lighting(lighting);
    if let Some(index) = owners.iter().position(|&owner| owner)
        && population.cover.ready()
    {
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

    if scene == "near-ground" {
        let direction = pose.position().local().metres().normalize();
        let drawn_radius = drawn_surface_radius(&population, direction)?;
        let old_eye_radius = pose.position().local().metres().length();
        let eye_radius = old_eye_radius.max(drawn_radius + 20.0);
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
            "\ninspection_only_radial_adjustment_m={} drawn_surface_radius_m={drawn_radius} clearance_above_ready_mesh_m={}",
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
        "Phase 5.8 gameplay authored-system capture\nscene={scene}\npreset=gameplay body_radius_scale={} orbital_distance_scale={} viewport={WIDTH}x{HEIGHT} fov_degrees=60\nscene_geometry={scene_description}\nlabels=manifest-only; screen-space identity markers are navigational aids and do not change physical scales\nadapter={} gpu_timestamps_unavailable=true\nupdates={updates} update_limit={max_updates} update_vertex_budget={VERTEX_BUDGET} elapsed_step_ms=16 morph_ms={}\nrequested_target={target:?} active_body={population_body:?} active_body_id={:?} surface_owner_count={}\nactive_cover_patches={} visible_cover_patches={} levels={levels:?} visible_levels={visible_levels:?} ready={} quality_pending={} settled={} budget_constrained={} max_level={} max_error_px={}\ncache_resident_patches={} cache_resident_bytes={} cache_pinned_patches={} cache_external_bytes={} cache_peak_aggregate_bytes={} cache_hits={} cache_misses={} cache_evictions={} pending_final={} pending_max={} vertices_per_update_max={} generated_samples_total={}\nmorph_active_updates={} morph_build_count={} morph_build_ms_total={} morph_build_ms_worst={} morph_transition_active={} morph_fraction={:?} morph_capture={}\n",
        preset.body_radius_scale,
        preset.orbital_distance_scale,
        capture.adapter_name(),
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
    let mut staging = CelestialStaging::default();
    let mut render_preparation_ms = Vec::new();
    let mut render_encode_ms = Vec::new();
    let mut image = None;
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
    writeln!(
        manifest,
        "terrain_lighting_sun_body={:?} mode=Lit ambient={} diffuse={}",
        lighting.sun_direction_body(),
        lighting.ambient_strength(),
        lighting.diffuse_strength()
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
                "draw_count_total={} sphere_draws={} terrain_draws={} navigation_polyline_draws={}",
                marker_counts.get("PhysicalSphere").copied().unwrap_or(0)
                    + report.surface.draws
                    + usize::from(report.polyline_segments > 0),
                marker_counts.get("PhysicalSphere").copied().unwrap_or(0),
                report.surface.draws,
                usize::from(report.polyline_segments > 0)
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
    if matches!(scene, "adaptive" | "morph") {
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
        )?;
        bitmap(
            &output.join(format!("{scene}-lod.bmp")),
            &capture.render(&frame)?,
        )?;
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
