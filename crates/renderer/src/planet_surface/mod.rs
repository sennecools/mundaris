//! Disposable planetary geometry, observer-dependent surface policy and shading.
//! Physical state and body capability remain outside this renderer module.
mod bounds;
mod cache;
mod cover;
#[cfg(feature = "surface-profile")]
mod cpu_profile;
mod gpu;
mod lighting;
mod lod;
mod prepare;
mod stitching;
mod terrain_geometry;
mod topology;
mod transition;
pub(crate) use bounds::projected_error as sphere_projected_error;
pub use bounds::{PatchMetadata, SurfaceErrorContributions, SurfaceExtent};
use cache::MetadataCache;
#[cfg(feature = "surface-profile")]
pub use cpu_profile::CpuStageTimer;
pub(crate) use gpu::PlanetSurfaceRenderer;
pub use lighting::{
    TerrainLighting, TerrainReadability, TerrainRenderMode, TerrainSunPreset, lod_color,
};
#[cfg(feature = "surface-profile")]
pub use lod::LodProfile;
pub use lod::{
    ActiveSurfacePatch, LodReport, LodSettings, SurfaceGeometryPolicy, SurfaceLodSession,
    SurfaceViewInput,
};
#[cfg(feature = "surface-profile")]
pub use prepare::SurfacePreparationProfile;
pub(crate) use prepare::SurfaceStaging;
pub use prepare::{SurfacePreparationReport, SurfaceStyle};
pub use stitching::{StitchedSurface, active_surface_cover};
pub use terrain_geometry::{GeneratedSurfacePatch, SurfaceGeometrySample};
pub use topology::{GRID_CELLS, GRID_SAMPLES, SurfaceTopology};
#[cfg(feature = "surface-profile")]
pub use transition::SurfaceTransitionProfile;
pub use transition::{SurfaceTransition, SurfaceTriangleReference, TransitionVertex};
