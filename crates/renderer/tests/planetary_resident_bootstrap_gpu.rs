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

const MOON_RADIUS_METERS: f64 = 1_737_400.0;
const CELLS: u32 = 4;
const LOGICAL_CAPACITY: usize = 64;

fn position(frame: FrameId, value: DVec3) -> FramePosition {
    FramePosition::new(frame, LocalPosition::try_metres(value).unwrap())
}

fn root_tile(face: CubeFace) -> Arc<TileData> {
    Arc::new(TileData {
        key: TileKey {
            body_identity: 0x4d4f_4f4e,
            definition_words: vec![0x706c_616e_6574],
            radius_bits: MOON_RADIUS_METERS.to_bits(),
            surface_revision: 1,
            material_revision: 1,
            format_version: TILE_FORMAT_VERSION,
            filter_version: TILE_FILTER_VERSION,
            address: CubePatchAddress::try_new(face, 0, 0, 0).unwrap(),
            cells: CELLS,
        },
        anchor_radius_m: MOON_RADIUS_METERS,
        min_max_radial_offset_m: [0.0, 0.0],
        texels: vec![
            TileTexel {
                radial_offset_m: 0.0,
                material: [1.0, 0.0, 0.0, 0.0],
            };
            (CELLS + 3).pow(2) as usize
        ],
    })
}

#[test]
#[ignore = "requires a real GPU adapter; verifies planetary six-root upload-only bootstrap"]
fn planetary_six_root_bootstrap_uploads_without_draw_then_warms_stably() {
    let tiles: Vec<_> = CubeFace::ALL.into_iter().map(root_tile).collect();
    let tile_map: BTreeMap<_, _> = tiles
        .iter()
        .map(|tile| (tile.key.address, Arc::clone(tile)))
        .collect();
    let boundaries = mundaris_renderer::regional_edges::build_boundaries(&tile_map).unwrap();

    let mut tree = FrameTree::new(NonZeroU64::new(0x504c_414e_4554).unwrap());
    let body = tree
        .insert(
            tree.root(),
            FrameState::stationary(RigidTransform::new(
                Displacement3::zero(),
                UnitRotation::identity(),
            )),
        )
        .unwrap();
    // A fixed camera looks inward from one Moon radius plus one million metres.
    let observer = FramePose::new(
        position(
            tree.root(),
            DVec3::new(0.0, 0.0, MOON_RADIUS_METERS + 1_000_000.0),
        ),
        UnitRotation::identity(),
    );
    let evaluation = tree.evaluate();
    let view = PreparedView::new(
        &evaluation,
        observer,
        RenderPrecisionBudget::try_new(10_000_000.0, 0.5).unwrap(),
    )
    .unwrap();
    let prepared = view.prepare_source(body).unwrap();
    let body_to_view = DMat3::from_quat(
        observer
            .orientation()
            .inverse()
            .compose(
                evaluation
                    .prepare_conversion(body, observer.position().frame())
                    .unwrap()
                    .rotation(),
            )
            .quaternion(),
    );

    let mut slot_states = vec![TileSlotState::default(); tiles.len()];
    let draws: Vec<_> = tiles
        .iter()
        .enumerate()
        .map(|(slot, tile)| TileDraw {
            tile: Arc::clone(tile),
            publication: slot_states[slot].request(&tile.key).unwrap(),
            anchor_view_m: prepared
                .view_displacement(position(body, tile.anchor_position_body().unwrap()))
                .unwrap()
                .metres(),
            body_to_view,
            mode: 0,
            sun_body: DVec3::Z,
            appearance: Default::default(),
        })
        .collect();

    let mut capture = TerrainCaptureRenderer::new(32, 32)
        .expect("GPU adapter required; this acceptance gate must not silently skip");
    let mut staging = CelestialStaging::default();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(32, 32, 60.0_f64.to_radians(), 0.1).unwrap();

    for chunk_start in [0, 3] {
        let uploads = draws
            .iter()
            .cloned()
            .enumerate()
            .skip(chunk_start)
            .take(3)
            .map(|(slot, tile)| RegionalTileUpload { slot, tile })
            .collect();
        let upload_only = RegionalResidentDraw {
            planetary: true,
            capacity: LOGICAL_CAPACITY,
            cells: CELLS,
            uploads,
            patches: vec![],
        };
        assert!(upload_only.validate().is_ok());
        {
            let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
            frame.set_resident_regional(upload_only).unwrap();
            capture.render(&frame).unwrap();
        }
        let report = capture.last_resident_regional_report();
        assert_eq!(report.capacity, LOGICAL_CAPACITY);
        assert_eq!(report.resident_count, chunk_start + 3);
        assert_eq!(report.pinned_count, 0);
        assert_eq!(report.boundary_upload_count, 0);
        assert_eq!(report.boundary_upload_bytes, 0);
        assert_eq!(report.metadata_upload_bytes, 0);
        assert_eq!(report.allocated_slot_count, chunk_start + 3);
        assert_eq!(report.slots.len(), chunk_start + 3);
        assert!(report.slots.iter().all(|slot| slot.key.is_some()));
    }

    let complete_cover = RegionalResidentDraw {
        planetary: true,
        capacity: LOGICAL_CAPACITY,
        cells: CELLS,
        uploads: vec![],
        patches: draws
            .iter()
            .enumerate()
            .map(|(slot, draw)| {
                let boundary = boundaries[&draw.tile.key.address].clone();
                RegionalPatchDraw {
                    own_slot: slot,
                    parent_slot: slot,
                    own: draw.clone(),
                    parent: draw.clone(),
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
    assert!(complete_cover.validate().is_ok());
    {
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_resident_regional(complete_cover.clone()).unwrap();
        capture.render(&frame).unwrap();
    }
    let first_cover_report = capture.last_resident_regional_report();
    assert_eq!(first_cover_report.resident_count, 6);
    assert_eq!(first_cover_report.pinned_count, 6);
    assert_eq!(first_cover_report.allocated_slot_count, 6);
    assert_eq!(first_cover_report.slots.len(), 6);
    assert_eq!(first_cover_report.tile_upload_count, 0);
    assert_eq!(first_cover_report.tile_upload_bytes, 0);
    assert!(first_cover_report.boundary_upload_count > 0);
    assert!(first_cover_report.tile_capacity_bytes > 0);
    assert!(first_cover_report.boundary_capacity_bytes > 0);
    assert!(first_cover_report.metadata_capacity_bytes > 0);
    assert!(first_cover_report.grid_capacity_bytes > 0);
    assert!(
        first_cover_report.tile_capacity_bytes
            + first_cover_report.boundary_capacity_bytes
            + first_cover_report.metadata_capacity_bytes
            + first_cover_report.grid_capacity_bytes
            > 0
    );

    let cumulative_tile_upload_bytes = first_cover_report.cumulative_tile_upload_bytes;
    let cumulative_tile_upload_count = first_cover_report.cumulative_tile_upload_count;
    let cumulative_boundary_upload_bytes = first_cover_report.cumulative_boundary_upload_bytes;
    let cumulative_boundary_upload_count = first_cover_report.cumulative_boundary_upload_count;
    for _ in 0..3 {
        {
            let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
            frame.set_resident_regional(complete_cover.clone()).unwrap();
            capture.render(&frame).unwrap();
        }
        let warm = capture.last_resident_regional_report();
        assert_eq!(warm.resident_count, 6);
        assert_eq!(warm.pinned_count, 6);
        assert_eq!(warm.allocated_slot_count, 6);
        assert_eq!(warm.tile_upload_bytes, 0);
        assert_eq!(warm.tile_upload_count, 0);
        assert_eq!(warm.boundary_upload_bytes, 0);
        assert_eq!(warm.boundary_upload_count, 0);
        assert_eq!(warm.metadata_upload_bytes, 0);
        assert_eq!(
            warm.cumulative_tile_upload_bytes,
            cumulative_tile_upload_bytes
        );
        assert_eq!(
            warm.cumulative_tile_upload_count,
            cumulative_tile_upload_count
        );
        assert_eq!(
            warm.cumulative_boundary_upload_bytes,
            cumulative_boundary_upload_bytes
        );
        assert_eq!(
            warm.cumulative_boundary_upload_count,
            cumulative_boundary_upload_count
        );
    }
}
