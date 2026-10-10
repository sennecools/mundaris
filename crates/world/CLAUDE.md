# astrum_world: bodies, systems, terrain definitions

Loaded automatically when you work in this crate. Terrain has its own guide in
`src/terrain/CLAUDE.md`. Map notes: `D:/Astrum/Astrum/systems/` (Solar system
and orbits, World map, Landforms, Rivers and lakes, Ground materials, Archetypes
and content).

## Owns
Authoritative celestial bodies and their kinematics in one system basis and
instant (`system.rs`, `body.rs`, `celestial_motion.rs`), the disposable frame
projection (`frame_projection.rs`), and all terrain definitions plus the CPU
terrain oracle (`terrain/`). Depends on `astrum_math` and `glam` only.

## Never
- No `wgpu`, `winit` or `egui` dependency. GPU code lives in the renderer and
  app; this crate holds the CPU oracle and data the GPU side mirrors.
- Reference frames are a projection, never the universe database.
- Do not reuse body/tree namespaces for independent instances; body storage is
  append-only and handles are not persisted.
- No ambient RNG or time in generation: same seed + inputs = same terrain.

## Rules that bite
- The GPU generator is the terrain authority (ADR 0023). Code here is the
  **test oracle**: change it together with the matching WGSL, never alone.
- On-disk format IDs stay `mundaris.*` (their hashes depend on it).
- `cargo fmt --all` rewrites ~157 unrelated files; format only what you touch
  (`rustfmt <file>`).

## Ownership (parallel lanes, `D:/Astrum/ai/lanes/LANES.md`)
`src/terrain/**` belongs to the Terrain lane, except `terrain/scatter.rs` (Flora
lane). `terrain/archetype.rs` is a seam shared with Studio. The rest of the
crate: ask the Coordinator.

## Test
`cargo test -p astrum_world` (headless). Terrain tests: `terrain_generation`,
`terrain_erosion`, `terrain_bands`, `terrain_bounds`, `terrain_craters`.

## Contracts
`docs/architecture.md` ("Celestial domain and time", "Amendment 2026-10-09"),
ADRs 0003, 0007, 0023, `docs/ASTRUM_TERRAIN_PIPELINE.md` (search by section).
