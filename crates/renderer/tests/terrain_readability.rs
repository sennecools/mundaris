use mundaris_renderer::planet_surface::{TerrainReadability, TerrainRenderMode, lod_color};

#[test]
fn readability_thresholds_require_ordered_finite_ranges() {
    assert!(TerrainReadability::try_new(0.0, 300.0, 1800.0, 3000.0, 5000.0, 12.0, 35.0).is_ok());
    assert!(TerrainReadability::try_new(0.0, 0.0, 1800.0, 3000.0, 5000.0, 12.0, 35.0).is_err());
    assert!(TerrainReadability::try_new(0.0, 300.0, 1800.0, 1700.0, 5000.0, 12.0, 35.0).is_err());
    assert!(TerrainReadability::try_new(0.0, 300.0, 1800.0, 3000.0, 5000.0, 35.0, 12.0).is_err());
    assert!(
        TerrainReadability::try_new(f64::NAN, 300.0, 1800.0, 3000.0, 5000.0, 12.0, 35.0).is_err()
    );
}

#[test]
fn modes_keep_legacy_values_and_lod_palette_cycles_twelve_hues() {
    assert_eq!(TerrainRenderMode::Elevation as u32, 0);
    assert_eq!(TerrainRenderMode::Lit as u32, 1);
    assert_eq!(TerrainRenderMode::Normals as u32, 2);
    assert_eq!(TerrainRenderMode::Diffuse as u32, 3);
    let colors: Vec<_> = (0..12).map(lod_color).collect();
    for i in 0..colors.len() {
        assert!(!colors[..i].contains(&colors[i]));
    }
    assert_eq!(lod_color(12), lod_color(0));
}
