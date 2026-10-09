//! Cube-map world-map storage and the parked W1 Moon world-map bake.
//!
//! `cube` is the shared cube-face field storage used by the Tier A world map
//! (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6). `moon` and `neukum` are the parked W1
//! Moon bake (`ai/tasks/stage2-world-map/PLAN.md`), kept as starting material for
//! M8 (Variety) crater and maria work.

mod cube;
mod moon;
mod neukum;

pub use cube::{CubeMap, locate, neighbours, texel_direction, texel_directions};
pub use moon::{
    BIOMES as MOON_BIOMES, BakeStats, BasinRecipe, BiomeRecipe, CraterRecipe, CrustRecipe,
    DegradationRecipe, LavaRecipe, MareRecipe, MoonWorldMap, MoonWorldMapRecipe, MorphologyRecipe,
    RECIPE_SCHEMA as MOON_RECIPE_SCHEMA, RayRecipe, SecondaryRecipe, bake as bake_moon,
};
pub use neukum::{cumulative as neukum_cumulative, n1 as neukum_n1};
