use mundaris_app::planet_terrain::terrain_surface_certificate;
use mundaris_math::surface::*;
use mundaris_renderer::planet_surface::*;
use mundaris_world::terrain::*;

#[test]
fn coarse_landmarks_charge_interpolation_and_refine_without_fading() {
    let radius = 100_000.0;
    let flat = TerrainBandConfig::new(
        0.0,
        TerrainScale::Metres {
            longest_wavelength_m: 256.0,
        },
        1,
    )
    .unwrap();
    let config = TerrainConfig::new(
        [flat; 5],
        TerrainControls::new(0.0, 1.0, 0.0, 0.0, 0.0, 0.4).unwrap(),
    )
    .unwrap()
    .with_crater_field(CraterFieldConfig::new(1, 10_000.0, 10_000.0, 0.085, 0.04).unwrap())
    .unwrap();
    let definition = TerrainDefinition::new(
        TerrainIdentity(8),
        TerrainSeed(2),
        TerrainGeneratorVersion::CrateredV1,
        config,
    );
    let generator = TerrainGenerator::new(&definition, radius).unwrap();
    let feature = generator.crater_features()[0];
    let location = SurfaceLocation::new(feature.center());
    let (face, uv) = location.face_uv();
    let topology = SurfaceTopology::new();
    let mut coarse_error = None;
    for level in [0, 12] {
        let count = 1u32 << level;
        let coordinate =
            |v: f64| (((v + 1.0) * 0.5 * f64::from(count)).floor() as u32).min(count - 1);
        let address =
            CubePatchAddress::try_new(face, level, coordinate(uv[0]), coordinate(uv[1])).unwrap();
        let metadata = PatchMetadata::build(address, &topology).unwrap();
        let (extent, error) = terrain_surface_certificate(&generator, address, metadata).unwrap();
        let footprint = TerrainFootprint::new(radius * 2.0 / (16.0 * f64::from(count))).unwrap();
        let sample = generator
            .evaluate_point(TerrainQuery {
                location,
                footprint,
            })
            .unwrap();
        assert!((sample.height_m() + feature.depth_m()).abs() < 1e-9);
        assert!(
            extent.min_height_m <= sample.height_m() && sample.height_m() <= extent.max_height_m
        );
        assert_eq!(error.unresolved_m, 0.0);
        if level == 0 {
            assert!(error.filtered_interpolation_m > feature.depth_m());
            coarse_error = Some(error.total_m().unwrap());
        } else {
            assert!(error.total_m().unwrap() < coarse_error.unwrap() * 1e-4);
        }
    }
}
