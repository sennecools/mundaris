# High-level roadmap

These are exploratory phases, not promises or fixed delivery dates. Experiments, profiling, and architectural learning may change their order or scope. The [engine design](../MUNDARIS_ENGINE_DESIGN.md) supersedes the bootstrap's static-planet-first ordering: moving-frame precision must be validated before terrain work.

0. Repository and native graphics bootstrap
1. [Coordinates, reference frames, and precision](../MUNDARIS_PHASE_1_REFERENCE_FRAMES.md)
2. Celestial bodies and simulation clock
3. Basic planet representation
4. Planet surface partition/LOD prototype
5. Procedural base terrain
6. Terrain representation and sparse-edit prototype
7. Atmosphere and ocean baseline
8. Environment and biomes
9. Vegetation hierarchy
10. World-builder editing workflow
11. Deeper simulation experiments: hydrology, erosion, tectonics, climate, and local physics

Only repository bootstrap and the smallest graphics/UI smoke application exist today.

Phase 1 is specified, not implemented. Its validation uses generic moving frames and debug primitives; it does not introduce planets, terrain, gravity, or orbital mechanics.
