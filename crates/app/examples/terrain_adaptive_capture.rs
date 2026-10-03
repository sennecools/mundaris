//! Static adaptive checkpoint: production GPU captures and separated host costs.
use anyhow::{Result, ensure};
use glam::{DMat3, DQuat, DVec3};
use mundaris_app::{gravity_fixtures::GravityFixture, planet_terrain::*};
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, terrain_capture::TerrainCaptureRenderer, *};
use mundaris_world::*;
use std::{
    collections::BTreeMap,
    fmt::Write,
    fs,
    num::NonZeroU64,
    path::Path,
    time::{Duration, Instant},
};

const WIDTH: u32 = 768;
const HEIGHT: u32 = 512;
fn bitmap(path: &Path, rgba: &[u8]) -> Result<()> {
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
            let a = (y * WIDTH as usize + x) * 4;
            let b = 54 + y * row as usize + x * 3;
            bytes[b..b + 3].copy_from_slice(&[rgba[a + 2], rgba[a + 1], rgba[a]]);
        }
    }
    fs::write(path, bytes)?;
    Ok(())
}
fn main() -> Result<()> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/phase57-static".into());
    let scene = std::env::args().nth(2).unwrap_or_else(|| "planet".into());
    fs::create_dir_all(&output)?;
    let mut world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(57).ok_or_else(|| anyhow::anyhow!("namespace"))?)?;
    let body = world
        .bodies()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("body"))?
        .0;
    let radius = world.body(body)?.properties().reference_radius_m();
    let definition = checkpoint_terrain_definition(radius)?;
    world.edit_terrain(body, Some(definition.clone()))?;
    let identity = TerrainGeometryIdentity::new(
        body,
        definition,
        world.body(body)?.terrain_revision(),
        radius,
    )?;
    let frames = CelestialFrameProjection::build(
        &world,
        NonZeroU64::new(57).ok_or_else(|| anyhow::anyhow!("namespace"))?,
    )?;
    let fixed = frames.frames_for(body)?.body_fixed;
    let (direction, clearance) = match scene.as_str() {
        "mountain" => (
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize(),
            20_000.0,
        ),
        "face" => (DVec3::new(1.0, 1.0, 0.3).normalize(), 100_000.0),
        "corner" => (DVec3::ONE.normalize(), 100_000.0),
        _ => (DVec3::Z, 10_000_000.0),
    };
    let back = direction;
    let right = if back.y.abs() < 0.9 {
        DVec3::Y.cross(back)
    } else {
        DVec3::X.cross(back)
    }
    .normalize();
    let up = back.cross(right);
    let pose = FramePose::new(
        FramePosition::new(
            fixed,
            LocalPosition::try_metres(direction * (radius + clearance))?,
        ),
        UnitRotation::try_from_quaternion(DQuat::from_mat3(&DMat3::from_cols(right, up, back)))?,
    );
    let pair = frames.coherent_view(&world)?;
    let view = PreparedView::new(
        &pair.evaluation(),
        pose,
        RenderPrecisionBudget::near_debug(),
    )?;
    let projection = CelestialProjection::try_new(WIDTH, HEIGHT, 60.0_f64.to_radians(), 0.1)?;
    let input = SurfaceViewInput {
        view: &view,
        body_fixed_frame: fixed,
        reference_radius_m: radius,
        projection,
    };
    let settings = LodSettings::default()
        .with_limits(2048, 2048, 30)?
        .with_work_limit(32)?;
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, MAX_TERRAIN_PATCHES)?;
    let mut cover = AdaptiveTerrainCover::default();
    let motion = std::env::args().nth(3).as_deref() == Some("motion");
    if motion {
        cover.set_morph_duration(Duration::from_millis(150))?;
    }
    let mut manifest = format!(
        "Adaptive checkpoint; morph_ms={}; seed17/V2 scene={scene} clearance={clearance} viewport={WIDTH}x{HEIGHT} direction={direction:?}\n",
        if motion { 150 } else { 0 }
    );
    let mut frame_times = Vec::new();
    let mut generation_times = Vec::new();
    let mut selection_times = Vec::new();
    let mut stitch_times = Vec::new();
    let mut morph_times = Vec::new();
    let mut active_morph_times = Vec::new();
    let mut total_generated = 0usize;
    let mut max_pending = 0;
    let mut max_vertices = 0;
    let mut idle = 0;
    for frame in 0..4000 {
        let start = Instant::now();
        let work = cover.update_with_elapsed(
            &mut cache,
            &identity,
            &input,
            &settings,
            1156,
            None,
            Duration::from_millis(16),
        )?;
        let elapsed = start.elapsed();
        max_pending = max_pending.max(work.pending_patches);
        max_vertices = max_vertices.max(work.vertices_generated);
        total_generated += work.vertices_generated;
        if cover.transition().is_some() && cover.morph_preparation.is_zero() {
            active_morph_times.push(elapsed.as_secs_f64() * 1000.0);
        }
        if !cover.morph_preparation.is_zero() {
            morph_times.push(cover.morph_preparation.as_secs_f64() * 1000.0);
        }
        if work.vertices_generated > 0 {
            selection_times.push(cover.selection_preparation.as_secs_f64() * 1000.0);
            generation_times.push(work.elapsed.as_secs_f64() * 1000.0);
            frame_times.push(elapsed.as_secs_f64() * 1000.0);
        }
        if !cover.stitch_preparation.is_zero() {
            stitch_times.push(cover.stitch_preparation.as_secs_f64() * 1000.0);
        }
        if frame % 100 == 0 {
            eprintln!(
                "{scene} frame={frame} cover={} visible={} maxlevel={} pending={} budget={} elapsed={elapsed:?}",
                cover.active().len(),
                cover.visible().len(),
                cover.report.max_level,
                work.pending_patches,
                cover.report.budget_constrained
            );
        }
        if work.vertices_generated == 0
            && cover.stitch_preparation.is_zero()
            && cover.transition().is_none()
        {
            idle += 1;
        } else {
            idle = 0;
        }
        if idle == 8 {
            break;
        }
    }
    let mut levels = BTreeMap::new();
    for p in cover.active() {
        *levels.entry(p.address.level()).or_insert(0usize) += 1;
    }
    ensure!(
        levels.len() > 1,
        "static adaptive view did not produce mixed LOD"
    );
    let mut max_delta = 0;
    for p in cover.active() {
        for edge in mundaris_math::surface::PatchEdge::ALL {
            let mut n = Some(p.address.neighbor(edge).address);
            while let Some(a) = n {
                if cover
                    .active()
                    .binary_search_by_key(&a, |p| p.address)
                    .is_ok()
                {
                    max_delta = max_delta.max(p.address.level().abs_diff(a.level()));
                    break;
                }
                n = a.parent();
            }
        }
    }
    ensure!(max_delta <= 1, "unsupported ready adjacency");
    writeln!(
        manifest,
        "cover={} visible={} levels={levels:?} max_delta={max_delta} quality={:?} cache={:?} derived_bytes={} aggregate_peak={} pending_max={max_pending} vertices_per_headless_frame_max={max_vertices}",
        cover.active().len(),
        cover.visible().len(),
        cover.report,
        cache.report(),
        cover.resident_bytes(),
        cover.peak_cpu_bytes
    )?;
    for (name, times) in [
        ("refinement_host_ms", &mut frame_times),
        ("generation_ms", &mut generation_times),
        ("refinement_selection_ms", &mut selection_times),
        ("stitch_build_ms", &mut stitch_times),
        ("morph_build_ms", &mut morph_times),
        ("active_morph_host_ms", &mut active_morph_times),
    ] {
        times.sort_by(f64::total_cmp);
        if !times.is_empty() {
            writeln!(
                manifest,
                "{name}: count={} median={} worst={}",
                times.len(),
                times[times.len() / 2],
                times[times.len() - 1]
            )?;
        }
    }
    writeln!(
        manifest,
        "warm_generated_samples={total_generated} peak_transition_bytes={} generation_us_per_sample={}",
        cover.peak_transition_bytes,
        generation_times.iter().sum::<f64>() * 1000.0 / (total_generated.max(1) as f64)
    )?;
    let mut steady = Vec::new();
    let mut steady_selection = Vec::new();
    for _ in 0..24 {
        let start = Instant::now();
        let work = cover.update(
            &mut cache,
            &identity,
            &input,
            &settings,
            64,
            Some(Duration::from_millis(2)),
        )?;
        ensure!(work.vertices_generated == 0, "view not steady");
        steady.push(start.elapsed().as_secs_f64() * 1000.0);
        steady_selection.push(cover.selection_preparation.as_secs_f64() * 1000.0);
    }
    steady.sort_by(f64::total_cmp);
    steady_selection.sort_by(f64::total_cmp);
    writeln!(
        manifest,
        "steady_selection_readiness_ms median={} worst={}",
        steady[12], steady[23]
    )?;
    writeln!(
        manifest,
        "steady_selector_ms median={} worst={}",
        steady_selection[12], steady_selection[23]
    )?;
    // Inspect each certificate term at representative visible levels.
    for level in levels.keys() {
        if let Some(p) = cover.visible().iter().find(|p| p.address.level() == *level) {
            let surface = cover.surface().ok_or_else(|| anyhow::anyhow!("surface"))?;
            let g = &surface.patches()[surface
                .patches()
                .binary_search_by_key(&p.address, GeneratedSurfacePatch::address)
                .map_err(|_| anyhow::anyhow!("missing constrained patch"))?];
            writeln!(
                manifest,
                "level={level} address={:?} error={:?} projected_px={}",
                p.address,
                g.error(),
                p.error_pixels
            )?;
        }
    }
    let mut gpu = TerrainCaptureRenderer::new(WIDTH, HEIGHT)?;
    writeln!(
        manifest,
        "adapter={} GPU timestamps unavailable",
        gpu.adapter_name()
    )?;
    let sphere = Icosphere::new();
    let mut staging = CelestialStaging::default();
    let lighting = TerrainLighting::try_new(
        (direction * 0.06 + right).normalize(),
        0.06,
        0.94,
        TerrainRenderMode::Lit,
    )?;
    for (name, mode, lod_colors) in [
        ("lit", TerrainRenderMode::Lit, false),
        ("normals", TerrainRenderMode::Normals, false),
        ("diffuse", TerrainRenderMode::Diffuse, false),
        ("lod", TerrainRenderMode::Elevation, true),
    ] {
        let start = Instant::now();
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_terrain_lighting(lighting.with_mode(mode));
        frame.append_stitched_surface(
            CelestialRenderBody {
                body_fixed_frame: fixed,
                reference_radius_m: radius,
                color: [0.5, 0.6, 0.3, 1.0],
                unlit: false,
                selected: false,
            },
            cover.visible(),
            cover.surface().ok_or_else(|| anyhow::anyhow!("surface"))?,
            cover
                .topology()
                .ok_or_else(|| anyhow::anyhow!("topology"))?,
            SurfaceStyle {
                elevation_colors: !lod_colors,
                lod_colors,
                ..Default::default()
            },
        )?;
        let prep = start.elapsed();
        let rgba = gpu.render(&frame)?;
        bitmap(
            &Path::new(&output).join(format!("{scene}-{name}.bmp")),
            &rgba,
        )?;
        writeln!(
            manifest,
            "{name}: preparation_ms={} upload_encode_ms={} render={:?}",
            prep.as_secs_f64() * 1000.0,
            gpu.last_cpu_encode().as_secs_f64() * 1000.0,
            frame.report().surface
        )?;
    }
    if motion {
        cover.set_morph_duration(Duration::from_millis(150))?;
        let mut selection = Vec::new();
        let mut generation = Vec::new();
        let mut stitching = Vec::new();
        let mut morph_build = Vec::new();
        let mut total = Vec::new();
        let mut active_frames = 0;
        let mut max_pins = 0;
        let mut generated = 0;
        for step in 0..96 {
            let moving_direction =
                (direction + right * (0.01 * (step as f64 * 0.2).sin())).normalize();
            let moving_clearance = if (step / 24) % 2 == 0 {
                clearance * 4.0
            } else {
                clearance
            };
            let back = moving_direction;
            let right = if back.y.abs() < 0.9 {
                DVec3::Y.cross(back)
            } else {
                DVec3::X.cross(back)
            }
            .normalize();
            let up = back.cross(right);
            let pose = FramePose::new(
                FramePosition::new(
                    fixed,
                    LocalPosition::try_metres(moving_direction * (radius + moving_clearance))?,
                ),
                UnitRotation::try_from_quaternion(DQuat::from_mat3(&DMat3::from_cols(
                    right, up, back,
                )))?,
            );
            let view = PreparedView::new(
                &pair.evaluation(),
                pose,
                RenderPrecisionBudget::near_debug(),
            )?;
            let input = SurfaceViewInput {
                view: &view,
                body_fixed_frame: fixed,
                reference_radius_m: radius,
                projection,
            };
            let started = Instant::now();
            let work = cover.update_with_elapsed(
                &mut cache,
                &identity,
                &input,
                &settings,
                64,
                None,
                Duration::from_millis(16),
            )?;
            total.push(started.elapsed().as_secs_f64() * 1000.0);
            selection.push(cover.selection_preparation.as_secs_f64() * 1000.0);
            generation.push(work.elapsed.as_secs_f64() * 1000.0);
            generated += work.vertices_generated;
            if !cover.stitch_preparation.is_zero() {
                stitching.push(cover.stitch_preparation.as_secs_f64() * 1000.0);
            }
            if !cover.morph_preparation.is_zero() {
                morph_build.push(cover.morph_preparation.as_secs_f64() * 1000.0);
            }
            active_frames += usize::from(cover.transition().is_some());
            max_pending = max_pending.max(cache.pending());
            max_pins = max_pins.max(cache.report().pinned_patches);
            ensure!(
                cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES,
                "motion exceeded CPU budget"
            );
        }
        writeln!(
            manifest,
            "heavy_motion_updates=96 admitted_step_ms=16 generation_vertex_limit=64 generated_samples={generated} active_morph_updates={active_frames} max_pending={max_pending} max_pins={max_pins} peak_transition_bytes={} peak_aggregate_bytes={}",
            cover.peak_transition_bytes,
            cache.report().peak_aggregate_bytes
        )?;
        for (name, times) in [
            ("heavy_motion_total_ms", &mut total),
            ("heavy_motion_selector_ms", &mut selection),
            ("heavy_motion_generation_ms", &mut generation),
            ("heavy_motion_stitch_ms", &mut stitching),
            ("heavy_motion_morph_build_ms", &mut morph_build),
        ] {
            times.sort_by(f64::total_cmp);
            if !times.is_empty() {
                writeln!(
                    manifest,
                    "{name}: count={} median={} worst={}",
                    times.len(),
                    times[times.len() / 2],
                    times[times.len() - 1]
                )?;
            }
        }
    }
    fs::write(
        Path::new(&output).join(format!("{scene}-manifest.txt")),
        &manifest,
    )?;
    println!("{manifest}");
    Ok(())
}
