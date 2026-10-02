use glam::DVec3;
use mundaris_app::{gravity_fixtures::GravityFixture, planet_terrain::*};
use mundaris_math::{Direction3, surface::*};
use mundaris_renderer::planet_surface::*;
use mundaris_world::terrain::*;
use std::num::NonZeroU64;

#[test]
fn all_stitch_certificates_bound_full_field_against_filtered_triangle_geometry() {
    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(404).unwrap())
        .unwrap();
    let body = world.bodies().nth(1).unwrap().0;
    let radius = world.body(body).unwrap().properties().reference_radius_m();
    let topology = SurfaceTopology::new();
    let base = checkpoint_terrain_definition(radius).unwrap();
    let mut random = 17u64;
    for seed in 0..8 {
        let definition = TerrainDefinition::new(
            base.identity(),
            TerrainSeed(seed),
            base.version(),
            base.config().clone(),
        );
        let generator = TerrainGenerator::new(&definition, radius).unwrap();
        let identity =
            TerrainGeometryIdentity::new(body, definition, TerrainRevision::default(), radius)
                .unwrap();
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 8).unwrap();
        for level in [0, 4, 10, 18, 30] {
            let address =
                CubePatchAddress::try_new(CubeFace::ALL[seed as usize % 6], level, 0, 0).unwrap();
            cache.request(&identity, address);
            cache.generate(289, 32, None).unwrap();
            let patch = cache.peek(&identity, address).unwrap();
            let error = patch.error().total_m().unwrap();
            for mask in 0..16 {
                let triangles = topology.indices(mask).as_chunks::<3>().0;
                for _ in 0..256 {
                    random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let triangle = triangles[(random as usize) % triangles.len()];
                    let a = ((random >> 20) & 65535) as f64 / 65536.0;
                    let b = ((random >> 40) & 65535) as f64 / 65536.0;
                    let weights = [1.0 - a, a * (1.0 - b), a * b];
                    let mut uv = [0.0; 2];
                    let mut mesh = DVec3::ZERO;
                    for (index, weight) in triangle.into_iter().zip(weights) {
                        let i = u32::from(index);
                        uv[0] += weight * f64::from(i % 17) / 16.0;
                        uv[1] += weight * f64::from(i / 17) / 16.0;
                        mesh += patch.samples()[usize::from(index)].position_body_m * weight;
                    }
                    let n = address
                        .face()
                        .direction(address.face_uv(uv).unwrap())
                        .unwrap();
                    let location = SurfaceLocation::new(n);
                    let full = generator
                        .evaluate_point(TerrainQuery {
                            location,
                            footprint: TerrainFootprint::COMPLETE,
                        })
                        .unwrap();
                    let truth = n.unit() * (radius + full.height_m());
                    assert!(truth.is_finite() && mesh.is_finite());
                    assert!(
                        (truth - mesh).length() <= error + 1e-6,
                        "seed={seed} level={level} mask={mask} residual={} error={error}",
                        (truth - mesh).length()
                    );
                    let normal = full.normal_body(location, radius).unwrap();
                    assert!(
                        normal
                            .unit()
                            .dot(Direction3::try_new(truth).unwrap().unit())
                            > 0.0
                    );
                }
            }
        }
    }
}
