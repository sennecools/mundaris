//! Read-only instantaneous relative two-body conics, never authoritative propagation.
use crate::{GRAVITATIONAL_CONSTANT_M3_KG_S2 as G, gravity::distance};
use glam::DVec3;

/// Dimensionless normalized-energy and angular-momentum classification thresholds.
pub const NEAR_PARABOLIC_ENERGY: f64 = 1e-10;
pub const DEGENERATE_ANGULAR_MOMENTUM: f64 = 1e-12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConicClass {
    Elliptic,
    Hyperbolic,
    NearParabolic,
    Degenerate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum OrbitalElementError {
    #[error("invalid or unrepresentable two-body state")]
    InvalidState,
    #[error("ill-conditioned energy/eccentricity consistency")]
    IllConditioned,
    #[error("closed elliptical guide unavailable for this conic")]
    NotElliptic,
}
/// Checked geometry in system-aligned axes; reference is at a focus.
#[derive(Debug, Clone, Copy)]
pub struct TwoBodyElements {
    class: ConicClass,
    a: Option<f64>,
    e: f64,
    p: f64,
    normal: DVec3,
    periapsis: DVec3,
    transverse: DVec3,
    periapsis_defined: bool,
}
pub fn osculating_elements(
    r: DVec3,
    v: DVec3,
    m1: f64,
    m2: f64,
) -> Result<TwoBodyElements, OrbitalElementError> {
    use OrbitalElementError::*;
    if !r.is_finite()
        || !v.is_finite()
        || !m1.is_finite()
        || !m2.is_finite()
        || m1 <= 0.0
        || m2 <= 0.0
    {
        return Err(InvalidState);
    }
    let radius = distance(r);
    // Multiply before summing avoids overflow of m1+m2 when mu is representable.
    let mu = G * m1 + G * m2;
    let potential = mu / radius;
    if !radius.is_normal() || !mu.is_normal() || !potential.is_normal() {
        return Err(InvalidState);
    }
    let radial = r / radius;
    let scaled_v = v / potential.sqrt();
    let speed = distance(scaled_v);
    let h = radial.cross(scaled_v);
    let hn = distance(h);
    let kinetic = 0.5 * speed * speed;
    let energy = (kinetic - 1.0) / (kinetic + 1.0);
    let ev = scaled_v.cross(h) - radial;
    let e = distance(ev);
    let p = radius * hn * hn;
    if ![speed, hn, kinetic, energy, e, p]
        .iter()
        .all(|x| x.is_finite())
    {
        return Err(InvalidState);
    }
    let class = if speed == 0.0 || hn <= DEGENERATE_ANGULAR_MOMENTUM * speed {
        ConicClass::Degenerate
    } else if energy.abs() <= NEAR_PARABOLIC_ENERGY {
        ConicClass::NearParabolic
    } else if energy < 0.0 {
        ConicClass::Elliptic
    } else {
        ConicClass::Hyperbolic
    };
    let consistency = 1.0 + 2.0 * (kinetic - 1.0) * hn * hn;
    if (e * e - consistency).abs() > 1e-9 * (1.0 + e * e)
        || (class == ConicClass::Elliptic && (e >= 1.0 || p <= 0.0))
        || (class == ConicClass::Hyperbolic && e <= 1.0)
    {
        return Err(IllConditioned);
    }
    let a = if class == ConicClass::Elliptic {
        Some(radius / (2.0 * (1.0 - kinetic)))
    } else {
        None
    };
    if a.is_some_and(|x| !x.is_normal()) {
        return Err(InvalidState);
    }
    let normal = if hn > 0.0 { h / hn } else { DVec3::ZERO };
    let periapsis_defined = e > 1e-10;
    let periapsis = if periapsis_defined { ev / e } else { radial };
    Ok(TwoBodyElements {
        class,
        a,
        e,
        p,
        normal,
        periapsis,
        transverse: normal.cross(periapsis),
        periapsis_defined,
    })
}
impl TwoBodyElements {
    pub fn class(self) -> ConicClass {
        self.class
    }
    pub fn semi_major_axis_m(self) -> Option<f64> {
        self.a
    }
    pub fn eccentricity(self) -> f64 {
        self.e
    }
    pub fn semilatus_rectum_m(self) -> f64 {
        self.p
    }
    pub fn normal(self) -> DVec3 {
        self.normal
    }
    pub fn periapsis_defined(self) -> bool {
        self.periapsis_defined
    }
    pub fn elliptic_position_m(self, anomaly: f64) -> Result<DVec3, OrbitalElementError> {
        let a = self.a.ok_or(OrbitalElementError::NotElliptic)?;
        let point = self.periapsis * (a * (anomaly.cos() - self.e))
            + self.transverse * (a * (1.0 - self.e * self.e).sqrt() * anomaly.sin());
        if point.is_finite() {
            Ok(point)
        } else {
            Err(OrbitalElementError::InvalidState)
        }
    }
    /// Exact Cartesian extrema; independent of tessellation density.
    pub fn elliptic_bounds_m(self) -> Result<[DVec3; 2], OrbitalElementError> {
        let a = self.a.ok_or(OrbitalElementError::NotElliptic)?;
        let center = -a * self.e * self.periapsis;
        let minor = a * (1.0 - self.e * self.e).sqrt() * self.transverse;
        let major = a * self.periapsis;
        let extent = DVec3::new(
            major.x.hypot(minor.x),
            major.y.hypot(minor.y),
            major.z.hypot(minor.z),
        );
        let bounds = [center - extent, center + extent];
        if bounds.iter().all(|p| p.is_finite()) {
            Ok(bounds)
        } else {
            Err(OrbitalElementError::InvalidState)
        }
    }
}
