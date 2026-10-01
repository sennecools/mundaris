//! Requested simulation time control. State evolution is an explicit producer's job.
//! Serial Newtonian gravity uses inertial SI f64 state, independent of reference radius.

#![forbid(unsafe_code)]

mod gravity;
mod time;
pub use gravity::*;
pub use mundaris_math::SimulationInstant;
pub use time::*;
