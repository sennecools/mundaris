//! Deterministic production-GPU A/B captures, not an adaptive terrain selector.
use anyhow::{Result, ensure};
use glam::{DMat3, DQuat, DVec3};
use mundaris_app::planet_terrain::*;
use mundaris_math::{surface::*, *};
use mundaris_renderer::{planet_surface::*, terrain_capture::TerrainCaptureRenderer, *};
use mundaris_world::{terrain::*, *};
use std::{collections::BTreeSet, fmt::Write, fs, num::NonZeroU64, path::Path};

const RADIUS: f64 = 6_371_000.0;
const WIDTH: u32 = 768;
const HEIGHT: u32 = 512;

fn location(n: DVec3) -> Result<SurfaceLocation> {
    Ok(SurfaceLocation::new(Direction3::try_new(n)?))
}
fn sample(field: &TerrainGenerator, n: DVec3, footprint: f64) -> Result<TerrainSample> {
    Ok(field.evaluate_point(TerrainQuery {
        location: location(n)?,
        footprint: TerrainFootprint::new(footprint)?,
    })?)
}
fn tangents(n: DVec3) -> (DVec3, DVec3) {
    let x = n
        .cross(if n.x.abs() < 0.8 { DVec3::X } else { DVec3::Y })
        .normalize();
    (x, n.cross(x))
}
fn bitmap(path: &Path, rgba: &[u8]) -> Result<()> {
    let row = (WIDTH * 3).div_ceil(4) * 4;
    let mut output = vec![0u8; (54 + row * HEIGHT) as usize];
    let len = output.len() as u32;
    output[..2].copy_from_slice(b"BM");
    output[2..6].copy_from_slice(&len.to_le_bytes());
    output[10..14].copy_from_slice(&54u32.to_le_bytes());
    output[14..18].copy_from_slice(&40u32.to_le_bytes());
    output[18..22].copy_from_slice(&(WIDTH as i32).to_le_bytes());
    output[22..26].copy_from_slice(&(-(HEIGHT as i32)).to_le_bytes());
    output[26..28].copy_from_slice(&1u16.to_le_bytes());
    output[28..30].copy_from_slice(&24u16.to_le_bytes());
    for y in 0..HEIGHT as usize {
        for x in 0..WIDTH as usize {
            let src = (y * WIDTH as usize + x) * 4;
            let dst = 54 + y * row as usize + x * 3;
            output[dst..dst + 3].copy_from_slice(&[rgba[src + 2], rgba[src + 1], rgba[src]]);
        }
    }
    fs::write(path, output)?;
    Ok(())
}

struct Scene {
    name: &'static str,
    center: DVec3,
    width_m: f64,
    level: u8,
    oblique: bool,
    sun: DVec3,
    full_cover: bool,
}

fn main() -> Result<()> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/phase56-captures".into());
    let filter = std::env::args().nth(2);
    fs::create_dir_all(&directory)?;
    let mut gpu = TerrainCaptureRenderer::new(WIDTH, HEIGHT)?;
    let definition = checkpoint_terrain_definition(RADIUS)?;
    let field = TerrainGenerator::new(&definition, RADIUS)?;
    let center = DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize();
    let (x, y) = tangents(center);
    let mut peak = center;
    let mut valley = center;
    let (mut highest, mut lowest) = (f64::NEG_INFINITY, f64::INFINITY);
    // Broad extrema only; never a claim of globally optimal terrain morphology.
    for i in 0..32 {
        for j in 0..32 {
            let n = (center
                + x * (i as f64 / 31.0 - 0.5) * 256_000.0 / RADIUS
                + y * (j as f64 / 31.0 - 0.5) * 256_000.0 / RADIUS)
                .normalize();
            let h = sample(&field, n, 50_000.0)?.height_m();
            if h > highest {
                highest = h;
                peak = n;
            }
            if h < lowest {
                lowest = h;
                valley = n;
            }
        }
    }
    let low_sun = (center * 0.06 + x).normalize();
    let scenes = [
        Scene {
            name: "range",
            center,
            width_m: 128_000.0,
            level: 10,
            oblique: false,
            sun: low_sun,
            full_cover: false,
        },
        Scene {
            name: "oblique_slope",
            center,
            width_m: 32_000.0,
            level: 12,
            oblique: true,
            sun: low_sun,
            full_cover: false,
        },
        Scene {
            name: "valley",
            center: valley,
            width_m: 32_000.0,
            level: 12,
            oblique: true,
            sun: low_sun,
            full_cover: false,
        },
        Scene {
            name: "ridge",
            center: peak,
            width_m: 32_000.0,
            level: 12,
            oblique: true,
            sun: low_sun,
            full_cover: false,
        },
        Scene {
            name: "gully",
            center,
            width_m: 8_000.0,
            level: 14,
            oblique: true,
            sun: low_sun,
            full_cover: false,
        },
        Scene {
            name: "near_terminator",
            center,
            width_m: 0.0,
            level: 4,
            oblique: false,
            sun: x,
            full_cover: true,
        },
        Scene {
            name: "overhead",
            center,
            width_m: 32_000.0,
            level: 12,
            oblique: true,
            sun: center,
            full_cover: false,
        },
        Scene {
            name: "grazing",
            center,
            width_m: 32_000.0,
            level: 12,
            oblique: true,
            sun: low_sun,
            full_cover: false,
        },
        Scene {
            name: "uniform4",
            center: DVec3::Z,
            width_m: 0.0,
            level: 4,
            oblique: false,
            sun: TerrainLighting::default().sun_direction_body(),
            full_cover: true,
        },
        Scene {
            name: "night",
            center: -TerrainLighting::default().sun_direction_body(),
            width_m: 0.0,
            level: 4,
            oblique: false,
            sun: TerrainLighting::default().sun_direction_body(),
            full_cover: true,
        },
        Scene {
            name: "face_boundary",
            center: DVec3::new(1.0, 1.0, 0.3).normalize(),
            width_m: 128_000.0,
            level: 10,
            oblique: false,
            sun: low_sun,
            full_cover: false,
        },
        Scene {
            name: "corner",
            center: DVec3::ONE.normalize(),
            width_m: 128_000.0,
            level: 10,
            oblique: false,
            sun: low_sun,
            full_cover: false,
        },
        Scene {
            name: "pole",
            center: DVec3::Y,
            width_m: 128_000.0,
            level: 10,
            oblique: false,
            sun: (DVec3::Y * 0.06 + DVec3::X).normalize(),
            full_cover: false,
        },
    ];
    let mut world = CelestialSystem::new(
        NonZeroU64::new(56).ok_or_else(|| anyhow::anyhow!("namespace"))?,
        SimulationInstant::ZERO,
    );
    let body = world.insert_body(
        "lighting capture",
        BodyProperties::new(1e20, RADIUS)?,
        BodyState::new(
            LocalPosition::origin(),
            LinearVelocity3::zero(),
            UnitRotation::identity(),
            AngularVelocity3::zero(),
        ),
    )?;
    world.edit_terrain(body, Some(definition.clone()))?;
    let identity = TerrainGeometryIdentity::new(
        body,
        definition,
        world.body(body)?.terrain_revision(),
        RADIUS,
    )?;
    let frames = CelestialFrameProjection::build(
        &world,
        NonZeroU64::new(56).ok_or_else(|| anyhow::anyhow!("namespace"))?,
    )?;
    let fixed = frames.frames_for(body)?.body_fixed;
    let topology = SurfaceTopology::new();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(WIDTH, HEIGHT, 60.0_f64.to_radians(), 0.1)?;
    let defaults = TerrainLighting::default();
    let mut manifest = format!(
        "adapter={}\nseed=17 version=V2 radius_m={RADIUS} viewport={WIDTH}x{HEIGHT} fov_deg=60\nambient={} diffuse={}; body-fixed surface->sun; no normal exaggeration\nlocal windows are same-level diagnostic crops, NOT integrated adaptive covers\nGPU timestamps unavailable; readback wall time is not GPU timing\n",
        gpu.adapter_name(),
        defaults.ambient_strength(),
        defaults.diffuse_strength()
    );
    for scene in scenes {
        if filter.as_ref().is_some_and(|f| f != scene.name) {
            continue;
        }
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, MAX_TERRAIN_PATCHES)?;
        let footprint = RADIUS * 2.0 / ((1u64 << scene.level) * 16) as f64;
        let target = scene.center * (RADIUS + sample(&field, scene.center, footprint)?.height_m());
        let (east, north) = tangents(scene.center);
        let eye = if scene.full_cover {
            scene.center * (RADIUS + 10_000_000.0)
        } else {
            target
                + scene.center * (scene.width_m * 0.65)
                + if scene.oblique {
                    north * (scene.width_m * 0.55)
                } else {
                    DVec3::ZERO
                }
        };
        let back = (eye - target).normalize();
        let right = north.cross(back).normalize();
        let up = back.cross(right);
        let orientation = UnitRotation::try_from_quaternion(DQuat::from_mat3(&DMat3::from_cols(
            right, up, back,
        )))?;
        let observer = FramePose::new(
            FramePosition::new(fixed, LocalPosition::try_metres(eye)?),
            orientation,
        );
        let pair = frames.coherent_view(&world)?;
        let view = PreparedView::new(
            &pair.evaluation(),
            observer,
            RenderPrecisionBudget::near_debug(),
        )?;
        let patches = if scene.full_cover {
            let mut cover = TerrainReadyCover::default();
            loop {
                cover.update(&mut cache, &identity, 4, 16_384, None)?;
                if cover
                    .active()
                    .first()
                    .is_some_and(|p| p.address.level() == 4)
                {
                    break;
                }
            }
            cover.prepare_visible(
                &cache,
                &identity,
                &SurfaceViewInput {
                    view: &view,
                    body_fixed_frame: fixed,
                    reference_radius_m: RADIUS,
                    projection,
                },
            )?;
            cover.visible().to_vec()
        } else {
            let mut addresses = BTreeSet::new();
            let side = 1u32 << scene.level;
            let crop_scale = if scene.oblique { 2.8 } else { 2.2 };
            // Oversized region keeps diagnostic window boundaries outside the view;
            // samples map across charts so edge/corner crops use canonical geometry.
            for i in 0..96 {
                for j in 0..96 {
                    let n = scene.center
                        + east * (i as f64 / 95.0 - 0.5) * scene.width_m * crop_scale / RADIUS
                        + north * (j as f64 / 95.0 - 0.5) * scene.width_m * crop_scale / RADIUS;
                    let (face, uv) = location(n)?.face_uv();
                    let [a, b] =
                        uv.map(|v| (((v + 1.0) * 0.5 * side as f64).floor() as u32).min(side - 1));
                    addresses.insert(CubePatchAddress::try_new(face, scene.level, a, b)?);
                }
            }
            ensure!(
                addresses.len() <= MAX_TERRAIN_PATCHES,
                "crop exceeds cache quota"
            );
            let mut patches = Vec::new();
            for address in addresses {
                cache.request(&identity, address);
                while cache.peek(&identity, address).is_none() {
                    cache.generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)?;
                }
                patches.push(ActiveSurfacePatch {
                    address,
                    metadata: PatchMetadata::build(address, &topology)?,
                    stitch_mask: 0,
                    error_pixels: 0.0,
                });
            }
            patches
        };
        let before = cache.report();
        let geometry: Vec<_> = patches
            .iter()
            .map(|p| {
                cache
                    .peek(&identity, p.address)
                    .ok_or_else(|| anyhow::anyhow!("missing ready geometry"))
            })
            .collect::<Result<_>>()?;
        let mut staging = CelestialStaging::default();
        writeln!(
            manifest,
            "\nscene={} level={} footprint_m={footprint} eye={eye:?} orientation={:?} target={target:?} sun={:?} patches={} generation={before:?}",
            scene.name,
            scene.level,
            orientation.quaternion(),
            scene.sun,
            patches.len()
        )?;
        for mode in TerrainRenderMode::ALL {
            let repetitions = if scene.name == "uniform4"
                && matches!(mode, TerrainRenderMode::Elevation | TerrainRenderMode::Lit)
            {
                24
            } else {
                1
            };
            let mut prep_ms = Vec::new();
            let mut encode_ms = Vec::new();
            let mut total_ms = Vec::new();
            for repetition in 0..repetitions {
                let start = std::time::Instant::now();
                let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                frame.set_terrain_lighting(TerrainLighting::try_new(
                    scene.sun,
                    defaults.ambient_strength(),
                    defaults.diffuse_strength(),
                    mode,
                )?);
                frame.append_generated_surface(
                    CelestialRenderBody {
                        body_fixed_frame: fixed,
                        reference_radius_m: RADIUS,
                        color: [0.5, 0.6, 0.3, 1.0],
                        unlit: false,
                        selected: false,
                    },
                    &patches,
                    &geometry,
                    &topology,
                    SurfaceStyle {
                        elevation_colors: true,
                        ..Default::default()
                    },
                )?;
                let preparation = start.elapsed();
                let rgba = gpu.render(&frame)?;
                if repetitions > 1 && repetition >= 4 {
                    prep_ms.push(preparation.as_secs_f64() * 1000.0);
                    encode_ms.push(gpu.last_cpu_encode().as_secs_f64() * 1000.0);
                    total_ms.push((preparation + gpu.last_cpu_encode()).as_secs_f64() * 1000.0);
                }
                if repetition + 1 == repetitions {
                    let filename =
                        format!("{}-{}.bmp", scene.name, format!("{mode:?}").to_lowercase());
                    bitmap(&Path::new(&directory).join(&filename), &rgba)?;
                    writeln!(
                        manifest,
                        "file={filename} report={:?}",
                        frame.report().surface
                    )?;
                    eprintln!(
                        "captured {filename}: {} patches, {} draws",
                        patches.len(),
                        frame.report().surface.draws
                    );
                }
            }
            if !total_ms.is_empty() {
                prep_ms.sort_by(f64::total_cmp);
                encode_ms.sort_by(f64::total_cmp);
                total_ms.sort_by(f64::total_cmp);
                writeln!(
                    manifest,
                    "CPU capture terrain-only matched mode={mode:?} samples=20 median_prep_ms={} median_upload_encode_ms={} median_host_total_ms={}; excludes submission/GPU wait/readback/UI/physics/readiness",
                    prep_ms[10], encode_ms[10], total_ms[10]
                )?;
                eprintln!(
                    "matched {mode:?}: preparation {:.3} ms, upload/encode {:.3} ms, host total {:.3} ms; NOT GPU time",
                    prep_ms[10], encode_ms[10], total_ms[10]
                );
            }
        }
        drop(geometry);
        let work = cache.generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)?;
        ensure!(
            work.vertices_generated == 0 && work.patches_completed == 0,
            "lighting regenerated geometry"
        );
        ensure!(
            cache.report().resident_patches == before.resident_patches,
            "lighting changed cache"
        );
        fs::write(Path::new(&directory).join("manifest.txt"), &manifest)?;
    }
    Ok(())
}
