use crate::config::{Config, Derived, Model};
use rustfft::{Fft, FftPlanner, num_complex::Complex64};
use serde::Serialize;
use std::{f64::consts::{FRAC_PI_2, TAU}, sync::Arc};

const H: usize = 0;
const P: usize = 1;
const Q: usize = 2;
const S: usize = 3;
const T: usize = 4;
type State = [Vec<f64>; 5];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("step rejected at tick {tick}: {reason}")]
    Step { tick: u64, reason: String },
}

/// Sources and explicitly measured filtering changes are distinguished from
/// unexplained discretization drift. Energy is per unit reference layer mass.
#[derive(Clone, Default, Debug, Serialize)]
pub struct Budgets {
    pub initial_energy: f64,
    pub forcing_work: f64,
    pub drag_work: f64,
    pub thermal_work: f64,
    pub filter_energy_change: f64,
    pub energy_residual: f64,
    pub initial_zonal_momentum: f64,
    pub zonal_momentum_source_change: f64,
    pub zonal_momentum_residual: f64,
    pub max_courant_rotation_bound: f64,
}

#[derive(Clone, Copy)]
pub struct Fields<'a> {
    pub resolution: usize,
    pub time_nondimensional: f64,
    pub tick: u64,
    pub h: &'a [f64],
    pub buoyancy: &'a [f64],
    pub u: &'a [f64],
    pub v: &'a [f64],
    pub tracer: &'a [f64],
    pub vorticity: &'a [f64],
}

struct Spectral {
    n: usize,
    forward: Arc<dyn Fft<f64>>,
    inverse: Arc<dyn Fft<f64>>,
    scratch: Vec<Complex64>,
    line: Vec<Complex64>,
    a: Vec<Complex64>,
    b: Vec<Complex64>,
    k: Vec<f64>,
}

impl Spectral {
    fn new(n: usize) -> Self {
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(n);
        let inverse = planner.plan_fft_inverse(n);
        let len = forward.get_inplace_scratch_len().max(inverse.get_inplace_scratch_len());
        Self { n, forward, inverse, scratch: vec![Complex64::default(); len],
            line: vec![Complex64::default(); n], a: vec![Complex64::default(); n*n],
            b: vec![Complex64::default(); n*n],
            k: (0..n).map(|i| if i < n/2 { i as f64 } else { i as f64-n as f64 }).collect() }
    }

    fn transform(n: usize, plan: &dyn Fft<f64>, scratch: &mut [Complex64],
        line: &mut [Complex64], data: &mut [Complex64]) {
        for row in data.chunks_exact_mut(n) { plan.process_with_scratch(row, scratch); }
        for x in 0..n {
            for y in 0..n { line[y] = data[y*n+x]; }
            plan.process_with_scratch(line, scratch);
            for y in 0..n { data[y*n+x] = line[y]; }
        }
    }

    fn divergence(&mut self, x: &[f64], y: &[f64], out: &mut [f64]) {
        for i in 0..x.len() { self.a[i] = Complex64::new(x[i], 0.0); self.b[i] = Complex64::new(y[i], 0.0); }
        Self::transform(self.n, self.forward.as_ref(), &mut self.scratch, &mut self.line, &mut self.a);
        Self::transform(self.n, self.forward.as_ref(), &mut self.scratch, &mut self.line, &mut self.b);
        let cut = self.n as f64/3.0;
        for ky in 0..self.n { for kx in 0..self.n {
            let i = ky*self.n+kx;
            self.a[i] = if self.k[kx].abs() < cut && self.k[ky].abs() < cut {
                Complex64::i() * (self.k[kx]*self.a[i] + self.k[ky]*self.b[i])
            } else { Complex64::default() };
        }}
        Self::transform(self.n, self.inverse.as_ref(), &mut self.scratch, &mut self.line, &mut self.a);
        let scale = 1.0/(self.n*self.n) as f64;
        for (value, z) in out.iter_mut().zip(&self.a) { *value = z.re*scale; }
    }

    fn filter(&mut self, data: &mut [f64], dt: f64, decay_time: f64) {
        for (a, &v) in self.a.iter_mut().zip(data.iter()) { *a = Complex64::new(v, 0.0); }
        Self::transform(self.n, self.forward.as_ref(), &mut self.scratch, &mut self.line, &mut self.a);
        let cut = self.n as f64/3.0;
        for y in 0..self.n { for x in 0..self.n {
            let ratio = self.k[x].hypot(self.k[y])/cut;
            let damping = if self.k[x].abs() < cut && self.k[y].abs() < cut {
                (-dt/decay_time*ratio.powi(8)).exp()
            } else { 0.0 };
            self.a[y*self.n+x] *= damping;
        }}
        Self::transform(self.n, self.inverse.as_ref(), &mut self.scratch, &mut self.line, &mut self.a);
        let scale = 1.0/(self.n*self.n) as f64;
        for (v, z) in data.iter_mut().zip(&self.a) { *v = z.re*scale; }
    }

    fn eddies(&mut self, seed: u64, epoch: u64, wavenumber: f64, u: &mut [f64], v: &mut [f64]) {
        self.a.fill(Complex64::default());
        self.b.fill(Complex64::default());
        let mut count = 0;
        for y in 0..self.n { for x in 1..self.n/2 {
            let radius = self.k[x].hypot(self.k[y]);
            if (radius-wavenumber).abs() <= 1.0 { count += 1; }
        }}
        let amplitude = (self.n*self.n) as f64/(2.0*count as f64).sqrt();
        for y in 0..self.n { for x in 1..self.n/2 {
            let radius = self.k[x].hypot(self.k[y]);
            if (radius-wavenumber).abs() > 1.0 { continue; }
            let code = (x as u64) | ((self.k[y] as i64 as u64).wrapping_mul(65537));
            let phase = TAU*unit_hash(seed ^ epoch.wrapping_mul(0x9e3779b97f4a7c15) ^ code);
            let psi = Complex64::from_polar(amplitude/radius, phase);
            let i = y*self.n+x;
            let conjugate = ((self.n-y)%self.n)*self.n+(self.n-x);
            self.a[i] = -Complex64::i()*self.k[y]*psi;
            self.b[i] = Complex64::i()*self.k[x]*psi;
            self.a[conjugate] = self.a[i].conj();
            self.b[conjugate] = self.b[i].conj();
        }}
        Self::transform(self.n, self.inverse.as_ref(), &mut self.scratch, &mut self.line, &mut self.a);
        Self::transform(self.n, self.inverse.as_ref(), &mut self.scratch, &mut self.line, &mut self.b);
        let scale = 1.0/(self.n*self.n) as f64;
        for i in 0..u.len() { u[i] = self.a[i].re*scale; v[i] = self.b[i].re*scale; }
    }
}

fn unit_hash(mut x: u64) -> f64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^= x >> 31;
    (x >> 11) as f64 / (1_u64 << 53) as f64
}

fn empty_state(len: usize) -> State { std::array::from_fn(|_| vec![0.0; len]) }
fn energy(s: &State) -> f64 {
    (0..s[H].len()).map(|i| 0.5*(s[P][i].powi(2)+s[Q][i].powi(2))/s[H][i]
        +0.5*s[S][i]*s[H][i]).sum::<f64>()/s[H].len() as f64
}
fn mean(a: &[f64]) -> f64 { a.iter().sum::<f64>() / a.len() as f64 }

#[derive(Default, Clone, Copy)]
struct Rates { forcing: f64, drag: f64, thermal: f64, momentum: f64 }

/// Five conservative fields on the periodic square: h, hu, hv, hb and hC.
/// Sources follow Warneford & Dellar (2017), equations (2), not a spherical GCM.
pub struct Solver {
    config: Config,
    derived: Derived,
    state: State,
    candidate: State,
    stage: State,
    slopes: [State; 4],
    spectral: Spectral,
    force: [Vec<f64>; 4],
    forcing_epoch: Option<u64>,
    flux_x: Vec<f64>,
    flux_y: Vec<f64>,
    h: Vec<f64>,
    u: Vec<f64>,
    v: Vec<f64>,
    buoyancy: Vec<f64>,
    tracer: Vec<f64>,
    vorticity: Vec<f64>,
    tick: u64,
    budgets: Budgets,
}

impl Solver {
    pub fn new(config: Config) -> Result<Self, Error> {
        let derived = config.derive().map_err(Error::Config)?;
        let n = config.numerics.resolution;
        let len = n*n;
        let mut spectral = Spectral::new(n);
        let mut state = empty_state(len);
        state[H].fill(1.0); state[S].fill(1.0);
        spectral.eddies(config.seed, 0, derived.forcing_wavenumber, &mut state[P], &mut state[Q]);
        let amplitude = config.closure.initial_eddy_speed_m_s/derived.wave_speed_m_s;
        for i in 0..len {
            state[P][i] *= amplitude; state[Q][i] *= amplitude;
            let x = TAU*(i%n) as f64/n as f64;
            let y = TAU*(i/n) as f64/n as f64;
            let phase = TAU*unit_hash(config.seed.wrapping_add(117));
            // A passive material marker, not a cloud model or latitude band layout.
            state[T][i] = 0.5 + 0.18*(2.0*x+y+phase).sin() + 0.12*(x-3.0*y-phase).cos();
        }
        let budgets = Budgets { initial_energy: energy(&state), initial_zonal_momentum: mean(&state[P]), ..Budgets::default() };
        let mut solver = Self { config, derived, state, candidate: empty_state(len), stage: empty_state(len),
            slopes: std::array::from_fn(|_| empty_state(len)), spectral,
            force: std::array::from_fn(|_| vec![0.0; len]), forcing_epoch: None,
            flux_x: vec![0.0; len], flux_y: vec![0.0; len], h: vec![0.0; len],
            u: vec![0.0; len], v: vec![0.0; len], buoyancy: vec![0.0; len],
            tracer: vec![0.0; len], vorticity: vec![0.0; len], tick: 0, budgets };
        solver.refresh();
        Ok(solver)
    }

    pub fn config(&self) -> &Config { &self.config }
    pub fn derived(&self) -> &Derived { &self.derived }
    pub fn budgets(&self) -> &Budgets { &self.budgets }
    pub fn fields(&self) -> Fields<'_> {
        Fields { resolution: self.config.numerics.resolution,
            time_nondimensional: self.tick as f64*self.derived.step_nondimensional, tick: self.tick,
            h: &self.h, buoyancy: &self.buoyancy, u: &self.u, v: &self.v,
            tracer: &self.tracer, vorticity: &self.vorticity }
    }

    fn forcing(&mut self, time: f64) -> (f64, f64) {
        let correlation = self.config.closure.forcing_correlation_s/self.derived.time_unit_s;
        let epoch = (time/correlation).floor() as u64;
        if self.forcing_epoch != Some(epoch) {
            let [u0, v0, u1, v1] = &mut self.force;
            self.spectral.eddies(self.config.seed.wrapping_add(12345), epoch,
                self.derived.forcing_wavenumber, u0, v0);
            self.spectral.eddies(self.config.seed.wrapping_add(12345), epoch+1,
                self.derived.forcing_wavenumber, u1, v1);
            self.forcing_epoch = Some(epoch);
        }
        let angle = FRAC_PI_2*(time/correlation-epoch as f64);
        (angle.cos(), angle.sin())
    }

    fn rhs(&mut self, stage_index: usize, time: f64) -> Rates {
        let (a, b) = self.forcing(time);
        let c = &self.config.closure;
        let d = &self.derived;
        let force_scale = (d.nondimensional_injection/(c.forcing_correlation_s/d.time_unit_s)).sqrt();
        let drag = d.time_unit_s/c.drag_time_s;
        let cooling = d.time_unit_s/c.cooling_time_s;
        let n = self.config.numerics.resolution;
        let len = n*n;
        let mut rates = Rates::default();
        for i in 0..len {
            self.h[i] = self.stage[H][i];
            self.u[i] = self.stage[P][i]/self.h[i];
            self.v[i] = self.stage[Q][i]/self.h[i];
        }
        if self.config.model == Model::Kinematic {
            let velocity_scale = (d.kinetic_injection_m2_s3*c.drag_time_s).sqrt()
                .max(c.initial_eddy_speed_m_s)/d.wave_speed_m_s;
            for i in 0..len {
                self.u[i] = velocity_scale*(a*self.force[0][i]+b*self.force[2][i]);
                self.v[i] = velocity_scale*(a*self.force[1][i]+b*self.force[3][i]);
            }
        }
        for field in 0..5 {
            if self.config.model == Model::Kinematic && field != T {
                self.slopes[stage_index][field].fill(0.0); continue;
            }
            for i in 0..len {
                if field == H {
                    self.flux_x[i] = self.stage[P][i]; self.flux_y[i] = self.stage[Q][i];
                } else {
                    self.flux_x[i] = self.stage[field][i]*self.u[i];
                    self.flux_y[i] = self.stage[field][i]*self.v[i];
                    let pressure = 0.5*self.stage[S][i]*self.h[i];
                    if field == P { self.flux_x[i] += pressure; }
                    if field == Q { self.flux_y[i] += pressure; }
                }
            }
            self.spectral.divergence(&self.flux_x, &self.flux_y, &mut self.slopes[stage_index][field]);
            for value in &mut self.slopes[stage_index][field] { *value = -*value; }
        }
        if self.config.model != Model::Kinematic {
            for i in 0..len {
                let x = TAU*(i%n) as f64/n as f64;
                let y = TAU*(i/n) as f64/n as f64;
                let f = d.rotation_parameter*y.sin();
                let fu = force_scale*(a*self.force[0][i]+b*self.force[2][i]);
                let fv = force_scale*(a*self.force[1][i]+b*self.force[3][i]);
                let fp = self.h[i]*fu;
                let fq = self.h[i]*fv;
                let dp = -drag*self.stage[P][i];
                let dq = -drag*self.stage[Q][i];
                let target = 1.0+c.stellar_equilibrium_contrast
                    *((x-c.substellar_angular_speed_rad_s*time*d.time_unit_s).cos()*y.cos()).max(0.0);
                let ds = if self.config.model == Model::Thermal { -cooling*self.h[i]*(self.stage[S][i]-target) } else { 0.0 };
                self.slopes[stage_index][P][i] += f*self.stage[Q][i]+fp+dp;
                self.slopes[stage_index][Q][i] += -f*self.stage[P][i]+fq+dq;
                self.slopes[stage_index][S][i] += ds;
                rates.forcing += self.u[i]*fp+self.v[i]*fq;
                rates.drag += self.u[i]*dp+self.v[i]*dq;
                rates.thermal += 0.5*self.h[i]*ds;
                rates.momentum += f*self.stage[Q][i]+fp+dp;
            }
        }
        rates.forcing /= len as f64; rates.drag /= len as f64;
        rates.thermal /= len as f64; rates.momentum /= len as f64;
        rates
    }

    fn validate(&self, s: &State) -> Result<f64, Error> {
        let n = self.config.numerics.resolution;
        let mut max_speed: f64 = 0.0;
        let mut max_wave: f64 = 0.0;
        for i in 0..n*n {
            if s.iter().any(|a| !a[i].is_finite()) || s[H][i] <= 0.1 || s[S][i]/s[H][i] <= 0.1 {
                return Err(Error::Step { tick: self.tick, reason: "nonfinite state or invalid layer/buoyancy; no clamping or timestep retry".into() });
            }
            max_speed = max_speed.max(s[P][i].hypot(s[Q][i])/s[H][i]);
            max_wave = max_wave.max(s[S][i].sqrt());
        }
        let bound = self.derived.step_nondimensional*(self.derived.rotation_parameter
            + 2.0_f64.sqrt()*n as f64/3.0*(max_speed+max_wave));
        if bound > 0.8 {
            return Err(Error::Step { tick: self.tick, reason: format!("conservative wave/rotation bound {bound:.4} > 0.8; reduce fixed step_s") });
        }
        Ok(bound)
    }

    /// Fixed-step classical RK4 followed by explicitly accounted eighth-order
    /// spectral damping. Rejection leaves the committed state/tick unchanged.
    pub fn step(&mut self) -> Result<(), Error> {
        let dt = self.derived.step_nondimensional;
        let time = self.tick as f64*dt;
        let mut rates = [Rates::default(); 4];
        let mut maximum_bound: f64 = 0.0;
        for (k, rate) in rates.iter_mut().enumerate() {
            let fraction = [0.0, 0.5, 0.5, 1.0][k];
            for field in 0..5 { for i in 0..self.stage[field].len() {
                self.stage[field][i] = self.state[field][i]
                    + if k == 0 { 0.0 } else { fraction*dt*self.slopes[k-1][field][i] };
            }}
            if self.config.model == Model::Isothermal { self.stage[S] = self.stage[H].clone(); }
            maximum_bound = maximum_bound.max(self.validate(&self.stage)?);
            *rate = self.rhs(k, time+fraction*dt);
        }
        for field in 0..5 { for i in 0..self.candidate[field].len() {
            self.candidate[field][i] = self.state[field][i]+dt/6.0*(self.slopes[0][field][i]
                +2.0*self.slopes[1][field][i]+2.0*self.slopes[2][field][i]+self.slopes[3][field][i]);
        }}
        if self.config.model == Model::Isothermal { self.candidate[S] = self.candidate[H].clone(); }
        maximum_bound = maximum_bound.max(self.validate(&self.candidate)?);
        let before_filter = energy(&self.candidate);
        for field in &mut self.candidate {
            self.spectral.filter(field, dt, self.config.numerics.cutoff_damping_time_s/self.derived.time_unit_s);
        }
        maximum_bound = maximum_bound.max(self.validate(&self.candidate)?);
        let filtered_energy = energy(&self.candidate);
        std::mem::swap(&mut self.state, &mut self.candidate);
        self.tick += 1;
        for (k, rate) in rates.iter().enumerate() {
            let weight = dt*[1.0,2.0,2.0,1.0][k]/6.0;
            self.budgets.forcing_work += weight*rate.forcing;
            self.budgets.drag_work += weight*rate.drag;
            self.budgets.thermal_work += weight*rate.thermal;
            self.budgets.zonal_momentum_source_change += weight*rate.momentum;
        }
        self.budgets.filter_energy_change += filtered_energy-before_filter;
        self.budgets.energy_residual = filtered_energy-self.budgets.initial_energy
            -self.budgets.forcing_work-self.budgets.drag_work-self.budgets.thermal_work-self.budgets.filter_energy_change;
        self.budgets.zonal_momentum_residual = mean(&self.state[P])-self.budgets.initial_zonal_momentum
            -self.budgets.zonal_momentum_source_change;
        self.budgets.max_courant_rotation_bound = self.budgets.max_courant_rotation_bound.max(maximum_bound);
        self.refresh();
        Ok(())
    }

    fn refresh(&mut self) {
        for i in 0..self.h.len() {
            self.h[i] = self.state[H][i];
            self.u[i] = self.state[P][i]/self.h[i]; self.v[i] = self.state[Q][i]/self.h[i];
            self.buoyancy[i] = self.state[S][i]/self.h[i]; self.tracer[i] = self.state[T][i]/self.h[i];
        }
        if self.config.model == Model::Kinematic {
            let time = self.tick as f64*self.derived.step_nondimensional;
            let (a,b) = self.forcing(time);
            let scale = (self.derived.kinetic_injection_m2_s3*self.config.closure.drag_time_s).sqrt()
                .max(self.config.closure.initial_eddy_speed_m_s)/self.derived.wave_speed_m_s;
            for i in 0..self.h.len() {
                self.u[i] = scale*(a*self.force[0][i]+b*self.force[2][i]);
                self.v[i] = scale*(a*self.force[1][i]+b*self.force[3][i]);
            }
        }
        for i in 0..self.h.len() { self.flux_x[i] = self.v[i]; self.flux_y[i] = -self.u[i]; }
        self.spectral.divergence(&self.flux_x, &self.flux_y, &mut self.vorticity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fft_derivative_independent_trigonometric_oracle() {
        let n = 32;
        let mut fft = Spectral::new(n);
        let x: Vec<_> = (0..n*n).map(|i| (3.0*TAU*(i%n) as f64/n as f64).sin()).collect();
        let y: Vec<_> = (0..n*n).map(|i| (2.0*TAU*(i/n) as f64/n as f64).cos()).collect();
        let mut out = vec![0.0;n*n];
        fft.divergence(&x,&y,&mut out);
        for (i, actual) in out.iter().enumerate() {
            let exact = 3.0*(3.0*TAU*(i%n) as f64/n as f64).cos()
                -2.0*(2.0*TAU*(i/n) as f64/n as f64).sin();
            assert!((actual-exact).abs()<1e-12);
        }
    }
    #[test]
    fn forcing_is_solenoidal_without_direct_zonal_modes() {
        let n=32;
        let mut fft=Spectral::new(n);
        let mut u=vec![0.0;n*n]; let mut v=u.clone(); let mut div=u.clone();
        fft.eddies(21,3,8.0,&mut u,&mut v);
        fft.divergence(&u,&v,&mut div);
        assert!(div.iter().all(|a| a.abs()<2e-14));
        assert!((mean(&u.iter().zip(&v).map(|(u,v)|u*u+v*v).collect::<Vec<_>>())-1.0).abs()<1e-12);
        for row in u.chunks_exact(n) { assert!(mean(row).abs()<1e-14); }
    }
    #[test]
    fn resting_layer_remains_resting() {
        let mut config=Config::preset("rapid",1,32).unwrap();
        config.closure.initial_eddy_speed_m_s=0.0;
        config.planet.internal_heat_w_m2=0.0;
        let mut s=Solver::new(config).unwrap();
        for _ in 0..100 {s.step().unwrap();}
        assert!(s.fields().u.iter().chain(s.fields().v).all(|a| a.abs()<1e-13));
        assert!(s.fields().h.iter().all(|a|(a-1.0).abs()<1e-13));
    }
    #[test]
    fn reproducible_mass_conservation_and_seed_variation() {
        let config=Config::preset("rapid",7,32).unwrap();
        let mut a=Solver::new(config.clone()).unwrap(); let mut b=Solver::new(config).unwrap();
        for _ in 0..120 {a.step().unwrap();b.step().unwrap();}
        assert_eq!(a.fields().u,b.fields().u);
        assert!((mean(a.fields().h)-1.0).abs()<1e-12);
        assert!(a.budgets.energy_residual.abs()<1e-6);
        assert!(a.budgets.zonal_momentum_residual.abs()<1e-12);
        let other=Solver::new(Config::preset("rapid",8,32).unwrap()).unwrap();
        assert_ne!(a.fields().u,other.fields().u);
    }
    #[test]
    fn rejected_step_is_transactional() {
        let mut config=Config::preset("rapid",9,32).unwrap();config.numerics.step_s=1e7;
        let mut a=Solver::new(config).unwrap();let before=a.state.clone();
        assert!(a.step().is_err());assert_eq!(a.tick,0);assert_eq!(a.state,before);
        // Scratch diagnostic fields must also still describe the committed state.
        a.refresh(); assert_eq!(a.fields().h,&before[H]);
    }
}
