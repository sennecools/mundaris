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

The bootstrap implemented none of these domain systems. Phase 1 validates generic
rigid-frame coordinates and observer-relative precision. Phase 2 implements
authoritative celestial properties/kinematics and analytic requested-time control.
Phase 3 implements deterministic Newtonian gravity/KDK, fixed-step admission and
bounded history/replay, explicitly coherent frame publication, minimal celestial
debug rendering and committed-history trails. Numerical h never changes to catch
up with playback; navigation aids never change physical size or state. Render/frame
rebuilds cannot become authoritative. Generation, terrain/LOD and persistence
remain deferred. Platform evidence is tracked separately from implementation.

## Phase 3.5 navigation and time-warp invariants

- Selection/focus identity is `BodyId`; a projection rebuild or renderer request
  index cannot change it. Selection is distinct from camera tracking/focus.
- Physical spheres retain authoritative radius. Markers, displaced labels,
  selection rings, fitted bounds and navigation clearances are derived aids.
- Historical trail means committed synchronized state history. An instantaneous
  two-body orbit guide is separate derived geometry, never stored as fake past or
  presented as authoritative future propagation.
- Automatic guide references and subsystem scopes are disposable navigation data,
  never N-body parents, persistent ownership or transform/spin ancestry.
- Camera transitions and editor flight update the same high-precision observer;
  they mutate no physical body state or global world origin. Numerical carriers
  preserve the documented system-stationary free-flight policy.
- High requested rate changes counts only. Exact baseline h/KDK/order remain
  unchanged. Budget/count/admission limitations and achieved live rate are honest
  separate measurements; private replay is not live playback advancement.
- Hidden time and unacceptable interactive clock gaps cannot create silent enormous
  simulation debt or navigation jumps. Excluded wall duration and cancelled demand
  are visible, distinct from overload-rejected simulation demand.
- Content projection/picking/GPU viewport agree; source-centred f64 conversion and
  clipping precede GPU narrowing for spheres, guides and history alike.
