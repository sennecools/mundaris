use glam::DVec3;
use mundaris_app::{gravity_fixtures::GravityFixture, planet_terrain::*};
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_world::*;
use std::num::NonZeroU64;

#[test]
fn lighting_changes_do_not_change_cached_terrain_geometry_or_identity() {
    let mut world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(71).unwrap())
        .unwrap();
    let body_id = world.bodies().nth(1).unwrap().0;
    let radius = world
        .body(body_id)
        .unwrap()
        .properties()
        .reference_radius_m();
    world
        .edit_terrain(
            body_id,
            Some(checkpoint_terrain_definition(radius).unwrap()),
        )
        .unwrap();
    let body = world.body(body_id).unwrap();
    let identity = TerrainGeometryIdentity::new(
        body_id,
        body.terrain().unwrap().clone(),
        body.terrain_revision(),
        radius,
    )
    .unwrap();
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 8).unwrap();
    let mut cover = TerrainReadyCover::default();
    let initial = cover
        .update(&mut cache, &identity, 0, 6 * GRID_SAMPLES, None)
        .unwrap();
    assert_eq!(initial.vertices_generated, 6 * GRID_SAMPLES);
    assert!(cover.ready());
    let baseline_addresses: Vec<_> = cover.active().iter().map(|patch| patch.address).collect();
    let baseline: Vec<_> = baseline_addresses
        .iter()
        .map(|&address| cache.peek(&identity, address).unwrap().samples().to_vec())
        .collect();
    let baseline_pointers: Vec<_> = baseline_addresses
        .iter()
        .map(|&address| cache.peek(&identity, address).unwrap().samples().as_ptr())
        .collect();
    let original_identity = identity.clone();

    let lights = [
        TerrainLighting::try_new(DVec3::X, 0.2, 0.8, TerrainRenderMode::Lit).unwrap(),
        TerrainLighting::try_new(DVec3::Z, 0.4, 0.6, TerrainRenderMode::Lit).unwrap(),
        TerrainLighting::try_new(DVec3::Y, 0.1, 0.3, TerrainRenderMode::Lit).unwrap(),
        TerrainLighting::default().with_mode(TerrainRenderMode::Elevation),
        TerrainLighting::default().with_mode(TerrainRenderMode::Normals),
        TerrainLighting::default().with_mode(TerrainRenderMode::Diffuse),
    ];
    assert_eq!(TerrainSunPreset::ALL.len(), 5);
    for lighting in lights {
        // Lighting is a per-frame renderer input and is not part of terrain identity.
        let report = cover
            .update(&mut cache, &identity, 0, 6 * GRID_SAMPLES, None)
            .unwrap();
        assert_eq!(report.vertices_generated, 0);
        assert_eq!(report.patches_completed, 0);
        assert_eq!(identity, original_identity);
        assert_eq!(
            cover
                .active()
                .iter()
                .map(|patch| patch.address)
                .collect::<Vec<_>>(),
            baseline_addresses
        );
        for (index, &address) in baseline_addresses.iter().enumerate() {
            let samples = cache.peek(&identity, address).unwrap().samples();
            assert_eq!(samples.as_ptr(), baseline_pointers[index]);
            assert_eq!(samples, baseline[index]);
        }
        // Exercise the public frame-level lighting route without requiring a GPU.
        let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(71).unwrap()).unwrap();
        let frame_id = frames.frames_for(body_id).unwrap().body_fixed;
        let observer = FramePose::new(
            FramePosition::new(
                frame_id,
                LocalPosition::try_metres(DVec3::Z * (radius + 10_000_000.0)).unwrap(),
            ),
            UnitRotation::identity(),
        );
        let pair = frames.coherent_view(&world).unwrap();
        let view = PreparedView::new(
            &pair.evaluation(),
            observer,
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection =
            CelestialProjection::try_new(640, 480, 60.0_f64.to_radians(), 0.1).unwrap();
        let mut staging = CelestialStaging::default();
        let sphere = Icosphere::new();
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_terrain_lighting(lighting);
        let geometry: Vec<_> = baseline_addresses
            .iter()
            .map(|&address| cache.peek(&identity, address).unwrap())
            .collect();
        frame
            .append_generated_surface(
                CelestialRenderBody {
                    body_fixed_frame: frame_id,
                    reference_radius_m: radius,
                    color: [0.5, 0.6, 0.3, 1.0],
                    unlit: false,
                    selected: false,
                },
                cover.active(),
                &geometry,
                &SurfaceTopology::new(),
                SurfaceStyle {
                    elevation_colors: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(frame.report().surface.samples, 6 * GRID_SAMPLES);
        assert_eq!(
            frame.report().surface.uploaded_bytes,
            6 * (GRID_SAMPLES * 32 + 64)
        );
    }
}
