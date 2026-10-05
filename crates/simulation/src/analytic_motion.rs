//! Direct, transactional sampling of authored system-local elliptical motion.

use glam::DVec3;
use mundaris_math::{AngularVelocity3, LinearVelocity3, LocalPosition, MathError, UnitRotation};
use mundaris_world::{
    BodyState, BodyStateUpdate, CelestialMotionDefinition, CelestialSystem, CelestialSystemError,
    CelestialTranslation, SimulationInstant,
};
use std::f64::consts::{PI, TAU};

/// Working-epoch seconds and elapsed seconds are bounded to about 2178 years.
pub const ANALYTIC_TIME_LIMIT_SECONDS: f64 = 68_719_476_736.0;
/// Beyond this many cycles, the f64 time/period phase is not supported.
pub const ANALYTIC_CYCLE_LIMIT: f64 = 4_294_967_296.0;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AnalyticMotionError {
    #[error("motion definition no longer matches the system or its revision")]
    StaleBinding,
    #[error("time/phase exceeds the supported analytic envelope")]
    UnsupportedTime,
    #[error("motion arithmetic is not representable")]
    UnrepresentableArithmetic,
    #[error("Kepler solver failed after {iterations} iterations")]
    Convergence { iterations: u32 },
    #[error("solver iteration limit must be in 1..=128")]
    InvalidIterationLimit,
    #[error(transparent)]
    Math(#[from] MathError),
    #[error(transparent)]
    System(#[from] CelestialSystemError),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MotionSampleStats {
    pub body_count: usize,
    pub solver_iterations: u64,
    pub maximum_solver_iterations: u32,
}

/// Concrete producer, not a gravity backend. Owns immutable world-domain authoring
/// and reusable full-state scratch. Every external celestial revision invalidates
/// this binding (terrain-only changes do not); construct a new producer to rebind.
pub struct AnalyticMotionProducer {
    definition: CelestialMotionDefinition,
    expected_revision: u64,
    candidates: Vec<BodyStateUpdate>,
    iteration_limit: u32,
}
impl AnalyticMotionProducer {
    pub fn new(
        system: &CelestialSystem,
        definition: CelestialMotionDefinition,
    ) -> Result<Self, AnalyticMotionError> {
        Self::with_iteration_limit(system, definition, 64)
    }
    /// A smaller limit is useful for bounded-work callers and explicit failure tests.
    pub fn with_iteration_limit(
        system: &CelestialSystem,
        definition: CelestialMotionDefinition,
        iteration_limit: u32,
    ) -> Result<Self, AnalyticMotionError> {
        if !(1..=128).contains(&iteration_limit) {
            return Err(AnalyticMotionError::InvalidIterationLimit);
        }
        if !definition.is_compatible_with(system) {
            return Err(AnalyticMotionError::StaleBinding);
        }
        let mut candidates = Vec::with_capacity(system.body_count());
        for motion in definition.definitions() {
            let body = system
                .body(motion.body)
                .map_err(|_| AnalyticMotionError::StaleBinding)?;
            candidates.push(BodyStateUpdate {
                body: motion.body,
                state: *body.state(),
            });
        }
        Ok(Self {
            definition,
            expected_revision: system.revision(),
            candidates,
            iteration_limit,
        })
    }
    pub fn definition(&self) -> &CelestialMotionDefinition {
        &self.definition
    }

    /// No integration/replay; one O(N) candidate pass followed by one complete commit.
    /// Failure leaves authority/revision/time unchanged, including on solver exhaustion.
    pub fn sample(
        &mut self,
        system: &mut CelestialSystem,
        time: SimulationInstant,
    ) -> Result<MotionSampleStats, AnalyticMotionError> {
        if system.revision() != self.expected_revision
            || !self.definition.is_compatible_with(system)
        {
            return Err(AnalyticMotionError::StaleBinding);
        }
        checked_elapsed(time, time)?;
        let mut stats = MotionSampleStats {
            body_count: self.candidates.len(),
            ..Default::default()
        };
        for &index in self.definition.evaluation_order() {
            let motion = self.definition.definitions()[index];
            let (center, velocity) = match motion.translation {
                CelestialTranslation::Stationary(center) => (center.metres(), DVec3::ZERO),
                CelestialTranslation::Elliptic(orbit) => {
                    let elapsed = checked_elapsed(time, orbit.epoch())?;
                    let mean = principal(
                        orbit.mean_anomaly_at_epoch_radians()
                            + periodic_phase(elapsed, orbit.period_seconds())?,
                    );
                    let (anomaly, iterations) =
                        solve_kepler(mean, orbit.eccentricity(), self.iteration_limit)?;
                    stats.solver_iterations += u64::from(iterations);
                    stats.maximum_solver_iterations =
                        stats.maximum_solver_iterations.max(iterations);
                    let e = orbit.eccentricity();
                    let a = orbit.semi_major_axis_m();
                    let beta = ((1.0 - e) * (1.0 + e)).sqrt();
                    let half_sin = (0.5 * anomaly).sin();
                    let denominator = (1.0 - e) + 2.0 * e * half_sin * half_sin;
                    let speed_scale = (a / orbit.period_seconds()) * TAU;
                    if !speed_scale.is_finite()
                        || speed_scale <= 0.0
                        || speed_scale * beta == 0.0
                        || a * (1.0 - e) == 0.0
                        || a * beta == 0.0
                    {
                        return Err(AnalyticMotionError::UnrepresentableArithmetic);
                    }
                    let (sin_e, cos_e) = anomaly.sin_cos();
                    let local_center = DVec3::new(
                        a * ((1.0 - e) - 2.0 * half_sin * half_sin),
                        a * beta * sin_e,
                        0.0,
                    );
                    let local_velocity = DVec3::new(
                        -speed_scale * sin_e / denominator,
                        speed_scale * beta * cos_e / denominator,
                        0.0,
                    );
                    let parent_index = self.definition.reference_indices()[index]
                        .ok_or(AnalyticMotionError::StaleBinding)?;
                    let parent = self.candidates[parent_index].state;
                    let rotation = orbit.plane_to_system().quaternion();
                    (
                        parent.center_in_system().metres() + rotation * local_center,
                        parent.center_velocity_in_system().metres_per_second()
                            + rotation * local_velocity,
                    )
                }
            };
            let spin = motion.spin;
            let elapsed = checked_elapsed(time, spin.epoch())?;
            let rate = spin.rate_radians_per_second();
            let angle = spin_phase(elapsed, rate)?;
            let initial = spin.orientation_at_epoch();
            let (orientation, angular) = if rate == 0.0 {
                (initial, DVec3::ZERO)
            } else {
                (
                    initial.compose(UnitRotation::from_axis_angle(spin.axis_in_body(), angle)?),
                    initial.quaternion() * (spin.axis_in_body().unit() * rate),
                )
            };
            self.candidates[index].state = BodyState::new(
                LocalPosition::try_metres(center)?,
                LinearVelocity3::try_metres_per_second(velocity)?,
                orientation,
                AngularVelocity3::try_radians_per_second(angular)?,
            );
        }
        system.update_states(time, &self.candidates)?;
        self.expected_revision = system.revision();
        Ok(stats)
    }
}

fn checked_elapsed(
    time: SimulationInstant,
    epoch: SimulationInstant,
) -> Result<f64, AnalyticMotionError> {
    let time = time.seconds_since_epoch();
    let epoch = epoch.seconds_since_epoch();
    let elapsed = time - epoch;
    if time.abs() > ANALYTIC_TIME_LIMIT_SECONDS
        || epoch.abs() > ANALYTIC_TIME_LIMIT_SECONDS
        || elapsed.abs() > ANALYTIC_TIME_LIMIT_SECONDS
    {
        return Err(AnalyticMotionError::UnsupportedTime);
    }
    Ok(elapsed)
}

fn periodic_phase(elapsed: f64, period: f64) -> Result<f64, AnalyticMotionError> {
    if !period.is_finite() || period <= 0.0 {
        return Err(AnalyticMotionError::UnrepresentableArithmetic);
    }
    if (elapsed / period).abs() > ANALYTIC_CYCLE_LIMIT {
        return Err(AnalyticMotionError::UnsupportedTime);
    }
    // Reduce time first; never multiply an astronomical elapsed time by mean motion.
    Ok((elapsed % period) / period * TAU)
}
fn principal(angle: f64) -> f64 {
    let reduced = angle % TAU;
    if reduced > PI {
        reduced - TAU
    } else if reduced < -PI {
        reduced + TAU
    } else {
        reduced
    }
}

fn spin_phase(elapsed: f64, rate: f64) -> Result<f64, AnalyticMotionError> {
    let product = elapsed * rate;
    if !product.is_finite() || (product / TAU).abs() > ANALYTIC_CYCLE_LIMIT {
        return Err(AnalyticMotionError::UnsupportedTime);
    }
    if product.abs() <= PI {
        return Ok(product);
    }
    // Reduce the explicit angular-rate product, not a rounded invented period.
    // FMA retains multiplication error; split 2pi avoids accumulated constant
    // roundoff over many rotations. The bounded quotient is an exact f64 integer.
    const TAU_LOW: f64 = 2.449_293_598_294_706_4e-16;
    let quotient = (product / TAU).round();
    let product_error = elapsed.mul_add(rate, -product);
    Ok(principal(
        (-quotient).mul_add(TAU, product) + product_error - quotient * TAU_LOW,
    ))
}

fn solve_kepler(
    mean: f64,
    eccentricity: f64,
    limit: u32,
) -> Result<(f64, u32), AnalyticMotionError> {
    if mean == 0.0 || eccentricity == 0.0 {
        return Ok((mean, 0));
    }
    // Symmetry gives a compact monotonic bracket. Newton is always safeguarded.
    let target = mean.abs();
    let mut low = target;
    let mut high = PI;
    let mut anomaly = if eccentricity < 0.8 { target } else { PI };
    for iterations in 1..=limit {
        let residual =
            (1.0 - eccentricity) * anomaly + eccentricity * anomaly_minus_sine(anomaly) - target;
        let derivative = (1.0 - eccentricity) + 2.0 * eccentricity * (anomaly * 0.5).sin().powi(2);
        let correction = residual / derivative;
        if residual == 0.0
            || correction.abs() <= 4.0 * f64::EPSILON * anomaly.abs().max(f64::MIN_POSITIVE)
        {
            return Ok((mean.signum() * anomaly, iterations));
        }
        if residual > 0.0 {
            high = anomaly;
        } else {
            low = anomaly;
        }
        let newton = anomaly - correction;
        let next = if newton > low && newton < high {
            newton
        } else {
            low + (high - low) * 0.5
        };
        if next == anomaly {
            return Err(AnalyticMotionError::Convergence { iterations });
        }
        anomaly = next;
    }
    Err(AnalyticMotionError::Convergence { iterations: limit })
}

// Stable near periapsis even when e is very close to one. Direct E - sin(E)
// would erase the cubic term that dominates the nearly parabolic equation.
fn anomaly_minus_sine(anomaly: f64) -> f64 {
    if anomaly.abs() >= 0.5 {
        return anomaly - anomaly.sin();
    }
    let x = anomaly * anomaly;
    anomaly
        * x
        * (1.0 / 6.0
            + x * (-1.0 / 120.0
                + x * (1.0 / 5040.0
                    + x * (-1.0 / 362_880.0
                        + x * (1.0 / 39_916_800.0
                            + x * (-1.0 / 6_227_020_800.0
                                + x * (1.0 / 1_307_674_368_000.0 - x / 355_687_428_096_000.0)))))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_math::Direction3;
    use mundaris_world::{AxialSpin, BodyMotion, BodyProperties};
    use std::num::NonZeroU64;

    #[test]
    fn repeated_sampling_keeps_candidate_storage_and_capacity() {
        let mut world = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
        let state = BodyState::new(
            LocalPosition::origin(),
            LinearVelocity3::zero(),
            UnitRotation::identity(),
            AngularVelocity3::zero(),
        );
        let body = world
            .insert_body("body", BodyProperties::new(1.0, 1.0).unwrap(), state)
            .unwrap();
        let spin = AxialSpin::new(
            UnitRotation::identity(),
            Direction3::try_new(DVec3::Z).unwrap(),
            0.0,
            SimulationInstant::ZERO,
        )
        .unwrap();
        let definition = CelestialMotionDefinition::new(
            &world,
            &[BodyMotion {
                body,
                translation: CelestialTranslation::Stationary(LocalPosition::origin()),
                spin,
            }],
        )
        .unwrap();
        let mut producer = AnalyticMotionProducer::new(&world, definition).unwrap();
        let pointer = producer.candidates.as_ptr();
        let capacity = producer.candidates.capacity();
        for seconds in 0..128 {
            producer
                .sample(
                    &mut world,
                    SimulationInstant::try_seconds_since_epoch(f64::from(seconds)).unwrap(),
                )
                .unwrap();
            assert_eq!(producer.candidates.as_ptr(), pointer);
            assert_eq!(producer.candidates.capacity(), capacity);
        }
    }
}
