//! Checked reference-frame mathematics for Mundaris.
//!
//! Physical values use `f64`, metres, seconds and radians. Frames are right-handed
//! and orthonormal; positive angles follow the right-hand rule. Column-vector
//! composition applies the inner transform first. Camera forward is local `-Z`.
//! Only the renderer narrows physical values to observer-relative `f32` data.

#![forbid(unsafe_code)]

mod coordinates;
mod frames;
pub mod noise;
pub mod surface;
mod time;
mod transform;

pub use coordinates::*;
pub use frames::*;
pub use time::*;
pub use transform::*;
