# High-level roadmap

These are exploratory phases, not promises or fixed delivery dates. Experiments, profiling, and architectural learning may change their order or scope. The [engine design](../MUNDARIS_ENGINE_DESIGN.md) supersedes the bootstrap's static-planet-first ordering: moving-frame precision must be validated before terrain work.

0. Repository and native graphics bootstrap
1. [Coordinates, reference frames, and precision](../MUNDARIS_PHASE_1_REFERENCE_FRAMES.md)
2. [Celestial model and simulation time](../MUNDARIS_PHASE_2_CELESTIAL_MODEL_AND_TIME.md)
3. [Gravity, orbits and basic celestial rendering](../MUNDARIS_PHASE_3_GRAVITY_ORBITS_AND_CELESTIAL_RENDERING.md) (specified; implementation not begun)
4. Planet surface partition/LOD prototype
5. Procedural base terrain
6. Terrain representation and sparse-edit prototype
7. Atmosphere and ocean baseline
8. Environment and biomes
9. Vegetation hierarchy
10. World-builder editing workflow
11. Deeper simulation experiments: hydrology, erosion, tectonics, climate, and local physics

Repository bootstrap, Phase 1 reference frames and Phase 2 celestial model/time implementations exist today. [Phase 1 evidence](phase-1-validation.md) and [Phase 2 evidence](phase-2-validation.md) distinguish implemented functionality from cross-platform acceptance.

Phase 1 validation uses generic moving frames and debug primitives. Phase 2 adds
authoritative body identity/properties/kinematics, explicit requested/sample time,
transactional batches, disposable two-frame body projections and an analytic
editor fixture. Its Windows automated/release checks and native startup/close
pass. Full visual sequences, Linux validation and current-change remote CI remain
open; Phase 1's pending cross-platform acceptance has not been waived. No Phase 3
gravity, orbital mechanics, terrain or full celestial rendering is implemented.
