//! App-owned validation fixtures and disposable observer/debug session state.
#![forbid(unsafe_code)]

pub mod celestial_camera;
pub mod celestial_labels;
pub mod celestial_selection;
pub mod gravity_fixtures;
mod gravity_orbits;
pub mod interactive_clock;
pub mod orbit_guides;
pub mod planet_surface;
pub mod playback_metrics;
pub mod system_view;
pub use gravity_orbits::GravityOrbitsDemo;
pub mod trails;
