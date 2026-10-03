use criterion::{Criterion, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_app::{gravity_fixtures::*, planet_terrain::*};
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_world::*;
use std::{hint::black_box, num::NonZeroU64, time::Duration};

fn benches(c: &mut Criterion) {
    let mut world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(81).unwrap())
        .unwrap();
    let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
    let body = ids[1];
    let radius_m = world.body(body).unwrap().properties().reference_radius_m();
    world
        .edit_terrain(body, Some(checkpoint_terrain_definition(radius_m).unwrap()))
        .unwrap();
    let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(81).unwrap()).unwrap();
    let body_fixed_frame = frames.frames_for(body).unwrap().body_fixed;
    let observer = FramePose::new(
        FramePosition::new(
            body_fixed_frame,
            LocalPosition::try_metres(DVec3::Z * (radius_m + 10_000_000.0)).unwrap(),
        ),
        UnitRotation::identity(),
    );
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let mut lod = SurfaceLodSession::new(4096).unwrap();
    let mut terrain_cache =
        TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, MAX_TERRAIN_PATCHES).unwrap();
    let terrain_identity = {
        let world_body = world.body(body).unwrap();
        TerrainGeometryIdentity::new(
            body,
            world_body.terrain().unwrap().clone(),
            world_body.terrain_revision(),
            radius_m,
        )
        .unwrap()
    };
    let mut ready_cover = TerrainReadyCover::default();
    let mut staging = CelestialStaging::default();
    let surface_style = SurfaceStyle::default();
    let visible;
    let geometry;
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
            body_fixed_frame,
            reference_radius_m: radius_m,
            projection,
        };
        let settings = LodSettings::default();
        for _ in 0..1000 {
            let report = lod.update(&input, &settings).unwrap();
            if !report.desired_estimate_incomplete && report.settled {
                break;
            }
        }
        let desired_level = lod
            .active_visible()
            .iter()
            .map(|patch| patch.address.level())
            .max()
            .unwrap_or(0)
            .min(4);
        assert!(
            lod.active_visible()
                .iter()
                .all(|patch| patch.address.level() <= 4),
            "Phase 5 uniform ready-cover preview is restricted to level 4"
        );
        loop {
            ready_cover
                .update(
                    &mut terrain_cache,
                    &terrain_identity,
                    desired_level,
                    1156,
                    None,
                )
                .unwrap();
            if ready_cover
                .active()
                .first()
                .is_some_and(|patch| patch.address.level() == desired_level)
            {
                break;
            }
        }
        ready_cover
            .prepare_visible(&terrain_cache, &terrain_identity, &input)
            .unwrap();
        // Both paths use the exact same actual uniform terrain-preview cover and
        // expanded-ball visibility, not different sphere/terrain patch counts.
        visible = ready_cover.visible().to_vec();
        assert!(visible.iter().all(|patch| patch.stitch_mask == 0));
        geometry = visible
            .iter()
            .map(|patch| {
                terrain_cache
                    .peek(&terrain_identity, patch.address)
                    .expect("uniform target cover geometry is ready")
            })
            .collect::<Vec<_>>();
        assert_eq!(geometry.len(), visible.len());
        // Retain the initial pure desired-LOD result for an auditable workload record.
        eprintln!(
            "planet terrain view setup: body={body:?} clearance_m=10000000 fov_deg=60 viewport=1280x800 desired_lod={} terrain_level={} cover={} visible={} samples={} cache={:?} terrain_pending={} terrain_bytes={} metadata_cache_bytes={} horizon=disabled_for_terrain",
            lod.active_visible()
                .iter()
                .map(|patch| patch.address.level())
                .max()
                .unwrap_or(0),
            desired_level,
            ready_cover.active().len(),
            visible.len(),
            visible.len() * GRID_SAMPLES,
            terrain_cache.report(),
            terrain_cache.pending(),
            terrain_cache.resident_bytes() + ready_cover.bookkeeping_bytes(),
            lod.cache_usage().1,
        );
    }

    let mut group = c.benchmark_group("planet_terrain_view_renderer_preparation");
    let sphere = Icosphere::new();
    group
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    for terrain in [false, true] {
        let label = if terrain {
            "generated_terrain"
        } else {
            "smooth_sphere"
        };
        group.bench_function(
            format!(
                "{label}_same_visible{}_samples{}",
                visible.len(),
                visible.len() * GRID_SAMPLES
            ),
            |b| {
                b.iter(|| {
                    let pair = frames.coherent_view(&world).unwrap();
                    let view = PreparedView::new(
                        &pair.evaluation(),
                        observer,
                        RenderPrecisionBudget::near_debug(),
                    )
                    .unwrap();
                    let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                    let request = CelestialRenderBody {
                        body_fixed_frame,
                        reference_radius_m: radius_m,
                        color: body_color(1),
                        unlit: false,
                        selected: false,
                    };
                    if terrain {
                        frame
                            .append_generated_surface(
                                request,
                                &visible,
                                &geometry,
                                lod.topology(),
                                surface_style,
                            )
                            .unwrap();
                    } else {
                        frame
                            .append_surface(request, &visible, lod.topology(), surface_style)
                            .unwrap();
                    }
                    frame.append_body_observations(&[request], &[true]).unwrap();
                    black_box(frame.report());
                    black_box(&frame);
                })
            },
        );
    }
    group.finish();
    drop(geometry);
    let mut integrated = c.benchmark_group("planet_terrain_view_steady_app_cpu");
    integrated
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_secs(1));
    for terrain in [false, true] {
        integrated.bench_function(
            if terrain {
                "cached_terrain"
            } else {
                "sphere_same_uniform_cover"
            },
            |b| {
                b.iter(|| {
                    let pair = frames.coherent_view(&world).unwrap();
                    let view = PreparedView::new(
                        &pair.evaluation(),
                        observer,
                        RenderPrecisionBudget::near_debug(),
                    )
                    .unwrap();
                    let input = SurfaceViewInput {
                        view: &view,
                        body_fixed_frame,
                        reference_radius_m: radius_m,
                        projection,
                    };
                    lod.update(&input, &LodSettings::default()).unwrap();
                    let work = ready_cover
                        .update(
                            &mut terrain_cache,
                            &terrain_identity,
                            4,
                            1156,
                            Some(Duration::from_millis(2)),
                        )
                        .unwrap();
                    assert_eq!(work.vertices_generated, 0);
                    ready_cover
                        .prepare_visible(&terrain_cache, &terrain_identity, &input)
                        .unwrap();
                    for patch in ready_cover.visible() {
                        terrain_cache.get(&terrain_identity, patch.address).unwrap();
                    }
                    let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                    let request = CelestialRenderBody {
                        body_fixed_frame,
                        reference_radius_m: radius_m,
                        color: body_color(1),
                        unlit: false,
                        selected: false,
                    };
                    if terrain {
                        let geometry = ready_cover
                            .visible()
                            .iter()
                            .map(|p| terrain_cache.peek(&terrain_identity, p.address).unwrap())
                            .collect::<Vec<_>>();
                        frame
                            .append_generated_surface(
                                request,
                                ready_cover.visible(),
                                &geometry,
                                lod.topology(),
                                surface_style,
                            )
                            .unwrap();
                    } else {
                        frame
                            .append_surface(
                                request,
                                ready_cover.visible(),
                                lod.topology(),
                                surface_style,
                            )
                            .unwrap();
                    }
                    frame.append_body_observations(&[request], &[true]).unwrap();
                    black_box((frame.report(), work));
                    black_box(&frame);
                })
            },
        );
    }
    integrated.finish();
    eprintln!(
        "matched steady view: cover={} visible={} samples={} pending={} cache={:?}; GPU payload={} bytes; physics paused, no guides/UI/upload/presentation; identical readiness work in sphere reference",
        ready_cover.active().len(),
        ready_cover.visible().len(),
        ready_cover.visible().len() * GRID_SAMPLES,
        terrain_cache.pending(),
        terrain_cache.report(),
        ready_cover.visible().len() * (GRID_SAMPLES * 48 + 64)
    );
}

criterion_group! {name=terrain_view; config=Criterion::default(); targets=benches}
criterion_main!(terrain_view);
