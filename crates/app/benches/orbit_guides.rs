mod common;
use criterion::{Criterion, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_app::orbit_guides::*;
use mundaris_simulation::*;
use std::{hint::black_box, time::Duration};
fn benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("orbit_guides");
    for n in [3, 16, 64, 256] {
        let world = common::fixture(n);
        let mut guides = OrbitGuides::default();
        guides.update(&world);
        assert!(guides.guides().iter().any(|g| g.elements.is_some()));
        group.bench_function(format!("reference_elements_tidal/N{n}"), |b| {
            b.iter(|| {
                guides.update(black_box(&world));
                black_box(guides.guides());
            })
        });
        let mut points = Vec::new();
        for segments in [64, 512] {
            group.bench_function(format!("tessellation/N{n}/segments{segments}"), |b| {
                b.iter(|| {
                    for g in guides.guides() {
                        g.vertices(segments, &mut points).unwrap();
                        black_box(&points);
                    }
                })
            });
        }
        group.bench_function(format!("screen_refinement/N{n}"), |b| {
            b.iter(|| {
                for g in guides.guides() {
                    g.tessellate(512, |p| Ok(Some([p.x / 1e9, p.y / 1e9])), &mut points)
                        .unwrap();
                    black_box(&points);
                }
            })
        });
    }
    for e in [0.0, 0.3] {
        let r = DVec3::X * (1e7 * (1.0 - e));
        let v = DVec3::Y * (6.67430e-11 * (1e24 + 1e20) * (1.0 + e) / r.x).sqrt();
        group.bench_function(format!("pure_elements/e{e}"), |b| {
            b.iter(|| {
                black_box(osculating_elements(black_box(r), black_box(v), 1e24, 1e20).unwrap())
            })
        });
    }
    group.finish();
}
criterion_group! {name=benches_group;config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500));targets=benches}
criterion_main!(benches_group);
