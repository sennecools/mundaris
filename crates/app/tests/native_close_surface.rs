//! Opt-in native GPU regression: backface culling under displaced terrain is not
//! confused with missing ready cover, near clipping, or invalid source conversion.
#![cfg(feature = "terrain-capture")]

use glam::DVec3;
use mundaris_math::{surface::*, *};
use mundaris_renderer::terrain_capture::TerrainCaptureRenderer;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_world::terrain::AnalyticTerrain;
use std::num::NonZeroU64;

#[test]
#[ignore = "requires a native graphics adapter and readback"]
fn displaced_shell_inside_backfaces_and_outside_regression() {
    let radius = 400_000.0;
    let height = 500.0;
    let field = AnalyticTerrain::constant(radius, height).unwrap();
    let topology = SurfaceTopology::new();
    let addresses: Vec<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    let active = active_surface_cover(&addresses, &topology).unwrap();
    let geometry: Vec<_> = addresses
        .iter()
        .map(|&address| {
            let samples = (0..GRID_SAMPLES)
                .map(|i| {
                    let location = SurfaceLocation::new(
                        address
                            .sample_direction(i as u32 % 17, i as u32 / 17, 16)
                            .unwrap(),
                    );
                    let sample = field.evaluate_point(mundaris_world::terrain::TerrainQuery {
                        location,
                        footprint: mundaris_world::terrain::TerrainFootprint::COMPLETE,
                    });
                    SurfaceGeometrySample {
                        position_body_m: location.direction().unit() * (radius + sample.height_m()),
                        normal_body: sample.normal_body(location, radius).unwrap().unit(),
                    }
                })
                .collect();
            GeneratedSurfacePatch::new(
                address,
                radius,
                0.0,
                samples,
                SurfaceExtent {
                    min_height_m: height,
                    max_height_m: height,
                    guaranteed_opaque_radius_m: 0.0,
                },
                SurfaceErrorContributions::default(),
            )
            .unwrap()
        })
        .collect();
    let refs: Vec<_> = geometry.iter().collect();
    let surface = StitchedSurface::build(&active, &refs, &topology).unwrap();
    let mut tree = FrameTree::new(NonZeroU64::new(5901).unwrap());
    let frame = tree
        .insert(
            tree.root(),
            FrameState::stationary(RigidTransform::identity()),
        )
        .unwrap();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(160, 120, 60_f64.to_radians(), 0.1).unwrap();
    let mut capture = TerrainCaptureRenderer::new(160, 120).unwrap();
    let background = {
        let evaluation = tree.evaluate();
        let pose = FramePose::new(
            FramePosition::new(frame, LocalPosition::origin()),
            UnitRotation::identity(),
        );
        let view =
            PreparedView::new(&evaluation, pose, RenderPrecisionBudget::near_debug()).unwrap();
        let mut staging = CelestialStaging::default();
        let empty = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        capture.render(&empty).unwrap()
    };
    let body = CelestialRenderBody {
        body_fixed_frame: frame,
        reference_radius_m: radius,
        color: [0.4, 0.6, 0.3, 1.0],
        unlit: false,
        selected: false,
    };
    let mut counts = Vec::new();
    for (clearance, underside) in [(-10.0, false), (-10.0, true), (10.0, false)] {
        let pose = FramePose::new(
            FramePosition::new(
                frame,
                LocalPosition::try_metres(DVec3::Z * (radius + height + clearance)).unwrap(),
            ),
            UnitRotation::identity(),
        );
        let evaluation = tree.evaluate();
        let view =
            PreparedView::new(&evaluation, pose, RenderPrecisionBudget::near_debug()).unwrap();
        let mut staging = CelestialStaging::default();
        let mut rendered = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        rendered.set_terrain_lighting(
            TerrainLighting::try_new(DVec3::Z, 0.3, 0.7, TerrainRenderMode::Readability).unwrap(),
        );
        rendered
            .append_stitched_surface(
                body,
                &active,
                &surface,
                &topology,
                SurfaceStyle {
                    underside,
                    ..Default::default()
                },
            )
            .unwrap();
        let report = rendered.report();
        assert!(report.surface.patches > 0);
        assert!(report.surface.max_projected_error_pixels.is_finite());
        let pixels = capture.render(&rendered).unwrap();
        counts.push(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .zip(background.as_chunks::<4>().0.iter())
                .filter(|(p, b)| p != b)
                .count(),
        );
    }
    println!(
        "native displaced shell pixels: inside culled={}, inside no-cull={}, outside={}",
        counts[0], counts[1], counts[2]
    );
    assert!(
        counts[1] > counts[0] + 100,
        "no-cull must expose the inside-facing surface"
    );
    assert!(
        counts[2] > counts[0] + 100,
        "outward camera must retain normal surface visibility"
    );
}
