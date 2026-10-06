#![cfg(feature = "terrain-capture")]

use glam::{DMat3, DVec3};
use mundaris_math::{surface::*, *};
use mundaris_renderer::{
    CelestialFrame, CelestialProjection, CelestialStaging, Icosphere, PreparedView,
    RegionalBoundaryEndpoints, RegionalPatchDraw, RegionalResidentDraw, RegionalTileUpload,
    RenderPrecisionBudget, TILE_FILTER_VERSION, TILE_FORMAT_VERSION, TileData, TileDraw, TileKey,
    TileSlotState, TileTexel, terrain_capture::TerrainCaptureRenderer,
};
use std::{collections::BTreeMap, num::NonZeroU64, sync::Arc};

fn rotation(axis: DVec3, angle: f64) -> UnitRotation {
    UnitRotation::from_axis_angle(Direction3::try_new(axis).unwrap(), angle).unwrap()
}

fn position(frame: FrameId, value: DVec3) -> FramePosition {
    FramePosition::new(frame, LocalPosition::try_metres(value).unwrap())
}

fn tile(radius: f64, address: CubePatchAddress) -> TileData {
    const CELLS: u32 = 8;
    let side = CELLS + 3;
    let [x, y] = address.coordinates();
    let count = 1_u32 << address.level();
    let texels = (0..side)
        .flat_map(|j| {
            (0..side).map(move |i| {
                let u = (f64::from(x) + (f64::from(i) - 1.0) / f64::from(CELLS)) / f64::from(count);
                let v = (f64::from(y) + (f64::from(j) - 1.0) / f64::from(CELLS)) / f64::from(count);
                let a = (0.22 + 0.04 * u) as f32;
                let b = (0.31 - 0.03 * u) as f32;
                let c = (0.12 + 0.02 * v) as f32;
                TileTexel {
                    radial_offset_m: (3.0 * (9.0 * u).sin() + 2.0 * (7.0 * v).cos()) as f32,
                    material: [a, b, c, 1.0 - a - b - c],
                }
            })
        })
        .collect();
    TileData {
        key: TileKey {
            body_identity: 911,
            definition_words: vec![7, 19, 23],
            radius_bits: radius.to_bits(),
            surface_revision: 3,
            material_revision: 5,
            format_version: TILE_FORMAT_VERSION,
            filter_version: TILE_FILTER_VERSION,
            address,
            cells: CELLS,
        },
        anchor_radius_m: radius,
        min_max_radial_offset_m: [-5.0, 5.0],
        texels,
    }
}

#[test]
#[ignore = "requires a real GPU adapter; verifies regional edge uploads and shader reconstruction"]
fn regional_shader_matches_cpu_and_keeps_adjacent_edges_watertight() {
    const RADIUS: f64 = 80_000.0;
    const CELLS: u32 = 8;
    let addresses = [
        CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 255, 256).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 256, 256).unwrap(),
    ];
    let tiles: Vec<_> = addresses
        .iter()
        .map(|address| Arc::new(tile(RADIUS, *address)))
        .collect();
    let tile_map: BTreeMap<_, _> = addresses
        .iter()
        .copied()
        .zip(tiles.iter().cloned())
        .collect();
    let boundaries = mundaris_renderer::regional_edges::build_boundaries(&tile_map).unwrap();

    let mut states = [
        TileSlotState::default(),
        TileSlotState::default(),
        TileSlotState::default(),
        TileSlotState::default(),
    ];
    let draws: Vec<_> = tiles
        .iter()
        .enumerate()
        .map(|(index, tile)| TileDraw {
            tile: Arc::clone(tile),
            publication: states[index].request(&tile.key).unwrap(),
            anchor_view_m: DVec3::ZERO,
            body_to_view: DMat3::IDENTITY,
            mode: 2,
            sun_body: DVec3::Z,
        })
        .collect();

    let mut regional = RegionalResidentDraw {
        capacity: 4,
        cells: CELLS,
        uploads: draws
            .iter()
            .cloned()
            .enumerate()
            .map(|(slot, tile)| RegionalTileUpload { slot, tile })
            .collect(),
        patches: addresses
            .iter()
            .enumerate()
            .map(|(index, _)| {
                let boundary = boundaries[&addresses[index]].clone();
                RegionalPatchDraw {
                    own_slot: index,
                    parent_slot: index,
                    own: draws[index].clone(),
                    parent: draws[index].clone(),
                    morph_fraction: 1.0,
                    boundary_fraction: 1.0,
                    quadrant: None,
                    quality_fallback: false,
                    boundary_endpoints: Arc::new(RegionalBoundaryEndpoints {
                        version: 1,
                        own_coarse: boundary.clone(),
                        own_fine: boundary.clone(),
                        parent: boundary,
                    }),
                }
            })
            .collect(),
    };

    let namespace = NonZeroU64::new(9821).unwrap();
    let mut tree = FrameTree::new(namespace);
    let common = tree
        .insert(
            tree.root(),
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(DVec3::new(4.0e10, 0.0, 0.0)).unwrap(),
                UnitRotation::identity(),
            )),
        )
        .unwrap();
    let body_rotation = rotation(DVec3::new(0.4, -0.7, 1.0), 0.43);
    let body = tree
        .insert(
            common,
            FrameState::stationary(RigidTransform::new(Displacement3::zero(), body_rotation)),
        )
        .unwrap();
    let regional_rotation = body_rotation.compose(rotation(DVec3::new(-0.2, 0.8, 0.3), -0.2));
    let observer_local = DVec3::new(1.0, -2.0, 1.5);
    let origin = body_rotation.quaternion() * (DVec3::Z * (RADIUS + 30.0))
        - regional_rotation.quaternion() * observer_local;
    let regional_frame = tree
        .insert(
            common,
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(origin).unwrap(),
                regional_rotation,
            )),
        )
        .unwrap();
    let camera_rotation = rotation(DVec3::new(0.3, 0.6, -0.4), 0.16);
    let observer = FramePose::new(position(regional_frame, observer_local), camera_rotation);
    let evaluation = tree.evaluate();
    let view =
        PreparedView::new(&evaluation, observer, RenderPrecisionBudget::near_debug()).unwrap();
    let prepared = view.prepare_source(body).unwrap();
    let conversion = evaluation
        .prepare_conversion(body, regional_frame)
        .unwrap()
        .rotation();
    let body_to_view = DMat3::from_quat(camera_rotation.inverse().compose(conversion).quaternion());
    for draw in &mut regional.patches {
        let anchor = draw.own.tile.anchor_position_body().unwrap();
        draw.own.anchor_view_m = prepared
            .view_displacement(position(body, anchor))
            .unwrap()
            .metres();
        draw.parent.anchor_view_m = draw.own.anchor_view_m;
    }
    let parent_address = CubePatchAddress::try_new(CubeFace::PositiveZ, 8, 127, 128).unwrap();
    let parent_tile = Arc::new(tile(RADIUS, parent_address));
    let parent_boundary = mundaris_renderer::regional_edges::build_boundaries(&BTreeMap::from([(
        parent_address,
        Arc::clone(&parent_tile),
    )]))
    .unwrap()
    .remove(&parent_address)
    .unwrap();
    let child_addresses = parent_address.children().unwrap();
    let child_tiles: Vec<_> = child_addresses
        .iter()
        .map(|address| Arc::new(tile(RADIUS, *address)))
        .collect();
    let child_tile_map: BTreeMap<_, _> = child_addresses
        .iter()
        .copied()
        .zip(child_tiles.iter().cloned())
        .collect();
    let child_boundaries =
        mundaris_renderer::regional_edges::build_boundaries(&child_tile_map).unwrap();
    let selected_child_index = child_addresses
        .iter()
        .position(|address| *address == addresses[0])
        .unwrap();
    let selected_child = Arc::clone(&child_tiles[selected_child_index]);
    let selected_child_boundary = child_boundaries[&addresses[0]].clone();
    let parent_anchor = parent_tile.anchor_position_body().unwrap();
    let parent_draw = TileDraw {
        tile: Arc::clone(&parent_tile),
        publication: states[2].request(&parent_tile.key).unwrap(),
        anchor_view_m: prepared
            .view_displacement(position(body, parent_anchor))
            .unwrap()
            .metres(),
        body_to_view,
        mode: 2,
        sun_body: DVec3::Z,
    };
    let child_draw = TileDraw {
        tile: Arc::clone(&selected_child),
        publication: states[3].request(&selected_child.key).unwrap(),
        anchor_view_m: prepared
            .view_displacement(position(
                body,
                selected_child.anchor_position_body().unwrap(),
            ))
            .unwrap()
            .metres(),
        body_to_view,
        mode: 2,
        sun_body: DVec3::Z,
    };
    let child_coarse_boundary = mundaris_renderer::regional_edges::subdivided_parent_boundary(
        &parent_tile,
        &parent_boundary,
        &selected_child,
    )
    .unwrap();
    regional.uploads.push(RegionalTileUpload {
        slot: 2,
        tile: parent_draw.clone(),
    });
    regional.uploads.push(RegionalTileUpload {
        slot: 3,
        tile: child_draw.clone(),
    });
    let parent_endpoints = Arc::new(RegionalBoundaryEndpoints {
        version: 1,
        own_coarse: parent_boundary.clone(),
        own_fine: parent_boundary.clone(),
        parent: parent_boundary.clone(),
    });
    regional.patches.push(RegionalPatchDraw {
        own_slot: 2,
        parent_slot: 2,
        own: parent_draw.clone(),
        parent: parent_draw.clone(),
        morph_fraction: 1.0,
        boundary_fraction: 1.0,
        quadrant: None,
        quality_fallback: false,
        boundary_endpoints: parent_endpoints,
    });
    regional.patches.push(RegionalPatchDraw {
        own_slot: 3,
        parent_slot: 2,
        own: child_draw,
        parent: parent_draw.clone(),
        morph_fraction: 0.0,
        boundary_fraction: 0.0,
        quadrant: Some([
            (selected_child_index % 2) as u32,
            (selected_child_index / 2) as u32,
        ]),
        quality_fallback: false,
        boundary_endpoints: Arc::new(RegionalBoundaryEndpoints {
            version: 1,
            own_coarse: child_coarse_boundary,
            own_fine: selected_child_boundary,
            parent: parent_boundary,
        }),
    });
    for (index, draw) in regional.patches.iter_mut().enumerate() {
        draw.own.body_to_view = body_to_view;
        draw.parent.body_to_view = body_to_view;
        regional.uploads[index].tile = draw.own.clone();
    }
    for patch in &regional.patches {
        patch.own.tile.validate().unwrap();
        patch.parent.tile.validate().unwrap();
        patch
            .own
            .validate_view_transform(RenderPrecisionBudget::near_debug())
            .unwrap();
        patch
            .parent
            .validate_view_transform(RenderPrecisionBudget::near_debug())
            .unwrap();
        for edge in patch
            .boundary_endpoints
            .own_coarse
            .edges
            .iter()
            .chain(&patch.boundary_endpoints.own_fine.edges)
            .chain(&patch.boundary_endpoints.parent.edges)
        {
            assert_eq!(edge.len(), CELLS as usize + 1);
            assert!(edge.iter().all(|v| {
                v.position_local_m.is_finite()
                    && v.normal_varying_body.is_finite()
                    && v.normal_varying_body.length_squared() > 0.0
                    && (v.material.iter().sum::<f64>() - 1.0).abs() < 1.0e-4
            }));
        }
    }
    regional.validate().unwrap();

    let mut capture = TerrainCaptureRenderer::new(32, 32)
        .expect("GPU adapter required; this acceptance gate must not silently skip");
    let mut staging = CelestialStaging::default();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(32, 32, 60.0_f64.to_radians(), 0.1).unwrap();
    {
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_resident_regional(regional.clone()).unwrap();
        capture.render(&frame).unwrap();
    }
    let report = capture.last_resident_regional_report();
    assert_eq!(report.capacity, 4);
    assert_eq!(report.resident_count, 4);
    assert_eq!(report.pinned_count, 4);
    assert_eq!(report.tile_upload_count, 4);
    assert!(report.boundary_upload_bytes > 0);
    assert_eq!(report.metadata_upload_bytes, 4 * 384);
    assert!(
        report
            .slots
            .iter()
            .all(|slot| slot.pinned && slot.key.is_some())
    );

    let mut gpu_patches = Vec::new();
    for patch_index in 0..regional.patches.len() {
        let gpu = capture
            .validate_resident_regional(&regional, patch_index)
            .unwrap();
        assert_eq!(gpu.len(), (CELLS + 1).pow(2) as usize);
        for (index, value) in gpu.iter().enumerate() {
            let grid = [index as u32 % (CELLS + 1), index as u32 / (CELLS + 1)];
            let cpu = mundaris_renderer::regional_resident::reconstruct_patch_node(
                &regional.patches[patch_index],
                grid,
                CELLS,
            )
            .unwrap();
            let position_local = DVec3::from_array(value.position_local_m.map(f64::from));
            assert!((position_local - cpu.position_local_m).length() <= 1.0e-3);
            let normal = DVec3::from_array(value.normal_body.map(f64::from)).normalize();
            let expected = cpu.normal_varying_body.normalize();
            let angle = normal.dot(expected).clamp(-1.0, 1.0).acos();
            assert!(angle <= 1.0e-3, "normal residual={angle} rad");
            for (actual, expected) in value.material.iter().zip(cpu.material) {
                assert!((f64::from(*actual) - expected).abs() <= 1.0e-5);
            }
        }
        gpu_patches.push(gpu);
    }
    let mut worst_seam_m = 0.0_f64;
    for along in 0..=CELLS {
        let left = &gpu_patches[0][(along * (CELLS + 1) + CELLS) as usize];
        let right = &gpu_patches[1][(along * (CELLS + 1)) as usize];
        let left_view = DVec3::from_array(left.position_view_m.map(f64::from));
        let right_view = DVec3::from_array(right.position_view_m.map(f64::from));
        worst_seam_m = worst_seam_m.max((left_view - right_view).length());
    }
    assert!(
        worst_seam_m <= 1.0e-3,
        "shared-edge residual={worst_seam_m} m"
    );

    // Exercise actual-parent triangle reconstruction and independently moving
    // interior and outgoing-boundary fractions through an in-progress split.
    for (morph_fraction, boundary_fraction) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)] {
        regional.patches[3].morph_fraction = morph_fraction;
        regional.patches[3].boundary_fraction = boundary_fraction;
        {
            let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
            frame.set_resident_regional(regional.clone()).unwrap();
            capture.render(&frame).unwrap();
        }
        let gpu = capture.validate_resident_regional(&regional, 3).unwrap();
        for (index, value) in gpu.iter().enumerate() {
            let grid = [index as u32 % (CELLS + 1), index as u32 / (CELLS + 1)];
            let cpu = mundaris_renderer::regional_resident::reconstruct_patch_node(
                &regional.patches[3],
                grid,
                CELLS,
            )
            .unwrap();
            let position_local = DVec3::from_array(value.position_local_m.map(f64::from));
            assert!((position_local - cpu.position_local_m).length() <= 1.0e-3);
            for (actual, expected) in value.material.iter().zip(cpu.material) {
                assert!((f64::from(*actual) - expected).abs() <= 1.0e-5);
            }
        }
    }

    // Stable resident draws upload only compact parameters after warm-up.
    {
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_resident_regional(regional.clone()).unwrap();
        capture.render(&frame).unwrap();
    }
    let warm = capture.last_resident_regional_report();
    assert_eq!(warm.tile_upload_bytes, 0);
    assert_eq!(warm.boundary_upload_bytes, 0);
    assert_eq!(warm.metadata_upload_bytes, 4 * 384);

    // Two local morph groups may advance their own endpoint versions while
    // unaffected patches keep their existing versions. Child dependencies
    // still agree on the exact boundary stored for their shared parent slot.
    let mut independently_versioned = regional.clone();
    for (index, version) in [(0, 17), (3, 29)] {
        let endpoints = &independently_versioned.patches[index].boundary_endpoints;
        independently_versioned.patches[index].boundary_endpoints =
            Arc::new(RegionalBoundaryEndpoints {
                version,
                own_coarse: endpoints.own_coarse.clone(),
                own_fine: endpoints.own_fine.clone(),
                parent: endpoints.parent.clone(),
            });
    }
    independently_versioned.validate().unwrap();
    {
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame
            .set_resident_regional(independently_versioned.clone())
            .unwrap();
        capture.render(&frame).unwrap();
    }
    let local_versions = capture.last_resident_regional_report();
    assert_eq!(local_versions.tile_upload_bytes, 0);
    assert_eq!(local_versions.boundary_upload_count, 2);

    // Exercise each regional diagnostic fragment branch through the production
    // shader pipeline while preserving the established modes 1 through 5.
    for mode in 6..=10 {
        let mut diagnostic = regional.clone();
        for patch in &mut diagnostic.patches {
            patch.own.mode = mode;
            patch.parent.mode = mode;
        }
        let pixels = {
            let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
            frame.set_resident_regional(diagnostic).unwrap();
            capture.render(&frame).unwrap()
        };
        assert_eq!(pixels.len(), 32 * 32 * 4);
    }

    // Fixed hierarchy and regional rendering use separate physical slot
    // namespaces. The same generation-1 token may therefore name a distinct
    // key in each mode, and the regional residency survives the round trip.
    let hierarchy = mundaris_renderer::ResidentHierarchyDraw {
        parent: parent_draw.clone(),
        children: std::array::from_fn(|_| None),
        draw_children: false,
        morph_fraction: 1.0,
    };
    {
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_resident_hierarchy(hierarchy.clone()).unwrap();
        capture.render(&frame).unwrap();
    }
    assert_eq!(
        capture.last_resident_hierarchy_report().resident_keys[0].as_ref(),
        Some(&parent_tile.key)
    );
    {
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_resident_regional(regional.clone()).unwrap();
        capture.render(&frame).unwrap();
    }
    let round_trip = capture.last_resident_regional_report();
    assert_eq!(round_trip.resident_count, 4);
    assert_eq!(round_trip.tile_upload_bytes, 0);
    assert_eq!(round_trip.allocated_slot_count, 4);

    // Reuse a slot after it is no longer part of the cover, then deliver its
    // obsolete generation. The stale transaction must fail before mutation,
    // while the three-patch cover that remains resident stays reconstructible.
    let mut replacement_tile = (*tiles[0]).clone();
    replacement_tile.key.surface_revision += 1;
    let replacement_tile = Arc::new(replacement_tile);
    let mut replacement_draw = regional.patches[0].own.clone();
    replacement_draw.tile = Arc::clone(&replacement_tile);
    replacement_draw.publication = states[0].request(&replacement_tile.key).unwrap();
    let mut reused = regional.clone();
    reused.uploads = vec![RegionalTileUpload {
        slot: 0,
        tile: replacement_draw.clone(),
    }];
    reused.patches.retain(|patch| patch.own_slot != 0);
    {
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_resident_regional(reused.clone()).unwrap();
        capture.render(&frame).unwrap();
    }
    let reused_report = capture.last_resident_regional_report();
    assert_eq!(
        reused_report.slots[0].key.as_ref(),
        Some(&replacement_tile.key)
    );
    assert_eq!(reused_report.slots[0].generation, 2);
    assert_eq!(reused_report.pinned_count, 3);

    let mut stale = reused.clone();
    stale.uploads = vec![RegionalTileUpload {
        slot: 0,
        tile: regional.patches[0].own.clone(),
    }];
    let stale_error = {
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_resident_regional(stale).unwrap();
        capture.render(&frame).unwrap_err()
    };
    assert!(matches!(
        stale_error,
        mundaris_renderer::RenderPreparationError::InvalidResidentTile
    ));
    let after_stale = capture.last_resident_regional_report();
    assert_eq!(
        after_stale.slots[0].key.as_ref(),
        Some(&replacement_tile.key)
    );
    assert_eq!(after_stale.slots[0].generation, 2);
    let retained_cover = capture.validate_resident_regional(&reused, 0).unwrap();
    assert_eq!(retained_cover.len(), (CELLS + 1).pow(2) as usize);
}
