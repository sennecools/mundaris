//! Bounded headless wall-clock convergence probe using the production population route.
use anyhow::{Context, Result, ensure};
use glam::{DMat3, DQuat, DVec3};
use mundaris_app::{
    planet_surface::PlanetSurfaceSession, planet_terrain::*, solar_system::*,
    terrain_population::TerrainPopulation,
};
use mundaris_math::{surface::SurfaceLocation, *};
use mundaris_renderer::planet_surface::{SurfaceStyle, TerrainLighting, TerrainRenderMode};
use mundaris_renderer::*;
use mundaris_world::{terrain::*, *};
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    num::NonZeroU64,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

const WIDTH: u32 = 768;
const HEIGHT: u32 = 512;
const TIMES_MS: [u64; 8] = [0, 100, 250, 500, 1000, 2000, 3000, 5000];
const DIRECTION: DVec3 = DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625);
const CLEARANCES_M: [(&str, f64); 7] = [
    ("high-orbit", 600_000.0),
    ("100km", 100_000.0),
    ("10km", 10_000.0),
    ("1km", 1_000.0),
    ("100m", 100.0),
    ("10m", 10.0),
    ("2m", 2.0),
];

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output = PathBuf::from(
        args.next()
            .context("usage: terrain_convergence_probe OUTPUT_DIR")?,
    );
    let workers_arg: Option<usize> = args
        .next()
        .map(|s| s.to_string_lossy().parse())
        .transpose()?;
    let morph_ms: u64 = args
        .next()
        .map(|s| s.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(150);
    let mode = args
        .next()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "descent".into());
    ensure!(
        ["descent", "cold-2m", "motion", "switch"].contains(&mode.as_str()),
        "unknown probe mode"
    );
    let workers = workers_arg.unwrap_or(if mode == "switch" { 4 } else { 0 });
    #[cfg(feature = "terrain-capture")]
    let capture = args.next().is_some_and(|s| s == "capture");
    ensure!(
        args.next().is_none(),
        "usage: terrain_convergence_probe OUTPUT_DIR [WORKERS] [MORPH_MS] [descent|cold-2m|motion|switch] [capture]"
    );
    fs::create_dir_all(&output)?;

    let namespace = NonZeroU64::new(5_100).context("invalid namespace")?;
    let world = SolarSystemPreset::gameplay().create(namespace)?;
    let frames = CelestialFrameProjection::build(&world, namespace)?;
    let mut sessions = Vec::new();
    let mut requests = Vec::new();
    for (id, body) in world.bodies() {
        let color = SOLAR_SYSTEM_CONTENT
            .iter()
            .find(|content| content.name == body.name())
            .context("body missing solar content")?
            .color;
        requests.push(CelestialRenderBody {
            body_fixed_frame: frames.frames_for(id)?.body_fixed,
            reference_radius_m: body.properties().reference_radius_m(),
            color,
            unlit: false,
            selected: false,
        });
        if body.terrain().is_some() {
            sessions.push(PlanetSurfaceSession::new(id, MAX_TERRAIN_PATCHES)?);
        }
    }
    let earth_index = SolarBody::Earth as usize;
    let earth_id = world.bodies().nth(earth_index).context("missing Earth")?.0;
    let definition = world
        .body(earth_id)?
        .terrain()
        .context("Earth terrain missing")?;
    let radius = requests[earth_index].reference_radius_m;
    let generator = TerrainGenerator::new(definition, radius)?;
    let direction = DIRECTION.normalize();
    let complete_height = generator
        .evaluate_point(TerrainQuery {
            location: SurfaceLocation::new(Direction3::try_new(direction)?),
            footprint: TerrainFootprint::COMPLETE,
        })?
        .height_m();
    let projection = CelestialProjection::try_new(WIDTH, HEIGHT, 60.0_f64.to_radians(), 0.1)?;
    let sphere = Icosphere::new();
    let mut staging = CelestialStaging::default();
    #[cfg(feature = "terrain-capture")]
    let mut gpu = if capture {
        Some(mundaris_renderer::terrain_capture::TerrainCaptureRenderer::new(WIDTH, HEIGHT)?)
    } else {
        None
    };
    #[cfg(feature = "terrain-capture")]
    let capture_adapter = gpu.as_ref().map_or("none", |gpu| gpu.adapter_name());
    #[cfg(not(feature = "terrain-capture"))]
    let capture_adapter = "none";
    #[cfg(feature = "terrain-capture")]
    let capture_timestamps = gpu.as_ref().map_or_else(
        || "not_requested".to_string(),
        |gpu| format!("{:?}", gpu.timestamp_availability()),
    );
    #[cfg(not(feature = "terrain-capture"))]
    let capture_timestamps = "capture_feature_disabled";
    fs::write(
        output.join("probe-manifest.txt"),
        format!(
            "preset=gameplay mode={mode} workers={workers} morph_ms={morph_ms}\nviewport={WIDTH}x{HEIGHT} fov_degrees=60 near_m=0.1 frame_opportunity_ms=16\nrequested_times_ms={TIMES_MS:?}\nearth_radius_m={radius} direction_body_fixed={direction:?} earth_complete_height_m={complete_height}\nearth_terrain={definition:?}\ncapture_adapter={capture_adapter} gpu_timestamp_capability={capture_timestamps}\nquality=radial-source-LOD-is-not-screen-wide-quality\nevidence=production-route-wall-clock-and-optional-native-offscreen-not-human-interactive-acceptance\n"
        ),
    )?;
    let mut repository = String::new();
    for args in [vec!["rev-parse", "HEAD"], vec!["status", "--short"]] {
        let value = std::process::Command::new("git")
            .args(&args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_else(|| "unavailable".into());
        repository.push_str(&format!("repository_command={args:?}\n{value}\n"));
    }
    fs::write(output.join("repository.txt"), repository)?;
    let mut population = TerrainPopulation::with_worker_count(workers)?;
    let headroom_mib: usize = std::env::var("MUNDARIS_TERRAIN_HEADROOM_MIB")
        .ok()
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(0);
    population
        .cache
        .set_soft_residency_headroom(headroom_mib * 1024 * 1024);
    fs::write(
        output.join("headroom.txt"),
        format!(
            "soft_headroom_mib={headroom_mib}; critical pinned replacement jobs may use the hard quota after all disposable raw entries are reclaimed\n"
        ),
    )?;
    let mut owners = vec![false; requests.len()];
    let mut csv = BufWriter::new(File::create(output.join("timeline.csv"))?);
    writeln!(
        csv,
        "view,target_ms,actual_ms,update_ms,cover_ready,owner,active_patches,visible_patches,active_levels,visible_levels,desired_patches,cover_patches,pending_patches,vertices_generated,patches_completed,selection_ms,generation_ms,stitch_ms,morph_build_ms,quality_pending,settled,budget_constrained,desired_local_lod,ready_local_lod,rendered_local_lod,render_prepare_ms,transition_deferred,body_index,body_id,clearance_m"
    )?;

    let mut frames_csv = BufWriter::new(File::create(output.join("frames.csv"))?);
    let mut detail = BufWriter::new(File::create(output.join("detail.csv"))?);
    writeln!(
        detail,
        "view,actual_ms,stitched_append_ms,transition_append_ms,far_append_ms,closure_parent,closure_dependencies,balance_parents,closure_local,completed_patches,completed_replacement_useful,completed_local_useful,total_completed,local_useful_total,useful_ratio,queued,worker_jobs,accounted_bytes,reservation_rejected,eviction_attempts,bytes_freed,evictions,hits,misses,soft_waterline_misses,desired_spacing_m,rendered_spacing_m,desired_error_px,restaged_patches,bytes_staged,stitch_reused,active_morph,construction_pending,proof_hits,proof_misses,successor_ready,morph_fraction,morph_duration_ms,overlay_budget_bytes,last_overlay_granted_bytes"
    )?;
    let mut memory_csv = BufWriter::new(File::create(output.join("memory.csv"))?);
    writeln!(
        memory_csv,
        "view,actual_ms,raw_and_bookkeeping_bytes,pinned_raw_bytes,worker_reserved_bytes,worker_fixed_bytes,completed_unpublished_bytes,stitched_source_bytes,morph_mesh_bytes,selector_scratch_bytes,renderer_outgoing_capacity_bytes,renderer_boundary_and_proof_capacity_bytes,renderer_staging_allowance_bytes,frame_end_accounted_bytes,peak_accounted_bytes"
    )?;
    #[cfg(feature = "surface-profile")]
    let mut render_profiles = BufWriter::new(File::create(output.join("render-profiles.txt"))?);
    let mut completed_total = 0usize;
    let mut useful_total = 0usize;
    #[cfg(feature = "surface-profile")]
    let mut stages = BufWriter::new(File::create(output.join("stages.csv"))?);
    #[cfg(feature = "surface-profile")]
    writeln!(
        stages,
        "frame,view,actual_ms,clearance_query_ms,population_ms,update_thread_cpu_ms,prepare_thread_cpu_ms,update_ms,prepare_ms"
    )?;
    #[cfg(feature = "surface-profile")]
    let mut frame_number = 0usize;
    writeln!(
        frames_csv,
        "view,actual_ms,update_ms,local_ready_lod,local_rendered_lod,pending,queued,worker_jobs,worker_cpu_ms,samples_completed,selection_ms,publication_ms,stitch_main_ms,morph_main_ms,stitch_worker_ms,morph_worker_ms,construction_pending,cancellations,accounted_bytes,render_prepare_ms,desired_local_lod,transition_deferred,active_morphs,diagnostics_ms,body_index,body_id,clearance_m,cache_bytes,worker_reservations,worker_fixed_bytes,completed_cover_reservation,external_bytes,peak_accounted_bytes,transition_peak_bytes,cover_publication_ms,scheduling_ms,stitched_reused,sphere_error_m,interpolation_error_m,unresolved_error_m,boundary_error_m,morph_error_m,numeric_error_m"
    )?;
    #[cfg(feature = "surface-profile")]
    let mut transition_profiles =
        BufWriter::new(File::create(output.join("transition-profiles.log"))?);
    #[cfg(feature = "surface-profile")]
    writeln!(transition_profiles, "view,actual_ms,transition_profile")?;
    #[cfg(feature = "terrain-capture")]
    let mut gpu_profiles = BufWriter::new(File::create(output.join("gpu-profiles.txt"))?);
    for (view_name, clearance) in CLEARANCES_M
        .into_iter()
        .filter(|(_, c)| match mode.as_str() {
            "cold-2m" => *c == 2.0,
            "motion" | "switch" => *c == CLEARANCES_M[0].1,
            _ => true,
        })
    {
        let start = Instant::now();
        let mut next = 0;
        let mut previous_update = start;
        let mut deadline = start;
        while next < TIMES_MS.len() {
            if Instant::now() < deadline {
                thread::sleep(deadline.saturating_duration_since(Instant::now()));
                continue;
            }
            let update_start = Instant::now();
            #[cfg(feature = "surface-profile")]
            let update_cpu = mundaris_renderer::planet_surface::CpuStageTimer::new();
            let elapsed = update_start.duration_since(previous_update);
            previous_update = update_start;
            let route_seconds = start.elapsed().as_secs_f64().min(5.0);
            let switch_bodies = [
                SolarBody::Earth,
                SolarBody::Moon,
                SolarBody::Mars,
                SolarBody::Earth,
            ];
            let target_body = if mode == "switch" {
                switch_bodies
                    [((route_seconds * 1000.0 / 350.0).floor() as usize) % switch_bodies.len()]
            } else {
                SolarBody::Earth
            };
            let body_index = target_body as usize;
            let body_id = world
                .bodies()
                .nth(body_index)
                .context("missing route body")?
                .0;
            let body = world.body(body_id)?;
            let body_radius = requests[body_index].reference_radius_m;
            let body_definition = body.terrain().context("route body terrain missing")?;
            let clearance_query_start = Instant::now();
            let body_generator = TerrainGenerator::new(body_definition, body_radius)?;
            let orbit_fraction = if mode == "motion" {
                if route_seconds < 1.0 {
                    1.0 - route_seconds
                } else if route_seconds < 3.0 {
                    0.0
                } else {
                    ((route_seconds - 3.0) / 2.0).powi(2)
                }
            } else {
                0.0
            };
            let frame_direction = if mode == "motion" && route_seconds >= 1.0 {
                DQuat::from_axis_angle(DVec3::Y, (route_seconds - 1.0).min(2.0) * 0.015) * direction
            } else {
                direction
            }
            .normalize();
            let frame_height = body_generator
                .evaluate_point(TerrainQuery {
                    location: SurfaceLocation::new(Direction3::try_new(frame_direction)?),
                    footprint: TerrainFootprint::COMPLETE,
                })?
                .height_m();
            let clearance_query_ms = clearance_query_start.elapsed().as_secs_f64() * 1000.0;
            let frame_clearance = if mode == "motion" {
                if route_seconds < 1.0 {
                    100.0 + (600_000.0 - 100.0) * orbit_fraction
                } else if route_seconds < 3.0 {
                    100.0
                } else {
                    100.0 + (600_000.0 - 100.0) * orbit_fraction
                }
            } else if mode == "switch" {
                10.0
            } else {
                clearance
            };
            let local_eye = frame_direction * (body_radius + frame_height + frame_clearance);
            let pair = frames.coherent_view(&world)?;
            let back = if mode == "motion" && route_seconds >= 1.0 {
                let tangent = -DVec3::Y.cross(frame_direction).normalize();
                let angle = ((route_seconds - 3.0).max(0.0) / 0.5).min(1.0) * std::f64::consts::PI;
                let turned = DQuat::from_axis_angle(frame_direction, angle) * tangent;
                turned
                    .lerp(
                        frame_direction,
                        ((route_seconds - 3.5) / 1.5).clamp(0.0, 1.0),
                    )
                    .normalize()
            } else if frame_clearance <= 10.0 {
                -frame_direction.cross(DVec3::Y).normalize()
            } else {
                frame_direction
            };
            let radial_right = frame_direction.cross(back);
            let right = if radial_right.length_squared() > 1e-12 {
                radial_right.normalize()
            } else {
                DVec3::Y.cross(back).normalize()
            };
            let up = back.cross(right).normalize();
            let view = PreparedView::new(
                &pair.evaluation(),
                FramePose::new(
                    FramePosition::new(
                        requests[body_index].body_fixed_frame,
                        LocalPosition::try_metres(local_eye)?,
                    ),
                    UnitRotation::try_from_quaternion(DQuat::from_mat3(&DMat3::from_cols(
                        right, up, back,
                    )))?,
                ),
                RenderPrecisionBudget::near_debug(),
            )?;
            let population_start = Instant::now();
            population.update(
                &pair,
                &view,
                projection,
                &requests,
                &mut sessions,
                &mut owners,
                &sphere,
                true,
                Duration::from_millis(morph_ms),
                64,
                Some(Duration::from_millis(2)),
                elapsed,
            )?;
            let population_ms = population_start.elapsed().as_secs_f64() * 1000.0;
            let key = TerrainGeometryIdentity::new(
                body_id,
                body_definition.clone(),
                body.terrain_revision(),
                body_radius,
            )?;
            if population.cover.ready() {
                ensure!(
                    population.active_body() == Some(body_id)
                        && population
                            .cover
                            .active()
                            .iter()
                            .all(|patch| population.cache.peek(&key, patch.address).is_some()),
                    "ready cover contains geometry from a stale body or radius candidate"
                );
            }
            #[cfg(feature = "surface-profile")]
            if !population.cover.result_publication.is_zero() {
                if let Some((mesh, _)) = population.cover.transition() {
                    writeln!(
                        transition_profiles,
                        "{view_name},{:.3},{:?}",
                        start.elapsed().as_secs_f64() * 1000.0,
                        mesh.profile()
                    )?;
                } else {
                    writeln!(
                        transition_profiles,
                        "{view_name},{:.3},no-active-transition",
                        start.elapsed().as_secs_f64() * 1000.0
                    )?;
                }
            }
            let update_ms = update_start.elapsed().as_secs_f64() * 1000.0;
            #[cfg(feature = "surface-profile")]
            let update_thread_cpu = update_cpu.thread_elapsed();
            let preparation_start = Instant::now();
            #[cfg(feature = "surface-profile")]
            let prepare_cpu = mundaris_renderer::planet_surface::CpuStageTimer::new();
            let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
            let mut lighting = TerrainLighting::try_new(
                (frame_direction * 0.55 + right * 0.8).normalize(),
                0.06,
                0.94,
                TerrainRenderMode::Readability,
            )?;
            if let Some(palette) = terrain_readability_config(target_body, body_radius)? {
                lighting = lighting.with_readability(palette);
            }
            frame.set_terrain_lighting(lighting);
            let mut stitched_append_ms = 0.0;
            let mut transition_append_ms = 0.0;
            let owner_count = owners.iter().filter(|owner| **owner).count();
            ensure!(owner_count <= 1, "multiple surface owners published");
            if let Some(owner_index) = owners.iter().position(|owner| *owner) {
                ensure!(
                    owner_index == body_index,
                    "surface published for a stale body candidate"
                );
            }
            if owners[body_index] {
                ensure!(
                    population.active_body() == Some(body_id),
                    "surface owner does not match current body"
                );
                ensure!(
                    population
                        .cover
                        .surface()
                        .context("missing ready surface")?
                        .patches()
                        .iter()
                        .all(|p| p.reference_radius_m() == body_radius),
                    "cover radius does not match the current candidate body"
                );
                let append_start = Instant::now();
                frame.append_stitched_surface(
                    requests[body_index],
                    population.cover.visible(),
                    population.cover.surface().context("missing surface")?,
                    population.cover.topology().context("missing topology")?,
                    SurfaceStyle::default(),
                )?;
                stitched_append_ms = append_start.elapsed().as_secs_f64() * 1000.0;
                if let Some((mesh, fraction)) = population.cover.transition() {
                    let append_start = Instant::now();
                    frame.append_surface_transition(
                        requests[body_index],
                        mesh,
                        fraction,
                        SurfaceStyle::default(),
                    )?;
                    transition_append_ms = append_start.elapsed().as_secs_f64() * 1000.0;
                }
            }
            let far: Vec<_> = requests
                .iter()
                .enumerate()
                .filter(|(i, _)| !owners[*i])
                .map(|(_, r)| *r)
                .collect();
            let far_start = Instant::now();
            frame.append_bodies(&far)?;
            let far_append_ms = far_start.elapsed().as_secs_f64() * 1000.0;
            let render_ms = preparation_start.elapsed().as_secs_f64() * 1000.0;
            #[cfg(feature = "surface-profile")]
            let prepare_thread_cpu = prepare_cpu.thread_elapsed();
            #[cfg(feature = "surface-profile")]
            {
                let cpu_ms = |d: Option<Duration>| {
                    d.map(|d| format!("{:.6}", d.as_secs_f64() * 1000.0))
                        .unwrap_or_else(|| "unavailable".into())
                };
                writeln!(
                    stages,
                    "{frame_number},{view_name},{:.3},{clearance_query_ms:.6},{population_ms:.6},{},{},{update_ms:.6},{render_ms:.6}",
                    start.elapsed().as_secs_f64() * 1000.0,
                    cpu_ms(update_thread_cpu),
                    cpu_ms(prepare_thread_cpu)
                )?;
            }
            #[cfg(not(feature = "surface-profile"))]
            let _ = (clearance_query_ms, population_ms);
            let surface_report = frame.report().surface;
            #[cfg(feature = "surface-profile")]
            writeln!(
                render_profiles,
                "{view_name},{:.3},frame={frame_number},render={:?},selection={:?},adaptive={:?},population={:?},work={:?}",
                start.elapsed().as_secs_f64() * 1000.0,
                surface_report.profile,
                population.cover.report.profile,
                population.cover.profile,
                population.profile,
                population.work
            )?;
            #[cfg(feature = "surface-profile")]
            {
                frame_number += 1;
            }
            let (face, uv) = SurfaceLocation::new(Direction3::try_new(frame_direction)?).face_uv();
            let address_at = |level: u8| {
                mundaris_math::surface::CubePatchAddress::try_new(
                    face,
                    level,
                    (((uv[0] + 1.0) * 0.5 * (1u64 << level) as f64).floor() as u32)
                        .min((1u32 << level) - 1),
                    (((uv[1] + 1.0) * 0.5 * (1u64 << level) as f64).floor() as u32)
                        .min((1u32 << level) - 1),
                )
            };
            let local = address_at(30)?;
            let ready_lod = (0..=30).rev().find(|&level| {
                address_at(level).is_ok_and(|a| population.cache.peek(&key, a).is_some())
            });
            let rendered_lod = population
                .cover
                .active()
                .iter()
                .find(|p| p.address.contains(local))
                .map(|p| p.address.level());
            let c = population.cache.report();
            completed_total += population.work.patches_completed;
            useful_total += population.work.local_useful_completed;
            let r = population.cover.report;
            let d = population.cover.convergence;
            writeln!(
                detail,
                "{view_name},{:.3},{stitched_append_ms:.6},{transition_append_ms:.6},{far_append_ms:.6},\"{:?}\",{},{},{},{},{},{},{completed_total},{useful_total},{:.6},{},{},{},{},{},{},{},{},{},{},{:.9e},{:.9e},{:.9e},{},{},{},{},{},{},{},{},{:.6},{},{},{}",
                start.elapsed().as_secs_f64() * 1000.0,
                r.refinement_parent,
                r.refinement_dependencies,
                r.refinement_balance_parents,
                r.refinement_local,
                population.work.patches_completed,
                population.work.replacement_useful_completed,
                population.work.local_useful_completed,
                if completed_total == 0 {
                    0.0
                } else {
                    useful_total as f64 / completed_total as f64
                },
                c.queued_patches,
                c.worker_jobs,
                c.resident_bytes + c.external_bytes,
                c.reservation_rejected,
                c.eviction_attempts,
                c.eviction_freed_bytes,
                c.evictions,
                c.hits,
                c.misses,
                c.soft_waterline_misses,
                d.desired_sample_spacing_m,
                d.rendered_sample_spacing_m,
                d.desired_total_pixels,
                surface_report.patches,
                surface_report.uploaded_bytes,
                population.cover.surface().map_or(0, |s| s.reused_patches()),
                population.cover.transition().is_some(),
                population.cover.construction_pending(),
                surface_report.proof_cache_hits,
                surface_report.proof_cache_misses,
                population.cover.successor_ready(),
                population
                    .cover
                    .transition()
                    .map_or(0.0, |(_, fraction)| fraction),
                population
                    .cover
                    .active_morph_duration()
                    .map_or(0, |d| d.as_millis()),
                population.cover.construction_budget_bytes(),
                population.cover.last_construction_budget_bytes()
            )?;
            writeln!(
                memory_csv,
                "{view_name},{:.3},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                start.elapsed().as_secs_f64() * 1000.0,
                c.resident_bytes.saturating_sub(
                    c.worker_reserved_bytes + c.worker_fixed_bytes + c.completed_unpublished_bytes
                ),
                c.pinned_bytes,
                c.worker_reserved_bytes,
                c.worker_fixed_bytes,
                c.completed_unpublished_bytes,
                population.cover.surface().map_or(0, |s| s.resident_bytes()),
                population
                    .cover
                    .transition()
                    .map_or(0, |(m, _)| m.resident_bytes()),
                r.scratch_bytes,
                surface_report.allocated_staging_bytes,
                surface_report.boundary_bytes,
                72 * 1024 * 1024 + 32 * 1024,
                c.resident_bytes + c.external_bytes,
                c.peak_aggregate_bytes
            )?;
            writeln!(
                frames_csv,
                "{view_name},{:.3},{update_ms:.6},{ready_lod:?},{rendered_lod:?},{},{},{},{:.6},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{},{},{render_ms:.6},{:?},{},{},{:.6},{body_index},\"{body_id:?}\",{frame_clearance:.3},{},{},{},{},{},{},{},{:.6},{:.6},{},{:.9e},{:.9e},{:.9e},{:.9e},{:.9e},{:.9e}",
                start.elapsed().as_secs_f64() * 1000.0,
                population.cache.pending(),
                c.queued_patches,
                c.worker_jobs,
                population.work.worker_cpu.as_secs_f64() * 1000.0,
                population.work.worker_samples_completed,
                population.cover.selection_preparation.as_secs_f64() * 1000.0,
                population.work.publication.as_secs_f64() * 1000.0,
                population.cover.stitch_preparation.as_secs_f64() * 1000.0,
                population.cover.morph_preparation.as_secs_f64() * 1000.0,
                population.cover.worker_stitch_cpu.as_secs_f64() * 1000.0,
                population.cover.worker_morph_cpu.as_secs_f64() * 1000.0,
                population.cover.construction_pending(),
                c.cancellations,
                c.resident_bytes + c.external_bytes,
                population.cover.convergence.desired_local_lod,
                population.cover.transition_deferred,
                usize::from(population.cover.transition().is_some()),
                population.cover.convergence.diagnostic_cpu.as_secs_f64() * 1000.0,
                c.resident_bytes,
                c.worker_reserved_bytes,
                c.worker_fixed_bytes,
                c.completed_unpublished_bytes,
                c.external_bytes,
                c.peak_aggregate_bytes,
                population.cover.peak_transition_bytes,
                population.cover.result_publication.as_secs_f64() * 1000.0,
                population.work.scheduling.as_secs_f64() * 1000.0,
                population.cover.surface().map_or(0, |s| s.reused_patches()),
                population.cover.convergence.local_error.sphere_m,
                population
                    .cover
                    .convergence
                    .local_error
                    .filtered_interpolation_m,
                population.cover.convergence.local_error.unresolved_m,
                population
                    .cover
                    .convergence
                    .local_error
                    .boundary_constraint_m,
                population.cover.convergence.local_error.morph_remaining_m,
                population.cover.convergence.local_error.numeric_m
            )?;
            deadline = (deadline + Duration::from_millis(16)).max(Instant::now());
            if start.elapsed().as_millis() < u128::from(TIMES_MS[next]) {
                continue;
            }
            let active_levels = population
                .cover
                .active()
                .iter()
                .map(|p| p.address.level().to_string())
                .collect::<Vec<_>>()
                .join("|");
            let visible_levels = population
                .cover
                .visible()
                .iter()
                .map(|p| p.address.level().to_string())
                .collect::<Vec<_>>()
                .join("|");
            let report = population.cover.report;
            writeln!(
                csv,
                "{view_name},{},{},{update_ms:.6},{},{},{},{},{active_levels},{visible_levels},{},{},{},{},{},{:.6},{:.6},{:.6},{:.6},{},{},{},{:?},{ready_lod:?},{rendered_lod:?},{render_ms:.6},{},{body_index},\"{body_id:?}\",{frame_clearance:.3}",
                TIMES_MS[next],
                start.elapsed().as_millis(),
                population.cover.ready(),
                owners[body_index],
                population.cover.active().len(),
                population.cover.visible().len(),
                report.desired_patches,
                report.balanced_patches,
                population.work.pending_patches,
                population.work.vertices_generated,
                population.work.patches_completed,
                population.cover.selection_preparation.as_secs_f64() * 1000.0,
                population.work.elapsed.as_secs_f64() * 1000.0,
                population.cover.stitch_preparation.as_secs_f64() * 1000.0,
                population.cover.morph_preparation.as_secs_f64() * 1000.0,
                report.quality_pending,
                report.settled,
                report.budget_constrained,
                population.cover.convergence.desired_local_lod,
                population.cover.transition_deferred,
            )?;
            #[cfg(feature = "terrain-capture")]
            if let Some(gpu) = &mut gpu {
                let rgba = gpu.render(&frame)?;
                writeln!(
                    gpu_profiles,
                    "{view_name},target_ms={},actual_ms={:.3},capability={:?},host_encode_ms={:.6},gpu={:?},terrain_upload={:?}",
                    TIMES_MS[next],
                    start.elapsed().as_secs_f64() * 1000.0,
                    gpu.timestamp_availability(),
                    gpu.last_cpu_encode().as_secs_f64() * 1000.0,
                    gpu.last_gpu_profile(),
                    gpu.last_terrain_upload_profile()
                )?;
                bitmap(
                    &output.join(format!("{view_name}-{}ms.bmp", TIMES_MS[next])),
                    &rgba,
                )?;
                let mut diagnostic = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                diagnostic.set_terrain_lighting(lighting.with_mode(TerrainRenderMode::Elevation));
                let style = SurfaceStyle {
                    lod_colors: true,
                    borders: true,
                    ..Default::default()
                };
                if owners[body_index] {
                    diagnostic.append_stitched_surface(
                        requests[body_index],
                        population.cover.visible(),
                        population
                            .cover
                            .surface()
                            .context("missing diagnostic surface")?,
                        population
                            .cover
                            .topology()
                            .context("missing diagnostic topology")?,
                        style,
                    )?;
                    if let Some((mesh, fraction)) = population.cover.transition() {
                        diagnostic.append_surface_transition(
                            requests[body_index],
                            mesh,
                            fraction,
                            style,
                        )?;
                    }
                }
                diagnostic.append_bodies(&far)?;
                bitmap(
                    &output.join(format!("{view_name}-{}ms-lod.bmp", TIMES_MS[next])),
                    &gpu.render(&diagnostic)?,
                )?;
            }
            next += 1;
        }
        writeln!(
            csv,
            "# {view_name}: radius_m={radius} complete_height_m={complete_height} clearance_m={clearance}; production-route wall-clock probe, not native interactive evidence"
        )?;
    }
    csv.flush()?;
    frames_csv.flush()?;
    detail.flush()?;
    memory_csv.flush()?;
    #[cfg(feature = "surface-profile")]
    render_profiles.flush()?;
    #[cfg(feature = "surface-profile")]
    stages.flush()?;
    #[cfg(feature = "surface-profile")]
    transition_profiles.flush()?;
    #[cfg(feature = "terrain-capture")]
    gpu_profiles.flush()?;
    println!(
        "wrote {} ({} views × 5 seconds; native interactive acceptance is separate)",
        output.join("timeline.csv").display(),
        if matches!(mode.as_str(), "cold-2m" | "motion" | "switch") {
            1
        } else {
            CLEARANCES_M.len()
        }
    );
    Ok(())
}

#[cfg(feature = "terrain-capture")]
fn bitmap(path: &std::path::Path, rgba: &[u8]) -> Result<()> {
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
    ensure!(
        rgba.len() == (WIDTH * HEIGHT * 4) as usize,
        "capture size mismatch"
    );
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let s = (y * WIDTH as usize + x) * 4;
            let t = 54 + y * row as usize + x * 3;
            bytes[t..t + 3].copy_from_slice(&[rgba[s + 2], rgba[s + 1], rgba[s]]);
        }
    }
    fs::write(path, bytes)?;
    Ok(())
}
