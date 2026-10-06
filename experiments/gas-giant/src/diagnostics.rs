//! Derived diagnostics for the periodic square-domain shallow-water experiment.
//!
//! These quantities describe the numerical surrogate; they are not spherical
//! atmospheric diagnostics. Vortex detections are heuristic field extrema.

use serde::Serialize;
use crate::solver::{Fields, Solver};

const PEAK_RELATIVE_THRESHOLD: f64 = 0.05;
const VORTEX_RMS_THRESHOLD: f64 = 1.5;
const MAX_VORTEX_DETECTIONS: usize = 64;

#[derive(Clone, Debug, Serialize)]
pub struct Diagnostics {
    pub time_s: f64,
    pub tick: u64,
    pub rms_speed_m_s: f64,
    pub zonal_energy_fraction: f64,
    pub mass_mean: f64,
    pub buoyancy_mass_mean: f64,
    pub kinetic_energy_mean_nondimensional: f64,
    pub potential_energy_mean_nondimensional: f64,
    pub depth_min: f64,
    pub depth_max: f64,
    pub buoyancy_min: f64,
    pub buoyancy_max: f64,
    pub tracer_min: f64,
    pub tracer_max: f64,
    pub vorticity_rms_s_inv: f64,
    pub zonal_wind_m_s: Vec<f64>,
    pub jet_peak_rows: Vec<usize>,
    pub enstrophy_mean_nondimensional: f64,
    pub zonal_momentum_mean_nondimensional: f64,
}

pub fn diagnose(solver: &Solver) -> Diagnostics {
    diagnose_fields(solver.fields(), solver.derived().time_unit_s, solver.derived().wave_speed_m_s)
}

fn diagnose_fields(fields: Fields<'_>, time_unit_s: f64, wave_speed_m_s: f64) -> Diagnostics {
    let n = fields.resolution;
    let count = n * n;
    let mean = |values: &[f64]| values.iter().sum::<f64>() / count as f64;
    let minmax = |values: &[f64]| -> (f64, f64) {
        values.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| (lo.min(v), hi.max(v)))
    };
    let mass = mean(fields.h);
    let mass_mean_b = fields.h.iter().zip(fields.buoyancy).map(|(h, b)| h * b).sum::<f64>() / count as f64;
    let speed_sq = fields.u.iter().zip(fields.v).map(|(u, v)| u * u + v * v).sum::<f64>() / count as f64;
    let kinetic = 0.5 * speed_sq;
    let potential = fields.h.iter().zip(fields.buoyancy).map(|(h, b)| 0.5 * b * h * h).sum::<f64>() / count as f64;
    let zonal_wind_nd: Vec<f64> = fields.u.chunks_exact(n).map(|row| row.iter().sum::<f64>() / n as f64).collect();
    let zonal_energy = zonal_wind_nd.iter().map(|u| u * u).sum::<f64>() / n as f64;
    let zonal_energy_fraction = if speed_sq > 0.0 { zonal_energy / speed_sq } else { 0.0 };
    let max_abs_zonal = zonal_wind_nd.iter().fold(0.0_f64, |a, u| a.max(u.abs()));
    let jet_peak_rows = (0..n).filter(|&y| {
        let a = zonal_wind_nd[y].abs();
        a >= PEAK_RELATIVE_THRESHOLD * max_abs_zonal
            && a > zonal_wind_nd[(y + n - 1) % n].abs()
            && a >= zonal_wind_nd[(y + 1) % n].abs()
    }).collect();
    let (depth_min, depth_max) = minmax(fields.h);
    let (buoyancy_min, buoyancy_max) = minmax(fields.buoyancy);
    let (tracer_min, tracer_max) = minmax(fields.tracer);
    Diagnostics {
        time_s: fields.time_nondimensional * time_unit_s,
        tick: fields.tick,
        rms_speed_m_s: speed_sq.sqrt() * wave_speed_m_s,
        zonal_energy_fraction,
        mass_mean: mass,
        buoyancy_mass_mean: mass_mean_b,
        kinetic_energy_mean_nondimensional: kinetic,
        potential_energy_mean_nondimensional: potential,
        depth_min, depth_max, buoyancy_min, buoyancy_max, tracer_min, tracer_max,
        vorticity_rms_s_inv: mean(&fields.vorticity.iter().map(|z| z * z).collect::<Vec<_>>()).sqrt() / time_unit_s,
        zonal_wind_m_s: zonal_wind_nd.into_iter().map(|u| u * wave_speed_m_s).collect(),
        jet_peak_rows,
        enstrophy_mean_nondimensional: 0.5 * mean(&fields.vorticity.iter().map(|z| z * z).collect::<Vec<_>>()),
        zonal_momentum_mean_nondimensional: fields.h.iter().zip(fields.u).map(|(h, u)| h * u).sum::<f64>() / count as f64,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct VortexObservation {
    pub id: u64,
    pub age_seconds: f64,
    pub x_rad: f64,
    pub y_rad: f64,
    pub peak_vorticity_s_inv: f64,
}

#[derive(Clone, Debug)]
struct TrackedVortex { id: u64, sign: i8, x: f64, y: f64, age: f64, last_time: f64 }

/// Tracks heuristic resolved vorticity extrema between observations. Disappearance
/// is a tracking gap, not evidence of physical storm death; mergers are not inferred.
#[derive(Default)]
pub struct VortexTracker { previous: Vec<TrackedVortex>, next_id: u64 }

impl VortexTracker {
    pub fn new() -> Self { Self::default() }

    pub fn observe(&mut self, solver: &Solver) -> Vec<VortexObservation> {
        let f = solver.fields();
        let n = f.resolution;
        let rms = (f.vorticity.iter().map(|z| z*z).sum::<f64>() / (n*n) as f64).sqrt();
        if rms == 0.0 { self.previous.clear(); return Vec::new(); }
        let dx = std::f64::consts::TAU / n as f64;
        let mut candidates = Vec::new();
        for y in 0..n { for x in 0..n {
            let i = y*n+x; let z = f.vorticity[i];
            if z.abs() < VORTEX_RMS_THRESHOLD*rms { continue; }
            let extremum = (-1isize..=1).all(|dy| (-1isize..=1).all(|xx| {
                if dy == 0 && xx == 0 { return true; }
                let j = ((y as isize+dy+n as isize)%n as isize) as usize*n + ((x as isize+xx+n as isize)%n as isize) as usize;
                if z > 0.0 { z >= f.vorticity[j] } else { z <= f.vorticity[j] }
            }));
            if extremum { candidates.push((x as f64*dx, y as f64*dx, z)); }
        }}
        candidates.sort_by(|a,b| b.2.abs().total_cmp(&a.2.abs()));
        let radius2 = (2.0*dx).powi(2);
        let mut selected: Vec<(f64,f64,f64)> = Vec::new();
        for c in candidates {
            if selected.iter().all(|p| periodic_distance2(c.0,c.1,p.0,p.1) >= radius2) { selected.push(c); }
            if selected.len() == MAX_VORTEX_DETECTIONS { break; }
        }
        let time = f.time_nondimensional * solver.derived().time_unit_s;
        let mut used = vec![false; self.previous.len()];
        let mut current = Vec::new(); let mut output = Vec::new();
        for (x,y,z) in selected {
            let sign = if z > 0.0 { 1 } else { -1 };
            let match_index = self.previous.iter().enumerate().filter(|(i,p)| !used[*i] && p.sign == sign)
                .map(|(i,p)| (i, periodic_distance2(x,y,p.x,p.y))).filter(|(_,d)| *d <= (4.0*dx).powi(2))
                .min_by(|a,b| a.1.total_cmp(&b.1)).map(|(i,_)| i);
            let (id,age) = if let Some(i)=match_index {
                used[i]=true; let p=&self.previous[i]; (p.id, p.age+(time-p.last_time).max(0.0))
            } else { let id=self.next_id; self.next_id=self.next_id.wrapping_add(1); (id,0.0) };
            current.push(TrackedVortex{id,sign,x,y,age,last_time:time});
            output.push(VortexObservation{id,age_seconds:age,x_rad:x,y_rad:y,peak_vorticity_s_inv:z/solver.derived().time_unit_s});
        }
        self.previous=current;
        output
    }
}

fn periodic_distance2(x: f64, y: f64, a: f64, b: f64) -> f64 {
    let wrap = |d: f64| { let d=d.abs(); d.min(std::f64::consts::TAU-d) };
    wrap(x-a).powi(2)+wrap(y-b).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_fields_have_expected_means_and_safe_zero_energy_fraction() {
        let n = 4;
        let fields = Fields {
            resolution: n,
            time_nondimensional: 2.0,
            tick: 7,
            h: &[2.0; 16],
            buoyancy: &[3.0; 16],
            u: &[0.0; 16],
            v: &[0.0; 16],
            tracer: &[1.0; 16],
            vorticity: &[0.0; 16],
        };
        let d = diagnose_fields(fields, 10.0, 5.0);
        assert_eq!(d.time_s, 20.0);
        assert_eq!(d.tick, 7);
        assert_eq!(d.mass_mean, 2.0);
        assert_eq!(d.buoyancy_mass_mean, 6.0);
        assert_eq!(d.potential_energy_mean_nondimensional, 6.0);
        assert_eq!(d.zonal_energy_fraction, 0.0);
        assert_eq!(d.vorticity_rms_s_inv, 0.0);
        assert!(d.jet_peak_rows.is_empty());
    }

    #[test]
    fn zonal_peak_ignores_rows_below_relative_cutoff() {
        let n = 4;
        let fields = Fields {
            resolution: n, time_nondimensional: 0.0, tick: 0,
            h: &[1.0; 16], buoyancy: &[1.0; 16],
            u: &[0.0,0.0,0.0,0.0, 0.01,0.01,0.01,0.01, 2.0,2.0,2.0,2.0, 0.0,0.0,0.0,0.0],
            v: &[0.0; 16], tracer: &[0.0; 16], vorticity: &[0.0; 16],
        };
        assert_eq!(diagnose_fields(fields, 1.0, 1.0).jet_peak_rows, vec![2]);
    }
}
