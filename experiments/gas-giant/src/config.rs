use serde::{Deserialize, Serialize};

/// Physical inputs, deliberately separate from effective-layer and numerical closures.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Planet {
    pub radius_m: f64,
    pub gravity_m_s2: f64,
    pub rotation_period_s: f64,
    pub reference_pressure_pa: f64,
    pub internal_heat_w_m2: f64,
}

/// Uncertain reduced-model choices; neither equivalent depth nor density contrast
/// is claimed to be derived uniquely from bulk atmospheric properties.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Closure {
    pub equivalent_depth_m: f64,
    pub fractional_density_contrast: f64,
    pub heat_to_kinetic_efficiency: f64,
    pub cooling_time_s: f64,
    pub drag_time_s: f64,
    pub forcing_correlation_s: f64,
    pub forcing_length_m: f64,
    pub stellar_equilibrium_contrast: f64,
    pub substellar_angular_speed_rad_s: f64,
    pub initial_eddy_speed_m_s: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Numerics {
    pub resolution: usize,
    pub step_s: f64,
    pub cutoff_damping_time_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Model {
    Thermal,
    Isothermal,
    /// Prescribed, seeded eddies advect a passive scalar without feedback.
    /// No jets or storm stamps are prescribed, and this is not a physical solver.
    Kinematic,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub name: String,
    pub seed: u64,
    pub model: Model,
    pub planet: Planet,
    pub closure: Closure,
    pub numerics: Numerics,
}

#[derive(Clone, Debug, Serialize)]
pub struct Derived {
    pub reduced_gravity_m_s2: f64,
    pub wave_speed_m_s: f64,
    pub time_unit_s: f64,
    pub rotation_parameter: f64,
    pub polar_deformation_radius_m: f64,
    pub column_mass_kg_m2: f64,
    pub kinetic_injection_m2_s3: f64,
    pub nondimensional_injection: f64,
    pub forcing_wavenumber: f64,
    pub step_nondimensional: f64,
}

impl Config {
    pub fn derive(&self) -> Result<Derived, String> {
        let p = &self.planet;
        let c = &self.closure;
        let n = &self.numerics;
        for (name, value) in [
            ("radius_m", p.radius_m),
            ("gravity_m_s2", p.gravity_m_s2),
            ("rotation_period_s", p.rotation_period_s),
            ("reference_pressure_pa", p.reference_pressure_pa),
            ("equivalent_depth_m", c.equivalent_depth_m),
            ("fractional_density_contrast", c.fractional_density_contrast),
            ("cooling_time_s", c.cooling_time_s),
            ("drag_time_s", c.drag_time_s),
            ("forcing_correlation_s", c.forcing_correlation_s),
            ("forcing_length_m", c.forcing_length_m),
            ("step_s", n.step_s),
            ("cutoff_damping_time_s", n.cutoff_damping_time_s),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("{name} must be finite and positive"));
            }
        }
        for (name, value) in [
            ("internal_heat_w_m2", p.internal_heat_w_m2),
            ("initial_eddy_speed_m_s", c.initial_eddy_speed_m_s),
            ("heat_to_kinetic_efficiency", c.heat_to_kinetic_efficiency),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(format!("{name} must be finite and nonnegative"));
            }
        }
        if c.fractional_density_contrast > 1.0 || c.heat_to_kinetic_efficiency > 1.0 {
            return Err("density contrast and kinetic efficiency must be <= 1".into());
        }
        if !c.stellar_equilibrium_contrast.is_finite()
            || !(0.0..=0.5).contains(&c.stellar_equilibrium_contrast)
            || !c.substellar_angular_speed_rad_s.is_finite()
        {
            return Err("stellar contrast must be in [0, 0.5] and angular speed finite".into());
        }
        if !(24..=512).contains(&n.resolution) || !n.resolution.is_power_of_two() {
            return Err("resolution must be a power of two in [32, 512]".into());
        }
        let g = p.gravity_m_s2 * c.fractional_density_contrast;
        let speed = (g * c.equivalent_depth_m).sqrt();
        let time = p.radius_m / speed;
        let omega = std::f64::consts::TAU / p.rotation_period_s;
        let column = p.reference_pressure_pa / p.gravity_m_s2;
        let injection = c.heat_to_kinetic_efficiency * p.internal_heat_w_m2 / column;
        let d = Derived {
            reduced_gravity_m_s2: g,
            wave_speed_m_s: speed,
            time_unit_s: time,
            rotation_parameter: 2.0 * omega * time,
            polar_deformation_radius_m: speed / (2.0 * omega),
            column_mass_kg_m2: column,
            kinetic_injection_m2_s3: injection,
            nondimensional_injection: injection * time / speed.powi(2),
            forcing_wavenumber: p.radius_m / c.forcing_length_m,
            step_nondimensional: n.step_s / time,
        };
        if [g, speed, time, omega, column, injection, d.rotation_parameter,
            d.polar_deformation_radius_m, d.nondimensional_injection,
            d.forcing_wavenumber, d.step_nondimensional].iter().any(|v| !v.is_finite())
            || speed <= 0.0 || time <= 0.0 || column <= 0.0
        {
            return Err("derived quantities overflow or underflow".into());
        }
        if d.forcing_wavenumber < 2.0 || d.forcing_wavenumber * 1.25 >= n.resolution as f64 / 3.0 {
            return Err("forcing ring must be resolved below the two-thirds spectral cutoff".into());
        }
        Ok(d)
    }

    pub fn preset(name: &str, seed: u64, resolution: usize) -> Result<Self, String> {
        let mut config = Self {
            name: name.to_owned(), seed, model: Model::Thermal,
            planet: Planet { radius_m: 7.0e7, gravity_m_s2: 25.0,
                rotation_period_s: 100_000.0, reference_pressure_pa: 100_000.0,
                internal_heat_w_m2: 40.0 },
            closure: Closure { equivalent_depth_m: 10_000.0,
                fractional_density_contrast: 0.36, heat_to_kinetic_efficiency: 0.2,
                cooling_time_s: 2.8e6, drag_time_s: 1.4e7,
                forcing_correlation_s: 46_666.666666666664,
                forcing_length_m: 8.75e6, stellar_equilibrium_contrast: 0.0,
                substellar_angular_speed_rad_s: 0.0, initial_eddy_speed_m_s: 15.0 },
            numerics: Numerics { resolution, step_s: 700.0,
                cutoff_damping_time_s: 46_666.666666666664 },
        };
        match name {
            "rapid" => {}
            "slow" => config.planet.rotation_period_s *= 8.0,
            "irradiated" => {
                config.planet.rotation_period_s *= 8.0;
                config.planet.internal_heat_w_m2 = 4.0;
                config.closure.cooling_time_s = 233_333.33333333334;
                config.closure.stellar_equilibrium_contrast = 0.15;
            }
            "weak" => {
                config.planet.internal_heat_w_m2 = 4.0;
                config.closure.drag_time_s = 2.8e6;
            }
            _ => return Err(format!("unknown preset {name}; use rapid, slow, irradiated, weak")),
        }
        // Keep the physical forcing unchanged in resolution comparisons.
        config.numerics.step_s *= 32.0 / resolution as f64;
        config.derive()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_derivation_and_regime_inputs() {
        let a = Config::preset("rapid", 1, 32).unwrap();
        let b = Config::preset("slow", 1, 32).unwrap();
        let d = a.derive().unwrap();
        assert!((d.wave_speed_m_s - 300.0).abs() < 1e-12);
        assert!((d.column_mass_kg_m2 - 4000.0).abs() < 1e-12);
        assert!((d.kinetic_injection_m2_s3 - 0.002).abs() < 1e-14);
        assert!((d.rotation_parameter / b.derive().unwrap().rotation_parameter - 8.0).abs() < 1e-12);
    }
    #[test]
    fn invalid_inputs_rejected() {
        let mut c = Config::preset("rapid", 1, 32).unwrap();
        c.planet.radius_m = f64::NAN;
        assert!(c.derive().is_err());
        assert!(Config::preset("rapid", 1, 16).is_err());
        let mut c = Config::preset("rapid", 1, 32).unwrap();
        c.closure.forcing_length_m = 1.0;
        assert!(c.derive().is_err());
    }
}
