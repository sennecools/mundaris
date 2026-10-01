//! Requested simulation time control. State evolution is an explicit producer's job.
//! Gravity, integration and fixed-step/catch-up policies are not implemented.

#![forbid(unsafe_code)]

mod time;
pub use mundaris_math::SimulationInstant;
pub use time::*;
