//! L1 world map (`docs/PLANET_DATA_PIPELINE.md` §2, §5): per-body cube-map fields
//! of macro elevation and biome weights at about 1 km/texel. The world map
//! carries all relief above the 1–2 km band; library tiles only add detail.
//!
//! Generation is a deterministic CPU bake (`ai/tasks/stage2-world-map/PLAN.md`,
//! W1 Moon world-map generation). Runtime composition (W3) is not implemented.

mod cube;
mod moon;
mod neukum;

pub use cube::{CubeMap, locate, texel_direction, texel_directions};
pub use moon::{
    BIOMES as MOON_BIOMES, BakeStats, BasinRecipe, BiomeRecipe, CraterRecipe, CrustRecipe,
    DegradationRecipe, MareRecipe, MoonWorldMap, MoonWorldMapRecipe,
    RECIPE_SCHEMA as MOON_RECIPE_SCHEMA, bake as bake_moon,
};
pub use neukum::{cumulative as neukum_cumulative, n1 as neukum_n1};
