//! App-owned validation fixtures and disposable observer/debug session state.
#![forbid(unsafe_code)]

pub mod celestial_camera;
pub mod celestial_labels;
pub mod celestial_selection;
#[cfg(feature = "developer-tools")]
pub mod developer_bridge;
pub mod developer_capture;
#[cfg(feature = "developer-tools")]
pub mod developer_protocol;
#[cfg(feature = "developer-tools")]
pub mod developer_scenarios;
#[cfg(feature = "developer-tools")]
pub mod developer_service;
pub mod developer_snapshot;
pub mod engine_profile;
pub mod gravity_fixtures;
mod gravity_orbits;
pub mod interactive_clock;
pub mod motion_session;
pub mod orbit_guides;
pub mod performance_capture;
pub mod performance_lab;
pub mod planet_surface;
pub mod planet_terrain;
pub mod playback_metrics;
pub mod regional_terrain;
pub mod resident_terrain;
#[cfg(feature = "terrain-capture")]
pub mod sky_capture;
pub mod sky_definition;
pub mod solar_system;
pub mod surface_probe;
pub mod system_view;
pub mod terrain_inspection;
pub mod terrain_population;
pub mod terrain_trace;
pub use gravity_orbits::GravityOrbitsDemo;
pub mod trails;
