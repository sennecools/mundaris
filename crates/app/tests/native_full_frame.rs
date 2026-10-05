//! Opt-in native readback across the production celestial frame's layered passes.
#![cfg(feature = "terrain-capture")]

use glam::DVec3;
use mundaris_math::{surface::*, *};
use mundaris_renderer::terrain_capture::TerrainCaptureRenderer;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_world::terrain::AnalyticTerrain;
use std::num::NonZeroU64;

#[test]
#[ignore = "requires a native graphics adapter and readback"]
fn full_celestial_frame_layers_and_overlay_handoff() {
    let radius = 400_000.0;
    let field = AnalyticTerrain::constant(radius, 350.0).unwrap();
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
                    min_height_m: 350.0,
                    max_height_m: 350.0,
                    guaranteed_opaque_radius_m: 0.0,
                },
                SurfaceErrorContributions::default(),
            )
            .unwrap()
        })
        .collect();
    let geometry_refs: Vec<_> = geometry.iter().collect();
    let surface = StitchedSurface::build(&active, &geometry_refs, &topology).unwrap();
    let destination_geometry: Vec<_> = geometry
        .iter()
        .map(|patch| {
            GeneratedSurfacePatch::new(
                patch.address(),
                radius,
                0.0,
                patch
                    .samples()
                    .iter()
                    .map(|sample| SurfaceGeometrySample {
                        position_body_m: sample.position_body_m.normalize() * (radius + 360.0),
                        normal_body: sample.normal_body,
                    })
                    .collect(),
                SurfaceExtent {
                    min_height_m: 360.0,
                    max_height_m: 360.0,
                    guaranteed_opaque_radius_m: 0.0,
                },
                SurfaceErrorContributions::default(),
            )
            .unwrap()
        })
        .collect();
    let destination_refs: Vec<_> = destination_geometry.iter().collect();
    let destination = StitchedSurface::build(&active, &destination_refs, &topology).unwrap();
    let transition = SurfaceTransition::build(
        &active,
        &surface,
        &active,
        &destination,
        &topology,
        16 * 1024 * 1024,
    )
    .unwrap();
    assert!(!transition.triangles().is_empty());

    let mut tree = FrameTree::new(NonZeroU64::new(9511).unwrap());
    let terrain_frame = tree
        .insert(
            tree.root(),
            FrameState::stationary(RigidTransform::identity()),
        )
        .unwrap();
    let far_transform = RigidTransform::new(
        Displacement3::try_metres(DVec3::new(650_000.0, 0.0, -2_000_000.0)).unwrap(),
        UnitRotation::identity(),
    );
    let far_frame = tree
        .insert(tree.root(), FrameState::stationary(far_transform))
        .unwrap();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(192, 144, 60_f64.to_radians(), 0.1).unwrap();
    let observer = FramePose::new(
        FramePosition::new(
            terrain_frame,
            LocalPosition::try_metres(DVec3::Z * 800_000.0).unwrap(),
        ),
        UnitRotation::identity(),
    );
    let terrain_body = CelestialRenderBody {
        body_fixed_frame: terrain_frame,
        reference_radius_m: radius,
        color: [0.45, 0.6, 0.35, 1.0],
        unlit: false,
        selected: false,
    };
    let far_body = CelestialRenderBody {
        body_fixed_frame: far_frame,
        reference_radius_m: 180_000.0,
        color: [0.85, 0.72, 0.48, 1.0],
        unlit: false,
        selected: false,
    };
    let mut capture = TerrainCaptureRenderer::new(192, 144).unwrap();
    let configs = [
        ("all", PlanetaryConfig::default()),
        (
            "ocean off",
            PlanetaryConfig {
                ocean_enabled: false,
                ..Default::default()
            },
        ),
        (
            "clouds off",
            PlanetaryConfig {
                clouds_enabled: false,
                ..Default::default()
            },
        ),
        (
            "atmosphere off",
            PlanetaryConfig {
                atmosphere_enabled: false,
                ..Default::default()
            },
        ),
        ("diagnostic", PlanetaryConfig::default()),
        ("far", PlanetaryConfig::default()),
        ("transition", PlanetaryConfig::default()),
        ("repeat", PlanetaryConfig::default()),
    ];
    let mut baseline = None;
    for (name, config) in configs {
        let evaluation = tree.evaluate();
        let view =
            PreparedView::new(&evaluation, observer, RenderPrecisionBudget::near_debug()).unwrap();
        let mut staging = CelestialStaging::default();
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        let lighting = TerrainLighting::default().with_mode(if name == "diagnostic" {
            TerrainRenderMode::Elevation
        } else {
            TerrainRenderMode::Natural
        });
        frame.set_terrain_lighting(lighting);
        if name != "far" {
            frame
                .set_planetary_environment(terrain_body, config)
                .unwrap();
            if name == "transition" {
                frame
                    .append_surface_transition(
                        terrain_body,
                        &transition,
                        0.5,
                        SurfaceStyle::default(),
                    )
                    .unwrap();
            } else {
                frame
                    .append_stitched_surface(
                        terrain_body,
                        &active,
                        &surface,
                        &topology,
                        SurfaceStyle::default(),
                    )
                    .unwrap();
            }
        }
        frame
            .append_body_observations(&[terrain_body, far_body], &[name != "far", false])
            .unwrap();
        assert_eq!(
            frame.markers()[0].representation,
            if name == "far" {
                SphereRepresentation::PhysicalSphere
            } else {
                SphereRepresentation::Surface
            }
        );
        let trail = [DebugLine {
            endpoints: [
                FramePosition::new(
                    terrain_frame,
                    LocalPosition::try_metres(DVec3::new(-300_000.0, -20_000.0, 350_000.0))
                        .unwrap(),
                ),
                FramePosition::new(
                    terrain_frame,
                    LocalPosition::try_metres(DVec3::new(300_000.0, -20_000.0, 350_000.0)).unwrap(),
                ),
            ],
            color: [1.0, 0.25, 0.1, 1.0],
        }];
        frame
            .append_historical_lines(terrain_frame, &trail)
            .unwrap();
        let points = [
            FramePosition::new(
                terrain_frame,
                LocalPosition::try_metres(DVec3::new(-320_000.0, 55_000.0, 350_000.0)).unwrap(),
            ),
            FramePosition::new(
                terrain_frame,
                LocalPosition::try_metres(DVec3::new(0.0, 90_000.0, 350_000.0)).unwrap(),
            ),
            FramePosition::new(
                terrain_frame,
                LocalPosition::try_metres(DVec3::new(320_000.0, 55_000.0, 350_000.0)).unwrap(),
            ),
        ];
        frame
            .append_polylines(&[CelestialPolyline {
                points: &points,
                colors: &[[0.1, 0.9, 1.0, 1.0]; 3],
                width_pixels: 3.0,
                style: CelestialLineStyle::Dashed,
            }])
            .unwrap();
        let report = frame.report();
        if name == "transition" {
            assert!(
                report.surface.morph_triangles > 0,
                "morph fallback draw not prepared"
            );
        } else if name == "far" {
            assert_eq!(report.surface.patches, 0);
            assert_eq!(
                report.triangles, 2560,
                "both bodies must use far sphere draws"
            );
        } else {
            assert!(
                report.surface.patches > 0,
                "{name}: generated terrain not prepared"
            );
        }
        assert_eq!(
            report.trail_segments, 1,
            "{name}: historical line not prepared"
        );
        assert_eq!(
            report.polyline_segments, 2,
            "{name}: styled curve not prepared"
        );
        assert!(
            report.triangles >= 1280,
            "{name}: far body must use sphere path"
        );
        let layers = name != "far" && name != "diagnostic";
        assert_eq!(
            (
                report.planetary_ocean_draws,
                report.planetary_cloud_draws,
                report.planetary_atmosphere_draws
            ),
            (
                usize::from(layers && config.ocean_enabled),
                usize::from(layers && config.clouds_enabled),
                usize::from(layers && config.atmosphere_enabled)
            )
        );
        let pixels = capture.render(&frame).unwrap();
        assert_eq!(pixels.len(), 192 * 144 * 4);
        let changed = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| **p != [5, 7, 12, 255])
            .count();
        assert!(changed > 100, "{name}: unexpectedly blank capture");
        if name == "all" {
            baseline = Some(pixels.clone());
        }
        if name == "repeat" {
            assert_eq!(
                &pixels,
                baseline.as_ref().unwrap(),
                "repeat capture should be deterministic"
            );
        }
    }
}
