//! One certificate construction shared by generation and adaptive desirability.
use super::*;

pub fn terrain_surface_certificate(
    generator: &TerrainGenerator,
    address: CubePatchAddress,
    metadata: PatchMetadata,
) -> Result<(SurfaceExtent, SurfaceErrorContributions)> {
    let radius = generator.radius_m();
    let footprint =
        TerrainFootprint::new(radius * 2.0 / (16.0 * (1u64 << address.level()) as f64))?;
    let (axis, alpha) = metadata.cap();
    let certificate = generator.bounds_for_region(
        DirectionalCap::new(Direction3::try_new(axis)?, alpha)?,
        footprint,
    )?;
    let interval = certificate.height_interval_m();
    // All sixteen stitched variants fit this conservative face-domain diameter.
    let diameter = 4.0 * 2.0 / (16.0 * (1u64 << address.level()) as f64);
    let represented = certificate.represented_height_bound_m();
    let gradient = certificate.cartesian_height_gradient_bound_m();
    let hessian = certificate.cartesian_height_hessian_bound_m();
    let interval_error = (2.0 * represented + represented * diameter.min(2.0)).next_up();
    let lipschitz_error = ((gradient + represented).next_up() * diameter).next_up();
    // ||Dn|| <= 1, ||D²n|| <= 3 for normalized radial cube charts.
    let vector_hessian =
        (hessian + (5.0 * gradient).next_up() + (3.0 * represented).next_up()).next_up();
    let curvature_error =
        SurfaceErrorContributions::interpolation_bound_m(vector_hessian, diameter)?;
    let sphere_correspondence =
        SurfaceErrorContributions::interpolation_bound_m((3.0 * radius).next_up(), diameter)?;
    Ok((
        SurfaceExtent {
            min_height_m: interval[0],
            max_height_m: interval[1],
            guaranteed_opaque_radius_m: 0.0,
        },
        SurfaceErrorContributions {
            sphere_m: (radius * metadata.error_unit())
                .next_up()
                .max(sphere_correspondence),
            filtered_interpolation_m: interval_error.min(lipschitz_error).min(curvature_error),
            unresolved_m: certificate.unresolved_height_bound_m(),
            boundary_constraint_m: 0.0,
            morph_remaining_m: 0.0,
            numeric_m: (512.0 * f64::EPSILON * (radius + interval[1].abs().max(interval[0].abs())))
                .next_up(),
        },
    ))
}

/// Intersect the global interval with a certified regional interval around the
/// already evaluated chart-centre sample. This is not a sampled-extrema bound:
/// integrating the global tangent-gradient bound along a unit-sphere geodesic
/// proves variation <= G * alpha throughout the cap. The omitted-band allowance
/// covers complete truth as well as the filtered field; numeric margins cover
/// radial reconstruction and the centre direction. Interpolation is unchanged.
pub(super) fn certificate_for_samples(
    generator: &TerrainGenerator,
    address: CubePatchAddress,
    metadata: PatchMetadata,
    samples: &[SurfaceGeometrySample],
) -> Result<(SurfaceExtent, SurfaceErrorContributions)> {
    let (mut extent, error) = terrain_surface_certificate(generator, address, metadata)?;
    let (axis, alpha) = metadata.cap();
    let footprint = TerrainFootprint::new(
        generator.radius_m() * 2.0 / (16.0 * (1u64 << address.level()) as f64),
    )?;
    let certificate = generator.bounds_for_region(
        DirectionalCap::new(Direction3::try_new(axis)?, alpha)?,
        footprint,
    )?;
    let centre = samples
        .get(8 * 17 + 8)
        .ok_or_else(|| anyhow::anyhow!("missing terrain centre sample"))?;
    let height = centre.position_body_m.length() - generator.radius_m();
    let variation = (certificate.cartesian_height_gradient_bound_m()
        * (alpha + 64.0 * f64::EPSILON).next_up())
    .next_up();
    let allowance = ((variation + certificate.unresolved_height_bound_m()).next_up()
        + error.numeric_m)
        .next_up();
    extent.min_height_m = extent.min_height_m.max((height - allowance).next_down());
    extent.max_height_m = extent.max_height_m.min((height + allowance).next_up());
    anyhow::ensure!(
        extent.min_height_m <= extent.max_height_m,
        "invalid regional terrain interval"
    );
    Ok((extent, error))
}
