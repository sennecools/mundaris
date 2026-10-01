# Mundaris — Phase 2: Celestial Model & Simulation Time

> **Status:** implemented; full visual, Linux and current-change remote CI acceptance remain open — see [Phase 2 validation](docs/phase-2-validation.md).
>
> **Targets:** stable Rust, Rust 2024, native Windows x86-64 and Linux x86-64
>
> **Prerequisite:** Phase 1 — Coordinates, Reference Frames & Precision
>
> **Architecture:** `MUNDARIS_ENGINE_DESIGN.md`, `MUNDARIS_PHASE_1_REFERENCE_FRAMES.md`, the existing six-crate workspace, [ADR 0002](docs/adr/0002-reference-frames-and-precision.md), and [ADR 0003](docs/adr/0003-celestial-domain-and-time.md).

---

## 1. Outcome and scope

Phase 2 introduces Mundaris' first real universe-domain model without introducing celestial mechanics yet.

At the end of this phase Mundaris can represent a coherent star-system-like collection of celestial bodies, assign each body physical and rotational properties, advance an explicit simulation-time value, edit body properties safely, publish body state into the Phase 1 reference-frame system, and inspect the result through a deterministic validation application.

The central architectural rule is:

> **Celestial domain state is authoritative. Reference frames are a derived runtime projection of that state.**

`FrameTree` must never become the database of the universe. A celestial body is not identified by a frame, and changing or rebuilding a frame projection must not change body identity or authoritative physical state.

Phase 2 exists to establish the domain/time boundary that Phase 3 gravity and orbital integration will depend on.

### 1.1 Included

Phase 2 includes:

- a generic `CelestialBody` domain model;
- body identity and append-only lifetime rules for this phase;
- validated physical properties such as mass and reference radius;
- authoritative translational and rotational state in one finite system coordinate basis;
- an explicit simulation instant and time-control model;
- body property and state mutation with transactional validation;
- a derived body-to-reference-frame projection using Phase 1 `FrameTree`;
- separate translating and rotating frames for each celestial body;
- coherent publication of body state and frame state at one simulation instant;
- deterministic analytic/scripted motion fixtures used only for validation;
- editor/debug UI for body inspection and property edits;
- headless tests and focused CPU benchmarks;
- an ADR recording the body/time/frame ownership decision.

### 1.2 Explicitly excluded

Do **not** implement in Phase 2:

- Newtonian gravity;
- orbital mechanics;
- N-body integration;
- Kepler solvers;
- barycentre solving;
- collision detection;
- body-body collision response;
- tidal forces;
- relativistic corrections;
- physically simulated spin evolution;
- terrain;
- LOD;
- procedural planet generation;
- atmosphere;
- water;
- climate;
- tectonics;
- vegetation;
- persistent world serialization;
- cross-star-system addressing;
- automatic celestial classification;
- an ECS;
- a general scene graph;
- a job system;
- a render graph;
- a general editor framework;
- spacecraft;
- gameplay.

Phase 2 may animate bodies with deterministic prescribed functions for validation, but those functions are **fixtures**, not orbital simulation.

---

## 2. Phase 1 assumptions that Phase 2 must preserve

Phase 2 is built on the following Phase 1 contracts and must not weaken them:

1. Physical transforms and authoritative positions use checked `f64` math.
2. Nearby rendering uses source-centered observer subtraction before `f32` narrowing.
3. Reference frames are generic rigid Cartesian frames with explicit parent/child semantics.
4. Runtime `FrameId` values are not persistent world identity.
5. Attached content remains local to its frame and is not rewritten when ancestors move.
6. A rotating frame contributes rotational motion to descendants.
7. Orbital/translation ancestry must not accidentally inherit body spin.
8. Renderer owns the `f64 -> f32` narrowing boundary.
9. Evaluation is coherent at one explicit sample instant.
10. The renderer consumes read-only derived state and does not own simulation state.

The Phase 2 design intentionally uses a **translating body anchor frame** and a **rotating body-fixed frame** as separate nodes. This is the concrete domain application of Phase 1's warning that a moon must not accidentally inherit the daily spin of its primary body.

---

## 3. Architectural ownership

Phase 2 establishes a sharper responsibility split between the existing crates.

### `mundaris_math`

Owns generic mathematics only:

- coordinate and velocity value types;
- the checked `SimulationInstant` value (resolved option A in Section 22);
- rotations and rigid transforms;
- `FrameTree`;
- frame evaluation and kinematic conversion.

It must not know about stars, planets, moons, masses, radii, body names, simulation clocks, or celestial-system semantics.

### `mundaris_world`

Owns **what exists** and the current authoritative celestial state:

- `CelestialSystem`;
- `BodyId`;
- `CelestialBody`;
- validated body physical properties;
- current body kinematic state;
- current celestial sample instant;
- body insertion and mutation;
- the runtime association between domain bodies and the derived frame projection.

`mundaris_world` may depend on `mundaris_math`.

### `mundaris_simulation`

Owns **how simulation time is requested/advanced and, later, how state evolves**.

In Phase 2 it owns:

- `PlaybackRate` and requested-time control; `SimulationInstant` is re-exported from math, not a simulation-owned value;
- time-rate / pause semantics;
- a small simulation-time controller;
- deterministic prescribed-motion helpers used by validation only if they are reusable simulation fixtures rather than app-only UI logic.

It does **not** own gravity or an integrator yet.

`mundaris_simulation` may depend on `mundaris_math` and `mundaris_world` only when a concrete Phase 2 implementation requires it. Avoid cyclic crate dependencies.

### `mundaris_renderer`

Remains domain-agnostic.

It consumes Phase 1 frame/view data and generic debug primitives. It must not depend on `mundaris_world` or `mundaris_simulation` merely to draw celestial validation markers.

### `mundaris_app`

Owns orchestration and temporary session/editor behavior:

- validation fixture setup;
- UI controls;
- user-selected body;
- time-control interaction;
- invoking world mutations;
- invoking frame publication;
- preparing generic debug primitives/labels for renderer/UI.

The app must not become the authoritative celestial model or duplicate transform mathematics.

### `mundaris_core`

No new responsibility is required in Phase 2 merely to make it non-empty. Do not move IDs, clocks, logging, or configuration here without a concrete cross-domain need.

---

## 4. The authoritative celestial model

### 4.1 `CelestialSystem`

A `CelestialSystem` is one finite working domain containing zero or more celestial bodies expressed in one canonical system Cartesian basis.

Phase 2 does **not** claim that one `CelestialSystem` represents the entire universe.

A system contains:

- a runtime namespace / identity used to validate `BodyId` ownership;
- an authoritative `SimulationInstant` at which all body states are valid;
- append-only body records;
- a checked revision counter;
- any compact indexing metadata required for validated lookup;
- no renderer state;
- no GPU state;
- no persistent-world UUID scheme.

All body kinematic state in a system is sampled at the same `SimulationInstant`.

Mixed-time body state is invalid.

### 4.2 System coordinates

For Phase 2, the canonical system basis is:

- right-handed;
- Cartesian;
- metres;
- `f64`;
- finite;
- aligned with the Phase 1 system-root frame when projected to the frame tree.

The system basis is a finite working coordinate system, not a universal absolute coordinate promise.

A body's authoritative center position is stored directly in this system basis. It is **not** stored as a `FramePosition` using a runtime `FrameId`, because frame IDs are derived runtime infrastructure rather than world identity.

### 4.3 No domain hierarchy encoded through transform ancestry

A celestial system must not assume that body relationships form a rigid transform tree.

In particular:

- a moon being semantically associated with a planet does not mean its translating frame must be a child of the planet's rotating frame;
- future gravity may update every body's system-space position independently;
- barycentric systems may have no single physically privileged rigid parent body;
- a body's display classification must not control mathematical ancestry.

Phase 2 therefore keeps authoritative body center position and velocity in the common system basis.

Future orbital relationships, primaries, barycentres, and gravitational bindings are Phase 3+ domain concepts and must not be guessed here.

---

## 5. Body identity and lifetime

### 5.1 `BodyId`

Use an opaque runtime `BodyId`, analogous in spirit to `FrameId` but owned by `mundaris_world`.

Recommended semantics:

```rust
pub struct BodyId {
    namespace: NonZeroU64,
    index: u32,
}
```

Fields remain private.

Requirements:

- `Copy`, `Eq`, `Hash`, debug-printable;
- body zero is **not** special;
- IDs are validated against system namespace and current body count;
- IDs remain stable while the body remains in the system;
- Phase 2 body storage is append-only;
- no body deletion;
- no slot reuse;
- no generational arena;
- no persistent serialization contract;
- no name-based identity.

If body deletion becomes necessary later, design it deliberately rather than silently reusing indices.

### 5.2 Names

A body may have a human-readable name.

The name:

- is editor/display metadata;
- is not required to be unique;
- is not an ID;
- may be edited without changing `BodyId`;
- must not determine procedural seeds or persisted identity in future systems.

Reject invalid strings only for concrete reasons such as an explicit byte-length cap or invalid internal invariants. Do not invent a restrictive naming grammar.

A reasonable Phase 2 cap such as 1-128 UTF-8 bytes is acceptable if documented and tested.

### 5.3 Classification

Do **not** make `Star`, `Planet`, and `Moon` a required physics enum in Phase 2.

Mundaris eventually needs bodies with unusual properties that may not fit a rigid taxonomy. Physical behavior should follow physical properties, not labels.

If the validation UI benefits from a descriptive category, use optional non-authoritative metadata and document that it has no effect on simulation or frame ancestry.

---

## 6. Celestial body data model

A celestial body is split into relatively slow-changing properties and current kinematic state.

### 6.1 `BodyProperties`

Phase 2 requires at least:

#### Mass

- canonical unit: kilograms;
- `f64`;
- finite;
- strictly greater than zero for `CelestialBody`;
- stored authoritatively;
- no derived surface gravity stored;
- no cached gravitational parameter required yet.

Phase 3 may derive `mu = G * mass` in the gravity subsystem.

Do not embed a gravitational constant in the body model merely because mass exists.

#### Reference radius

Store one positive finite **reference radius** in metres.

This value means:

> a coarse characteristic body size useful for inspection, future surface anchoring, and early visualization.

It is **not** a commitment to:

- a perfect sphere;
- a heightmap terrain representation;
- a spherical surface topology;
- a fixed ocean level;
- an equatorial radius model;
- oblateness rules.

Terrain and actual rendered shape remain future systems.

Use a name such as `reference_radius_m` rather than `planet_radius`.

#### Optional descriptive metadata

A body may contain lightweight user-facing metadata if it is genuinely used by the Phase 2 app. Avoid speculative fields such as atmosphere composition, luminosity, temperature, albedo, density class, biome data, or tectonic parameters until the phase that needs them.

Density is derived from mass/reference radius only if explicitly requested for diagnostics; do not store it as competing authoritative state.

### 6.2 `BodyState`

The authoritative current kinematic state contains:

- center position in system coordinates;
- center linear velocity in system axes;
- body-fixed orientation relative to system axes;
- body angular velocity relative to system axes;
- all values valid at `CelestialSystem.sample_time`.

Recommended conceptual form:

```rust
pub struct BodyState {
    center_in_system: LocalPosition,
    center_velocity_in_system: LinearVelocity3,
    body_to_system: UnitRotation,
    angular_velocity_in_system: AngularVelocity3,
}
```

The exact API may differ, but these semantics are required.

### 6.3 Why orientation is authoritative instead of axial tilt / period

Do not store axial tilt and rotation period as the core physical state.

Those are useful authoring/UI representations, but the engine foundation should support arbitrary orientations and future spin evolution.

The authoritative state is:

- a unit quaternion for orientation;
- angular velocity as an axial vector.

The editor may later expose conveniences such as:

- spin-axis direction;
- day length;
- axial tilt;
- rotation phase.

Those must convert explicitly into the authoritative representation.

### 6.4 State validity

All body state constructors/mutations reject:

- NaN;
- infinity;
- invalid unit rotations;
- overflow produced by checked operations.

Zero linear velocity and zero angular velocity are valid.

Do not clamp invalid physical state to a plausible value.

---

## 7. Mutation and authoritative revisions

### 7.1 Transactional mutation

Mutating authoritative body properties or state must be transactional.

A failed edit leaves:

- the body record;
- system sample time;
- system revision;
- body/frame associations

unchanged.

### 7.2 Revision semantics

`CelestialSystem` owns a checked monotonically increasing runtime revision.

Increment it on successful authoritative changes such as:

- body insertion;
- body property edit;
- body state edit;
- coherent state publication to a new sample instant.

Do not increment for:

- reads;
- renderer preparation;
- derived frame publication alone when authoritative state is unchanged;
- UI selection changes.

The revision is runtime coherence metadata, not a persistent world version.

### 7.3 Batch state publication

Future N-body simulation will update many or all bodies together.

Phase 2 therefore needs an efficient coherent batch API rather than encouraging one-body-at-a-time time advancement.

A batch state update must:

1. identify the target `SimulationInstant`;
2. validate all referenced body IDs;
3. reject duplicates;
4. validate every proposed state before mutation;
5. commit all states and the sample instant atomically;
6. increment the system revision once.

Do not require the public API to permanently expose one specific ordering optimization unless benchmark evidence justifies it.

The implementation may use sorted internal buffers, dense body-index iteration, or another no-surprise approach.

The invariant is **coherent transactional publication**, not "callers forever sort by body index".

The implementation requires a state for every body when the target instant differs from the current instant; same-instant editor batches may update a subset. It accepts arbitrary caller ordering and reuses dense duplicate flags without hot allocation. `BodyState` inputs are already numerically checked by their private math value types, so publication validates identity/completeness/revision before committing rather than revalidating raw scalars. These concrete choices prevent accidental mixed-time retention and are recorded in ADR 0003.

### 7.4 Property edits versus state edits

Property edits (mass, radius, name) do not implicitly change translational or rotational state.

State edits do not implicitly change mass/radius.

Phase 3 will define what a gravity solver does if mass changes during a running simulation.

---

## 8. Simulation time

Time is foundational but Phase 2 must not prematurely design the gravity integrator.

### 8.1 `SimulationInstant`

Introduce a checked domain type representing seconds relative to a caller-defined working epoch.

Conceptually:

```rust
pub struct SimulationInstant {
    seconds_since_epoch: f64,
}
```

Requirements:

- finite `f64`;
- seconds;
- may be negative;
- may move forward or backward in editor/analytic fixtures;
- does not imply UTC, TAI, Julian Date, calendar time, or geological epoch representation;
- no claim of nanosecond precision at arbitrarily huge epoch magnitudes.

Phase 2 deliberately keeps the working epoch local to the current system/session.

Cross-system astronomical dating can be designed later.

### 8.2 Authoritative time belongs to state

A `CelestialSystem` has exactly one authoritative sample instant.

Bodies do not individually carry timestamps.

The system's body states are all valid at that same instant.

The Phase 1 frame projection published from the system must use exactly that same sample time.

### 8.3 Playback/time control versus state evolution

Separate two ideas:

1. **What simulation time does the user want to observe/advance toward?**
2. **How does physical state become valid at that time?**

Phase 2 implements the first and only a deterministic analytic fixture for the second.

Do not create an API where changing a clock's number silently asserts that arbitrary world state has evolved correctly.

Recommended conceptual flow:

```text
wall-clock delta / UI seek
        ↓
time controller computes requested SimulationInstant
        ↓
state producer advances or samples authoritative CelestialSystem
        ↓
CelestialSystem commits coherent state at that instant
        ↓
frame projection publishes same instant
        ↓
renderer evaluates read-only state
```

In Phase 2, the state producer is the analytic validation fixture.

In Phase 3, it becomes the gravity/orbit simulation runner.

### 8.4 Time rate

Provide a checked playback/simulation rate for the validation app and future orchestration.

Requirements:

- finite `f64` multiplier;
- `1.0` means one simulation second per real second;
- values greater than `1` accelerate requested time;
- values between `0` and `1` slow it;
- negative values may request reverse movement for analytic/editor-compatible producers;
- pause is represented explicitly rather than relying solely on rate `0`;
- no arbitrary engine-wide maximum is baked into the math type.

The UI may provide practical presets such as:

```text
-100x  -10x  -1x  pause  1x  10x  100x  1000x
```

but these are UI policy, not engine limits.

### 8.5 Wall-clock deltas

Use monotonic host durations for interactive playback.

Do not feed wall-clock timestamps into celestial state.

A long UI stall/resume must not cause an accidental enormous physical integration step in future phases. Therefore the Phase 2 time-control API should expose the requested time change rather than quietly integrating world state itself.

Phase 3 will decide fixed-step accumulation, catch-up limits, and integration policies.

### 8.6 Seeking

Seeking is supported in Phase 2's analytic validation because state is a deterministic function of requested time.

This does **not** promise that future integrated simulations can jump to arbitrary times for free.

When gravity integration arrives, seeking may require:

- reset + deterministic replay;
- snapshots/checkpoints;
- stored ephemerides;
- analytic solutions for limited cases;
- a different editor mode.

Do not lock that policy now.

---

## 9. Celestial-to-frame projection

Phase 2 creates a derived runtime bridge from authoritative bodies to Phase 1 frames.

### 9.1 Ownership

The projection belongs to the world/domain layer, not the renderer and not the math crate.

Recommended conceptual type:

```rust
pub struct CelestialFrameProjection {
    tree: FrameTree,
    body_frames: Vec<BodyFrames>,
    published_system_revision: u64,
}

pub struct BodyFrames {
    translating: FrameId,
    body_fixed: FrameId,
}
```

Exact field/API choices may differ.

The projection is disposable and reconstructable from authoritative system state.

### 9.2 Frame topology per body

For every body:

```text
system root
└── body translating anchor
    └── body-fixed rotating frame
```

#### Translating anchor

Represents body-center translation only.

Required state:

- translation = authoritative body center in system coordinates;
- rotation = identity;
- origin velocity = authoritative body center velocity in system axes;
- angular velocity = zero.

#### Body-fixed frame

Represents body orientation/spin relative to the translating anchor.

Required state:

- translation = zero;
- rotation = authoritative `body_to_system` orientation, because the anchor axes remain system-aligned;
- origin velocity = zero;
- angular velocity = authoritative body angular velocity expressed in the anchor/system basis.

This topology guarantees that body spin does not rotate its own center translation or unrelated bodies.

### 9.3 No moon-under-spinning-planet default

A moon-like body gets its **own** translating anchor under the system root in Phase 2.

Do not parent it beneath the rotating frame of another body.

Future orbital relationships may create other useful non-rotating reference frames, but they must be designed from actual simulation requirements.

### 9.4 Frame association is derived identity

A body record must not store `FrameId` as authoritative persistent data.

The projection maintains runtime `BodyId -> BodyFrames` association.

Rebuilding the projection may produce different `FrameId` values while the same `BodyId` and physical state remain.

Terrain/generation/persistence IDs must never derive from these runtime frame IDs.

### 9.5 Publication

Publishing the world to frames must:

1. verify the projection belongs to the same system namespace/body topology;
2. ensure every body has the expected two frames;
3. publish all changed frame states at exactly `CelestialSystem.sample_time`;
4. use one coherent frame-tree update batch where practical;
5. set/record the world revision represented by the projection;
6. not mutate authoritative body state;
7. not touch renderer state.

If publication fails, authoritative celestial state remains valid. Do not partially claim a newer world revision in the projection.

### 9.6 Initial construction and append

When a projection is first built:

- create the Phase 1 root;
- append each body's translating and fixed frames;
- publish the authoritative sample instant;
- retain dense associations in body-index order.

If Phase 2 allows a body to be appended after projection creation, append exactly two frames and preserve all existing body/frame mappings.

Body deletion remains deferred.

---

## 10. Celestial system snapshots and borrowing

Phase 2 should preserve coherent read access without cloning the entire universe each frame.

A lightweight immutable system evaluation/view may be useful if it provides concrete value similar to Phase 1 `FrameEvaluation`.

However, do **not** create a parallel elaborate snapshot framework merely for symmetry.

Minimum requirement:

- rendering/app inspection cannot observe half-committed body state;
- batch mutation is transactional;
- frame publication occurs only after successful authoritative commit;
- renderer receives the frame evaluation after publication.

If ordinary `&CelestialSystem` borrowing already guarantees this in the single-threaded Phase 2 loop, prefer that simplicity.

Concurrent snapshots, interpolation buffers, and lock-free simulation/render handoff are future work.

---

## 11. Derived quantities

Phase 2 may expose cheap deterministic derived values when useful for diagnostics, but must not create competing authoritative state.

Examples:

### Mean density

If requested:

```text
rho = mass / ((4/3) * pi * reference_radius^3)
```

Label it clearly as density implied by the **reference sphere**, not actual internal structure.

### Angular speed / nominal rotation period

For nonzero angular velocity:

```text
angular_speed = |omega|
period = 2*pi / angular_speed
```

Zero angular velocity has no finite rotation period and should return `None` rather than infinity.

### Surface gravity

Do **not** make surface gravity part of Phase 2's core model.

It depends on a gravitational constant and spherical approximation and belongs naturally with gravity/physics diagnostics in Phase 3. If a UI convenience is added early, label it explicitly as a derived spherical approximation and keep the formula outside authoritative body storage.

---

## 12. Validation fixture

Phase 2 needs an inspectable validation mode that proves domain/time/frame ownership, not orbital physics.

Use:

```text
cargo run -p mundaris_app -- --celestial-model
```

The existing Phase 1 `--reference-frames` mode remains intact.

### 12.1 Fixture bodies

Create three generic named bodies with Earth/Sun/Moon-scale magnitudes but do not present the motion as a real solar-system simulation.

Suggested values:

#### Body A — `Solace`

```text
mass               1.98847e30 kg
reference radius   6.957e8 m
center              (0, 0, 0) m
center velocity     (0, 0, 0) m/s
orientation         identity
angular velocity    small +Y spin
```

#### Body B — `Aurelia`

```text
mass               5.9722e24 kg
reference radius   6.371e6 m
center at t=0       (1.5e11, 0, 0) m
base velocity       (0, 30_000, 0) m/s
orientation         arbitrary documented axial orientation
angular velocity    exaggerated but finite demo spin about its authored axis
```

#### Body C — `Luma`

```text
mass               7.342e22 kg
reference radius   1.7374e6 m
center at t=0       (1.5e11 + 384_000_000, 0, 0) m
base velocity       (0, 31_000, 0) m/s
orientation         independent
angular velocity    independent
```

Names are placeholders and have no classification semantics.

### 12.2 Prescribed validation motion

Use analytic motion that is intentionally **not an orbit**.

For bounded validation time `t` (for example `-600..600 s`):

```text
Solace center = constant

Aurelia center =
    initial_center
    + base_velocity * t
    + (0, 2.0e7 * sin(0.001*t), 0)

Aurelia center velocity = analytic derivative

Aurelia orientation = q_initial * axis_angle(spin_axis, spin_rate * t)
Aurelia angular velocity = analytic constant axial vector in system axes

Luma center =
    initial_center
    + luma_velocity * t
    + (0, 5.0e6 * sin(0.0017*t + 0.4), 3.0e6 * sin(0.0011*t))

Luma center velocity = analytic derivative

Luma orientation = independent analytic spin
```

The fixture exists to prove:

- shared system time;
- independent body translation;
- independent body spin;
- moon-like body state does not inherit planet-like spin;
- frame projection reproduces authoritative state;
- forward/reverse/seek time controls are deterministic.

Do not draw an orbit ellipse or call the motion orbital.

### 12.3 UI

Provide a compact engineering/editor panel showing:

- current requested time;
- current authoritative system sample time;
- playback rate;
- pause/resume;
- forward/reverse presets;
- seek/reset controls;
- system revision;
- published frame-projection revision;
- body count;
- selected body;
- selected `BodyId` debug display;
- body name;
- mass in kg;
- reference radius in metres/km;
- center position;
- center velocity;
- orientation diagnostics;
- angular velocity;
- translating/body-fixed frame IDs as **runtime debug information only**.

Allow safe editing of at least:

- body name;
- mass;
- reference radius;

State editing may be provided behind an explicit debug section if useful, but property editing is sufficient to prove mutation separation.

Invalid edits show errors and leave authoritative state unchanged.

### 12.4 Visual inspection

Use only existing generic rendering/debug facilities.

Possible visualization:

- axes at a currently focused body's body-fixed frame;
- small local debug markers near the focused center;
- UI bearing/distance markers for other bodies;
- textual system-space position/velocity diagnostics.

Do not implement full-size star/planet spheres or a system-scale depth solution in Phase 2.

The validation purpose is state/frame/time correctness, not celestial rendering.

### 12.5 Focus/re-expression

Allow selecting a body and placing/re-expressing the observer relative to its translating or body-fixed frame using Phase 1 APIs.

Changing the selected body must not alter authoritative body state.

When focusing `Aurelia`, advancing time should show:

- translating-frame movement according to body center state;
- body-fixed axes spinning according to body orientation;
- nearby body-fixed debug geometry remaining stable for a co-moving observer;
- `Luma` remaining independent of `Aurelia` spin.

---

## 13. Time-control behavior in the validation app

### 13.1 Playback

The app owns wall-clock interaction and asks the simulation time controller for a requested new `SimulationInstant`.

For the analytic fixture:

1. calculate requested instant;
2. analytically sample all body states at that instant;
3. transactionally publish them into `CelestialSystem`;
4. publish the resulting system to `CelestialFrameProjection`;
5. evaluate/render.

Never update the frame tree first and then infer body state from it.

### 13.2 Pause

Pause freezes requested simulation time.

UI/redraw may continue.

Body state and frame state remain valid at the frozen instant.

### 13.3 Reverse

Negative playback rate is valid for the analytic fixture.

The state at a given explicit instant must be identical regardless of whether that instant was reached by:

- forward playback;
- reverse playback;
- direct seek;
- reset and replay.

This determinism is a fixture guarantee, not a promise about future chaotic N-body replay without explicit deterministic policies.

### 13.4 Reset

Reset returns:

- requested time;
- authoritative system time;
- body state;
- projection state;
- selection/focus only if documented

into one deterministic baseline.

Do not leave the frame projection representing the previous time after reset.

---

## 14. Error handling

Use focused error enums at library boundaries and contextual application errors at orchestration boundaries.

Representative world errors may include:

- `WrongSystem`;
- `UnknownBody`;
- `CapacityExceeded`;
- `RevisionOverflow`;
- `DuplicateBodyUpdate`;
- `NonFiniteMass`;
- `NonPositiveMass`;
- `NonFiniteRadius`;
- `NonPositiveRadius`;
- `InvalidName`;
- `NonFiniteState`;
- `InvalidSimulationInstant`;
- `FrameProjectionMismatch`;
- `FramePublicationFailed`.

Representative time-control errors may include:

- `NonFiniteRate`;
- `NonFiniteInstant`;
- `TimeArithmeticOverflow`.

Rules:

- user/editor invalid input returns an error;
- programmer-invariant corruption may assert/panic with useful context;
- no silent clamping of mass/radius/time/state;
- no invalid value replaced with a default;
- no partial batch mutation;
- no partial projection revision advance.

---

## 15. Determinism

Phase 2 determinism expectations:

1. Identical explicit body data and explicit simulation instant produce identical domain values on the same build/target.
2. Analytic validation state is a pure function of explicit time and fixture constants.
3. Seeking to time `t` yields the same body state as forward/reverse playback arriving at `t`, within the defined floating-point tolerances.
4. Frame projection of identical authoritative body state produces numerically equivalent Phase 1 frame state.
5. Body IDs/runtime frame IDs need not reproduce across sessions.
6. Names do not affect physical state.
7. UI frame rate must not affect analytic body state.
8. No RNG is introduced by Phase 2 domain/time code.
9. No hash-map iteration order may affect physical state.
10. Cross-platform bit identity is not promised; Windows/Linux must satisfy the same numerical tolerance tests.

Future gravity determinism, procedural generation determinism, and save compatibility remain separate contracts.

---

## 16. Performance design

Phase 2 should remain simple and data-oriented without speculative optimization.

### 16.1 Expected complexity

| Operation | Expected behavior |
| --- | --- |
| Body lookup by valid `BodyId` | `O(1)` |
| Body insertion | amortized `O(1)` |
| Body property edit | `O(1)` |
| Single body-state edit | `O(1)` |
| Batch body-state commit | `O(U)` for `U` updated bodies |
| Full system state commit | `O(B)` for `B` bodies |
| Initial frame projection build | `O(B)` |
| Frame projection state publish | `O(B)` or `O(U)` depending on chosen implementation |
| Body -> frame association | `O(1)` dense lookup |
| Time-control update | `O(1)` |

No body mutation should iterate terrain/vegetation/render objects because those systems do not exist and, later, must remain body-local.

### 16.2 Memory layout

A simple contiguous `Vec<CelestialBody>` is appropriate initially.

Do not introduce SoA solely because future simulations may benefit from it.

Phase 3 N-body benchmarks can determine whether the solver wants separate dense mass/position/velocity arrays or a derived simulation working set.

The authoritative domain representation may remain ergonomic while simulation derives optimized working buffers.

### 16.3 No premature parallelism

Do not add Rayon, custom thread pools, tasks, locks, atomics, or async execution in Phase 2.

The purpose is to prove ownership and semantics first.

---

## 17. Benchmarks

Add focused Criterion benchmarks only where they measure likely future costs.

### 17.1 World benchmarks

Measure:

1. lookup/edit throughput for dense systems of 64, 1,024, and 16,384 bodies;
2. coherent full-state publication for 64, 1,024, and 4,096 bodies;
3. frame-projection initial build for 64, 1,024, and 4,096 bodies;
4. frame-projection republish for 64, 1,024, and 4,096 bodies;
5. body insertion with an already-live projection if supported.

Do not set arbitrary nanosecond pass/fail thresholds.

Record:

- CPU;
- OS;
- Rust version;
- build profile;
- body counts;
- whether projection updates are full or dirty-subset;
- allocations if easily inspectable with existing tooling.

### 17.2 Time benchmarks

A simulation-time controller is too trivial to justify standalone microbenchmarks unless profiling reveals an actual issue.

Do not manufacture benchmarks for every type merely to claim benchmark coverage.

---

## 18. Concrete automated test matrix

All ordinary automated tests remain headless.

### 18.1 Body property validation

Test:

- finite positive mass accepted;
- zero/negative mass rejected;
- NaN/infinity mass rejected;
- finite positive radius accepted;
- zero/negative radius rejected;
- NaN/infinity radius rejected;
- valid UTF-8 names accepted within documented limits;
- invalid over-limit name rejected transactionally if a cap is implemented.

### 18.2 Body identity

Test:

- insertion returns unique `BodyId` values;
- existing IDs remain valid after later insertions;
- IDs from another system namespace fail;
- out-of-range index fails;
- name edits do not change identity;
- no body is special because it is index zero.

### 18.3 Body mutation transactionality

Test:

- valid property edit increments revision exactly once;
- invalid property edit changes no body values/revision;
- valid state edit increments revision;
- failed state edit leaves state unchanged;
- batch with one invalid body changes nothing;
- batch with duplicate body IDs changes nothing;
- batch state publication changes sample time only on successful commit.

### 18.4 Simulation instant

Test:

- finite positive/zero/negative values accepted;
- NaN/infinity rejected;
- explicit arithmetic overflow reported;
- unit is seconds and named APIs preserve that meaning.

### 18.5 Time-control behavior

Test:

- `1x` advances requested time by real delta;
- `10x` scales correctly;
- `0.1x` scales correctly;
- negative rate requests decreasing time;
- pause keeps requested time fixed;
- resume continues from frozen value;
- rate changes do not themselves jump time;
- reset/seek is explicit;
- huge finite inputs that overflow return errors rather than infinity.

### 18.6 Analytic fixture determinism

For a bounded set such as integer sample ticks over `-600..600 s`:

- direct sample at `t` equals forward-playback sample at `t` within tolerance;
- reverse-playback sample at `t` equals direct sample;
- reset/replay equals direct sample;
- sample order does not affect result.

### 18.7 Frame topology per body

For one body:

- translating anchor is root child;
- translating anchor rotation is identity;
- body-fixed frame is child of translating anchor;
- body-fixed translation is zero;
- body-fixed rotation equals authoritative orientation;
- translating motion equals authoritative center velocity;
- body-fixed angular velocity equals authoritative angular velocity;
- translating angular velocity is zero.

### 18.8 Multiple bodies do not inherit each other's spin

Construct two bodies A and B.

Change only A orientation/angular velocity.

Assert:

- A body-fixed frame changes;
- A translating center does not rotate around itself;
- B translating/body-fixed state is unchanged;
- B frame transform is numerically unchanged;
- B local debug point observed from system frame is unaffected by A spin.

This is a critical regression test.

### 18.9 Frame publication coherence

Test:

- projection sample time equals system sample time;
- projection records represented system revision;
- after successful body state commit + publish, frame state matches authoritative world state;
- property-only mass/name/radius edits do not spuriously alter frame transforms;
- publication failure does not claim the new system revision;
- rebuilding projection from the same world produces equivalent transforms even if runtime frame IDs differ.

### 18.10 Extreme coordinates

Use body center positions around:

- `0 m`;
- `6.371e6 m`;
- `1.5e11 m`;
- shared ancestry stress values already covered by Phase 1 where applicable.

Confirm the domain -> frame projection itself introduces no avoidable narrowing and keeps all physical values `f64`.

Nearby renderer precision continues to be validated by Phase 1 tests rather than duplicated unnecessarily.

### 18.11 Orientation/spin

Test:

- identity orientation;
- arbitrary unit quaternion;
- zero angular velocity;
- nonzero angular velocity;
- authored analytic spin at multiple positive/negative times;
- quaternion sign equivalence where relevant;
- no Euler-angle round trip is required by the core model.

### 18.12 Derived diagnostics

If density/rotation period helpers are implemented:

- compare against independently calculated values;
- zero angular speed returns `None` period;
- no derived helper mutates authoritative state.

---

## 19. Numerical tolerances

Use Phase 1 math types and their established tolerance philosophy.

Phase 2 introduces no reason to loosen local/frame precision budgets.

For domain -> frame projection:

- translations copied into Phase 1 `f64` frame state should be component-identical when no arithmetic is required;
- velocities likewise should not be narrowed;
- orientation comparison uses rotated basis/dot-equivalence rather than raw quaternion sign;
- analytic fixture comparisons use scale-appropriate tolerances, generally `1e-9` to `1e-6` for local/velocity formulas and looser only when Phase 1 astronomical root representation itself requires it.

Do not create extremely tight tolerances merely to boast about precision.

Document each tolerance by the magnitude and arithmetic involved.

---

## 20. Representative public API sketches

These are semantic sketches, not mandatory exact signatures.

### 20.1 World domain

```rust
pub struct BodyId { /* opaque namespace + index */ }

pub struct BodyProperties {
    /* private */
}

impl BodyProperties {
    pub fn new(
        mass_kg: f64,
        reference_radius_m: f64,
    ) -> Result<Self, BodyPropertyError>;

    pub fn mass_kg(&self) -> f64;
    pub fn reference_radius_m(&self) -> f64;
}

pub struct BodyState {
    /* private */
}

impl BodyState {
    pub fn new(
        center_in_system: LocalPosition,
        center_velocity_in_system: LinearVelocity3,
        body_to_system: UnitRotation,
        angular_velocity_in_system: AngularVelocity3,
    ) -> Result<Self, BodyStateError>;
}

pub struct CelestialBody {
    /* id is owned by containing system */
}

impl CelestialBody {
    pub fn name(&self) -> &str;
    pub fn properties(&self) -> &BodyProperties;
    pub fn state(&self) -> &BodyState;
}

pub struct CelestialSystem {
    /* namespace, sample time, revision, contiguous bodies */
}

impl CelestialSystem {
    pub fn new(namespace: NonZeroU64, initial_time: SimulationInstant) -> Self;

    pub fn insert_body(
        &mut self,
        name: impl Into<String>,
        properties: BodyProperties,
        state: BodyState,
    ) -> Result<BodyId, CelestialSystemError>;

    pub fn body(&self, id: BodyId) -> Result<&CelestialBody, CelestialSystemError>;

    pub fn edit_properties(
        &mut self,
        id: BodyId,
        properties: BodyProperties,
    ) -> Result<(), CelestialSystemError>;

    pub fn update_states(
        &mut self,
        sample_time: SimulationInstant,
        updates: &[BodyStateUpdate],
    ) -> Result<(), CelestialSystemError>;

    pub fn sample_time(&self) -> SimulationInstant;
    pub fn revision(&self) -> u64;
}
```

### 20.2 Time

```rust
pub struct SimulationInstant { /* finite seconds */ }
pub struct PlaybackRate { /* finite multiplier */ }

pub struct TimeController {
    requested: SimulationInstant,
    rate: PlaybackRate,
    paused: bool,
}

impl TimeController {
    pub fn requested_time(&self) -> SimulationInstant;
    pub fn set_rate(&mut self, rate: PlaybackRate);
    pub fn set_paused(&mut self, paused: bool);
    pub fn seek(&mut self, target: SimulationInstant);

    pub fn advance_wall_time(
        &mut self,
        elapsed: Duration,
    ) -> Result<SimulationInstant, TimeError>;
}
```

The controller changes **requested time only**. It does not mutate `CelestialSystem`.

### 20.3 Frame projection

```rust
pub struct BodyFrames {
    translating: FrameId,
    body_fixed: FrameId,
}

pub struct CelestialFrameProjection {
    /* FrameTree + dense body/frame association + represented revision */
}

impl CelestialFrameProjection {
    pub fn build(system: &CelestialSystem) -> Result<Self, FrameProjectionError>;

    pub fn publish(
        &mut self,
        system: &CelestialSystem,
    ) -> Result<(), FrameProjectionError>;

    pub fn frames_for(
        &self,
        id: BodyId,
    ) -> Result<BodyFrames, FrameProjectionError>;

    pub fn tree(&self) -> &FrameTree;
    pub fn represented_revision(&self) -> u64;
}
```

Do not expose mutation of the projection's `FrameTree` to arbitrary callers unless needed for a concrete derived local frame feature. Phase 2 should keep projection ownership disciplined.

---

## 21. File-level implementation plan

This plan records the Phase 2 implementation responsibilities. Concrete API choices and evidence are in ADR 0003 and the validation record; unchecked Definition of Done criteria remain open.

### `crates/world`

#### `crates/world/Cargo.toml`

Add only dependencies actually used:

- `mundaris_math`;
- `thiserror` if consistent with project error policy.

If `SimulationInstant` must be shared without creating a world<->simulation cycle, prefer placing the minimal time value type in the lower-level crate that semantically owns it, then keep playback control in `simulation`. Do **not** solve this by moving unrelated systems into `core` automatically.

The implemented direction is world -> math, simulation -> math; simulation may add world when gravity needs it. World does not depend on simulation.

#### `crates/world/src/body.rs`

Own:

- `BodyId` if identity is colocated here;
- `BodyProperties`;
- `BodyState`;
- `CelestialBody`;
- validation errors.

#### `crates/world/src/system.rs`

Own:

- `CelestialSystem`;
- runtime namespace;
- sample instant;
- revision;
- append-only storage;
- transactional insertion/property/state mutation.

#### `crates/world/src/frame_projection.rs`

Own:

- `BodyFrames`;
- `CelestialFrameProjection`;
- domain -> Phase 1 frame conversion/publication;
- projection revision/topology checks.

#### `crates/world/src/lib.rs`

Focused exports and crate-level ownership documentation.

#### `crates/world/tests/celestial_system.rs`

Public API tests for identity, property validation, transactionality, revisions, coherent time.

#### `crates/world/tests/frame_projection.rs`

Body -> frame topology, publication, independence-of-spin, rebuild equivalence.

#### `crates/world/benches/celestial_system.rs`

State batch and lookup/edit benchmarks.

#### `crates/world/benches/frame_projection.rs`

Projection build/publish benchmarks.

### `crates/simulation`

#### `crates/simulation/src/time.rs`

Own:

- playback-only types; the instant value is implemented in `crates/math/src/time.rs`;
- `PlaybackRate`;
- `TimeController`;
- checked wall-time scaling.

Keep fixed-step integration policy out of this file.

#### `crates/simulation/src/lib.rs`

Exports and documentation that gravity/integration are not implemented yet.

#### `crates/simulation/tests/time.rs`

Pause/rate/reverse/seek/overflow behavior.

No simulation benchmark is required merely for the clock.

### `crates/app`

#### `crates/app/src/celestial_model.rs`

Own:

- analytic fixture constants;
- fixture state sampling;
- selected body;
- validation UI;
- body edit commands;
- focus/re-expression orchestration;
- debug markers/diagnostics.

It must not reimplement frame math or body validation.

#### `crates/app/src/main.rs`

Add `--celestial-model` mode selection and route the update loop without breaking `--reference-frames` or the ordinary smoke mode.

### `crates/renderer`

Prefer no new domain-specific module.

Only extend generic debug APIs if the celestial validation mode reveals a reusable need.

Do not add `mundaris_world` as a renderer dependency.

### Workspace/docs

Update as needed:

- root `Cargo.toml` / lockfile;
- `README.md`;
- `docs/architecture.md`;
- `docs/roadmap.md`;
- `docs/performance.md`;
- `docs/phase-2-validation.md`;
- `docs/adr/0003-celestial-domain-and-time.md`.

---

## 22. Dependency-direction decision for `SimulationInstant`

**Resolved:** option A is implemented in `crates/math/src/time.rs` and recorded in ADR 0003. The options below preserve the decision rationale; they are not an unresolved instruction to relocate the type.

We want:

```text
world state needs SimulationInstant
simulation algorithms/control operate on world
```

A naive layout can produce:

```text
world -> simulation
simulation -> world
```

which Rust crates cannot and should not form.

Use one of these two clean solutions:

### Preferred option A: low-level time value in `mundaris_math`

Place only the **dimensioned, validated instant value** in `mundaris_math` if the existing math crate's charter reasonably includes physical scalar/time primitives.

Then:

```text
math: SimulationInstant value only
world -> math
simulation -> math + world
```

Playback/time-control policy remains in `simulation`.

This is acceptable because an instant is a low-level physical quantity, not a simulation algorithm.

### Acceptable option B: world-owned sample instant type

Place `SimulationInstant` in `mundaris_world`, then let `simulation` depend on `world` and use that type.

This is acceptable if the project prefers time identity to be explicitly tied to world state.

### Do not

- make `world` depend on `simulation` while `simulation` also depends on `world`;
- move the entire clock into `core` simply to break a cycle;
- duplicate `SimulationInstant` types in world and simulation;
- use raw `f64` everywhere to avoid the design problem.

The implementation chose A. World and simulation re-export the same math-owned type; playback remains simulation-owned and no crate cycle exists.

---

## 23. Editing semantics and future world-builder use

Mundaris may eventually become a commercial world-building tool. Phase 2 should therefore distinguish **authoring operations** from **simulation evolution** even though both can modify state.

Examples:

### Authoring

- rename body;
- change mass;
- change reference radius;
- set orientation;
- place body center;
- set initial velocity;
- set initial angular velocity.

### Simulation

- gravity updates body velocity;
- integrator updates body position;
- future torque updates spin.

Phase 2 may use the same validated mutation foundations, but APIs/docs should preserve the conceptual distinction so future undo/redo, scenario reset, and initial-condition editing remain possible.

Do not implement a command history or undo stack yet.

---

## 24. Initial conditions versus current state

A future simulator/editor may need both:

- authored initial conditions;
- current evolved state.

Phase 2 does **not** need to duplicate every body into `initial` and `current` records yet.

The validation fixture can regenerate its baseline analytically.

However, avoid APIs that make future reset impossible. In particular:

- body identity should survive state reset;
- properties/state should be independently replaceable;
- current frame IDs must not be persisted as initial-condition identity.

Phase 3 should decide whether simulation sessions own an explicit initial-state snapshot.

---

## 25. No physical behavior from names or categories

This is a strict future-proofing rule.

Bad:

```rust
match body.kind {
    Star => apply_star_rules(),
    Planet => apply_planet_rules(),
    Moon => apply_moon_rules(),
}
```

for basic gravitational or frame behavior.

Preferred:

```text
mass + state + explicitly attached components/physical models
        ↓
behavior
```

Later rendering/editor layers may classify bodies for convenience, but physics must follow explicit properties and active models.

This lets Mundaris support:

- binary stars;
- rogue planets;
- double planets;
- captured moons;
- artificial bodies;
- tiny moons;
- unusually massive satellites;
- objects that do not fit a simple taxonomy.

---

## 26. No implied orbit from `primary`

Do not add a `primary_body: BodyId` field merely because the validation fixture resembles star/planet/moon.

An N-body system does not need a rigid primary hierarchy to evolve.

If the editor later wants semantic grouping such as "this moon belongs to that planet," add it as explicit metadata or a derived relationship with clear meaning.

It must not silently control frame ancestry or gravity.

---

## 27. Phase 3 readiness requirements

Phase 2 is successful only if Phase 3 can add gravity without redesigning the domain ownership model.

The gravity phase should be able to:

1. enumerate bodies densely;
2. read each body's mass;
3. read each body's system-space center position;
4. read each body's system-space center velocity;
5. compute accelerations in a simulation working buffer;
6. integrate to a new `SimulationInstant`;
7. transactionally publish all new body states;
8. publish those body states into the existing frame projection;
9. leave body-fixed/local attached content untouched by center translation;
10. independently update spin if/when rotational dynamics exist.

If Phase 2 implementation makes this awkward, treat that as a design problem before declaring completion.

---

## 28. Intentionally deferred decisions

The following remain open after Phase 2:

- gravitational constant representation/policy;
- N-body solver;
- integrator type;
- adaptive versus fixed simulation timestep;
- fixed-step accumulator/catch-up policy;
- collision handling;
- massive versus test-particle bodies;
- Barnes-Hut / fast multipole or other scaling optimizations;
- barycentric coordinates;
- orbital elements as authoritative or derived data;
- primary-body semantics;
- exact rewind/seek policy for integrated simulations;
- simulation snapshots/checkpoints;
- persistent body IDs;
- body deletion;
- cross-system transfer/addressing;
- full celestial rendering;
- star luminosity/spectrum;
- atmospheric properties;
- material composition;
- body shape/oblateness;
- terrain surface parameterization;
- surface gravity models;
- tidal locking;
- spin-orbit coupling;
- relativistic corrections.

Do not fill these gaps speculatively during implementation.

---

## 29. Implementation order

Implement Phase 2 in this order:

1. Resolve and document `SimulationInstant` crate ownership/dependency direction.
2. Add checked time value/playback control and tests.
3. Add `BodyId`, `BodyProperties`, `BodyState`, `CelestialBody` and tests.
4. Add `CelestialSystem`, revisioning, insertion, transactional state/property mutation and tests.
5. Add `CelestialFrameProjection` and body/frame topology tests.
6. Add analytic fixture and deterministic time sampling.
7. Add app validation UI/focus controls using existing generic renderer/debug features.
8. Add world/projection benchmarks.
9. Perform native validation.
10. Update documentation and ADR 0003.

Each step must leave the existing Phase 1 modes and tests passing.

Do not start Phase 3 gravity work in the same change set.

---

## 30. Definition of Done

Phase 2 is complete only when all applicable criteria have evidence.

### 30.1 Build and quality

- [ ] Stable Rust/Rust 2024.
- [ ] Existing six-package workspace remains intact unless a documented hard reason requires otherwise.
- [ ] `publish = false` remains set appropriately.
- [ ] No project-owned unsafe code is introduced.
- [ ] No unrelated dependency upgrades.
- [ ] `cargo build --locked --workspace` passes.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo check --locked --workspace --all-targets --all-features` passes.
- [ ] `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test --locked --workspace --all-features` passes.
- [ ] focused release tests for world/simulation/math pass.
- [ ] Rustdoc passes with warnings denied.
- [ ] `git diff --check` passes.

### 30.2 Domain correctness

- [ ] `CelestialSystem` owns authoritative body state and one coherent sample instant.
- [ ] `BodyId` is runtime opaque identity, not a name/frame ID/persistent ID.
- [ ] Bodies are append-only in Phase 2; existing IDs remain stable after insertion.
- [ ] Mass/reference radius validation is explicit and transactional.
- [ ] Current translational and rotational state is finite, checked, and `f64`.
- [ ] Body orientation uses a checked unit quaternion rather than Euler authoritative storage.
- [ ] System revision changes only for successful authoritative mutations.
- [ ] Batch body state publication is coherent and transactional.
- [ ] Names/categories do not alter physical behavior.

### 30.3 Time correctness

- [ ] `SimulationInstant` has explicit seconds semantics and rejects non-finite values.
- [ ] Time-control rate/pause/seek semantics are tested.
- [ ] Time controller requests a target instant but does not silently evolve world state.
- [ ] Analytic fixture state at time `t` is independent of route to `t`.
- [ ] Negative time/rate works for the analytic fixture without implying future integrators must support free rewind.
- [ ] Wall-clock time is not stored as celestial simulation state.

### 30.4 Frame projection

- [ ] Each body has a translating root child and rotating body-fixed child in the derived projection.
- [ ] Translating frame carries center position/linear velocity and no spin.
- [ ] Body-fixed frame carries orientation/angular velocity and no center translation.
- [ ] Moon-like/other bodies are not parented beneath another body's spinning frame by default.
- [ ] Projection sample time exactly equals authoritative system sample time.
- [ ] Projection records which system revision it represents.
- [ ] Rebuilding projection from identical body state yields numerically equivalent transforms.
- [ ] Property-only edits do not spuriously move frames.
- [ ] Frame publication failure does not mutate authoritative world state or falsely advance represented revision.
- [ ] Runtime frame IDs never become body/persistence/procedural identity.

### 30.5 Validation application

- [ ] `--celestial-model` launches without breaking existing modes.
- [ ] Three-body analytic fixture loads with documented physical magnitudes.
- [ ] Pause/resume works.
- [ ] Positive/negative playback rate works.
- [ ] Direct seek/reset works.
- [ ] Selecting/focusing bodies works through Phase 1 frame APIs.
- [ ] Focused body-fixed local geometry remains stable while body center translates/spins.
- [ ] Independent body does not inherit another body's spin.
- [ ] Property editor accepts valid mass/radius/name edits and rejects invalid edits transactionally.
- [ ] UI clearly distinguishes authoritative system time from requested playback time if they can differ.
- [ ] No UI/renderer code becomes authoritative physics state.

### 30.6 Performance

- [ ] Lookup/state publication/projection benchmarks exist for meaningful body counts.
- [ ] Benchmark results are recorded with hardware/OS/compiler/profile.
- [ ] No unexpected quadratic behavior exists in body publication/projection.
- [ ] No per-body heap allocation occurs in hot state publication after buffers are prepared, unless measured and justified.
- [ ] No speculative parallelism/cache framework is introduced.

### 30.7 Platform validation

- [ ] Windows native build/check/tests pass.
- [ ] Linux native build/check/tests pass.
- [ ] `--celestial-model` interactive sequence is exercised on Windows with OS/GPU/backend recorded.
- [ ] `--celestial-model` interactive sequence is exercised on Linux with OS/GPU/backend recorded.
- [ ] Current-revision CI passes available Linux-quality and Windows-compatibility jobs.
- [ ] Missing platform evidence is recorded honestly rather than marked complete.

### 30.8 Documentation

- [ ] `README.md` describes Phase 2 implemented scope accurately.
- [ ] `docs/architecture.md` records world/simulation/math/renderer ownership.
- [ ] `docs/roadmap.md` advances status without claiming Phase 3 work.
- [ ] `docs/performance.md` records Phase 2 benchmark baseline.
- [ ] `docs/phase-2-validation.md` records numerical and interactive evidence.
- [ ] ADR 0003 records celestial-state/time/frame-projection decisions and alternatives.
- [ ] Deferred decisions remain deferred.

---

## 31. Required implementation-review questions

Before Phase 2 is declared complete, explicitly answer these questions in the final implementation report:

1. Where does `SimulationInstant` live, and why does that dependency direction avoid a crate cycle?
2. What data is authoritative for a celestial body's translation and rotation?
3. Can the complete `FrameTree` projection be destroyed and rebuilt without changing celestial identity/state?
4. Does any `CelestialBody` store a runtime `FrameId` as authoritative data? It should not.
5. Does any moon-like body inherit another body's rotating frame accidentally? It should not.
6. Can changing a body's mass or radius move it without an explicit state edit? It should not.
7. Can a failed multi-body update leave some bodies at a new time and others at an old time? It must not.
8. Can the renderer or UI directly mutate physical state without going through validated world APIs? It should not.
9. Does playback time itself mutate body state, or does a state producer explicitly make the world valid at the requested instant?
10. Could Phase 3 enumerate mass/position/velocity for all bodies and publish a new coherent state without redesigning Phase 2?

Any "no" answer where "yes" is required, or vice versa, blocks Phase 2 completion.

---

## 32. Phase 2 north-star scenario

The simplest end-to-end scenario that should work after Phase 2 is:

1. Create one celestial system at simulation time `0 s`.
2. Insert three bodies with star/planet/moon-scale mass and radius values.
3. Give each body independent system-space position, velocity, orientation, and spin.
4. Build the derived Phase 1 frame projection.
5. Select the planet-like body and inspect its translating and body-fixed frames.
6. Run analytic time forward at `1x`, then `100x`.
7. Pause.
8. Seek backward.
9. Reverse playback.
10. Confirm that reaching the same explicit time always restores the same authoritative body states.
11. Confirm that body-center translation and body spin publish into separate frames.
12. Confirm that the moon-like body does not rotate around the planet merely because the planet spins.
13. Edit the planet-like body's mass and radius.
14. Confirm identity and kinematic state remain unchanged.
15. Destroy and rebuild the frame projection.
16. Confirm the celestial system is unchanged and the rebuilt frame transforms are equivalent.
17. Focus the observer in the body-fixed frame and verify local debug geometry is stable while the body's astronomical center moves.

If this works, Phase 3 can add actual gravitational evolution on top of the system without changing who owns the universe.

---

## 33. Final architectural statement

Phase 1 established **where things are** across extreme scales.

Phase 2 establishes **what celestial things exist, what physical state they currently have, and what time that state belongs to**.

The separation is intentional:

```text
CelestialSystem
(authoritative bodies + state + sample time)
        ↓
Simulation
(decides how/when authoritative state evolves)
        ↓
CelestialFrameProjection
(derived runtime reference frames)
        ↓
FrameEvaluation
(coherent spatial query)
        ↓
Renderer
(observer-relative disposable representation)
```

No later terrain, LOD, gravity, climate, vegetation, or editor feature should need to reverse this ownership chain.
