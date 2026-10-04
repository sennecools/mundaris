//! Physical-resolution and error-term audit of the ordinary gameplay certificate.
use anyhow::{Context, Result};
use glam::DVec3;
use mundaris_app::{planet_terrain::terrain_surface_certificate, solar_system::*};
use mundaris_math::{Direction3, surface::*};
use mundaris_renderer::{CelestialProjection, planet_surface::*};
use mundaris_world::terrain::*;
use std::{fs, num::NonZeroU64, path::PathBuf};

const DIRECTION: DVec3 = DVec3::new(0.04592207301441123, 0.4595519490375417, 0.886962890625);
const LEVELS: [u8; 13] = [10, 12, 14, 16, 18, 19, 20, 21, 22, 23, 24, 25, 30];

fn arc_m(a: DVec3, b: DVec3, radius_m: f64) -> f64 {
    radius_m * a.cross(b).length().atan2(a.dot(b))
}

fn main() -> Result<()> {
    let output = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .context("usage: terrain_quality_probe OUTPUT_DIR")?,
    );
    fs::create_dir_all(&output)?;
    let world =
        SolarSystemPreset::gameplay().create(NonZeroU64::new(5101).context("namespace")?)?;
    let (_, body) = world
        .bodies()
        .nth(SolarBody::Earth as usize)
        .context("Earth")?;
    let radius_m = body.properties().reference_radius_m();
    let generator = TerrainGenerator::new(body.terrain().context("terrain")?, radius_m)?;
    let direction = DIRECTION.normalize();
    let location = SurfaceLocation::new(Direction3::try_new(direction)?);
    let height = generator
        .evaluate_point(TerrainQuery {
            location,
            footprint: TerrainFootprint::COMPLETE,
        })?
        .height_m();
    let (face, uv) = location.face_uv();
    let topology = SurfaceTopology::new();
    let projection = CelestialProjection::try_new(768, 512, 60.0_f64.to_radians(), 0.1)?;
    let mut csv = String::from(
        "clearance_m,lod,width_u_m,width_v_m,min_spacing_m,max_spacing_m,evaluator_footprint_m,pixel_footprint_m,represented_bound_m,gradient_bound_m,hessian_bound_m,sphere_m,interpolation_m,unresolved_m,boundary_m,morph_m,numeric_m,sphere_px,interpolation_px,unresolved_px,boundary_px,morph_px,numeric_px,total_px,projection_depth_floor_m,dominant\n",
    );
    let mut targets =
        String::from("clearance_m,certificate_target_lod,sample_spacing_m,total_px\n");
    for clearance_m in [1000.0, 100.0, 10.0, 2.0] {
        let eye = direction * (radius_m + height + clearance_m);
        let right = DVec3::Y.cross(direction).normalize();
        let up = direction.cross(right);
        let mut found = false;
        for level in 0..=30 {
            let count = 1u32 << level;
            let coordinate =
                |v: f64| (((v + 1.0) * 0.5 * f64::from(count)).floor() as u32).min(count - 1);
            let address =
                CubePatchAddress::try_new(face, level, coordinate(uv[0]), coordinate(uv[1]))?;
            let n = |x, y| -> Result<DVec3> { Ok(address.sample_direction(x, y, 16)?.unit()) };
            let width_u = arc_m(n(0, 8)?, n(16, 8)?, radius_m);
            let width_v = arc_m(n(8, 0)?, n(8, 16)?, radius_m);
            let mut min_spacing = f64::INFINITY;
            let mut max_spacing: f64 = 0.0;
            for y in 0..=16 {
                for x in 0..=16 {
                    let a = n(x, y)?;
                    for (dx, dy) in [(1, 0), (0, 1)] {
                        if x + dx <= 16 && y + dy <= 16 {
                            let spacing = arc_m(a, n(x + dx, y + dy)?, radius_m);
                            min_spacing = min_spacing.min(spacing);
                            max_spacing = max_spacing.max(spacing);
                        }
                    }
                }
            }
            let metadata = PatchMetadata::build(address, &topology)?;
            let (extent, mut error) = terrain_surface_certificate(&generator, address, metadata)?;
            let footprint_m = radius_m * 2.0 / (16.0 * f64::from(count));
            let owner_footprint =
                radius_m * 2.0 / (16.0 * (1u64 << level.saturating_sub(2)) as f64);
            error.boundary_constraint_m = generator.profile_difference_bound_m(
                TerrainFootprint::new(footprint_m)?,
                TerrainFootprint::new(owner_footprint)?,
            )? + error.numeric_m;
            let certificate = generator.bounds_for_region(
                DirectionalCap::new(Direction3::try_new(metadata.cap().0)?, metadata.cap().1)?,
                TerrainFootprint::new(footprint_m)?,
            )?;
            let expanded = SurfaceExtent {
                min_height_m: (extent.min_height_m - error.boundary_constraint_m).next_down(),
                max_height_m: (extent.max_height_m + error.boundary_constraint_m).next_up(),
                ..extent
            };
            let (center, ball_radius) = metadata.ball(radius_m, expanded)?;
            let relative = center - eye;
            let center_view = DVec3::new(
                relative.dot(right),
                relative.dot(up),
                relative.dot(direction),
            );
            let total_px =
                metadata.projected_total_error(error, center_view, ball_radius, projection)?;
            let terms = [
                error.sphere_m,
                error.filtered_interpolation_m,
                error.unresolved_m,
                error.boundary_constraint_m,
                error.morph_remaining_m,
                error.numeric_m,
            ];
            let names = [
                "sphere",
                "interpolation",
                "unresolved",
                "boundary",
                "morph",
                "numerical",
            ];
            let mut pixels = [0.0; 6];
            for (i, term) in terms.iter().enumerate() {
                pixels[i] = metadata.projected_total_error(
                    SurfaceErrorContributions {
                        sphere_m: *term,
                        ..Default::default()
                    },
                    center_view,
                    ball_radius,
                    projection,
                )?;
            }
            let dominant = (0..6)
                .max_by(|&a, &b| terms[a].total_cmp(&terms[b]))
                .context("terms")?;
            if !found && total_px <= LodSettings::default().split_pixels() {
                targets.push_str(&format!(
                    "{clearance_m},{level},{max_spacing:.12e},{total_px:.12e}\n"
                ));
                found = true;
            }
            if LEVELS.contains(&level) {
                csv.push_str(&format!("{clearance_m},{level},{width_u:.12e},{width_v:.12e},{min_spacing:.12e},{max_spacing:.12e},{footprint_m:.12e},{:.12e},{:.12e},{:.12e},{:.12e}", clearance_m / projection.focal_pixels(), certificate.represented_height_bound_m(), certificate.cartesian_height_gradient_bound_m(), certificate.cartesian_height_hessian_bound_m()));
                for value in terms.into_iter().chain(pixels) {
                    csv.push_str(&format!(",{value:.12e}"));
                }
                csv.push_str(&format!(
                    ",{total_px:.12e},{:.12e},{}\n",
                    (-center_view.z - ball_radius).max(projection.near_m()),
                    names[dominant]
                ));
            }
        }
    }
    fs::write(output.join("quality.csv"), csv)?;
    fs::write(output.join("targets.csv"), &targets)?;
    fs::write(
        output.join("manifest.txt"),
        format!(
            "gameplay Earth radius_m={radius_m} direction={direction:?} complete_height_m={height}\nviewport=768x512 vertical_fov_deg=60 near_m=0.1 split_px=0.125\nview=nadir; source=ungenerated patch certificate plus two-level boundary ownership allowance; morph=0\nspacing=actual normalized-cube Grid16 geodesic neighbor distances at reference radius; width=two midline geodesics\npixel_footprint=clearance/focal_pixels (nadir tangent-plane center); certificate projection retains full conservative displaced ball\n"
        ),
    )?;
    print!("{targets}");
    Ok(())
}
