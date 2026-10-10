# world::terrain: terrain definitions and the CPU oracle

Owner: **Terrain lane** (`D:/Astrum/ai/lanes/LANES.md`), except `scatter.rs`
(Flora lane). Live state: `D:/Astrum/ai/STATE.md`. Map notes in
`D:/Astrum/Astrum/systems/`.

## The one rule
The GPU is the terrain authority (ADR 0023). Everything here is either data the
GPU reads or the **CPU oracle** the GPU is tested against. A change on one side
needs the matching change on the other and a passing oracle test
(`crates/app/tests/gpu_terrain_oracle.rs`, `gpu_tier_a.rs`, `gpu_terrain_seams.rs`).

## Pipeline order (coarse to fine)
| Stage | Module | GPU side | Milestone |
| --- | --- | --- | --- |
| Planet definition | `archetype.rs` (`ParamField` registry), `biome_lut.rs` | — | M1 |
| World map (Tier A): plates, uplift, climate, macro erosion | `tier_a/` (`tectonics.rs`, `climate.rs`, `erosion.rs`) | `tier_a.wgsl`, `renderer/src/tier_a.rs` | M1–M2 |
| Landforms: recipes weighted by the world map | `landform/` (`ir.rs`, `expr.rs`, `eval.rs`, `gpu.rs`, `world_source.rs`) | `landform_eval.wgsl` | M2 |
| Detail noise ladders | `surface/ladder.rs`, `surface/noise.rs` | `terrain_noise.wgsl` | M0–M2 |
| Rivers and lakes | `hydrology/` (`flood.rs`, `rivers.rs`, `carve.rs`) | `river_carve.wgsl` | M3 |
| Ground materials | `material_rules.rs` | `material_rules.wgsl` | M4 |
| Scatter (Flora lane) | `scatter.rs` | `scatter*.wgsl` | M5 |
| Queries and CPU generator | `generator.rs`, `surface_query.rs`, `query.rs`, `prepared.rs` | atlas producer | M0 |

Older material kept as reference only: `world_map/` (W1 Moon craters,
`neukum.rs`, `cube.rs`), `moon/`, `crater.rs`, parts of `surface/`.

## Rules that bite
- Landform recipe schema (`landform/schema.rs`) belongs to Terrain; Studio edits
  recipes only through the loader/serializer.
- Recipes and archetypes are data (`repo/content/landforms`, `content/archetypes`)
  and hot-reload in the running app; tune data before code.
- The GPU interpreter caps a recipe at 16 ops (prototype debt, plan step 6b).
- Noise anchors are one i32 per tile: bodies up to ~1,000 km radius only.
- Score shape changes before claiming them: `crates/app/examples/terrain_grid_export.rs`
  + `D:/Astrum/ai/research/terrain/tools/terrain-scorecard` against the DEM scorecard.

## Test
`cargo test -p astrum_world` for the oracle; GPU comparisons:
`cargo test -p astrum_app --test gpu_terrain_oracle` (also `gpu_tier_a`,
`gpu_terrain_seams`; they return early without an adapter). Lanes use their own `CARGO_TARGET_DIR`.

## Contracts
`docs/ASTRUM_TERRAIN_PIPELINE.md`: read only the sections your step names
(§0 holds the amendments). Milestone plans: `D:/Astrum/ai/tasks/m2-shape/`, `m3-water/`, `m4-surface/`, `m5-life/`.
