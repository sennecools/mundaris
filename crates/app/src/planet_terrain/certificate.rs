//! One certificate construction shared by generation and adaptive desirability.
use super::*;

pub fn terrain_surface_certificate(
    generator: &TerrainGenerator,
    address: CubePatchAddress,
    metadata: PatchMetadata,
) -> Result<(SurfaceExtent, SurfaceErrorContributions)> {
    surface_certificate(generator, address, metadata, None)
}

fn surface_certificate(
    generator: &TerrainGenerator,
    address: CubePatchAddress,
    metadata: PatchMetadata,
    centre_height_m: Option<f64>,
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
    // Every emitted all-stitch edge has squared Grid16 length <= 5
    // (renderer topology regression). Barycentrics use the affine face UV
    // domain, not angular distance: one grid step is 2/(16*2^level).
    let diameter =
        (5.0_f64.sqrt().next_up() * 2.0 / (16.0 * (1u64 << address.level()) as f64)).next_up();
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
    let mut extent = SurfaceExtent {
        min_height_m: interval[0],
        max_height_m: interval[1],
        guaranteed_opaque_radius_m: 0.0,
    };
    let error = SurfaceErrorContributions {
        sphere_m: (radius * metadata.error_unit())
            .next_up()
            .max(sphere_correspondence),
        filtered_interpolation_m: interval_error.min(lipschitz_error).min(curvature_error),
        unresolved_m: certificate.unresolved_height_bound_m(),
        boundary_constraint_m: 0.0,
        morph_remaining_m: 0.0,
        numeric_m: (512.0 * f64::EPSILON * (radius + interval[1].abs().max(interval[0].abs())))
            .next_up(),
    };
    let variation = (gradient * (alpha + 64.0 * f64::EPSILON).next_up()).next_up();
    // Evaluate an anchor only when the global Lipschitz interval can actually
    // narrow the global range. This anchor is not a sampled-extrema certificate.
    if variation < (interval[1] - interval[0]) * 0.5 {
        let height = match centre_height_m {
            Some(height) => height,
            None => generator
                .evaluate_point(TerrainQuery {
                    location: SurfaceLocation::new(Direction3::try_new(axis)?),
                    footprint,
                })?
                .height_m(),
        };
        let allowance = ((variation + certificate.unresolved_height_bound_m()).next_up()
            + error.numeric_m)
            .next_up();
        extent.min_height_m = extent.min_height_m.max((height - allowance).next_down());
        extent.max_height_m = extent.max_height_m.min((height + allowance).next_up());
    }
    anyhow::ensure!(
        extent.min_height_m <= extent.max_height_m,
        "invalid regional terrain interval"
    );
    Ok((extent, error))
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
    let centre = samples
        .get(8 * 17 + 8)
        .ok_or_else(|| anyhow::anyhow!("missing terrain centre sample"))?;
    let height = centre.position_body_m.length() - generator.radius_m();
    surface_certificate(generator, address, metadata, Some(height))
}
