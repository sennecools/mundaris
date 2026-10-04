//! Dimensionless cap, all-stitch plane envelope and conservative curvature error.
use super::{GRID_SAMPLES, SurfaceTopology};
use crate::{CelestialProjection, RenderPreparationError};
use glam::DVec3;
use mundaris_math::surface::{CubePatchAddress, SurfaceMathError};

const FLOOR: f64 = 64.0 * f64::EPSILON;

/// Domain-free metre contributions against full terrain, not just filtered
/// samples. Values must already be conservative for the actual stitch/overlay
/// topology. This record does not manufacture a certificate from sampled extrema.
#[derive(Debug, Clone, Copy, Default)]
pub struct SurfaceErrorContributions {
    pub sphere_m: f64,
    pub filtered_interpolation_m: f64,
    pub unresolved_m: f64,
    pub boundary_constraint_m: f64,
    pub morph_remaining_m: f64,
    pub numeric_m: f64,
}
impl SurfaceErrorContributions {
    pub fn total_m(self) -> Result<f64, RenderPreparationError> {
        let terms = [
            self.sphere_m,
            self.filtered_interpolation_m,
            self.unresolved_m,
            self.boundary_constraint_m,
            self.morph_remaining_m,
            self.numeric_m,
        ];
        if !terms.iter().all(|x| x.is_finite() && *x >= 0.0) {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let total = terms.into_iter().fold(
            0.0,
            |sum, x| {
                if x == 0.0 { sum } else { (sum + x).next_up() }
            },
        );
        if !total.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        Ok(total)
    }
    /// M bounds the vector-valued displacement Hessian in the same face-domain
    /// parameter used for triangle barycentrics. D bounds every allowed triangle's
    /// diameter. A height-only or regular-grid-only M/D is not sufficient.
    pub fn interpolation_bound_m(
        vector_hessian_bound_m: f64,
        triangle_diameter: f64,
    ) -> Result<f64, RenderPreparationError> {
        if !vector_hessian_bound_m.is_finite()
            || vector_hessian_bound_m < 0.0
            || !triangle_diameter.is_finite()
            || triangle_diameter < 0.0
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        if vector_hessian_bound_m == 0.0 || triangle_diameter == 0.0 {
            return Ok(0.0);
        }
        // Taylor expansion about the barycentric point cancels linear terms.
        // The remainder is <= M/2 * sum_i lambda_i |v_i - x|².
        // Variance = sum_{i<j} lambda_i lambda_j |v_i-v_j|², and for
        // three weights sum_{i<j} lambda_i lambda_j <= 1/3. Thus M*D²/6
        // bounds arbitrary triangles (the equilateral centroid is sharp).
        // Round each nonnegative operation outward, including subnormal results.
        let bound = (vector_hessian_bound_m / 6.0).next_up();
        let bound = (bound * triangle_diameter).next_up();
        let bound = (bound * triangle_diameter).next_up();
        if !bound.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        Ok(bound)
    }
}
pub(crate) fn angle(a: DVec3, b: DVec3) -> f64 {
    a.cross(b).length().atan2(a.dot(b))
}
/// Future displacement/occlusion boundary; smooth sphere uses zero height.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceExtent {
    pub min_height_m: f64,
    pub max_height_m: f64,
    pub guaranteed_opaque_radius_m: f64,
}
impl SurfaceExtent {
    /// Zero displacement. No independent opaque occluder is claimed here: the
    /// chordal horizon path certifies its cached geometric plane envelope instead.
    pub fn smooth(_reference_radius_m: f64) -> Self {
        Self {
            min_height_m: 0.0,
            max_height_m: 0.0,
            guaranteed_opaque_radius_m: 0.0,
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct PatchMetadata {
    axis: DVec3,
    alpha: f64,
    error: f64,
    normal_beta: f64,
    c_min: f64,
    horizon_available: bool,
}
impl PatchMetadata {
    pub fn build(
        patch: CubePatchAddress,
        topology: &SurfaceTopology,
    ) -> Result<Self, SurfaceMathError> {
        let mut samples = [DVec3::ZERO; GRID_SAMPLES];
        for (index, p) in samples.iter_mut().enumerate() {
            *p = patch
                .sample_direction(index as u32 % 17, index as u32 / 17, 16)?
                .unit();
        }
        let axis = patch.sample_direction(8, 8, 16)?.unit();
        let alpha = [0, 16, 272, 288]
            .into_iter()
            .map(|i| angle(axis, samples[i]))
            .fold(0.0, f64::max)
            + FLOOR;
        let mut normal_beta: f64 = 0.0;
        let mut c_min: f64 = 1.0;
        let mut horizon_available = true;
        for &[i, j, k] in topology.triangles() {
            let [a, b, c] = [i, j, k].map(|i| samples[usize::from(i)]);
            let ab = b - a;
            let ac = c - a;
            let cross = ab.cross(ac);
            let norm = cross.length();
            // Unit sample component error <= 16eps, edge subtraction <= 2eps.
            // Cross error includes both operand perturbations and six products.
            // Normalization error grows as edge length / cross magnitude at depth.
            let operand_error = 36.0 * f64::EPSILON;
            let cross_error = operand_error * (ab.length() + ac.length())
                + 3.0 * operand_error.powi(2)
                + 16.0 * f64::EPSILON * ab.length() * ac.length();
            if norm <= cross_error * 4.0 || !norm.is_finite() {
                horizon_available = false;
                continue;
            }
            let m = cross / norm;
            let allowance = 2.0 * cross_error / (norm - cross_error) + FLOOR;
            normal_beta = normal_beta.max(angle(axis, m) + allowance);
            let c = a.dot(m).min(b.dot(m)).min(c.dot(m)) - allowance - FLOOR;
            c_min = c_min.min(c);
        }
        // Both bounds are valid; the analytic bound prevents normal cancellation
        // from spuriously making deep curvature error grow like inverse grid size.
        let delta = 0.5 / (1u64 << patch.level()) as f64;
        let analytic = 2.0 * (delta * 0.5).sin().powi(2) + FLOOR;
        let error = (1.0 - c_min).max(FLOOR).min(analytic);
        horizon_available &= normal_beta < std::f64::consts::FRAC_PI_2 && c_min > 0.0;
        Ok(Self {
            axis,
            alpha,
            error,
            normal_beta,
            c_min,
            horizon_available,
        })
    }
    pub fn error_unit(self) -> f64 {
        self.error
    }
    pub fn cap(self) -> (DVec3, f64) {
        (self.axis, self.alpha)
    }
    pub fn normal_envelope(self) -> Option<(DVec3, f64, f64)> {
        self.horizon_available
            .then_some((self.axis, self.normal_beta, self.c_min))
    }
    pub fn ball(
        self,
        radius_m: f64,
        extent: SurfaceExtent,
    ) -> Result<(DVec3, f64), RenderPreparationError> {
        if !radius_m.is_finite()
            || radius_m <= 0.0
            || ![
                extent.min_height_m,
                extent.max_height_m,
                extent.guaranteed_opaque_radius_m,
            ]
            .iter()
            .all(|x| x.is_finite())
            || extent.min_height_m > extent.max_height_m
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        // Recenter the reference cap at the interval midpoint. A radial sample
        // is within R_mid*sin(alpha) + half_width of this centre; convex drawn
        // triangles remain inside the same ball. Absolute elevation must not
        // act like uncertainty when a regional interval becomes narrow.
        let midpoint = extent.min_height_m * 0.5 + extent.max_height_m * 0.5;
        let effective_radius = radius_m + midpoint;
        if effective_radius <= 0.0 {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let half_width = (extent.max_height_m - midpoint)
            .max(midpoint - extent.min_height_m)
            .next_up();
        let center = self.axis * (effective_radius * self.alpha.cos());
        let radius = (effective_radius * self.alpha.sin()
            + half_width
            + (radius_m + midpoint.abs() + half_width) * FLOOR)
            .next_up();
        if !center.is_finite() || !radius.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        Ok((center, radius))
    }
    /// Plane-normal-aware smooth chordal rejection; arbitrary displacement disables it.
    pub fn horizon_reject(self, observer: DVec3, radius: f64, extent: SurfaceExtent) -> bool {
        let d = observer.length();
        if !self.horizon_available
            || !d.is_finite()
            || d <= radius
            || extent.min_height_m != 0.0
            || extent.max_height_m != 0.0
        {
            return false;
        }
        let theta = angle(observer / d, self.axis);
        let max_dot = d * (theta - self.normal_beta).max(0.0).cos();
        max_dot < radius * self.c_min - (1e-7_f64.max(FLOOR * (d + radius)))
    }
    pub fn projected_error(
        self,
        center_view: DVec3,
        ball_radius: f64,
        radius: f64,
        projection: CelestialProjection,
    ) -> f64 {
        projected_error(self.error * radius, center_view, ball_radius, projection)
    }
    /// Project a caller-certified complete terrain error using the existing
    /// conservative projection and displaced/transition-expanded ball.
    pub fn projected_total_error(
        self,
        error: SurfaceErrorContributions,
        center_view: DVec3,
        ball_radius: f64,
        projection: CelestialProjection,
    ) -> Result<f64, RenderPreparationError> {
        if !center_view.is_finite() || !ball_radius.is_finite() || ball_radius < 0.0 {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        Ok(projected_error(
            error.total_m()?,
            center_view,
            ball_radius,
            projection,
        ))
    }
}
pub(crate) fn projected_error(
    error: f64,
    center: DVec3,
    ball_radius: f64,
    projection: CelestialProjection,
) -> f64 {
    let z0 = (-center.z - ball_radius).max(projection.near_m());
    if error >= z0 * 0.5 {
        return f64::INFINITY;
    }
    let z = z0 - error;
    let [w, h] = projection.viewport();
    let ty = (projection.vertical_fov_rad() * 0.5).tan();
    let tx = ty * w as f64 / h as f64;
    let lever = (1.0 + (tx + error / z).powi(2) + (ty + error / z).powi(2)).sqrt();
    projection.focal_pixels() * error / z * lever
}
