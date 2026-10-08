#![cfg(feature = "terrain-capture")]

use glam::{DMat3, DVec3};
use mundaris_math::{surface::*, *};
use mundaris_renderer::{terrain_capture::TerrainCaptureRenderer, *};
use std::{num::NonZeroU64, sync::Arc};

fn rotation(axis: DVec3, angle: f64) -> UnitRotation {
    UnitRotation::from_axis_angle(Direction3::try_new(axis).unwrap(), angle).unwrap()
}

fn position(frame: FrameId, value: DVec3) -> FramePosition {
    FramePosition::new(frame, LocalPosition::try_metres(value).unwrap())
}

// Synthetic, spatially varying derived payload isolates reconstruction precision
// from geological approximation. Runtime captures separately use the world oracle.
fn payload(radius: f64, address: CubePatchAddress, quadrant: Option<[u32; 2]>) -> TileData {
    const CELLS: u32 = 8;
    let side = CELLS + 3;
    let texels: Vec<_> = (0..side)
        .flat_map(|j| {
            (0..side).map(move |i| {
                let st = [
                    (f64::from(i) - 1.0) / f64::from(CELLS),
                    (f64::from(j) - 1.0) / f64::from(CELLS),
                ];
                let uv = quadrant.map_or(st, |q| {
                    std::array::from_fn(|axis| (f64::from(q[axis]) + st[axis]) * 0.5)
                });
                let a = (0.2 + 0.05 * uv[0]) as f32;
                let b = (0.3 - 0.05 * uv[0]) as f32;
                let c = (0.1 + 0.03 * uv[1]) as f32;
                TileTexel {
                    radial_offset_m: (4.0 * (6.0 * uv[0]).sin() + 3.0 * (4.0 * uv[1]).cos()) as f32,
                    material: [a, b, c, 1.0 - a - b - c],
                }
            })
        })
        .collect();
    TileData {
        key: TileKey {
            body_identity: 23,
            definition_words: vec![1, 0x2b],
            radius_bits: radius.to_bits(),
            surface_revision: 1,
            material_revision: 1,
            format_version: TILE_FORMAT_VERSION,
            filter_version: TILE_FILTER_VERSION,
            address,
            cells: CELLS,
        },
        anchor_radius_m: radius,
        min_max_radial_offset_m: [-7.0, 7.0],
        texels,
    }
}

#[test]
#[ignore = "requires a real GPU adapter; fixed hierarchy endpoints, seams and sibling-frame precision"]
fn hierarchy_gpu_matches_f64_and_preserves_borders_under_common_offsets() {
    const CELLS: u32 = 8;
    let mut renderer = TerrainCaptureRenderer::new(32, 32)
        .expect("GPU adapter required; this acceptance gate must not silently skip");
    let mut staging = CelestialStaging::default();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(32, 32, 60.0_f64.to_radians(), 0.1).unwrap();
    let mut slots: [TileSlotState; 5] = std::array::from_fn(|_| TileSlotState::default());
    let mut namespace = 900_u64;
    let mut worst_position_m = 0.0_f64;
    let mut worst_normal_rad = 0.0_f64;
    let mut worst_seam_m = 0.0_f64;
    let mut checked_points = 0;

    for (radius, level) in [(80_000.0_f64, 9), (6_371_000.0, 15), (70_000_000.0, 19)] {
        let extent = 1_u32 << level;
        let parent_address =
            CubePatchAddress::try_new(CubeFace::PositiveZ, level, extent / 2 - 1, extent / 2)
                .unwrap();
        let addresses = parent_address.children().unwrap();
        let tiles: [Arc<TileData>; 5] = std::array::from_fn(|i| {
            Arc::new(if i == 0 {
                payload(radius, parent_address, None)
            } else {
                payload(
                    radius,
                    addresses[i - 1],
                    Some(ResidentHierarchyDraw::quadrant(i - 1).unwrap()),
                )
            })
        });
        let publications: Vec<_> = slots
            .iter_mut()
            .zip(&tiles)
            .map(|(slot, tile)| slot.request(&tile.key).unwrap())
            .collect();
        let parent_anchor = tiles[0].anchor_position_body().unwrap();
        let mut baseline: Option<Vec<[f32; 3]>> = None;

        for offset in [0.0, 1.5e11, 1.0e16] {
            namespace += 1;
            let mut tree = FrameTree::new(NonZeroU64::new(namespace).unwrap());
            let common = tree
                .insert(
                    tree.root(),
                    FrameState::stationary(RigidTransform::new(
                        Displacement3::try_metres(DVec3::new(offset, 0.0, 0.0)).unwrap(),
                        UnitRotation::identity(),
                    )),
                )
                .unwrap();
            let body_rotation = rotation(DVec3::new(1.0, -2.0, 0.7), 0.63);
            let body = tree
                .insert(
                    common,
                    FrameState::stationary(RigidTransform::new(
                        Displacement3::zero(),
                        body_rotation,
                    )),
                )
                .unwrap();
            let regional_rotation =
                body_rotation.compose(rotation(DVec3::new(-0.2, 0.8, 1.0), -0.37));
            let observer_local = DVec3::new(2.0, -1.5, 3.0);
            let origin = body_rotation.quaternion() * (DVec3::Z * (radius + 40.0))
                - regional_rotation.quaternion() * observer_local;
            let regional = tree
                .insert(
                    common,
                    FrameState::stationary(RigidTransform::new(
                        Displacement3::try_metres(origin).unwrap(),
                        regional_rotation,
                    )),
                )
                .unwrap();
            let camera_rotation = rotation(DVec3::new(0.4, 1.0, -0.3), 0.29);
            let observer = FramePose::new(position(regional, observer_local), camera_rotation);
            let evaluation = tree.evaluate();
            let view =
                PreparedView::new(&evaluation, observer, RenderPrecisionBudget::near_debug())
                    .unwrap();
            let prepared = view.prepare_source(body).unwrap();
            let conversion = evaluation
                .prepare_conversion(body, regional)
                .unwrap()
                .rotation();
            let body_to_view =
                DMat3::from_quat(camera_rotation.inverse().compose(conversion).quaternion());
            let parent_view = prepared
                .view_displacement(position(body, parent_anchor))
                .unwrap()
                .metres();
            let draws: [TileDraw; 5] = std::array::from_fn(|i| TileDraw {
                tile: Arc::clone(&tiles[i]),
                publication: publications[i].clone(),
                anchor_view_m: parent_view
                    + body_to_view * (tiles[i].anchor_position_body().unwrap() - parent_anchor),
                body_to_view,
                mode: 2,
                sun_body: DVec3::new(0.3, -0.4, 0.8).normalize(),
                appearance: Default::default(),
            });
            let mut hierarchy = ResidentHierarchyDraw {
                parent: draws[0].clone(),
                children: std::array::from_fn(|i| Some(draws[i + 1].clone())),
                draw_children: true,
                morph_fraction: 0.0,
            };
            hierarchy.validate().unwrap();
            let mut offset_points = Vec::new();
            let mut stable_allocations = None;
            let mut stable_upload_count = None;
            for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
                hierarchy.morph_fraction = fraction;
                {
                    let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                    frame.set_resident_hierarchy(hierarchy.clone()).unwrap();
                    renderer.render(&frame).unwrap();
                }
                let report = renderer.last_resident_hierarchy_report();
                assert!(report.parent_pinned);
                assert!(
                    report
                        .resident_keys
                        .iter()
                        .zip(&tiles)
                        .all(|(key, tile)| key.as_ref() == Some(&tile.key))
                );
                if let Some(count) = stable_upload_count {
                    assert_eq!(report.resources.cumulative_content_upload_count, count);
                    assert_eq!(report.resources.tile_content_upload_bytes, 0);
                    assert_eq!(Some(report.resources.allocation_count), stable_allocations);
                } else {
                    stable_upload_count = Some(report.resources.cumulative_content_upload_count);
                    stable_allocations = Some(report.resources.allocation_count);
                }
                let mut gpu_patches = Vec::new();
                for patch in 0..5 {
                    let points = renderer
                        .validate_resident_hierarchy(&hierarchy, patch)
                        .unwrap();
                    assert_eq!(points.len(), (CELLS + 1).pow(2) as usize);
                    for (index, point) in points.iter().enumerate() {
                        assert!(
                            point
                                .position_local_m
                                .iter()
                                .chain(&point.position_view_m)
                                .chain(&point.normal_body)
                                .chain(&point.material)
                                .all(|v| v.is_finite())
                        );
                        let grid = [index as u32 % (CELLS + 1), index as u32 / (CELLS + 1)];
                        let cpu = hierarchy.reconstruct_patch(patch, grid).unwrap();
                        let local = DVec3::from_array(point.position_local_m.map(f64::from));
                        let gpu_view = DVec3::from_array(point.position_view_m.map(f64::from));
                        let exact = prepared
                            .view_displacement(position(
                                body,
                                parent_anchor + cpu.position_parent_local_m,
                            ))
                            .unwrap()
                            .metres();
                        let residual = (gpu_view - exact)
                            .length()
                            .max((local - cpu.position_parent_local_m).length());
                        worst_position_m = worst_position_m.max(residual);
                        assert!(
                            residual <= 1.0e-3,
                            "radius={radius} offset={offset} t={fraction} patch={patch} grid={grid:?} position={residual}m"
                        );
                        let normal = DVec3::from_array(point.normal_body.map(f64::from));
                        assert!((normal.length() - 1.0).abs() <= 1.0e-4);
                        let angle = normal
                            .normalize()
                            .dot(cpu.normal_body)
                            .clamp(-1.0, 1.0)
                            .acos();
                        worst_normal_rad = worst_normal_rad.max(angle);
                        assert!(angle <= 1.0e-3, "normal={angle}rad");
                        for (a, b) in point.material.iter().zip(cpu.material) {
                            assert!((f64::from(*a) - b).abs() <= 1.0e-5);
                        }
                        assert!((point.material.iter().sum::<f32>() - 1.0).abs() <= 1.0e-4);
                        offset_points.push(point.position_view_m);
                        checked_points += 1;
                    }
                    gpu_patches.push(points);
                }
                for (a, b, horizontal) in [(1, 2, true), (3, 4, true), (1, 3, false), (2, 4, false)]
                {
                    for i in 0..=CELLS {
                        let (ia, ib) = if horizontal {
                            (i * (CELLS + 1) + CELLS, i * (CELLS + 1))
                        } else {
                            (CELLS * (CELLS + 1) + i, i)
                        };
                        let left = &gpu_patches[a][ia as usize];
                        let right = &gpu_patches[b][ib as usize];
                        let error = (DVec3::from_array(left.position_local_m.map(f64::from))
                            - DVec3::from_array(right.position_local_m.map(f64::from)))
                        .length();
                        worst_seam_m = worst_seam_m.max(error);
                        assert!(error <= 1.0e-3, "GPU seam={error}m t={fraction}");
                        let nl = DVec3::from_array(left.normal_body.map(f64::from)).normalize();
                        let nr = DVec3::from_array(right.normal_body.map(f64::from)).normalize();
                        assert!(nl.dot(nr).clamp(-1.0, 1.0).acos() <= 1.0e-3);
                        for (l, r) in left.material.iter().zip(right.material) {
                            assert!((*l - r).abs() <= 1.0e-5);
                        }
                    }
                }
            }
            if let Some(previous) = &baseline {
                for (a, b) in offset_points.iter().zip(previous) {
                    assert!(
                        (DVec3::from_array(a.map(f64::from)) - DVec3::from_array(b.map(f64::from)))
                            .length()
                            <= 1.0e-3
                    );
                }
            } else {
                baseline = Some(offset_points);
            }
        }
    }
    println!(
        "hierarchy_precision points={checked_points} worst_position_m={worst_position_m:.12} worst_normal_rad={worst_normal_rad:.12} worst_shared_border_m={worst_seam_m:.12}"
    );
}
