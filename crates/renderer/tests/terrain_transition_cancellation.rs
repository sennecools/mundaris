use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_renderer::planet_surface::{
    ActiveSurfacePatch, GRID_SAMPLES, GeneratedSurfacePatch, StitchedSurface,
    SurfaceErrorContributions, SurfaceExtent, SurfaceGeometrySample, SurfaceTopology,
    SurfaceTransition, active_surface_cover,
};

fn make_surface(
    addresses: &[CubePatchAddress],
    profile: f64,
    topology: &SurfaceTopology,
) -> (Vec<ActiveSurfacePatch>, StitchedSurface) {
    let mut addresses = addresses.to_vec();
    addresses.sort_unstable();
    let cover = active_surface_cover(&addresses, topology).unwrap();
    let patches: Vec<_> = addresses
        .iter()
        .map(|&address| {
            let samples = (0..GRID_SAMPLES)
                .map(|i| {
                    let direction = address
                        .sample_direction((i % 17) as u32, (i / 17) as u32, 16)
                        .unwrap()
                        .unit();
                    SurfaceGeometrySample {
                        position_body_m: direction * (1000.0 + profile),
                        normal_body: direction,
                    }
                })
                .collect();
            GeneratedSurfacePatch::new(
                address,
                1000.0,
                1.0,
                samples,
                SurfaceExtent {
                    min_height_m: -100.0,
                    max_height_m: 100.0,
                    guaranteed_opaque_radius_m: 0.0,
                },
                SurfaceErrorContributions::default(),
            )
            .unwrap()
        })
        .collect();
    let refs: Vec<_> = patches.iter().collect();
    (
        cover.clone(),
        StitchedSurface::build(&cover, &refs, topology).unwrap(),
    )
}

#[test]
fn cancellation_interrupts_inner_build_and_uncancelled_output_is_bitwise_stable() {
    let topology = SurfaceTopology::new();
    let roots: Vec<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    let child = roots[0].children().unwrap();
    let mut refined: Vec<_> = roots.iter().copied().skip(1).collect();
    refined.extend(child);
    let (old_cover, old) = make_surface(&roots, 0.0, &topology);
    let (new_cover, new) = make_surface(&refined, 2.0, &topology);
    let mut polls = 0;
    let cancelled = SurfaceTransition::build_cancellable(
        &old_cover,
        &old,
        &new_cover,
        &new,
        &topology,
        16 * 1024 * 1024,
        || {
            polls += 1;
            polls == 15_000
        },
    )
    .unwrap();
    assert!(cancelled.is_none());
    assert_eq!(polls, 15_000);

    let ordinary = SurfaceTransition::build(
        &old_cover,
        &old,
        &new_cover,
        &new,
        &topology,
        16 * 1024 * 1024,
    )
    .unwrap();
    let cancellable = SurfaceTransition::build_cancellable(
        &old_cover,
        &old,
        &new_cover,
        &new,
        &topology,
        16 * 1024 * 1024,
        || false,
    )
    .unwrap()
    .unwrap();
    assert_eq!(ordinary.triangles().len(), cancellable.triangles().len());
    for (a, b) in ordinary
        .triangles()
        .iter()
        .flatten()
        .zip(cancellable.triangles().iter().flatten())
    {
        for (a, b) in [(a.old, b.old), (a.new, b.new)] {
            assert_eq!(
                a.position_body_m.to_array().map(f64::to_bits),
                b.position_body_m.to_array().map(f64::to_bits)
            );
            assert_eq!(
                a.normal_body.to_array().map(f64::to_bits),
                b.normal_body.to_array().map(f64::to_bits)
            );
        }
        assert_eq!(
            a.old_reference.weights.map(f64::to_bits),
            b.old_reference.weights.map(f64::to_bits)
        );
        assert_eq!(
            a.new_reference.weights.map(f64::to_bits),
            b.new_reference.weights.map(f64::to_bits)
        );
        assert_eq!(a.old_elevation.to_bits(), b.old_elevation.to_bits());
        assert_eq!(a.new_elevation.to_bits(), b.new_elevation.to_bits());
    }
    assert_eq!(
        ordinary.max_displacement_m().to_bits(),
        cancellable.max_displacement_m().to_bits()
    );
}
