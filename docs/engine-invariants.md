# Long-term engine invariants

These constraints preserve room for future experiments. They do not prescribe a complete implementation or commit the project to a specific algorithm.

## Scale, motion, and frames

- **Seamless scale:** support observation from astronomical to local surface scales without treating them as unrelated worlds or requiring visible level transitions. Internal representations and frames may change as an implementation detail.
- **Moving bodies:** planets and moons may translate and rotate. Their local terrain, vegetation, objects, and observers should be expressible relative to bodies rather than stored only in one universal coordinate system.
- **Hierarchical frames:** allow a future hierarchy such as universe → stellar system → celestial body → local surface → observer/render frame.
- **Precision boundaries:** use `f64` for authoritative long-distance simulation where precision matters and `f32` for local GPU data. Convert intentionally at clear boundaries; do not casually downcast in domain code.
- **Observer-relative rendering:** derive visible work and detail around observers. The renderer is a view of authoritative state, not its owner.

## Authoritative and derived data

- **Procedural data is authoritative; meshes are caches.** Meshes and GPU data must be rebuildable from world definitions, generation inputs, and edits.
- **Deterministic generation:** the same project/world and generator versions, seed, coordinates, and configuration should produce the same untouched region. Do not rely on ambient RNG or system time for authoritative generation.
- **Sparse edits:** persistent data should primarily record deviations from a procedural base. User edits must outlive cache invalidation, unloading, and changes in LOD.
- **Locally editable terrain:** keep a path open for caves, tunnels, overhangs, excavation, craters, and added material. A hybrid global surface plus sparse local volumetric modifications is a possible direction, not an implemented representation.
- **Future persistence:** save formats will need explicit versions and migration strategies; authoritative edits are valuable data and caches must remain safely rebuildable.

## Independent systems and representations

- **Independent layers:** terrain, vegetation, water, atmosphere, simulation, and rendering are separate systems. Do not bake vegetation into terrain meshes or let the renderer own world state.
- **Hierarchical LOD:** reduce representation quality continuously with distance. An individual may disappear only when its contribution has transitioned into a higher-level aggregate representation.
- **Multi-scale generation:** planetary structure, regional terrain, and local detail are meaningful scales. One noise function should not be stretched to every scale.
- **Replaceable algorithms:** approximations such as simple terrain, orbit, biome, or water models should be replaceable without unrelated systems being rewritten. Interfaces should emerge from concrete needs rather than hypothetical variants.
- **Generation and rendering separation:** later terrain algorithms, including erosion- or tectonic-informed approaches, must not require the renderer to know how authoritative terrain was produced.

## Future domain directions

- Vegetation should be independently addressable, deterministically generated from environmental inputs, and sparsely overridable. Distant forest aggregates may replace individual instances as a representation.
- Biome/ecological classification should be able to derive from environmental fields such as temperature, moisture, elevation, slope, and latitude rather than being permanently limited to painted integer regions.
- Initial ocean rendering may use a simple sea-level surface; detailed fluid simulation is not an early requirement. Hydrology may produce rivers without simulating every volume of water.
- Celestial motion belongs to simulation state. Body transforms should carry body-local features into other frames instead of individually updating every attached feature.
- Streaming applies to derived representations and caches; distant authoritative world state does not cease to exist when unloaded.
- Future parallel work should favor immutable inputs and produced outputs, batch work, minimize synchronization, and preserve determinism where required.

The bootstrap implemented none of these domain systems. Phase 1 validates generic rigid-frame coordinates and observer-relative precision; Phase 2 implements authoritative celestial properties/kinematics and requested-time control with an analytic fixture. Gravitational evolution and the generation/terrain/editing systems above remain deferred. Phase 3 gravity/orbits/celestial rendering is specified separately and is not implemented.
