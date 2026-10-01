//! Independent compensated diagnostics; no force cache or frame queries.

use crate::{GRAVITATIONAL_CONSTANT_M3_KG_S2 as G, SimulationError};
use glam::DVec3;
use mundaris_math::SimulationInstant;
use mundaris_world::CelestialSystem;

#[derive(Default)]
struct Sum {
    value: f64,
    correction: f64,
}
impl Sum {
    fn add(&mut self, x: f64) {
        let next = self.value + x;
        self.correction += if self.value.abs() >= x.abs() {
            (self.value - next) + x
        } else {
            (x - next) + self.value
        };
        self.value = next;
    }
    fn total(&self) -> f64 {
        self.value + self.correction
    }
}
#[derive(Default)]
struct VectorSum([Sum; 3]);
impl VectorSum {
    fn add(&mut self, v: DVec3) {
        for (s, x) in self.0.iter_mut().zip(v.to_array()) {
            s.add(x);
        }
    }
    fn total(&self) -> DVec3 {
        DVec3::from_array(std::array::from_fn(|i| self.0[i].total()))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SystemDiagnostics {
    pub sampled_time: SimulationInstant,
    pub kinetic_energy_j: f64,
    pub potential_energy_j: f64,
    pub linear_momentum_kg_m_s: DVec3,
    pub angular_momentum_com_kg_m2_s: DVec3,
    pub center_of_mass_m: DVec3,
    pub center_of_mass_velocity_m_s: DVec3,
    pub momentum_scale: f64,
    pub angular_momentum_scale: f64,
}
impl SystemDiagnostics {
    pub fn total_energy_j(self) -> f64 {
        self.kinetic_energy_j + self.potential_energy_j
    }
}

pub fn system_diagnostics(system: &CelestialSystem) -> Result<SystemDiagnostics, SimulationError> {
    let mut mass = Sum::default();
    let mut kinetic = Sum::default();
    let mut potential = Sum::default();
    let mut momentum = VectorSum::default();
    let mut com_delta = VectorSum::default();
    let mut qp = Sum::default();
    let anchor = system
        .bodies()
        .next()
        .map_or(DVec3::ZERO, |(_, b)| b.state().center_in_system().metres());
    for (_, b) in system.bodies() {
        mass.add(b.properties().mass_kg());
    }
    let total_mass = mass.total();
    if !total_mass.is_finite() {
        return Err(SimulationError::DiagnosticArithmetic);
    }
    for (_, b) in system.bodies() {
        let m = b.properties().mass_kg();
        let x = b.state().center_in_system().metres();
        let v = b.state().center_velocity_in_system().metres_per_second();
        kinetic.add((0.5 * m) * v.dot(v));
        momentum.add(m * v);
        qp.add(m * crate::gravity::distance(v));
        com_delta.add((x - anchor) * (m / total_mass));
    }
    let center = anchor + com_delta.total();
    let momentum = momentum.total();
    let vcom = if total_mass == 0.0 {
        DVec3::ZERO
    } else {
        momentum / total_mass
    };
    let mut angular = VectorSum::default();
    let mut ql = Sum::default();
    for (i, (_, b)) in system.bodies().enumerate() {
        let m = b.properties().mass_kg();
        let x = b.state().center_in_system().metres();
        let relative = x - center;
        let v = b.state().center_velocity_in_system().metres_per_second() - vcom;
        angular.add(relative.cross(m * v));
        ql.add((m * crate::gravity::distance(relative)) * crate::gravity::distance(v));
        for (_, other) in system.bodies().skip(i + 1) {
            let r = crate::gravity::distance(other.state().center_in_system().metres() - x);
            if !r.is_normal() {
                return Err(SimulationError::DiagnosticArithmetic);
            }
            potential.add(-((G * m / r) * other.properties().mass_kg()));
        }
    }
    let result = SystemDiagnostics {
        sampled_time: system.sample_time(),
        kinetic_energy_j: kinetic.total(),
        potential_energy_j: potential.total(),
        linear_momentum_kg_m_s: momentum,
        angular_momentum_com_kg_m2_s: angular.total(),
        center_of_mass_m: center,
        center_of_mass_velocity_m_s: vcom,
        momentum_scale: qp.total(),
        angular_momentum_scale: ql.total(),
    };
    if ![
        result.kinetic_energy_j,
        result.potential_energy_j,
        result.total_energy_j(),
        result.momentum_scale,
        result.angular_momentum_scale,
    ]
    .iter()
    .all(|v| v.is_finite())
        || !center.is_finite()
        || !momentum.is_finite()
        || !vcom.is_finite()
        || !result.angular_momentum_com_kg_m2_s.is_finite()
    {
        return Err(SimulationError::DiagnosticArithmetic);
    }
    Ok(result)
}

/// Explicit branch reference; authoring changes reseed rather than count as drift.
#[derive(Debug, Clone, Copy)]
pub struct DiagnosticBaseline(pub SystemDiagnostics);
#[derive(Debug, Clone, Copy)]
pub struct DiagnosticDrift {
    pub energy_j: f64,
    pub relative_energy: f64,
    pub uses_near_zero_energy_scale: bool,
    pub momentum_kg_m_s: DVec3,
    pub normalized_momentum: f64,
    pub angular_momentum_kg_m2_s: DVec3,
    pub normalized_angular_momentum: f64,
    pub com_residual_m: DVec3,
}
impl DiagnosticBaseline {
    pub fn drift(self, current: SystemDiagnostics) -> DiagnosticDrift {
        let initial = self.0;
        let absolute_scale = initial.kinetic_energy_j + initial.potential_energy_j.abs();
        let near_zero = initial.total_energy_j().abs() <= 1e-12 * absolute_scale;
        let energy_scale = if near_zero {
            absolute_scale
        } else {
            initial.total_energy_j().abs()
        };
        let energy = current.total_energy_j() - initial.total_energy_j();
        let p = current.linear_momentum_kg_m_s - initial.linear_momentum_kg_m_s;
        let l = current.angular_momentum_com_kg_m2_s - initial.angular_momentum_com_kg_m2_s;
        let elapsed =
            current.sampled_time.seconds_since_epoch() - initial.sampled_time.seconds_since_epoch();
        DiagnosticDrift {
            energy_j: energy,
            relative_energy: if energy_scale == 0.0 {
                0.0
            } else {
                energy / energy_scale
            },
            uses_near_zero_energy_scale: near_zero,
            momentum_kg_m_s: p,
            normalized_momentum: if initial.momentum_scale == 0.0 {
                0.0
            } else {
                crate::gravity::distance(p) / initial.momentum_scale
            },
            angular_momentum_kg_m2_s: l,
            normalized_angular_momentum: if initial.angular_momentum_scale == 0.0 {
                0.0
            } else {
                crate::gravity::distance(l) / initial.angular_momentum_scale
            },
            com_residual_m: current.center_of_mass_m
                - (initial.center_of_mass_m + initial.center_of_mass_velocity_m_s * elapsed),
        }
    }
}
