//! Opt-in stage/workload probe, separate from uninstrumented Criterion estimates.
use glam::{DQuat, DVec3};
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use std::{hint::black_box, num::NonZeroU64, time::Instant};

fn distribution(label: &str, mut values: Vec<f64>) {
    values.sort_by(f64::total_cmp);
    println!(
        "{label}: median_us={:.3} p95_us={:.3} max_us={:.3}",
        values[values.len() / 2],
        values[(values.len() * 95 / 100).min(values.len() - 1)],
        values[values.len() - 1]
    );
}

fn main() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let source = tree.root();
    let radius = 6.4e6;
    let sphere = Icosphere::new();
    let settings = LodSettings::default();
    for (name, clearance, horizon) in [
        ("A_tiny", 1e11, false),
        ("B_100px", 8.3e7, false),
        ("original_full", 1e7, false),
        ("C_fills", 6.4e6, false),
        ("D_low_orbit", 1e6, false),
        ("E_100km", 1e5, false),
        ("F_10km", 1e4, false),
        ("G_1km", 1e3, false),
        ("H_100m", 100.0, false),
        ("I_10m", 10.0, false),
        ("I_2m", 2.0, false),
        ("I_2m_horizon", 2.0, true),
    ] {
        let orientation = if horizon {
            UnitRotation::try_from_quaternion(DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2))
                .unwrap()
        } else {
            UnitRotation::identity()
        };
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    source,
                    LocalPosition::try_metres(DVec3::Z * (radius + clearance)).unwrap(),
                ),
                orientation,
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection =
            CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: source,
            reference_radius_m: radius,
            projection,
        };
        let body = CelestialRenderBody {
            body_fixed_frame: source,
            reference_radius_m: radius,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: true,
        };
        let mut session = SurfaceLodSession::default();
        let mut transition = Vec::new();
        let mut transition_vectors = 0;
        let mut transition_bytes = 0;
        let mut report = LodReport::default();
        for _ in 0..1000 {
            let start = Instant::now();
            report = session.update(&input, &settings).unwrap();
            transition_vectors += report.profile.dependency_vectors;
            transition_bytes += report.profile.dependency_bytes;
            transition.push(start.elapsed().as_secs_f64() * 1e6);
            if report.settled && !report.desired_estimate_incomplete {
                break;
            }
        }
        assert!(report.settled && !report.budget_constrained);
        println!(
            "\n{name}: h={clearance} diameter_px={:.3} settle_updates={} desired={} balanced={} active={} visible={} level={} constrained={} scratch_bytes={} cache_records={}",
            projection
                .sphere_apparent_diameter_pixels(DVec3::NEG_Z * (radius + clearance), radius)
                .unwrap(),
            transition.len(),
            report.desired_patches,
            report.balanced_patches,
            report.active_patches,
            report.visible_patches,
            report.max_level,
            report.budget_constrained,
            report.scratch_bytes,
            report.cache_records
        );
        distribution("transition_selection", transition);
        println!(
            "transition_dependency_vectors={transition_vectors} cumulative_capacity_bytes={transition_bytes}"
        );
        let mut masks = [0usize; 16];
        for p in session.active_visible() {
            masks[p.stitch_mask as usize] += 1;
        }
        println!("stitch_masks={masks:?}");
        let mut staging = CelestialStaging::default();
        let mut times: [Vec<f64>; 15] = std::array::from_fn(|_| Vec::with_capacity(100));
        let mut capacity_growths = 0;
        let mut selector_growths = 0;
        let mut warm_dependencies = 0;
        for iteration in 0..110 {
            let select_start = Instant::now();
            let r = session.update(&input, &settings).unwrap();
            let select = select_start.elapsed();
            let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
            frame
                .append_surface(
                    body,
                    session.active_visible(),
                    session.topology(),
                    SurfaceStyle::default(),
                )
                .unwrap();
            let s = frame.report().surface;
            black_box(&frame);
            if iteration == 0 {
                println!(
                    "first_prepare_growth_vectors={}",
                    s.profile.capacity_growths
                );
            }
            if iteration >= 10 {
                capacity_growths += s.profile.capacity_growths;
                selector_growths += r.profile.capacity_growths;
                warm_dependencies += r.profile.dependency_vectors;
                let l = r.profile;
                let p = s.profile;
                for (out, duration) in times.iter_mut().zip([
                    select,
                    l.merge_decisions,
                    l.split_ready_transactions,
                    l.visible_and_masks,
                    l.desired_traversal,
                    l.final_reporting,
                    p.total,
                    p.boundary,
                    p.setup,
                    p.samples,
                    p.proof,
                    p.packing,
                    p.grouping,
                    l.total,
                    select + p.total,
                ]) {
                    out.push(duration.as_secs_f64() * 1e6);
                }
            }
            if iteration == 109 {
                println!(
                    "samples={} patches={} triangles={} fallback={} draws={} bytes={} staging_bytes={} boundary_bytes={} whole_proofs={} clipped_proofs={} cache_hits={} cache_misses={} evictions={}",
                    s.samples,
                    s.patches,
                    s.triangles,
                    s.fallback_triangles,
                    s.draws,
                    s.uploaded_bytes,
                    s.allocated_staging_bytes,
                    s.boundary_bytes,
                    s.profile.whole_patch_proofs,
                    s.profile.clipped_patch_proofs,
                    r.cache_hits,
                    r.cache_misses,
                    r.cache_evictions
                );
            }
        }
        println!("warm_prepare_growth_vectors_100={capacity_growths}");
        println!(
            "warm_selector_growth_vectors_100={selector_growths} warm_dependency_vectors_100={warm_dependencies}"
        );
        for (label, values) in [
            "selection",
            "merge",
            "split_ready_balance",
            "visible_masks",
            "desired",
            "report",
            "prepare",
            "boundary",
            "setup",
            "sample_eval_transform_narrow",
            "precision_proof",
            "pack",
            "group",
            "selector_internal",
            "combined",
        ]
        .into_iter()
        .zip(times)
        {
            distribution(label, values);
        }
        // Isolate existing public operation boundaries without per-sample clocks.
        // These probes do not sum to a production stage decomposition.
        let patches = session.active_visible();
        let mut directions = vec![Direction3::try_new(DVec3::Z).unwrap(); patches.len() * 289];
        let mut mapping = Vec::new();
        let mut transforms = Vec::new();
        let prepared = view.prepare_source(source).unwrap();
        let mut positions = vec![DVec3::ZERO; directions.len()];
        let mut normals = positions.clone();
        for _ in 0..100 {
            let start = Instant::now();
            for (p, samples) in patches.iter().zip(directions.chunks_mut(289)) {
                for (i, d) in samples.iter_mut().enumerate() {
                    *d = p
                        .address
                        .sample_key(i as u32 % 17, i as u32 / 17, 16)
                        .unwrap()
                        .direction();
                }
            }
            black_box(&directions);
            mapping.push(start.elapsed().as_secs_f64() * 1e6);
            let start = Instant::now();
            for ((d, p), n) in directions.iter().zip(&mut positions).zip(&mut normals) {
                *p = prepared
                    .view_displacement(FramePosition::new(
                        source,
                        LocalPosition::try_metres(d.unit() * radius).unwrap(),
                    ))
                    .unwrap()
                    .metres();
                *n = prepared.view_direction(*d).unwrap().unit();
            }
            black_box((&positions, &normals));
            transforms.push(start.elapsed().as_secs_f64() * 1e6);
        }
        distribution("isolated_keys_f64_mapping", mapping);
        distribution("isolated_source_transform_normals", transforms);
    }
}
