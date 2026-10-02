//! Domain-free reusable body-fixed terrain geometry.
use crate::RenderPreparationError;
use glam::DVec3;
use mundaris_math::surface::CubePatchAddress;

use super::{GRID_SAMPLES, SurfaceErrorContributions, SurfaceExtent};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceGeometrySample {
    pub position_body_m: DVec3,
    pub normal_body: DVec3,
}

/// Validated, disposable grid16 geometry produced outside the renderer.
#[derive(Debug, Clone)]
pub struct GeneratedSurfacePatch {
    address: CubePatchAddress,
    reference_radius_m: f64,
    footprint_m: f64,
    samples: Box<[SurfaceGeometrySample]>,
    extent: SurfaceExtent,
    error: SurfaceErrorContributions,
}

impl GeneratedSurfacePatch {
    pub fn new(
        address: CubePatchAddress,
        reference_radius_m: f64,
        footprint_m: f64,
        samples: Vec<SurfaceGeometrySample>,
        extent: SurfaceExtent,
        error: SurfaceErrorContributions,
    ) -> Result<Self, RenderPreparationError> {
        if !reference_radius_m.is_finite()
            || reference_radius_m <= 0.0
            || reference_radius_m > 1.0e8
            || !footprint_m.is_finite()
            || footprint_m < 0.0
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        if samples.len() != GRID_SAMPLES
            || ![
                extent.min_height_m,
                extent.max_height_m,
                extent.guaranteed_opaque_radius_m,
            ]
            .iter()
            .all(|value| value.is_finite())
            || extent.guaranteed_opaque_radius_m < 0.0
            || extent.min_height_m > extent.max_height_m
            || reference_radius_m + extent.min_height_m
                <= 1.0e-3_f64.max(64.0 * f64::EPSILON * reference_radius_m)
            || extent.min_height_m.abs().max(extent.max_height_m.abs()) > 0.1 * reference_radius_m
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        error.total_m()?;
        let radial_margin = 64.0 * f64::EPSILON * reference_radius_m.max(1.0);
        for (index, sample) in samples.iter().enumerate() {
            let direction = address
                .sample_direction(index as u32 % 17, index as u32 / 17, 16)
                .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?
                .unit();
            if !sample.position_body_m.is_finite()
                || !sample.normal_body.is_finite()
                || (sample.normal_body.length_squared() - 1.0).abs() > 1.0e-8
                || sample.position_body_m.dot(sample.normal_body) <= 0.0
                || sample.position_body_m.length_squared() == 0.0
                || sample.position_body_m.normalize().dot(direction) < 1.0 - 1.0e-12
            {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            let radius = sample.position_body_m.length();
            if radius < reference_radius_m + extent.min_height_m - radial_margin
                || radius > reference_radius_m + extent.max_height_m + radial_margin
            {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
        }
        Ok(Self {
            address,
            reference_radius_m,
            footprint_m,
            samples: samples.into_boxed_slice(),
            extent,
            error,
        })
    }
    pub fn address(&self) -> CubePatchAddress {
        self.address
    }
    pub fn reference_radius_m(&self) -> f64 {
        self.reference_radius_m
    }
    pub fn footprint_m(&self) -> f64 {
        self.footprint_m
    }
    pub fn samples(&self) -> &[SurfaceGeometrySample] {
        &self.samples
    }
    pub fn extent(&self) -> SurfaceExtent {
        self.extent
    }
    pub fn error(&self) -> SurfaceErrorContributions {
        self.error
    }
    pub fn resident_heap_bytes(&self) -> usize {
        self.samples.len() * std::mem::size_of::<SurfaceGeometrySample>()
    }
    pub fn resident_heap_capacity_bytes(&self) -> usize {
        self.samples.len() * std::mem::size_of::<SurfaceGeometrySample>()
    }
}
