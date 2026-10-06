#![cfg(feature = "terrain-capture")]

use std::{num::NonZeroU64, path::PathBuf, sync::Arc};

use anyhow::Result;
use glam::{DMat3, DVec3};
use mundaris_app::resident_terrain::{ResidentTileBuilder, TileBuildIdentity};
use mundaris_math::{
    Direction3, FrameId, FramePose, FramePosition, FrameTree, LocalPosition, UnitRotation,
    surface::{CubeFace, CubePatchAddress},
};
use mundaris_renderer::{
    CelestialFrame, CelestialProjection, CelestialStaging, Icosphere, PreparedView,
    RenderPrecisionBudget, TileData, TileDraw, TileSlotState,
    terrain_capture::TerrainCaptureRenderer,
};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};

const BODY_IDENTITY: u64 = 5_931_033_225_171_238_913;
const CELLS: u32 = 64;
const MORPH_FRACTIONS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

#[derive(Default)]
struct Residuals {
    max_local_m: f64,
    sum_local_sq: f64,
    max_view_m: f64,
    sum_view_sq: f64,
    max_normal_rad: f64,
    sum_normal_sq: f64,
    max_material_l2: f64,
    sum_material_sq: f64,
    samples: u64,
}

impl Residuals {
    fn record(&mut self, local_m: f64, view_m: f64, normal_rad: f64, material_l2: f64) {
        self.max_local_m = self.max_local_m.max(local_m);
        self.sum_local_sq += local_m * local_m;
        self.max_view_m = self.max_view_m.max(view_m);
        self.sum_view_sq += view_m * view_m;
        self.max_normal_rad = self.max_normal_rad.max(normal_rad);
        self.sum_normal_sq += normal_rad * normal_rad;
        self.max_material_l2 = self.max_material_l2.max(material_l2);
        self.sum_material_sq += material_l2 * material_l2;
        self.samples += 1;
    }

    fn report(&self) -> serde_json::Value {
        let denominator = self.samples.max(1) as f64;
        serde_json::json!({
            "samples": self.samples,
            "max_local_position_m": self.max_local_m,
            "rms_local_position_m": (self.sum_local_sq / denominator).sqrt(),
            "max_view_position_m": self.max_view_m,
            "rms_view_position_m": (self.sum_view_sq / denominator).sqrt(),
            "max_normal_angle_radians": self.max_normal_rad,
            "rms_normal_angle_radians": (self.sum_normal_sq / denominator).sqrt(),
            "max_material_l2": self.max_material_l2,
            "rms_material_l2": (self.sum_material_sq / denominator).sqrt(),
        })
    }
}

fn direction(view: &PreparedView<'_>, body: FrameId, axis: DVec3) -> Result<DVec3> {
    Ok(view
        .prepare_source(body)?
        .view_direction(Direction3::try_new(axis)?)?
        .unit())
}

fn build_draw(
    tile: Arc<TileData>,
    publication: mundaris_renderer::TilePublicationToken,
    anchor_view_m: DVec3,
    body_to_view: DMat3,
) -> TileDraw {
    TileDraw {
        tile,
        publication,
        anchor_view_m,
        body_to_view,
        mode: 0,
        sun_body: DVec3::new(0.3, 0.7, 0.5).normalize(),
    }
}

fn angle(a: DVec3, b: DVec3) -> f64 {
    let a = a.normalize();
    let b = b.normalize();
    a.cross(b).length().atan2(a.dot(b))
}

fn point_at(
    points: &[mundaris_renderer::ReconstructedTileVertex],
    grid: [u32; 2],
) -> &mundaris_renderer::ReconstructedTileVertex {
    &points[(grid[1] * (CELLS + 1) + grid[0]) as usize]
}

fn material_l2(a: &[f32; 4], b: [f64; 4]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(left, right)| (f64::from(*left) - right).powi(2))
        .sum::<f64>()
        .sqrt()
}

fn gpu_shared_seam_metrics(
    patches: &[Vec<mundaris_renderer::ReconstructedTileVertex>],
) -> (f64, f64, f64) {
    let mut max_position_m = 0.0_f64;
    let mut max_normal_rad = 0.0_f64;
    let mut max_material_l2 = 0.0_f64;
    // Child order: lower-left, lower-right, upper-left, upper-right.
    let shared_edges = [
        (1usize, 2usize, true),
        (3, 4, true),
        (1, 3, false),
        (2, 4, false),
    ];
    for (left_patch, right_patch, vertical) in shared_edges {
        for along in 0..=CELLS {
            let (left_grid, right_grid) = if vertical {
                ([CELLS, along], [0, along])
            } else {
                ([along, CELLS], [along, 0])
            };
            let left = point_at(&patches[left_patch], left_grid);
            let right = point_at(&patches[right_patch], right_grid);
            max_position_m = max_position_m.max(
                (DVec3::from_array(left.position_local_m.map(f64::from))
                    - DVec3::from_array(right.position_local_m.map(f64::from)))
                .length(),
            );
            max_normal_rad = max_normal_rad.max(angle(
                DVec3::from_array(left.normal_body.map(f64::from)),
                DVec3::from_array(right.normal_body.map(f64::from)),
            ));
            max_material_l2 =
                max_material_l2.max(material_l2(&left.material, right.material.map(f64::from)));
        }
    }
    // The central four-child corner is distinct from the four edge interiors.
    let corners = [
        point_at(&patches[1], [CELLS, CELLS]),
        point_at(&patches[2], [0, CELLS]),
        point_at(&patches[3], [CELLS, 0]),
        point_at(&patches[4], [0, 0]),
    ];
    for candidate in &corners[1..] {
        max_position_m = max_position_m.max(
            (DVec3::from_array(corners[0].position_local_m.map(f64::from))
                - DVec3::from_array(candidate.position_local_m.map(f64::from)))
            .length(),
        );
        max_normal_rad = max_normal_rad.max(angle(
            DVec3::from_array(corners[0].normal_body.map(f64::from)),
            DVec3::from_array(candidate.normal_body.map(f64::from)),
        ));
        max_material_l2 = max_material_l2.max(material_l2(
            &corners[0].material,
            candidate.material.map(f64::from),
        ));
    }
    (max_position_m, max_normal_rad, max_material_l2)
}

#[test]
#[ignore = "requires a real GPU adapter; runs the canonical world-generated parent/four-child hierarchy"]
fn world_generated_hierarchy_gpu_matches_f64_oracle_and_shared_borders() -> Result<()> {
    let mut renderer = TerrainCaptureRenderer::new(32, 32)
        .expect("GPU adapter required; acceptance gate must not silently skip");
    let mut staging = CelestialStaging::default();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(32, 32, 60.0_f64.to_radians(), 0.1)?;
    let mut slots: [TileSlotState; 5] = std::array::from_fn(|_| TileSlotState::default());
    let mut fixture_reports = Vec::new();

    for (radius_m, level, x, y, label) in [
        (80_000.0, 9, 157, 39, "canonical_interior"),
        (
            6_371_000.0,
            15,
            (1 << 15) - 1,
            (1 << 14) / 2,
            "large_radius_edge",
        ),
        (70_000_000.0, 19, 0, 0, "very_large_radius_corner"),
    ] {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(BODY_IDENTITY),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        let generator = SurfaceGenerator::new(&definition, radius_m)?;
        let identity = TileBuildIdentity {
            body_identity: BODY_IDENTITY,
            surface_revision: 2,
            material_revision: definition.material_identity(),
        };
        let parent_address = CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y)?;
        let child_addresses = parent_address.children()?;
        let mut build_diagnostics = Vec::with_capacity(5);
        let tiles: [Arc<TileData>; 5] = std::array::from_fn(|index| {
            let address = if index == 0 {
                parent_address
            } else {
                child_addresses[index - 1]
            };
            let (tile, diagnostics) =
                ResidentTileBuilder::build(&generator, identity, address, CELLS).unwrap();
            build_diagnostics.push(serde_json::json!({
                "address": {
                    "face": "positive_z",
                    "level": address.level(),
                    "x": address.coordinates()[0],
                    "y": address.coordinates()[1],
                },
                "query_count": diagnostics.authoritative_query_count,
                "payload_bytes": diagnostics.payload_bytes,
                "elapsed_ms": diagnostics.elapsed.as_secs_f64() * 1000.0,
            }));
            Arc::new(tile)
        });
        let parent_anchor = tiles[0].anchor_position_body()?;

        let tree = FrameTree::new(NonZeroU64::new(70_000 + u64::from(level)).unwrap());
        let body = tree.root();
        let radial = parent_anchor.normalize();
        let eye = parent_anchor + radial * 1_000.0;
        let observer = FramePose::new(
            FramePosition::new(body, LocalPosition::try_metres(eye)?),
            UnitRotation::identity(),
        );
        let evaluation = tree.evaluate();
        let view = PreparedView::new(&evaluation, observer, RenderPrecisionBudget::near_debug())?;
        let source = view.prepare_source(body)?;
        let parent_view = source
            .view_displacement(FramePosition::new(
                body,
                LocalPosition::try_metres(parent_anchor)?,
            ))?
            .metres();
        let body_to_view = DMat3::from_cols(
            direction(&view, body, DVec3::X)?,
            direction(&view, body, DVec3::Y)?,
            direction(&view, body, DVec3::Z)?,
        );
        let publications: Vec<_> = slots
            .iter_mut()
            .zip(&tiles)
            .map(|(slot, tile)| slot.request(&tile.key).unwrap())
            .collect();
        let draws: [TileDraw; 5] = std::array::from_fn(|index| {
            let tile_anchor = tiles[index].anchor_position_body().unwrap();
            let anchor_view = parent_view + body_to_view * (tile_anchor - parent_anchor);
            build_draw(
                Arc::clone(&tiles[index]),
                publications[index].clone(),
                anchor_view,
                body_to_view,
            )
        });
        let mut hierarchy = mundaris_renderer::ResidentHierarchyDraw {
            parent: draws[0].clone(),
            children: std::array::from_fn(|index| Some(draws[index + 1].clone())),
            draw_children: true,
            morph_fraction: 0.0,
        };
        hierarchy.validate()?;

        // Keep the complete world oracle as authority and report its actual
        // approximation by the parent derived tile for each fixture.
        let world_to_tile = ResidentTileBuilder::measure_approximation(&generator, &tiles[0])?;
        let mut residuals = Residuals::default();
        let mut seam_max = [0.0_f64; 3];
        let mut uploaded_bytes_by_morph = Vec::with_capacity(MORPH_FRACTIONS.len());
        let mut cumulative_upload_count = None;

        for fraction in MORPH_FRACTIONS {
            hierarchy.morph_fraction = fraction;
            {
                let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                frame.set_resident_hierarchy(hierarchy.clone())?;
                renderer.render(&frame)?;
            }
            let report = renderer.last_resident_hierarchy_report();
            assert!(
                report.parent_pinned,
                "parent must remain pinned through the morph"
            );
            assert!(
                report
                    .resident_keys
                    .iter()
                    .zip(&tiles)
                    .all(|(key, tile)| { key.as_ref() == Some(&tile.key) })
            );
            if let Some(previous_count) = cumulative_upload_count {
                assert_eq!(
                    report.resources.cumulative_content_upload_count,
                    previous_count
                );
                assert_eq!(report.resources.tile_content_upload_bytes, 0);
            } else {
                cumulative_upload_count = Some(report.resources.cumulative_content_upload_count);
            }
            uploaded_bytes_by_morph.push(report.resources.tile_content_upload_bytes);

            let mut gpu_patches = Vec::with_capacity(5);
            for patch in 0..5 {
                let points = renderer.validate_resident_hierarchy(&hierarchy, patch)?;
                assert_eq!(points.len(), (CELLS + 1).pow(2) as usize);
                for (index, point) in points.iter().enumerate() {
                    let grid = [index as u32 % (CELLS + 1), index as u32 / (CELLS + 1)];
                    let cpu = hierarchy.reconstruct_patch(patch, grid)?;
                    let gpu_local = DVec3::from_array(point.position_local_m.map(f64::from));
                    let gpu_view = DVec3::from_array(point.position_view_m.map(f64::from));
                    let gpu_normal = DVec3::from_array(point.normal_body.map(f64::from));
                    let gpu_material = point.material;
                    assert!((gpu_normal.length() - 1.0).abs() <= 1.0e-4);
                    assert!(
                        gpu_material
                            .iter()
                            .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
                    );
                    assert!((gpu_material.iter().sum::<f32>() - 1.0).abs() <= 1.0e-4);
                    let local_error = (gpu_local - cpu.position_parent_local_m).length();
                    let view_error = (gpu_view - hierarchy.position_view(&cpu)).length();
                    let normal_error = angle(gpu_normal, cpu.normal_body);
                    let material_error = material_l2(&gpu_material, cpu.material);
                    assert!(
                        local_error <= 1.0e-3,
                        "local residual {local_error}m at {label} t={fraction} patch={patch} grid={grid:?}"
                    );
                    assert!(
                        view_error <= 1.0e-3,
                        "view residual {view_error}m at {label} t={fraction} patch={patch} grid={grid:?}"
                    );
                    assert!(
                        normal_error <= 1.0e-3,
                        "normal residual {normal_error}rad at {label} t={fraction} patch={patch} grid={grid:?}"
                    );
                    assert!(
                        material_error <= 1.0e-5,
                        "material residual {material_error} at {label} t={fraction} patch={patch} grid={grid:?}"
                    );
                    residuals.record(local_error, view_error, normal_error, material_error);
                }
                gpu_patches.push(points);
            }
            let seams = gpu_shared_seam_metrics(&gpu_patches);
            seam_max[0] = seam_max[0].max(seams.0);
            seam_max[1] = seam_max[1].max(seams.1);
            seam_max[2] = seam_max[2].max(seams.2);
            assert!(
                seams.0 <= 1.0e-3,
                "GPU child seam position {}m at t={fraction}",
                seams.0
            );
            assert!(
                seams.1 <= 1.0e-3,
                "GPU child seam normal {}rad at t={fraction}",
                seams.1
            );
            assert!(
                seams.2 <= 1.0e-5,
                "GPU child seam material {} at t={fraction}",
                seams.2
            );
        }

        fixture_reports.push(serde_json::json!({
            "label": label,
            "body_identity": BODY_IDENTITY,
            "definition_identity": definition.identity().0,
            "terrain_identity": definition.terrain_identity(),
            "material_identity": definition.material_identity(),
            "radius_m": radius_m,
            "parent_address": {
                "face": "positive_z",
                "level": level,
                "x": x,
                "y": y,
            },
            "cells": CELLS,
            "builds": build_diagnostics,
            "world_to_parent_tile": {
                "query_count": world_to_tile.texel_query_count,
                "triangle_centroid_query_count": world_to_tile.triangle_centroid_query_count,
                "max_texel_radial_error_m": world_to_tile.max_texel_radial_error_m,
                "rms_texel_radial_error_m": world_to_tile.rms_texel_radial_error_m,
                "max_triangle_centroid_error_m": world_to_tile.max_triangle_centroid_error_m,
                "rms_triangle_centroid_error_m": world_to_tile.rms_triangle_centroid_error_m,
                "max_normal_angular_error_radians": world_to_tile.max_normal_angular_error_radians,
                "rms_normal_angular_error_radians": world_to_tile.rms_normal_angular_error_radians,
            },
            "gpu_vs_cpu_f64": residuals.report(),
            "gpu_shared_child_seams": {
                "max_position_m": seam_max[0],
                "max_normal_angle_radians": seam_max[1],
                "max_material_l2": seam_max[2],
            },
            "terrain_content_upload_bytes_by_morph": uploaded_bytes_by_morph,
            "cumulative_content_upload_count": cumulative_upload_count,
        }));
    }

    let evidence = serde_json::json!({
        "schema": "mundaris.resident_hierarchy_world_gpu.v1",
        "gpu_adapter": renderer.adapter_name(),
        "gpu_backend": renderer.adapter_backend(),
        "fixtures": fixture_reports,
    });
    println!("{}", serde_json::to_string_pretty(&evidence)?);
    if let Some(path) = std::env::var_os("MUNDARIS_HIERARCHY_EVIDENCE") {
        let path = PathBuf::from(path);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&evidence)?)?;
        println!("wrote hierarchy evidence to {}", path.display());
    }
    Ok(())
}
