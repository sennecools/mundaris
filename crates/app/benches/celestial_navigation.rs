mod common;
use criterion::{Criterion, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_app::{
    celestial_camera::*, celestial_labels::*, celestial_selection::*, orbit_guides::*,
    system_view::*,
};
use mundaris_renderer::*;
use mundaris_world::*;
use std::{hint::black_box, num::NonZeroU64, time::Duration};
fn benches(c: &mut Criterion) {
    let projection = CelestialProjection::try_new(960, 662, 60.0_f64.to_radians(), 0.1)
        .unwrap()
        .with_origin([320, 110])
        .unwrap();
    let viewport = ScreenRect {
        min: [320.0, 110.0],
        max: [1280.0, 772.0],
    };
    let mut group = c.benchmark_group("celestial_navigation");
    for n in [3, 16, 64, 256] {
        let world = common::fixture(n);
        let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
        let mut guides = OrbitGuides::default();
        guides.update(&world);
        let projection_frames =
            CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
        let pair = projection_frames.coherent_view(&world).unwrap();
        for dense in [false, true] {
            let inputs: Vec<_> = ids
                .iter()
                .enumerate()
                .map(|(i, &body)| LabelInput {
                    body,
                    marker: if dense {
                        [800.0, 400.0]
                    } else {
                        [
                            340.0 + (i % 10) as f64 * 85.0,
                            140.0 + (i / 10) as f64 * 24.0,
                        ]
                    },
                    size: [100.0, 20.0],
                    selected: i == n - 1,
                    focused: false,
                    hovered: false,
                    diameter: 0.02 + i as f64,
                    distance_m: 1e11 + i as f64 * 1e8,
                })
                .collect();
            let mut layout = LabelLayout::default();
            let mut labels = Vec::new();
            layout.layout(&inputs, viewport, &[], &mut labels);
            assert!(!labels.is_empty());
            group.bench_function(format!("labels/N{n}/dense{dense}"), |b| {
                b.iter(|| {
                    layout.layout(black_box(&inputs), viewport, &[], &mut labels);
                    black_box(&labels);
                })
            });
            let targets: Vec<_> = inputs
                .iter()
                .map(|i| BodyHitTarget {
                    body: i.body,
                    marker: Some(i.marker),
                    marker_radius_pixels: 8.0,
                    label: None,
                    center_in_view_m: DVec3::new(0.0, 0.0, -i.distance_m),
                    radius_m: 1e6,
                    occluded_overlay: false,
                })
                .collect();
            group.bench_function(format!("picking/N{n}/dense{dense}"), |b| {
                b.iter(|| {
                    black_box(pick_body(black_box(&targets), [800.0, 400.0], projection).unwrap())
                })
            });
        }
        for history_count in [0, 1024, 8192] {
            let history: Vec<_> = (0..history_count)
                .map(|i| DVec3::new((i as f64 * 0.01).cos(), (i as f64 * 0.01).sin(), 0.0) * 1e11)
                .collect();
            group.bench_function(format!("bounds/N{n}/display{history_count}"), |b| {
                b.iter(|| {
                    black_box(
                        SystemViewBounds::calculate(
                            black_box(&world),
                            &OverviewScope::WholeSystem,
                            guides.guides(),
                            &history,
                            false,
                        )
                        .unwrap(),
                    )
                })
            });
        }
        let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
        group.bench_function(format!("transition/N{n}/AU_to_body"), |b| {
            b.iter(|| {
                camera
                    .transition_to(&pair, FocusTarget::Body(ids[1]))
                    .unwrap();
                camera
                    .update_navigation(
                        &pair,
                        &NavigationInput::default(),
                        Duration::from_millis(16),
                    )
                    .unwrap();
                black_box(camera.pose());
                camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
            })
        });
    }
    group.finish();
}
criterion_group! {name=benches_group;config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500));targets=benches}
criterion_main!(benches_group);
