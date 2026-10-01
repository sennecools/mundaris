//! Unsoftened deterministic unordered pairs. Output scratch is partial on failure.

use glam::DVec3;

/// CODATA 2018 central value, fixed for this model, in SI units.
pub const GRAVITATIONAL_CONSTANT_M3_KG_S2: f64 = 6.67430e-11;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GravityError {
    #[error("gravity slice lengths differ")]
    LengthMismatch,
    #[error("invalid positive finite mass at dense index {0}")]
    InvalidMass(usize),
    #[error("nonfinite position at dense index {0}")]
    InvalidPosition(usize),
    #[error("coincident body centres: {first}, {second}")]
    CoincidentBodies { first: usize, second: usize },
    #[error("unrepresentable gravity: pair {first}, {second}")]
    UnrepresentableGravity { first: usize, second: usize },
    #[error("unresolved encounter: pair {first}, {second}, chi={chi}")]
    UnresolvedEncounter {
        first: usize,
        second: usize,
        chi: f64,
    },
    #[error("invalid positive finite resolution step")]
    InvalidStep,
    #[error("pair count overflow")]
    PairCountOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GravityEvaluationReport {
    pub pair_evaluations: u64,
    pub max_chi: f64,
}

pub fn pair_count(bodies: usize) -> Result<u64, GravityError> {
    let n = bodies as u128;
    u64::try_from(n * n.saturating_sub(1) / 2).map_err(|_| GravityError::PairCountOverflow)
}

pub(crate) fn distance(value: DVec3) -> f64 {
    value.x.hypot(value.y).hypot(value.z)
}

/// Validates raw input; zeroes outputs, then lexicographic i<j accumulation.
/// A failed output must never be published. No radius, frames, wall clock or softening.
pub fn evaluate_accelerations(
    masses_kg: &[f64],
    positions_m: &[DVec3],
    output_m_s2: &mut [DVec3],
) -> Result<GravityEvaluationReport, GravityError> {
    evaluate(masses_kg, positions_m, output_m_s2, None)
}

/// The same force pass with the radius-independent chi<=0.02 session envelope.
pub fn evaluate_resolved_accelerations(
    masses_kg: &[f64],
    positions_m: &[DVec3],
    output_m_s2: &mut [DVec3],
    step_s: f64,
) -> Result<GravityEvaluationReport, GravityError> {
    if !step_s.is_finite() || step_s <= 0.0 {
        return Err(GravityError::InvalidStep);
    }
    evaluate(masses_kg, positions_m, output_m_s2, Some(step_s))
}

fn evaluate(
    masses: &[f64],
    positions: &[DVec3],
    output: &mut [DVec3],
    step: Option<f64>,
) -> Result<GravityEvaluationReport, GravityError> {
    if masses.len() != positions.len() || masses.len() != output.len() {
        return Err(GravityError::LengthMismatch);
    }
    let pairs = pair_count(masses.len())?;
    for (i, (&mass, position)) in masses.iter().zip(positions).enumerate() {
        if !mass.is_finite() || mass <= 0.0 {
            return Err(GravityError::InvalidMass(i));
        }
        if !position.is_finite() {
            return Err(GravityError::InvalidPosition(i));
        }
    }
    output.fill(DVec3::ZERO);
    let mut max_chi: f64 = 0.0;
    for i in 0..masses.len() {
        for j in i + 1..masses.len() {
            let bad = || GravityError::UnrepresentableGravity {
                first: i,
                second: j,
            };
            let delta = positions[j] - positions[i];
            let r = distance(delta);
            if r == 0.0 {
                return Err(GravityError::CoincidentBodies {
                    first: i,
                    second: j,
                });
            }
            if !delta.is_finite() || !r.is_normal() {
                return Err(bad());
            }
            let unit = delta / r;
            let scalar = (GRAVITATIONAL_CONSTANT_M3_KG_S2 / r) / r;
            if !scalar.is_finite() || scalar == 0.0 {
                return Err(bad());
            }
            let ai = scalar * masses[j];
            let aj = scalar * masses[i];
            if !ai.is_finite() || !aj.is_finite() || ai == 0.0 || aj == 0.0 {
                return Err(bad());
            }
            let ci = unit * ai;
            let cj = unit * aj;
            if !ci.is_finite() || !cj.is_finite() {
                return Err(bad());
            }
            for axis in 0..3 {
                if delta[axis] != 0.0 && (unit[axis] == 0.0 || ci[axis] == 0.0 || cj[axis] == 0.0) {
                    return Err(bad());
                }
            }
            if let Some(h) = step {
                // sqrt before multiplying avoids mass-sum and r^3 overflow. ai/aj
                // are already checked; hypot avoids squaring frequency components.
                let frequency = ai.sqrt().hypot(aj.sqrt()) / r.sqrt();
                let chi = h * frequency;
                if !chi.is_finite() || chi > 0.02 {
                    return Err(GravityError::UnresolvedEncounter {
                        first: i,
                        second: j,
                        chi,
                    });
                }
                max_chi = max_chi.max(chi);
            }
            output[i] += ci;
            output[j] -= cj;
            if !output[i].is_finite() || !output[j].is_finite() {
                return Err(bad());
            }
        }
    }
    Ok(GravityEvaluationReport {
        pair_evaluations: pairs,
        max_chi,
    })
}
