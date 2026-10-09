//! Requested simulation time control. State evolution is an explicit producer's job.
//! Serial Newtonian gravity uses inertial SI f64 state, independent of reference radius.

#![forbid(unsafe_code)]

mod analytic_motion;
mod diagnostics;
mod gravity;
mod history;
mod integrator;
mod orbital_elements;
mod runner;
mod time;
pub use analytic_motion::*;
pub use diagnostics::*;
pub use gravity::*;
pub use integrator::*;
pub use astrum_math::SimulationInstant;
pub use orbital_elements::*;
pub use runner::*;
pub use time::*;
