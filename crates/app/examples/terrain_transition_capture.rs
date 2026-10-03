//! Deterministic production-GPU captures of balanced terrain morphs.
use anyhow::{Result, ensure};
use glam::{DMat3, DQuat, DVec3};
use mundaris_app::planet_terrain::*;
use mundaris_math::{surface::*, *};
use mundaris_renderer::{planet_surface::*, terrain_capture::TerrainCaptureRenderer, *};
use mundaris_world::{terrain::*, *};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    fs,
    num::NonZeroU64,
    path::Path,
    time::Instant,
};

const WIDTH: u32 = 768;
const HEIGHT: u32 = 512;
const TRANSITION_RESERVATION: usize = 16 * 1024 * 1024;

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

fn split(cover: &mut BTreeSet<CubePatchAddress>, address: CubePatchAddress) -> Result<()> {
    ensure!(
        cover.remove(&address),
        "attempt to split a non-leaf patch {address:?}"
    );
    cover.extend(address.children()?);
    Ok(())
}

fn balance(cover: &mut BTreeSet<CubePatchAddress>) -> Result<()> {
    loop {
        let snapshot: Vec<_> = cover.iter().copied().collect();
        let mut coarse = BTreeSet::new();
        for address in &snapshot {
            for edge in PatchEdge::ALL {
                let mut neighbor = Some(address.neighbor(edge).address);
                while let Some(candidate) = neighbor {
                    if cover.contains(&candidate) {
                        if address.level() > candidate.level() + 1 {
                            coarse.insert(candidate);
                        }
                        break;
                    }
                    neighbor = candidate.parent();
                }
            }
        }
        if coarse.is_empty() {
            break;
        }
        for address in coarse {
            split(cover, address)?;
        }
    }
    Ok(())
}

fn balanced_cover(target: CubePatchAddress, split_target: bool) -> Result<Vec<CubePatchAddress>> {
    let mut cover: BTreeSet<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    while target.parent().is_some() {
        let leaf = cover
            .iter()
            .find(|p| p.contains(target))
            .copied()
            .ok_or_else(|| anyhow::anyhow!("no cover leaf contains target"))?;
        if leaf == target || leaf.level() >= target.level() {
            break;
        }
        split(&mut cover, leaf)?;
    }
    balance(&mut cover)?;
    if split_target {
        ensure!(cover.contains(&target), "split destination leaf is absent");
        split(&mut cover, target)?;
        balance(&mut cover)?;
    }
    Ok(cover.into_iter().collect())
}

fn generate_cover(
    cache: &mut TerrainPatchCache,
    identity: &TerrainGeometryIdentity,
    addresses: &[CubePatchAddress],
) -> Result<(Vec<ActiveSurfacePatch>, StitchedSurface, u128, usize)> {
    let topology = SurfaceTopology::new();
    for &address in addresses {
        ensure!(
            cache.request(identity, address),
            "terrain request queue is full"
        );
        while cache.peek(identity, address).is_none() {
            let work = cache.generate(32 * GENERATION_MICROBATCH, GENERATION_MICROBATCH, None)?;
            ensure!(work.vertices_generated > 0, "terrain generation stalled");
        }
    }
    let cover = active_surface_cover(addresses, &topology)?;
    let geometry: Vec<_> = addresses
        .iter()
        .map(|&a| {
            cache
                .peek(identity, a)
                .ok_or_else(|| anyhow::anyhow!("missing generated geometry"))
        })
        .collect::<Result<_>>()?;
    let surface = StitchedSurface::build(&cover, &geometry, &topology)?;
    let triangles = cover
        .iter()
        .map(|p| topology.indices(p.stitch_mask).len() / 3)
        .sum::<usize>();
    let bytes = surface.resident_bytes();
    Ok((cover, surface, triangles as u128, bytes))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let output = args
        .next()
        .unwrap_or_else(|| "target/terrain-transition".into());
    let scene = args.next().unwrap_or_else(|| "mountain".into());
    ensure!(
        matches!(scene.as_str(), "mountain" | "face" | "corner"),
        "scene must be mountain, face, or corner"
    );
    fs::create_dir_all(&output)?;
    let direction = match scene.as_str() {
        "mountain" => {
            DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625).normalize()
        }
        "face" => DVec3::new(1.0, 1.0, 0.3).normalize(),
        _ => DVec3::ONE.normalize(),
    };
    let mut world = CelestialSystem::new(
        NonZeroU64::new(17).ok_or_else(|| anyhow::anyhow!("namespace"))?,
        SimulationInstant::ZERO,
    );
    let body = world.insert_body(
        "terrain transition capture",
        BodyProperties::new(5.972e24, 6_371_000.0)?,
        BodyState::new(
            LocalPosition::origin(),
            LinearVelocity3::zero(),
            UnitRotation::identity(),
            AngularVelocity3::zero(),
        ),
    )?;
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
        NonZeroU64::new(17).ok_or_else(|| anyhow::anyhow!("namespace"))?,
    )?;
    let fixed = frames.frames_for(body)?.body_fixed;
    let face = CubeFace::ALL
        .into_iter()
        .max_by(|a, b| {
            a.basis()[0]
                .dot(direction)
                .total_cmp(&b.basis()[0].dot(direction))
        })
        .ok_or_else(|| anyhow::anyhow!("no cube face"))?;
    let basis = face.basis();
    let uv = [
        direction.dot(basis[1]) / direction.dot(basis[0]),
        direction.dot(basis[2]) / direction.dot(basis[0]),
    ];
    let grid = 1u32 << 13;
    let coordinate =
        |value: f64| (((value + 1.0) * 0.5 * f64::from(grid)).floor() as u32).min(grid - 1);
    let target = CubePatchAddress::try_new(face, 13, coordinate(uv[0]), coordinate(uv[1]))?;
    let old_addresses = balanced_cover(target, false)?;
    let new_addresses = balanced_cover(target, true)?;
    ensure!(
        max_edge_delta(&old_addresses) <= 1,
        "old cover is not edge-balanced"
    );
    ensure!(
        max_edge_delta(&new_addresses) <= 1,
        "new cover is not edge-balanced"
    );
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, MAX_TERRAIN_PATCHES)?;
    let started = Instant::now();
    let (old_cover, old_surface, old_triangles, _) =
        generate_cover(&mut cache, &identity, &old_addresses)?;
    let old_build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let (new_cover, new_surface, new_triangles, _) =
        generate_cover(&mut cache, &identity, &new_addresses)?;
    let new_build_ms = started.elapsed().as_secs_f64() * 1000.0;
    let topology = SurfaceTopology::new();
    let started = Instant::now();
    let transition = SurfaceTransition::build(
        &old_cover,
        &old_surface,
        &new_cover,
        &new_surface,
        &topology,
        TRANSITION_RESERVATION,
    )?;
    let morph_prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
    ensure!(
        transition.triangles().len() <= 65_536,
        "transition exceeds triangle cap"
    );
    let (reverse, reverse_prepare_ms) = {
        let started = Instant::now();
        let mesh = SurfaceTransition::build(
            &new_cover,
            &new_surface,
            &old_cover,
            &old_surface,
            &topology,
            TRANSITION_RESERVATION,
        )?;
        (mesh, started.elapsed().as_secs_f64() * 1000.0)
    };

    let east = if direction.y.abs() < 0.9 {
        DVec3::Y.cross(direction)
    } else {
        DVec3::X.cross(direction)
    }
    .normalize();
    let north = direction.cross(east).normalize();
    let width = radius * 2.0 / 8192.0;
    let footprint = TerrainFootprint::new(width / 16.0)?;
    let generator = TerrainGenerator::new(&identity.definition, radius)?;
    let location = SurfaceLocation::new(Direction3::try_new(direction)?);
    let center_sample = generator.evaluate_point(TerrainQuery {
        location,
        footprint,
    })?;
    let eye = direction * (radius + center_sample.height_m() + width * 0.8) + north * (width * 0.5);
    let back = (eye - direction * (radius + center_sample.height_m())).normalize();
    let up = back.cross(east).normalize();
    let pose = FramePose::new(
        FramePosition::new(fixed, LocalPosition::try_metres(eye)?),
        UnitRotation::try_from_quaternion(DQuat::from_mat3(&DMat3::from_cols(east, up, back)))?,
    );
    let pair = frames.coherent_view(&world)?;
    let view = PreparedView::new(
        &pair.evaluation(),
        pose,
        RenderPrecisionBudget::near_debug(),
    )?;
    let projection = CelestialProjection::try_new(WIDTH, HEIGHT, 60.0_f64.to_radians(), 0.1)?;
    let mut gpu = TerrainCaptureRenderer::new(WIDTH, HEIGHT)?;
    let mut staging = CelestialStaging::default();
    let sphere = Icosphere::new();
    let body_request = CelestialRenderBody {
        body_fixed_frame: fixed,
        reference_radius_m: radius,
        color: [0.5, 0.6, 0.3, 1.0],
        unlit: false,
        selected: false,
    };
    let lighting = TerrainLighting::try_new(
        (direction * 0.06 + east).normalize(),
        0.06,
        0.94,
        TerrainRenderMode::Lit,
    )?;
    let mut manifest = format!(
        "balanced complete cover transition capture seed=17/V2 radius={radius} scene={scene} direction={direction:?} target={target:?} width={width}\nadapter={} GPU timestamps unavailable\n",
        gpu.adapter_name()
    );
    let mut capture = |tag: &str,
                       mode: TerrainRenderMode,
                       transition: Option<(&SurfaceTransition, f64)>,
                       cover: &[ActiveSurfacePatch],
                       surface: &StitchedSurface,
                       manifest: &mut String|
     -> Result<Vec<u8>> {
        let started = Instant::now();
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_terrain_lighting(lighting.with_mode(mode));
        let excluded = transition.map_or(&[][..], |(t, _)| t.affected_old());
        let patches: Vec<_> = cover
            .iter()
            .enumerate()
            .filter(|(_, p)| excluded.binary_search(&p.address).is_err())
            .map(|(i, p)| (i, *p))
            .collect();
        if transition.is_none() {
            frame.append_stitched_surface(
                body_request,
                cover,
                surface,
                &topology,
                SurfaceStyle {
                    elevation_colors: true,
                    ..Default::default()
                },
            )?;
        } else if !patches.is_empty() {
            let active: Vec<_> = patches.iter().map(|(_, p)| *p).collect();
            frame.append_stitched_surface(
                body_request,
                &active,
                surface,
                &topology,
                SurfaceStyle {
                    elevation_colors: true,
                    ..Default::default()
                },
            )?;
        }
        if let Some((mesh, fraction)) = transition {
            frame.append_surface_transition(
                body_request,
                mesh,
                fraction,
                SurfaceStyle {
                    elevation_colors: true,
                    ..Default::default()
                },
            )?;
        }
        let prep_ms = started.elapsed().as_secs_f64() * 1000.0;
        let rgba = gpu.render(&frame)?;
        let encode_ms = gpu.last_cpu_encode().as_secs_f64() * 1000.0;
        bitmap(
            &Path::new(&output).join(format!("{scene}-{tag}.bmp")),
            &rgba,
        )?;
        writeln!(
            manifest,
            "{tag}: prepare_ms={prep_ms} upload_encode_ms={encode_ms} surface={:?}",
            frame.report().surface
        )?;
        Ok(rgba)
    };
    let modes = [
        ("lit", TerrainRenderMode::Lit),
        ("normals", TerrainRenderMode::Normals),
        ("diffuse", TerrainRenderMode::Diffuse),
    ];
    let mut actual_old = Vec::new();
    let mut actual_new = Vec::new();
    for &(mode_name, mode) in &modes {
        actual_old.push(capture(
            &format!("actual-old-{mode_name}"),
            mode,
            None,
            &old_cover,
            &old_surface,
            &mut manifest,
        )?);
        actual_new.push(capture(
            &format!("actual-new-{mode_name}"),
            mode,
            None,
            &new_cover,
            &new_surface,
            &mut manifest,
        )?);
        for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let image = capture(
                &format!("split-{fraction:.2}-{mode_name}"),
                mode,
                Some((&transition, fraction)),
                &old_cover,
                &old_surface,
                &mut manifest,
            )?;
            if fraction == 0.0 {
                compare(
                    &image,
                    &actual_old[actual_old.len() - 1],
                    &mut manifest,
                    "split-old",
                )?;
            }
            if fraction == 1.0 {
                compare(
                    &image,
                    &actual_new[actual_new.len() - 1],
                    &mut manifest,
                    "split-new",
                )?;
            }
        }
        for fraction in [0.0, 0.5, 1.0] {
            let image = capture(
                &format!("merge-{fraction:.2}-{mode_name}"),
                mode,
                Some((&reverse, fraction)),
                &new_cover,
                &new_surface,
                &mut manifest,
            )?;
            if fraction == 0.0 {
                compare(
                    &image,
                    &actual_new[actual_new.len() - 1],
                    &mut manifest,
                    "merge-new",
                )?;
            }
            if fraction == 1.0 {
                compare(
                    &image,
                    &actual_old[actual_old.len() - 1],
                    &mut manifest,
                    "merge-old",
                )?;
            }
        }
    }
    let cache_report = cache.report();
    let estimated_peak = cache_report.resident_bytes
        + old_surface.resident_bytes()
        + new_surface.resident_bytes()
        + transition.resident_bytes()
        + reverse.resident_bytes()
        + TRANSITION_RESERVATION;
    writeln!(
        &mut manifest,
        "old_cover={} levels={:?} max_edge_delta={} triangles={} bytes={} build_ms={old_build_ms}; new_cover={} levels={:?} max_edge_delta={} triangles={} bytes={} build_ms={new_build_ms}\ntransition_triangles={} transition_bytes={} affected_old={} affected_new={} split_morph_prepare_ms={morph_prepare_ms}\nreverse_triangles={} reverse_bytes={} affected_old={} affected_new={} merge_morph_prepare_ms={reverse_prepare_ms}\ncache_bytes={} cache_peak_bytes={} estimated_construction_peak_bytes={} budget_bytes={}\n",
        old_cover.len(),
        distribution(&old_cover),
        max_edge_delta(&old_addresses),
        old_triangles,
        old_surface.resident_bytes(),
        new_cover.len(),
        distribution(&new_cover),
        max_edge_delta(&new_addresses),
        new_triangles,
        new_surface.resident_bytes(),
        transition.triangles().len(),
        transition.resident_bytes(),
        transition.affected_old().len(),
        transition.affected_new().len(),
        reverse.triangles().len(),
        reverse.resident_bytes(),
        reverse.affected_old().len(),
        reverse.affected_new().len(),
        cache_report.resident_bytes,
        cache_report.peak_bytes,
        estimated_peak,
        TERRAIN_CPU_CAP_BYTES
    )?;
    ensure!(
        estimated_peak <= TERRAIN_CPU_CAP_BYTES,
        "estimated construction exceeds 128 MiB"
    );
    fs::write(
        Path::new(&output).join(format!("{scene}-manifest.txt")),
        manifest,
    )?;
    Ok(())
}

fn compare(a: &[u8], b: &[u8], manifest: &mut String, name: &str) -> Result<()> {
    ensure!(a.len() == b.len(), "bitmap capture lengths differ");
    let mut different = 0usize;
    let mut maximum = 0u8;
    for (left, right) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let delta = (0..3)
            .map(|i| left[i].abs_diff(right[i]))
            .max()
            .unwrap_or(0);
        maximum = maximum.max(delta);
        different += usize::from(delta != 0);
    }
    writeln!(
        manifest,
        "{name}: differing_pixels={different} max_channel_delta={maximum} (float raster differences permitted)"
    )?;
    Ok(())
}

fn distribution(cover: &[ActiveSurfacePatch]) -> BTreeMap<u8, usize> {
    let mut levels = BTreeMap::new();
    for patch in cover {
        *levels.entry(patch.address.level()).or_insert(0) += 1;
    }
    levels
}

fn max_edge_delta(addresses: &[CubePatchAddress]) -> u8 {
    let set: BTreeSet<_> = addresses.iter().copied().collect();
    let mut maximum = 0;
    for address in addresses {
        for edge in PatchEdge::ALL {
            let mut neighbor = Some(address.neighbor(edge).address);
            while let Some(candidate) = neighbor {
                if set.contains(&candidate) {
                    maximum = maximum.max(address.level().abs_diff(candidate.level()));
                    break;
                }
                neighbor = candidate.parent();
            }
        }
    }
    maximum
}
