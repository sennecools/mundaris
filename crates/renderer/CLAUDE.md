# astrum_renderer: GPU presentation

Loaded automatically when you work in this crate. Map notes:
`D:/Astrum/Astrum/systems/` (Terrain LOD and atlas, Lighting and HDR, Sky,
Profiling).

## Owns
Rendering the scene into an offscreen texture the app shows inside its UI, and
all disposable GPU resources: terrain atlas and tile producer
(`terrain_atlas.rs`, `tier_a.rs`, `terrain_rivers.rs`, `terrain_capture.rs`),
lighting, shadows and post (`lighting.rs`, `shadows.rs`, `post.rs`), sky
(`sky*.rs`), celestial bodies and orbit lines (`celestial*.rs`), GPU/CPU
profiling, and WGSL in `src/shaders/`. Depends on `astrum_math`, `wgpu`, `glam`.

## Never
- Never own world or simulation truth; everything here must be rebuildable.
- f64 never reaches the GPU. Narrow to observer-relative f32 at the boundary
  and check finite output and range.
- No upload of a partially failed frame.

## Rules that bite
- Terrain WGSL is the authority for terrain (ADR 0023); its CPU twin is in
  `crates/world/src/terrain`. Change both, run the oracle tests in `crates/app/tests/gpu_*`.
- WGSL is validated headlessly by naga in tests; a shader that compiles in the
  app but fails naga still fails CI.
- The device asks for 8 storage textures per stage (overlays); check limits
  before adding storage bindings.
- `GpuContext::new_software` / `TerrainCaptureRenderer::new_software` validate
  without a GPU.

## Ownership (parallel lanes)
Terrain lane: `terrain_atlas.rs`, `terrain_capture.rs`, `terrain_rivers.rs`,
`tier_a.rs`, shaders `terrain_*`, `tier_a`, `landform_eval`, `river_carve`,
`material_rules`, `cube_map`; `lib.rs` is a Terrain seam. Flora lane:
`scatter*.wgsl`, new `flora*.rs`/`scatter*.rs`. Lighting, sky, celestial:
ask the Coordinator.

## Test
`cargo test -p astrum_renderer`; GPU paths are exercised from
`cargo test -p astrum_app` (`tests/gpu_*`). Benches:
`view_preparation`, `celestial_preparation`.

## Contracts
`docs/RENDER_PIPELINE_HDR.md` (§3.1 amendments), `docs/SKY_V2.md`,
`docs/PLANET_TERRAIN_ATLAS_LOD.md`, ADRs 0016, 0018, 0019, 0023.
