# Architecture

## Workspace responsibilities

| Crate | Responsibility |
| --- | --- |
| `mundaris_core` | Small, dependency-light foundations shared by multiple systems when a concrete need exists. |
| `mundaris_math` | Checked f64 SI coordinates and working-epoch time value, rigid transforms, runtime frame tree, LCA evaluation and instantaneous derivative conversion using `glam`. |
| `mundaris_world` | Authoritative celestial properties/kinematics, validated prescribed-motion definitions, one coherent sample instant, transactional mutations and derived body/frame associations. |
| `mundaris_simulation` | Bounded direct analytic motion sampling; serial Newtonian gravity/KDK, integer fixed ticks, admitted demand, bounded snapshots/replay and numerical diagnostics. |
| `mundaris_renderer` | GPU presentation infrastructure and disposable visual representations. |
| `mundaris_app` | Native process, event loop, logging, and top-level subsystem composition. |

Dependencies point inward toward lightweight/domain foundations. World and simulation code must not depend on `wgpu`, `winit`, or `egui`; renderer code must not own simulation truth. The app is the composition root. Avoid cycles and dependencies that exist only to make a diagram look complete.

The long-term intent is detailed in [the engine design](../MUNDARIS_ENGINE_DESIGN.md). [Phase 1](../MUNDARIS_PHASE_1_REFERENCE_FRAMES.md) implements generic frame-tree mathematics in `mundaris_math`, an app-owned validation fixture/observer session, and read-only mathematical evaluation passed to the renderer. Future domain-owned tree instances/associations belong in `mundaris_world`. Runtime frame identity remains distinct from persistent domain/generated-content identity.

Current project dependencies are app → renderer/math/world/simulation, renderer → math, world → math, simulation → math/world. Core remains documentation-only. Criterion is a dev dependency of math/renderer/world/simulation/app; matching naga validates renderer WGSL headlessly. World/simulation have no graphics dependencies.

## Celestial domain and time

`CelestialSystem` owns contiguous append-only bodies with opaque namespaced `BodyId`,
validated positive mass/reference radius, checked system-space center and velocity,
body-to-system unit quaternion, angular velocity in system axes, one
`SimulationInstant` and checked revision. Names are display metadata limited to
1–128 UTF-8 bytes. No classification, primary hierarchy or gravitational constant
is needed. `BodyState::new` is infallible because its four private math value types
have already validated every component; raw non-finite values cannot enter it.

Property/metadata edits are atomic and independent of state edits. State batches
accept arbitrary ID ordering, reject foreign/unknown/duplicate IDs before commit,
and increment revision once. Reusable dense duplicate flags cost O(U) without hot
allocations. Moving to a different instant requires every body's state; same-time
authoring can edit subsets. Immutable borrowing supplies coherent reads without
cloning an entire system or introducing a snapshot framework.

`SimulationInstant` lives in math as a finite, dimensioned seconds value relative
to a local working epoch. Simulation's `TimeController` consumes monotonic host
durations and changes requested time only. The app's pure bounded analytic
producer explicitly samples/commits the world and then publishes frames; this is
not an integrator or a free-seek promise for future integrated simulations.

Phase 3's `FixedStepRunner` operates on borrowed mutable world state, never frames.
It gathers stable dense IDs/masses/full states into reusable candidate buffers,
evaluates serial lexicographic unordered pairs in f64, and uses full-time KDK
velocities. One new force pass is needed per warm step; old acceleration was
validated at initialization or the previous successful drift. Candidate validation
precedes one transactional full-state world commit. Cache promotion and complete
history recording follow success. A narrow callback observes every public commit
for app trail sampling. External revisions/appends reject stale sessions.

Integer time is reconstructed from epoch/tick/fixed h with resolution guards.
Rate-segment anchors plus cumulative exact Duration distinguish admitted demand
from achieved authority. Work is bounded; over-cap intervals are rejected visibly,
preserving previous debt. Reverse uses recent complete snapshots and deterministic
positive replay outside retention. Private replay publishes only its completed
target. Physical edits preflight proposed force/diagnostics/capacity, then capture a
new immutable branch baseline; name edits preserve physical history. See
[ADR 0004](adr/0004-gravity-integration-and-playback.md).

`CelestialFrameProjection` is world-owned, disposable and rebuildable using a
fresh caller-supplied tree namespace. Every body has a root-child translating
anchor (center/velocity, identity rotation, zero spin) and a rotating body-fixed
child (zero translation/linear motion, authoritative orientation/spin). Other
bodies never inherit that spin. Dense body/frame mappings survive live appends.
Full publication reuses staging, preflights capacity/revisions and copies f64
state into one Phase 1 batch at the exact system instant; represented revision is
recorded after success. No mutable tree access is exposed and no celestial body
stores a frame ID. Renderer/UI receive read-only views; editor commands use world
validation. See [ADR 0003](adr/0003-celestial-domain-and-time.md).

The explicit `coherent_view` gate validates system namespace/topology, represented
revision and exact sample time, retaining both immutable borrows through render
preparation/submission. App projects once after pumping; failure pauses and
suppresses celestial drawing, retaining valid physics. Fresh projection namespaces
are remapped through body identity/attachment role rather than persisted FrameId.

## Phase 5.13A prescribed celestial motion

World's immutable `CelestialMotionDefinition` validates a complete namespaced set
of stationary/elliptic translations plus independent `AxialSpin`. An iterative
single-parent traversal derives reference-before-child order, regardless of body
insertion order. Orbital-plane orientation uses system axes; references add only
center position/velocity. Explicit authored periods do not change with mass/radius.

Simulation's concrete `AnalyticMotionProducer` owns that definition and reusable
dense candidate states. It reduces elapsed time by the period, solves bounded
elliptic Kepler motion, derives analytic velocity and local-axis spin, then commits
the entire candidate at one instant. No integration/replay precedes a seek. Errors
preserve authority, time and revision; external celestial revisions/appends reject
stale bindings. Its own commits advance the binding; terrain-only edits remain
independent. Default solver limit is 64, configurable 1–128. See
[ADR 0007](adr/0007-prescribed-celestial-motion.md) and the
[phase specification](../MUNDARIS_PHASE_5_13A_ANALYTIC_CELESTIAL_MOTION.md) for the
bounded time/parameter envelope and evidence requirements.

Each independent system evaluates locally, without a galactic offset. Future
universe location → system-local state → body-local content → observer-relative
rendering is a boundary, not implemented universe addressing/streaming. The producer
is not mass-consistent N-body gravity or a calendar ephemeris.

Phase 5.13B's concrete app-owned `MotionSession` selects analytic motion for both
solar presets and retains `FixedStepRunner` for the hierarchy/circular scenarios.
`TimeController` requests signed fractional instants; complete world sampling then
coherent frame publication precede navigation/render preparation. Unchanged valid
times are not sampled again. Failed targets retain authority and pause; failed
frame publication retains the complete sample but suppresses incoherent drawing.
Atomic supported edits explicitly reconstruct the immutable binding without
changing periods. Analytic guides use authored ellipses, trails record only
successful synchronized publications, and snapshot schema 3 separates motion-mode
diagnostics. See [analytic playback](ANALYTIC_PLAYBACK.md).

## State and representation boundaries

World state is authoritative; rendering is a view of that state. Generated meshes and GPU resources are disposable caches, never the only representation of terrain or user changes. Future edits should be stored as authoritative modifications, invalidate affected derived data, and remain valid across changes in level of detail.

All Phase 1 physical values, including local geometry and observer state, use `f64`. Local GPU-facing representations use `f32`; conversion belongs at the renderer's observer boundary. Frame-local attached content stays unchanged when its ancestors move.

The implemented precision path cancels shared frame ancestry and subtracts the observer in the source frame before rotation and renderer-owned narrowing. Independently flattened root positions can already have lost local detail even in `f64`. Transform ancestry carries rotation as well as translation and is distinct from orbital relationships; a moon must not inherit a planet's body spin accidentally. All evaluated poses and motion derivatives describe one coherent instant.

## Evaluation and renderer contracts

- Append-only contiguous nodes use caller-assigned nonzero namespace/u32-index handles. The root is immutable identity. A checked session counter allocates fresh namespaces, including resets. Handles are not serialized; deletion/reuse is deferred.
- Strictly ordered `update_states` validates a complete batch before publication and costs O(U), with no descendant/object work. Reparenting explicitly supplies new local state and transactionally rebuilds depths in O(F). Finite sample time is metadata, never an extrapolation request.
- `FrameEvaluation` is a small copyable immutable borrow. Prepared conversions/views retain that borrow independently of the evaluation wrapper's lifetime, making mutation while they are live a compiler error. No interior mutability, owned snapshot, transform cache or dirty-subtree machinery exists.
- Pose preparation walks only branches below the LCA using stack values. Branch origins are subtracted before target rotation. Kinematic preparation separately composes origin/angular derivatives on those same branches; unknown required motion is an error. Pose-only rendering does not compose derivatives.
- Re-expression preserves instantaneous pose and physical velocity. Attached inspection deliberately supplies zero relative regional velocity; these are separate operations. Camera orientation maps camera-local axes to the observer frame; camera forward is -Z.
- A prepared source centres in source coordinates, then rotates into camera axes. Narrowing checks finite output, Euclidean range and actual component round-trip error. The fixture selects whole out-of-range primitives explicitly and shows distant bearing/distance markers; the 10 km debug range is not an engine draw limit.
- `DebugFrame` accepts f64 line requests through its retained view, resets reusable staging, and poisons itself on any failed batch. It cannot append arbitrary owned relative vertices from another view. Only a complete validated frame reaches GPU upload. Shader data consists of 32-byte position/color vertices and a 64-byte column-major projection; no view translation or astronomical origin is present.

Concrete API adjustments and evidence are recorded in [ADR 0002](adr/0002-reference-frames-and-precision.md) and [Phase 1 validation](phase-1-validation.md).

Future large procedural regions should be reconstructed from explicit deterministic inputs and overlaid with sparse persistent modifications. This leaves room for a global procedural surface plus sparse local volumetric changes, including edits that cannot be represented as a heightfield. Terrain, vegetation, water, atmosphere, simulation, and rendering remain independent concerns.

## Native presentation boundary

The renderer creates a native surface/device, presents, handles resizing, draws
the existing forward-depth debug path or separate infinite reverse-Z celestial
path, and integrates egui. Celestial requests contain copied physical radius,
fixed-frame handles and styles, without BodyId/world/simulation dependencies.
One 642-vertex/1280-triangle indexed icosphere topology is reusable; each vertex is
formed in f64 source coordinates, source-centred, camera-rotated and narrowed under
physical-radius and 0.05-pixel budgets. Camera-axis normals and color/flags provide
debug shading. Opaque spheres write reverse depth; historical lines test without
writes, before no-depth navigation overlays/UI. No depth bias is used.

App owns the focus camera, selection and bounded synchronized f64 TrailHistory.
Trail samples are committed tick/time/all-body centres, without frame handles.
Simultaneous relative history subtracts both bodies at each historical sample;
today's projection only labels conversion sources. Clipping/narrowing/staging are
renderer-owned. Failed frames cannot upload partial batches. The app library target
supports headless session tests and history benchmarks in the existing package.
Procedural planet generation, displaced terrain, persistence and general editor
systems remain deferred; smooth surface LOD is described below.

The app supplies the bootstrap panel through a per-frame UI callback; the renderer owns only UI input and GPU integration. Native events and UI types stay at this app/renderer boundary. An `Arc<Window>` safely keeps the surface's native handle alive without leaks or shared mutable domain state. Presentation notifies `winit` before submitting the frame to the compositor.

The smoke app uses bounded continuous redraw to exercise presentation without adding an editor repaint scheduler. Its 16 ms deadline remains stable across input wakeups, skips catch-up frames, and stops timer wakeups while minimized, zero-sized, occluded, or suspended. FIFO presentation also provides GPU/display pacing. Suspension releases the renderer before the window; resume recreates them. This bootstrap cadence is not a future simulation timestep.

## Phase 3.5 derived celestial explorer

App headless modules own `SystemViewBounds`/scopes, validated BodyId selection,
physical-sphere/marker/displaced-label picking, deterministic label layout,
camera control policy, guide reference choices, history display, rolling playback
metrics and `InteractiveClock`. All their physical inputs are immutable current
world or coherent world/projection borrows. No guide or navigation grouping is
stored in `BodyState`, changes force pairs, reparents frames or conveys spin.

Simulation's checked `osculating_elements` derives an instantaneous relative
two-body conic from position/velocity/pair masses. Only resolved elliptic geometry
has a closed guide. App chooses conservative automatic or explicit references,
evaluates differential external perturbations and supplies f64 frame-tagged
derived curves. These are neither actual historical motion nor full N-body future
predictions. Simultaneous relative history subtracts both recorded bodies at the
same sample; display/reference changes retain absolute history.

System Orbit, Body Orbit and scale-aware Free Flight operate on one Phase 1
observer. Focus smoothing, clearance zoom, numerical carrier compensation and
frame rebuilding update observer/navigation state only. Render preparation remains
source-centred f64 before narrowing. The renderer's explicit content viewport drives
projection, ray unprojection, overlays and GPU viewport/scissor. Generic pixel-width
solid/dashed polylines use checked clipped clip-relative quads, alpha blending and
reverse-Z/no writes; the app keeps guide/history/debug-axis semantics separate.

Runner `pump_with_work_limit` preserves baseline fixed h, pair order, snapshots,
rollback and replay. App work opportunities start with one unit, then at-most-32
chunks/512 total, checking 4 ms between chunks. Achieved playback uses real public
commit deltas and accounted active wall duration, excluding seek/reset/single/replay
jumps. Long drawable gaps reject the entire elapsed interval, cancel demand and
camera progression, and require Resume. Detected hidden time resets capture without
background catch-up. The same host-clock classification is used by analytic modes.

Implemented decisions, API deviations and unresolved platform evidence are in
[ADR 0005](adr/0005-celestial-navigation-system-view-and-timewarp.md) and
[Phase 3.5 validation](phase-3-5-validation.md). Approximate authoritative
propagation, preview, terrain and Phase 4 remain outside this milestone.

## Phase 4 planetary surface boundary

The [Phase 4 specification](../MUNDARIS_PHASE_4_PLANET_SURFACE_REPRESENTATION_AND_LOD.md)
is implemented as the smooth-sphere CPU baseline. It selects normalized-cube surface charts and compact computed
patch addresses, while body-fixed surface locations and future persistent layer
identity remain independent of transient render leaves. Generic address/tangent
math belongs in math; observer-dependent LOD, culling, ready coverage, metadata
cache and GPU resources are renderer-derived state owned through app sessions.
World retains the one authoritative BodyId/reference radius/state; no permanent
terrain state is introduced by the smooth-sphere validation.

Current clearance zoom already reaches 1 m above reference radius, but the fixed
icosphere is not locally surface-accurate. Its all-vertex precision fallback and
single-sphere draw constants require a separate patch representation. Infinite-far
reverse-Z is reusable; surface and far bodies must share its depth ownership.
Default orbit-centre look and system-stationary free flight need an explicit
co-rotating inspection control for stable local look-around on a moving planet.
The projection exposes no arbitrary regional-frame insertion; Phase 4
uses body-fixed f64 inspection anchors without allocating a frame for each patch.

Math supplies canonical dyadic samples, address/neighbor/tangent mathematics.
Renderer sessions own complete sorted contiguous balanced covers, previous quality
splits, pending local ready transactions and a bounded metadata LRU. Only visible
leaves are evaluated/uploaded. Shared sixteen u16 stitch variants and sample32/
instance64 storage drive at most sixteen instanced draws plus counted clipped
fallback, in the existing celestial depth pass. App owns explicit BodyId capability,
readiness/error handoff, one-observer co-rotating inspection and validation routes.
Observations/labels/picking keep their dense body association even when sphere
geometry is suppressed. No world/simulation functionality or dependency changes.
See [ADR0006](adr/0006-planet-surface-topology-and-lod.md) and
[evidence](phase-4-validation.md) for API choices, measurements and open acceptance.

## Phase 5 terrain design boundary (not implemented)

The [Phase 5 specification](../MUNDARIS_PHASE_5_PROCEDURAL_TERRAIN_GENERATION.md)
adds pure deterministic terrain queries in world, with generic noise/derivative
math in math. App adapts immutable terrain definitions and regional certificates to
renderer-owned domain-free geometry, and owns bounded generation/cache orchestration.
Renderer consumes reusable f64 body-fixed samples and prepares current observer-relative
data; it does not own seed, generator version or terrain truth. The existing dependency
graph and six crates remain unchanged.

Geometry readiness extends metadata readiness without removing parent coverage.
Scale filtering, displaced canonical boundaries and temporary old/new triangle-overlay
morphs compose with the frozen balanced grid16/16-stitch representation. Terrain
certificates extend error/culling/handoff conservatively; uncertified terrain horizon
occlusion is disabled. Stable views and celestial motion do not regenerate terrain.

Phase 5 development is permitted despite retained Phase 4 acceptance debt and the
accepted approximately 11 ms full-view preparation limitation. Outstanding platform/
operator gates remain required before a later release-quality milestone; this design
neither changes the frozen Phase 4 architecture nor claims those gates completed.

## Procedural crater terrain foundation

Phase 5.15 adds `CrateredV1` (code 3) as a distinct deterministic world algorithm.
Immutable identity, seed, configuration and reference radius produce a compact
body-fixed crater catalogue. Analytic profiles and gradients
feed the existing query, certificate and terrain geometry paths. V1/V2 definitions
retain their previous composition. The app's reusable crater recipe accepts any
supported body radius and explicit seed; the existing Moon is its integration
fixture. Procedural planetary-system assembly remains a separate domain task.

Compact support permits regional height and derivative certificates to omit distant
craters. Landmarks retain their complete height at every footprint; their curvature
drives interpolation error. Crater profile differences across LODs are exactly zero;
background noise and erosion remain filtered. Adaptive selection, stale transaction checks, prefetch and
convergence use the same regional boundary certificate. Stitch owners share the
same canonical direction, and interior boundary corrections are convex combinations
of edge deltas within the patch cap. Background noise/erosion retain their global
bounds. Catalogue and query workspace bytes enter the existing serial, worker and
coordinator reservations. Quality thresholds are unchanged. The user explicitly
retired the 128 MiB terrain ceiling on 2026-10-05; the demand-driven aggregate CPU
terrain ceiling is now 512 MiB, including staging and transition reservations.
This foundation does not establish whole-view convergence or visual acceptance;
see the [contract](../MUNDARIS_PHASE_5_15_PROCEDURAL_CRATER_TERRAIN_FOUNDATION.md)
and [handoff](PHASE_5_15_PROCEDURAL_CRATER_TERRAIN_FOUNDATION_REPORT.md).

## Planet terrain redesign reference boundary

The [Slice 1 contract](PLANET_TERRAIN_SLICE_1.md) begins a separate moon-like
terrain path in `world::terrain`. `MoonTerrainDefinition` and `MoonLikeV1`/`MoonLikeV2` carry
explicit immutable identity, seed and configuration; complete queries produce
height, body-tangent gradient and correlated material weights. The existing
V1/V2/CrateredV1 definitions and production terrain path retain their semantics.
Slice 1B publishes compositional definitions through
`CelestialBody::surface_definition()` and `CelestialSystem::edit_surface_definition()`.
Legacy `terrain()`/`edit_terrain()` remain compatible. Publication selects one
authority and clears the competing legacy/compositional definition, advances the
terrain revision only, and validates the complete envelope before mutation.
Body radius edits validate the selected authority before publication as well.

The app's `moon_surface_reference` example consumes that world query for a
temporary fixed-resolution software reference rasterizer. It retains f64 through
sampling and observer-relative geometry, and records representation spacing and
sampled residuals separately from complete normals/materials. These captures
establish a visual review fixture, not GPU reconstruction, streaming convergence,
native performance or user acceptance. The GPU tile/regular-patch replacement
remains gated by the [redesign](PLANET_TERRAIN_RENDERING_REDESIGN.md).

`MoonLikeV2` fills the first prototype's scale gaps with ten impact epochs and
two independently seeded cell layouts per epoch. Convex age composition bounds
overlap; regional plains modulate impact relief and correlated highland structure,
with analytic mask derivatives included. V1 stays separately selectable with
its original numerical semantics. The reference example uses fixed triangle
meshes and a CPU triangle-ray index for cast-shadow comparisons; that index is
disposable render data, not terrain authority, GPU tile residency or a
collision/walking implementation. Local shadow queries use crop-only casters.

The [redesign variety requirement](PLANET_TERRAIN_RENDERING_REDESIGN.md#variety-across-moons--user-clarification-2026-10-05)
treats orbital role, body shape, structural terrain family, materials and optional
atmosphere as separate definition choices. The implemented lunar family is not a
generic appearance model for all moons. Its spherical reference, radial envelope
and regolith/rock/basalt weights do not establish irregular, ice, volcanic or
atmospheric-family capability by themselves. The [Slice 1B contract](PLANET_TERRAIN_SLICE_1B.md)
adds `SurfaceDefinition` with independent shape, geological parameters, material
definition and atmosphere descriptor. `RockyV3` composes the unchanged V2 oracle
with a seeded body history; `IcyV1` and `VolcanicV1` supply distinct complete fields.
Material format versions must agree with geological channel semantics; composition
and regional contrast remain independent controls and cannot move terrain.

`SurfaceGenerator` supplies complete radial shape/displacement, analytic tangent
gradients, combined outward normal, four normalized material weights and explicit
global envelopes. Shapes are positive radial graphs, including bounded asymmetric
triaxial definitions. This represents star-shaped bodies only: arbitrary concavity,
caves, overhangs and every contact-binary topology require a separate representation.
See [ADR 0009](adr/0009-compositional-body-surfaces.md).

The production clearance/navigation adapter reads the same selected world surface,
including shape radius and gradient. These radial clearance queries are not solid
collision or walking. The native adapter now dispatches the selected legacy or
compositional authority into the existing worker/cache, adaptive selection,
stitching and morph pipeline. The Solar System Moon selects `RockyV5` in both
presets; each LOD queries that same complete field, and camera clearance uses the
world-owned complete query. This reuses the current CPU tile architecture and
renderer material presentation; it does not add GPU tile residency or four-channel
material shading. The reference examples remain temporary fixed-grid renderers for
family review. This integration makes no production performance or visual-approval
claim. See the [native Moon integration handoff](NATIVE_MOON_INTEGRATION_REPORT.md).

For compositional native surfaces the radial envelope must remain within the
renderer’s existing ten-percent support limit. The safe patch error certificate
uses twice the complete radial amplitude bound plus the sphere correspondence
error, with zero footprint-profile delta. It does not provide a finite derivative
certificate or demonstrate error convergence, so desired radial LODs can be
pathological (the current Moon snapshot requests LOD 30) while a ready coarse cover
remains `quality_pending`. The compositional field's material presentation,
mesh-normal and shadow treatment have not migrated to the reference presentation.
These quality limits remain under review; see the
[native Moon integration handoff](NATIVE_MOON_INTEGRATION_REPORT.md).

The first refinement fix adds a separate 1 px projected-sample-spacing demand
signal; it can guide refinement without pretending that the global error target is
certified. Optional snapshot fields identify whether refinement demand comes from
`projected_sample_spacing` or `certified_error`, and expose `target_certifiable`.
Compositional positions still come from the complete field. Approximate mesh normals
use finite secants at the mesh footprint along three fixed body axes, avoiding a hard
basis switch; about seven normal queries per vertex run on workers. Population caches
the native identity and bound in eight fixed slots keyed by body, revision, exact
definition and reference-radius bits, avoiding repeated generator compilation
without retaining generator heap.

Slice 1B.1 extends the same world-owned authority with versioned geological
directors (`RockyV4`, `IcyV2`, `VolcanicV2`). Body phenotype biases smooth
body-fixed province weights, which control family morphology and bounded local
process composition. The prior algorithms retain their numerical semantics.
`SurfaceGenerator::geological_controls()` exposes diagnostic history/province
values without introducing camera or renderer inputs into generation. Individual
director maps, neutral geometry and deterministic province crops belong to the
reference evidence path. See [ADR 0010](adr/0010-geological-province-directors.md)
and the [Slice 1B.1 contract](PLANET_TERRAIN_SLICE_1B_1.md). Production clearance
continues to query selected world authority; new GPU/LOD rendering remains gated.

Slice 1B.2 adds explicit `RockyV5`, `IcyV3` and `VolcanicV3` hierarchical
residuals over their preserved province fields. Signed normalized larger process
contributions provide parent morphology independently of broad elevation. Three
physical regimes add family-specific local features and bounded continuous
process networks; finer bands inherit already evaluated coarser contributions.
`SurfaceGenerator::detail_diagnostics()` exposes derived band contributions,
gradients and work from the complete authority. Historical query semantics remain
versioned separately. The fixed reference package adds 8 m and same-feature crops,
not a production LOD path. See [ADR 0011](adr/0011-hierarchical-geological-residuals.md)
and the [Slice 1B.2 contract](PLANET_TERRAIN_SLICE_1B_2.md).

## Development session boundary

### Fixed resident terrain prototype

The explicitly enabled Slice 2A/2B fixture is separate from the planetary adaptive
selector. App's `ResidentTileBuilder` samples immutable world surface definitions
into versioned radial-displacement/material tiles with a one-sample halo. Renderer
owns their resident buffers, shared regular-grid topology, and presentation
metadata. Shader reconstruction consumes derived payloads and contains no world
surface-generation algorithms. Tile identity includes body/definition, radius,
authority revisions, address, resolution, filter, and format; camera and lighting
remain presentation inputs.

The fixed hierarchy retains one parent and four children in five bounded slots.
Child workers and uploads complete independently while the parent covers the
whole region. Draw ownership transfers atomically after all four are resident.
GPU morphing starts on the actual indexed parent triangles, preserving their raw
normal varying and material interpolation, and ends on each child's derived tile.
Both endpoints use the parent's local anchor and prepared observer-relative
transform. Request epochs reject late worker results; slot generations protect
publication. This prototype has no whole-body streaming, pressure eviction, or
global mixed-level topology. See [ADR 0012](adr/0012-resident-terrain-tile-prototype.md)
and [ADR 0013](adr/0013-fixed-resident-terrain-hierarchy.md).

### Ordinary planetary resident terrain (Slice 2D)

The ordinary `--solar-system` and `--real-solar-system` presets use an app-owned
`PlanetaryTerrain` runtime over the six-face resident regional scheduler. The
explicit `--legacy-terrain` switch retains the earlier adaptive terrain path for
comparison. This integrates resident terrain into the ordinary solar presets; it
does not replace world-owned surface definitions or change terrain generation
authority. Exact tile identity and source revisions gate rebinding and drawing, so
a stale resident result cannot be presented after the selected body's definition,
radius, or terrain revision changes.

Planetary construction uses a bounded four-worker generation pool and a separate
publication coordinator with at most two parallel background calculation tasks.
The runtime bounds the desired set to 16,384 patches,
CPU residency to 49,152 tiles, and GPU residency to 32,768 logical slots; each frame admits
at most four uploads / 8 MiB and eight local publication adoptions, with sixteen
active morph groups. These are current runtime
configuration values, not measured performance targets or physical VRAM use. The
selector checks a 2 ms work budget and reports overruns. Publication uses one
in-flight bounded batch and eight retained prepared products; locally blocked
work waits without rebuilding while independent products can proceed. Exact
keys, current local sources/demand/balance and affected boundary expectations
validate each adoption. Frame-thread admission defers against a cumulative 2 ms
budget with 1.5 ms dispatch/adoption headroom and reports overruns. These are runtime limits with
observable overruns, not guaranteed frame-time ceilings. The observer-relative
anchor narrowing uses the original one-millimetre gate through 10 km. Beyond that
envelope, its presentation allowance is the maximum of
distance / 2^23, chart footprint / 262,144, and one millimetre; finite fixtures
keep their original precision policy. This bounds f64-to-f32 anchor presentation,
not terrain approximation or local tile reconstruction. Projected relief and
sagitta drive an explicitly uncertified quality proxy, so
capacity pressure or incomplete convergence remains `quality_pending`; matching
the proxy does not certify terrain quality. Frustum and horizon culling affect
visibility while the resident topology remains a complete balanced cover.

The publication worker retains canonical sample owners and boundary dependencies
for its current immutable tile cover. A local replacement invalidates affected
sample owners, edge neighbors and interpolation dependents; full cover validation
and the original boundary reconstruction arithmetic remain in force. Failure
clears this derived cache, and stale work can be reversed to the acknowledged
cover. Renderer validation may reuse exact immutable endpoint objects while
validating presentation transforms and transaction state on every draw. Memory
diagnostics distinguish owned cache/index/boundary storage from aliased tile
references and conservative workspace limits; these are not RSS or physical VRAM.

Stationary scheduling reuses a selector fixed point only with complete resident
coverage and no pending work; completion draining and LRU touches continue, and
view/residency/dependency changes restore admission. Planetary frame diagnostics
retain scalar coverage, debt, jobs, timings and resources, explicitly omit
per-tile arrays and export bounded recent history. Finite fixtures retain their
detailed arrays. Snapshot retirement is included in diagnostic elapsed time.

Slices 2A, 2B, and 2C remain separately available as finite resident, fixed
hierarchy, and regional scheduling fixtures with their existing numerical and
representation contracts. Slice 2D wires the regional runtime into ordinary
planetary presentation; it does not supersede those fixtures or authorize Slice
3A. Native acceptance is pending the [dated Slice 2D report](PLANET_TERRAIN_SLICE_2D_REPORT.md).

### Session interface

The opt-in `developer-tools` application feature and `--dev-interface` flag expose
the Phase 5.14 session interface. App owns typed operations, handle namespaces,
bounded requests/receipts/events, control leases, and coherent observations. Only
the native event-loop thread applies operations. Loopback transport and immutable
PNG/JSON publication run on bounded IO workers. Ordinary launches create no
endpoint, and idle enabled sessions request no readback or procedural diagnostic
queries.

Native and deterministic offscreen scenarios share the production application
update/preparation path. Host adapters select presentation/readback and clock/work
scheduling; they do not duplicate authoritative world or simulation state. Renderer
owns actual acquired-surface copies after scene/UI composition, asynchronous map
completion and explicit submitted/skipped outcomes. Submission and a presentation
request do not prove monitor presentation. CLI and MCP remain thin app clients;
world/simulation acquire no networking or assistant dependencies. See
[ADR 0008](adr/0008-development-session-interface.md), the
[phase contract](../MUNDARIS_PHASE_5_14_AI_ENGINE_DEVELOPMENT_INTERFACE.md), and
[interface guide](AI_DEVELOPMENT_INTERFACE.md).
