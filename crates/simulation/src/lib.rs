//! Requested simulation time control. State evolution is an explicit producer's job.
//! Serial Newtonian gravity uses inertial SI f64 state, independent of reference radius.

#![forbid(unsafe_code)]

mod diagnostics;
mod gravity;
mod integrator;
mod time;
pub use diagnostics::*;
pub use gravity::*;
pub use integrator::*;
pub use mundaris_math::SimulationInstant;
pub use time::*;
