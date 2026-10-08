//! Compare the opt-in raw Moon profile source with CPU-derived resident fields.
//!
//! Usage: cargo run --locked -p mundaris_app --features developer-tools --example phase2g_profile -- <raw.r16> <new-output-directory>

use anyhow::{Context, Result, ensure};
use glam::DVec3;
use mundaris_app::{
    resident_terrain::{FieldDensity, ResidentTileBuilder, SharedFieldPages, TileBuildIdentity},
    terrain_profile::load_height_profile,
};
use mundaris_math::surface::{CubeFace, CubePatchAddress, SurfaceLocation};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};
use serde_json::json;
use std::{
    fs::{self, File},
    path::Path,
    sync::Arc,
    time::Instant,
};

const MESH_CELLS: u32 = 32;
const FIELD_CAPACITY_BYTES: usize = 8 * 1024 * 1024;
const IMAGE_SIDE: usize = 256;
const PANEL_SIDE: usize = IMAGE_SIDE / 2;
const DENSITIES: [FieldDensity; 3] = [
    FieldDensity::Cells32,
    FieldDensity::Cells64,
    FieldDensity::Cells128,
];

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let source_path = args
        .next()
        .map(std::path::PathBuf::from)
        .context("usage: phase2g_profile <raw.r16> <new-output-directory>")?;
    let output_dir = args
        .next()
        .map(std::path::PathBuf::from)
        .context("usage: phase2g_profile <raw.r16> <new-output-directory>")?;
    let mut detail_inputs = Vec::new();
    let mut cubic = false;
    let mut terrain_scale = None;
    while let Some(flag) = args.next() {
        if flag == "--kernel" {
            let kernel = args.next().context("missing kernel")?;
            ensure!(
                kernel == "smoothstep" || kernel == "bspline",
                "unknown kernel"
            );
            cubic = kernel == "bspline";
            continue;
        }
        if flag == "--terrain-scale" {
            ensure!(terrain_scale.is_none(), "duplicate --terrain-scale");
            let footprint = args
                .next()
                .context("missing terrain footprint")?
                .to_string_lossy()
                .parse::<f64>()?;
            let amplitude = args
                .next()
                .context("missing terrain amplitude")?
                .to_string_lossy()
                .parse::<f64>()?;
            terrain_scale = Some((footprint, amplitude));
            continue;
        }
        ensure!(
            flag == "--detail-profile",
            "expected --detail-profile <raw.r16> <footprint_m> <amplitude_m>"
        );
        ensure!(detail_inputs.len() < 2, "at most two detail layers");
        let path = args
            .next()
            .map(std::path::PathBuf::from)
            .context("missing detail profile path")?;
        let footprint = args
            .next()
            .context("missing footprint metres")?
            .to_string_lossy()
            .parse::<f64>()?;
        let amplitude = args
            .next()
            .context("missing amplitude metres")?
            .to_string_lossy()
            .parse::<f64>()?;
        detail_inputs.push((path, footprint, amplitude));
    }
    ensure!(
        !output_dir.exists(),
        "refusing to replace existing evidence directory {}",
        output_dir.display()
    );
    let load_started = Instant::now();
    let mut profile = load_height_profile(&source_path)?;
    if cubic {
        profile = profile.with_cubic_bspline();
    }
    if let Some((footprint, amplitude)) = terrain_scale {
        profile = profile.with_terrain_scale(footprint, amplitude)?;
    }
    for (path, footprint, amplitude) in &detail_inputs {
        let detail = load_height_profile(path)?;
        let detail = if cubic {
            detail.with_cubic_bspline()
        } else {
            detail
        };
        profile = profile.with_detail_layer(detail, *footprint, *amplitude)?;
    }
    let profile_kernel = profile.kernel_name();
    let profile_load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
    let profile_identity_words = profile.identity_words();
    let profile_sample_range = profile.sample_range();
    let profile_dimensions = [profile.width(), profile.height()];
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(0x4d4f_4f4e),
        TerrainSeed(2),
        SurfaceAlgorithm::MoonProfileV1,
    )
    .with_height_profile(profile)?;
    let identity = TileBuildIdentity {
        body_identity: 0x4d4f_4f4e,
        surface_revision: 1,
        material_revision: 1,
    };
    fs::create_dir_all(&output_dir)?;

    let mut cases = Vec::new();
    let mut captures = Vec::new();
    for radius_m in [109_081.776_8, 1_737_400.0] {
        let generator_started = Instant::now();
        let generator = Arc::new(SurfaceGenerator::new(&definition, radius_m)?);
        let generator_build_ms = generator_started.elapsed().as_secs_f64() * 1000.0;
        for level in [2u8, 5, 12, 16] {
            let count = 1u32 << level;
            let address =
                CubePatchAddress::try_new(CubeFace::PositiveZ, level, count / 2, count / 2)?;
            let exact_started = Instant::now();
            let (exact_tile, exact_cost) =
                ResidentTileBuilder::build(&generator, identity, address, MESH_CELLS)?;
            let exact_wall_ms = exact_started.elapsed().as_secs_f64() * 1000.0;
            let mut field_tiles = Vec::with_capacity(DENSITIES.len());

            for density in DENSITIES {
                let cold_started = Instant::now();
                let pages = SharedFieldPages::new(
                    generator.clone(),
                    identity,
                    density,
                    FIELD_CAPACITY_BYTES,
                )?;
                let preparation_ms = cold_started.elapsed().as_secs_f64() * 1000.0;
                let (field_tile, cold_cost) = ResidentTileBuilder::build_fields(
                    &generator, identity, address, MESH_CELLS, &pages,
                )?;
                let cold_including_preparation_ms = cold_started.elapsed().as_secs_f64() * 1000.0;
                let cold_builder_wall_ms = cold_including_preparation_ms - preparation_ms;
                let warm_started = Instant::now();
                let (warm_tile, warm_cost) = ResidentTileBuilder::build_fields(
                    &generator, identity, address, MESH_CELLS, &pages,
                )?;
                let warm_wall_ms = warm_started.elapsed().as_secs_f64() * 1000.0;
                ensure!(
                    field_tile == warm_tile,
                    "warm field rebuild changed tile data"
                );

                let mut max_field_vs_exact_height_m = 0.0f64;
                let mut height_squared = 0.0f64;
                for (field_texel, exact_texel) in field_tile.texels.iter().zip(&exact_tile.texels) {
                    let delta = (f64::from(field_texel.radial_offset_m)
                        - f64::from(exact_texel.radial_offset_m))
                    .abs();
                    max_field_vs_exact_height_m = max_field_vs_exact_height_m.max(delta);
                    height_squared += delta * delta;
                }
                let mut max_field_vs_exact_normal_radians = 0.0f64;
                for y in 0..=MESH_CELLS {
                    for x in 0..=MESH_CELLS {
                        let exact_normal = exact_tile.normal_local([x, y])?;
                        let field_normal = field_tile.normal_local([x, y])?;
                        max_field_vs_exact_normal_radians = max_field_vs_exact_normal_radians
                            .max(exact_normal.dot(field_normal).clamp(-1.0, 1.0).acos());
                    }
                }
                let source_error =
                    ResidentTileBuilder::measure_approximation(&generator, &field_tile)?;
                let stats = pages.statistics();
                cases.push(json!({
                    "radius_m": radius_m,
                    "level": level,
                    "face": "PositiveZ",
                    "coordinates": address.coordinates(),
                    "seed": 2,
                    "mesh_cells": MESH_CELLS,
                    "field_cells": density.cells(),
                    "filter_version": density.filter_version(),
                    "source_profile_load_ms": profile_load_ms,
                    "surface_generator_build_ms": generator_build_ms,
                    "field_store_preparation_ms": preparation_ms,
                    "exact_tile_ms": exact_wall_ms,
                    "exact_authoritative_queries": exact_cost.authoritative_query_count,
                    "field_cold_including_preparation_ms": cold_including_preparation_ms,
                    "field_builder_wall_ms": cold_builder_wall_ms,
                    "field_builder_elapsed_ms": cold_cost.elapsed.as_secs_f64() * 1000.0,
                    "field_cold_authoritative_queries": cold_cost.authoritative_query_count,
                    "field_warm_reuse_ms": warm_wall_ms,
                    "field_warm_authoritative_queries": warm_cost.authoritative_query_count,
                    "field_warm_node_hits": pages.statistics().node_hits,
                    "field_vs_exact_max_height_m": max_field_vs_exact_height_m,
                    "field_vs_exact_rms_height_m":
                        (height_squared / field_tile.texels.len() as f64).sqrt(),
                    "field_vs_exact_max_normal_radians": max_field_vs_exact_normal_radians,
                    "field_vs_full_source_max_height_m": source_error.max_texel_radial_error_m,
                    "field_vs_full_source_rms_height_m": source_error.rms_texel_radial_error_m,
                    "field_vs_full_source_max_normal_radians": source_error.max_normal_angular_error_radians,
                    "field_triangle_centroid_max_error_m": source_error.max_triangle_centroid_error_m,
                    "field_capacity_bytes": stats.capacity_bytes,
                    "field_retained_bytes": stats.retained_bytes,
                    "field_page_allocations": stats.page_allocations,
                    "field_evictions": stats.evictions,
                    "quality_certificate": false
                }));
                // Keep owned tile payloads alive for the comparison PNG until all
                // panels for this address have been built.
                field_tiles.push(field_tile);
            }

            let image_name = format!("radius-{radius_m:.1}-level-{level}.png");
            write_mesh_comparison_png(
                &output_dir.join(&image_name),
                radius_m,
                level,
                [
                    &exact_tile,
                    &field_tiles[0],
                    &field_tiles[1],
                    &field_tiles[2],
                ],
            )?;
            let source_image_name = format!("radius-{radius_m:.1}-level-{level}-full-source.png");
            let source_image_started = Instant::now();
            write_full_source_png(
                &output_dir.join(&source_image_name),
                &generator,
                address,
                &exact_tile,
                radius_m,
            )?;
            captures.push(json!({
                "radius_m": radius_m,
                "level": level,
                "tile_comparison_png": image_name,
                "full_source_png": source_image_name,
                "full_source_sample_count": IMAGE_SIDE * IMAGE_SIDE,
                "full_source_capture_ms": source_image_started.elapsed().as_secs_f64() * 1000.0
            }));
        }
    }

    let result = json!({
        "schema_version": 1,
        "experiment": "MoonProfileV1 CPU field versus exact-source resident tile",
        "profile_kernel": profile_kernel,
        "terrain_scale_m": terrain_scale,
        "status": "opt-in CPU geometry comparison; not ordinary-native acceptance",
        "source_fixture": {
            "path": source_path,
            "width": profile_dimensions[0],
            "height": profile_dimensions[1],
            "encoding": "big-endian u16 raw height samples; canonical world content digest uses little-endian samples",
            "profile_identity_words": profile_identity_words,
            "source_sample_range": profile_sample_range,
            "amplitude_calibration": "MoonProfileV1 maps source min/max to default radius-scaled bands or explicit physical terrain scale; constant map is neutral",
            "profile_load_ms": profile_load_ms
        },
        "detail_layers": detail_inputs.iter().map(|(path, footprint, amplitude)| json!({"path": path, "footprint_m": footprint, "amplitude_m": amplitude})).collect::<Vec<_>>(),
        "comparison": {
            "seed": 2,
            "radii_m": [109081.7768, 1737400.0],
            "levels": [2, 5, 12, 16],
            "face": "PositiveZ",
            "mesh_cells": MESH_CELLS,
            "field_cells": [32, 64, 128],
            "field_capacity_bytes": FIELD_CAPACITY_BYTES,
            "tile_png_view": "four orthographic panels rasterized from reconstructed resident mesh triangles; exact filtered resident tile, field32, field64, field128",
            "tile_png_panel_order": ["exact_filtered_tile", "field32", "field64", "field128"],
            "full_source_png_view": "256x256 image rasterized from a 256x256 exact f64 source sample grid triangulated at the same physical patch footprint and orthographic view as the tile PNG",
            "cases": cases
        },
        "captures": captures,
        "limitations": [
            "finite sampled geometry metrics do not certify a global error bound",
            "captures use one face and centered patch at each listed level",
            "no native runtime, GPU producer, or visual acceptance is claimed"
        ]
    });
    fs::write(
        output_dir.join("comparison.json"),
        serde_json::to_vec_pretty(&result)?,
    )?;
    Ok(())
}

fn write_mesh_comparison_png(
    path: &Path,
    radius_m: f64,
    level: u8,
    tiles: [&mundaris_app::resident_terrain::TileData; 4],
) -> Result<()> {
    let mut rgba = vec![0u8; IMAGE_SIDE * IMAGE_SIDE * 4];
    for (panel_index, tile) in tiles.into_iter().enumerate() {
        let origin = [
            (panel_index % 2) * PANEL_SIDE,
            (panel_index / 2) * PANEL_SIDE,
        ];
        rasterize_tile(&mut rgba, origin, tile, radius_m, level)?;
    }
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(file, IMAGE_SIDE as u32, IMAGE_SIDE as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&rgba)?;
    Ok(())
}

fn rasterize_tile(
    image: &mut [u8],
    origin: [usize; 2],
    tile: &mundaris_app::resident_terrain::TileData,
    radius_m: f64,
    level: u8,
) -> Result<()> {
    let cells = tile.key.cells as usize;
    let mut positions = Vec::with_capacity((cells + 1) * (cells + 1));
    let mut normals = Vec::with_capacity((cells + 1) * (cells + 1));
    for y in 0..=cells {
        for x in 0..=cells {
            positions
                .push(tile.position_local([x as f64 / cells as f64, y as f64 / cells as f64])?);
            normals.push(tile.normal_local([x as u32, y as u32])?);
        }
    }
    rasterize_grid(
        image,
        origin,
        &positions,
        &normals,
        cells,
        PANEL_SIDE,
        radius_m / f64::from(1u32 << level) * 1.08,
    );
    Ok(())
}

fn write_full_source_png(
    path: &Path,
    generator: &SurfaceGenerator,
    address: CubePatchAddress,
    tile: &mundaris_app::resident_terrain::TileData,
    radius_m: f64,
) -> Result<()> {
    const SOURCE_CELLS: usize = IMAGE_SIDE - 1;
    let level = address.level();
    let count = 1u32 << level;
    let [patch_x, patch_y] = address.coordinates();
    let [anchor_x, anchor_y, anchor_z] = tile.anchor_position_body()?.to_array();
    let anchor = DVec3::new(anchor_x, anchor_y, anchor_z);
    let mut positions = Vec::with_capacity((SOURCE_CELLS + 1) * (SOURCE_CELLS + 1));
    let mut normals = Vec::with_capacity((SOURCE_CELLS + 1) * (SOURCE_CELLS + 1));
    for y in 0..=SOURCE_CELLS {
        for x in 0..=SOURCE_CELLS {
            let u = 2.0 * (f64::from(patch_x) + x as f64 / SOURCE_CELLS as f64) / f64::from(count)
                - 1.0;
            let v = 2.0 * (f64::from(patch_y) + y as f64 / SOURCE_CELLS as f64) / f64::from(count)
                - 1.0;
            let direction = address.face().direction([u, v])?;
            let location = SurfaceLocation::new(direction);
            let sample = generator.evaluate_point(location)?;
            positions.push(sample.position(location) - anchor);
            normals.push(sample.normal());
        }
    }
    let mut rgba = vec![0u8; IMAGE_SIDE * IMAGE_SIDE * 4];
    rasterize_grid(
        &mut rgba,
        [0, 0],
        &positions,
        &normals,
        SOURCE_CELLS,
        IMAGE_SIDE,
        radius_m / f64::from(count) * 1.08,
    );
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(file, IMAGE_SIDE as u32, IMAGE_SIDE as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&rgba)?;
    Ok(())
}

fn rasterize_grid(
    image: &mut [u8],
    origin: [usize; 2],
    positions: &[DVec3],
    normals: &[DVec3],
    cells: usize,
    panel_side: usize,
    half_extent: f64,
) {
    let light = DVec3::new(-0.38, -0.46, 0.80).normalize();
    let mut depth = vec![f64::NEG_INFINITY; panel_side * panel_side];
    let stride = cells + 1;
    for y in 0..cells {
        for x in 0..cells {
            let a = y * stride + x;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            for triangle in [[a, b, c], [b, d, c]] {
                let mut screen = [[0.0f64; 2]; 3];
                let mut z = [0.0f64; 3];
                let mut n = [DVec3::ZERO; 3];
                for i in 0..3 {
                    let p = positions[triangle[i]];
                    screen[i] = [
                        panel_side as f64 * 0.5 + p.x / half_extent * panel_side as f64 * 0.5,
                        panel_side as f64 * 0.5 - p.y / half_extent * panel_side as f64 * 0.5,
                    ];
                    z[i] = p.z;
                    n[i] = normals[triangle[i]];
                }
                let area = edge(screen[0], screen[1], screen[2]);
                if area.abs() < 1.0e-10 {
                    continue;
                }
                let min_x = screen
                    .iter()
                    .map(|p| p[0].floor() as isize)
                    .min()
                    .unwrap()
                    .max(0) as usize;
                let max_x = screen
                    .iter()
                    .map(|p| p[0].ceil() as isize)
                    .max()
                    .unwrap()
                    .min(panel_side as isize - 1) as usize;
                let min_y = screen
                    .iter()
                    .map(|p| p[1].floor() as isize)
                    .min()
                    .unwrap()
                    .max(0) as usize;
                let max_y = screen
                    .iter()
                    .map(|p| p[1].ceil() as isize)
                    .max()
                    .unwrap()
                    .min(panel_side as isize - 1) as usize;
                for py in min_y..=max_y {
                    for px in min_x..=max_x {
                        let point = [px as f64 + 0.5, py as f64 + 0.5];
                        let weights = [
                            edge(screen[1], screen[2], point) / area,
                            edge(screen[2], screen[0], point) / area,
                            edge(screen[0], screen[1], point) / area,
                        ];
                        if weights.iter().any(|w| *w < -1.0e-8) {
                            continue;
                        }
                        let pixel = py * panel_side + px;
                        let pixel_depth = weights[0] * z[0] + weights[1] * z[1] + weights[2] * z[2];
                        if pixel_depth <= depth[pixel] {
                            continue;
                        }
                        depth[pixel] = pixel_depth;
                        let normal =
                            (n[0] * weights[0] + n[1] * weights[1] + n[2] * weights[2]).normalize();
                        let shade = (0.20 + 0.80 * normal.dot(light).max(0.0)).clamp(0.0, 1.0);
                        let value = (shade * 255.0).round() as u8;
                        let index = ((origin[1] + py) * IMAGE_SIDE + origin[0] + px) * 4;
                        image[index..index + 4].copy_from_slice(&[value, value, value, 255]);
                    }
                }
            }
        }
    }
}

fn edge(a: [f64; 2], b: [f64; 2], point: [f64; 2]) -> f64 {
    (point[0] - a[0]) * (b[1] - a[1]) - (point[1] - a[1]) * (b[0] - a[0])
}
