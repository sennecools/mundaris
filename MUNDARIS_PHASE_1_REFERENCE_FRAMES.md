# Mundaris — Phase 1: Coordinates, Reference Frames & Precision

> **Status:** implementation specification; implementation has not begun
>
> **Targets:** stable Rust, Rust 2024, native Windows x86-64 and Linux x86-64
>
> **Architecture:** [Engine design](MUNDARIS_ENGINE_DESIGN.md), especially Sections 6, 8–13, 30, 42, 46–50, and 68
>
> **Baseline:** [Project bootstrap](MUNDARIS_PROJECT_BOOTSTRAP.md) and the existing six-crate workspace

## 1. Outcome and scope

Phase 1 proves that authoritative frame-local `f64` state can produce stable human-scale GPU geometry while shared ancestors translate and rotate at astronomical distances. It establishes reusable mathematical semantics, one observer pose, explicit rendering boundaries, and a small inspectable validation mode.

The motivating future situation is a star/system origin, a planet roughly `1.5 × 10^11 m` away, orbital translation, planetary spin, a moon, an observer `1.7 m` above a surface, a nearby tree, and a mountain kilometres away. Phase 1 represents the **coordinate relationships** using generic frames and debug primitives. It does not implement those domain objects or their simulation.

### Included

- Frame-local positions, displacements, directions, linear velocities, angular velocities, and poses with explicit semantics.
- Checked double-precision unit rotations and rigid transforms; composition, inversion, and frame conversion.
- A single-root mutable frame tree with opaque runtime handles and validated edits.
- Read-only, coherent-instant evaluation, lowest-common-ancestor conversion, reusable prepared conversions, and instantaneous kinematic conversion.
- Observer pose re-expression and checked camera-relative rendering.
- Debug primitives, a validation mode in the existing application, headless tests, and focused CPU benchmarks.

### Excluded

No orbital mechanics, gravity, celestial-body model, planet meshes, terrain, LOD, procedural generation, water, atmosphere, vegetation, tectonics, hydrology, collision, physics engine, gameplay, serialization, or persistence format. No universe catalogue, general scene graph, ECS, job system, render graph, trait-based transform providers, full units library, or new crate.

Instantaneous velocity conversion is mathematical differentiation, not time integration. The demo's analytic animation is a fixture, not a celestial simulation or simulation-clock design.

## 2. Repository baseline and audit conclusions

The current app owns a `winit` lifecycle, bounded redraw, and a bootstrap UI callback. Renderer owns a safe `Arc<Window>`-backed surface, default-feature `wgpu` device, clear/presentation, resize handling, and `egui` integration. Core, math, world, and simulation contain crate documentation and unsafe prohibitions only. The only current project dependency is app → renderer. `glam = "0.30"` is selected in the workspace but is not linked by math yet. Graphics/UI selections are `wgpu 27`, `winit 0.30`, and `egui 0.33`.

Existing tests cover redraw scheduling; no math or GPU test infrastructure exists. CI runs Linux formatting, locked Clippy and headless tests, plus a Windows locked all-targets check. Benchmarks and graphical execution stay outside ordinary CI. Preserve these constraints, the Rustdoc conventions, explicit errors, and `forbid(unsafe_code)`.

The engine-design audit preserves the authoritative/derived-state split, sparse modifications, independent layers, six crates, replaceable algorithms, and open terrain representations. Corrections in the engine design address:

1. **Precision:** `f64` root subtraction can lose local information before subtraction. Cancel shared ancestry and center in the source frame before rotation/narrowing.
2. **Moving ancestry:** transform parents carry rotation as well as translation. Orbital relationships must not implicitly attach a moon to planetary spin.
3. **Ownership:** math owns generic frame algorithms; future world state owns domain associations and tree instances; renderer owns GPU conversion policy.
4. **Temporal/velocity semantics:** all evaluation uses one instant; coordinate re-expression differs from attachment; moving-frame velocity needs derivatives.
5. **Future content:** observer frames, runtime handles, and LOD must not become terrain/edit/generation identity. Regional Cartesian frames do not choose surface topology.
6. **Repository consistency:** distinguish intended from actual dependencies, preserve the unsafe prohibition, align roadmap ordering, and avoid unverified platform-completion claims.

These are the reasons for the decisions below, not grounds for replacing the overall architecture.

## 3. Coordinate spaces: the minimal model

All physical frames are right-handed, orthonormal, metre-based Cartesian frames. There is one generic frame implementation, with runtime identity. “System”, “body-fixed”, and “regional” describe uses, not an enum of mandatory frame kinds or independent coordinate implementations.

| Space/use | Responsibility | Phase 1 representation |
| --- | --- | --- |
| System/root | Finite working origin for one connected frame tree; future solar-system state may be expressed here. No universal absolute location is promised. | One identity root `FrameId`. |
| Body-local/body-fixed | Attached content inherits body translation and orientation while keeping local coordinates unchanged. | Generic child frame; no `Planet`, `BodyId`, or special transform. |
| Surface/regional local | Small Cartesian offsets around a chosen body-local anchor; useful for human-scale data. It is a rigid basis/origin, not latitude/longitude, a terrain patch, or a surface definition. | Optional generic child frame, used by the fixture. |
| Observer pose | Position in an existing frame plus camera-local-to-frame orientation. Control mode is separate. | `FramePose`; observer is not required to occupy a tree node. |
| Render/view relative | Disposable coordinates with origin exactly at the observer and axes equal to camera axes for this phase. | Renderer-owned `RenderRelativePosition` with `f32` components, tied to one prepared view. |
| Clip | Projection output only; not a physical coordinate. | Shader output, `wgpu` depth range `0..1`. |

There is no distinct `UniversePosition`, mandatory solar-system child under a universe root, spherical coordinate API, or standalone floating-origin space. Additional working roots/large-scale addressing can be designed when cross-system requirements exist.

An eventual tree may use a translating planet anchor and a rotating body-fixed child. A moon can use the translating anchor or system as parent. A moon below the rotating child really does inherit spin; the math must not special-case or suppress that rotation. Simulation, not the tree, decides the appropriate relation.

## 4. Units and scalar validity

| Quantity | Canonical unit/representation |
| --- | --- |
| Position, translation, displacement | metres, `f64` |
| Time/sample time | seconds, `f64`, relative to a caller-defined working epoch |
| Mass | kilograms when introduced later; no mass type in Phase 1 |
| Linear velocity | metres per second, `f64` |
| Angular velocity | radians per second, axial `f64` vector |
| Angles | radians, `f64`; degrees only at explicit UI conversion |
| Rotation/direction | dimensionless unit quaternion/vector, `f64` |
| Render position | metres, `f32`, explicitly derived |

Time need not be positive or monotonically increasing: deterministic fixture seeking and future backward sampling are legitimate. It must be finite. Sample time is metadata about supplied state, not an instruction to extrapolate the tree. No Julian-date precision, geological-time arithmetic, duration algebra, or simulation clock is introduced.

Public constructors reject NaN/infinity and invalid normalization. Arithmetic returning new dimensional values checks for non-finite results; finite inputs can overflow. Do not clamp to a “safe” value, replace invalid rotations with identity, or silently rescale physical coordinates. Units are communicated by types plus named accessors such as `metres()` and `metres_per_second()`; no project-wide `Real` alias.

## 5. Type semantics and safety

Use a small set of concrete wrappers in math:

- `LocalPosition(DVec3)`: a point in a basis/origin supplied by its containing frame-aware type or transform operation.
- `Displacement3(DVec3)`: an offset in metres, no origin; can have zero length.
- `Direction3(DVec3)`: dimensionless unit direction, never zero.
- `LinearVelocity3(DVec3)`: derivative in metres/second, not a displacement.
- `AngularVelocity3(DVec3)`: axial radians/second vector, not Euler angle rates.
- `UnitRotation(DQuat)`: checked unit quaternion, private storage.
- `FramePosition`, `FrameDisplacement`, `FrameDirection`, `FrameVelocity`: the corresponding value plus a `FrameId`.
- `FramePose`: frame-local position plus local-to-frame orientation.
- `KinematicPoint`: one `FramePosition` and relative velocity in that same frame; constructors enforce frame agreement.

Use composition of these concrete structs rather than phantom types for every dynamically allocated frame, generic quantity dimensions, or duplicate `SystemPosition`/`BodyLocalPosition` representations. Runtime frame identities cannot be checked solely by Rust's type system; public operations check them.

Fields are private. No `Deref` to `DVec3`, blanket `From<Vec3>`, implicit `Into` render conversions, or arbitrary point/vector casts. Raw `glam` accessors are explicit escape hatches for narrow numerical code. Renderer wrappers expose only GPU-layout accessors, never conversion back into authoritative positions.

Provide named, checked operations initially:

- point − point in the same frame → displacement;
- point + displacement in the same frame → point;
- rotation of a displacement/direction → same semantic kind;
- frame conversion of each kind with its correct rule.

Do not provide point + point, translation of a direction, position-as-velocity, or a “transform vector” API shared by velocity and displacement. Avoid generic arithmetic/operator proliferation. Zero-displacement direction extraction may return `Option<Direction3>`; an explicit direction constructor returns a useful error for zero or invalid input.

## 6. Rotation and rigid-transform conventions

### 6.1 Axes and handedness

- Right-handed Cartesian basis: `+X × +Y = +Z`.
- Camera local `+X` is right, `+Y` is up, and viewing direction is `-Z`.
- `+Y` is a fixture's local up, not universal gravity or a constraint on a planet's spin axis.
- Positive angles follow the right-hand rule. A `+π/2` rotation about `+Z` sends `+X` to `+Y`; about `+Y` sends `+X` to `-Z`.
- Quaternion components exposed at explicit construction are `(x, y, z, w)`, matching `glam`.
- Mathematical notation and GPU matrices use column vectors. `R * p` applies the rotation; `(R2 * R1) * p` applies `R1` first.

### 6.2 Quaternion contract

Use `DQuat`, not `Quat`, Euler storage, dual quaternions, or a general `DMat4` as authoritative rotation. Quaternions compactly compose arbitrary orientation without Euler singularities. Matrices are derived renderer products.

`UnitRotation::try_from_quaternion` accepts finite quaternions whose `abs(length_squared - 1) <= 1e-10`, then normalizes once. Reject zero, near-zero, materially non-unit, and non-finite input. Normalization is documented correction of roundoff, not recovery of corrupt state. Axis-angle construction requires a `Direction3` and finite radians and produces a normalized quaternion.

Quaternion composition normalizes its product; conjugation is the inverse of a valid unit rotation. Maintain `abs(length_squared - 1) <= 1e-12` after public operations. Treat `q` and `-q` as equivalent orientations: compare rotated basis vectors or `abs(dot(q1, q2))`, not component equality. No canonical sign or serialization representation is locked. Future interpolation must choose a consistent shortest-arc/sign policy before use; no interpolation API in Phase 1.

### 6.3 Transform notation and operations

`T_B_from_A = (t_B_from_A, R_B_from_A)` maps coordinates in A to B:

```text
p_B = t_B_from_A + R_B_from_A p_A
d_B = R_B_from_A d_A
n_B = R_B_from_A n_A
```

`t_B_from_A` is A's origin expressed in B, in metres. A child stores exactly `T_parent_from_child`. No scale/shear/reflection exists in a reference-frame edge. Non-uniform mesh scale is outside this layer.

Composition and inverse:

```text
T_C_from_A = T_C_from_B ∘ T_B_from_A
R_C_from_A = R_C_from_B R_B_from_A
t_C_from_A = t_C_from_B + R_C_from_B t_B_from_A

R_A_from_B = conjugate(R_B_from_A)
t_A_from_B = -R_A_from_B t_B_from_A
```

Use a named method `compose(self, inner)` meaning “self after inner”; no ambiguous multiplication overload initially. Small mathematical values may be `Copy`. The untagged `RigidTransform` is a primitive whose caller supplies source/destination semantics. Graph-produced conversions carry checked endpoints, so normal callers do not assemble untagged chains.

An identity rotation/transform and quaternion conjugation cannot fail. Rigid composition, inverse translation, and applying a transform return `Result` if arithmetic becomes non-finite. Rigid inversion has no singular-matrix branch because scale is excluded; invalid quaternion input is rejected at construction.

## 7. Frame tree, identity, mutation, and evaluation

### 7.1 Identity and topology

`FrameTree` stores contiguous append-only nodes. Each node contains parent index, depth, and `FrameState`. `FrameId` is opaque, `Copy`, `Eq`, and `Hash`, internally containing a caller-assigned nonzero tree namespace and a `u32` node index. No string lookup in conversion loops.

`FrameTree::new(namespace: NonZeroU64)` creates node zero as immutable identity root. The app owns namespace allocation using a checked monotonic session counter; every independently created/reset tree gets a fresh namespace. Reusing a namespace for different live trees violates a documented constructor invariant. No global registry/random UUID or core ID framework is needed. A tree is not `Clone`; read-only evaluations borrow it. Handles remain valid across state updates and reparenting and are not persistent IDs.

There is exactly one root, all other nodes have exactly one valid parent in the same tree, and every node reaches the root. Child insertion requires an existing parent. Public reparenting rejects root edits, self-parenting, cross-tree/missing parents, and descendant-parent cycles. Capacity/index overflow returns an error.

Do not implement deletion, slot reuse, generational arenas, graph import, or batch topology construction in Phase 1. Append-only handles deliberately avoid stale-slot aliasing. Removing frames later requires an explicit handle-lifetime design rather than reusing indices silently.

Reparenting takes an explicit **new local state** and can intentionally change physical pose. Name it `reparent_with_local_state`. Observer pose re-expression is a separate operation (Section 10); no hidden preserve-world option. Check the proposed edit completely before mutation. On failure, parent, depths, state, time, and revision remain unchanged.

### 7.2 Instantaneous state

`FrameState` contains a `RigidTransform` and `Option<FrameMotion>`:

- `FrameMotion` has child-origin velocity relative to parent, expressed in parent axes, and child angular velocity relative to parent, also in parent axes.
- `Some(FrameMotion::stationary())` is an explicit zero relative derivative, not a prediction that the edge will never change.
- `None` means derivatives are unknown; pose conversion remains valid, kinematic conversion through that edge fails.
- Root transform is identity and root relative derivatives are zero by definition; it is a working reference, not proof of physical inertiality.

Publish a batch via `update_states(sample_time_s, updates)`, with entries strictly ordered by node index and no duplicates. Validate all IDs, root rules, ordering, time, and numeric state before writing. This permits `O(U)` validation/application for U updates without per-update hashing or allocation. Empty batches may update sample time for an entirely unchanged tree. Unlisted states are retained; the caller guarantees they remain valid at the new instant. In particular, it must sample all changed moving edges at that instant.

Transform construction does not infer derivatives from successive poses. Future simulation must supply consistent states; the tree cannot prove that given velocities are derivatives of supplied transforms. Demo animation supplies both analytically. Do not approximate derivatives using render-frame `dt`.

State edits and topology edits increment a checked revision. Topology edits occur at the current sample instant and rebuild depths/parent-first traversal metadata as needed. Pose-only update is `O(U)` and does not recompute descendants or touch attached objects. Topology changes may allocate/rebuild `O(F)` metadata for F frames and are explicitly outside the hot update path.

### 7.3 Evaluation lifetime

`FrameEvaluation<'a>` is an immutable borrow of one tree state, with its sample time and revision. It cannot coexist with mutable edits of that tree. Prepared conversions and prepared render views borrow the evaluation/tree lifetime, preventing stale transforms from silently surviving edits. Avoid owned snapshots, internal mutability, synchronization, and “latest transform” globals.

Evaluate only after all updates for an instant are published. Observer, object poses, and derivatives supplied alongside the evaluation must belong to that same sample instant. Initial implementation is single-threaded; coherence comes from explicit update/evaluate phases and borrowing. Render interpolation is deferred and must eventually create a separate coherent evaluation, never mix ticks per object.

### 7.4 Local, root, and frame-to-frame transforms

Root/world transform is the composed `T_root_from_frame`. Expose `transform_to_root` as an explicitly depth-dependent diagnostic/coarse-query operation. It is not authoritative replacement state and is not used to center nearby geometry.

For from frame A and to frame B, find lowest common ancestor L by aligning depths and walking parents. Accumulate `T_L_from_A` and `T_L_from_B` from **only** their respective branches below L. Parent walking and composition use stack values; no allocated path vectors or recursive stack dependence.

Let accumulated origins be `t_A`, `t_B` and rotations `R_A`, `R_B`, all expressed in L. Point conversion is:

```text
p_B = conjugate(R_B) ((t_A - t_B) + R_A p_A)
R_B_from_A = conjugate(R_B) R_A
```

Subtract branch origins before rotating their difference. Do not implement the precise API by composing two astronomical root transforms. Keep the two branch origins in the prepared conversion so application does not reconstruct them from a rounded root matrix.

Same-frame conversion validates the handle and returns the original value. Conversion of displacement/direction uses only `R_B_from_A`. Common ancestry above L contributes neither pose nor motion to conversion between A and B.

`prepare_conversion(from, to)` costs `O(h_A + h_B)` and returns a reusable `PreparedFrameConversion`. Applying it to points, displacements, or directions costs `O(1)`, checks endpoint agreement and finite output, and allocates nothing. Direct convenience conversion delegates to preparation. Do not add a global all-pairs cache or root-transform cache. Group visible objects by source frame to reuse preparation; optimizing branch lookup further requires benchmark evidence.

## 8. Precision and the CPU → GPU contract

### 8.1 Required precision

All authoritative Phase 1 physical positions, local anchors, translations, orientations, motion derivatives, observer poses, and frame computations use `f64`. Human-scale authoritative state also uses `f64`: narrowing it early needlessly restricts later queries and accumulated motion.

`f32` is acceptable for final render-relative vertices, projection matrices, colors, GPU depth, UI values that are not authoritative state, and eventual small mesh-local visual data with a documented error budget. The fixture keeps source debug geometry in `f64` so it measures the boundary cleanly. No GPU doubles, high/low position pairs, or astronomical `f32` transforms are required.

Only renderer owns physical `f64` → `f32` conversion. Mathematical APIs never return `Vec3` render data. UI angle/time controls explicitly convert to the authoritative units/type. Shader inputs contain no root position, frame-local astronomical origin, `DVec3`, `DQuat`, or authoritative velocity.

### 8.2 Why flattening is insufficient

Approximate spacing between adjacent floating-point values:

| Magnitude in metres | `f32` spacing | `f64` spacing |
| --- | --- | --- |
| `100` | `7.63e-6 m` | `1.42e-14 m` |
| `1e4` | `9.77e-4 m` | `1.82e-12 m` |
| `6.371e6` | `0.5 m` | `9.31e-10 m` |
| `1.5e11` | `16384 m` | `3.05e-5 m` |
| `1e16` | about `1.07e9 m` | `2 m` |

Subtracting two nearby values is not itself necessarily rounded inaccurately; the problem is that adding local detail to large origins and independently rotating large points may already have lost the information. Narrowing after that loss cannot recover it.

Shared ancestry is removed before point evaluation. For each source frame A, convert the observer **into A**, then subtract locally before applying relative rotation:

```text
c_A = convert_position(observer.position, to = A)             // f64, LCA path
R_camera_from_A = inverse(observer.orientation)
                  * R_observer_frame_from_A                   // f64
p_view = R_camera_from_A (p_A - c_A)                           // subtract first
p_gpu = checked_f32(p_view)                                    // renderer boundary
```

Observer orientation maps camera-local to observer-frame coordinates. Its inverse places points into camera axes. The render origin is exactly the observer position; render and view space coincide for Phase 1. There is no additional GPU view-translation matrix and no second camera subtraction in WGSL.

This arrangement preserves small deltas in a shared body/regional frame even if a common ancestor is at `1e16 m`, and avoids rotating two million-metre points before subtracting. An observer expressed in a different branch with large nearly equal translations can still carry source quantization. Guarantees below distinguish those cases.

### 8.3 Supported error envelope

Test norms using Euclidean error unless a component rule is stated. Required CPU results for hierarchy depth at most 8 and finite unit rotations:

| Case | Required absolute error |
| --- | --- |
| Local points/anchors within `1e3 m`, shared local ancestry | `<= 1e-9 m` |
| Body-local branch coordinates within `1e7 m` | `<= 1e-7 m` |
| Root/independent-branch coordinates around `1.5e11 m` | `<= 1e-3 m` |
| Common astronomical translation/spin changes with unchanged local observer/content | Same local results within `1e-9 m`; magnitude of excluded ancestry does not relax this budget. |
| Unit-direction/rotated-basis vector error | `<= 1e-12` |

These are implementation acceptance envelopes, not limits on graph depth or a universal error bound for arbitrary numbers/ill-conditioned inputs. Use the specified finite bounded fixtures and report larger-scale tests separately. No epsilon silently classifies physically distinct frames or points as identical.

Renderer's near-debug path accepts final view coordinates with Euclidean length `<= 10_000 m` and checks `max(abs(f64(f32(component)) - component)) <= 1e-3 m`. Also test tighter budgets `1e-5 m` within `100 m` and `1e-4 m` within `1_000 m`. These cover the tens-of-metres proxy and kilometre-scale marker; they do not claim sub-millimetre precision everywhere in a solar system.

`RenderPrecisionBudget` has validated finite positive `max_distance_m` and `max_component_error_m`. `try_render_position` checks source/result finiteness, range, and actual round-trip narrowing error. Exceeding the budget returns an error; it does not clamp, silently disappear, or upload infinity. Representation selection may deliberately omit out-of-range debug geometry before conversion and report it as such. Invalid data remains a failure even when off-screen.

### 8.4 Floating origin and future limits

No conventional world-wide floating-origin rebase is needed: moving the observer changes derived view coordinates only. Physical frame transforms and attached content are never shifted to keep a renderer near zero. Observer re-expression (Section 10) changes one pose's coordinate frame without mutating the world.

Local physics libraries may eventually need a separately owned rebasing working space. Cross-system addressing may need sector/integer or extended precision. Distant mesh precision, depth spanning metres to AU, shadows, atmosphere, and terrain LOD need additional representation techniques. Do not lock those choices to this debug range or put a permanent world draw cutoff in the frame layer.

## 9. Moving frames and instantaneous velocity semantics

### 9.1 Attached points

An attached point stores `(FrameId, LocalPosition)`. Changing its frame's local state changes where it is observed, not the point's stored coordinates. Updating an astronomical parent is `O(1)` state work for that edge (plus batch validation), independent of the number of attached objects. Descendant **evaluation** may change when requested, but their local states and all attached point data remain unchanged.

A stationary local point has zero **relative** velocity in its frame, not zero physical velocity in the root. Directions and displacements do not inherit origin velocity.

### 9.2 Child-to-parent derivative

Given `p_P = t + R p_C`, let:

- `u = d(t)/dt`, child origin velocity relative to parent, expressed in P;
- `omega`, child angular velocity relative to parent, expressed in P;
- `v_C = d(p_C)/dt`, derivative of child coordinate components relative to C.

Then:

```text
v_P = u + omega × (R p_C) + R v_C
v_C = inverse(R) (v_P - u - omega × (p_P - t))
```

Thus a velocity conversion requires the **point at which it applies**, motion derivatives, and a sample instant. There is no position-free `convert_velocity(FrameVelocity)` between moving frames. Merely changing the basis of an already defined physical vector is a rotation operation, not this conversion.

### 9.3 Composition of moving edges

For A → B → C, translations/rotations compose as Section 6. To carry derivatives, expressed in the destination axes:

```text
omega_C_from_A = omega_C_from_B + R_C_from_B omega_B_from_A
u_C_from_A = u_C_from_B
           + omega_C_from_B × (R_C_from_B t_B_from_A)
           + R_C_from_B u_B_from_A
```

Compose motion only on branches below the LCA. Shared ancestor motion cancels when expressing coordinates in either descendant. If any required branch motion is `None`, return `MissingMotion(FrameId)`. Do not interpret unknown as zero. Same-frame conversion does not require motion, nor does conversion between descendants require motion above their LCA.

For A/B branches into L, evaluate a point and velocity in L, then invert the B relation. When inverting B, compute the lever arm as `(t_A - t_B) + R_A p_A`, rather than forming a huge `p_L` and subtracting `t_B` afterward. Zero/local common-motion cases must retain the precision envelope. A prepared pose conversion may obtain a separate prepared kinematic conversion when motion is requested; point-only rendering does not require derivative composition.

Phase 1 implements these small formulas and tests their analytic results and finite-difference checks. It does not derive acceleration, apply Coriolis/centrifugal terms, enforce inertial frames, integrate an observer, or select a future physics working frame. Acceleration/force conversion later requires time derivatives beyond this first-order contract.

Velocity round-trip budgets must include position error multiplied by angular speed: reconstructing a lever arm with position error `epsilon_p` can add approximately `|omega| * epsilon_p` velocity error. The astronomical observer-handoff fixture therefore permits `5e-4 m/s` alongside its `1e-3 m` position budget at `0.2 rad/s`; local/body-scale fixtures retain their tighter velocity budgets. Do not require a root round trip to preserve velocity more accurately than its rounded position permits.

## 10. Observer state and seamless frame changes

The app owns `ObserverState` containing a mathematical `FramePose` and relative `FrameVelocity` with the same frame. Projection settings and validation controls are separate. Initial relative velocity is explicitly zero in the fixture's regional frame; no controller/physics loop is implied.

The mathematical pose type is not camera-specific and belongs in math. The app owns observer/session meaning; renderer receives a pose and evaluation read-only. No camera matrix is authoritative.

`FrameEvaluation::reexpress_pose(pose, target)` converts position and orientation at the evaluation instant:

```text
p_target = convert_position(pose.position, target)
R_target_from_camera = R_target_from_source R_source_from_camera
```

To preserve a moving observer's physical state, convert its `KinematicPoint` in the same evaluation. Compute both outputs before atomically replacing app observer state. On invalid target or unknown required motion, leave the original observer unchanged and return a diagnostic. Pose-only re-expression is still useful for non-physical views, but must not be presented as velocity-preserving migration.

Changing coordinate frame preserves instantaneous physical pose and velocity; it does **not** constrain subsequent observer motion. Setting relative velocity to zero in a rotating frame is a deliberate attachment/control operation and generally changes physical velocity. Keep that action separate.

Future approach can start with a system-frame observer and re-express it in a body/regional frame when local precision is useful. Both reference the same connected world. Automatic frame selection, hysteresis, gravity-aligned walking, collision, speed policy, and interpolation are deferred. The validation mode uses an explicit reproducible handoff threshold and manual re-expression control to test continuity.

## 11. Rendering ownership and concrete data flow

```text
app owns fixture tree + observer + f64 debug points
    ↓ update all moving edge states at t_s
math FrameEvaluation borrows coherent tree
    ↓ renderer prepares one view from observer pose
renderer prepares source-frame centering/rotation once per source frame
    ↓ f64 local subtraction and rotation for each requested debug vertex
renderer checks budget and narrows to f32
    ↓ packs view-relative vertex bytes and projection uniform
wgpu debug pass → egui pass → presentation
```

Renderer must not mutate the tree, integrate animation, persist observer state, or depend on `mundaris_world`. App must not perform ad hoc casts of astronomical positions for rendering. Precision tests exercise renderer's CPU preparation without creating a device.

### 11.1 Prepared view and batches

`PreparedView<'a>` retains the immutable tree borrow represented by an evaluation, the observer pose, and a validated budget. `prepare_source(FrameId)` builds a `PreparedRenderFrame<'a>` holding the source frame ID, observer position in that source, and source-to-camera `UnitRotation`. These prepared values may outlive the lightweight evaluation wrapper, but cannot outlive the tree or permit mutation while retained.

`PreparedRenderFrame::try_position(FramePosition)` checks the source frame before centering. A batch method writes to caller-owned output storage, checks lengths/space agreement, and returns the first indexed failure. Document that batch output may be partially written on failure and must not be submitted; app builds/validates the full frame before uploading. No hidden allocation or automatic precision fallback.

Owned render-relative outputs are valid only for that view/sample. Rebuild the outgoing batch whenever observer or frame state changes; do not append vertices prepared for another view or reuse an old relative batch as world state. Keep this lifetime rule explicit in the debug-frame API, without adding a general render-cache framework.

### 11.2 Minimal GPU contract

Use a line-list pipeline for colored axes and wire boxes. Source edges/primitives are finite `f64` positions supplied by app. No instance transform framework, general asset mesh, lighting, texture, or terrain shader is needed. CPU conversion of this small number of vertices keeps the precision proof visible.

Pack each debug vertex as eight `f32` values: view position `(x, y, z, 1)` followed by color `(r, g, b, a)`. Vertex stride is 32 bytes; shader locations 0/1 are `Float32x4`, offsets 0/16. Projection uniform is a 64-byte, column-major `mat4x4<f32>` with explicitly tested byte order/layout. Use safe explicit byte packing (`to_le_bytes`) into a reused staging vector; do not expose `glam` memory layout, add unsafe casts, or require a packing library for this fixture. Supported targets are little-endian x86-64.

WGSL computes `clip_position = projection * view_position`. No astronomical origin or view translation exists in shader inputs. Use right-handed `0..1` projection, vertical FOV `60°` converted explicitly to radians, near `0.05 m`, far `10_000 m`, and viewport aspect ratio. Standard forward depth, clear `1`, compare `LessEqual`, and `Depth32Float` are sufficient for non-overlapping debug lines. Recreate depth resources on nonzero resize; preserve suspension/minimize/surface-recovery behavior. These are fixture renderer decisions, not a final planet/depth strategy.

Out-of-range astronomical frames are shown as UI bearing/distance markers computed in `f64`; only finite dimensionless screen coordinates/color cross to UI/GPU. Labels identify them as markers, not distance-scaled meshes. Near geometry uses the single view-relative path. Shader validation and layout tests are headless; native interactive behavior is checked manually.

## 12. Error model and invariants

Use small `thiserror` enums at library boundaries and `anyhow` context in app, consistent with the bootstrap. Include the relevant frame/source/target or vertex index in errors; avoid per-point logging in hot loops.

| Operation | Contract |
| --- | --- |
| Identity values, accessors, valid quaternion inverse, immutable evaluation borrow | Cannot fail. |
| Extract a direction from a valid displacement that may be zero | `Option`; `None` means exactly zero length, not corruption. Use robust norm computation. |
| Parent lookup | `Result<Option<FrameId>, FrameError>`; root is `Ok(None)`, invalid handle is an error. |
| Optional state lookup | `Option<&FrameState>` only when absence is expected; conversion never hides invalid handles this way. |
| Numeric constructors, transform arithmetic, conversion, graph edits, sample publication, render narrowing/projection parameters | `Result` with explicit invalid/non-finite/overflow/mismatch/range categories. |
| Kinematic conversion with unknown derivatives | `Result::Err(MissingMotion)`; never default to stationary. |
| Allocation exhaustion | Normal Rust allocation behavior; no custom recoverable allocator contract. Checked index/revision/session-counter overflow returns an explicit error. |
| Internal parent/depth metadata disagreement or impossible traversal after validated edits | Programmer invariant assertion with useful message. Debug checks may be more expensive; do not silently repair a cycle. |

Representative errors: `NonFinite`, `InvalidQuaternionNorm`, `ZeroDirection`, `ArithmeticOverflow`, `UnknownFrame`, `WrongTree`, `FrameMismatch`, `RootMutation`, `Cycle`, `UnorderedUpdates`, `CapacityExceeded`, `RevisionOverflow`, `MissingMotion`, `OutsideRenderRange`, `PrecisionBudgetExceeded`, and `InvalidProjection`.

Do not use a panic for user-controlled invalid transforms or IDs. Preserve all authoritative state on failed mutations. There is no non-invertible-scale fallback because such transforms cannot be constructed.

## 13. Determinism

- Identical finite input state and query order on the same executable/target produce repeatable numerical results, independent of wall-clock time or previous evaluations.
- Tree traversal/branch composition has a defined parent order. Prepared and direct queries use the same algorithm; caching must not alter summation order.
- Windows/Linux and debug/release must satisfy the same numerical tolerances. Cross-platform/component-bit identity, cross-version libm behavior, GPU pixels, and lockstep replay are not promised.
- No RNG, ambient state, hash-map iteration dependency, or frame-rate-dependent integration in the math layer.
- Namespaces/handle values need not reproduce across sessions; topology and values are what determine numerical output. Persistent/generated identity must not depend on runtime frame IDs.
- Fixture motion is a pure function of explicit sample seconds; pause/resume/seek use explicit app state. Replay tests sample `t = n / 60` for fixed integer n.

Do not expand procedural determinism policy based on these math guarantees. Future terrain identity and persisted generation demand their own versioned contracts.

## 14. Performance design and benchmark strategy

### 14.1 Expected costs

| Operation | Cost/allocation expectation |
| --- | --- |
| State batch update | `O(U)`; no allocations after input preparation. No attached-object iteration. |
| Frame insert/reparent | Explicit topology work; may allocate/rebuild `O(F)` metadata. |
| Read-only evaluation | Borrow and metadata access, `O(1)`, no allocation. |
| Prepare frame/source conversion | `O(h_A + h_B)`, no heap path allocation. |
| Apply prepared point/direction/kinematic conversion | `O(1)` per value, no allocation. |
| Render centering/narrowing batch | `O(N)`, caller-reused storage. |
| Root/world query | `O(h)`, explicitly requested, never per-vertex by default. |

Contiguous frame records and batched vertex storage are sufficient initially. Use static dispatch and simple loops. Recompute relative transforms per relevant frame pair once per view/evaluation; no persistent dirty-descendant cache is needed for this workload. Per-frame `wgpu` command objects and existing UI allocations are outside the math/batch no-allocation promise.

### 14.2 Focused benchmarks to add during implementation

Use stable Criterion with `harness = false` in math and renderer benchmark targets only. Centralize its selected non-prerelease version at implementation time; no benchmarks run in normal CI. Use `std::hint::black_box`, non-constant fixtures, and preallocated input/output. Timing excludes graph construction, device/window startup, shader compilation, printing, and allocation of fixture data.

Required groups:

1. **Preparation:** same-frame, sibling, ancestor/descendant, and cross-branch pairs at depths 1, 4, 8, and 32. Measure pose-only and motion-aware preparation separately.
2. **Repeated conversion:** 1,024 and 65,536 points from the same source frame, direct query per point versus one preparation followed by a batch. Verify equal outputs within the test budgets before timing. The direct path is the meaningful baseline.
3. **Render preparation:** local `100 m`, kilometre-scale `10 km`, and common-ancestor `1.5e11 m`/`1e16 m` fixtures. Measure preparation separately from centering/rotation/checked narrowing. The astronomical shared offset should not add point-count work.
4. **Moving-state publication:** 1 and 64 updated edges in trees of 64 and 4,096 frames, followed by preparation for a fixed visible subset. Separate update from preparation. Confirm cost is independent of whether the app has 0 or 65,536 attached debug points.

Record throughput, median/distribution, batch sizes, CPU/OS, compiler/profile, relevant `glam` features, and lockfile/dependency versions. Run on at least one supported native target, capture the baseline in `docs/performance.md`, and investigate unexpected allocation, quadratic behavior, or batch regressions. No hardware-independent nanosecond/FPS threshold or flaky timing assertion. Windows/Linux interactive and numerical checks remain required regardless of benchmark host.

Validate allocation expectations with inspection and an existing external profiler where available; do not weaken unsafe policy to add a custom counting allocator. Future benchmarks may examine culling, terrain patches, instancing, and local physics only when those workloads exist.

## 15. Validation application

Add `cargo run -p mundaris_app -- --reference-frames`. Normal invocation may retain the bootstrap smoke view. Parse this one flag simply; no CLI/configuration framework or separate application crate.

### 15.1 Fixture graph and geometry

```text
system root (identity)
├── translating anchor
│   ├── rotating local frame
│   │   └── regional anchor at (0, 6_371_000, 0) m
│   └── distant sibling marker at (384_000_000, 0, 0) m
└── root-origin marker (point, not another required frame)

observer pose: regional frame, (0, 1.7, 0) m, identity orientation
```

Debug objects in the regional frame:

- RGB axes with a `1 m` unit reference;
- a wire box at `(0, 1, -5) m`, dimensions `1 × 2 × 1 m`;
- a taller wire proxy at `(20, 5, -30) m`, dimensions `2 × 10 × 2 m`;
- a large wire marker at `(0, 100, -3000) m`, dimensions `200 × 200 × 200 m`;
- a short local line/box pair separated by `0.01 m` around the nearby box.

These are abstract shapes, not terrain, trees, mountains, celestial meshes, or generated content. Keep their local arrays unchanged throughout parent animation.

For explicit demo time t in seconds, use:

```text
translating origin = (1.5e11 + 30_000 t, 20_000_000 sin(0.001 t), 0) m
origin velocity   = (30_000, 20_000 cos(0.001 t), 0) m/s
rotating rotation = quaternion about +Y by 0.2 t radians
angular velocity  = (0, 0.2, 0) rad/s in parent axes
other edges       = explicit stationary derivatives
```

This is scripted linear/wobble motion with exaggerated spin, not an orbit. In attached regional inspection, a stress toggle replaces the shared `1.5e11` offset with `1e16` while retaining local state. Approach and root-based handoff validation use the `1.5e11` baseline; entering those modes resets the stress fixture while paused. This deliberately avoids claiming millimetre-preserving root round trips at `1e16`. Use bounded explicit sample times (for example `0..600 s`) to avoid treating a diagnostic as unbounded epoch arithmetic.

### 15.2 Modes and interactions

1. **Attached local inspection (default):** fixed regional observer with animation playing. Objects stay fixed on screen while parent diagnostics change. Pausing freezes all sample values; reset restores a deterministic fixture. UI changing time seeks to a coherent analytic state.
2. **Continuous approach:** begin on regional `+Z` at `1e8 m` separation, looking along `-Z`, and decrease separation smoothly to zero over the repeatable 30-second path below. The physical prescribed pose is `(0, 1.7, separation)` in regional coordinates. Express it in root while far, then re-express it to regional when separation passes `1e5 m`, preserving pose and velocity at that instant. Sample the prescribed trajectory in one coherent evaluation; derivative calculation includes both local trajectory and frame motion. Do not zero velocity at handoff. Root precision is adequate for the far interval; the near interval never round-trips through root. There is one tree and one observer, with no scene replacement.
3. **Manual re-expression while paused:** root ↔ rotating frame ↔ regional. Report pose/velocity residuals before and after. Root round trips use the astronomical tolerance; local-only round trips use the tighter local envelope.
4. **Local movement:** simple local offset buttons/slider, with metre-labelled `f64` state; no walking/gameplay controller required. Test `0.01 m` movement after approach.

Prescribed approach, for elapsed seconds `tau` within `0..30`, with parent motion sampled at the same demo time:

```text
u = tau / 30
a = 1 - 3u^2 + 2u^3
separation_m = 1e8 * (exp(12a) - 1) / (exp(12) - 1)
d(separation)/dt = 1e8 * 12 * exp(12a) * (-6u + 6u^2)
                   / (30 * (exp(12) - 1))
```

Use `exp_m1` for the numerator/denominator near zero. Endpoint relative velocity is zero; outside the interval hold the endpoint pose with zero relative velocity. This is a scripted trajectory, not numerical integration. At handoff compare the converted state against the independent regional trajectory before installing it; subsequent near samples are evaluated directly in regional coordinates.

Far objects/anchors may only have UI bearing markers and numerical distances; small debug boxes need not be visible from astronomical distances. Entering the debug draw range is explicitly indicated. This demo validates coordinates and frame continuity, not a distant-planet representation or infinite-depth renderer.

### 15.3 Diagnostics and objective behavior

Show sample time, current observer frame ID/label, frame-tree revision, root anchor translation, observer local pose, render origin policy, root-query position labelled diagnostic, relative box coordinates, maximum narrowing error, draw/omission counts, and handoff pose/velocity residuals. Keep fixture labels in app, not graph identity.

At fixed regional observer/geometry, compare converted nearby vertices against the paused `t=0` result over 36,001 samples (`n/60`, `0..600 s`) and both shared offsets. CPU displacement error must stay within `1e-9 m`; renderer coordinates obey the near budget. Rotation of a common ancestor must not introduce local jitter. A separate root-frame observer case must show changing relative positions, proving motion is not simply ignored.

For interactive validation at 1280×800, run attached mode for at least 60 seconds at both offsets, pause/seek/reset, complete approach/handoff, move locally, and resize/minimize/restore/close. No visible shimmer attributable to parent motion, no frame-handoff snap, no normal-path errors, and no busy redraw while non-drawable. CPU residual tests are the numerical authority; visual observation supplements them and is recorded per OS/backend rather than replacing them.

## 16. Concrete test matrix

All automated tests are headless and need no display/device. Compare against independently derived analytic answers, not only another route through the same implementation. Use explicit tolerances, checking finite output before approximate comparisons. Round-trip error budgets must match the largest intermediate coordinate, not just the final local magnitude.

| Test | Fixture and assertions |
| --- | --- |
| Parent/child composition | Parent translated `(10,20,30) m` with `+π/2` Z rotation; child translated `(2,0,0) m` with `+π/2` Y rotation. Child point `+X` maps to `(10,22,29) m`. Position error `<=1e-12 m`; verify direction result independently and composition order. |
| Inverse | Apply transform/inverse and both identity compositions to points within `1e3 m`; error `<=1e-9 m`, basis error `<=1e-12`. Test inverse translation is rotated, not just negated. |
| Frame-to-frame | Siblings with different translation/rotation, ancestor→descendant, descendant→ancestor, and same-frame. Compare analytic answers; use `1e-9 m` local budget. Invalid same-frame handles still fail. |
| Rotation | +Z/+Y quarter turns, identity, arbitrary normalized axis, `q`/`-q`, noncommuting composition. Apply 100,000 small rotations; unit norm within `1e-12`, rotated basis stays orthogonal within `1e-12`. |
| Constructor errors | NaN/infinity in every physical scalar/vector/quaternion, zero direction/quaternion, materially non-unit quaternion, invalid projection/FOV/budget, arithmetic overflow from finite inputs. Verify errors rather than fallback or panic. |
| Body-scale offsets | Regional anchor near `6.371e6 m`, arbitrary orientation, observer `1.7 m` above, proxy tens of metres away, marker `3 km` away. Error `<=1e-7 m` for branch conversion, tighter `1e-9 m` for direct shared-local centering. |
| Astronomical root delta | Root observer/object around `1.5e11 m`, known offsets `1.7 m`, `20 m`, and `0.01 m`. Error `<=1e-3 m`. Include an independently rounded source case to demonstrate source precision limits. |
| Shared extreme ancestry | Repeat local source/observer fixtures with common translation `0`, `1.5e11`, and `1e16 m` and changing common orientation. Result invariant within `1e-9 m`. Include `0.001 m` local deltas which flattening at `1e16` loses. |
| Root round trip | Local→root→local at `1.5e11 m`: `<=1e-3 m`. Do not assert nanometre preservation through a root round trip. At `1e16`, explicitly demonstrate local detail is not recoverable by that route. |
| Invalid topology | Wrong namespace, missing index, root update/reparent, self-parent, descendant cycle, unordered/duplicate state batch, invalid sample time. Rejected edit leaves state/revision/time/depth unchanged. |
| Reparenting | Valid new-parent/new-local-state edit preserves ID, changes ancestry correctly, updates descendant evaluation without changing descendant local states. Earlier borrow-based conversions cannot be retained across edit (Rustdoc compile-fail example). |
| Moving parent | Pure translation and spin update requested edge only; attached-point/source-array values unchanged. Fixed root observer sees motion; co-moving observer sees invariant local geometry. Replay the Section 15 sample sweep. |
| Velocity: translation | Child origin `u=(30_000,0,0) m/s`, local point velocity zero: parent velocity equals u. Inverse recovers zero within `1e-9 m/s`. |
| Velocity: rotation | Identity orientation at sample, `omega=(0,0,2) rad/s`, fixed point `(3,0,0) m`: velocity `(0,6,0) m/s`. Add nonzero u and local velocity and verify all three contributions. |
| Nested motion | Two edges with nonparallel angular velocities and nonzero translated child origin. Independently verify composed omega/u and forward/inverse point+velocity, `<=1e-8 m/s` at local scale; `<=1e-6 m/s` for body-scale scripted fixtures. |
| Derivative check | For an analytic translating/rotating test at t, compare converted velocity to central difference of converted positions at `t±1e-4 s` using local coordinates `<=10 m`; error `<=1e-6 m/s`. Never finite-difference flattened astronomical coordinates as the oracle. |
| Missing motion | Pose-only edges allow pose conversion; kinematic conversion crossing them returns `MissingMotion`. Unknown shared motion above LCA does not prevent local conversion. Same-frame conversion needs no derivatives. |
| Pose/observer handoff | Preserve relative point observations, camera orientation, and physical velocity across root/regional handoff. Position error `<=1e-3 m`, basis error `<=1e-12`, velocity error `<=5e-4 m/s` at the `1.5e11 m` fixture offset. Local/body-only handoff uses the tighter position and `1e-6 m/s` velocity budgets. Verify the approach derivative and zero-velocity endpoints independently. |
| Camera conversion | Observer maps to zero; point straight ahead maps to negative Z; changing observer yaw changes view coordinates correctly. Source-frame mismatch fails; common ancestry movement does not change near vertices. |
| Narrowing | Cast only after centering. Round-trip component bounds at 100 m/1 km/10 km. Reject out-of-range/overflow/error-budget violation and exact boundary behavior. Never silently accept large astronomical `f32` positions. |
| Projection/GPU layout | Validate 32-byte vertex stride, offsets 0/16, 64-byte column-major uniform; forward points yield positive clip w, near→depth 0/far→1 within `2e-6`, right/up screen orientation correct. Parse/validate WGSL headlessly with `naga` matching wgpu 27. |
| Deterministic repeat | Repeat identical queries/samples on one build: same numeric component bits (quaternion comparisons account for equivalent sign); revision/namespace labels excluded from numerical expectations. No dependence on previous evaluation order. |
| Batch/direct equivalence | Grouped prepared output matches direct API within applicable budgets. Bad frame/vertex index returns explicit failure; no partial output is submitted. |

A bounded deterministic property sweep adds useful coverage of inverse/round-trip/order semantics without a dependency: enumerate translations with negative/zero/positive components, noncommuting axis-angle rotations, several points, and depths 1/4/8. Verify A→B→A and direction length with scale-specific tolerances. Do not sample arbitrary enormous numbers expecting local accuracy. A property-testing library is unnecessary initially; introduce one only if shrinking genuinely improves failure diagnosis.

Compile-fail Rustdoc examples should demonstrate point/displacement/velocity/render-type misuse and borrowed-evaluation invalidation. Keep only examples verifying meaningful public invariants; no placeholder coverage tests.

## 17. Representative public API sketches

These are signature sketches, not compilable implementations or promises to expose every helper. Fields remain private; implement only methods required by tests, app, or renderer.

### 17.1 Math primitives

```rust
pub struct LocalPosition { metres: DVec3 }
pub struct Displacement3 { metres: DVec3 }
pub struct Direction3 { unit: DVec3 }
pub struct LinearVelocity3 { metres_per_second: DVec3 }
pub struct AngularVelocity3 { radians_per_second: DVec3 }
pub struct UnitRotation { quaternion: DQuat }

impl LocalPosition {
    pub fn try_metres(value: DVec3) -> Result<Self, MathError>;
    pub fn metres(self) -> DVec3;
    pub fn displaced(self, offset: Displacement3) -> Result<Self, MathError>;
    pub fn displacement_from(self, origin: Self) -> Result<Displacement3, MathError>;
}

impl UnitRotation {
    pub fn identity() -> Self;
    pub fn try_from_quaternion(value: DQuat) -> Result<Self, MathError>;
    pub fn from_axis_angle(axis: Direction3, angle_rad: f64) -> Result<Self, MathError>;
    pub fn compose(self, inner: Self) -> Self;
    pub fn inverse(self) -> Self;
    pub fn rotate_displacement(self, value: Displacement3)
        -> Result<Displacement3, MathError>;
}

pub struct RigidTransform { translation: Displacement3, rotation: UnitRotation }
impl RigidTransform {
    pub fn new(translation: Displacement3, rotation: UnitRotation) -> Self;
    pub fn identity() -> Self;
    pub fn compose(self, inner: Self) -> Result<Self, MathError>;
    pub fn inverse(self) -> Result<Self, MathError>;
    pub fn transform_position(self, value: LocalPosition)
        -> Result<LocalPosition, MathError>;
}
```

The primitive operations are untagged because a rigid transform defines the relation between local bases. Most callers use the tagged frame operations below. Velocity constructors/accessors name their units; a kinematic method never accepts a `Displacement3` as velocity.

### 17.2 Identity, graph, and mathematical pose

```rust
pub struct FrameId { namespace: NonZeroU64, index: u32 } // opaque, Copy/Eq/Hash
pub struct FramePosition { frame: FrameId, local: LocalPosition }
pub struct FrameVelocity { frame: FrameId, relative: LinearVelocity3 }
pub struct FramePose { position: FramePosition, orientation: UnitRotation }
pub struct KinematicPoint { position: FramePosition, velocity: FrameVelocity }

pub struct FrameMotion {
    origin_velocity_in_parent: LinearVelocity3,
    angular_velocity_in_parent: AngularVelocity3,
}
pub struct FrameState { parent_from_local: RigidTransform, motion: Option<FrameMotion> }
pub struct FrameTree { /* private contiguous records, namespace, time, revision */ }
pub struct FrameEvaluation<'a> { /* borrowed tree */ }
pub struct PreparedFrameConversion<'a> { /* endpoints and LCA branches */ }
pub struct PreparedKinematicConversion<'a> { /* endpoints, branches, derivatives */ }

impl FrameTree {
    pub fn new(namespace: NonZeroU64) -> Self;
    pub fn root(&self) -> FrameId;
    pub fn insert(&mut self, parent: FrameId, state: FrameState)
        -> Result<FrameId, FrameError>;
    pub fn reparent_with_local_state(&mut self, frame: FrameId,
        parent: FrameId, state: FrameState) -> Result<(), FrameError>;
    pub fn update_states(&mut self, sample_time_s: f64,
        ordered_updates: &[(FrameId, FrameState)]) -> Result<(), FrameError>;
    pub fn evaluate(&self) -> FrameEvaluation<'_>;
}

impl<'a> FrameEvaluation<'a> {
    pub fn prepare_conversion(&self, from: FrameId, to: FrameId)
        -> Result<PreparedFrameConversion<'a>, FrameError>;
    pub fn prepare_kinematic_conversion(&self, from: FrameId, to: FrameId)
        -> Result<PreparedKinematicConversion<'a>, FrameError>;
    pub fn convert_position(&self, point: FramePosition, target: FrameId)
        -> Result<FramePosition, FrameError>;
    pub fn reexpress_pose(&self, pose: FramePose, target: FrameId)
        -> Result<FramePose, FrameError>;
    pub fn transform_to_root(&self, frame: FrameId)
        -> Result<RigidTransform, FrameError>;
}

impl PreparedFrameConversion<'_> {
    pub fn convert_position(&self, point: FramePosition)
        -> Result<FramePosition, FrameError>;
    pub fn convert_displacement(&self, value: FrameDisplacement)
        -> Result<FrameDisplacement, FrameError>;
    pub fn convert_direction(&self, value: FrameDirection)
        -> Result<FrameDirection, FrameError>;
    pub fn rotation(&self) -> UnitRotation;
}

impl PreparedKinematicConversion<'_> {
    pub fn convert_point(&self, value: KinematicPoint)
        -> Result<KinematicPoint, FrameError>;
}
```

Construction of tagged values is explicit; membership is checked when used by a tree. `FramePose` orientation's source is pose-local axes and destination is the position's frame. `KinematicPoint::try_new(position, velocity)` rejects differing frame IDs. `FrameMotion` fields are interpreted only with their containing edge; graph setters do not accept independent mismatched basis labels.

Prepared conversions retain a shared tree borrow (not merely copied numeric values plus a revision that might be forgotten). Their results are owned numeric values labelled with a destination frame, valid at the documented evaluation instant. The caller can retain those results as state, but not treat them as dynamically tracking later frame edits.

### 17.3 Renderer and app boundary

```rust
pub struct RenderPrecisionBudget { /* checked distance and component-error limits */ }
pub struct RenderRelativePosition { view_metres: [f32; 3] }
pub struct PreparedView<'a> { /* tree borrow, observer pose, budget */ }
pub struct PreparedRenderFrame<'a> { /* source ID, source-space observer, rotation */ }

impl<'a> PreparedView<'a> {
    pub fn new(evaluation: &FrameEvaluation<'a>, observer: FramePose,
        budget: RenderPrecisionBudget) -> Result<Self, RenderPreparationError>;
    pub fn prepare_source(&self, source: FrameId)
        -> Result<PreparedRenderFrame<'a>, RenderPreparationError>;
}

impl PreparedRenderFrame<'_> {
    pub fn try_position(&self, point: FramePosition)
        -> Result<RenderRelativePosition, RenderPreparationError>;
    pub fn write_positions(&self, points: &[FramePosition],
        output: &mut [RenderRelativePosition]) -> Result<(), RenderPreparationError>;
}

// App-owned session data; not a world or renderer singleton.
struct ObserverState { pose: FramePose, velocity: FrameVelocity }
```

Projection and debug vertex packing APIs accept only validated render-relative values. Do not add an overload accepting raw `DVec3` “world positions”. The app assembles frame requests; renderer can borrow math state without accessing world internals.

## 18. File-level implementation plan

All entries below are **future implementation work**. This design task creates/modifies documentation only.

| File | Responsibility and important types/relationships |
| --- | --- |
| `crates/math/Cargo.toml` | Activate workspace `glam` and `thiserror`; add Criterion as a used dev dependency/bench target. No core dependency just for IDs. |
| `crates/math/src/lib.rs` | Crate conventions, precision/units documentation, focused module declarations and public re-exports. |
| `crates/math/src/coordinates.rs` | Dimensional local wrappers and tagged `FramePosition`/displacement/direction/velocity/pose/kinematic point; `MathError` numeric validation. Uses opaque frame identity from `frames`. No render types. |
| `crates/math/src/transform.rs` | `UnitRotation`, `RigidTransform`, `FrameMotion`, checked pose/motion composition formulas. Uses coordinate wrappers; no topology or rendering policy. |
| `crates/math/src/frames.rs` | `FrameId`, `FrameState`, `FrameTree`, `FrameEvaluation`, prepared pose/kinematic conversion, topology/revision/error handling. Instance-owned, not global. |
| `crates/math/tests/reference_frames.rs` | Public-API analytic composition/inverse/topology/velocity/property-sweep tests; private constructor/invariant tests may stay in corresponding modules. |
| `crates/math/tests/precision.rs` | Extreme-scale, cancellation, moving-ancestry, source-quantization, and replay regression fixtures. |
| `crates/math/benches/reference_frames.rs` | Preparation, conversion batches, and state-publication measurements from Section 14. |
| `crates/renderer/Cargo.toml` | Add project math dependency and direct `glam` only for projection operations actually used; Criterion dev dependency, matching naga WGSL-validation dev dependency. No world/simulation dependency. |
| `crates/renderer/src/view.rs` | CPU-only `PreparedView`, `PreparedRenderFrame`, `RenderPrecisionBudget`, `RenderRelativePosition`, and preparation errors. Testable without a GPU. |
| `crates/renderer/src/debug.rs` | Minimal line-list GPU pipeline/depth/staging resources, projection validation and explicit byte packing. Consumes `view` outputs. No domain objects. |
| `crates/renderer/src/shaders/debug.wgsl` | Explicit view-relative position/color to clip contract; embedded with `include_str!`. |
| `crates/renderer/src/lib.rs` | Extend current Renderer with a narrow debug-frame input alongside the UI callback; keep surface/device/egui ownership and lifecycle. Wire debug draw before UI, propagate errors. |
| `crates/renderer/tests/view_precision.rs` | Camera centering/basis, narrowing budgets, layout/projection, batch failure, and shader parse/validation tests. Pure CPU helpers may stay in `view`/`debug` modules if exporting them solely for tests would broaden API. |
| `crates/renderer/benches/view_preparation.rs` | Prepare source/view and checked conversion batches; excludes GPU startup. |
| `crates/app/Cargo.toml` | Add project math dependency; direct `glam` only for fixture constructors if needed. No world/simulation dependency until domain callers exist. |
| `crates/app/src/reference_frames.rs` | App-local `ReferenceFrameDemo`, `ObserverState`, analytic samples, debug primitive data, continuous approach, handoff, and small diagnostics UI. Test pure sampling/handoff helpers here. |
| `crates/app/src/main.rs` | Select the validation flag, update fixture before preparing/rendering, route UI commands, preserve lifecycle/error context. No mathematical conversion implementation here. |
| `Cargo.toml`, `Cargo.lock` | Declare only the used benchmark/shader-validation dependencies centrally, resolve the existing glam selection, update lockfile deliberately. No version upgrades unrelated to Phase 1. |
| `README.md`, `docs/architecture.md`, `docs/performance.md`, `docs/roadmap.md` | Record implemented status, flag/run command, agreed boundaries, measured baseline, and platform validation evidence. |
| `docs/adr/0002-reference-frames-and-precision.md` | At Phase 1 completion record decisions, measured validation, consequences, alternatives, and triggers for revisiting precision/handle policy. Do not claim experimental evidence before it exists. |

Keep coordinate/transform/frame cross-module references within math; they do not require crate cycles. Split further only if implementation size justifies it. Core, world, simulation, and `crates/app/src/redraw.rs` require no functionality changes for Phase 1. Existing CI need not gain benchmark/GPU jobs; all-targets checks compile benchmark code and Linux tests validate headless math.

Implementation order:

1. Numeric wrappers, units, axes/quaternion/rigid transforms and analytic tests.
2. Checked frame tree/evaluation, LCA precision paths, and topology/scale tests.
3. First-order derivative conversion and observer re-expression tests.
4. CPU renderer boundary/budgets, layout/projection/WGSL validation, then minimal debug draw.
5. App fixture, approach/handoff, platform interactive validation, benchmarks, documentation/ADR.

Each step should leave the existing smoke app buildable. Do not use the app fixture to hide mathematical logic that belongs in tested library code.

## 19. Intentionally deferred decisions and review triggers

- **Universe/cross-system addressing:** revisit when a single finite root/branch cannot meet measured precision needs. No sector scheme now.
- **Frame deletion and persistence mapping:** add generation/lifetime rules when a real lifecycle requires removal; runtime handles are never serialized as stable world identity.
- **Automatic observer frame choice and interpolation:** design when navigation/clock policy exists; preserve instantaneous pose/velocity and coherent sampling.
- **Physics:** acceleration transforms, inertial working frames, collision, gravity, integrator, and local-library origin rebasing await actual physics requirements.
- **Terrain/LOD/editing:** body-fixed authoritative coordinates are preserved, but surface topology, parameterization, edit encoding, cache invalidation schemas, and volumetric meshing remain open.
- **Geological deformation/hydrological feedback:** moving rigid frames transport attached content; they do not imply terrain/material coordinates cannot evolve. Those systems may update their own authoritative fields independently.
- **Rendering across the whole system:** distant representations, reverse-Z/log depth/multi-pass choices, precision splits, shadow frames, instancing, and GPU procedural evaluation require separate experiments. Debug range is not an engine invariant.
- **Stricter determinism:** bit-identical cross-platform physical math or generation can be considered per subsystem. Do not promise it through this layer.
- **Cache/parallel optimizations:** root caches, dirty subtrees, precomputed LCA tables, SoA, and concurrent snapshots require measured conversion/tree workloads.

Before implementation, review the numeric envelope, namespace/lifetime contract, derivative-basis conventions, and source-centered rendering formulas together. Those are foundational choices; terrain algorithms and celestial mechanics are not prerequisites.

## 20. Definition of Done for Phase 1 implementation

Phase 1 is complete only when all criteria below have recorded evidence. This specification alone completes none of the implementation criteria.

### Build and quality

- [ ] Stable Rust/Rust 2024, existing six packages, `publish = false`, no project-owned unsafe code, no unrelated dependency upgrades.
- [ ] `cargo build --locked --workspace` passes.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo check --locked --workspace --all-targets --all-features` passes.
- [ ] `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test --locked --workspace --all-features` passes, including compile-fail documentation examples and the concrete matrix/sweep. No test requires GPU/display.
- [ ] `cargo test --locked -p mundaris_math -p mundaris_renderer --release` passes once for numerical/profile agreement; this focused release check stays outside normal CI.
- [ ] `cargo doc --locked --workspace --all-features --no-deps` passes with Rustdoc warnings denied (`RUSTDOCFLAGS=-D warnings` supplied using the host shell's environment syntax).
- [ ] `git diff --check` passes; documentation links, final newlines, UTF-8/LF, and public units/space contracts are checked.

### Mathematical correctness and architecture

- [ ] Checked rigid transforms, unit quaternion normalization, composition/inverse, axis/handedness conventions, and all point/displacement/direction/velocity distinctions are implemented and documented.
- [ ] Single-root frame graph rejects invalid identities/cycles/root edits and mutates transactionally; handle namespaces and append-only limitations are documented/tested.
- [ ] One-instant borrowed evaluations and prepared conversions cannot silently survive tree mutation; unknown derivatives cause explicit kinematic errors.
- [ ] LCA-relative and source-centered paths pass local/body/astronomical/extreme shared-ancestry tolerances. No nearby-render path flattens via root transforms.
- [ ] Moving a parent updates no attached point/local primitive arrays. Both co-moving and independent observers produce the expected observations.
- [ ] Observer pose and velocity re-expression preserves instantaneous physical state within stated budgets. Physical attachment is a distinct deliberate operation.
- [ ] Renderer depends on math, owns narrowing/projection/GPU data, and consumes read-only state. World/simulation remain independent of graphics; app contains only fixture/session/orchestration behavior.
- [ ] No conventional global rebasing, celestial mechanics/domain model, terrain/LOD/generation, physics engine, persistence format, speculative framework, or new crate is introduced.

### Renderer and validation mode

- [ ] `--reference-frames` runs the specified primitive fixture, analytic motion, pause/seek/reset, stress-offset mode, approach, and manual re-expression with numerical diagnostics.
- [ ] Shader/layout tests prove view-relative inputs, 32-byte vertices, 64-byte projection, right-handed forward view, and `wgpu` `0..1` depth. No astronomical `f32` coordinates or duplicate camera subtraction cross to GPU.
- [ ] Near conversion budgets and out-of-range handling are explicit; distant UI markers are labelled and finite. There is one connected tree/observer rather than disconnected scenes.
- [ ] Replay over `0..600 s` at both offsets meets Section 15's numerical stability; approach/handoff meets position/orientation/velocity budgets.
- [ ] Windows x86-64 and Linux x86-64 both pass native build/check and headless numerical test requirements, including the focused release check. Existing Linux quality and Windows compatibility CI jobs pass; local Windows tests need not become a duplicated full CI job.
- [ ] The Section 15 interactive sequence is actually performed on both native platforms with OS/GPU/backend recorded: stable nearby geometry, no handoff snap, responsive UI, resize/minimize/restore, and clean shutdown. If a graphical session or second host is unavailable, record the missing evidence and leave this criterion open.

### Performance and documentation

- [ ] Focused math and CPU view benchmarks exist, compile under all-targets checks, and are run locally with a reproducible baseline on at least one supported native target.
- [ ] Benchmark groups separate preparation/update/batch cost and compare prepared batches with direct queries. No window/device bootstrap timings or normal-CI benchmark execution.
- [ ] Hot conversion/batch paths reuse storage and allocate no per-point/path data; measured structural surprises are investigated. No arbitrary timing threshold is substituted for evidence.
- [ ] README/current architecture describe implemented scope accurately; performance notes record workload/hardware/compiler/results, numerical tradeoffs, and remaining platform evidence.
- [ ] ADR 0002 records the validated reference-frame/precision decision and explicit revisit triggers; deferred decisions remain deferred.
- [ ] Correctness, architecture, and performance review gates have been completed against this checklist before work expands to Phase 2.
