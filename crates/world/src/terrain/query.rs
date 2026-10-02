use super::TerrainError;
use glam::DVec3;
use mundaris_math::{
    Direction3,
    surface::{DirectionalCap, SurfaceLocation},
};

/// Maximum adjacent reference-sphere sample distance, not camera distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainFootprint(f64);
impl TerrainFootprint {
    pub const COMPLETE: Self = Self(0.0);
    pub fn new(metres: f64) -> Result<Self, TerrainError> {
        if !metres.is_finite() || metres < 0.0 {
            return Err(TerrainError::InvalidFootprint);
        }
        Ok(Self(if metres == 0.0 { 0.0 } else { metres }))
    }
    pub fn metres(self) -> f64 {
        self.0
    }
    /// Caller supplies a certified derivative-aware effective wavelength. A
    /// nominal warped/ridged octave wavelength alone is not a certificate.
    pub fn octave_weight(self, effective_wavelength_m: f64) -> Result<f64, TerrainError> {
        if !effective_wavelength_m.is_finite() || effective_wavelength_m <= 0.0 {
            return Err(TerrainError::InvalidConfig);
        }
        if self.0 == 0.0 {
            return Ok(1.0);
        }
        let ratio = effective_wavelength_m / self.0;
        Ok(if ratio <= 4.0 {
            0.0
        } else if ratio >= 8.0 {
            1.0
        } else {
            let t = (ratio - 4.0) * 0.25;
            t * t * (3.0 - 2.0 * t)
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TerrainQuery {
    pub location: SurfaceLocation,
    pub footprint: TerrainFootprint,
}

/// Tangent derivative in body axes, metres per unit-direction change.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct TerrainSample {
    height_m: f64,
    tangent_gradient_m_per_unit_direction: DVec3,
}
impl TerrainSample {
    pub(crate) fn from_parts(height_m: f64, gradient: DVec3) -> Self {
        Self {
            height_m,
            tangent_gradient_m_per_unit_direction: gradient,
        }
    }
    pub fn height_m(self) -> f64 {
        self.height_m
    }
    pub fn tangent_gradient_m_per_unit_direction(self) -> DVec3 {
        self.tangent_gradient_m_per_unit_direction
    }
    pub fn normal_body(
        self,
        location: SurfaceLocation,
        radius_m: f64,
    ) -> Result<Direction3, TerrainError> {
        let radial = checked_radius(radius_m, self.height_m.abs())? + self.height_m;
        Direction3::try_new(
            location.direction().unit() - self.tangent_gradient_m_per_unit_direction / radial,
        )
        .map_err(|_| TerrainError::NonFiniteResult)
    }
    pub fn slope_angle_rad(self, radius_m: f64) -> Result<f64, TerrainError> {
        let radial = checked_radius(radius_m, self.height_m.abs())? + self.height_m;
        Ok((self.tangent_gradient_m_per_unit_direction.length() / radial).atan())
    }
}
fn checked_radius(radius_m: f64, absolute_height_m: f64) -> Result<f64, TerrainError> {
    if !radius_m.is_finite()
        || radius_m > 1e8
        || radius_m <= 0.0
        || absolute_height_m > 0.1 * radius_m
        || radius_m - absolute_height_m <= (64.0 * f64::EPSILON * radius_m).max(1e-3)
    {
        return Err(TerrainError::InvalidRadius);
    }
    Ok(radius_m)
}

/// Independent analytical fixtures for certificate/transition validation before
/// artistic V1 integration. These do not masquerade as the V1 procedural field.
#[derive(Debug, Clone, Copy)]
pub struct AnalyticTerrain {
    field: AnalyticField,
    radius_m: f64,
}
#[derive(Debug, Clone, Copy)]
enum AnalyticField {
    Constant(f64),
    Linear { amplitude_m: f64, axis: Direction3 },
}
impl AnalyticTerrain {
    /// Exact cap interval with outward numeric allowance. Analytic bounds do not
    /// depend on mesh samples or a camera; no unresolved fixture bands exist.
    pub fn bounds_for_region(self, cap: DirectionalCap) -> AnalyticTerrainCertificate {
        let (interval, gradient_bound) = match self.field {
            AnalyticField::Constant(h) => ([h, h], 0.0),
            AnalyticField::Linear { amplitude_m, axis } => {
                let a = cap.axis().unit();
                let k = axis.unit();
                let theta = a.cross(k).length().atan2(a.dot(k));
                let angle_margin = 128.0 * f64::EPSILON;
                let lo = (theta + cap.half_angle_rad() + angle_margin)
                    .min(std::f64::consts::PI)
                    .cos();
                let hi = (theta - cap.half_angle_rad() - angle_margin).max(0.0).cos();
                let ends = [amplitude_m * lo, amplitude_m * hi];
                let margin = (amplitude_m.abs() * 256.0 * f64::EPSILON).next_up();
                (
                    [
                        (ends[0].min(ends[1]).next_down() - margin).next_down(),
                        (ends[0].max(ends[1]).next_up() + margin).next_up(),
                    ],
                    if amplitude_m == 0.0 {
                        0.0
                    } else {
                        (amplitude_m.abs() * (1.0 + 128.0 * f64::EPSILON)).next_up()
                    },
                )
            }
        };
        AnalyticTerrainCertificate {
            min_height_m: interval[0],
            max_height_m: interval[1],
            cartesian_height_gradient_bound_m: gradient_bound,
        }
    }
    pub fn constant(radius_m: f64, height_m: f64) -> Result<Self, TerrainError> {
        if !height_m.is_finite() {
            return Err(TerrainError::InvalidConfig);
        }
        checked_radius(radius_m, height_m.abs())?;
        Ok(Self {
            field: AnalyticField::Constant(if height_m == 0.0 { 0.0 } else { height_m }),
            radius_m,
        })
    }
    /// h(n)=A(n·k); exact tangent derivative A(k−n(n·k)).
    pub fn linear(radius_m: f64, amplitude_m: f64, axis: Direction3) -> Result<Self, TerrainError> {
        if !amplitude_m.is_finite() {
            return Err(TerrainError::InvalidConfig);
        }
        checked_radius(radius_m, amplitude_m.abs())?;
        if amplitude_m == 0.0 {
            return Self::constant(radius_m, 0.0);
        }
        Ok(Self {
            field: AnalyticField::Linear { amplitude_m, axis },
            radius_m,
        })
    }
    pub fn radius_m(self) -> f64 {
        self.radius_m
    }
    pub fn evaluate_point(self, query: TerrainQuery) -> TerrainSample {
        self.evaluate(query.location)
    }
    fn evaluate(self, location: SurfaceLocation) -> TerrainSample {
        let n = location.direction().unit();
        match self.field {
            AnalyticField::Constant(height_m) => TerrainSample {
                height_m,
                tangent_gradient_m_per_unit_direction: DVec3::ZERO,
            },
            AnalyticField::Linear { amplitude_m, axis } => {
                let k = axis.unit();
                let dot = n.dot(k);
                TerrainSample {
                    height_m: amplitude_m * dot,
                    tangent_gradient_m_per_unit_direction: amplitude_m * (k - n * dot),
                }
            }
        }
    }
    /// Same fixed-order core as scalar evaluation. Caller-owned output; no heap
    /// allocation, frame/camera access or hidden numerical differentiation.
    pub fn evaluate_batch(
        self,
        locations: &[SurfaceLocation],
        _footprint: TerrainFootprint,
        output: &mut [TerrainSample],
    ) -> Result<(), TerrainError> {
        if locations.len() != output.len() {
            return Err(TerrainError::LengthMismatch);
        }
        for (location, sample) in locations.iter().zip(output) {
            *sample = self.evaluate(*location);
        }
        Ok(())
    }
    /// Exact global interval; independent of the sampled mesh and footprint.
    pub fn height_interval_m(self) -> [f64; 2] {
        match self.field {
            AnalyticField::Constant(h) => [h, h],
            AnalyticField::Linear { amplitude_m, .. } => {
                let a = if amplitude_m == 0.0 {
                    0.0
                } else {
                    (amplitude_m.abs() * (1.0 + 128.0 * f64::EPSILON)).next_up()
                };
                [-a, a]
            }
        }
    }
}

/// Analytic fixture's ambient extension is h(x)=c or A(k·x), so its Cartesian
/// height Hessian is exactly zero. Representation code must still include the
/// mapping derivatives and height-direction coupling for h(n)n.
#[derive(Debug, Clone, Copy)]
pub struct AnalyticTerrainCertificate {
    min_height_m: f64,
    max_height_m: f64,
    cartesian_height_gradient_bound_m: f64,
}
impl AnalyticTerrainCertificate {
    pub fn height_interval_m(self) -> [f64; 2] {
        [self.min_height_m, self.max_height_m]
    }
    pub fn cartesian_height_gradient_bound_m(self) -> f64 {
        self.cartesian_height_gradient_bound_m
    }
    pub fn cartesian_height_hessian_bound_m(self) -> f64 {
        0.0
    }
    pub fn unresolved_height_bound_m(self) -> f64 {
        0.0
    }
}
