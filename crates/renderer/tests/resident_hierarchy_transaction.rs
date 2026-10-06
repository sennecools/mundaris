#![cfg(feature = "terrain-capture")]

use glam::{DMat3, DVec3};
use mundaris_math::{
    surface::{CubeFace, CubePatchAddress},
    *,
};
use mundaris_renderer::{
    CelestialFrame, CelestialProjection, CelestialStaging, Icosphere, PreparedView,
    RenderPrecisionBudget, ResidentHierarchyDraw, TILE_FILTER_VERSION, TILE_FORMAT_VERSION,
    TileData, TileDraw, TileKey, TileSlotState, TileTexel, terrain_capture::TerrainCaptureRenderer,
};
use std::{num::NonZeroU64, sync::Arc};

fn position(frame: FrameId, value: DVec3) -> FramePosition {
    FramePosition::new(frame, LocalPosition::try_metres(value).unwrap())
}

fn tile(radius: f64, revision: u64, address: CubePatchAddress, invalid: bool) -> TileData {
    const CELLS: u32 = 8;
    let side = CELLS + 3;
    let mut texels = vec![
        TileTexel {
            radial_offset_m: 0.0,
            material: [0.1, 0.2, 0.3, 0.4],
        };
        (side * side) as usize
    ];
    if invalid {
        texels.last_mut().unwrap().material[0] = f32::NAN;
    }
    TileData {
        key: TileKey {
            body_identity: 91,
            definition_words: vec![4, 9, 1],
            radius_bits: radius.to_bits(),
            surface_revision: revision,
            material_revision: revision,
            format_version: TILE_FORMAT_VERSION,
            filter_version: TILE_FILTER_VERSION,
            address,
            cells: CELLS,
        },
        anchor_radius_m: radius,
        min_max_radial_offset_m: [0.0, 0.0],
        texels,
    }
}

fn hierarchy_tiles(radius: f64, revision: u64, invalid_child: Option<usize>) -> [Arc<TileData>; 5] {
    let parent_address =
        CubePatchAddress::try_new(CubeFace::PositiveZ, 15, 16_383, 16_384).unwrap();
    let children = parent_address.children().unwrap();
    std::array::from_fn(|slot| {
        Arc::new(if slot == 0 {
            tile(radius, revision, parent_address, false)
        } else {
            tile(
                radius,
                revision,
                children[slot - 1],
                invalid_child == Some(slot - 1),
            )
        })
    })
}

fn hierarchy_draw(
    tiles: &[Arc<TileData>; 5],
    tokens: &[mundaris_renderer::TilePublicationToken; 5],
    parent_anchor_view: DVec3,
) -> ResidentHierarchyDraw {
    let parent_anchor = tiles[0].anchor_position_body().unwrap();
    let body_to_view = DMat3::IDENTITY;
    let draws: [TileDraw; 5] = std::array::from_fn(|index| TileDraw {
        tile: Arc::clone(&tiles[index]),
        publication: tokens[index].clone(),
        anchor_view_m: parent_anchor_view
            + body_to_view * (tiles[index].anchor_position_body().unwrap() - parent_anchor),
        body_to_view,
        mode: 2,
        sun_body: DVec3::Z,
    });
    ResidentHierarchyDraw {
        parent: draws[0].clone(),
        children: std::array::from_fn(|index| Some(draws[index + 1].clone())),
        draw_children: true,
        morph_fraction: 0.5,
    }
}

fn render_hierarchy(
    renderer: &mut TerrainCaptureRenderer,
    view: &PreparedView,
    staging: &mut CelestialStaging,
    projection: CelestialProjection,
    sphere: &Icosphere,
    draw: ResidentHierarchyDraw,
) -> Result<(), mundaris_renderer::RenderPreparationError> {
    let mut frame = CelestialFrame::new(view, staging, projection, sphere);
    frame.set_resident_hierarchy(draw)?;
    renderer.render(&frame).map(|_| ())
}

#[test]
#[ignore = "requires a real GPU adapter; verifies failed hierarchy publication is transactional"]
fn invalid_late_child_does_not_publish_any_slot_or_invalidate_previous_tokens() {
    let radius = 6_371_000.0;
    let mut renderer = TerrainCaptureRenderer::new(32, 32)
        .expect("GPU adapter required; this regression gate must not silently skip");
    let staging = &mut CelestialStaging::default();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(32, 32, 60.0_f64.to_radians(), 0.1).unwrap();
    let tree = FrameTree::new(NonZeroU64::new(991).unwrap());
    let body = tree.root();
    let observer = FramePose::new(
        position(body, DVec3::new(0.0, 0.0, radius + 40.0)),
        UnitRotation::identity(),
    );
    let evaluation = tree.evaluate();
    let view =
        PreparedView::new(&evaluation, observer, RenderPrecisionBudget::near_debug()).unwrap();
    let source = view.prepare_source(body).unwrap();
    let first_tiles = hierarchy_tiles(radius, 1, None);
    let parent_anchor = first_tiles[0].anchor_position_body().unwrap();
    let parent_anchor_view = source
        .view_displacement(position(body, parent_anchor))
        .unwrap()
        .metres();
    let mut slots: [TileSlotState; 5] = std::array::from_fn(|_| TileSlotState::default());
    let first_tokens: [mundaris_renderer::TilePublicationToken; 5] =
        std::array::from_fn(|index| slots[index].request(&first_tiles[index].key).unwrap());
    let first = hierarchy_draw(&first_tiles, &first_tokens, parent_anchor_view);
    first.validate().expect("valid initial hierarchy fixture");
    render_hierarchy(
        &mut renderer,
        &view,
        staging,
        projection,
        &sphere,
        first.clone(),
    )
    .unwrap();
    let accepted = renderer.last_resident_hierarchy_report();
    assert_eq!(
        accepted.resident_keys,
        std::array::from_fn(|i| Some(first_tiles[i].key.clone()))
    );
    assert_eq!(
        accepted.slot_generations,
        std::array::from_fn(|i| first_tokens[i].generation())
    );

    let invalid_tiles = hierarchy_tiles(radius, 2, Some(3));
    let invalid_tokens: [mundaris_renderer::TilePublicationToken; 5] =
        std::array::from_fn(|index| slots[index].request(&invalid_tiles[index].key).unwrap());
    let invalid = hierarchy_draw(&invalid_tiles, &invalid_tokens, parent_anchor_view);
    assert!(
        render_hierarchy(&mut renderer, &view, staging, projection, &sphere, invalid,).is_err()
    );
    let after_rejection = renderer.last_resident_hierarchy_report();
    assert_eq!(after_rejection.resident_keys, accepted.resident_keys);
    assert_eq!(after_rejection.slot_generations, accepted.slot_generations);

    render_hierarchy(
        &mut renderer,
        &view,
        staging,
        projection,
        &sphere,
        first.clone(),
    )
    .expect("failed candidate must not invalidate the previous accepted tokens");
    let recovered = renderer.last_resident_hierarchy_report();
    assert_eq!(recovered.resident_keys, accepted.resident_keys);
    assert_eq!(recovered.slot_generations, accepted.slot_generations);
    assert_eq!(recovered.resources.tile_content_upload_count, 0);
    assert_eq!(recovered.slot_content_upload_bytes, [0; 5]);

    let invalid_single_tile = Arc::new(tile(radius, 3, first_tiles[0].key.address, true));
    let invalid_single_token = slots[0].request(&invalid_single_tile.key).unwrap();
    let invalid_single = TileDraw {
        tile: invalid_single_tile,
        publication: invalid_single_token,
        anchor_view_m: parent_anchor_view,
        body_to_view: DMat3::IDENTITY,
        mode: 2,
        sun_body: DVec3::Z,
    };
    let mut frame = CelestialFrame::new(&view, staging, projection, &sphere);
    frame.set_resident_tile(invalid_single).unwrap();
    assert!(renderer.render(&frame).is_err());

    render_hierarchy(&mut renderer, &view, staging, projection, &sphere, first)
        .expect("invalid single-tile payload must not invalidate the hierarchy token");
    let single_recovered = renderer.last_resident_hierarchy_report();
    assert_eq!(single_recovered.resident_keys, accepted.resident_keys);
    assert_eq!(single_recovered.slot_generations, accepted.slot_generations);
    assert_eq!(single_recovered.resources.tile_content_upload_count, 0);
}
