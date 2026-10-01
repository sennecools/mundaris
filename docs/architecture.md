# Architecture

## Workspace responsibilities

| Crate | Responsibility |
| --- | --- |
| `mundaris_core` | Small, dependency-light foundations shared by multiple systems when a concrete need exists. |
| `mundaris_math` | Checked f64 SI coordinates and working-epoch time value, rigid transforms, runtime frame tree, LCA evaluation and instantaneous derivative conversion using `glam`. |
| `mundaris_world` | Authoritative celestial properties/kinematics, one coherent sample instant, transactional mutations and derived body/frame associations. |
| `mundaris_simulation` | Requested-time playback control; future evolution of authoritative world state. |
| `mundaris_renderer` | GPU presentation infrastructure and disposable visual representations. |
| `mundaris_app` | Native process, event loop, logging, and top-level subsystem composition. |

Dependencies point inward toward lightweight/domain foundations. World and simulation code must not depend on `wgpu`, `winit`, or `egui`; renderer code must not own simulation truth. The app is the composition root. Avoid cycles and dependencies that exist only to make a diagram look complete.

The long-term intent is detailed in [the engine design](../MUNDARIS_ENGINE_DESIGN.md). [Phase 1](../MUNDARIS_PHASE_1_REFERENCE_FRAMES.md) implements generic frame-tree mathematics in `mundaris_math`, an app-owned validation fixture/observer session, and read-only mathematical evaluation passed to the renderer. Future domain-owned tree instances/associations belong in `mundaris_world`. Runtime frame identity remains distinct from persistent domain/generated-content identity.

Current project dependencies are app → renderer/math/world/simulation, renderer → math, world → math, simulation → math. Simulation can later depend on world without a cycle. Core remains documentation-only. Criterion is a dev dependency of math/renderer/world; matching naga is a renderer dev dependency for headless shader validation. World/simulation have no graphics dependencies.

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

The renderer creates a native surface/device, presents, handles resizing, draws an optional line-list/depth debug pass, and integrates `egui`. The app's celestial validation uses those generic facilities without adding renderer domain dependencies. Gravity, planet generation, terrain, persistence and general editor systems remain unimplemented.

The app supplies the bootstrap panel through a per-frame UI callback; the renderer owns only UI input and GPU integration. Native events and UI types stay at this app/renderer boundary. An `Arc<Window>` safely keeps the surface's native handle alive without leaks or shared mutable domain state. Presentation notifies `winit` before submitting the frame to the compositor.

The smoke app uses bounded continuous redraw to exercise presentation without adding an editor repaint scheduler. Its 16 ms deadline remains stable across input wakeups, skips catch-up frames, and stops timer wakeups while minimized, zero-sized, occluded, or suspended. FIFO presentation also provides GPU/display pacing. Suspension releases the renderer before the window; resume recreates them. This bootstrap cadence is not a future simulation timestep.
