use criterion::{Criterion, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_app::{gravity_fixtures::*, planet_terrain::*};
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_world::*;
use std::{hint::black_box, num::NonZeroU64, time::Duration};

fn benches(c: &mut Criterion) {
    let mut world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(17).unwrap())
        .unwrap();
    let body = world.bodies().nth(1).unwrap().0;
    let radius = world.body(body).unwrap().properties().reference_radius_m();
    world
        .edit_terrain(body, Some(checkpoint_terrain_definition(radius).unwrap()))
        .unwrap();
    let definition = world.body(body).unwrap().terrain().unwrap().clone();
    let revision = world.body(body).unwrap().terrain_revision();
    let identity = TerrainGeometryIdentity::new(body, definition, revision, radius).unwrap();
    let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(17).unwrap()).unwrap();
    let body_frame = frames.frames_for(body).unwrap().body_fixed;
    let observer = FramePose::new(
        FramePosition::new(
            body_frame,
            LocalPosition::try_metres(DVec3::Z * (radius + 10_000_000.0)).unwrap(),
        ),
        UnitRotation::identity(),
    );
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, MAX_TERRAIN_PATCHES).unwrap();
    let mut cover = TerrainReadyCover::default();
    loop {
        cover
            .update(&mut cache, &identity, 4, 16_384, None)
            .unwrap();
        if cover
            .active()
            .first()
            .is_some_and(|p| p.address.level() == 4)
        {
            break;
        }
    }
    assert_eq!(cover.active().len(), 6 * 4usize.pow(4));
    let mut staging = CelestialStaging::default();
    let sphere = Icosphere::new();
    let style = SurfaceStyle {
        elevation_colors: true,
        ..SurfaceStyle::default()
    };
    let modes = [TerrainRenderMode::Elevation, TerrainRenderMode::Lit];
    let topology = SurfaceTopology::new();
    let mut lod = SurfaceLodSession::new(4096).unwrap();
    {
        let pair = frames.coherent_view(&world).unwrap();
        let view = PreparedView::new(
            &pair.evaluation(),
            observer,
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: body_frame,
            reference_radius_m: radius,
            projection,
        };
        for _ in 0..1000 {
            if lod.update(&input, &LodSettings::default()).unwrap().settled {
                break;
            }
        }
        cover.prepare_visible(&cache, &identity, &input).unwrap();
    }
    let visible = cover.visible().to_vec();
    let geometry: Vec<_> = visible
        .iter()
        .map(|p| cache.peek(&identity, p.address).unwrap())
        .collect();
    let request = CelestialRenderBody {
        body_fixed_frame: body_frame,
        reference_radius_m: radius,
        color: body_color(1),
        unlit: false,
        selected: false,
    };
    let mut renderer = c.benchmark_group("terrain_lighting_renderer_preparation");
    renderer
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    for mode in modes {
        let light = TerrainLighting::default().with_mode(mode);
        renderer.bench_function(format!("{mode:?}_uniform4_cover"), |b| {
            b.iter(|| {
                let pair = frames.coherent_view(&world).unwrap();
                let view = PreparedView::new(
                    &pair.evaluation(),
                    observer,
                    RenderPrecisionBudget::near_debug(),
                )
                .unwrap();
                let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                frame.set_terrain_lighting(light);
                frame
                    .append_generated_surface(request, &visible, &geometry, &topology, style)
                    .unwrap();
                black_box(frame.report());
                black_box(&frame);
            })
        });
    }
    renderer.finish();
    drop(geometry);
    let mut prepare = |lighting,
                       staging: &mut CelestialStaging,
                       cover: &mut TerrainReadyCover,
                       cache: &mut TerrainPatchCache| {
        let pair = frames.coherent_view(&world).unwrap();
        let view = PreparedView::new(
            &pair.evaluation(),
            observer,
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: body_frame,
            reference_radius_m: radius,
            projection,
        };
        let mut frame = CelestialFrame::new(&view, staging, projection, &sphere);
        frame.set_terrain_lighting(lighting);
        lod.update(&input, &LodSettings::default()).unwrap();
        let work = cover
            .update(cache, &identity, 4, 1156, Some(Duration::from_millis(2)))
            .unwrap();
        assert_eq!(work.vertices_generated, 0);
        cover.prepare_visible(cache, &identity, &input).unwrap();
        for patch in cover.visible() {
            cache.get(&identity, patch.address).unwrap();
        }
        let geometry: Vec<_> = cover
            .visible()
            .iter()
            .map(|patch| cache.peek(&identity, patch.address).unwrap())
            .collect();
        frame
            .append_generated_surface(request, cover.visible(), &geometry, &topology, style)
            .unwrap();
        black_box(frame.report());
        black_box(&frame);
    };
    let mut integrated = c.benchmark_group("terrain_lighting_integrated_cpu");
    integrated
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_secs(1));
    for mode in modes {
        let light = TerrainLighting::default().with_mode(mode);
        integrated.bench_function(
            format!("{mode:?}_selection_readiness_lookup_preparation"),
            |b| b.iter(|| prepare(light, &mut staging, &mut cover, &mut cache)),
        );
    }
    integrated.finish();
    eprintln!(
        "matched uniform4: visible={} samples={} draws=1 staged_bytes={} CPU_patch_bytes=13872 cache={:?}; no GPU/upload/presentation in CPU benchmarks",
        visible.len(),
        visible.len() * GRID_SAMPLES,
        visible.len() * (GRID_SAMPLES * 32 + 64),
        cache.report()
    );
}

criterion_group! {name=terrain_lighting; config=Criterion::default(); targets=benches}
criterion_main!(terrain_lighting);
