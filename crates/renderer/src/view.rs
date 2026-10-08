//! CPU precision boundary; no GPU/device is required to prepare a view.

use glam::DVec3;
use mundaris_math::{
    Displacement3, FrameError, FrameEvaluation, FrameId, FramePose, FramePosition, LocalPosition,
    MathError, UnitRotation,
};

/// Validated representation-specific range and actual per-component narrowing error.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderPrecisionBudget {
    max_distance_m: f64,
    max_component_error_m: f64,
}
impl RenderPrecisionBudget {
    pub fn try_new(
        max_distance_m: f64,
        max_component_error_m: f64,
    ) -> Result<Self, RenderPreparationError> {
        if !max_distance_m.is_finite()
            || !max_component_error_m.is_finite()
            || max_distance_m <= 0.0
            || max_component_error_m <= 0.0
        {
            return Err(RenderPreparationError::InvalidBudget);
        }
        Ok(Self {
            max_distance_m,
            max_component_error_m,
        })
    }
    pub fn near_debug() -> Self {
        Self {
            max_distance_m: 10_000.0,
            max_component_error_m: 1e-3,
        }
    }
    pub fn max_distance_m(self) -> f64 {
        self.max_distance_m
    }
    pub fn max_component_error_m(self) -> f64 {
        self.max_component_error_m
    }

    /// Validate an already observer-relative coordinate before a renderer
    /// stages its f64 value for f32 GPU parameters.
    pub fn try_view_relative_position(
        self,
        view_metres: DVec3,
    ) -> Result<RenderRelativePosition, RenderPreparationError> {
        narrow(view_metres, self)
    }
}

/// Disposable view-relative GPU value, only valid for the view that produced it.
/// No constructor accepts world coordinates and no conversion back to math exists.
/// ```compile_fail
/// use mundaris_math::LocalPosition;
/// use mundaris_renderer::RenderRelativePosition;
/// fn authoritative(render: RenderRelativePosition) -> LocalPosition { render.into() }
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderRelativePosition {
    view_metres: [f32; 3],
    component_error_m: f64,
}
impl RenderRelativePosition {
    pub fn gpu_xyz(self) -> [f32; 3] {
        self.view_metres
    }
    pub fn max_component_error_m(self) -> f64 {
        self.component_error_m
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RenderPreparationError {
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Math(#[from] MathError),
    #[error("render budget must have finite positive limits")]
    InvalidBudget,
    #[error("view distance {distance_m} exceeds budget {limit_m}")]
    OutsideRenderRange { distance_m: f64, limit_m: f64 },
    #[error("component narrowing error {error_m} exceeds budget {limit_m}")]
    PrecisionBudgetExceeded { error_m: f64, limit_m: f64 },
    #[error("batch lengths differ: {input} inputs, {output} outputs")]
    LengthMismatch { input: usize, output: usize },
    #[error("vertex {index}: {source}")]
    Vertex {
        index: usize,
        source: Box<RenderPreparationError>,
    },
    #[error("invalid debug projection parameters")]
    InvalidProjection,
    #[error("invalid debug color or incomplete line list")]
    InvalidDebugGeometry,
    #[error("debug frame contains a failed batch and cannot be submitted")]
    FailedDebugFrame,
    #[error("resident terrain tile data or draw parameters are invalid")]
    InvalidResidentTile,
    #[error("GPU progress during bounded resource growth: {0}")]
    GpuProgress(String),
    #[error("terrain atlas: {0}")]
    TerrainAtlas(String),
}

/// Observer-centred view with camera-local axes (+X right, +Y up, -Z forward).
/// Retains a tree borrow even if the lightweight evaluation wrapper is dropped.
/// ```compile_fail
/// use std::num::NonZeroU64;
/// use mundaris_math::*;
/// use mundaris_renderer::*;
/// let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
/// let pose = FramePose::new(FramePosition::new(tree.root(), LocalPosition::origin()), UnitRotation::identity());
/// let view = PreparedView::new(&tree.evaluate(), pose, RenderPrecisionBudget::near_debug()).unwrap();
/// tree.update_states(1.0, &[]).unwrap();
/// let _ = view.prepare_source(tree.root());
/// ```
pub struct PreparedView<'a> {
    evaluation: FrameEvaluation<'a>,
    observer: FramePose,
    budget: RenderPrecisionBudget,
}
impl<'a> PreparedView<'a> {
    pub fn new(
        evaluation: &FrameEvaluation<'a>,
        observer: FramePose,
        budget: RenderPrecisionBudget,
    ) -> Result<Self, RenderPreparationError> {
        evaluation.state(observer.position().frame())?;
        Ok(Self {
            evaluation: *evaluation,
            observer,
            budget,
        })
    }
    pub fn prepare_source(
        &self,
        source: FrameId,
    ) -> Result<PreparedRenderFrame<'a>, RenderPreparationError> {
        let observer_in_source = self
            .evaluation
            .convert_position(self.observer.position(), source)?
            .local();
        let relative_rotation = self
            .evaluation
            .prepare_conversion(source, self.observer.position().frame())?
            .rotation();
        Ok(PreparedRenderFrame {
            evaluation: self.evaluation,
            source,
            observer_in_source,
            camera_from_source: self
                .observer
                .orientation()
                .inverse()
                .compose(relative_rotation),
            budget: self.budget,
        })
    }
    pub fn budget(&self) -> RenderPrecisionBudget {
        self.budget
    }
    pub fn sample_time_s(&self) -> f64 {
        self.evaluation.sample_time_s()
    }
}

/// Source-space centering is prepared once per batch. No root flattening occurs.
pub struct PreparedRenderFrame<'a> {
    evaluation: FrameEvaluation<'a>,
    source: FrameId,
    observer_in_source: LocalPosition,
    camera_from_source: UnitRotation,
    budget: RenderPrecisionBudget,
}
impl PreparedRenderFrame<'_> {
    /// Read-only source-centred observer for body-local bounds and surface policy.
    pub fn observer_in_source(&self) -> LocalPosition {
        self.observer_in_source
    }
    /// Camera-axis unit normal/direction; no origin or translation contribution.
    pub fn view_direction(
        &self,
        direction: mundaris_math::Direction3,
    ) -> Result<mundaris_math::Direction3, RenderPreparationError> {
        Ok(self.camera_from_source.rotate_direction(direction)?)
    }
    /// High-precision view displacement for CPU diagnostics/explicit range selection.
    pub fn view_displacement(
        &self,
        point: FramePosition,
    ) -> Result<Displacement3, RenderPreparationError> {
        if point.frame() != self.source {
            return Err(FrameError::FrameMismatch {
                expected: self.source,
                actual: point.frame(),
            }
            .into());
        }
        Ok(self
            .camera_from_source
            .rotate_displacement(point.local().displacement_from(self.observer_in_source)?)?)
    }
    pub fn try_position(
        &self,
        point: FramePosition,
    ) -> Result<RenderRelativePosition, RenderPreparationError> {
        narrow(self.view_displacement(point)?.metres(), self.budget)
    }
    pub fn sample_time_s(&self) -> f64 {
        self.evaluation.sample_time_s()
    }
    /// Writes caller-owned storage without allocation on success. On first indexed
    /// failure output may be partial and must not be submitted. Rebuild for each view.
    pub fn write_positions(
        &self,
        points: &[FramePosition],
        output: &mut [RenderRelativePosition],
    ) -> Result<(), RenderPreparationError> {
        if points.len() != output.len() {
            return Err(RenderPreparationError::LengthMismatch {
                input: points.len(),
                output: output.len(),
            });
        }
        for (index, (point, output)) in points.iter().zip(output).enumerate() {
            *output =
                self.try_position(*point)
                    .map_err(|source| RenderPreparationError::Vertex {
                        index,
                        source: Box::new(source),
                    })?;
        }
        Ok(())
    }
}

fn narrow(
    view: DVec3,
    budget: RenderPrecisionBudget,
) -> Result<RenderRelativePosition, RenderPreparationError> {
    if !view.is_finite() {
        return Err(MathError::NonFinite.into());
    }
    let distance_m = view.x.hypot(view.y).hypot(view.z);
    if distance_m > budget.max_distance_m {
        return Err(RenderPreparationError::OutsideRenderRange {
            distance_m,
            limit_m: budget.max_distance_m,
        });
    }
    let gpu = view.as_vec3();
    if !gpu.is_finite() {
        return Err(MathError::ArithmeticOverflow.into());
    }
    let error_m = (gpu.as_dvec3() - view).abs().max_element();
    if error_m > budget.max_component_error_m {
        return Err(RenderPreparationError::PrecisionBudgetExceeded {
            error_m,
            limit_m: budget.max_component_error_m,
        });
    }
    Ok(RenderRelativePosition {
        view_metres: gpu.to_array(),
        component_error_m: error_m,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budgets_boundaries_and_nonfinite_input() {
        for scalar in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(RenderPrecisionBudget::try_new(scalar, 1e-3).is_err());
            assert!(RenderPrecisionBudget::try_new(100.0, scalar).is_err());
        }
        for (range, error) in [(100.0, 1e-5), (1000.0, 1e-4), (10_000.0, 1e-3)] {
            let budget = RenderPrecisionBudget::try_new(range, error).unwrap();
            assert!(narrow(DVec3::new(range, 0.0, 0.0), budget).is_ok());
            assert!(narrow(DVec3::new(range + 0.001, 0.0, 0.0), budget).is_err());
            for i in 0..1000 {
                let value = DVec3::new(range * (i as f64 / 1001.0), 0.123, -0.01);
                assert!(narrow(value, budget).unwrap().max_component_error_m() <= error);
            }
        }
        assert!(narrow(DVec3::splat(f64::NAN), RenderPrecisionBudget::near_debug()).is_err());
        assert!(
            narrow(
                DVec3::splat(f64::INFINITY),
                RenderPrecisionBudget::near_debug()
            )
            .is_err()
        );
        assert!(matches!(
            narrow(
                DVec3::new(0.1, 0.0, 0.0),
                RenderPrecisionBudget::try_new(1.0, 1e-12).unwrap()
            ),
            Err(RenderPreparationError::PrecisionBudgetExceeded { .. })
        ));
        assert!(
            narrow(
                DVec3::new(1.5e11, 0.0, 0.0),
                RenderPrecisionBudget::near_debug()
            )
            .is_err()
        );
        assert!(matches!(
            narrow(
                DVec3::new(f64::MAX, 0.0, 0.0),
                RenderPrecisionBudget::try_new(f64::MAX, 1.0).unwrap()
            ),
            Err(RenderPreparationError::Math(MathError::ArithmeticOverflow))
        ));
    }
}
