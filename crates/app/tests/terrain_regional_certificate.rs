use glam::DVec3;
use mundaris_app::planet_terrain::{checkpoint_terrain_definition, terrain_surface_certificate};
use mundaris_math::{Direction3, surface::*};
use mundaris_renderer::{CelestialProjection, planet_surface::*};
use mundaris_world::terrain::*;

#[test]
fn ungenerated_regional_intervals_contain_complete_truth_across_radius_fixtures() {
    let topology = SurfaceTopology::new();
    let projection = CelestialProjection::try_new(768, 512, 60.0_f64.to_radians(), 0.1).unwrap();
    for radius in [
        50_000.0,
        100_000.0,
        400_000.0,
        1_000_000.0,
        6_371_000.0,
        12_742_000.0,
    ] {
        let generator =
            TerrainGenerator::new(&checkpoint_terrain_definition(radius).unwrap(), radius).unwrap();
        let mut last_target = 0;
        for clearance in [1000.0, 100.0, 10.0, 2.0] {
            let direction = DVec3::new(0.1, 0.4, 1.0).normalize();
            let location = SurfaceLocation::new(Direction3::try_new(direction).unwrap());
            let height = generator
                .evaluate_point(TerrainQuery {
                    location,
                    footprint: TerrainFootprint::COMPLETE,
                })
                .unwrap()
                .height_m();
            let (face, uv) = location.face_uv();
            let eye = direction * (radius + height + clearance);
            let right = DVec3::Y.cross(direction).normalize();
            let up = direction.cross(right);
            let target = (0..=30)
                .find(|&level| {
                    let count = 1u32 << level;
                    let coordinate = |v: f64| {
                        (((v + 1.0) * 0.5 * f64::from(count)).floor() as u32).min(count - 1)
                    };
                    let address = CubePatchAddress::try_new(
                        face,
                        level,
                        coordinate(uv[0]),
                        coordinate(uv[1]),
                    )
                    .unwrap();
                    let metadata = PatchMetadata::build(address, &topology).unwrap();
                    let (extent, error) =
                        terrain_surface_certificate(&generator, address, metadata).unwrap();
                    let (center, ball) = metadata.ball(radius, extent).unwrap();
                    let relative = center - eye;
                    let center_view = DVec3::new(
                        relative.dot(right),
                        relative.dot(up),
                        relative.dot(direction),
                    );
                    metadata
                        .projected_total_error(error, center_view, ball, projection)
                        .unwrap()
                        <= LodSettings::default().split_pixels()
                })
                .expect("quality terminates by certificate, not max LOD");
            assert!(
                target >= last_target && target < 30,
                "radius={radius} clearance={clearance} target={target}"
            );
            last_target = target;
        }
        for level in [12, 16, 20, 25] {
            let count = 1u32 << level;
            for (x, y) in [
                (count / 2, count / 2),
                (count - 1, count / 2),
                (count - 1, count - 1),
            ] {
                let address = CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y).unwrap();
                let metadata = PatchMetadata::build(address, &topology).unwrap();
                let (extent, _) =
                    terrain_surface_certificate(&generator, address, metadata).unwrap();
                let (center, ball) = metadata.ball(radius, extent).unwrap();
                for uv in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.37, 0.61]] {
                    let direction = address
                        .face()
                        .direction(address.face_uv(uv).unwrap())
                        .unwrap();
                    let height = generator
                        .evaluate_point(TerrainQuery {
                            location: SurfaceLocation::new(direction),
                            footprint: TerrainFootprint::COMPLETE,
                        })
                        .unwrap()
                        .height_m();
                    assert!(height >= extent.min_height_m && height <= extent.max_height_m);
                    assert!((direction.unit() * (radius + height) - center).length() <= ball);
                }
            }
        }
    }
}
