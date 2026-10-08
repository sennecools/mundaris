//! App-owned validation fixtures and disposable observer/debug session state.
#![forbid(unsafe_code)]

pub mod celestial_camera;
pub mod celestial_labels;
pub mod celestial_selection;
#[cfg(feature = "developer-tools")]
pub mod developer_bridge;
#[cfg(feature = "terrain-capture")]
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
pub mod planet_lod;
pub mod playback_metrics;
pub mod profile_export;
pub mod profiler;
pub mod shared_system;
pub mod sky_definition;
pub mod solar_system;
pub mod studio;
pub mod surface_anchor;
pub mod system_view;
pub mod terrain_inspection;
pub mod terrain_profile;
pub use gravity_orbits::GravityOrbitsDemo;
/// Toolkit-neutral viewport input consumed by [`GravityOrbitsDemo::viewport_event`].
pub use gravity_orbits::navigation_input as viewport;
pub mod trails;
