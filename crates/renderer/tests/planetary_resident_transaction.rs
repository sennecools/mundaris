use glam::{DMat3, DVec3};
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_renderer::{
    RegionalResidentDraw, RegionalTileUpload, TILE_FILTER_VERSION, TILE_FORMAT_VERSION, TileData,
    TileDraw, TileKey, TileSlotState, TileTexel,
};
use std::sync::Arc;

fn draw(level: u8, anchor_view_m: DVec3) -> TileDraw {
    let tile = Arc::new(TileData {
        key: TileKey {
            body_identity: 1,
            definition_words: vec![1],
            radius_bits: 1_737_400.0f64.to_bits(),
            surface_revision: 1,
            material_revision: 1,
            format_version: TILE_FORMAT_VERSION,
            filter_version: TILE_FILTER_VERSION,
            address: CubePatchAddress::try_new(CubeFace::PositiveZ, level, 0, 0).unwrap(),
            cells: 4,
        },
        anchor_radius_m: 1_737_400.0,
        min_max_radial_offset_m: [0.0, 0.0],
        texels: vec![
            TileTexel {
                radial_offset_m: 0.0,
                material: [1.0, 0.0, 0.0, 0.0]
            };
            49
        ],
    });
    let publication = TileSlotState::default().request(&tile.key).unwrap();
    TileDraw {
        tile,
        publication,
        anchor_view_m,
        body_to_view: DMat3::IDENTITY,
        mode: 0,
        sun_body: DVec3::Z,
    }
}

#[test]
fn planetary_orbital_translation_is_explicit_and_local_gate_is_preserved() {
    let orbital = draw(0, DVec3::new(0.01, 0.02, -2_737_400.125));
    assert!(
        orbital
            .validate_view_transform(mundaris_renderer::RenderPrecisionBudget::near_debug())
            .is_err()
    );
    assert!(
        orbital
            .validate_view_transform(orbital.planetary_precision_budget())
            .is_ok()
    );
    let local = draw(14, DVec3::new(0.0, 0.0, -100.0));
    assert_eq!(
        local.planetary_precision_budget().max_component_error_m(),
        0.001
    );
    assert!(
        local
            .validate_view_transform(local.planetary_precision_budget())
            .is_ok()
    );
    let far_local = draw(14, DVec3::new(0.0, 0.0, -1_000_000.013));
    assert!(
        far_local
            .validate_view_transform(mundaris_renderer::RenderPrecisionBudget::near_debug())
            .is_err()
    );
    assert!(
        far_local
            .validate_view_transform(far_local.planetary_precision_budget())
            .is_ok()
    );
    let retreat = draw(14, DVec3::new(0.0, 0.0, -100_000.013));
    let error = (retreat.anchor_view_m.z - f64::from(retreat.anchor_view_m.z as f32)).abs();
    assert!(error > 0.001);
    assert!(error <= retreat.planetary_precision_budget().max_component_error_m());
    assert!(
        retreat
            .validate_view_transform(retreat.planetary_precision_budget())
            .is_ok()
    );
    let near_limit = draw(14, DVec3::new(0.0, 0.0, -10_000.0));
    assert_eq!(
        near_limit
            .planetary_precision_budget()
            .max_component_error_m(),
        0.001
    );
}

#[test]
fn upload_only_bootstrap_retains_empty_draw_coverage() {
    let transaction = RegionalResidentDraw {
        planetary: true,
        capacity: 6,
        cells: 4,
        patches: vec![],
        uploads: vec![RegionalTileUpload {
            slot: 0,
            tile: draw(0, DVec3::ZERO),
        }],
    };
    assert!(transaction.validate().is_ok());
    let mut invalid = transaction;
    invalid.uploads[0].slot = 6;
    assert!(invalid.validate().is_err());
}
