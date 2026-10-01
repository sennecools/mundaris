# Mundaris — Phase 3: Gravity, Orbits & Celestial Rendering

> **Status:** implemented and Windows-validated; Linux/current-revision CI acceptance remains open — see [Phase 3 validation](docs/phase-3-validation.md).
>
> **Targets:** stable Rust, Rust 2024, native Windows x86-64 and Linux x86-64.
>
> **Prerequisites:** [Phase 1](MUNDARIS_PHASE_1_REFERENCE_FRAMES.md), [Phase 2](MUNDARIS_PHASE_2_CELESTIAL_MODEL_AND_TIME.md), [ADR 0002](docs/adr/0002-reference-frames-and-precision.md), and [ADR 0003](docs/adr/0003-celestial-domain-and-time.md).
>
> **Architecture:** [Engine design](MUNDARIS_ENGINE_DESIGN.md), [bootstrap](MUNDARIS_PROJECT_BOOTSTRAP.md), and the existing six-crate workspace.
>
> **Acceptance:** existing platform gaps remain open. Design is not implementation, benchmark or native-runtime evidence.

## 1. Outcome and decisions

Phase 3 makes Mundaris a visible primitive solar-system simulator. A small connected celestial system evolves under mutual Newtonian gravity and displays physically sized debug spheres, navigation markers, selection/focus, actual historical trails and numerical diagnostics. The final gravity validation contains no prescribed orbital animation.

| Decision | Phase 3 choice |
| --- | --- |
| Gravity | Unsoftened Newtonian point masses, SI, f64, symmetric unordered pairs, deterministic serial accumulation. |
| Integrator | Kick-drift-kick (KDK) leapfrog, full-time published velocities; equivalent to velocity Verlet for position-dependent gravity. |
| Timestep | One fixed step per branch: circular fixture 10 s; hierarchical fixture 60 s. Playback never changes it. |
| Advancement | Integer ticks and explicit wall demand; at most 512 work steps per update. |
| Overload | Preserve admitted backlog; visibly halt new demand at a bounded ceiling. Never increase physical timestep. |
| Reverse/seek | Bounded recent full-state snapshots; reset and deterministic positive-step replay outside retention. |
| Rendering | Reusable indexed icosphere topology, source-centred f64 preparation, debug shading, reverse-Z depth, screen markers/labels. |
| Trails | App-owned bounded f64 actual committed history; inertial or explicitly labelled simultaneous body-relative history. |
| Threading | Single-threaded; no parallel reduction, job system or new crate. |

```text
wall elapsed × playback rate -> requested time/tick
                               -> zero or more fixed integration steps
CelestialSystem -> simulation scratch -> coherent world commit
                                          -> CelestialFrameProjection
                                          -> Phase 1 FrameTree evaluation
                                          -> renderer -> GPU + UI
```

Bodies are free particles in one inertial system basis. No fixed star, transform-parent orbit, force based on name, or rotating-frame gravity is introduced.

## 2. Phase 2 audit

### 2.1 Baseline and evidence

The audit covers current working-tree source, manifests/lockfile, all tests/benchmarks/shaders, validation applications/lifecycle, specifications, ADRs, CI, validation and performance notes, including existing uncommitted Phase 2 work. Build products under `target/` are not repository source. Existing changes must be preserved.

Current links: app -> math/renderer/world/simulation; renderer -> math; world -> math; simulation -> math. Core is documentation-only. Simulation can add world/glam without a cycle. The selected wgpu 27/winit 0.30/glam 0.30/egui 0.33 stack remains appropriate.

Audit rerun on Windows: `cargo test --locked --workspace --all-features` passed **42 runtime tests and six compile-fail documentation tests**. `cargo tree --workspace --depth 1` agrees with manifests. Historical release/runtime/benchmark evidence is in [Phase 1 validation](docs/phase-1-validation.md), [Phase 2 validation](docs/phase-2-validation.md), and [performance](docs/performance.md); those records are distinct from checks rerun for this design.

### 2.2 Boundary findings

| Required boundary | Evidence / conclusion |
| --- | --- |
| World owns authoritative celestial state | `world/src/body.rs` private properties/state and `world/src/system.rs` contiguous bodies, instant, revision and validated mutations. Preserved. |
| Simulation controls evolution/time policy | `simulation/src/time.rs` requests time only; physical evolution is intentionally absent and can be added here. |
| Frames are derived | `world/src/frame_projection.rs` copies world state into its private tree, exposing only `&FrameTree`. |
| Renderer is derived/disposable | Renderer owns view/staging/device/surface/debug buffers, with no world/simulation dependency. |
| No authoritative FrameId | Neither BodyState nor CelestialBody stores one; projection associations and observer state do. |
| Translation/rotation separate | Root-child translating anchors have identity rotation/zero spin; their fixed children have zero translation and independent spin. |
| No inherited foreign spin | Separate root anchors; world frame tests change A spin without changing B or A centre. |
| Properties independent of kinematics | edit_body/edit_properties touch metadata/properties only; edit_state touches state only. Tested. |
| Coherent full publication | update_states validates IDs/duplicates/completeness/revision before writes; a new instant requires every body. Same-time subset authoring is intentional. |
| Instant avoids cycles | One checked SimulationInstant in math; world/simulation re-export it. Playback stays in simulation. |
| Playback does not evolve state | App explicitly samples its analytic producer, commits world, then publishes frames. Controller has no world access. |
| Identity independent of frames | BodyId is caller-namespaced append-only world identity; rebuild uses a fresh tree namespace and preserves bodies. |
| Rebuild without state loss | Frame tests/app verify equivalent projections with changed handles and unchanged world. |
| N-body readiness | Stable dense bodies() enumeration and mass/position/velocity accessors plus full update_states suffice; solver needs no frame query or renderer mutation. |

No ownership defect warrants replacing Phase 2. Infallible BodyState::new is correct because its inputs are checked private math types. Arbitrary-order world batches and ordered frame batches coexist: projection produces its own ordered list. Duplicate scratch is cleared after rejected duplicates.

Projection append preflights capacity/revision; private depth-two topology validates subsequent insertion invariants. World commit and projection publish are **separate transactions**. A projection failure leaves a valid newer world and older projection; app must not combine their revisions while rendering.

### 2.3 Genuine inconsistencies and integration limits

1. At audit baseline, Phase 2's header incorrectly said implementation had not begun and its ownership section placed SimulationInstant in simulation, contrary to code/resolved ADR. Phase 1 final status also said Phase 2 had not begun. Documentation now links evidence and resolves ownership without completing outstanding acceptance boxes.
2. The baseline engine roadmap assigned orbital motion to Phase 2/basic representation to Phase 3 and conflicted with the current roadmap. Phase 2 motion is analytic only. Documentation now aligns Phase 3 to gravity/orbits/minimal rendering and reserves ADR 0004 for gravity/integrator/time/history, leaving terrain algorithms open.
3. SimulationInstant accepts a finite rounded unchanged sum at huge epochs: finiteness is not tick resolution. A physical runner must reject collapsed adjacent instants without changing generic math time policy.
4. Current app render-time elapsed accounting can include minimized/occluded time; resume resets the tick but restoration is insufficient for physical stepping. Failed bounded analytic sampling can leave requested time ahead of authoritative time, as documented. Phase 3 needs explicit lifecycle/demand/failure handling rather than copying this loop.
5. Full Windows Phase 2 visual sequence, Linux native/release/interactive and current-change CI remain open. ADR 0002's prerequisite is not waived. Resolve gates before implementation or record an explicit reviewed change.

The audit above records the pre-implementation baseline. The implementation request authorized Phase 3 and an explicit reviewed prerequisite deferral, recorded in ADR 0004. Projection capacity/revision preflight regressions are now covered. Existing publication benchmarks often use coincident body centres: valid publication inputs, invalid gravity fixtures.

## 3. Scope and invariants

Include mutual gravity, one fixed integrator, runner/playback, branch reset/history/replay, diagnostics, deterministic fixtures, debug spheres/markers/labels, selection/focus/orbit/zoom, actual trails, headless tests, dedicated validation, benchmarks and ADR.

Preserve world/render separation, disposable meshes/buffers, frame-independent identity, graphics-independent simulation, read-only rendering, f64 physical state, source-centred GPU narrowing, no global origin mutation, no attached-object rewrites, replaceable algorithms and measured optimization. Debug sphere topology selects no future terrain/LOD algorithm.

Explicit exclusions: terrain, planetary LOD, procedural generation/editing, vegetation, oceans, water/hydrology, atmosphere/clouds, tectonics/geology, stellar evolution, universe catalogue, persistent save format, multiplayer, ECS migration, jobs/Rayon/distributed simulation, GPU gravity, Barnes-Hut/FMM/spatial trees, gameplay, spacecraft/thrust/drag/rigid-body spacecraft physics, trajectory planning/prediction, patched conics/sphere-of-influence logic, collisions/response/fragmentation, Roche limits, tides/deformation/locking, oblateness/J2, relativity, radiation pressure, PBR/physical planetary lighting, luminosity/spectra, textures, eclipses/shadows, HDR/bloom. The Hill-radius sanity check below is fixture design, never force-switching logic.

## 4. Newtonian gravity

### 4.1 Units and deterministic pairs

Use `GRAVITATIONAL_CONSTANT_M3_KG_S2: f64 = 6.67430e-11`, CODATA 2018's recommended central value, as a fixed explicit model constant. G is measured, not mathematically exact; all fixtures use the specified value. No per-body authoritative mu, radius-dependent force or user G slider.

Mass kg, position m, velocity m/s, acceleration m/s² and step s are f64; numerical vectors are DVec3. Existing authoritative checked wrappers retain f64. Treat the finite system basis as inertial for these isolated fixtures. Initial barycentric translation/boost is allowed; never recenter evolved world state to hide drift.

Zero acceleration outputs, then lexicographic dense unordered pairs `i=0..N`, `j=i+1..N`:

```text
d = x[j] - x[i]
r = hypot(hypot(d.x, d.y), d.z)
u = d/r
s = (G/r)/r
a[i] += u*(s*mass[j])
a[j] -= u*(s*mass[i])
```

No self pairs. Both bodies react, including the star. Shared displacement/distance halves duplicate geometry work and enforces analytic third-law symmetry, though mass-weighted floating-point contributions need not cancel bit-exactly. Accumulate ordinary f64 components in that specified serial order. No hash iteration, ambient time, parallel reduction or fixed primary. Checked pair count is N*(N-1)/2; one warmed KDK step needs one new force pass.

Robust distance avoids squared-distance overflow/underflow. Check subtraction, distance, inverse-square scalar, contributions and accumulations. Raw-slice public evaluation validates lengths, finite positions and positive finite masses. Failure may leave partially written scratch, which must not be committed. Do not skip a bad pair or form mass products unnecessarily for acceleration.

### 4.2 Separation, admissibility and softening

**No softening.** The well-separated fixtures do not require it. Reference radius never enters gravity, even if reference spheres overlap. Such overlap is not modelled collision/interior physics.

Exactly coincident centres return `CoincidentBodies { first, second }`. For small nonzero r use inverse-square force while representable; subnormal distance, overflowing inverse-square/contribution or underflow losing a required nonzero scalar returns `UnrepresentableGravity`. No epsilon clamp, zero-force substitution or corrupt-state repair. Arbitrary finite world data need not be admissible for gravity.

Reuse pair scalars for an explicit resolution guard `chi = h*sqrt(G*(mi+mj)/r³)`, evaluated with checked/scaled arithmetic. Require chi <= 0.02 at old and drifted positions; otherwise return `UnresolvedEncounter` and pause at the last good state. This radius-independent session envelope is not a universal stability theorem. A smaller h requires a separately configured branch; no adaptive catch-up. Fast flybys are outside validated workloads.

### 4.3 Complexity and scale

Normal Phase 3 N is 3–16; 64 is a useful extension. O(N²) forces/O(N) scratch are appropriate. Pair counts at 3/16/64/256/1024 are 3/120/2016/32640/523776. Large counts are scale probes, not product requirements.

Revisit when real workloads need sustained N >=256 and measured gravity exceeds 25% of the agreed CPU budget at required throughput, or N >=1024 becomes normal. A slow 1024 benchmark alone does not justify approximation. Report steps/s and sustainable playback approximately h*steps/s including publication/history. Faster exact loops, compensation, parallelism and approximate algorithms require separate measurements.

## 5. Integrator decision

| Method | Orbital properties/cost | Decision |
| --- | --- | --- |
| Explicit Euler | First order, non-symplectic, secular orbital energy/radius growth; one force pass. | Reject. |
| Semi-implicit/symplectic Euler | First order; bounded energy often possible, but biased phase and not self-adjoint; central kicks preserve angular momentum analytically. One pass. | Comparison only. |
| Velocity Verlet | Second order, symmetric/symplectic for position-only forces; bounded modified-Hamiltonian energy in resolved smooth orbits; full-time velocities; one new cached force pass. | Appropriate/equivalent to KDK here. |
| Leapfrog/KDK | Same second-order orbital behaviour; clear half-kick/drift/half-kick and full-time boundaries; one new force pass. | Select. |
| RK4 | Fourth-order short-run accuracy; four force evaluations; neither symplectic nor self-adjoint, possible long-run drift. | Future comparison for other requirements, not implemented. |

```text
v_half = v_n + (h/2)*a(x_n)
x_next = x_n + h*v_half
a_next = gravity(x_next)
v_next = v_half + (h/2)*a_next
```

Drift all bodies before the new force pass; publish full-time v_next, never v_half. Validate the entire candidate before world mutation. Promote cached a_next only after commit; it is derived/recomputable. Initialization/regather force evaluation is counted separately.

KDK is symmetric under signed h in exact arithmetic; floating-point reversal is approximate and chaotic dynamics amplify errors. User reverse uses snapshots/replay, not that symmetry as exact history. An optional signed-kernel test does not add negative-dt playback.

Use one concrete workspace/kernel, no trait hierarchy/plugin/registry. Future integrators can consume the same dense arrays and return full-time candidates without changing identity. Velocity-dependent forces, adaptive/individual steps, higher-order symplectic methods and encounter regularization require a new decision.

### Constant-spin kinematics

No torques/inertia dynamics. Keep authored system-axis omega constant and transport orientation with `q_next = axis_angle(omega/|omega|, |omega|*h) * q_n`; left multiplication is required for system axes. Zero omega copies q. Use checked UnitRotation composition/normalization and reject non-finite angular displacement. This is spin kinematics, not tidal locking. Diagnostics exclude rotational energy without an inertia model; snapshots include orientation/omega.

## 6. Fixed time and demand

### 6.1 Distinct concepts

| Concept | Meaning/owner |
| --- | --- |
| Wall-clock time | Monotonic host durations captured by app for scheduling/input, not physics/calendar state. |
| Render cadence | Presentation/redraw frequency; approximately 16 ms scheduler is not h. |
| Playback rate | Signed finite requested simulation seconds per wall second, existing PlaybackRate. |
| Requested time | Possibly fractional/ahead demand; not proof of evolution. |
| Authoritative instant | CelestialSystem.sample_time for every committed body and projected frame. |
| Integration timestep | Positive fixed session h independent of rate/FPS. |

Simulation owns FixedStepRunner and demand accumulation; app supplies durations/commands. Kernel never reads wall time or render dt. Pumping before a render is acceptable: rendering schedules work opportunities, not trajectory. Headless callers advance target ticks independently.

Represent progression by integer u64 k and `t(k)=epoch+(k as f64)*h`, reconstructed each tick rather than repeatedly adding h. Reject overflow, k above 2^53-1, non-finite or collapsed adjacent times. Require `ulp(t) <= h/1024`; adjacent difference agrees with h within `max(1e-12*h,4*ulp(t))`. Recheck as time increases. No geological-epoch precision promise.

h is configured once per branch. Circular h=10 s; hierarchy h=60 s. A changed h captures a new baseline at current instant and clears history/trails/diagnostic reference. No live h slider masquerading as acceleration. Smaller-step convergence validates shortest-orbit resolution.

### 6.2 Fractional accumulation/FPS independence

Reuse TimeController/PlaybackRate as control/requested values, not a repeatedly incremented physical counter. Runner retains a rate-segment anchor and cumulative exact Duration (checked addition), computes `anchor+total_elapsed.as_secs_f64()*rate`, and sets requested target. Identical admitted total duration and command timestamps therefore produce identical demand regardless of frame/UI partitions. Existing Phase 2 advance_wall_time behaviour stays intact for analytic callers.

Forward target ticks use floor of `(requested-epoch)/h`; retain/display fractional remainder. Negative playback uses ceil to request decreasing ticks only after a full reverse h. Store explicit target tick; no shortened remainder step or epsilon-triggered early tick. At exact boundary a tick is due.

Provide seek_tick(k). Seconds-seek shows nearest tick/result seconds/quantization delta before confirmation, ties toward lower tick; reject pre-origin requests. UI wakeups account only elapsed time since previous capture, not an extra step. FPS tests partition equal integer-nanosecond totals (final remainder included), not unequal rounded 1/FPS totals. Same baseline/h/target yields identical physics; different host measurements need not imply identical demand.

### 6.3 Work, backlog and lifecycle

Default max_work_steps_per_update=512, including integration, snapshot restoration and private replay. Render once after pumping. Timing measurements do not decide dt. Render may continue while authority lags request; show both times/debt/achieved rate.

Default admitted backlog cap=65,536 ticks, configurable resource policy. Check candidate debt before admission. If beyond cap, reject that entire new interval, retain previous admitted demand, and visibly latch **demand halted: overload**, reporting rejected simulation seconds/interval. Require explicit resume/lower rate. Previously admitted debt drains at fixed work cap. Below ceiling show lagging and preserve debt. Backlog is counters, not growing allocation; never silently drop it or increase h.

Overload is an admission latch, distinct from explicit pause. Clearing it opens a fresh duration segment at the retained requested target and preserves admitted debt, rather than applying paused Resume's authority anchor. Diagnostics may sample app wall time while admission is halted. Different overload admissions may produce different targets; determinism compares identical admitted requests/step counts, not rejected wall time.

At h=60, 1000x requests 16.67 steps/s; 1,000,000x requests about 16,667. At 60 updates/s work cap permits at most 30,720 work units/s before CPU limits, not measured throughput. Slow CPUs show honest lag/overload.

Minimize/occlusion/zero-size/OS suspension automatically suspends work/demand, clears debt with explicit lifecycle status, and resets host timestamp on restoration; hidden intervals are excluded. No background jobs. Surface timeout/lost-frame retries never repeat a committed step.

### 6.4 Controls

| Command | Behaviour |
| --- | --- |
| Forward | 0.1x/1x/10x/100x/1000x; hierarchy additionally 100000x/1000000x. Request different counts of identical steps. |
| Pause/0x | Stop all stepping; explicitly cancel whole/fractional debt and reanchor request to authority; report cancellation. Keep explicit paused flag separate from rate. Camera/UI work. |
| Resume | Fresh duration segment from authority at selected rate; no hidden paused interval. |
| Single forward/back | Pause/clear demand; exactly k+1 integration or k-1 restore/replay; remain paused. At origin report boundary. |
| Same-sign rate change | Retain debt/fraction; open duration segment at requested time. Rate alone changes no physics. |
| Direction change | Clear old demand explicitly, reanchor to current tick, collect full steps in new direction. |
| Reset branch | Restore branch baseline/tick zero, preserve IDs/properties, clear history/trails/debt, reseed diagnostics; 1x paused. Retain selection/attachment. |
| Seek | Pause/clear demand, validate target, snapshot or private reset/replay; complete paused. Cancel retains live world. |
| Load original fixture | Explicit scenario replacement restores original properties/initial states with fresh system namespace and overview/selection; separate from reset/undo. |

## 7. Reverse/history/seek

Negative h is cheap/self-adjoint mathematically but cannot restore past floating-point state. Reset/replay uses O(N) baseline memory but grows costly far from origin. Checkpoints improve latency but add retention/invalidation policy; unlimited snapshots grow O(N*ticks).

Choose **one immutable branch baseline and a bounded ring of recent full BodyState snapshots**, with positive-step reset/replay outside retention. No checkpoint database, serialization, ephemeris or prediction system. History belongs to simulation session, not renderer or live world truth.

Store complete states/tick after every successful commit, branch IDs/masses once. Cap 2048 complete ticks and 16 MiB payload, whichever is smaller, including recorded metadata and actual storage layout. Check accounting and reserve at setup; at least current/previous states must fit or reject configuration. Three bodies' 104-byte semantic state payload is approximately 624 KiB for 2048 records before metadata/layout; 1024 bodies hit byte cap much earlier. Report retained ticks/time. Baseline is separate O(N) memory.

Negative playback restores retained complete states exactly on the same target/build, recomputes accelerations in fixed order, and discards later snapshots. Later forward playback integrates again rather than reusing a potentially divergent future. At retention boundary begin private reset/replay to older target and display progress. Hold last live coherent world visible; freeze additional wall demand during replay, restarting a fresh duration segment afterward. Reverse outside retention may visibly stall; it is not free analytic sampling. At tick zero visibly auto-pause; never evolve before branch epoch.

Seek-current needs no commit. Seek-retained restores; otherwise replay baseline -> target under the 512-work cap in private scratch, rebuilding recent history. No intermediate replay state is published into live world/frames/diagnostics. Cancel or replacement seek leaves live world untouched; restart private replay explicitly. Completion commits a full target batch, replaces runner history/cache/tick, then projects. Runtime revisions stay monotonic and are not rewound. Same baseline/h/order/target reproduces physical component bits on the same executable/target; not cross-platform bits, old revision values or GPU pixels.

Replay latency is steps divided by measured throughput: show remaining work/estimate without allowing timings to alter physics. Missing recent history is normal replay fallback. Invalid/pre-origin/nonfinite/unrepresentable seek fails visibly; missing/mismatched baseline is a session error, never invented state.

## 8. Ownership/scratch/publication

| Component | Ownership |
| --- | --- |
| Physical bodies/current states/time/revision | Existing mundaris_world CelestialSystem, validated mutations. |
| Gravity/KDK/acceleration/demand/runner/baseline/history/replay | mundaris_simulation, no frame/renderer access. |
| Pure E/P/L/COM calculations | simulation; app caches sampled diagnostics. |
| Body -> frames and derived tree/revision | Existing world CelestialFrameProjection. |
| Selection/camera/UI/styles/trail history | app session/debug state. |
| Sphere/GPU/depth/precision/marker/trail preparation | renderer, math-only project dependency/read-only inputs. |
| Coordinates/rotations/frame math/instant value | math; no gravity or playback policy. |
| Core | No new responsibility. |

Gather stable bodies() order into reused arrays: IDs/masses/positions/velocities/orientations/omega, old acceleration, candidate positions/half-velocities/velocities/orientations/next acceleration and full BodyStateUpdate staging. Use DVec3/f64 with unit-labelled fields. Do not redesign world into SoA. Candidate buffers separate failing work from committed caches.

Initialize/regather O(N) and compute acceleration. Tag caches with expected world revision/time/count/IDs. Before pumping detect external mutation; reject stale session until explicit paused rebranch. Live append requires full rebranch; deletion/reuse remains deferred.

Determinism guarantees identical physical components for identical initial data, insertion order, h and step count on the same executable/target, independent of render FPS, UI wakeups and diagnostic queries. Enumeration/pair order are fixed; no wall reads/randomness/parallel reductions occur in stepping. Windows/Linux must satisfy the same numerical envelopes, not cross-platform bits. Changing accumulation order is a numerical algorithm change requiring review/measurement.

Each normal step: calculate/validate all candidates; create checked math/full-state batch at next integer instant; call existing update_states once; only after success promote scratch/tick/revision/acceleration and append snapshot; invoke optional narrow read-only post-commit callback `(tick,&CelestialSystem)` for app trail sampling. No rendering dependency in simulation.

No heap allocation per step after capacities are prepared: ring slots/candidates/update vectors are reused. Topology/branch creation may allocate. Per-step world publication is O(N), gravity O(N²); do not add unsafe setters. A later once-per-update world commit needs measurement/design, not incidental optimization.

App projects once after pump/restore/edit, checks represented revision and time against world, then prepares one borrowed coherent view. On projection failure pause, suppress mismatched celestial draw and show an error; rebuild/retry disposable projection. World remains last good committed physics. Rebuild uses fresh namespace and remaps observer attachment by BodyId/frame role, not persisted FrameId.

## 9. Editing/branch semantics

Validate drafts at command boundary before stepping, including proposed solver admissibility, branch-generation and scratch/history capacity before committing the world edit. Rejected edits leave world/request/history/trails/diagnostics/baseline unchanged, including playback. Successful physical edits pause/cancel demand, retain current authoritative instant, capture complete current data as new branch epoch/tick zero, and increment separate checked branch generation. Promote prevalidated branch buffers after world success so a later recoverable numerical error cannot pair edited state with stale future history; projection failure remains separately handled derived failure.

| Edit | Consequence |
| --- | --- |
| Mass | Recompute all future accelerations; no implicit position/velocity/spin change. |
| Radius | Geometry only, never gravity; conservatively rebranch for baseline/display context. |
| Position | Explicit placement/potential change, no inferred velocity. |
| Velocity | Explicit kinetic/future change, no position change. |
| Orientation | Fixed frame/visual rotation only, no centre/gravity change. |
| Angular velocity | Constant-spin future changes, no translational force/orientation jump. |
| Name only | Metadata; update expected/projected revision, retain physical history/trails/baseline. |

Physical edits clear history/replay/trails/diagnostic drift baseline and seed epoch state. Old future/past is unavailable in new branch; show origin. Reset restores edited baseline, not original fixture; fixture reload is explicit replacement. Authoring energy/momentum jumps are not integrator drift. Radius/orientation rebranch may reset diagnostics but must pass identical-translation tests. No undo/timeline framework.

## 10. Initial conditions

### 10.1 Analytic circular oracle

M=1e24 kg, m=1e20 kg, separation R=1e7 m, radii 1e6/1e5 m, h=10 s, zero spin for orbital tests. `mu=G*(M+m)`, `v=sqrt(mu/R)`, `n=sqrt(mu/R³)`, `T=2*pi/n ≈2.432e4 s` (derive precise T; rounded value is not input).

```text
x_primary   = -(m/(M+m))*R*X
x_secondary =  (M/(M+m))*R*X
v_primary   = -(m/(M+m))*v*Y
v_secondary =  (M/(M+m))*v*Y
```

Independent analytic relative orbit is R*(cos(n*t),sin(n*t),0) with its derivative; E=-G*M*m/(2R); L is reduced mass times R*v along +Z. COM/P zero to initial rounding. Also test common initial offset (1e9,-2e9,3e9) m and boost (100,-200,50) m/s against independent straight-line COM. Never correct it continuously.

n*h≈0.00258, second-order scale≈6.7e-6. Required eccentric supplement e=0.3, semimajor axis R: start pericentre R*(1-e), speed sqrt(mu*(1+e)/(R*(1-e))), barycentrically split as above. This exposes energy oscillation hidden by circular cancellation. No fixed primary/guessed speeds.

### 10.2 Hierarchical visible fixture

Familiar scales, not a calibrated Solar System ephemeris:

| Body/display style | Mass kg | Reference radius m |
| --- | --- | --- |
| Solace, warm unlit star | 1.98847e30 | 6.957e8 |
| Aurelia, blue planet | 5.9722e24 | 6.371e6 |
| Luma, grey moon | 7.342e20 | 3.74e5 |

h=60 s, outer separation A=1.5e11 m, inner r=1e8 m. Initialize star versus combined planet/moon mass with circular barycentric formula at A, then split planet/moon barycentre with inner circular formula at r. Outer tangent +Y; inner tangent cos(5°)*Y+sin(5°)*Z, initial inner separation +X. Add outer barycentre state to both inner states. This produces reacting star, zero total COM/P to rounding and inclined satellite. Every subsequent body follows all pair forces.

Outer T≈3.17e7 s (~367 days); inner T≈3.15e5 s (~3.64 days); exact periods derive from constants. Moon is ~0.067 of approximate Hill radius A*cbrt((Mp+Mm)/(3Ms))≈1.5e9 m, safely conservative; initial reference spheres do not overlap. Solar perturbations mean lunar radius/local two-body energy are not exact constants.

Independent orientations/system-axis spins: star 25 days, planet 24 hours/23.4° authored tilt, moon 4 days/different axis. Spin is visual kinematics, no tidal-locking claim. Names/styles never alter force. No ongoing analytic position sampler.

At 1000x moon period takes about five wall minutes. Higher presets expose outer motion; 1,000,000x takes ~32 wall seconds per outer orbit if throughput sustains it. Show achieved rate honestly.

Scale benchmarks use deterministic noncoincident index-derived lattice/ring data with positive masses and admissible short h, no RNG. Optional equal-mass/extreme mass-ratio pair is useful; chaotic many-body motion is not the accuracy oracle.

## 11. Diagnostics and numerical acceptance

### 11.1 Definitions/sampling

Use full-time states in inertial axes:

```text
K=sum(0.5*m*dot(v,v)); U=-sum(i<j,G*mi*mj/rij); E=K+U
P=sum(m*v); Mtotal=sum(m); C=sum(m*x)/Mtotal; Vcom=P/Mtotal
Lcom=sum((x-C) cross (m*(v-Vcom)))
```

Diagnostics use deterministic compensated scalar/component sums, independent of force accumulation. Guard arithmetic; avoid unnecessary mass-product overflow via (G*mi/r)*mj and COM anchored sums/mass fractions. No rotational energy without inertia.

Baseline nonzero scales Qp=sum(m*|v|), Ql=sum(m*|x-C|*|v-Vcom|). Normalize drift by these, not near-zero P/L. Bound-fixture energy uses |E0|; near-zero E uses K0+|U0|, explicitly labelled. Display absolute drift too. COM residual against C0+Vcom0*(t-t0).

Tests sample every tick/stated dense cadence. Production UI diagnostics at most once per 250 ms admitted wall time, immediately on pause/reset/edit/seek; show sampled tick/time. No O(N²) diagnostics every substep. Also show moon-relative distance/speed/local Kepler energy labelled not conserved under third-body perturbations; total energy can hide satellite error.

Simulation panel: requested/authoritative time, branch/tick, rate/pause/overload, h, steps/update, debt/replay, bodies/pairs/force passes, achieved rate, E/P/L/COM drift, selected state/speed/mass/radius/spin. Renderer panel separately: triangles/markers/trail segments, narrowing/projected error, culled/subpixel counts, preparation time, observer frame/depth. Focus never changes simulation diagnostics.

### 11.2 Prospective acceptance bounds

These remain the required acceptance bounds. Actual measured maxima/convergence are recorded in [Phase 3 validation](docs/phase-3-validation.md); no tolerance was loosened. First require finite values; orbital accuracy never uses exact float equality.

| Fixture/duration | Metric/bound and rationale |
| --- | --- |
| Circular h=10, 100 periods, ordinary tests | Max relative separation-radius drift <=2e-5, several times (n*h)² with bounded oscillation. |
| Circular same run | Max abs((E-E0)/E0) <=1e-7; circular energy cancellation often higher-order-small, allowing rounding but not heating. |
| Circular first 20 returns | Each measured period error <=1e-5*T; test-only unwrapped-angle crossing interpolation, never shortened physics step. |
| Circular 100 periods | Analytic accumulated phase error <=0.003 rad, allowing second-order secular phase bias. |
| Circular 1000 periods, focused release | Same radius/energy bounds, phase <=0.03 rad; approximately 2.43 million steps. |
| Circular/eccentric runs | Normalized P/Qp and L/Ql drift <=1e-10; analytic central-force invariance with roundoff allowance. |
| Circular boosted/unboosted, 100 periods | COM straight-line residual <=1e-8*R=0.1 m; independent expected motion/source quantization. |
| Eccentric e=0.3 h=10, 100 periods | Max relative energy error <=2e-5; stronger pericentre curvature but bounded energy. |
| Eccentric 1000 periods release | Max energy <=2e-5 and <=2*max_100+1e-9; reject secular drift rather than relying on endpoint. |
| Circular h=10 versus h=5, 20 periods | Analytic phase error fine/coarse ratio [0.20,0.35] at common final instant; second-order convergence. |
| Hierarchy h=60, 2 outer periods release | E drift <=2e-6, normalized P/L <=1e-9; hundreds of moon periods, ~1.06 million steps. |
| Hierarchy same run | Moon separation [0.9*r,1.1*r], COM residual <=1e-8*A=1500 m; broad bounds allow genuine perturbations. |
| Optional signed-kernel circular 10 periods forward/back | Position residual <=1e-8*R, velocity <=1e-8*v; approximate symmetry, not user restoration. |
| Constant spin 100000 steps, omega<=1e-3 rad/s,h<=60 | Norm drift <=1e-12, basis error against analytic rotation <=1e-9; q/-q equivalent. |

Period tests unwrap relative XY atan2 and interpolate full-return crossings; report interpolation error separately. T/h need not be integer, so exact state return at T is not required. Analytic initial E/L/force and convergence complement conservation. Native debug/release use same normal numerical bounds.

## 12. Basic celestial rendering

### 12.1 Sphere/transform/GPU contract

One deterministic level-3 subdivided indexed icosphere: 642 unit vertices/1280 triangles, generated once in renderer, unit normals/outward winding/indices validated, immutable indices uploaded once. It is a debug primitive, not terrain topology/LOD or asset pipeline. Planar facets approximate reference sphere; document chordal error <=0.005*radius.

App supplies domain-free CelestialRenderBody: fixed-frame handle, copied f64 reference radius, color, selected/unlit flags. App maps request index to BodyId. Renderer needs no world identity dependency. Model semantics: physical radius scale -> authoritative fixed-frame transform -> observer conversion; no competing renderer orbit/state.

For each unit vertex u form radius*u in f64; subtract observer in that source frame before camera rotation/narrowing, using Phase 1 prepared conversion. Do not narrow a huge centre/model matrix or flatten local points through root. Stream derived positions/view normals into reused GPU staging and draw reusable indices against each vertex slice. Normals rotate without translation. Small N justifies this clear CPU path; instancing/caches require measurement.

Vertex layout: view-relative `(x,y,z,1)` and camera-basis `(nx,ny,nz,0)`, two Float32x4, stride 32 bytes. Per-draw uniform 32 bytes: color vec4<f32>, flags vec4<u32>, explicit padding/meaning. Projection 64-byte column-major matrix. Safe little-endian packing as existing debug code, no unsafe casts/new packing dependency. Full-frame validation/poisoning prevents partially prepared uploads.

### 12.2 Radius versus visibility/precision

Physical sphere draw when projected diameter >=2 physical pixels and precision passes. Subpixel bodies retain labelled markers/selection. Radius is never exaggerated authoritatively; no exaggerated-sphere mode is required.

Visibility/distance/bounds/label projection use f64 observer-relative data. Actual vertex round-trip narrowing error must be <=min(0.001*radius, permitted error for 0.05-pixel projection). Default celestial representation range=1e12 m, configurable/debug-specific, not world/math cutoff. f32 spacing at 1.5e11 m is ~16 km: distant meshes have screen-space budgets, not Phase 1 millimetre claims.

Compare f64 screen projection before/after narrowing <=0.05 physical pixels at current viewport/FOV for finite front-of-near-plane points; use conservative clipped bounds for near-plane crossings, not division by near-zero z. Mesh failure yields explicit diagnostic/labelled marker fallback; nonfinite data is an error even when culled. Phase 1 near-debug budgets stay 1e-5/1e-4/1e-3 m at 100 m/1 km/10 km. Independent rounded source coordinates cannot recover arbitrary detail.

### 12.3 Depth/shading

Separate celestial reverse-Z infinite-far perspective: right-handed/-Z camera, wgpu 0..1, depth=near/(-z), Depth32Float, clear 0, GreaterEqual, opaque sphere depth writes. Choose near=max(0.1 m,0.01*nearest_positive_surface_clearance) for outside-body camera; validate/report near. No general depth engine/log/multipass framework. Existing Phase 1 forward-depth path remains.

Spheres before trails/UI. Celestial historical lines share reverse-Z, depth test/no writes; any tiny coincident-line bias is documented debug-only. Markers/labels are no-depth egui navigation overlays, optionally dashed when reference-sphere CPU f64 ray test says occluded. Overlay identity/selection is independent of geometry culling.

Planet/moon normal debug shading, e.g. color*(0.2+0.8*max(dot(normal,normalize(0.3,0.6,1)),0)) in camera axes; star warm unlit. Label **debug celestial shading**, not stellar irradiance/PBR. Selected ring/local axes; optional basis/longitude line makes independent spin visible without textures. Styles/names/masses create no lighting physics.

## 13. Camera/labels

App owns one FramePose/observer velocity plus orbit parameters: target BodyId/frame role, distance/yaw/pitch. Selection differs from focus. Default root-frame overview fits initial physical bounds about initial COM; it never recentres universe as bodies move.

Click marker or panel name; hit radius 8 physical pixels. Overlap resolution: screen distance, then nearest f64 depth, then stable request order. Ignore camera gestures consumed by egui. Focus deliberately attaches to selected translating frame; fit-to-body sets distance=4*radius. Keep useful existing zoom on ordinary focus, with minimum 4*radius. Optional body-fixed attachment inspects local axes/spin. Camera changes never mutate world.

Drag orbit, exponential wheel zoom, overview button, previous/next or Tab selection. Bound pitch away from poles and camera distance >=1.05*radius; navigation constraint, not collisions. No walking/spacecraft/gameplay. Re-expression action uses Phase 1 pose/kinematic conversion and preserves instantaneous state; focus/attachment deliberately changes control/velocity semantics. Rebuild remaps by BodyId/role. No disconnected scene load/global teleport.

Renderer overlay preparation projects f64 relative centres with FOV/viewport, rejects behind-camera projections and handles zero distance; only bounded screen coordinates narrow to UI. Labels show name/distance/selection, selected panel mass/reference radius/system velocity/speed/position/orientation/spin. Existing egui is enough; labels/markers own no physics.

## 14. Actual trails

App TrailHistory stores synchronized sample tick/time and all recorded body centres in fixed system axes as f64, no FrameId. It is disposable debug history, not authoritative world storage. Sample only successful world commits through post-commit callback, never requested time or once-per-render. All substeps are eligible, making cadence FPS-independent.

Default stride circular=8 ticks (80 s), hierarchy=64 (3840 s); 8192 complete samples, total payload cap=8 MiB including ticks/positions. Checked accounting reduces capacity at larger N, report stride/retention. Three-body hierarchy retains ~one year/~82 points per lunar orbit. Seed epoch, draw current endpoint separately without changing stored cadence. Preallocated synchronized ring evicts oldest whole sample, no per-sample allocation.

Default **inertial system-space history**. Planet focus also offers labelled `history relative to Aurelia at each sample`: subtract Aurelia's recorded position at that same historical tick, then render relative points in today's translating frame/system-aligned axes. This is real relative history, not parented motion or ideal ellipse. Today's planet minus yesterday's moon is not that mode. No predicted trajectories/conics.

For inertial history label recorded system coordinates with current projection root for conversion only: historical records do not claim all points physically exist at today's tick. Convert through current observer in f64. Relative mode uses translating frame and simultaneous historical deltas. Clip segments in f64 at view frustum/near plane before <=0.05-pixel narrowing, reuse GPU staging. Branch/direction/mode tags prevent joining different histories.

Reset/physical edit/seek/direction change/fixture replacement/h change clear and seed. Continuous reverse starts a separate decreasing-time strip after first restored commit, forward-after-reverse starts another; never join pre/post branch or direction. Private seek replay is not presented as publicly observed motion: completion seeds fresh trails and subsequent real commits refill them. Projection rebuild alone preserves system-coordinate samples and re-prepares fresh handles.

## 15. API sketches

Semantic sketches, not source to implement here or immutable signatures. Private fields/defaults; expose actual caller needs only.

```rust
// Simulation; raw output may be partial on error and must not be committed.
pub const GRAVITATIONAL_CONSTANT_M3_KG_S2: f64 = 6.67430e-11;
pub fn evaluate_accelerations(
    masses_kg: &[f64], positions_m: &[DVec3], output_m_s2: &mut [DVec3],
) -> Result<GravityEvaluationReport, GravityError>;
pub struct SimulationConfig { /* checked h, work/debt/history limits */ }
impl SimulationConfig {
    pub fn try_new(fixed_step_s: f64) -> Result<Self, SimulationError>;
}
pub struct IntegrationWorkspace { /* reused arrays/candidates/old-next a */ }
fn prepare_kdk_step(work: &mut IntegrationWorkspace, step_s: f64)
    -> Result<(), SimulationError>;
pub struct FixedStepRunner {
    // config/controller/cumulative Duration/baseline/tick/expected revision,
    // workspace/ring/optional private ReplayTask; no owned CelestialSystem
}
pub struct SimulationAdvanceReport {
    pub authoritative_time: SimulationInstant,
    pub requested_time: SimulationInstant,
    pub tick: u64,
    pub work_steps: u32,
    pub forward_steps: u32,
    pub backlog_ticks: u64,
    pub pair_evaluations: u64,
    // force passes, replay progress, overload/rejected/cancelled demand
}
impl FixedStepRunner {
    // Setup/rebranch may allocate; hot steps/snapshots do not.
    pub fn new(system: &CelestialSystem, config: SimulationConfig)
        -> Result<Self, SimulationError>;
    pub fn admit_wall_elapsed(&mut self, elapsed: Duration)
        -> Result<(), SimulationError>;
    pub fn pump(&mut self, system: &mut CelestialSystem,
        after_commit: impl FnMut(u64, &CelestialSystem))
        -> Result<SimulationAdvanceReport, SimulationError>;
    pub fn seek_tick(&mut self, target: u64) -> Result<(), SimulationError>;
    pub fn cancel_seek(&mut self);
    pub fn reset_branch(&mut self, system: &mut CelestialSystem)
        -> Result<(), SimulationError>;
    pub fn rebranch_after_edit(&mut self, system: &CelestialSystem)
        -> Result<(), SimulationError>;
    // Explicit pause/rate/single-step obey Section 6.4.
}
pub struct SystemDiagnostics {
    pub sampled_time: SimulationInstant,
    pub kinetic_energy_j: f64,
    pub potential_energy_j: f64,
    pub linear_momentum_kg_m_s: DVec3,
    pub angular_momentum_com_kg_m2_s: DVec3,
    pub center_of_mass_m: DVec3,
    // drift against an explicit captured baseline/scales
}
pub fn system_diagnostics(system: &CelestialSystem)
    -> Result<SystemDiagnostics, SimulationError>;
```

Pump error includes structured last committed tick/work already done. Earlier good steps remain committed; failing candidate rolls back, not entire pump. Admission error preserves old target; private replay failure preserves live world.

```rust
// Renderer: domain-free copied requests; no world/BodyId import.
pub struct CelestialRenderBody {
    pub body_fixed_frame: FrameId,
    pub reference_radius_m: f64,
    pub color: [f32; 4],
    pub unlit: bool,
    pub selected: bool,
}
pub struct CelestialStaging { /* reused vertex/uniform/trail bytes */ }
pub struct CelestialFrame<'view, 'tree, 'storage> {
    // PreparedView borrow/projection/staging/full-frame poison guard
}
impl CelestialFrame<'_, '_, '_> {
    pub fn append_bodies(&mut self, bodies: &[CelestialRenderBody])
        -> Result<(), RenderPreparationError>;
    pub fn append_historical_lines(&mut self, source: FrameId, lines: &[DebugLine])
        -> Result<(), RenderPreparationError>;
    // CPU overlay output uses request index and bounded screen coordinates.
}
// App: BodyId associations and debug history only.
struct TrailHistory { /* synchronized f64 samples/branch/direction */ }
impl TrailHistory {
    fn record_committed(&mut self, tick: u64, system: &CelestialSystem);
    fn clear_and_seed(&mut self, branch: u64, tick: u64, system: &CelestialSystem);
}
```

Existing BodyStateUpdate/system publication/projection/SimulationInstant should suffice. No public snapshot traits, acceleration hierarchy, renderer-world link, frame mutation hook or new crate.

## 16. Failure categories

| Category | Examples/response |
| --- | --- |
| Domain/user input | Invalid mass/radius/state/name/ID/seek: reject draft transactionally, show error. |
| Configuration | Invalid h/work cap/history capacity/epoch resolution/resource limits: reject before stepping. |
| Numerical | Coincidence/nonfinite arithmetic/unrepresentable gravity/encounter/spin overflow: retain last committed state, halt visibly with IDs/tick/context, no clamps. |
| Session/recoverable | Missing recent snapshot -> replay; overload -> halted demand/preserved debt; cancel -> retain live state. |
| Programmer invariant | Private arrays/ring inconsistent after validated setup -> useful assertions; public stale-session detection is structured error. |
| Projection/render | Valid world survives; suppress mixed-revision draw/show error/rebuild; surface recovery stays renderer-owned. |

Finite inputs do not guarantee finite arithmetic. Never upload NaN/substitute defaults or continue zero-force on corruption. Expected revision guards keep cached masses/accelerations/history from bypassing branch policy.

## 17. File-level implementation plan

The table records the implementation responsibilities and ownership. Concrete APIs, the additional app library/history benchmark target and evidence are documented in ADR 0004 and the validation record. Preserve current conventions.

| File | Responsibility/types/functions/dependencies and placement rationale |
| --- | --- |
| `crates/simulation/Cargo.toml` | Add world/glam and existing Criterion dev dependency; register gravity/fixed_steps benches. Real domain evolution now needs world; no graphics. |
| `crates/simulation/src/lib.rs` | Concrete exports/config/runner/errors/diagnostics/kernel and inertial SI/full-time documentation. |
| `crates/simulation/src/gravity.rs` | G, pair loop, GravityError/report/resolution guard; glam numerical slices, no frames. |
| `crates/simulation/src/integrator.rs` | IntegrationWorkspace/KDK candidates/constant spin; math/glam/gravity, no render queries. |
| `crates/simulation/src/runner.rs` | Config/FixedStepRunner/tick guards/demand/work/debt/publication/revision/report; world/math/time/integrator/history. |
| `crates/simulation/src/history.rs` | Private baseline/preallocated snapshots/ReplayTask/branch generation; world states and numerical work, not visualization. |
| `crates/simulation/src/diagnostics.rs` | Pure compensated E/P/L/COM and baseline/scales; world/math/glam. |
| `crates/simulation/src/time.rs` | Retain Phase 2 controls, narrow helpers only if needed; no integration in clock. |
| `crates/simulation/tests/common/mod.rs` | Independent analytic initial conditions/diagnostic answers, test-only world/math/glam; no production fixture framework. |
| `crates/simulation/tests/gravity.rs` | Formula/symmetry/self/radius/error/encounter tests. |
| `crates/simulation/tests/orbits.rs` | Many-period/conservation/convergence/spin; named long_run tests ignored by default for focused release execution. |
| `crates/simulation/tests/fixed_steps.rs` | Controls/FPS/wakeup/debt/history/seek/reset/edit/transaction/projection/stale-session tests. |
| `crates/simulation/benches/gravity.rs` | Pure acceleration N=3/16/64/256/1024, valid noncoincident fixtures. |
| `crates/simulation/benches/fixed_steps.rs` | Warm step/512-step batch/gather/publish/history/replay overhead separated, no window. |
| `crates/world/src/system.rs` | Existing API should suffice; modify only for proven gap, not to add gravity/history ownership. |
| `crates/world/tests/frame_projection.rs` | Additional projection failures/rebuild regressions; integrated gravity consistency belongs in simulation tests, avoiding world -> simulation dev cycle. |
| `crates/world/benches/celestial_system.rs` | Add 3/16/256 sizes and new-time complete publication, still pure world cost. |
| `crates/world/benches/frame_projection.rs` | Add representative counts; full 2N-edge republish separate from build/append. |
| `crates/renderer/src/celestial.rs` | Domain-free requests/icosphere/CPU preparation/layout/staging/CelestialFrame/narrow GPU resources; math/glam/wgpu. |
| `crates/renderer/src/celestial_view.rs` | Headless reverse-Z/f64 projection/clip/error/overlay/segment functions; math/glam. |
| `crates/renderer/src/shaders/celestial.wgsl` | Observer-relative triangle/normals and minimal shading/unlit flag. |
| `crates/renderer/src/shaders/celestial_trails.wgsl` | Observer-relative line/color with reverse-Z, no simulation. |
| `crates/renderer/src/lib.rs` | render_celestial wiring/draw before egui/lazy resources/resize; preserve render/render_debug. |
| `crates/renderer/src/view.rs` | Narrow required centring/rotation/CPU helpers, checked budgets; preserve near-debug contract. |
| `crates/renderer/tests/celestial_precision.rs` | Headless layout/shader/depth/icosphere/sphere-label-trail precision/occlusion. |
| `crates/renderer/benches/celestial_preparation.rs` | Body/marker/trail preparation separately with reused storage. |
| `crates/renderer/Cargo.toml` | Register actual bench/reuse naga-Criterion; no world/simulation/unsafe-packing dependency. |
| `crates/app/src/gravity_orbits.rs` | GravityOrbitsDemo/commands/world-runner-projection/egui diagnostics/editor orchestration; current subsystem links. |
| `crates/app/src/gravity_fixtures.rs` | Physical initial-condition constructors/display styles using world/math/simulation G, no continuing analytic motion. |
| `crates/app/src/celestial_camera.rs` | Debug observer orbit/zoom/focus/overview/re-expression/input routing; math/world associations, no gravity. |
| `crates/app/src/trails.rs` | Bounded synchronized committed history/invalidation/modes/renderer requests; app owns BodyId/debug lifetime. |
| `crates/app/src/main.rs` | Mutually exclusive flag/input routing/once-captured elapsed/pre-render pump/lifecycle/render. |
| `crates/app/src/redraw.rs` | Preserve bounded cadence; only change/test concrete lifecycle needs, never use interval as h. |
| `crates/app/src/reference_frames.rs`, `crates/app/src/celestial_model.rs` | Preserve existing modes; shared wiring only as needed, no gravity replacement. |
| `Cargo.toml`, `Cargo.lock` | Only used dependency edges/bench configuration; no new crate/unrelated upgrades. |
| `README.md`, `docs/architecture.md`, `docs/roadmap.md` | Actual implementation flag/dependency/capability/status when evidence exists. |
| `docs/performance.md` | Measured baseline/workloads/throughput/error/allocation context. |
| `docs/phase-3-validation.md` | Actual maxima/native controls/CI evidence and open criteria. |
| `docs/adr/0004-gravity-integration-and-playback.md` | Gravity/KDK/time/debt/history/replay choices/alternatives/evidence/revisit triggers. |
| `.github/workflows/ci.yml` | Existing all-target/headless jobs cover new code; modify only for real need, no GPU/bench/ordinary long release execution. |

Math/core need no physics modules; renderer trail buffers and app history are enough. No world-trail storage or speculative future-terrain modules.

## 18. Implementation order

1. Resolve outstanding Phase 1/2 visual/platform/CI gates or record explicit reviewed change; preserve current baseline deliberately.
2. Gravity kernel/independent formula and failure tests.
3. KDK/diagnostics/circular-eccentric-convergence-spin tests and preliminary release drift; do this before visuals can hide wrong physics.
4. Connect full-state publication/cache/revision/rollback/frame consistency headlessly.
5. Runner/demand/history/replay/edit/reset/FPS/work/debt/lifecycle policy tests.
6. CPU celestial view/layout/precision/depth/shaders then spheres/markers GPU path, old forward-depth mode intact.
7. Camera/labels/session UI, connected inspection and numerical diagnostics.
8. Actual committed-tick trails/bounds/invalidation/clipping.
9. Wire --gravity-orbits, headless session-command tests and native operator sequence.
10. Separated benchmarks/full quality/focused release/native/remote evidence, drift docs/ADR/review.

Each implemented stage left smoke/reference-frames/celestial-model modes buildable and their tests passing. Healthy milestone commits and validation evidence are recorded separately.

## 19. Validation application

Implemented command:

```text
cargo run --locked -p mundaris_app -- --gravity-orbits
```

Default hierarchy at 0 s, h=60 s, overview/Aurelia selected, markers/labels/trails enabled, selected 1000x but paused. Circular fixture is explicit scenario replacement; focus never loads another scene.

Operator sequence at 1280x800, independently recorded on Windows/Linux with OS/GPU/backend/driver:

1. Inspect initial masses/radii/COM, star physical sphere and planet/moon navigation markers; select all bodies.
2. Resume 1x/10x/1000x, pause/single-step; authority changes by h only, camera works paused.
3. Focus/orbit/zoom Aurelia and fit moon neighbourhood, higher rate shows genuine relative orbital motion.
4. Run 100000x/1000000x for at least one outer orbit if throughput permits; inspect conserved values/rate/debt. Excessive rate visibly overloads without changing h.
5. Seek known retained tick, reverse within/beyond history, inspect private replay progress/cancel, compare known tick; no free-seek claim.
6. Reset/refill trails. Valid mass/velocity edit creates new branch; invalid draft changes nothing. Radius changes geometry, not gravity.
7. Toggle labelled inertial/planet-relative trails, focus/overview, rebuild projection; one connected world/unchanged body identity.
8. Resize/minimize/restore/occlude/surface recovery/close. No hidden-time catchup, non-drawable busy loop or mismatched revisions/numerical errors.

Residual tests are numerical authority; visual checks prove inspectability/lifecycle, not conservation tolerances. Discrete fixed-step motion is honest; render interpolation/extrapolation is deferred rather than mixing times/guessing orbits.

## 20. Automated tests

Ordinary tests are headless. Exact equality is appropriate for copied history/counters/unchanged authoring/same-build replay, not orbital accuracy.

| Test | Fixture/assertions |
| --- | --- |
| Pair force/units/symmetry | 2e20/3e20 kg at 1e7 m X: a1=G*3e20/1e14 X, a2=-G*2e20/1e14 X, relative components <=2e-14; mass-weighted force residual normalized by force magnitudes <=2e-14. Also non-axis independent formula. |
| No self/mutual force | One body exactly zero/zero pairs; empty kernel valid, app fixture nonempty. Equal-mass pair/noncollinear three-body independent per-body force error <=5e-14; star reacts. |
| Radius independence | Radius x100 with same masses/states: accelerations/1000-step translations same bits, no radius kernel argument. |
| Numerical rejection | Invalid masses/lengths, coincidence, finite arithmetic overflow/subnormal force, chi>0.02 at drift: structured error, unchanged world/time/revision/cache. |
| Many-period orbits | Section 11 circular/E/P/L/COM/period/phase/eccentric/convergence criteria; maxima over ticks, independent analytic answers. |
| Long release | Named ignored long_run circular/eccentric 1000 periods/hierarchy 2 outer periods, specified bounds. |
| Deterministic replay | 10000 ticks reset/replayed twice same executable/target: physical component bits match; identities/revisions excluded. No wall reads/RNG/parallel reductions. |
| FPS/UI equivalence | Equal exact 10 s duration at rates 0.1/1/10/100/1000 and -10/-1000 from sufficiently advanced retained ticks, 30/60/144 FPS and irregular extra UI partitions; drain admitted debt, same target ticks/state bits. Include zero/many-step frames. |
| Accumulator | Zero elapsed; representable just-before/at/after h; cumulative fractions; duration/rate/time overflow; huge epoch collapse; invalid work/config. No early epsilon step. |
| Pause/rates | 0x/paused 100 updates execute zero; single-step exactly one; setting rate evolves nothing; sign change reports cancelled debt. |
| Backlog/overload | Test work cap 4/debt cap 16: max four work units, admitted debt retained, over-cap interval rejected/reported, no h change/drop. |
| Reverse/seek/reset | Restore 100 recent tick snapshots exactly; 8-slot ring forces reset/replay outside retention; same forward-run target bits. Cancel retains live state/pre-origin rejects/origin visibly pauses. Reset tick123 -> branch zero/IDs. |
| Editing/history | Physical edit at current time rebases tick/history/trails/diagnostic reference; no pre-edit future restoration. Name only retains trajectories. Invalid edit changes no session/world data. |
| Transactionality | Duplicate/foreign/incomplete new-time batch rejected coherently; bad candidate does not promote acceleration/history/tick. Earlier successful pump steps reported, no whole-pump rollback claim. |
| Frame projection | 1000 steps: exact centre/velocity copies, separate spin/zero fixed translation, world time/revision match; rebuild fresh namespace equivalent/IDs unchanged; A spin independent of B. |
| Memory/stale sessions | Small cap stores whole snapshots only; unexpected revision/append rejects until rebranch. Restore/mass edit recalculates acceleration. |
| Sphere/marker precision | Centre 1.5e11 m, local focus preserves Phase 1 1e-9/1e-7 m CPU and near budgets; overview/narrowed geometry/labels <=0.05 pixels at 1280x800/3840x2160, no absolute-root f32. |
| Depth/layout/shaders | Reverse-Z near=1 within2e-6, decreases at 1e3/1e8/1e12 m, positive w, clear/compare correct; 32/32/64-byte layouts, sphere units/winding/chordal bound, naga validates both WGSL. |
| Camera/selection | Focus/overview/re-expression do not mutate world/revision; rebuilt attachment role/ID stable; ordered marker tie-breaking/behind-camera/zero-distance errors. |
| Trails | Same committed stride across FPS, whole-sample eviction, failed step records none, reset/edit/seek/direction seeds no joined strip; simultaneous reference subtraction; rebuild retains system samples. |
| Trail clipping/poison | Astronomical f64 clipping before narrowing, near/behind crossings finite, <=0.05 pixels; partial/failed frame not uploaded. |
| Lifecycle | Synthetic long hidden duration generates no debt; resumed timestamp fresh; render/surface retry does not double-advance. |

Tests use independent formulas/convergence/explicit coordinates; comparing only two implementation routes is insufficient. Same-build equality verifies reproducibility, not gravity correctness.

## 21. Performance/threading

Existing stable Criterion/black_box/reused finite nonconstant data. Compile all targets, run benches locally outside CI. Record hardware/OS/compiler/profile/glam features/lockfile, N/pairs/h/history, medians/distributions/allocation inspection. No hardware timing assertions in correctness tests. The measured Windows baseline is in [performance](docs/performance.md#phase-3-baseline--2026-10-01).

| Group | Workloads |
| --- | --- |
| Acceleration | 3/16/64/256/1024 bodies, checked force pass/pairs per second. |
| Integrator | Same N, warm candidate-only KDK; initialization force separate. |
| Many steps | 512-step committed batch; history on/off separated, no render/projection inside. |
| Gather/world publication | Each N, scratch gather and new-time full commit separately. |
| Projection | Each N, full 2N-edge republish; retain existing build/4096 probes. |
| Celestial preparation | Visible 3/16/64 spheres, marker-heavy256/1024; view/source versus vertex/packing. |
| Trails | 3/16 bodies x1024/8192 samples when byte cap permits; sampling/clip/narrow/packing separately. |
| Diagnostics/history | E/COM and snapshot restore/replay overhead if material. |

On documented baseline desktop aim to sustain small hierarchy 1000x, measure whether 1,000,000x at h60 sustains responsive60 Hz. Aim 512-step pump <8 ms/small render preparation <2 ms on that host, investigate rather than weakening physics; no cross-hardware promise. Report actual achieved rate/debt/larger-N sustainable rates. This specification claims no Phase 3 timing result.

Single-threading preserves deterministic accumulation/debuggability at small N. Dense inputs/candidates/explicit ownership allow future parallelism but introduce no job/lock/atomic/Rayon framework. Measured scale triggers are Section 4.3.

## 22. Deferred decisions/before implementation

- Approximate/parallel/GPU gravity: real workload/CPU trigger, not 1024 probe alone.
- Adaptive/individual steps/new forces/other integrators: new accuracy/encounter decision; never silent h change.
- Cross-platform bits: deliberate compiler/math contract only; current guarantee same executable/target.
- Longer checkpoints/persistent timeline: measured replay latency workflow blocker; ring/baseline enough now.
- Test particles/deletion/generational or persistent IDs/save/cross-system addresses: separate world-domain design.
- Render interpolation/instancing/new depth: measured stepping/preparation problem with coherent/source-centred constraints.
- Final lighting/shape/terrain/LOD/materials: separate phases; debug sphere locks none.
- Orbital elements/primary metadata/prediction: not needed for mutual force/history inspection.

Before implementation review numerical tolerances against analytic error scales, memory accounting, overload admission, branch-reset UX and reverse latency. Resolve pending Phase 1/2 acceptance. Foundational changes belong explicitly in spec/ADR, not incidental app workarounds.

## 23. Definition of Done

Checked boxes below are supported by [Phase 3 validation](docs/phase-3-validation.md), ADR 0004 and measured performance. Windows evidence does not satisfy the open Linux/remote-CI criteria.

### Quality

- [x] Stable Rust/Rust2024, six non-publishable crates, no unsafe/cycles/prohibited framework/unrelated upgrades.
- [x] `cargo fmt --all -- --check` passes.
- [x] `cargo build --locked --workspace` passes.
- [x] `cargo check --locked --workspace --all-targets --all-features` passes, including benches.
- [x] `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` passes.
- [x] `cargo test --locked --workspace --all-features` passes headlessly, including old tests/compile-fail contracts.
- [x] `cargo test --locked -p mundaris_simulation -p mundaris_world -p mundaris_math -p mundaris_renderer -p mundaris_app --release --all-features` passes.
- [x] `cargo test --locked -p mundaris_simulation --release --test orbits long_run -- --ignored --nocapture` passes/records maxima.
- [x] `cargo doc --locked --workspace --all-features --no-deps` passes with RUSTDOCFLAGS=-D warnings in host shell.
- [x] `git diff --check`, UTF-8/LF/final-newline, Markdown fences/links/status checked.

### Physics/architecture

- [x] Symmetric unsoftened gravity/no self-force/KDK full-time velocities/constant spin pass independent tests.
- [x] Section11 E/P/L/COM/radius/period/phase/convergence/long-run bounds pass; actual drift documented per debug/release/native target.
- [x] Fixed h independent of FPS/UI/rate; fractions/caps/lifecycle/overload report honest accuracy.
- [x] Pause/step/reset/reverse/seek/cancel/edit semantics implemented; history bounded/replay guarantees honest.
- [x] Transactional per-step world publication, post-success cache/history promotion, coherent/rebuildable projection/stale-state detection.
- [x] No authoritative render/frame identity, global rebase/attached-object rewriting or graphics dependence in simulation.

### Visible/performance/platform

- [x] --gravity-orbits star/planet/moon visibly evolve from all-pair gravity; old modes still work.
- [x] Physical spheres/debug shading/markers/labels/selection/depth/precision/layout/WGSL verified; aids change no radius/physics/distance/identity.
- [x] One connected selection/focus/orbit/zoom/overview experience with diagnostics.
- [x] Actual inertial/labelled relative trails bounded/invalidated/refilled correctly.
- [x] Numerical failure preserves last good state; lag/overload visible without changing h.
- [x] Separated measured benchmark baseline/sustainable rates with workload/hardware/profile/error/allocation evidence.
- [x] Windows native build/headless/focused release and full Section19 visual sequence pass with backend evidence.
- [ ] Linux native build/headless/focused release and full Section19 visual sequence pass with backend evidence.
- [ ] Current-revision remote Linux-quality/Windows-compatibility CI pass through normal workflow. CI compilation is not native visual evidence.

### Documentation/review

- [x] Phase3 validation record reports actual maxima/native/CI evidence; absent evidence remains open.
- [x] ADR0004 records gravity/KDK/time/overload/history/reverse/seek alternatives/guarantees/measurements/revisit triggers.
- [x] README/architecture/roadmap/performance distinguish implemented versus proposed capabilities.
- [x] Correctness/architecture/performance review and Phase1/2 prerequisite acceptance or explicit reviewed gate change.
- [x] No terrain/LOD/generation/atmosphere/gameplay/general graphics/collision/timeline/scaling scope leakage.

Objective end state: a small, genuine, numerically inspectable and visibly moving celestial system preserving authoritative world and disposable frame/render ownership.
