//! Disposable planetary geometry, observer-dependent surface policy and shading.
//! Physical state and body capability remain outside this renderer module.
mod bounds;
mod cache;
mod cover;
mod gpu;
mod lighting;
mod lod;
mod prepare;
mod terrain_geometry;
mod topology;
pub(crate) use bounds::projected_error as sphere_projected_error;
pub use bounds::{PatchMetadata, SurfaceErrorContributions, SurfaceExtent};
use cache::MetadataCache;
pub(crate) use gpu::PlanetSurfaceRenderer;
pub use lighting::{TerrainLighting, TerrainRenderMode, TerrainSunPreset};
#[cfg(feature = "surface-profile")]
pub use lod::LodProfile;
pub use lod::{ActiveSurfacePatch, LodReport, LodSettings, SurfaceLodSession, SurfaceViewInput};
#[cfg(feature = "surface-profile")]
pub use prepare::SurfacePreparationProfile;
pub(crate) use prepare::SurfaceStaging;
pub use prepare::{SurfacePreparationReport, SurfaceStyle};
pub use terrain_geometry::{GeneratedSurfacePatch, SurfaceGeometrySample};
pub use topology::{GRID_CELLS, GRID_SAMPLES, SurfaceTopology};
