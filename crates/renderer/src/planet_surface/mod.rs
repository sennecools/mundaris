//! Disposable smooth-sphere topology and observer-dependent surface policy.
//! Physical state and body capability remain outside this renderer module.
mod bounds;
mod cache;
mod gpu;
mod lod;
mod prepare;
mod topology;
pub(crate) use bounds::projected_error as sphere_projected_error;
pub use bounds::{PatchMetadata, SurfaceExtent};
use cache::MetadataCache;
pub(crate) use gpu::PlanetSurfaceRenderer;
pub use lod::{ActiveSurfacePatch, LodReport, LodSettings, SurfaceLodSession, SurfaceViewInput};
pub(crate) use prepare::SurfaceStaging;
pub use prepare::{SurfacePreparationReport, SurfaceStyle};
pub use topology::{GRID_CELLS, GRID_SAMPLES, SurfaceTopology};
