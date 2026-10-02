use glam::DVec3;
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_renderer::planet_surface::{
    GRID_SAMPLES, GeneratedSurfacePatch, SurfaceErrorContributions, SurfaceExtent,
    SurfaceGeometrySample,
};

fn flat_samples(address: CubePatchAddress, radius: f64) -> Vec<SurfaceGeometrySample> {
    (0..GRID_SAMPLES)
        .map(|index| {
            let direction = address
                .sample_direction(index as u32 % 17, index as u32 / 17, 16)
                .unwrap()
                .unit();
            SurfaceGeometrySample {
                position_body_m: direction * radius,
                normal_body: direction,
            }
        })
        .collect()
}

#[test]
fn generated_grid16_is_domain_free_and_reports_resident_sample_storage() {
    let address = CubePatchAddress::root(CubeFace::PositiveZ);
    let geometry = GeneratedSurfacePatch::new(
        address,
        10.0,
        0.0,
        flat_samples(address, 10.0),
        SurfaceExtent::smooth(10.0),
        SurfaceErrorContributions::default(),
    )
    .unwrap();
    assert_eq!(geometry.address(), address);
    assert_eq!(geometry.footprint_m(), 0.0);
    assert_eq!(geometry.samples().len(), GRID_SAMPLES);
    assert_eq!(geometry.extent().min_height_m, 0.0);
    assert_eq!(geometry.error().total_m().unwrap(), 0.0);
    assert_eq!(
        geometry.resident_heap_bytes(),
        289 * 2 * std::mem::size_of::<DVec3>()
    );
    assert_eq!(
        geometry.resident_heap_capacity_bytes(),
        geometry.resident_heap_bytes()
    );
}

#[test]
fn generated_patch_rejects_invalid_footprint_sample_count_and_inward_sample() {
    let address = CubePatchAddress::root(CubeFace::PositiveZ);
    let extent = SurfaceExtent::smooth(10.0);
    let error = SurfaceErrorContributions::default();
    assert!(
        GeneratedSurfacePatch::new(
            address,
            10.0,
            f64::NAN,
            flat_samples(address, 10.0),
            extent,
            error
        )
        .is_err()
    );
    assert!(GeneratedSurfacePatch::new(address, 10.0, 0.0, vec![], extent, error).is_err());
    let mut bad = flat_samples(address, 10.0);
    bad[17].normal_body = -bad[17].normal_body;
    assert!(GeneratedSurfacePatch::new(address, 10.0, 0.0, bad, extent, error).is_err());
    assert!(
        GeneratedSurfacePatch::new(
            address,
            11.0,
            0.0,
            flat_samples(address, 10.0),
            extent,
            error
        )
        .is_err()
    );
}
