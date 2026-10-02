use criterion::{Criterion, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_app::{gravity_fixtures::*, planet_surface::*};
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_simulation::*;
use mundaris_world::*;
use std::{hint::black_box, num::NonZeroU64, time::Duration};
fn benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("integrated_planet_surface");
    for clearance in [1e7, 1e4, 100.0, 2.0] {
        let mut world = GravityFixture::Hierarchy
            .create(NonZeroU64::new(1).unwrap())
            .unwrap();
        let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
        let mut frames =
            CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
        let mut runner =
            FixedStepRunner::new(&world, SimulationConfig::try_new(60.0).unwrap()).unwrap();
        let mut session = PlanetSurfaceSession::new(ids[1], 2048).unwrap();
        let sphere = Icosphere::new();
        let mut staging = CelestialStaging::default();
        let projection =
            CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
        let source = frames.frames_for(ids[1]).unwrap().body_fixed;
        let radius = world
            .body(ids[1])
            .unwrap()
            .properties()
            .reference_radius_m();
        let observer = FramePose::new(
            FramePosition::new(
                source,
                LocalPosition::try_metres(DVec3::Z * (radius + clearance)).unwrap(),
            ),
            UnitRotation::identity(),
        );
        {
            let view = PreparedView::new(
                &frames.tree().evaluate(),
                observer,
                RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            for _ in 0..1000 {
                session
                    .update(
                        &SurfaceViewInput {
                            view: &view,
                            body_fixed_frame: source,
                            reference_radius_m: radius,
                            projection,
                        },
                        &LodSettings::default(),
                    )
                    .unwrap();
                if !session.report.desired_estimate_incomplete && session.report.settled {
                    break;
                }
            }
        }
        let workload = format!(
            "h{clearance}/cover{}/visible{}/h60/N3",
            session.report.active_patches, session.report.visible_patches
        );
        group.bench_function(workload, |b| {
            b.iter(|| {
                runner.single_step(true).unwrap();
                runner.pump(&mut world, |_, _| {}).unwrap();
                frames.publish(&world).unwrap();
                let pair = frames.coherent_view(&world).unwrap();
                let view = PreparedView::new(
                    &pair.evaluation(),
                    observer,
                    RenderPrecisionBudget::near_debug(),
                )
                .unwrap();
                session
                    .update(
                        &SurfaceViewInput {
                            view: &view,
                            body_fixed_frame: source,
                            reference_radius_m: radius,
                            projection,
                        },
                        &LodSettings::default(),
                    )
                    .unwrap();
                let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                for (index, (id, body)) in world.bodies().enumerate() {
                    let request = CelestialRenderBody {
                        body_fixed_frame: frames.frames_for(id).unwrap().body_fixed,
                        reference_radius_m: body.properties().reference_radius_m(),
                        color: body_color(index),
                        unlit: index == 0,
                        selected: id == ids[1],
                    };
                    if id == ids[1] {
                        frame
                            .append_surface(
                                request,
                                session.lod().active_visible(),
                                session.lod().topology(),
                                SurfaceStyle::default(),
                            )
                            .unwrap();
                        frame.append_body_observations(&[request], &[true]).unwrap();
                    } else {
                        frame.append_bodies(&[request]).unwrap();
                    }
                }
                black_box(frame.report());
                black_box(&frame);
                black_box(&world);
            })
        });
    }
    group.finish();
}
criterion_group! {name=approach;config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500));targets=benches}
criterion_main!(approach);
