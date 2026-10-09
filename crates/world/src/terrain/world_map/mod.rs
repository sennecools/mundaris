//! L1 world map (`docs/PLANET_DATA_PIPELINE.md` §2, §5): per-body cube-map fields
//! of macro elevation and biome weights at about 1 km/texel. The world map
//! carries all relief above the 1–2 km band; library tiles only add detail.
//!
//! Generation is a deterministic CPU bake (`ai/tasks/stage2-world-map/PLAN.md`,
//! W1 Moon world-map generation). Runtime composition (W3) is not implemented.

mod catalog;
mod compose;
mod cube;
mod detail;
mod moon;
mod neukum;

pub use catalog::{
    BiomeCatalog, CATALOG_FORMAT, CatalogBiome, FieldRange, SubBiome, Variant, WorldField,
};
pub use compose::ComposedSurface;
pub use cube::{CubeMap, locate, neighbours, texel_direction, texel_directions};
pub use detail::{BUNDLE_FORMAT, DetailTile};
pub use moon::{
    BIOMES as MOON_BIOMES, BakeStats, BasinRecipe, BiomeRecipe, CraterRecipe, CrustRecipe,
    DegradationRecipe, LavaRecipe, MareRecipe, MoonWorldMap, MoonWorldMapRecipe, MorphologyRecipe,
    RECIPE_SCHEMA as MOON_RECIPE_SCHEMA, RayRecipe, SecondaryRecipe, bake as bake_moon,
};
pub use neukum::{cumulative as neukum_cumulative, n1 as neukum_n1};
