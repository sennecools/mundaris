# Mundaris — Engine Design Document

> **Document type:** architectural north star
>
> **Status:** living design document
>
> **Repository:** `mundaris`
>
> **Product:** Mundaris
>
> **Audience:** engine developers, technical reviewers, implementation agents, and future maintainers
>
> **Important:** this document defines architectural intent, invariants, subsystem boundaries, and long-term constraints. It is **not** an instruction to implement every system described here. Implementation must happen through small, phase-specific design and execution documents.

---

# 1. Purpose

Mundaris is a native desktop world-building and planetary simulation application focused on **continuous scale, editable procedural worlds, and physically coherent celestial motion**.

The long-term goal is to let a user create a star system, generate or author planets and moons, adjust physical and procedural parameters, simulate selected systems, and move seamlessly from astronomical views down to a human-scale observer standing on the surface.

The product should eventually support experiences such as:

1. create a star;
2. create a planet with configurable mass, radius, rotation, orbit, atmosphere, water, terrain, and moons;
3. let that planet orbit and rotate in a physically meaningful reference frame;
4. observe it from millions of kilometres away;
5. approach the same planet without changing to a separate level;
6. see continents, mountain systems, oceans, biomes, forests, and increasingly local detail emerge through hierarchical representations;
7. descend through the atmosphere;
8. stand at human scale on terrain that is consistent with what was visible from orbit;
9. edit terrain locally, including destructive edits that may later support tunnels, excavation, craters, and added material;
10. remove or place individual vegetation and objects without requiring the entire procedural planet to be stored explicitly;
11. change planetary or orbital parameters and observe the consequences at the appropriate simulation level;
12. save the authored world in a deterministic, versioned project format.

Mundaris is **not primarily a game engine**. It may eventually contain first-person navigation, interaction, and game-like controls because they are useful ways to experience scale, but gameplay systems are not the architectural center of the project.

The engine should be designed first as a **world authoring and simulation platform**.

---

# 2. Product North Star

A useful long-term demonstration of Mundaris is the following sequence:

```text
Create star system
      ↓
Create procedural planet
      ↓
Give planet one or more moons
      ↓
Run orbital/rotational simulation
      ↓
Observe planet from astronomical distance
      ↓
Approach continuously
      ↓
Continents resolve into regions
      ↓
Regions resolve into mountain systems and valleys
      ↓
Biomes and forest canopies become visible
      ↓
Individual trees and terrain detail appear
      ↓
Enter surface observer mode
      ↓
Walk beneath the same mountain seen from orbit
      ↓
Look up and see the configured moon moving through the sky
      ↓
Edit terrain or remove a tree
      ↓
Leave, unload the region, return later
      ↓
The procedural world regenerates identically and the sparse edits remain
```

If Mundaris can eventually perform that sequence robustly, the central architecture has succeeded.

---

# 3. What Mundaris Is Optimizing For

Mundaris is not trying to maximize the number of simultaneously detailed planets.

It is optimizing for:

- enormous apparent scale;
- continuity between scales;
- high local detail where the observer needs it;
- deterministic procedural generation;
- editable generated worlds;
- sparse persistence;
- moving and rotating celestial bodies;
- replaceable simulation/generation algorithms;
- strong performance on consumer hardware;
- tooling suitable for world builders and technical users;
- architecture that can grow for years without requiring every early prototype to become permanent.

A solar system containing ten bodies that can each be explored and edited deeply is more aligned with Mundaris than a universe containing trillions of bodies with shallow interaction.

Large-scale procedural generation may be added later, but **depth per authored world** is the primary differentiator.

---

# 4. What Mundaris Is Not Optimizing For

The initial engine architecture should not be shaped around:

- multiplayer;
- MMO-scale networking;
- competitive gameplay;
- deterministic lockstep networking;
- console support;
- mobile support;
- browser/WebGPU support;
- public modding APIs;
- scripting ecosystems;
- ECS purity;
- physically exact simulation of every natural process;
- full fluid simulation;
- direct simulation of every tree, rock, cloud particle, water molecule, or terrain grain;
- enormous explicit databases of generated content;
- reproducing a real universe at catalogue scale;
- photorealism at the expense of architecture and interactivity.

None of these are forbidden forever. They are simply not current architectural drivers.

---

# 5. Decision Classes

Every important design statement should belong to one of four classes.

## 5.1 Locked invariant

A principle expected to survive major engine rewrites.

Examples:

- meshes are not authoritative world state;
- procedural generation must be deterministic;
- celestial bodies need body-local coordinate systems;
- astronomical precision must not depend on GPU `f32` world coordinates;
- persistent edits are conceptually overlays on procedural/generated state.

Changing a locked invariant requires an explicit architectural review.

## 5.2 Current architecture decision

A concrete decision used by the current engine and expected to remain for a significant period, but replaceable if evidence justifies it.

Examples:

- Rust;
- `wgpu`;
- `winit`;
- `glam`;
- `egui`;
- the current six-crate workspace.

## 5.3 Preferred direction

A currently favored solution that should guide experiments but must not be treated as permanent until validated.

Examples:

- cube-sphere planetary partitioning;
- quadtree-like surface refinement;
- density-field overlays for local terrain edits;
- GPU-assisted procedural terrain evaluation;
- tree cluster/impostor hierarchy.

## 5.4 Open question

A decision intentionally deferred until requirements, prototypes, or measurements provide enough evidence.

Examples:

- exact planet LOD topology;
- exact terrain meshing algorithm for editable volumetric areas;
- clipmap vs patch hierarchy combinations;
- job scheduler design;
- long-term asset format;
- exact atmosphere technique;
- exact hydrology model.

This classification exists to avoid two opposite failures:

1. **prematurely freezing experimental algorithms**, and
2. **leaving foundational invariants vague until implementation accidentally decides them**.

---

# 6. Locked Engine Invariants

The following are the architectural constitution of Mundaris.

## 6.1 Seamless conceptual world

Surface, atmosphere, orbit, and interplanetary space must not be treated as unrelated levels.

The engine may use different coordinate frames, render representations, simulation fidelities, and caches at different scales, but those transitions are implementation details.

A user should perceive one continuous world.

## 6.2 Celestial bodies move

Planets and moons are not static backdrops.

Their position, orientation, linear velocity, and angular state must be representable independently from the surface content attached to them.

Terrain, vegetation, water, structures, and surface observers belong to a celestial body or another reference frame. They must not need to be individually translated through universal coordinates whenever the body moves.

## 6.3 Hierarchical reference frames

The engine must support nested coordinate frames.

Conceptually:

```text
universe / root
    ↓
stellar system
    ↓
celestial body
    ↓
regional or surface-local frame
    ↓
observer / render frame
```

The exact number and type of frames may evolve.

The invariant is that **one global Cartesian coordinate space cannot be assumed sufficient for every subsystem and scale**.

## 6.4 Explicit precision boundaries

Authoritative long-distance simulation should use sufficient precision, expected primarily to mean `f64` for astronomical transforms and calculations.

GPU-facing local geometry will generally use `f32`.

Conversion between the two is deliberate and occurs at owned boundaries.

No subsystem may silently assume that a world-space `Vec3<f32>` can represent astronomical and human scale simultaneously.

## 6.5 Procedural definition is authoritative

Generated geometry is derived state.

The authoritative world consists of:

```text
world parameters
+ seeds
+ authored configuration
+ simulation state
+ sparse persistent edits
```

Meshes, GPU buffers, impostors, collision meshes, and derived textures are caches.

They may be destroyed and regenerated at any time.

## 6.6 Deterministic generation

Given the same:

- project data;
- generator version;
- seed;
- coordinates/identifiers;
- relevant parameters;

untouched generated content should reproduce the same result.

Procedural systems must not depend on ambient global random-number state.

## 6.7 Sparse modifications

A user should not need to store an entire planet because one tree was removed or one tunnel was excavated.

Persistent world state should conceptually be:

```text
procedural/generated base
          +
      sparse edits
          =
      authored world
```

This pattern should be applicable independently to terrain, vegetation, objects, and other generated layers.

## 6.8 Local volumetric terrain must remain possible

The global terrain system may make extensive use of surface/elevation representations, but the architecture cannot permanently define terrain as only a two-dimensional height field.

Future local edits may need:

- caves;
- tunnels;
- overhangs;
- excavation;
- craters;
- added material;
- local terrain sculpting.

The expected direction is a hybrid system in which most of a planet is represented cheaply while modified regions can opt into richer volumetric data.

## 6.9 Independent world layers

At minimum, these concepts remain separable:

- celestial dynamics;
- terrain definition;
- terrain edits;
- biome/environment fields;
- vegetation;
- placed objects;
- water/hydrology;
- atmosphere/weather;
- rendering;
- editor/UI.

Their visual representations can interact, but no layer should become accidental storage for another.

## 6.10 No conceptual hard world draw cutoff

Mundaris should aim for continuous reduction in detail with distance rather than an obvious arbitrary terrain boundary.

This does **not** mean infinite detail.

It means distant information transitions into cheaper representations.

Examples:

```text
individual tree
→ simplified tree
→ tree cluster
→ canopy field
→ biome/terrain contribution
```

and:

```text
local terrain
→ regional terrain
→ continental terrain
→ planetary surface
```

## 6.11 Multi-scale generation

Fine detail must refine large-scale features rather than replace them.

A mountain visible from orbit should remain the same mountain when approached.

Planet generation should therefore be compositional across scales rather than one giant high-frequency noise function.

## 6.12 Observer-relative rendering

The renderer should spend detail according to perceptual and geometric contribution to the current view.

The entire authored world does not need equal representation at once.

GPU positions must also be relative to an observer/render origin. Detail selection alone does not establish numerical precision; the conversion contract in Section 13 applies independently of LOD.

## 6.13 Algorithms are replaceable

Noise generation, erosion approximation, tectonics, atmosphere rendering, tree placement, LOD selection, and similar algorithms must not own the persistent world format more than necessary.

Where possible, algorithms implement well-defined roles and can be replaced without changing unrelated world state.

## 6.14 Editor actions mutate world intent, not render caches

User edits target authoritative world state.

Example:

```text
user excavates terrain
        ↓
terrain edit layer changes
        ↓
affected derived caches are invalidated
        ↓
LOD meshes / collision / GPU data regenerate
```

Never:

```text
user edits currently visible mesh
        ↓
mesh becomes the only record of the edit
```

---

# 7. Current Technology Foundation

Current architecture decisions:

- language: Rust stable;
- graphics API abstraction: `wgpu`;
- windowing/event loop: `winit`;
- math: `glam`;
- editor/developer UI: `egui`;
- supported native platforms: Windows and Linux;
- repository visibility: private/proprietary;
- primary graphics backends: those exposed through `wgpu` and supported by the target OS/hardware;
- source control: Git;
- CI: fast quality/compatibility checks.

These choices are foundational for the current codebase but are not philosophical requirements of the product itself.

---

# 8. Workspace Architecture

The bootstrap currently uses six primary crates:

```text
crates/
├── app/
├── core/
├── math/
├── simulation/
├── renderer/
└── world/
```

Their logical package names are:

```text
mundaris_app
mundaris_core
mundaris_math
mundaris_simulation
mundaris_renderer
mundaris_world
```

The dependency intent as engine functionality is introduced is:

```text
mundaris_app
    ├── mundaris_renderer
    ├── mundaris_simulation
    ├── mundaris_world
    ├── mundaris_math
    └── mundaris_core

mundaris_renderer
    ├── mundaris_math
    └── mundaris_core

mundaris_simulation
    ├── mundaris_world
    ├── mundaris_math
    └── mundaris_core

mundaris_world
    ├── mundaris_math
    └── mundaris_core

mundaris_math
    └── mundaris_core only when genuinely useful

mundaris_core
    └── no domain dependency upward
```

Current project dependencies are app → renderer/math/world/simulation, renderer → math, world → math, and simulation → math/world. Math owns frame algorithms and the checked instant value; world owns celestial state and disposable frame projection; simulation owns gravity/KDK and fixed-step playback/history. Core remains documentation-only. Add dependencies for real callers, not to reproduce the diagram. [Phase 1 validation](docs/phase-1-validation.md), [Phase 2 validation](docs/phase-2-validation.md), and [Phase 3 validation](docs/phase-3-validation.md) distinguish implementation from outstanding platform/remote-CI acceptance.

This structure is intentionally small.

New crates should only be introduced when there is a concrete architectural or build reason, not because every subsystem name deserves a crate.

Potential future crates such as `terrain`, `procedural`, `assets`, or `editor` are **not pre-approved**. They should emerge only when existing crate boundaries become meaningfully overloaded.

---

# 9. Crate Responsibilities

## 9.1 `mundaris_core`

Owns low-level project-wide concepts with no domain dependence.

Appropriate examples:

- strongly typed stable identifiers;
- small shared utility abstractions;
- common diagnostic types;
- version identifiers;
- carefully selected result/error infrastructure;
- generic time/index helpers when truly cross-domain.

It must not become a miscellaneous dumping ground.

## 9.2 `mundaris_math`

Owns mathematical concepts specific enough to Mundaris that raw `glam` types are insufficient.

Expected future responsibilities:

- reference-frame transforms;
- precision-aware coordinate types;
- unit-safe or unit-explicit helpers;
- astronomical/local conversion utilities;
- geometric queries shared by world/simulation/renderer;
- numerical robustness helpers.

It should not contain rendering or world-generation policy.

Phase 1 places the generic frame tree, runtime frame handles, checked rigid transforms, and frame-aware mathematical pose/conversion types here. They have no celestial-body or project semantics. Mathematical evaluation is reusable by simulation and rendering; it is not a GPU resource or renderer policy.

## 9.3 `mundaris_world`

Owns authoritative authored world state and semantic world definitions.

Expected future responsibilities:

- star systems and celestial-body definitions;
- planet configuration;
- seeds and generator configuration;
- terrain definition metadata;
- persistent terrain edits;
- biome/environment configuration;
- vegetation rules and overrides;
- water configuration;
- placed object state;
- project/world serialization model.

It does not own GPU resources.

Phase 2 world state owns celestial bodies and a separate disposable `CelestialFrameProjection` containing the tree and domain-to-frame associations. Authoritative bodies store no runtime frame handle. Math owns the tree algorithms and the low-level `SimulationInstant`, not a global world instance or playback policy. Runtime frame handles are not persistent body, terrain-region, or generated-object identities.

## 9.4 `mundaris_simulation`

Owns systems that advance authoritative state over time or derive simulation results.

Expected future responsibilities:

- orbital dynamics;
- rotational dynamics;
- simulation clocks/time scaling;
- future climate or long-timescale simulation orchestration;
- optional coarse tectonic evolution;
- future event-driven simulation systems.

It should operate on world/domain state without depending on rendering.

## 9.5 `mundaris_renderer`

Owns visual representation and GPU resources.

Expected future responsibilities:

- `wgpu` device/surface integration;
- frame graph or render-pass organization when needed;
- camera-relative transforms;
- terrain patch rendering;
- planet rendering;
- atmosphere/ocean rendering;
- vegetation rendering;
- GPU caches;
- shader management;
- visibility/culling;
- render LOD selection or rendering-side LOD requests;
- profiling/diagnostic GPU integration.

It must not become authoritative world storage.

The app passes read-only mathematical frame evaluation and explicit render requests to the renderer. Math performs high-precision frame conversion; renderer owns render-origin policy, precision/range checks, `f64` → `f32` conversion, projection, and GPU layouts. This preserves the renderer → math boundary without introducing renderer → world.

## 9.6 `mundaris_app`

Owns application orchestration and user-facing product behavior.

Expected responsibilities:

- window/event-loop integration;
- editor state;
- command routing;
- application lifecycle;
- loading/saving orchestration;
- connecting world, simulation, and rendering;
- UI composition;
- user input modes;
- project/session state.

The app crate may coordinate subsystems without absorbing their core logic.

Phase 1's small validation fixture and observer session are app-owned. This does not move future celestial state or simulation algorithms into app.

---

# 10. Authoritative State Model

A core Mundaris distinction is between **authoritative state** and **derived representation**.

## 10.1 Authoritative state

Examples:

- celestial body mass;
- body radius/base shape parameters;
- orbit state;
- orientation and angular velocity;
- terrain generator configuration;
- deterministic seed;
- terrain edit operations/data;
- biome parameters;
- vegetation generator version and configuration;
- deleted/generated-object overrides;
- explicitly placed user objects;
- water level/configuration;
- saved simulation time.

## 10.2 Derived state

Examples:

- current terrain mesh;
- vertex/index buffers;
- normal maps;
- generated biome lookup texture;
- tree instance buffers;
- forest impostors;
- collision mesh cache;
- visibility sets;
- LOD patch selection;
- shadow maps;
- atmosphere lookup tables;
- streamed tile cache;
- generated thumbnails.

Derived state must be safe to discard and rebuild.

## 10.3 Cached state may be persisted opportunistically

The engine may eventually save expensive derived caches for load-time performance.

Such caches must be:

- versioned;
- invalidatable;
- reconstructible;
- non-authoritative.

Corruption or deletion of a derived cache must not destroy the authored world.

---

# 11. Identity Model

Stable identity matters because generated content can be unloaded and regenerated.

Mundaris should eventually distinguish:

- authored entity identity;
- celestial-body identity;
- procedural region identity;
- deterministic generated-object identity;
- runtime/render-cache identity.

A generated tree may need a stable identity even when it does not currently exist as an allocated runtime object.

Conceptually:

```text
GeneratedObjectId = deterministic function(
    generator_version,
    body_id,
    region_coordinate,
    local_candidate_index,
    layer_seed
)
```

The exact encoding is open.

The invariant is that a sparse override such as "tree removed" can refer to a deterministic generated object without requiring every tree on the planet to be permanently serialized.

---

# 12. Units and Physical Quantities

Units must always be explicit in public APIs and persistent data.

Preferred SI basis:

- distance: metres;
- mass: kilograms;
- time: seconds;
- velocity: metres/second;
- acceleration: metres/second²;
- angle: radians internally;
- temperature: kelvin internally where physical calculations require it;
- pressure: pascals;
- luminosity/power: watts when modeled physically.

UI may expose friendlier units:

- kilometres;
- astronomical units;
- Earth masses;
- Earth radii;
- solar masses;
- hours/days/years;
- Celsius;
- atmospheres.

Conversion belongs at boundaries.

Avoid variables such as `distance`, `speed`, or `radius` when the unit is not obvious from type/context.

Prefer names such as:

```text
radius_m
semi_major_axis_m
rotation_period_s
surface_gravity_m_s2
```

until stronger quantity types provide genuine value.

---

# 13. Coordinate and Reference-Frame Architecture

This is the first major engine foundation that must be designed and validated before terrain work.

## 13.1 Problem

Mundaris must simultaneously support values such as:

```text
star ↔ planet distance: ~10^11 metres
planet radius:          ~10^6 metres
mountain detail:        ~10^3 metres
human movement:         ~10^-1 to 10^1 metres
surface detail:         potentially below 10^-2 metres visually
```

A single `f32` world coordinate system cannot represent these scales accurately.

Even `f64` alone does not solve every rendering issue because GPU pipelines and local geometry benefit from camera-relative `f32` values.

## 13.2 Reference-frame tree

Mundaris should model transforms hierarchically.

Example:

```text
SystemFrame (Phase 1 root)
├── StarBodyFrame
├── PlanetTranslationFrame
│   ├── PlanetBodyFixedFrame
│   │   └── Surface/RegionalFrame
│   │       └── Observer pose (not necessarily a tree node)
│   └── MoonTranslationFrame
│       └── MoonBodyFixedFrame
└── other independently moving frames
```

This diagram is conceptual; Phase 1 needs generic rigid frames, not these domain types or a mandatory universe frame.

Transform ancestry, domain ownership, and gravitational/orbital relationships are distinct. A child inherits **all** parent motion, including rotation. A moon must not accidentally inherit a planet's body spin simply because it orbits that planet. Future simulation can express moon motion in a non-body-fixed translating parent or directly in the system frame. Neither approach locks an integrator or physical ownership model.

## 13.3 Body-local content

Terrain and surface content should be represented relative to the body.

A tree does not have to know that the planet moved millions of metres through system space during a time step.

The body transform moves the body-local coordinate system.

Regional Cartesian frames may provide small local offsets without defining terrain topology or flattening a curved surface into authoritative planar data. Runtime observer/frame changes must not change deterministic region keys or sparse-edit identity.

## 13.4 Render-relative origin

Before GPU submission, visible positions should be converted to coordinates relative to an observer/render origin.

Conceptually:

```text
frame-local high-precision point + frame-local observer
        ↓
find lowest common ancestor; exclude shared ancestral motion
        ↓
evaluate relative displacement in f64 (subtract before rotating where possible)
        ↓
small observer-relative vector
        ↓
checked conversion to f32 under the selected representation's error budget
        ↓
GPU
```

Math owns frame conversion. Renderer owns projection and the GPU precision boundary. All inputs to one evaluation must describe the same instant; mixing simulation ticks or interpolated and non-interpolated transforms is invalid.

This keeps nearby GPU values numerically small. Distant objects still have large relative vectors and require distance-appropriate representations; camera-relative conversion does not by itself solve depth-buffer range or distant mesh precision. Those rendering techniques remain open.

## 13.5 Frame transitions

The observer may conceptually move from system-scale navigation into a planet-local or regional representation.

Such transitions must not produce visible teleportation or simulation discontinuity.

The engine can change which frame is used for numerical convenience while preserving the same physical pose at one evaluation instant. Future physics must also preserve physical velocity, including inherited frame motion. A coordinate change is distinct from deliberately attaching the observer to a moving surface or changing control mode.

## 13.6 Velocities

Position frame conversion alone is insufficient.

Velocity and angular motion must also have explicit semantics when switching frames.

For example, a surface observer inherits:

- orbital velocity of the planet;
- velocity caused by planetary rotation;
- local relative velocity.

A point includes an origin, a displacement includes a basis but no origin, and a direction is dimensionless. Velocity is the time derivative of a point relative to a specified frame, not merely a displacement rotated into another basis.

For child-to-parent pose `p_parent = t + R p_child`, the required future relation is:

```text
v_parent = t_dot + omega_parent × (R p_child) + R v_child
```

Here `t_dot` is the child origin's velocity relative to parent, expressed in parent axes, and `omega_parent` is child angular velocity relative to parent, expressed in parent axes. The Phase 1 specification defines the conversion contract without a physics engine; acceleration and non-inertial force models remain future work.

## 13.7 Orientation

Celestial body orientation should be represented independently of terrain content.

A point fixed to the surface remains body-local while the body orientation changes relative to the system frame.

Phase 1 uses checked `f64` unit quaternions and rigid transforms only. Scale, shear, reflections, and astronomical `f32` matrices do not belong in physical frame transforms; visual mesh scale is a rendering concern. Conventions and normalization are specified in the phase document.

## 13.8 Precision rule

Never convert a large astronomical absolute position to `f32` and then subtract the camera in `f32`.

Subtract in high precision first, while retaining the local representation:

```text
f64 frame-local relative evaluation → small f64 delta → checked f32 render vector
```

Subtracting independently flattened `f64` root/object and root/camera positions is insufficient for the strongest local guarantees: low bits may already have been rounded away. At `1.5 × 10^11 m`, adjacent `f64` values are about `3.05 × 10^-5 m` apart; at `10^16 m`, they are about `2 m` apart. Cancel shared ancestry before introducing those offsets and keep authoritative content frame-local.

Relative conversion cannot restore precision already lost in source coordinates or unrelated branches with large nearly equal offsets. Phase specifications must state tested scale/error budgets. Do not promise arbitrary universe-scale absolute precision or implement extended-number schemes before requirements justify them. Conventional global floating-origin rebasing is not required for Phase 1.

---

# 14. Time Architecture

Mundaris eventually needs multiple notions of time.

They must not be conflated.

## 14.1 Wall-clock/application time

Used for:

- UI animation;
- input timing;
- frame scheduling;
- performance diagnostics.

## 14.2 Render time

Frames occur according to presentation/display scheduling.

Rendering should not define physical simulation speed.

## 14.3 Simulation time

Authoritative world time that can potentially run at:

```text
0×
0.1×
1×
10×
1,000×
1,000,000×
```

Different simulation subsystems may require different numerical integration strategies at different rates.

## 14.4 Long-timescale model time

Processes such as erosion, tectonic approximation, climate equilibrium, or stellar evolution should not be naively stepped using the real-time orbital update loop.

Mundaris should allow specialized models operating on meaningful coarse intervals.

Example:

```text
realtime / seconds
    orbit, rotation, observer movement

hours / days
    optional weather approximation

years / centuries
    climate trend or orbital sampling

thousands / millions of years
    erosion / tectonic evolution / cratering models
```

The simulation architecture should support multiple model time scales rather than one universal fixed tick for everything.

---

# 15. Celestial Body Model

A celestial body is authoritative domain state, not a rendered sphere.

Expected future semantic data may include:

```text
CelestialBody
├── stable identity
├── body kind
├── mass
├── characteristic radius / shape model
├── translational state
├── rotational state
├── gravitational parameters
├── parent/system relationships
├── world-generation configuration
└── optional layers
    ├── terrain
    ├── atmosphere
    ├── hydrosphere
    ├── biome model
    └── vegetation model
```

Do not require every body to have every layer.

A star, gas giant, rocky moon, and terrestrial world need different surface capabilities.

Composition is preferred over an enormous object with meaningless fields.

---

# 16. Orbital and Gravity Simulation

The first orbital implementation can be simple.

The architecture should nevertheless permit progression such as:

```text
analytical/predefined orbit
        ↓
two-body integration
        ↓
N-body Newtonian gravity
        ↓
optional specialized corrections
```

The world representation should not encode "planet follows fixed ellipse forever" as an irreversible assumption.

## 16.1 Separation from rendering

Simulation produces body transforms/state.

Rendering consumes those transforms.

The renderer does not integrate gravity.

## 16.2 Integrator replaceability

The exact numerical integrator should be replaceable.

Potential future candidates can be evaluated for:

- energy conservation;
- stability under time acceleration;
- determinism;
- cost;
- suitability for interactive edits.

No integrator is locked by this document.

## 16.3 Scope

Mundaris does not initially need relativistic orbital physics.

Physical plausibility and stability matter more than maximal astrophysical completeness.

---

# 17. Planet Surface Representation

The surface is one of the most important engine abstractions.

It must support both global procedural evaluation and future local volumetric edits.

## 17.1 Global base surface

A planet can efficiently define a base surface as a function of direction or another planetary parameterization.

Conceptually:

```text
surface_radius(direction, scale/context) -> metres
```

or an equivalent field representation.

This is excellent for:

- continents;
- basins;
- mountain systems;
- regional terrain;
- global LOD;
- deterministic regeneration.

Any scale/context argument selects a filtered or refined evaluation of one body-fixed definition; observer distance and render LOD must not change authoritative terrain or edit meaning. Generation keys and edit coordinates remain independent of runtime frame handles and render origins. The exact evaluation/parameterization contract remains open until the terrain phase.

## 17.2 Why a pure height field is insufficient

A height-only representation cannot naturally encode:

- tunnels;
- caves;
- arches;
- undercuts;
- arbitrary excavation;
- terrain added above overhangs.

Therefore it cannot be the complete long-term authoritative terrain model.

## 17.3 Preferred hybrid direction

Most of the planet remains implicit/procedural.

Only areas requiring volumetric freedom gain local volumetric edit data.

Conceptually:

```text
procedural global base surface
          +
regional procedural refinements
          +
sparse volumetric edit overlay
          =
final terrain field
```

The exact representation of the volumetric overlay is open.

Candidates may include:

- sparse density grids;
- signed distance fields;
- constructive edit primitives;
- chunked voxel/density data;
- hybrid operation logs plus baked local chunks.

No choice should be made without prototype measurements.

---

# 18. Terrain Editing Model

Terrain editing must act on authoritative terrain state.

## 18.1 Required future operations

The architecture should eventually permit:

- remove material;
- add material;
- smooth;
- flatten;
- raise/lower surface;
- stamp crater/feature;
- sculpt;
- create tunnel/cave;
- undo/redo through editor commands where practical.

## 18.2 Sparse edit locality

Editing a 20-metre cave must not require rewriting a continental height map or entire planet asset.

Edits should invalidate only affected regions and dependencies.

## 18.3 Cache invalidation

A terrain edit may invalidate:

- visible terrain mesh patches;
- lower/higher LOD representations;
- collision data;
- vegetation placement inside affected area;
- biome samples if slope/elevation dependent;
- water/flow data if relevant;
- shadows or baked derived data.

Invalidation should be explicit and region-based.

## 18.4 Editing and procedural updates

If generator parameters change after edits exist, the engine needs a defined policy.

Potential future choices include:

- edits are expressed relative to the generated base and reapply;
- edits become invalid if topology/generator version changes materially;
- user can bake/rebase edits;
- migration tool attempts conversion.

This is an open persistence/editor design problem and must be addressed before commercial project files depend on it.

---

# 19. Planetary LOD Architecture

LOD is likely the most technically important rendering system in Mundaris.

Its responsibility is not merely to reduce polygon count. It must preserve **perceived continuity of scale**.

## 19.1 Desired behavior

The observer should be able to move continuously from astronomical distances to surface scale without obvious world replacement.

A distant feature should refine into more detail rather than becoming a different feature.

## 19.2 Preferred surface partition direction

The [Phase 4 design](MUNDARIS_PHASE_4_PLANET_SURFACE_REPRESENTATION_AND_LOD.md)
now selects normalized radial cube mapping and computed balanced quadtrees for
the first smooth-sphere implementation, after comparing alternate sphere charts.
Its stitched topology, screen-error/readiness/handoff and body-local contracts
are implementation choices with recorded numerical/native/performance evidence
and outstanding complete acceptance, not new locked world or persistent-edit
invariants. The discussion below retains
the broader replaceability rationale.

A cube-sphere with hierarchical patch subdivision is a strong candidate because it provides:

- six manageable root faces;
- relatively straightforward spatial hierarchy;
- local refinement;
- deterministic addressing;
- compatibility with procedural evaluation;
- no single polar singularity like ordinary latitude/longitude grids.

A quadtree-like hierarchy per face is also a strong candidate.

However:

> **Cube-sphere + quadtree is a preferred direction, not yet a locked invariant.**

It must be validated for:

- distortion;
- seam handling;
- morphing;
- culling;
- editable terrain integration;
- CPU cost;
- GPU generation;
- neighbor LOD constraints;
- very close surface views.

## 19.3 LOD selection

LOD should eventually consider projected screen error rather than simple distance thresholds alone.

Factors may include:

- patch geometric error;
- camera distance;
- field of view;
- viewport size;
- terrain displacement bounds;
- horizon visibility;
- performance budget.

## 19.4 Morphing

Terrain should not visibly pop between resolutions.

Potential strategies include:

- geomorphing;
- skirts;
- edge stitching;
- parent/child cross-fade;
- height morphing;
- meshlet/patch transition schemes.

The exact technique remains open.

## 19.5 LOD and editing

All LODs must derive from the same authoritative terrain state.

An edit is not stored separately per LOD.

## 19.6 Horizon and planetary curvature

Surface rendering must preserve correct curvature at appropriate scales.

A local flat approximation may be used for numerically convenient calculations/rendering, but it must remain a representation of the curved body.

## 19.7 “Infinite render distance” definition

Mundaris does not promise infinite detail.

It aims for:

> no obvious arbitrary cutoff for meaningful world-scale geometry; distant features transition into progressively cheaper representations until they become part of a planet-scale representation.

Atmospheric visibility, planetary curvature, occlusion, and perceptual contribution naturally limit what must be drawn.

---

# 20. Procedural Terrain Generation

Terrain generation should be multi-scale and deterministic.

## 20.1 Generation layers

A likely conceptual pipeline:

```text
planet base shape
    ↓
continental distribution
    ↓
ocean basins / macro elevation
    ↓
mountain-chain masks
    ↓
regional relief
    ↓
valleys / ridges
    ↓
local terrain
    ↓
surface micro-detail
```

Each layer should have clear frequency/amplitude responsibilities.

## 20.2 Mountains

Initial mountain generation may use techniques such as:

- fractal noise;
- ridged noise;
- domain warping;
- gradients/slope-derived erosion approximations;
- procedural ridge masks;
- drainage-aware carving;
- DLA-inspired structures;
- authored procedural graphs later.

The goal is believable large-scale morphology without requiring full physical erosion simulation.

## 20.3 Fake erosion as a valid production technique

Mundaris is allowed to use inexpensive procedures that approximate the visual result of natural processes.

The standard is not:

> did the engine literally simulate ten million years of rainfall?

The standard is:

> does the generated terrain exhibit coherent features at the intended scale, and can a more physical algorithm replace the approximation later?

## 20.4 Deterministic evaluation

Terrain generation must support evaluating requested regions independently.

It must not require generating the entire planet at highest resolution first.

## 20.5 Context problem

Some terrain processes require neighborhood/global context.

For example:

- drainage;
- erosion;
- tectonics;
- river networks.

Such systems may precompute lower-resolution global fields that local generation samples.

This supports local streaming without sacrificing planetary coherence.

---

# 21. Tectonics

Tectonics is a future enhancement, not an initial dependency.

The architecture should permit multiple tectonic approaches.

## 21.1 Procedural tectonic approximation

An early future version could generate:

- plate-like regions;
- convergent boundaries;
- divergent boundaries;
- transform boundaries;
- mountain-chain masks;
- rift regions;
- volcanic likelihood fields.

These fields then influence terrain generation.

No time evolution is required initially.

## 21.2 Simulated tectonic evolution

A much later version may simulate plate movement over geological time and derive terrain-driving fields.

This should still feed the terrain pipeline rather than requiring the renderer to know about tectonic algorithms.

## 21.3 Replaceability goal

A planet created with simple noise mountains should not require a different rendering architecture from a planet created by a tectonic generator.

Tectonics produces authoritative/generation inputs, not special rendering semantics.

---

# 22. Erosion and Hydrology

Erosion can exist at multiple fidelity levels.

## 22.1 Visual erosion approximation

Cheap deterministic procedures can create:

- channels;
- worn ridges;
- valley structure;
- drainage-like patterns.

## 22.2 Offline/coarse erosion preprocessing

Selected planetary fields can be generated at moderate resolution using more expensive erosion algorithms and then sampled/refined by local terrain generation.

## 22.3 Full local simulation

Real-time hydraulic simulation is not a core goal.

It may eventually be useful for specialized editing previews or experiments but should not define the architecture.

## 22.4 Hydrology graph

Large-scale water flow may eventually derive from a lower-resolution global elevation field:

```text
macro terrain
    ↓
flow direction
    ↓
flow accumulation
    ↓
river graph
    ↓
lakes / basins
    ↓
local terrain carving + water rendering
```

This allows rivers visible at multiple LODs to remain coherent.

---

# 23. Biome and Environment Model

Biomes should emerge primarily from continuous environmental fields, not only from hard painted IDs.

Potential fields:

- latitude;
- elevation;
- temperature;
- moisture;
- rainfall;
- slope;
- solar exposure;
- distance from ocean;
- prevailing climate zone;
- soil/geology later.

Then biome classification can be derived.

Example:

```text
temperature_k             = 285
relative_moisture_fraction = 0.78 (dimensionless, 0..1)
elevation_m               = 430
slope_rise_over_run        = 0.11 (dimensionless)
        ↓
wet temperate forest
```

Transitions should be blendable.

The engine should preserve the underlying fields even if the UI presents named biome categories.

This makes future vegetation and climate systems more flexible.

---

# 24. Vegetation Architecture

Vegetation must not require explicit storage of every generated plant.

## 24.1 Generation hierarchy

Vegetation should be derived from:

```text
planet seed
+ biome/environment fields
+ vegetation species rules
+ regional density fields
+ deterministic candidate placement
+ sparse overrides
```

## 24.2 Generated object identity

Each generated tree or plant that can be interacted with should have reproducible identity.

This allows:

```text
procedural tree exists
        ↓
user deletes tree
        ↓
store deletion override only
        ↓
region unloads
        ↓
region regenerates
        ↓
procedural tree candidate is suppressed by override
```

## 24.3 Species

Species should eventually define behavior/data such as:

- valid temperature range;
- moisture preference;
- altitude range;
- slope tolerance;
- density;
- clustering behavior;
- model/variant pool;
- size distribution;
- season/color behavior later.

## 24.4 Forest-scale representation

Far forests should not consist of millions of individually rendered tree meshes.

Possible hierarchy:

```text
full individual tree
→ simplified tree
→ impostor / billboard / low-poly proxy
→ tree cluster
→ canopy field / volumetric impression
→ contribution to distant terrain shading
```

The exact implementation is open.

## 24.5 Uniqueness

Vegetation should avoid obvious repeated identical instances.

Variation can come from deterministic:

- scale;
- rotation;
- model variants;
- branch/leaf variants later;
- color/material variance;
- local growth parameters.

The system should still batch/instance efficiently.

---

# 25. Rocks, Props, and Procedural Objects

Vegetation is one specialization of a broader generated-object pattern.

Rocks, debris, surface formations, and future procedural structures may use similar principles:

- deterministic placement;
- stable generated identity;
- independent layer;
- sparse overrides;
- LOD hierarchy;
- optional promotion into explicit authored objects when edited.

A generic generated-object framework may eventually emerge, but it should not be created prematurely until multiple real systems share enough behavior.

---

# 26. Water Architecture

Water fidelity should be layered.

## 26.1 Initial ocean model

A terrestrial planet can initially define sea level relative to the body.

A simple ocean may be rendered as a sphere or body-conforming surface intersecting terrain.

## 26.2 Visual water

Later visual features may include:

- reflection;
- refraction;
- Fresnel response;
- depth coloration;
- waves;
- foam;
- shoreline treatment;
- underwater fog/absorption.

These do not require physical fluid simulation.

## 26.3 Rivers and lakes

Rivers/lakes can be data-driven surfaces derived from hydrology.

They may animate flow visually without simulating fluid particles.

## 26.4 Terrain edits and water

When terrain edits intersect water systems, the initial behavior may be limited.

Full dynamic hydrological recomputation after arbitrary excavation is a later capability.

The architecture should permit future local recomputation without requiring it in early versions.

## 26.5 Physical ocean simulation

Large-scale fluid simulation is explicitly not required for the primary product vision.

It may remain experimental indefinitely.

---

# 27. Atmosphere Architecture

Atmosphere rendering is visually important because it establishes planetary scale.

The atmosphere is also a physical/configuration layer that can eventually influence climate.

## 27.1 Separation

Keep distinct concepts for:

- atmospheric composition/physical parameters;
- climate/environment simulation;
- atmosphere rendering.

A renderer may use an approximation that does not exactly match a future climate model, as long as the boundary is explicit.

## 27.2 Visual goals

Eventually support:

- limb scattering from orbit;
- blue/red sky behavior depending on atmosphere;
- aerial perspective;
- horizon haze;
- sunset/sunrise transitions;
- altitude transitions from ground to space;
- planetary shadowing.

## 27.3 Technique

Precomputed scattering LUTs, analytical models, or other approaches may be evaluated later.

No algorithm is selected by this document.

---

# 28. Clouds and Weather

Clouds are future visual/simulation layers.

Potential progression:

```text
static/procedural cloud texture
    ↓
animated global cloud field
    ↓
volumetric clouds
    ↓
weather-driven cloud systems
```

Weather does not need to become a full computational fluid dynamics simulation.

A lower-dimensional/global model can drive plausible visual patterns.

The world format should not require clouds to be permanently baked into planet textures.

---

# 29. Lighting and Stars

Initial lighting can be simple while preserving future physical coherence.

## 29.1 Star as light source

A star can initially provide a dominant directional/point-like light relationship to a planet.

Later the renderer may account for:

- inverse-square irradiance;
- star radius;
- spectral color;
- eclipses;
- multiple stars;
- physically meaningful exposure.

## 29.2 Shadows

Planetary-scale shadowing may need different techniques from local terrain shadows.

For example:

- celestial eclipse shadows;
- terrain/local cascaded shadows;
- cloud shadows.

These should be composable rather than forced into one universal shadow representation.

---

# 30. Camera and Observer Model

The camera is not merely a matrix.

It represents an observer embedded in the reference-frame system.

Potential observer modes:

- free astronomical camera;
- orbit camera;
- surface free-flight camera;
- first-person/walking camera;
- editor focus/orbit camera.

## 30.1 One physical pose, multiple controls

Camera control mode should be separable from the underlying spatial pose/reference frame.

Switching from free flight to walking should not require teleporting to a separate scene.

## 30.2 Surface gravity

Walking mode should eventually orient "down" toward the local gravity/surface direction rather than a global Y axis.

On a simple spherical planet:

```text
down ≈ toward body center
```

More advanced gravity models can refine this later.

## 30.3 Speed scaling

Astronomical navigation needs speed controls spanning huge orders of magnitude.

Movement semantics should avoid accumulating precision errors when speed changes dramatically.

---

# 31. Rendering Architecture

The renderer should be designed around multiple representations of the same authoritative world.

## 31.1 Render pipeline principle

Conceptually:

```text
world/simulation state
       ↓
visibility + representation requests
       ↓
cache/stream/generate required data
       ↓
GPU representation
       ↓
render passes
```

## 31.2 Renderer is not world owner

GPU resources may reference stable world IDs, but the renderer cannot become the only location where world information exists.

## 31.3 Representation selection

Different distances may use different render representations for:

- planets;
- terrain;
- forests;
- water;
- atmosphere;
- clouds;
- stars.

These representations should overlap enough to transition without obvious popping.

## 31.4 Culling

Potential culling stages:

- body-level frustum culling;
- planet horizon/occlusion culling;
- terrain patch frustum culling;
- back-facing planetary patch culling;
- vegetation cluster culling;
- object-level GPU culling later.

## 31.5 Render graph

Do not introduce a complex render graph until the actual number of passes/resources justifies it.

When introduced, it should make dependencies/lifetimes explicit and aid diagnostics rather than exist as architectural decoration.

---

# 32. GPU Compute Philosophy

GPU compute is expected to be valuable for Mundaris, but CPU/GPU responsibility should follow data movement and workload evidence.

Potential GPU-friendly tasks:

- procedural terrain evaluation;
- normal generation;
- vegetation candidate generation/culling;
- terrain patch generation;
- atmosphere LUT creation;
- visibility culling;
- indirect drawing;
- noise field generation.

Potential CPU responsibilities:

- authoritative world state;
- project editing;
- simulation orchestration;
- sparse edit storage;
- high-precision frame transforms;
- global generation metadata;
- persistence.

Do not move a system to GPU merely because it sounds faster.

Measure:

- dispatch overhead;
- readback requirements;
- cache behavior;
- bandwidth;
- synchronization;
- portability.

---

# 33. Streaming Architecture

Mundaris will eventually stream/generate representations dynamically.

Streaming means more than disk IO.

It includes:

- procedural region generation;
- terrain LOD creation/destruction;
- vegetation instance generation;
- GPU uploads;
- derived cache eviction;
- optional disk cache loading.

## 33.1 Priority

Work should be prioritized according to visible need.

Possible priority factors:

- screen-space error;
- time until region enters view;
- camera velocity/direction;
- editor focus;
- current interaction target;
- dependency readiness.

## 33.2 Cancellation

Generation jobs should eventually be cancellable or safely discardable when the observer moves away before completion.

This is important for high-speed travel.

## 33.3 Budgeting

Streaming must use explicit frame/time/memory budgets rather than greedily generating everything requested immediately.

---

# 34. Concurrency and Job Scheduling

Mundaris will eventually need parallel background work.

Do not implement a custom job system before real workloads exist.

Likely workloads include:

- terrain generation;
- mesh generation;
- hydrology preprocessing;
- vegetation generation;
- serialization/compression;
- derived cache building.

Requirements when a scheduler is introduced:

- priorities;
- cancellation;
- dependency handling where necessary;
- bounded queues;
- clean shutdown;
- profiling;
- avoidance of unbounded task spawning;
- explicit ownership of GPU submission vs CPU work.

A standard runtime/thread pool should be preferred initially unless measurements require custom behavior.

---

# 35. Memory Architecture

The engine's apparent world size must be decoupled from resident memory size.

## 35.1 Memory categories

Track separately where practical:

- authoritative project state;
- simulation state;
- CPU procedural caches;
- CPU geometry staging;
- GPU terrain resources;
- GPU vegetation resources;
- textures;
- editor resources;
- disk cache.

## 35.2 Bounded caches

Large derived caches should eventually have explicit budgets and eviction policies.

## 35.3 Avoid per-object overhead at planetary scale

Millions of theoretical generated trees must not imply millions of heap-allocated Rust objects.

Use region/cluster/data-oriented representations on hot paths.

## 35.4 Allocation awareness

Frame-critical loops should avoid unpredictable per-frame allocation.

This is a performance goal, not a ban on allocation everywhere.

---

# 36. Data-Oriented Design

Use data-oriented layouts when profiling or scale demonstrates value.

Likely candidates:

- vegetation instances;
- terrain patch metadata;
- culling data;
- simulation body arrays;
- GPU upload batches.

Do not force every high-level domain object into a structure-of-arrays layout prematurely.

Semantic clarity in world/editor code remains important.

---

# 37. Persistence and Project Files

Because Mundaris may become a commercial world builder, project-file stability matters.

## 37.1 Project file contains authoritative intent

Persistent projects should contain enough to reproduce the world without derived render caches.

Likely categories:

```text
project metadata
engine/project format version
world/system definition
body definitions
seeds
generator configuration
simulation state
sparse terrain edits
vegetation/object overrides
authored objects
editor metadata where useful
```

## 37.2 Versioning

The project format needs explicit versioning from the first real saved-world implementation.

## 37.3 Migrations

Once users can build valuable worlds, incompatible changes require migration tooling or clear compatibility behavior.

## 37.4 Generator versioning

Determinism requires procedural algorithms to be version-aware.

Changing a noise implementation can otherwise silently change an entire saved planet.

A future project may therefore record generator versions independently from overall app version.

## 37.5 Derived cache directory

Expensive generated caches may be stored separately from authoritative project data and deleted safely.

---

# 38. Undo and Editor Command Architecture

World-building software benefits from command-based editing.

Potential future user actions:

- change planet parameter;
- move celestial body;
- sculpt terrain;
- remove vegetation;
- place object;
- change biome/generator setting.

These should eventually support undo/redo where feasible.

Not every simulation action must be reversible indefinitely.

The editor should distinguish:

- reversible authoring commands;
- simulation advancement;
- destructive/baked operations.

The exact command framework should not be implemented until first real editing features exist.

---

# 39. Editor Architecture

Mundaris is editor-first.

The UI is not a debug overlay that later becomes the product.

Eventually, editor responsibilities may include:

- project management;
- hierarchy/system browser;
- property inspectors;
- planet generation controls;
- terrain tools;
- biome tools;
- simulation controls;
- timeline/time-scale controls;
- viewport modes;
- layer visibility;
- diagnostics/performance overlays;
- export tools.

The initial `egui` usage can remain simple while the engine stabilizes.

A future custom UI layer is not required unless `egui` becomes a proven product limitation.

---

# 40. Parameter Editing and Regeneration

A major Mundaris use case is adjusting sliders/parameters and seeing the world update.

The engine needs dependency-aware invalidation.

Example:

```text
mountain amplitude changed
        ↓
terrain generator state version increments
        ↓
affected terrain representations invalidated
        ↓
derived slope/biome fields may invalidate
        ↓
vegetation placement may invalidate
        ↓
render caches regenerate by priority
```

Not every parameter change should trigger a blind full-world rebuild.

The dependency system can begin coarse and become more granular later.

---

# 41. Layer Dependency Direction

A reasonable conceptual dependency flow is:

```text
celestial/system parameters
        ↓
planet physical parameters
        ↓
macro terrain/geology
        ↓
terrain
        ↓
environment/climate fields
        ↓
biomes
        ↓
vegetation / surface objects
```

Water/hydrology interacts with terrain and environment.

Rendering consumes all relevant layers but does not define them.

When cycles appear, they should be modeled as explicit simulation iterations rather than hidden module dependencies.

Example:

```text
terrain influences rainfall
rainfall influences erosion
erosion changes terrain
```

This is a model feedback loop, not justification for mutually dependent software modules.

---

# 42. Determinism

Determinism is central to sparse procedural worlds.

## 42.1 Deterministic random streams

Procedural systems should derive local random streams from stable seeds/keys.

Example conceptual key:

```text
hash(
  world_seed,
  body_id,
  generator_version,
  layer_id,
  region_key,
  candidate_index
)
```

## 42.2 Parallel generation

Results should not change simply because worker scheduling order changes.

Avoid generation logic whose result depends on thread completion order unless explicitly designed to do so.

## 42.3 Cross-platform determinism

Bit-identical floating-point procedural output across all hardware may be expensive or unrealistic for every system.

The exact required determinism level must be decided per subsystem.

For persistent generated identities and terrain, avoid fragile dependence on unspecified floating-point behavior where feasible.

Phase 1 frame math requires repeatable evaluation for identical inputs on the same build/target and numerical agreement within explicit tolerances on Windows/Linux. It does not promise cross-platform bit identity or define procedural-generation determinism. Runtime frame allocation and observer coordinates must never become procedural identity inputs.

---

# 43. Performance Philosophy

Mundaris must be architected for performance but optimized using measurements.

## 43.1 Scale through representation, not brute force

The central performance strategy is:

> do not represent information at a higher fidelity than its current contribution requires.

## 43.2 Theoretical world size is cheap

A body being 150 million kilometres away is not expensive.

Detailed local geometry is expensive.

Performance focus should therefore remain on:

- visible terrain complexity;
- generation throughput;
- GPU bandwidth;
- vegetation density;
- atmosphere/cloud cost;
- cache churn;
- synchronization;
- draw/dispatch count.

## 43.3 Preserve budgets

Eventually define budgets for:

- CPU frame time;
- GPU frame time;
- background generation time;
- GPU memory;
- CPU cache memory;
- upload bandwidth;
- patch counts;
- vegetation instances.

## 43.4 Graceful degradation

When budgets are exceeded, reduce representation detail or delay noncritical generation instead of stalling unpredictably.

---

# 44. Target Performance Philosophy

Do not hard-code final commercial requirements yet.

Development should nevertheless aim toward sensible consumer-hardware performance.

Useful internal targets for experiments may include:

- smooth interactive editor performance at 60 FPS on representative mid/high-range desktop hardware;
- ability to benefit from 120+ Hz displays where workloads allow;
- no simulation dependence on render FPS;
- predictable memory budgets;
- fast response to camera travel and editing.

These are goals, not contractual minimum specifications.

Every major renderer feature should eventually be benchmarked on more than one GPU class.

---

# 45. Profiling and Instrumentation

Performance-sensitive architecture without observability becomes guesswork.

Mundaris should eventually expose metrics such as:

- CPU frame duration;
- GPU frame duration;
- terrain patches by LOD;
- patch generation queue length;
- cache hit/miss counts;
- vegetation instance counts;
- draw calls / indirect draws;
- GPU memory estimates;
- CPU cache memory;
- streaming jobs in flight;
- simulation step cost;
- editor invalidation counts.

Use `tracing`-style spans/events for subsystem diagnostics where practical.

GPU timestamp queries can be added when rendering complexity justifies them.

---

# 46. Error Handling

Failures should be categorized.

## 46.1 Recoverable runtime failures

Examples:

- surface lost/outdated;
- optional cache unavailable;
- project thumbnail generation failure;
- recoverable asset load failure.

These should produce diagnostics and recover when possible.

## 46.2 Project/data corruption

Loading must fail clearly rather than silently inventing replacement authoritative data.

## 46.3 Programmer invariants

Assertions are appropriate for impossible internal states during development.

Avoid panics for ordinary runtime/environment conditions.

---

# 47. Unsafe Code

The current workspace forbids project-owned unsafe code through inherited lints and crate attributes.

Any future policy exception requires architectural review and should only be introduced if:

- a concrete feature requires it;
- the benefit is significant;
- a safe alternative is inadequate;
- the unsafe surface is very small;
- invariants are documented;
- dedicated tests/validation exist.

Graphics dependencies may internally use unsafe code; that does not require Mundaris itself to do so. Phase 1 preserves the existing prohibition.

---

# 48. Testing Strategy

Testing should focus on stable semantics and numerical correctness, not meaningless coverage numbers.

## 48.1 Unit tests

Good candidates:

- coordinate conversions;
- reference-frame composition;
- time-step logic;
- deterministic key/seed derivation;
- region addressing;
- terrain edit combination;
- persistence migrations.

## 48.2 Property tests

Potential future properties:

- frame A → B → A approximately preserves position;
- LOD region addressing covers expected domain without overlap/gaps;
- deterministic generator produces same output for same key;
- serialization round trip preserves authoritative state.

## 48.3 Numerical tests

Use explicit tolerances.

Do not assert floating-point equality when not semantically justified.

## 48.4 Renderer tests

Prefer:

- deterministic CPU-side preparation tests;
- shader compilation/validation;
- resource-state tests;
- selected image/reference tests later if stable enough.

Avoid a brittle giant screenshot suite early.

## 48.5 Performance tests

Benchmarks should be added once there is a stable hot path worth protecting.

---

# 49. Validation Scenes

As systems are implemented, maintain small purpose-built validation scenes rather than testing everything only in the full editor.

Potential future validation scenarios:

```text
reference frame precision test
orbit stability test
planet horizon test
LOD seam test
LOD morph test
terrain edit test
high-speed approach test
forest transition test
atmosphere ground-to-space test
hydrology coherence test
```

These can be developer modes rather than separate applications if simpler.

---

# 50. Shader Architecture

Shaders should be organized by purpose with explicit CPU/GPU data contracts.

Avoid enormous universal shaders controlled by hundreds of permutations unless measurements justify them.

Shader inputs should use clearly documented spaces:

- object/local;
- body-local;
- render-relative;
- view;
- clip.

Do not pass astronomical absolute positions as raw `f32` shader coordinates.

Generated shader code or shared schema systems may be introduced later if manual layouts become error-prone.

---

# 51. Materials and Surface Appearance

Terrain appearance should eventually derive from fields rather than one texture stretched across a planet.

Potential inputs:

- biome;
- altitude;
- slope;
- rock/geology type;
- moisture;
- snow coverage;
- latitude;
- local detail noise.

Large-scale albedo should remain coherent from orbit while near-field materials add detail.

Techniques may include:

- procedural material blending;
- triplanar mapping;
- virtual texturing;
- detail textures;
- macro/micro texture layers.

No final material architecture is selected yet.

---

# 52. Collision and Surface Queries

Rendering terrain and querying terrain are related but distinct.

Future systems may need:

- raycast against terrain;
- altitude above surface;
- surface normal;
- collision for walking;
- editor brush intersection;
- object placement queries.

Do not require full visual meshes to answer all queries.

Where possible, use authoritative terrain functions/data or dedicated collision representations.

---

# 53. Physics Scope

Mundaris needs some physics-like behavior but should not accidentally become a general-purpose physics engine project.

Likely needs:

- body gravity queries;
- observer movement/collision;
- future placed object collision;
- celestial-body dynamics.

A third-party physics library may eventually handle local rigid-body interactions.

Astronomical orbital integration may remain custom due to scale/reference-frame requirements.

Keep these concerns separable.

---

# 54. Gravity Field Architecture

Initial body gravity can be simple:

```text
a = -μ * r / |r|^3
```

for spherical point-mass approximation outside the body.

Later possibilities:

- multiple-body contributions;
- non-spherical harmonics;
- local anomalies;
- rotating-frame effects.

Surface terrain does not need to influence gravity initially.

The gravity query API should therefore permit different field implementations without requiring terrain/render changes.

---

# 55. Content Scale Hierarchy

A useful mental model for nearly every visual system is:

```text
astronomical
    ↓
planetary
    ↓
continental
    ↓
regional
    ↓
local
    ↓
human
    ↓
surface detail
```

Every feature should identify which scales it meaningfully contributes to.

Examples:

### Mountain range

```text
planetary: subtle albedo/shape contribution
continental: clear chain silhouette
regional: valleys/ridges
local: cliffs/slopes
human: rocks/surface material
```

### Forest

```text
planetary: biome/albedo tint
continental: broad canopy regions
regional: canopy structure
local: tree clusters
human: individual trees/plants
```

Designing representations per scale is preferable to forcing one representation across all scales.

---

# 56. High-Speed Travel

Seamless traversal may involve extreme speed changes.

The engine must handle scenarios such as:

```text
observer moves hundreds of km/s toward planet
        ↓
LOD requests change rapidly
        ↓
old generation work becomes irrelevant
        ↓
critical near-future patches receive priority
```

Important future techniques may include:

- predictive streaming;
- cancellation;
- lower-fidelity emergency fallback;
- hysteresis in LOD selection;
- bounded generation work per frame.

The renderer should never require every intermediate LOD to finish before a camera can continue moving.

---

# 57. Occlusion and Planet Horizon

A spherical/planetary world provides powerful visibility constraints.

A surface observer cannot see terrain through the planet.

Planetary horizon tests can eliminate enormous amounts of work.

LOD/visibility systems should eventually use:

- body horizon geometry;
- frustum culling;
- back-facing patch rejection;
- atmospheric visibility distance where relevant.

This is one reason “infinite render distance” can remain practical: much of the planet is geometrically invisible from any one surface location.

---

# 58. Asset Philosophy

Mundaris will eventually use authored assets for:

- trees;
- plants;
- rocks;
- materials;
- editor icons;
- possible structures.

Asset systems should support:

- stable asset identity;
- versioning/import metadata;
- hot reload during development where useful;
- platform-independent source projects;
- GPU-ready cached/transcoded outputs later.

Do not build a large asset database before a real asset type requires it.

---

# 59. Commercial Product Considerations

Mundaris may eventually be sold as world-building software.

Architectural implications:

- user project data must be treated as valuable;
- project saves need robust versioning;
- crashes should minimize risk of project loss;
- autosave/recovery will eventually matter;
- deterministic procedural changes must be migration-aware;
- performance settings should degrade gracefully;
- diagnostics should be exportable without exposing user secrets;
- licensing architecture, telemetry, storefront integration, and DRM are outside the engine core and should not contaminate world/simulation layers.

No open-source licensing is assumed.

---

# 60. Security and Untrusted Data

Once Mundaris loads user-created projects or imported assets, parsers should treat files as untrusted input.

Requirements eventually include:

- bounds checking;
- decompression limits;
- no arbitrary code execution from project files;
- safe path handling;
- clear plugin/scripting isolation if those features ever exist.

This is not urgent for early internal prototypes but should be remembered before public distribution.

---

# 61. Replaceability Boundaries

Subsystems expected to be replaceable behind stable roles include:

- orbit integrator;
- terrain macro generator;
- local noise/detail generator;
- erosion algorithm;
- tectonic model;
- biome classifier;
- vegetation placement algorithm;
- atmosphere renderer;
- ocean renderer;
- LOD selection heuristic;
- local terrain mesher;
- cache eviction strategy;
- background task scheduler.

Replacement should not require rewriting project identity, body ownership, or authoritative-vs-derived semantics.

---

# 62. Things We Intentionally Do Not Decide Yet

The following should remain open until prototypes provide evidence:

- exact cube-sphere mapping method;
- exact planetary patch topology;
- quadtree vs clipmap combinations;
- GPU tessellation/mesh shader use;
- terrain mesh generation location (CPU vs compute) for each LOD;
- marching cubes vs dual contouring vs another local volumetric mesher;
- precise sparse edit encoding;
- virtual texturing architecture;
- exact tree impostor technique;
- exact cloud technique;
- exact atmosphere implementation;
- exact N-body integrator;
- local rigid-body physics library;
- custom job system;
- ECS adoption;
- scripting/plugin architecture;
- mod support;
- export formats;
- city/civilization simulation;
- biological evolution;
- full fluid simulation.

These are not omissions. They are deliberate deferrals.

---

# 63. Architectural Anti-Patterns

Avoid the following.

## 63.1 One global `Vec3<f32>` world

This makes astronomical-to-human scale precision impossible.

## 63.2 Mesh-as-world-state

Editing a render mesh must not become the only record of terrain state.

## 63.3 One object per theoretical procedural object

Do not allocate billions of tree entities because billions of trees could theoretically exist.

## 63.4 Renderer-driven domain state

The renderer should not decide what the planet fundamentally is.

## 63.5 Hidden units

Do not let "1000" ambiguously mean metres, kilometres, seconds, or simulation ticks.

## 63.6 Global RNG

Procedural generation must not depend on mutable global random order.

## 63.7 Full regeneration on every parameter edit

Invalidate only what logically depends on the changed parameter when practical.

## 63.8 Premature universal abstraction

Do not build a generic "everything node graph ECS asset component framework" before concrete needs exist.

## 63.9 Premature microservices-style crates

Crates are compile/ownership boundaries, not folders with status.

## 63.10 Simulation tied to render FPS

Never make orbital stability depend on monitor refresh rate.

## 63.11 Distance-only LOD forever

Simple thresholds are fine for prototypes, but final terrain quality should consider projected error/perceptual contribution.

## 63.12 Non-versioned procedural changes

Changing generator behavior without tracking compatibility can corrupt the visual identity of saved projects.

---

# 64. Architectural Decision Records

Significant decisions should receive ADRs once they become concrete.

Existing and next ADR responsibilities:

```text
0002 reference frame and precision model (implemented)
0003 celestial domain, time and frame projection (implemented)
0004 gravity, integration and playback (implemented, platform acceptance open)
0005 celestial navigation, system overview and exact time warp (implemented,
     complete operator/platform acceptance open)
0006 planetary surface topology, LOD, cracks and handoff (implemented,
     complete operator/platform and profiling acceptance open)
later terrain/edit, generation, persistence,
       vegetation identity and CPU/GPU decisions receive numbers when concrete
```

An ADR should describe:

- context;
- constraints;
- considered options;
- decision;
- consequences;
- validation evidence;
- what would justify revisiting it.

---

# 65. Development Method

Major engine systems should follow this pattern:

```text
1. state the user-visible capability
2. identify architectural constraints
3. write a narrow design/specification
4. build the smallest validating prototype
5. instrument it
6. test edge cases
7. review the architecture
8. commit a coherent milestone
9. only then expand scope
```

Do not implement five speculative subsystems at once.

The engine should always have a runnable, inspectable state.

---

# 66. Milestone Philosophy

Each milestone should prove one important capability.

Good milestone:

> A camera can remain numerically stable from interplanetary distance to one metre above a moving/rotating sphere.

Bad milestone:

> Implement the space engine.

Good milestone:

> A cube-sphere prototype refines patches near the camera and preserves seams under continuous approach.

Bad milestone:

> Implement terrain, water, vegetation, biomes, and atmosphere.

---

# 67. Proposed Development Roadmap

This roadmap describes dependencies, not promises or dates. It supersedes the bootstrap's exploratory static-planet-first ordering; motion-compatible coordinates must be validated before terrain.

## Phase 0 — Foundation

**Status:** bootstrap code and hardening are present; platform/runtime validation is recorded separately from design intent.

Capabilities:

- Rust workspace;
- native app;
- `winit` event loop;
- `wgpu` initialization;
- `egui` integration;
- CI/lint/test baseline;
- project architectural documentation.

No engine features.

## Phase 1 — Coordinates, reference frames, and precision

Goal:

> prove that Mundaris can represent and render astronomical and human scale coherently.

Design/implementation topics:

- explicit units;
- high-precision transforms;
- frame identities;
- parent/frame composition;
- body-local coordinates;
- render-relative conversion;
- camera pose;
- velocity semantics;
- frame transition tests;
- large-distance precision validation.

Validation scene:

```text
abstract system origin
moving frame far from origin
rotating child and local debug primitives
observer approaches the same frame continuously
observer reaches metre-scale offset
nearby test geometry remains stable
```

Use generic frames and debug primitives only; no celestial domain model, orbital mechanics, planet surface, or terrain. The detailed contract is [Phase 1](MUNDARIS_PHASE_1_REFERENCE_FRAMES.md).

## Phase 2 — Celestial bodies and simulation clock

Goal:

> represent stars/planets/moons as authoritative moving bodies.

Topics:

- body identity/configuration;
- mass/radius;
- translation/orientation;
- simulation time;
- independent prescribed analytic motion for validation, not orbital mechanics;
- rotation;
- time acceleration;
- renderer consumes body transforms.

Validation:

- one star;
- one planet;
- one moon;
- stable relative views under accelerated time.

Implementation exists; see [Phase 2 specification](MUNDARIS_PHASE_2_CELESTIAL_MODEL_AND_TIME.md) and its validation record. Outstanding platform/visual acceptance is not implied by this roadmap.

## Phase 3 — Gravity, orbits and basic celestial rendering

Goal:

> evolve a small connected celestial system under Newtonian gravity and make its actual motion visually and numerically inspectable.

Topics:

- point-mass N-body gravity and a justified fixed-step orbital integrator;
- deterministic authoritative full-state publication;
- separate wall/render/requested/authoritative time and explicit acceleration/backlog policy;
- honest bounded reverse/seek/reset/edit semantics and numerical diagnostics;
- physically sized debug spheres, navigation markers, labels and selection;
- connected focus/orbit/zoom/overview using reference frames;
- historical trails from actual committed simulation states;
- orbital tests, validation application and measured benchmarks.

The [Phase 3 specification](MUNDARIS_PHASE_3_GRAVITY_ORBITS_AND_CELESTIAL_RENDERING.md) resolves gravity/integrator/time/history and debug rendering choices. Implementation and Windows numerical/native evidence are recorded in [Phase 3 validation](docs/phase-3-validation.md) and [ADR 0004](docs/adr/0004-gravity-integration-and-playback.md). Linux/current CI acceptance remains open. No terrain, planetary LOD, atmosphere, collision system or general lighting engine is included.

## Phase 3.5 — Celestial navigation, system overview and exact time warp

Goal:

> make the same connected physical system immediately understandable and useful to explore.

The [Phase 3.5 contract](MUNDARIS_PHASE_3_5_CELESTIAL_NAVIGATION_SYSTEM_VIEW_AND_TIMEWARP.md)
adds current geometric framing, BodyId selection through sphere/marker/label/list,
deterministic readable labels, one-observer smooth focus/orbit/clearance zoom/free
flight, actual-history presentation and separate osculating two-body guides.
Derived guide references/subsystem membership never become N-body ownership or
frame/spin ancestry. Exact pumping is responsive and bounded; requested/achieved
rate and clock discontinuities are explicit. All rates retain Phase 3 KDK and h.

Implementation/measurements and open operator/platform acceptance are recorded in
[Phase 3.5 validation](docs/phase-3-5-validation.md) and
[ADR 0005](docs/adr/0005-celestial-navigation-system-view-and-timewarp.md).
Approximate authority, larger automatic timesteps, adaptive/multi-rate mappings
and preview remain future orbital-performance research. This phase introduces no
terrain, LOD, spacecraft mechanics or Phase 4 implementation.

## Phase 4 — Planet surface representation and LOD

Goal:

> approach the same physically moving/rotating smooth body continuously from system scale to metre-scale clearance, validating topology and LOD before terrain generation.

The [Phase 4 implementation-ready design](MUNDARIS_PHASE_4_PLANET_SURFACE_REPRESENTATION_AND_LOD.md)
selects:

- normalized radial cube-sphere roots;
- hierarchical patch addressing;
- patch refinement;
- neighbor constraints;
- debug coloring;
- horizon/frustum culling;
- error metric;
- seam validation;
- approach from orbit to surface.

It additionally defines shared topology/batched preparation, stable surface
locations distinct from render patches, readiness-based far-sphere handoff and
explicit co-rotating surface inspection. Default geometry remains a perfect sphere
at authoritative reference radius; procedural displacement begins in Phase 5.

Implementation and directed Windows evidence are recorded in
[Phase 4 validation](docs/phase-4-validation.md) and
[ADR 0006](docs/adr/0006-planet-surface-topology-and-lod.md). Complete operator/Linux/
remote-CI and profiling acceptance remain open. Full-planet CPU preparation misses
the review target; measured revisiting must preserve topology, ownership and precision.
Phase 5 has not begun.

## Phase 5 — Procedural base terrain

Goal:

> replace perfect sphere patches with deterministic multi-scale displacement.

Topics:

- deterministic seeds;
- scale-separated noise;
- macro continents;
- mountain masks;
- local detail;
- patch-independent evaluation;
- normals;
- stable LOD transitions.

No vegetation or terrain editing yet.

## Phase 6 — Terrain representation and edit prototype

Goal:

> prove the authoritative procedural-base + sparse-local-edit model.

Topics:

- edit region addressing;
- local density/volume experiment;
- excavation;
- mesh regeneration;
- invalidation across LODs;
- save/reload sparse edits.

This phase should select the local edit representation through an ADR.

## Phase 7 — Atmosphere and ocean baseline

Goal:

> establish convincing planetary scale and ground-to-space continuity.

Topics:

- atmosphere scattering prototype;
- simple sea level;
- ocean rendering;
- ground-to-orbit transitions.

## Phase 8 — Environment and biomes

Goal:

> produce coherent environmental fields across the planet.

Topics:

- temperature approximation;
- moisture/rainfall approximation;
- elevation/slope;
- biome classification/blending;
- debug visualization.

## Phase 9 — Vegetation hierarchy

Goal:

> generate deterministic biome-driven vegetation with multiple representations across distance.

Topics:

- species rules;
- deterministic placement;
- generated identity;
- sparse delete/place overrides;
- individual trees;
- clustered distant representation;
- forest transition testing.

## Phase 10 — World-builder editing workflow

Goal:

> turn engine systems into useful authoring tools.

Topics:

- parameter panels;
- terrain brushes;
- layer visualization;
- undo/redo;
- save/project workflows;
- project recovery.

## Phase 11+ — Deepening simulation

Possible independent tracks:

- hydrology;
- rivers/lakes;
- procedural erosion;
- tectonic generation;
- climate improvements;
- clouds/weather;
- advanced terrain materials;
- local object physics;
- world export;
- higher-quality vegetation;
- additional celestial-body types;
- eclipses;
- multi-star systems;
- geological time evolution.

These should be selected based on product value and engine readiness.

---

# 68. Phase 1 Architectural Questions

Before implementing Phase 1, a dedicated specification must resolve at least:

1. What is the minimal reference-frame type model?
2. Are transforms represented as translation + quaternion directly, or another structure?
3. Which values are `f64`?
4. Which values are ever allowed to be `f32` before renderer submission?
5. How are frame IDs represented?
6. Does the frame hierarchy live in `mundaris_math`, `mundaris_world`, or get split between mathematical transform primitives and world ownership?
7. How are cycles prevented in a frame graph?
8. What is the semantic difference between a body frame and a general local frame?
9. How are velocities transformed between rotating frames?
10. How is camera pose represented?
11. How is a render origin selected?
12. Is origin rebasing explicit state, or simply a consequence of camera-relative conversion?
13. How do we test metre-scale stability at astronomical offsets?
14. How do we expose debug visualization of current frames/origins?
15. What numerical tolerances are acceptable?

These are resolved for the first implementation in [the Phase 1 specification](MUNDARIS_PHASE_1_REFERENCE_FRAMES.md). No terrain work should precede validating that foundation.

---

# 69. Phase 4 LOD Questions

Before committing to a planetary LOD structure, prototypes must answer:

1. Does cube-sphere distortion remain acceptable?
2. Which cube-to-sphere mapping is used?
3. How are patch bounds represented?
4. How is geometric error estimated?
5. How are neighbor LOD differences constrained?
6. How are cracks prevented?
7. How does morphing behave while moving rapidly?
8. How many active patches are typical from orbit, atmosphere, and ground?
9. Can patches be generated asynchronously?
10. Can patch requests be canceled cheaply?
11. How does local volumetric editing integrate later?
12. How does horizon culling reduce work?
13. How are forests/water/biomes keyed to the same spatial hierarchy or a compatible hierarchy?
14. Does the topology support deterministic stable region IDs?
15. Does it behave well near cube-face edges and corners?

The chosen system should be benchmarked rather than selected because it is common in tutorials.

---

# 70. Terrain Generation Questions

Before committing to the first serious generator:

1. Which fields are global vs locally evaluable?
2. What is the maximum displacement bound needed by culling/LOD?
3. How are continental/ocean masks generated?
4. How are mountain systems separated from fine noise?
5. How does the generator guarantee cross-patch continuity?
6. How is generator versioning encoded?
7. Which parts run on CPU vs GPU?
8. How are normals derived consistently across LODs?
9. How are fake erosion/gradient operations made patch-safe?
10. Which lower-resolution global fields should be cached?
11. How do terrain parameter edits invalidate generated regions?
12. How do future tectonics feed the same interface?

---

# 71. Terrain Editing Questions

Before commercial/user-facing sculpting:

1. What is the sparse edit spatial index?
2. What field does an edit modify?
3. How are edit operations composed?
4. Is edit history stored as operations, baked density, or hybrid?
5. How are caves meshed?
6. How are surface/base terrain and volumetric modifications joined without seams?
7. How are collision and rendering updated?
8. How does undo work?
9. How do edits survive generator-version changes?
10. How large can an edited region become before it should be rebaked?
11. How are edited chunks compressed?
12. How are LOD representations of edits generated?

---

# 72. Future Tectonic Integration Contract

To guarantee tectonics can be added without replacing the engine, future terrain generation should be able to accept macro fields such as:

```text
base_elevation(direction)
crust_type(direction)
plate_id(direction)
boundary_type(direction)
uplift_strength(direction)
rift_strength(direction)
volcanic_activity(direction)
geological_age(direction)
```

Not all of these need to exist initially.

The important concept is that tectonics can become a provider of **large-scale generation fields**.

Terrain LOD then samples/refines the resulting planet in exactly the same way it samples a simpler generator.

Tectonics should improve the planet definition, not replace the renderer.

---

# 73. Future Climate Integration Contract

Likewise, climate can later provide fields such as:

```text
temperature(direction, elevation, time)
precipitation(direction, time)
humidity(direction, time)
wind(direction, altitude, time)
snow_fraction(direction, time)
```

Early versions may use static approximations.

Later versions may use seasonal or dynamic models.

Biome and vegetation systems consume these fields without caring how sophisticated the climate provider is.

---

# 74. Future Procedural Graphs

A node-based world-generation editor could eventually be valuable for a commercial world builder.

Do **not** architect the engine around a node graph yet.

Instead, ensure generation systems have explicit inputs/outputs so they could later be exposed as graph nodes.

Potential graph domains:

- terrain fields;
- biome masks;
- vegetation distribution;
- materials;
- atmosphere settings.

The runtime representation should remain efficient even if the authoring UI becomes graph-based.

---

# 75. Export Architecture

A future world-building product may export:

- height maps;
- normal maps;
- biome maps;
- masks;
- terrain meshes;
- planet textures;
- orbit/body metadata;
- selected regions;
- screenshots/video;
- possibly common DCC/game-engine formats.

Export is downstream of authoritative world state.

Do not let an external format dictate internal engine representation prematurely.

---

# 76. Diagnostics UI

Developer/editor diagnostics should eventually make invisible engine state visible.

Useful overlays:

- current reference frame;
- render origin;
- camera absolute/body-local position;
- body velocity;
- active terrain patches;
- patch LOD colors;
- patch bounds;
- generation queue;
- cache occupancy;
- biome/environment fields;
- vegetation clusters;
- GPU timings;
- horizon culling statistics.

Good visualization will be essential for debugging scale systems.

---

# 77. Logging Semantics

Logs should describe domain meaning.

Prefer:

```text
terrain patch generation failed: body=..., patch=..., lod=...
```

rather than:

```text
worker 7 failed
```

High-frequency systems should use structured tracing/metrics rather than flooding text logs.

---

# 78. Versioning Philosophy

Mundaris will have several independent version concerns:

- application version;
- project format version;
- terrain generator version;
- biome generator version;
- vegetation generator version;
- derived cache version;
- shader/resource schema version where necessary.

Do not use application version as the only compatibility key.

---

# 79. Naming and Semantic Clarity

Prefer domain-specific names.

Good:

```text
BodyLocalPosition
SystemPosition
RenderRelativePosition
TerrainPatchId
SimulationSeconds
SurfaceRadiusMetres
```

Bad:

```text
Pos
Thing
Data
Manager
Handler
GlobalCoord
```

Use `Manager` only if the type genuinely manages a lifecycle/resource set that cannot be named more precisely.

---

# 80. Code Ownership Rules

A useful test for subsystem placement:

> If the renderer were replaced tomorrow, would this data still be part of the saved world?

If yes, it probably does not belong in `mundaris_renderer`.

Another:

> If the UI disappeared, would this simulation still make sense?

If yes, it probably does not belong in `mundaris_app`.

And:

> Is this mathematical concept meaningful without a specific planet/project instance?

If yes, it may belong in `mundaris_math`; if it requires world identity/configuration, likely `mundaris_world`.

---

# 81. Review Gates

Before each major phase is considered complete, perform three reviews.

## 81.1 Correctness review

- Does it behave as intended?
- Are tests meaningful?
- Are edge cases understood?

## 81.2 Architecture review

- Did implementation violate engine invariants?
- Did rendering accidentally own world state?
- Did a prototype abstraction become too broad?
- Are future replacement paths still clear?

## 81.3 Performance review

- Is it measured?
- Are allocations/workloads understood?
- Is a bottleneck structural or merely unoptimized?
- Does it degrade predictably?

Do not optimize merely because code looks low-level.

---

# 82. Commit Philosophy for Engine Work

Commits should be coherent and reviewable.

Examples:

```text
feat(math): add high-precision frame transform primitives

test(math): add astronomical precision regression cases

feat(renderer): add camera-relative transform upload path

docs(adr): select planetary surface partition prototype

perf(terrain): batch patch generation uploads
```

Avoid giant commits containing unrelated architecture, feature, formatting, and dependency changes.

Repository content and commit messages should describe engineering intent without provenance headers.

---

# 83. Definition of Architectural Success

The architecture is succeeding if, over time:

- new terrain generators do not require renderer rewrites;
- new LOD algorithms do not destroy saved worlds;
- planets can move without updating every attached object;
- high local detail can exist on astronomical-scale bodies without precision failure;
- generated objects can be removed persistently without serializing the whole forest;
- terrain edits survive unloading and LOD changes;
- more advanced tectonics/climate can replace simple approximations through established inputs;
- GPU caches can be discarded without losing authored work;
- product/editor features can evolve without contaminating simulation/math layers;
- performance improvements can replace derived representations without changing world semantics.

---

# 84. Definition of Failure

The architecture needs reconsideration if we discover that:

- terrain edits require storing or rebuilding an entire planet;
- moving a planet requires translating all surface objects;
- GPU precision determines simulation precision;
- LOD meshes contain information unavailable anywhere else;
- changing a procedural algorithm silently destroys existing projects;
- vegetation persistence requires serializing every generated tree;
- the editor cannot change parameters without blocking on a full synchronous world rebuild;
- every natural simulation feature becomes coupled directly to rendering;
- adding tectonics requires replacing the entire terrain/render pipeline;
- local caves require abandoning the global planet representation entirely.

These are architectural red flags, not merely implementation bugs.

---

# 85. Immediate Next Step

Do **not** start implementing terrain, planets, or gravity directly from this document.

The first implementation contract is:

```text
MUNDARIS_PHASE_1_REFERENCE_FRAMES.md
```

That document turns the Phase 1 goals into a narrow implementation specification covering:

- coordinate type semantics;
- units;
- frame identity and ownership;
- transform composition;
- translations and rotations;
- velocity semantics;
- high-precision to render-relative conversion;
- camera representation;
- test matrix;
- validation scene;
- crate/file placement;
- performance constraints;
- exact non-goals;
- acceptance criteria.

Design documentation is not implementation evidence. Phase 1 must be implemented and validated before expanding scope.

## Architectural review — 2026-10-01

The review retains the six-crate structure, authoritative/derived-state split, sparse-edit model, independent layers, multi-scale generation, and intentionally open terrain/LOD/simulation algorithms.

Focused corrections:

| Sections | Issue | Correction and reason |
| --- | --- | --- |
| 6.12, 13.4, 13.8 | Observer-relative detail selection and subtracting flattened `f64` positions did not guarantee local precision. | Require shared-ancestry-relative evaluation, explicit error budgets, and renderer-owned narrowing. Distinguish coordinate precision from depth and distant representations. |
| 13.2 | A moon shown below a body-fixed planet could inherit planetary spin. | Separate transform ancestry from orbital/domain relationships; translating and body-fixed frames may differ. |
| 8, 9 | Intended dependencies were easy to mistake for actual links; frame-tree ownership and render consumption were unspecified. | Record the bootstrap graph, place generic math in math and future instance/domain ownership in world, and define the read-only app → renderer/math boundary. |
| 13.3, 17.1, 42.3 | Scale/context and mutable reference coordinates could leak into terrain truth or persistent identity. | Keep body-fixed definition/keys independent of observer, LOD, runtime handles, and origin choice. No terrain representation is selected. |
| 13.4–13.7 | Rotating-frame velocity, pose transitions, rigid-transform limits, and evaluation time were too vague. | Define derivative semantics, coherent-instant evaluation, and separate pose re-expression from physical attachment; defer force/integration models. |
| 23 | Example physical fields had ambiguous units. | Label temperature in kelvin and dimensionless normalized fields explicitly. |
| 47, 67, 68, 85 | Policy/status and bootstrap roadmap diverged from repository reality and the next-phase scope. | Preserve the unsafe prohibition, avoid unverified runtime-completion claims, align phase ordering, and link the implementation contract. |

Remaining choices include distant/depth rendering, universe-scale addressing, physics/integrator policy, terrain topology, edit encoding, geological deformation, hydrological/climate feedback, persistence, and stricter cross-platform determinism. None requires a speculative Phase 1 subsystem.

---

# 86. Final Guiding Principle

Mundaris should not attempt to keep the entire universe detailed.

It should maintain a **coherent truth** about the authored world and continuously produce the cheapest representation that preserves what matters at the observer's current scale.

The engine's job is therefore not:

> render everything.

It is:

> **preserve one consistent world across extraordinary changes in scale, fidelity, time, and representation.**

That principle should guide every major architectural decision.
