//! Authoritative celestial bodies in one finite system coordinate basis and instant.
//! Reference frames are a disposable projection, never the universe database.
//! Body and tree namespaces are caller-assigned and must not be reused for independent
//! instances. Body storage is append-only; handles have no persistence contract.

#![forbid(unsafe_code)]

mod body;
mod celestial_motion;
mod frame_projection;
mod system;
pub mod terrain;

pub use body::*;
pub use celestial_motion::*;
pub use frame_projection::*;
pub use astrum_math::SimulationInstant;
pub use system::*;
