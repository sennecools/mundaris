use criterion::{Criterion, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_app::trails::{TrailDisplayScratch, TrailHistory};
use mundaris_math::*;
use mundaris_renderer::*;
use mundaris_simulation::*;
use mundaris_world::*;
use std::{hint::black_box, num::NonZeroU64, time::Duration};
fn benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("trail_history_sampling");
    for bodies in [3, 16] {
        for samples in [1024, 8192] {
            let mut system =
                CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
            for i in 0..bodies {
                system
                    .insert_body(
                        "recorded body",
                        BodyProperties::new(1e18, 1e3).unwrap(),
                        BodyState::new(
                            LocalPosition::try_metres(DVec3::X * i as f64 * 1e9).unwrap(),
                            LinearVelocity3::zero(),
                            UnitRotation::identity(),
                            AngularVelocity3::zero(),
                        ),
                    )
                    .unwrap();
            }
            let updates: Vec<_> = system
                .bodies()
                .map(|(body, b)| BodyStateUpdate {
                    body,
                    state: *b.state(),
                })
                .collect();
            let mut history =
                TrailHistory::with_limits(&system, 1, samples, 8 * 1024 * 1024).unwrap();
            let mut tick = 0u64;
            // Stationary externally committed samples isolate sampling from integration.
            group.bench_function(format!("B{bodies}/samples{samples}"), |b| {
                b.iter(|| {
                    tick += 1;
                    system
                        .update_states(
                            SimulationInstant::try_seconds_since_epoch(tick as f64).unwrap(),
                            &updates,
                        )
                        .unwrap();
                    history.record_committed(0, black_box(tick), black_box(&system));
                    black_box(history.sample_count());
                    black_box(&history);
                })
            });
        }
    }
    group.finish();
    let mut group = c.benchmark_group("hierarchy_committed_512_trails_projection");
    let mut world = mundaris_app::gravity_fixtures::GravityFixture::Hierarchy
        .create(NonZeroU64::new(2).unwrap())
        .unwrap();
    let mut runner =
        FixedStepRunner::new(&world, SimulationConfig::try_new(60.0).unwrap()).unwrap();
    let mut history = TrailHistory::new(&world, 64).unwrap();
    let mut projection =
        CelestialFrameProjection::build(&world, NonZeroU64::new(2).unwrap()).unwrap();
    group.bench_function("h60", |b| {
        b.iter(|| {
            runner.request_forward_to_tick(runner.tick() + 512).unwrap();
            black_box(
                runner
                    .pump(&mut world, |tick, system| {
                        history.record_committed(0, tick, system)
                    })
                    .unwrap(),
            );
            projection.publish(&world).unwrap();
            black_box(&projection);
            black_box(history.sample_count());
            black_box(&history);
        })
    });
    group.finish();
    let mut group = c.benchmark_group("trail_history_line_requests");
    for bodies in [3, 16] {
        for samples in [1024, 8192] {
            let mut world =
                CelestialSystem::new(NonZeroU64::new(3).unwrap(), SimulationInstant::ZERO);
            for i in 0..bodies {
                world
                    .insert_body(
                        "committed probe",
                        BodyProperties::new(1.0, 1.0).unwrap(),
                        BodyState::new(
                            LocalPosition::try_metres(DVec3::X * i as f64 * 10.0).unwrap(),
                            LinearVelocity3::zero(),
                            UnitRotation::identity(),
                            AngularVelocity3::zero(),
                        ),
                    )
                    .unwrap();
            }
            let updates: Vec<_> = world
                .bodies()
                .map(|(body, b)| BodyStateUpdate {
                    body,
                    state: *b.state(),
                })
                .collect();
            let mut history =
                TrailHistory::with_limits(&world, 1, samples, 8 * 1024 * 1024).unwrap();
            for tick in 1..samples as u64 {
                world
                    .update_states(
                        SimulationInstant::try_seconds_since_epoch(tick as f64).unwrap(),
                        &updates,
                    )
                    .unwrap();
                history.record_committed(0, tick, &world);
            }
            let projection =
                CelestialFrameProjection::build(&world, NonZeroU64::new(3).unwrap()).unwrap();
            let mut lines = Vec::new();
            group.bench_function(format!("B{bodies}/samples{samples}"), |b| {
                b.iter(|| {
                    black_box(
                        history
                            .prepare_lines(black_box(&world), &projection, &mut lines)
                            .unwrap(),
                    );
                    black_box(&lines);
                })
            });
        }
    }
    group.finish();
    let mut group = c.benchmark_group("exact_chunked_planetary_commits");
    for n in [3, 8, 16, 20] {
        let mut world = planetary::fixture(n);
        let mut runner =
            FixedStepRunner::new(&world, SimulationConfig::try_new(60.0).unwrap()).unwrap();
        let mut history = TrailHistory::new(&world, 64).unwrap();
        let mut projection =
            CelestialFrameProjection::build(&world, NonZeroU64::new(4).unwrap()).unwrap();
        for chunk in [1, 32, 512] {
            group.bench_function(format!("N{n}/h60/chunk{chunk}"), |b| {
                b.iter(|| {
                    let target = runner.tick() + 512;
                    runner.request_forward_to_tick(target).unwrap();
                    while runner.tick() < target {
                        runner
                            .pump_with_work_limit(&mut world, chunk, |tick, w| {
                                history.record_committed(0, tick, w)
                            })
                            .unwrap();
                    }
                    projection.publish(&world).unwrap();
                    black_box(&world);
                    black_box(&history);
                    black_box(runner.report());
                })
            });
        }
    }
    group.finish();
    let mut group = c.benchmark_group("bounded_committed_trail_display");
    for bodies in [3, 16] {
        for samples in [1024, 8192] {
            let mut world = planetary::fixture(bodies);
            let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
            let mut history =
                TrailHistory::with_limits(&world, 1, samples, 8 * 1024 * 1024).unwrap();
            // Actual externally committed curved samples isolate presentation from KDK.
            for tick in 1..samples {
                let updates: Vec<_> = world
                    .bodies()
                    .enumerate()
                    .map(|(i, (body, b))| {
                        let s = *b.state();
                        let angle = tick as f64 * 0.002;
                        BodyStateUpdate {
                            body,
                            state: BodyState::new(
                                LocalPosition::try_metres(
                                    DVec3::new(angle.cos(), angle.sin(), i as f64 * 0.001) * 1e11,
                                )
                                .unwrap(),
                                s.center_velocity_in_system(),
                                s.body_to_system(),
                                s.angular_velocity_in_system(),
                            ),
                        }
                    })
                    .collect();
                world
                    .update_states(
                        SimulationInstant::try_seconds_since_epoch(tick as f64).unwrap(),
                        &updates,
                    )
                    .unwrap();
                history.record_committed(0, tick as u64, &world);
            }
            let projection =
                CelestialFrameProjection::build(&world, NonZeroU64::new(5).unwrap()).unwrap();
            let view = PreparedView::new(
                &projection.tree().evaluate(),
                FramePose::new(
                    FramePosition::new(
                        projection.tree().root(),
                        LocalPosition::try_metres(DVec3::Z * 4e11).unwrap(),
                    ),
                    UnitRotation::identity(),
                ),
                RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            let screen = CelestialProjection::try_new(1280, 800, 1.0, 0.1).unwrap();
            let sphere = Icosphere::new();
            let mut storage = CelestialStaging::default();
            let mut points = Vec::new();
            let mut colors = Vec::new();
            let mut scratch = TrailDisplayScratch::default();
            for relative in [false, true] {
                if relative {
                    history
                        .set_mode(
                            mundaris_app::trails::TrailMode::SimultaneousBodyRelative(ids[1]),
                            0,
                            (samples - 1) as u64,
                            &world,
                        )
                        .unwrap();
                }
                group.bench_function(
                    format!("B{bodies}/retained{samples}/relative{relative}"),
                    |b| {
                        b.iter(|| {
                            let mut frame =
                                CelestialFrame::new(&view, &mut storage, screen, &sphere);
                            for &id in &ids {
                                history
                                    .display_points(
                                        &world,
                                        &projection,
                                        id,
                                        8193,
                                        &mut points,
                                        &mut colors,
                                    )
                                    .unwrap();
                                black_box(
                                    scratch
                                        .simplify(&view, screen, 1024, &mut points, &mut colors)
                                        .unwrap(),
                                );
                                frame
                                    .append_polylines(&[CelestialPolyline {
                                        points: &points,
                                        colors: &colors,
                                        width_pixels: if id == ids[1] { 2.5 } else { 1.5 },
                                        style: CelestialLineStyle::Solid,
                                    }])
                                    .unwrap();
                            }
                            black_box(frame.report());
                            black_box(&frame);
                        })
                    },
                );
            }
        }
    }
    group.finish();
}
#[path = "common/mod.rs"]
mod planetary;
criterion_group! {name=trails;config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500));targets=benches}
criterion_main!(trails);
