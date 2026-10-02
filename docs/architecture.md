# Architecture

## Workspace responsibilities

| Crate | Responsibility |
| --- | --- |
| `mundaris_core` | Small, dependency-light foundations shared by multiple systems when a concrete need exists. |
| `mundaris_math` | Checked f64 SI coordinates and working-epoch time value, rigid transforms, runtime frame tree, LCA evaluation and instantaneous derivative conversion using `glam`. |
| `mundaris_world` | Authoritative celestial properties/kinematics, one coherent sample instant, transactional mutations and derived body/frame associations. |
| `mundaris_simulation` | Serial Newtonian gravity/KDK, integer fixed ticks, admitted demand, bounded snapshots/replay and numerical diagnostics. |
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
