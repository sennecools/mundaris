#![cfg(feature = "terrain-capture")]
use glam::{DQuat, DVec3};
use mundaris_math::*;
use mundaris_renderer::{
    CelestialFrame, CelestialProjection, CelestialRenderBody, CelestialStaging, Icosphere,
    PreparedView, RenderPrecisionBudget,
    sky::{
        SkyBackground, SkyBranch, SkyComplex, SkyDefinition, SkyIdentity, SkyMorphology,
        SkySettings, SkyStar, inspect_background_mips, sample_background,
    },
    terrain_capture::TerrainCaptureRenderer,
};
use std::{num::NonZeroU64, sync::Arc};

fn linear(byte: u8) -> f64 {
    let encoded = f64::from(byte) / 255.0;
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

#[test]
#[ignore = "requires a real GPU adapter; focal chart filtering, seam/pole and residency"]
fn focal_chart_pixels_reuse_and_return_at_seam_and_pole() {
    let tree = FrameTree::new(NonZeroU64::new(513_133).unwrap());
    let sphere = Icosphere::new();
    let mut staging = CelestialStaging::default();
    let projection = CelestialProjection::try_new(257, 257, 60_f64.to_radians(), 0.1).unwrap();
    let mut renderer =
        TerrainCaptureRenderer::new(257, 257).expect("adapter required, never silently skipped");
    let mut maximum_filtered_error = 0.0_f64;
    for (index, forward) in [-DVec3::X, DVec3::Y, -DVec3::Z].into_iter().enumerate() {
        let complex = SkyComplex {
            direction: forward,
            half_extent_rad: 0.24,
            roll_rad: 0.3,
            emission: [0.9, 0.17, 0.2],
            reflection: [0.1, 0.4, 1.0],
            luminosity: 0.8,
            light_position: [0.1, 0.3],
            clouds: vec![SkyBranch {
                start: [-0.6, -0.15],
                end: [0.6, 0.15],
                widths: [0.25, 0.14],
                density: 1.0,
            }],
            dust: vec![],
            cavities: vec![],
        };
        let definition = Arc::new(
            SkyDefinition::try_new(
                SkyIdentity {
                    preset: "focal-gpu-fixture",
                    version: 3,
                    seed: 42,
                },
                DVec3::ZERO,
                DQuat::IDENTITY,
                vec![],
                SkyBackground {
                    width: 128,
                    height: 64,
                    brightness: 0.25,
                    ..Default::default()
                },
            )
            .unwrap()
            .with_morphology(SkyMorphology {
                complexes: vec![complex],
                disk_regions: vec![],
                detail_size: 256,
            })
            .unwrap(),
        );
        let reference = if forward.y.abs() > 0.9 {
            DVec3::Z
        } else {
            DVec3::Y
        };
        let right = forward.cross(reference).normalize();
        let up = right.cross(forward);
        let q = DQuat::from_mat3(&glam::DMat3::from_cols(right, up, -forward));
        let pose = FramePose::new(
            FramePosition::new(tree.root(), LocalPosition::origin()),
            UnitRotation::try_from_quaternion(q).unwrap(),
        );
        let view =
            PreparedView::new(&tree.evaluate(), pose, RenderPrecisionBudget::near_debug()).unwrap();
        let mut original = None;
        for iteration in 0..3 {
            let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
            frame
                .set_distant_sky(
                    tree.root(),
                    Arc::clone(&definition),
                    SkySettings {
                        enabled: iteration != 1,
                        ..Default::default()
                    },
                )
                .unwrap();
            let image = renderer.render(&frame).unwrap();
            if iteration == 0 {
                let raw = sample_background(&definition, forward).unwrap();
                let cached = inspect_background_mips(&definition);
                // Raw field points are not an oracle for a minified texture.
                // Reproduce centred bilinear/trilinear cached-mip filtering at
                // this fixture's known angular footprint instead.
                let lod = (256.0 / (2.0 * 0.24_f64.tan() * projection.focal_pixels()))
                    .log2()
                    .max(0.0);
                let levels = &cached[1];
                let low = lod.floor() as usize;
                let high = (low + 1).min(levels.len() - 1);
                let center_sample = |level: usize, channel: usize| {
                    let mip = &levels[level];
                    let x = mip.width / 2;
                    let y = mip.height / 2;
                    let mut sum = 0.0;
                    for py in [y.saturating_sub(1), y.min(mip.height - 1)] {
                        for px in [x.saturating_sub(1), x.min(mip.width - 1)] {
                            sum += linear(mip.rgba[((py * mip.width + px) * 4) as usize + channel])
                                * 0.25;
                        }
                    }
                    sum
                };
                let center = (128 * 257 + 128) * 4;
                for channel in 0..3 {
                    let predicted = center_sample(low, channel) * (1.0 - lod.fract())
                        + center_sample(high, channel) * lod.fract();
                    maximum_filtered_error = maximum_filtered_error
                        .max((linear(image[center + channel]) - predicted).abs());
                    assert!(
                        (linear(image[center + channel]) - predicted).abs() < 0.012,
                        "focal sample {index}, channel {channel}: {} vs filtered {predicted}; raw {}",
                        linear(image[center + channel]),
                        raw[channel]
                    );
                }
                original = Some(image);
            } else if iteration == 2 {
                assert_eq!(original.as_ref().unwrap(), &image);
            }
            let resources = renderer.last_sky_resource_report();
            assert_eq!(resources.background_upload_count, index as u64 + 1);
            if iteration > 0 {
                assert_eq!(resources.static_upload_bytes, 0);
            }
            assert_eq!(
                resources.frame_upload_bytes,
                if iteration == 1 { 0 } else { 112 }
            );
        }
    }
    println!(
        "GPU focal cached-mip maximum linear-channel error: {maximum_filtered_error:.9}; 257x257/60deg, seam/pole/-Z, 256px focal chart"
    );
}

#[test]
#[ignore = "requires a real GPU adapter; explicitly selected Phase 5.13C coverage"]
fn rendered_centers_cache_toggle_occlusion_and_invalid_state() {
    let (width, height) = (960, 640);
    let projection = CelestialProjection::try_new(width, height, 60_f64.to_radians(), 0.1).unwrap();
    let direction = projection.unproject_ray([551.23, 281.37]).unwrap();
    let position = direction * 1.1e18;
    let definition = Arc::new(
        SkyDefinition::try_new(
            SkyIdentity {
                preset: "gpu-precision-fixture",
                version: 1,
                seed: 51313,
            },
            DVec3::ZERO,
            DQuat::IDENTITY,
            vec![SkyStar {
                position_m: position,
                color: [1.0; 3],
                flux: 4.0,
                radius_pixels: 1.4,
            }],
            SkyBackground {
                width: 64,
                height: 32,
                ..Default::default()
            },
        )
        .unwrap(),
    );
    let mut tree = FrameTree::new(NonZeroU64::new(513130).unwrap());
    let root = tree.root();
    let body_frame = tree
        .insert(
            root,
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(direction * 1000.0).unwrap(),
                UnitRotation::identity(),
            )),
        )
        .unwrap();
    let body = CelestialRenderBody {
        body_fixed_frame: body_frame,
        reference_radius_m: 100.0,
        color: [0.1, 0.3, 0.5, 1.0],
        unlit: true,
        selected: false,
        material: Default::default(),
        emission_nits: 0.0,
    };
    let sphere = Icosphere::new();
    let mut staging = CelestialStaging::default();
    let mut renderer = TerrainCaptureRenderer::new(width, height)
        .expect("adapter required, never silently skipped");
    let settings = SkySettings {
        background_intensity: 0.0,
        halo_strength: 0.0,
        ..Default::default()
    };
    let mut first_image = None;
    let mut maximum_error = 0.0_f64;
    for (iteration, observer) in [
        DVec3::ZERO,
        DVec3::X * 1.495978707e11,
        DVec3::X * 9.0e13,
        DVec3::ZERO,
    ]
    .into_iter()
    .enumerate()
    {
        let pose = FramePose::new(
            FramePosition::new(root, LocalPosition::try_metres(observer).unwrap()),
            UnitRotation::identity(),
        );
        let view =
            PreparedView::new(&tree.evaluate(), pose, RenderPrecisionBudget::near_debug()).unwrap();
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame
            .set_distant_sky(
                root,
                Arc::clone(&definition),
                SkySettings {
                    enabled: false,
                    ..settings
                },
            )
            .unwrap();
        let off = renderer.render(&frame).unwrap();
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame
            .set_distant_sky(root, Arc::clone(&definition), settings)
            .unwrap();
        let on = renderer.render(&frame).unwrap();
        let relative = position - observer;
        let focal = f64::from(height) / (2.0 * (60_f64.to_radians() * 0.5).tan());
        let oracle = [
            f64::from(width) * 0.5 + focal * relative.x / -relative.z,
            f64::from(height) * 0.5 - focal * relative.y / -relative.z,
        ];
        let mut weight = 0.0;
        let mut moments = [0.0; 2];
        for y in (oracle[1] as i32 - 9)..=(oracle[1] as i32 + 9) {
            for x in (oracle[0] as i32 - 9)..=(oracle[0] as i32 + 9) {
                let index = (y as usize * width as usize + x as usize) * 4;
                let value = (linear(on[index]) - linear(off[index])).max(0.0);
                weight += value;
                moments[0] += (f64::from(x) + 0.5) * value;
                moments[1] += (f64::from(y) + 0.5) * value;
            }
        }
        assert!(
            weight > 3.5,
            "star must actually draw after re-enabling: {weight}"
        );
        let center = [moments[0] / weight, moments[1] / weight];
        let error = (center[0] - oracle[0]).hypot(center[1] - oracle[1]);
        maximum_error = maximum_error.max(error);
        assert!(error <= 0.05, "rendered centroid error {error} physical px");
        let resources = renderer.last_sky_resource_report();
        assert_eq!(resources.catalogue_upload_count, 1);
        assert_eq!(resources.background_upload_count, 1);
        assert_eq!(resources.frame_upload_bytes, 112);
        if iteration > 0 {
            assert_eq!(resources.static_upload_bytes, 0);
        }
        if iteration == 0 {
            first_image = Some(on);
        } else if iteration == 3 {
            assert_eq!(first_image.as_ref().unwrap(), &on);
        }
    }
    println!(
        "GPU rendered centroid maximum error: {maximum_error:.9} physical pixels; 960x640, 60 deg, observer 0/1 AU/9e13 m"
    );

    let view = PreparedView::new(
        &tree.evaluate(),
        FramePose::new(
            FramePosition::new(root, LocalPosition::origin()),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
    frame
        .set_distant_sky(root, Arc::clone(&definition), settings)
        .unwrap();
    frame.append_bodies(&[body]).unwrap();
    let occluded_on = renderer.render(&frame).unwrap();
    let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
    frame
        .set_distant_sky(
            root,
            Arc::clone(&definition),
            SkySettings {
                enabled: false,
                ..settings
            },
        )
        .unwrap();
    frame.append_bodies(&[body]).unwrap();
    let occluded_off = renderer.render(&frame).unwrap();
    for y in 273..290 {
        for x in 543..560 {
            let i = (y * width as usize + x) * 4;
            assert_eq!(
                &occluded_on[i..i + 4],
                &occluded_off[i..i + 4],
                "opaque geometry must occlude distant content"
            );
        }
    }
    let before = renderer.last_sky_resource_report();
    let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
    assert!(
        frame
            .set_distant_sky(
                root,
                definition,
                SkySettings {
                    intensity: f32::NAN,
                    ..settings
                }
            )
            .is_err()
    );
    assert!(renderer.render(&frame).is_err());
    assert_eq!(
        before,
        renderer.last_sky_resource_report(),
        "invalid frame cannot upload partial state"
    );
}
