# Mundaris — Phase 3.5: Celestial Navigation, System View & Time Warp

> **Status:** implemented with Windows automated/benchmark and directed native evidence; full operator/high-DPI/sleep, Linux and remote-CI acceptance remain open. See [implementation evidence](docs/phase-3-5-validation.md) and [ADR 0005](docs/adr/0005-celestial-navigation-system-view-and-timewarp.md).
>
> **Audit date/baseline:** 2026-10-02, repository revision `92fbe1323a8143a0a2ec47267cfc9edfe208472b`, initially clean working tree.
>
> **Targets:** stable Rust/Rust 2024, native Windows x86-64 and Linux x86-64; existing six crates.
>
> **Prerequisites:** [bootstrap](MUNDARIS_PROJECT_BOOTSTRAP.md), [engine design](MUNDARIS_ENGINE_DESIGN.md), [Phase 1](MUNDARIS_PHASE_1_REFERENCE_FRAMES.md), [Phase 2](MUNDARIS_PHASE_2_CELESTIAL_MODEL_AND_TIME.md), [Phase 3](MUNDARIS_PHASE_3_GRAVITY_ORBITS_AND_CELESTIAL_RENDERING.md), and ADRs [0002](docs/adr/0002-reference-frames-and-precision.md), [0003](docs/adr/0003-celestial-domain-and-time.md), [0004](docs/adr/0004-gravity-integration-and-playback.md).
>
> **Evidence:** [Phase 1 validation](docs/phase-1-validation.md), [Phase 2 validation](docs/phase-2-validation.md), [Phase 3 validation](docs/phase-3-validation.md), [performance](docs/performance.md). Historical evidence does not complete new acceptance criteria.

## 1. Outcome and scope decision

Phase 3 proved authoritative full N-body physics, precision, coherent publication and a minimal renderer. Its application remains a validation harness. Phase 3.5 makes the same connected celestial system understandable and navigable immediately:

```text
open --gravity-orbits
  → fitted system structure, markers and instantaneous orbit guides
  → select any body by sphere, marker, label or list
  → smoothly focus, orbit and zoom toward its physical radius
  → select/focus its moon
  → return to whole-system or local-subsystem overview
  → optionally free-fly, then refocus
  → accelerate time with explicit method, requested/achieved rate and debt
```

### Chosen implementation

- Extend `--gravity-orbits`; retain both hierarchy and circular-oracle fixtures, paused startup and all existing validation modes.
- Geometric system fitting, whole-system and selected-body/local-satellites scopes.
- `BodyId` selection, unified hit testing, readable deterministic label layout and derived visibility aids.
- System orbit, body orbit and scale-aware editor free flight using one Phase 1 observer representation; smooth navigation transitions.
- Clear actual-history trails plus **separate instantaneous elliptical orbit guides**.
- Exact-mode playback UX, coherent achieved-rate measurement, responsive bounded pump scheduling and safe lifecycle/stall accounting.
- **No new authoritative propagator, larger physical timestep, adaptive/multi-rate integration or analytic extreme warp in this implementation.** No selectable unimplemented quality modes.

This choice follows measured normal workloads, not the 1024-body stress probe. The three-body hierarchy already has considerable CPU headroom at the requested rates. Extreme acceleration remains a genuine asymptotic problem: counting identical fixed steps cannot scale indefinitely. Sections 12–17 establish its future deterministic solution path without expanding this milestone into an orbital-integrator research project.

“Exact” means **the unchanged Phase 3 fixed-step full N-body numerical method and baseline timestep**, not an exact solution to Newtonian differential equations. Guides are approximate derived geometry; they do not make simulation approximate. Do not label a larger KDK step baseline-exact merely because its force law remains full N-body.

## 2. Audit coverage and evidence limits

The audit read all tracked repository files: source, shaders, tests, benchmarks, manifests/lockfile, specifications, ADRs, validation/performance documents, CI and repository configuration. There is no repository `AGENTS.md`. Ignored build products and external dependency implementations are not repository source.

Current boundaries are appropriate:

```text
app → math, world, simulation, renderer
world → math
simulation → math, world
renderer → math
core: no domain APIs
```

`CelestialSystem` owns bodies/time/revision; each body's translating frame is a root child and its rotating frame a child of that anchor. Projection is disposable. `CoherentCelestialView` retains immutable world/projection borrows at one committed instant. Renderer owns source-centred conversion, projection, checked narrowing and GPU data, with no world dependency. No navigation change requires replacing these contracts.

The audit reran `cargo test --locked --workspace --all-features` on Windows: **75 runtime tests and seven compile-fail documentation tests passed; two long-run tests remained ordinarily ignored**. Existing performance results were inspected together with their benchmark implementations; no new throughput or GPU measurement is claimed here. Startup/interactive evidence remains the earlier recorded Windows execution and operator report. This audit did not conduct a new native visual sequence or directly observe every requested playback preset. Numerical demand/framing calculations below are source-derived calculations, not captured presentation performance.

Remote CI could not be inspected because `gh` is unavailable on this host. Linux/current-revision CI acceptance remains unverified; no existing platform checkbox is completed by this design.

## 3. Current navigation/rendering audit

### 3.1 Selection, focus and the apparent star bias

| Behavior | Actual implementation and consequence |
| --- | --- |
| Initial selection | `GravityOrbitsDemo::create` collects dense IDs and selects index 1: **Aurelia**, not Solace. Camera begins in system overview. |
| Panel selection | All body names are selectable buttons; commands update a dense selection index and edit draft. That index maps to a `BodyId`. Selection alone does not focus. |
| Keyboard | Tab selects next, Shift+Tab previous, when egui does not want keyboard input. Neither automatically changes focus. |
| Marker selection | `select_marker` uses an 8-physical-pixel centre hit radius; nearest screen distance, then f64 depth, then stable request index. App maps result to its body IDs. Available only outside UI areas and while markers are enabled. |
| Physical-sphere click | No sphere raycast. A click near the centre may hit its marker, but a large sphere's visible outer area is not selectable by geometry. |
| Label click | Painter text has no interactive hit rectangle. The label itself does not select. Panel name buttons are different from projected labels. |
| Highlight | Selected overlay is yellow, ring radius 8 px versus normal 4 px; selected body-fixed RGB axes are appended. The packed sphere selected flag is not used by `celestial.wgsl`, so physical sphere shading itself does not change. |
| Focus any body | `CelestialCamera::focus(pair, id, body_fixed, fit)` resolves **the supplied BodyId**. Panel focus/fit/fixed-spin commands pass current selection. Planet and moon are supported; no star-only branch exists. |
| Focus change | Immediately replaces attachment/anchor/basis/distance and observer pose. No transition. Ordinary focus from system also fits to 4R; focus from another body retains old distance with a 4R floor. |
| Input commands | One pending `Option<Command>`; later input can overwrite earlier commands in the same UI pass, including an orbit gesture superseding a click. This is a routing fragility, not evidence of permanent star focus. |

Source: `crates/app/src/gravity_orbits.rs` (`create`, `command`, `draw_ui`), `crates/app/src/celestial_camera.rs` (`focus`), `crates/renderer/src/celestial.rs` (`select_marker`), `crates/renderer/src/shaders/celestial.wgsl`.

**Why it can feel stuck on the star:** overview orbits a fixed initial COM, almost at the star because of the mass ratio. Selecting a planet does not change this pivot. Focus actions live deep in a large scrollable engineering window; there is no double-click/F focus shortcut. Solace is the only initially drawable physical sphere. Aurelia/Luma markers nearly coincide, labels overwrite each other, and ties can favor an earlier/nearer request. Thus a user may keep orbiting the system/star while selection silently changes. The code does not establish a universal “click always selects star” defect: initial star and planet markers are hundreds of pixels apart. Fix discoverability and hit ambiguity rather than inventing a broken BodyId focus implementation.

### 3.2 Camera, overview, zoom and physical visibility

- Current `CameraAttachment` is `System`, `Translating(BodyId)` or `BodyFixed(BodyId)`. All use one orbit controller, not three navigation modes. There is no free-flight WASD control.
- Overview anchor is the **initial** diagnostic COM. Extent is the initial maximum centre distance from COM plus physical radius; observer distance is `2.5 * extent`, identity orientation looking down −Z. Overview button recreates this initial view, never recomputes bounds from current state/history or available viewport aspect.
- Root overview is a real initial bound-based view, not merely a look-at-origin command. It nevertheless biases the pivot to mass rather than useful visual centre and can become stale. No subsystem fit exists.
- Orbit uses drag factors 0.005 radians per egui-point delta, yaw wrapped, pitch clamped to ±1.5 radians. Wheel uses raw egui scroll Y: `distance *= exp(-wheel * 0.001)` with a minimum. Exponential zoom already exists; it is not linear.
- Focus minimum is `1.05R`, refreshed after radius edits. Fit is `4R`. At Aurelia, the minimum leaves about **318,550 m** clearance; it cannot reach metre-scale near-surface inspection. The wheel scales centre distance, so near-surface increments remain proportional to R rather than altitude. No calibrated line/pixel scroll normalization, maximum-navigation-distance policy or zoom transition exists.
- `reexpress` uses Phase 1 pose and kinematic conversion before committing camera fields. The next `orbit_zoom` calls `update_pose`, setting relative velocity zero: re-expression is instantaneous preservation, while subsequent orbit control deliberately attaches. Rebuild maps by BodyId/role.
- Physical spheres draw at projected diameter ≥2 physical pixels, subject to range (`1e12 m`) and radius/pixel error checks. Smaller/range/precision-fallback bodies still have overlays if centre projection is on-screen. Markers stay enabled even on large spheres; no fade. Centre-behind-near/offscreen projection returns no marker. There is no offscreen direction indicator or overlap picker.
- Marker occlusion is an O(N²) CPU ray/reference-sphere check. It annotates text `(occluded overlay)`; marker rendering does not currently become dashed. Overlays do not depth-test. Selected axes use the historical-line pipeline/report counter even though they are not history.

Source-derived initial hierarchy at 1280×800, 60° FOV, scale factor 1:

| Quantity | Approximate result |
| --- | --- |
| Camera distance | `3.75250e11 m` |
| Star / planet / moon screen X | 640.00 / 916.94 / 917.13 px; all initially screen Y≈400 |
| Physical diameters | star 2.569 px; planet 0.0235 px; moon 0.00138 px |
| Planet/moon marker separation | **0.185 px**, far below 8 px hit radius and label dimensions |

These explain correct subpixel physics and poor navigation visibility simultaneously. Do not enlarge body radius or promise individually separated moon markers in a physically scaled whole-system projection.

### 3.3 Labels and history trails

- Names, distance in scientific-notation metres, representation and occlusion text are painted at marker centre +10 px. No collision avoidance, priority sorting, leader lines or label hit rectangles. Large engineering window can cover viewport content. Selected labels are yellow but do not reserve space.
- Trails are **implemented and enabled by default**, initially inertial. Startup is paused at tick 0 with one seeded sample, so **no historical segment exists yet**.
- Every successful public commit is eligible; hierarchy stores stride 64 (`3840 s`), circular stride 8 (`80 s`). Maximum 8192 synchronized all-body samples/8 MiB, reduced by actual payload at large N. Current endpoint is added independently of stride, so a segment can exist after the first committed step; it need not wait until the next stored sample.
- System-space f64 centres/ticks/times are stored without FrameId. Inertial preparation labels old positions with current projection root. Explicit relative preparation subtracts body and reference positions **at each same historical sample**, then places that relative curve in today's nonrotating translating reference frame.
- App relative-trail command hardcodes `ids[1]`, Aurelia in hierarchy and Companion in circular. The UI says “selected fixture companion”, but changing selection does **not** change the reference. Changing trail mode currently clears/seeds storage although its synchronized absolute samples contain enough data to rederive either mode.
- Branch/reset/seek/direction/physical edits clear/seed; private replay intermediates do not refill observed trails. Seek seeds before replay and again at completion. Projection rebuild preserves samples.
- App prepares `DebugLine` endpoints in f64. Renderer source-centres, rotates and clips in f64 before checked ≤0.05-pixel narrowing. It does **not** first cast astronomical history positions into f32.
- History uses physical GPU line-list rasterization, approximately 1 px, constant body colors, no age fade or selected-trail highlight. Pipeline has `blend: None`; changing alpha alone would not implement fading. Spheres render first, trails depth-test/no writes; egui text/panels cover them. There is no ideal ellipse or orbit-guide implementation in another module.

Why trails are difficult in overview: at h60 one outer-orbit step moves only about **0.0033 px**. Hundreds of simulated seconds still make a subpixel arc. At 1000x about 18 wall seconds accumulate roughly one pixel of outer motion; a full outer period takes about 8.8 wall hours. The moon's local orbit has radius about 0.185 px in overview and is unresolved even after it accumulates; inertial lunar motion largely follows the planet's outer motion. The slowly reacting star's trail is tiny and may be inside its own sphere. Paused startup, low rate, guide absence, thin lines, similar colors, overlapping labels, reset/seek/mode invalidation and offscreen/clipped history all contribute. None proves history is missing or incorrectly prepared.

## 4. Current time-acceleration audit

### 4.1 Actual request/work path

`GravityOrbitsDemo::render` captures monotonic elapsed once per drawable redraw, processes pending commands, calls `admit_wall_elapsed`, then `pump`, publishes frames once and prepares rendering. Physics precedes surface acquisition; lost/outdated/timeout surface results cannot repeat that same elapsed interval/commit. This schedules physics alongside redraw without using render dt as physical h.

`FixedStepRunner` uses one h per branch, reconstructed integer ticks, cumulative exact `Duration` per rate segment, fractional requested time, floor forward/ceil reverse. Warm KDK does one resolved O(N²) force pass plus O(N) state/spin/commit/snapshot work per tick. The count cap is **512 work units per update**, not a CPU-time bound. History restore recomputes forces; private replay initialization, steps and final publication also consume work units.

At candidate debt >65,536 ticks, the **whole new interval is rejected**, prior admitted target retained and `DemandHaltedOverload` latched. Admitted debt drains; user must resume admission or lower rate explicitly. Pause cancels whole/fractional demand. Same-sign rate change retains it; sign change cancels. Numerical candidate failure retains last good committed world. No dt inflation or force approximation occurs.

Reverse is not negative-step integration. The live/private snapshot rings reserve halves of one 16 MiB payload ceiling, each retaining at most 2048 full BodyStates/ticks (less at large N). At h60 that is roughly 34 hours of recent simulated history, distinct from the approximately one-year sparse trail retention. Retained restore publishes stored full-time state, recomputes acceleration and discards later snapshots. Missing history starts private baseline-to-target positive replay under the work cap; additional demand freezes, live world stays unchanged until completion, cancellation retains live state. `History::get` scans the bounded slots; this is a secondary restore cost, not a normal forward gravity bottleneck. Distant seek/reverse latency still grows with ticks from branch origin; visual guide generation cannot make exact replay free.

### 4.2 Requested rates in the actual fixtures

Changing a preset does not unpause. Startup selects 1000x but remains paused. **10000x is not currently a button**; the public rate type/runner support it, so the table specifies its behavior if requested via those APIs. Current forward buttons also include 0.1/10/1,000,000/1,000,000,000x.

| Requested rate | h60 hierarchy steps/s | Average ticks per 60 Hz update | Time to first stored stride64 sample | h10 circular steps/s |
| --- | --- | --- | --- | --- |
| 1x | 0.0167 | 0.000278 | 3840 wall s | 0.1 |
| 100x | 1.667 | 0.0278 | 38.4 wall s | 10 |
| 1000x | 16.667 | 0.2778 | 3.84 wall s | 100 |
| 10000x | 166.667 | 2.7778 | 0.384 wall s | 1000 |
| 100000x | 1666.667 | 27.7778 | 0.0384 wall s | 10000 |

These are mean demand calculations assuming resumed forward playback and sustainable work; integer updates alternate counts. At 1x hierarchy the first committed step takes 60 wall seconds, so authority remains unchanged beforehand. The first visible endpoint segment can precede the stride sample. At all rates physical h and trajectory per tick remain identical. Small-system measured headroom supports these demands on the recorded release benchmark host; this is not a measured FPS/achieved-rate table for every host/debug build.

At 60 pumps/s, the count limit permits ≤30,720 work units/s: h60 ≤1,843,200x, h10 ≤307,200x before CPU/render limits. A 100 ms hierarchy update at 1,000,000x requests ~1666 ticks and performs at most 512; the app test demonstrates lag. At 1,000,000,000x a normal ~16 ms h60 interval requests ~266,667 ticks, exceeding admission cap immediately even with no previous debt. Overload can therefore be **count/scheduling/admission limited**, not force-cost limited.

### 4.3 Workload evidence and reporting weaknesses

| Existing measurement | Interpretation |
| --- | --- |
| 3-body physical h60 hierarchy: 512 commits + snapshots + stride64 trails + one frame publication, median 110.346 µs | Normal solar-system work is very cheap on baseline Windows host; ~4.64 M CPU-only steps/s, not presented throughput. |
| 16-body scale probe: 512 commits/history, median 1.4001 ms | More pairs but still modest; probe h=0.001 s is not a planetary-session h. |
| 1024-body force pass, median 9.73088 ms | 523,776 pairs; warmed checked KDK costs more than force alone. |
| 1024×512 commits/history, median 7.0403 s | Count cap allows a multi-second blocking pump. Self-stall contributes large next elapsed demand while drawable. |
| 3×8192 visible trail preparation, median 2.3897 ms; 16×8192 12.7885 ms | Visualization can dominate normal 3-body simulation; benchmark separately. |

Source: [performance](docs/performance.md#phase-3-baseline--2026-10-01), actual `fixed_steps`, `gravity`, `trail_history` and `celestial_preparation` benches. N1024 is not selectable in the current three-/two-body app fixture menu. Do not ascribe its multi-second force workload to the normal explorer.

Achieved rate is currently `(authoritative_seconds - achieved_anchor_s) / accumulated_elapsed_seconds`, sampled whenever accumulated supplied wall duration ≥250 ms. It uses signed live authority, not the requested preset. Latest work throughput is separate: `work_steps / pump_elapsed`, potentially including restore/private replay, so it is not achieved wall playback.

The panel already shows requested and achieved rates, requested and authoritative instants, fixed KDK/h, status, pending ticks/seconds/fraction, last rejected/cancelled demand and replay progress. It is truthful in principle but buried in dense diagnostics. Short samples at 1x/h60 alternate zero and large tick spikes; rate/pause/direction changes do not consistently reset measurement windows. Seek/reset/single/restoration can pollute a playback sample or make it negative/spiky. Private replay advances internal scratch without live authority, then publishes a large jump; that jump is not wall playback. Reset/seek completion and lifecycle reset some anchors, but no unified reporting policy exists.

### 4.4 Minimize/occlusion and residual stall gap

Current gravity mode **does not continue requesting hidden elapsed demand when hidden state is detected**:

- `main.rs::drawable` checks occlusion, nonzero size and `is_minimized`.
- `about_to_wait` calls `set_lifecycle_drawable`; OS suspension calls it before releasing presentation resources.
- Gravity app resets host capture/rate sample; runner suspends work/admission and explicitly cancels debt/replay/single work. Restore captures a fresh first timestamp with zero elapsed; selected rate and explicit user pause preference survive.
- Redraw scheduler waits without timer wakeups while nondrawable.

This corrects the **pre-Phase-3** audit concern in Phase 3 §2.3: it is a historical baseline finding, not the present gravity-mode implementation. The Phase 1/2 analytic modes still have their own render timestamps and do not receive all gravity-mode lifecycle accounting; preserve their mathematical fixtures while sharing safe host timing in the future change.

There is **no long-elapsed discontinuity guard** while nominally drawable. Sleep/debugger pauses or a multi-second stall without a delivered hidden/suspend event can enter `last_wall` elapsed in full. Depending on rate, the interval is admitted as large fixed-step debt or rejected/latches overload. No enormous single dt occurs, but unexpected enormous requested advancement is still possible. OS monotonic-clock behavior differs; correctness must not depend on sleep always producing a lifecycle event.

## 5. System overview and subsystem framing

### 5.1 Bounds and fit algorithm

App-owned `SystemViewBounds` is a **visual framing product**, never a gravity/world bound. Calculate it from a coherent current view and explicit scope at open, Overview/Refit, scope change and viewport resize:

1. Enumerate relevant BodyIds; include selected body even if its guide is unavailable.
2. Choose a nearby numerical anchor, e.g. first relevant body position. Accumulate centre/radius and curve bounds using f64 offsets from that anchor; use stable midpoint arithmetic (`min + (max-min)/2`) with overflow checks. Avoid mass-weighted COM as the visual centre.
3. Include physical reference-sphere bounds, accepted important elliptical-guide extrema and currently presented trail extent, according to the policy below. Never expand a body's physical radius by marker radius.
4. Derive enclosing sphere from the resulting AABB, or tighter projected extrema when convenient. One body fits its physical radius; empty system returns a labelled empty view with a finite default camera. No guessed universe origin as meaningful fit.
5. For a conservative sphere of radius B and chosen orientation, fit distance `d ≥ B/sin(theta)` where theta is the smaller vertical/horizontal half FOV **after viewport margins**. Also satisfy near clearance. For tighter projected bounds test every bound corner/sphere against the projected margins and expand d if necessary.
6. Reserve ~10% geometric padding plus marker/label gutter (initially 24 physical px minimum), using actual content viewport excluding docked panels. Iterate the screen-space margin fit once or twice; do not inflate physics bounds with pixels. Refit on DPI/size changes.

Guide extrema are derived from ellipse axes, not only coarse sample vertices. Trail bound aggregation uses the same display subset used to prepare lines. Provide separate diagnostics for body, guide and history contributions and included/omitted IDs/extents.

Default overview direction looks substantially normal to the dominant accepted orbital plane (fixture outer plane +Z), slightly tilted to show inclination. Derive angular-momentum plane from accepted guides with finite significance weighting and stable sign; if ambiguous use documented system +Z. Preserve last user yaw/pitch on ordinary Refit; startup alone picks this direction. Avoid eigenvector/PCA/spatial-tree infrastructure for a few dozen bodies.

No per-tick camera snapping or auto zoom to moving bounds. While in system orbit, smoothly track centre updates; only automatically grow fit when essential bodies leave a generous margin, with hysteresis (~10% excursion, no automatic shrink). User wheel/drag disables auto-fit until Overview/Refit. Explicit overview fits current bounds afresh. World positions remain untouched.

### 5.2 Extreme extents and local structure

- **One very distant body:** Whole System includes every body centre/radius. Do not silently drop the outlier. Explain its effect in bounds diagnostics; offer Selected Subsystem and explicit core-only/user membership filtering. A distant-body label/list remains reachable even beyond sphere-render range; representation range is not fit/world cutoff.
- **Tiny moon:** Whole System shows true projection; do not separate its physical position. A cluster indicator/overlap picker plus distinct labels/list identifies the moon. Selected Subsystem makes local separation resolvable.
- **History far outside body bounds:** default history fit contribution is capped at 2× the body+important-guide enclosing radius about that centre. Show “history extends outside fit” and an explicit **Fit including all displayed history** action with uncapped bounds. Clips/fades must not invent a completed orbit. Guides with extreme eccentric apoapsis likewise report their contribution and can be deselected explicitly; never silently alter their math.
- **Selected subsystem:** include selected body and bodies whose current **accepted derived automatic guide reference** is that body, one local generation, plus their physical spheres/local guides. Omit unrelated outer trajectories. Root body selection naturally covers its immediate companions. For a moon with none, provide **Overview around reference + companions** as a separate action. Explicit user membership works when automatic reference is ambiguous. Draw excluded distant bodies as optional bearing indicators/list entries, not invisible physical relocation.
- **Trails in subsystem:** preserve chosen semantics. System history may carry the subsystem far across its outer orbit; indicate truncated fit contribution. Offer explicit simultaneous relative-history reference to this body, but do not secretly change history mode on focus.

Subsystem membership is navigation state only. It does not change frame ancestry, force pairs, velocity or simulation ownership.

## 6. Unified BodyId selection and lightweight labeling

### 6.1 Selection pipeline

Store `selected: Option<BodyId>`, not a renderer request/frame ID. Stable dense request mapping remains app-owned for renderer outputs. Validate all selection/focus commands against current system namespace. Projection rebuild cannot change selection; scenario replacement explicitly resets it.

Inputs converge on `SelectBody(BodyId)`:

1. **Label:** use the actual final displaced label rectangle and BodyId association; click selects, double-click focuses. Pointer over a label consumes that click before camera gestures.
2. **Marker/ring:** use centre hit disc, configured minimum ≥8 physical px radius and optional selection-ring extent; resolve overlaps with an ID-tagged candidate list.
3. **Physical sphere:** unproject pointer through current f64 FOV/camera pose; use observer-relative f64 ray versus **actual reference sphere**, nearest positive surface intersection. No GPU ID readback or triangulated-mesh picking needed for a debug sphere. This is a derived picking approximation to its reference sphere, not collision physics. If sphere centre is offscreen but visible edge is hit, selection still works.
4. **Body list:** flat scrollable list of all BodyIds with names, observer distance and current guide reference; keyboard selection reachable with markers/labels hidden or bodies offscreen/occluded.

Hit order: explicit UI/list/label targets first, then visible marker/ring, then closest sphere intersection. Mark occluded overlay picks as overlays; offer all coincident-marker candidates in a small deterministic popup or cycle with repeated/modified clicks. Stable dense body order breaks residual ties. No overlapping body becomes unreachable because another is nearer. Always retain the list fallback.

Separate click from drag by a ~4 logical-point threshold; a drag never selects on release. Replace one overwriteable pending command with a small ordered command buffer: selection before focus, viewport input after UI consumption, at most one combined orbit/zoom delta per update. Double-click focuses the body picked on that event; keyboard F focuses selection, Home returns to overview, Tab/Shift+Tab select, dedicated Next/Previous Focus both selects and transitions.

Selection highlight includes an overlay ring and selected label/list priority; visible physical spheres may use a modest derived rim/outline. Selected orbit guide and history use independent highlighted styles. None affects radius or force.

### 6.2 Markers, labels and physical-size transition

Keep distinct products:

```text
physical reference sphere → actual radius, checked sphere rendering
navigation marker        → minimum screen-space visibility and picking
selection ring           → selected identity, independent of sphere draw
label                    → display metadata, observer distance, layout rectangle
orbit guide              → instantaneous derived conic, not history
historical trail         → recorded committed samples, not a conic
```

Markers initially have configurable 8 px minimum diameter (separate ≥16 px hit diameter). For physical apparent diameter p, use opacity `1 - smoothstep(8 px, 24 px, p)`; selected ring remains visible and scales to surround the apparent sphere with a bounded gutter. Marker fallback remains fully visible if sphere precision/range fails; avoid fading the only representation. Existing 2 px mesh threshold may remain, but aids transition smoothly. Labels fade lower-priority content when close to large surfaces, with selected/hovered label retained. Zoom does not modify physical radius, positions, velocities, collision meaning, gravity, body identity or physics bounds.

### 6.3 Deterministic few-dozen-body label layout

App measures egui text once per text/zoom-unit change, supplying physical-pixel rectangles to a headless layout helper. Use short name + compact m/km/AU observer-centre distance; details/representation diagnostics on hover or inspector, not in every label.

Priority: selected, hovered, then apparent physical diameter descending, distance ascending, stable body order. For each label try eight fixed offsets around its true marker plus limited farther slots. Reject rectangles outside content viewport, intersecting protected panels/previous accepted label rectangles or covering important markers. Selected label gets first placement and a reserved edge slot if necessary. Draw a leader line to the true centre when displaced significantly. Hide/fade the lower-priority labels that have no slot; marker and list remain selectable. Keep accepted slot while valid using deterministic hysteresis to limit shimmer; invalidate on view/viewport/text changes. Layout is O(N²) with reused storage, bounded candidate count, no general UI solver.

Marker coordinates never move with label displacement. Label rectangle and leader association use BodyId. Display lists/overlap menus distinguish same-named bodies by a secondary index/ID hint; names remain metadata. DPI conversion occurs once at the UI boundary. Do not rely on glyph pixels for hit testing.

## 7. Camera, focus transitions and connected precision

### 7.1 One observer, three control policies

`CelestialCamera` continues to own one checked f64 `FramePose` plus frame-aware instantaneous derivative metadata where needed. `CameraMode` is a control policy:

| Mode | Pivot/motion | Typical use |
| --- | --- | --- |
| System orbit | Derived bounds centre in system-aligned axes; distance/yaw/pitch, optional visual-centre tracking | Whole system or selected subsystem overview |
| Body orbit | Selected focus BodyId's **translating** anchor; nonrotating axes, radial clearance/yaw/pitch | Inspect planet/moon and scale approach |
| Free flight | Same observer, camera-axis direct translation/look input, no inertia | Explore at any scale |

Keep body-fixed spin attachment as an explicit debug option under Body Orbit; it is not ordinary planetary orbit-camera behavior. Focusing a body does not mean inheriting its daily spin. There is no second camera universe, scene loading, world rebase or body teleport.

Navigation policy and fixture UI belong in `mundaris_app`. Pure fitting/zoom/layout helpers remain small headless app-library modules used by actual app/tests/benches. Existing generic `FrameEvaluation`, `UnitRotation` and `FramePose` math suffices; no new gameplay-camera crate or framework. Move a generic helper into math only after a second real independent caller justifies it.

### 7.2 Commands and continuity

- **Select** changes identity/highlights only.
- **Focus body/F/double-click** transitions to that body's translating frame and body-orbit policy, preserving a useful incoming viewing direction. First focus chooses comfortable ~4R distance; revisit restores that body's last orbit clearance/orientation. Explicit Fit Body computes a FOV-aware radius fit, not a fixed unexamined multiplier.
- **Unfocus/Escape** leaves pose/orientation unchanged and enters free flight. It removes tracking deliberately. Returning to overview is a different visible navigation action.
- **Focus next/previous** selects the next/previous BodyId in stable list order and begins the same focus transition; no star special case.
- **Return to overview/Home** stores current body view, derives current scoped bounds and smoothly transitions to System Orbit. Paused overview→body→overview restores prior system yaw/pitch/fit parameters; if authority advanced, refit current structure using that orientation.
- Switching free flight/orbit without a new focus starts at current physical pose; distance/pivot inferred in f64. No unrelated scene is loaded.

Transitions interpolate **navigation state only**. Capture source semantic frame/body role and local pose; resolve fresh handles in each coherent evaluation. Endpoint focus anchor tracks the currently committed body. Use source/target-relative f64 displacement, shortest-arc normalized DQuat interpolation and eased wall progress (`3u²−2u³`, initial 0.6–1.2 s). Interpolate log clearance/distance over orders of magnitude. Progress is based on admitted navigation wall seconds, independent of simulation rate and render FPS.

For distant focus changes use a short pull-back/transit/approach path rather than a straight line through the old or target centre: derive a transit waypoint outside the relevant physical-sphere envelope, then approach with a positive target radial clearance and directional interpolation. Near same-body changes stay in its translating frame. Check the segment/arc against the old/target reference-sphere navigation envelope in f64; this is camera convenience, not an authoritative collision system. Invalid/overlapping envelopes yield a cancelled transition with unchanged last valid observer, not world repair. User navigation cancels/rebases a transition from its current pose. A second focus request starts from that same pose without snapping.

Do not store astronomical endpoints in egui Vec3/f32 or interpolate absolute GPU positions. At end re-express pose into target frame using Phase 1 APIs, validate before replacing observer. Small offsets thereafter are calculated directly in the target translating/body-fixed frame. A coordinate re-expression computes pose and kinematic conversion atomically when preserving a defined physical derivative. Focus/unfocus deliberately change tracking policy; zero relative velocity is attachment, not claimed physical-velocity-preserving migration.

All camera/body/frame/render inputs use one committed instant. Navigation smoothing does not mix world ticks. At high warp committed body motion may still be discrete; Phase 3.5 does not introduce authoritative or hidden visual ephemeris interpolation. Tests separate smooth **camera** transitions in paused/slow fixtures from tick-visible accelerated motion.

### 7.3 Free-flight scaling and frame policy

WASD translate in current camera horizontal/forward axes; Q/E up/down, right drag look, Shift temporary multiplier. Normalize multi-axis movement. Scroll with a modifier or dedicated UI adjusts manual speed by powers of two/ten; ordinary wheel in orbit mode remains zoom. Show speed in m/s, km/s or AU/s and control mode prominently.

Use `speed_m_per_wall_s = clamp(0.5 * characteristic_distance_m, 1, 1e12) * manual_multiplier`, initial manual multiplier 1, configurable logarithmic range e.g. `1e-3..1e3`. Near an outside sphere use clearance, not centre distance; when no nearby body use focus/pivot distance or overview extent. Blend between scales in log space and apply hysteresis when nearest body changes. Examples with multiplier 1:

| Relevant clearance/scale | Base navigation speed |
| --- | --- |
| 1.7 m | 1 m/wall s floor |
| 1 km | 500 m/wall s |
| ~planetary radius 6.37e6 m | ~3.19e6 m/wall s |
| Moon distance 1e8 m | 5e7 m/wall s |
| AU scale 1.5e11 m | 7.5e10 m/wall s |

These are editor-navigation speeds, not spacecraft physical velocities. No thrust/inertia/orbit mechanics. Translation is commanded velocity × accepted wall duration; mouse deltas are event displacement, not multiplied by FPS. No motion persists after input release.

Free flight is stationary in system space without input by default; it does not silently orbit with the previous focus. Use a translating-body numerical carrier near a body (initial enter <32R, leave >64R) and root while far. Before/after a coherent frame change preserve observer pose via Phase 1 re-expression. While on a numerical carrier, compensate its committed translation delta in f64 local observer offset to maintain this system-space policy. Carrier choice is precision only, not attachment; an explicit **co-moving free flight** option may use body tracking and must say so. Do not use a rotating body-fixed carrier for ordinary free flight. Large unavoidable source quantization retains Phase 1's independent-branch budgets; no arbitrary precision promise at 1e16 m.

Navigation speed in metres/wall second is separate from `FrameVelocity`'s derivative with respect to simulation seconds. Do not pass wall-input velocity to kinematic conversion as though it were spacecraft state. Frame derivative metadata describes the chosen observer tracking policy at a scene instant; changing the policy is explicit. Paused navigation still works.

## 8. Astronomical zoom and near-surface envelope

Orbit-body distance is `d = R + clearance`, with physical R copied read-only. Control **clearance**, not just d:

```text
clearance_next = exp(log(clearance) - k * normalized_scroll)
d_next = R + max(clearance_min, clearance_next)
```

Choose k≈ln(1.25) per wheel notch as an initial tunable value. Normalize line wheel events/notches and pixel gestures consistently, keeping fractional accumulated scroll; egui logical/physical scaling is explicit. Large gestures subdivide into bounded log increments for smoothing. Checked finite arithmetic and a navigation-only maximum (initial `1e15 m` within the current finite-system use) reject overflow; disclose navigation limits rather than clamp physical state.

`clearance_min = max(1 m, 64 * ulp(R))` for current debug reference sphere, not `0.05R`. At 1 m clearance a notch changes clearance by ~20%, not hundreds of kilometres. At AU clearance the same notch changes astronomical distance quickly. Never let d<R or cross focus centre. System Orbit uses multiplicative pivot distance with a bounds/near-plane minimum, not a body's invented radius. Smooth wheel response over ~80 ms in wall time with exact target log state; no FPS-dependent repeated multipliers.

R remains real; Fit Body remains a radius/FOV-derived distance. Radius editor changes refresh the navigation envelope without modifying camera-selected identity or gravity. If body radius grows around the observer, explicitly move only the observer outward to the new envelope and report why.

Phase 3.5 approaches a **reference sphere**, not terrain. The current icosphere's ≤0.005R chordal error can be tens of kilometres; metre-scale clearance is measured above the reference sphere and does not imply a physically accurate local surface mesh. Keep this label and renderer precision fallback visible. Phase 4 receives focused BodyId, frame role, checked observer pose, radius/clearance and coherent sample instant; it chooses surface representation later.

## 9. Historical trail presentation

History remains where a body actually was at committed states. Keep synchronized f64 system samples and post-commit cadence; never fill initial empty history with guide points.

- Default enabled, solid body-colored lines on dark background, selected trail ~2.5 px, others ~1.5 px. Configurable width/opacity; prioritize selected trail contrast.
- Default display at most 1024 sampled vertices per body, selected up to 2048; retain existing up-to-8192/8 MiB synchronized storage. Allow explicit full-history display for validation. Screen-space simplification may choose **existing committed vertices only** and preserve endpoints/extrema; target ≤0.5 px deviation from stored polyline, report if display vertex cap prevents tolerance. Interpolated segments are ordinary visualization between recorded samples, not extra committed states.
- Age fade uses sample timestamps and direction/order, newest brightest with a nonzero floor (~0.15). Reverse strips fade by age within that traversal, not assumptions that increasing time is always newest. Separate counts/time span/retention from guide count. No shader alpha-only fade on the old opaque pipeline.
- Inertial System History stays default. Explicit relative reference picker accepts any recorded BodyId, with exact same-sample subtraction; label it “History relative to X at each sample, displayed at X now.” Reference body's own curve is zero; optionally suppress it as uninformative.
- Changing presentation/reference re-prepares existing synchronized records; it need not erase their physical history. Retain branch/direction/seek/edit invalidation, so no strip joins unrelated states. Hide/show does not erase. If this differs from Phase 3 mode-clearing policy, ADR 0005 records the presentation-only change.
- Historical points in root get current observer-source subtraction in f64; relative points get reference translating frame. Never use today's reference minus historical body or rotating axes accidentally.
- After first public commit show recorded span and whether projected history is still subpixel; indicate “history accumulating” when no useful visible arc exists. Guides provide immediate structure without making a misleading complete history trail.

Use generic renderer-owned pixel-width celestial polylines (Section 18). Solid age-fading history and thin dashed guide styles must remain distinguishable without relying only on color. Keep sphere depth testing/no writes for curves; optional explicitly labelled x-ray overlay is deferred unless necessary for validation. Selected local axes are debug geometry, never counted as history.

## 10. Orbit-guide mathematics and semantics

### 10.1 Ownership and reference selection

An **ORBIT GUIDE** is an instantaneous/osculating two-body conic calculated from the current committed relative position/velocity and pair mass. It is neither historical data nor a prediction of the full N-body future.

Pure checked numerical elements live in `mundaris_simulation/src/orbital_elements.rs`, where G and physical state derivation belong. This module reads f64 values but never advances/mutates world. App `orbit_guides.rs` chooses reference BodyIds, owns display policy/cache and supplies domain-free frame-tagged polyline requests. Renderer does not decide attractors or solve conics.

Evaluate these reference choices:

| Choice | Use/limitation |
| --- | --- |
| User explicitly chooses reference BodyId | Most transparent; supports comparable-mass/binary/unusual cases. Display pair and classification even if no ellipse. |
| Largest `G*mj/rij²` at body | Cheap strongest attraction, but not a binding proof; ignores differential external acceleration and can jump. |
| App-derived dominant reference with binding/tidal checks | Useful automatic default for hierarchy; still a disposable visualization relation. |
| Selected body as primary for all guides | Good temporary local-inspection action, poor automatic whole-system default. |

Choose explicit override plus conservative automatic default. For automatic candidates require strictly greater mass (avoids mutual/cyclic “parents”), finite nonzero separation, resolved nondegenerate bound relative ellipse and largest external attraction at the body. Validate differential perturbation relative to pair acceleration (Section 16); initial automatic threshold eta≤0.05, evaluated now and at guide diagnostic sample locations, is a **display-reference heuristic**, not a proven propagation tolerance. If none qualifies, show “no automatic reference”; offer explicit reference. Equal-mass binaries require explicit reference, not guessed ownership. Keep an existing valid reference until a challenger exceeds its attraction score by ~25%; explicit override always wins. Display reference and caution if perturbations are substantial.

No `primary_body` field in world. No hierarchy affects all-pair gravity or frame projection. Automatic references must not be serialized as physical truth.

### 10.2 Checked two-body construction

At one committed instant in system-aligned axes, for body b/reference q:

```text
r = x_b - x_q                  metres
v = v_b - v_q                  metres/second
mu = G * (m_b + m_q)           m³/s², checked/scaled mass arithmetic
H = r × v                      specific angular momentum
epsilon = |v|²/2 - mu/|r|      specific two-body energy
e_vec = (v × H)/mu - r/|r|
e = |e_vec|
p = |H|² / mu                  semilatus rectum
a = -mu / (2*epsilon)          ellipse only
normal = H / |H|
```

Use scaled/robust norms and finite-overflow checks consistent with gravity code. Classify with dimensionless normalized energy `epsilon/(|v|²/2+mu/|r|)` and scale-aware angular-momentum conditioning, not a dimensional universal epsilon. Starting tolerances (~1e-10 near-parabolic normalized energy, ~1e-12 relative angular-momentum degeneracy) are documented numeric classifications to test, not force-law softening. Cross-check e/energy/p consistency; return an explicit ill-conditioned result when they disagree beyond tolerance.

For a well-conditioned ellipse (`epsilon<0`, `0≤e<1`, p>0): periapsis axis P=e_vec/e, Q=normal×P. For nearly circular e use normalized current r for P to avoid undefined periapsis; keep deterministic basis orientation and report periapsis undefined. Do not use ascending-node/inclination Euler singularities to build geometry.

For eccentric anomaly E in `[0, 2π]`:

```text
r_guide(E) = a*(cos(E)-e)*P + a*sqrt(1-e²)*sin(E)*Q
```

The reference is at a focus, not ellipse centre. Draw relative r_guide in the **current reference translating frame**, with system-aligned axes; its present position maps onto this relative conic within numerical tolerance. For comparable masses this is the relative separation conic anchored to reference now, not both bodies' barycentric trajectory; UI must say so. Primary has no fake self-orbit guide. R is irrelevant to conic equations except optional display annotation of reference-sphere intersection.

Guide metadata: body/reference IDs, current sampled instant/revision, conic class, a/e/p, periapsis/apoapsis, reference-selection source, perturbation caution and numerical failure reason. Never authoritative orbital elements replacing positions/velocities.

### 10.3 Sampling, updating and invalid states

Generate 64 initial E segments, deterministic screen-curvature refinement to ~0.5 physical px where possible, cap 512 segments/guide (selected 1024). Conservative exact extrema contribute to framing regardless of sample resolution. Zoom may refine derived tessellation; it does not change elements. If cap misses tolerance, report coarse guide. Thin dashed (~1 px) style, selected ~1.5 px; persistent legend “dashed = instantaneous two-body guide; solid/fading = committed history”. No guide is appended to TrailHistory.

Recompute elements from newest committed state after each pump when displayed. For small N cost is tiny; tessellation can be reused while projected error/bounds remain within tolerance. Any reuse is tagged with displayed sample time/revision and cannot present stale elements as current. If later throttled for large counts, show the sampled instant and “updating”, invalidate immediately on edits/reference change/seek. N-body perturbations therefore update the osculating guide rather than freezing the initial ellipse.

| State | Phase 3.5 behavior |
| --- | --- |
| Well-conditioned ellipse | Closed relative guide and metadata. |
| Hyperbolic (`epsilon>0`, e>1) | Correctly classify, show “unbound hyperbolic; closed guide unavailable”. **Do not draw an ellipse.** Bounded open hyperbolic arcs are later scope. |
| Parabolic/near-parabolic | Classify as near-parabolic/ill-conditioned; no enormous guessed ellipse or infinite bounds. Open universal-variable arc later. |
| Radial/zero-H, zero separation, invalid arithmetic | Degenerate/numerical-invalid diagnostic; no line, finite framing unaffected. |
| No sensible dominant reference | Marker/list/history remain; user may explicitly choose reference and see honest conic class. |
| Bound ellipse with strong perturbation | Explicit-reference guide can draw with “instantaneous, strongly perturbed” caution; automatic reference may be unavailable. Not an accuracy guarantee. |

No analytic propagation is necessary to draw an ellipse. Do not extrapolate spins/centres or fake orbital histories to populate guides.

## 11. Navigation-only body relationships

**Defer a collapsible authoritative-looking tree.** Use a flat body list with a “guide reference: X (automatic/explicit)” column and Selected Subsystem action. This provides the fixture's Solace/Aurelia/Luma structure without implying permanent ownership or misrepresenting binaries/capture events.

Local satellites are the inverse set of current accepted **automatic** reference edges, filtered to the selected scope. Explicit user reference overrides do not silently reparent list/subsystem membership; users can explicitly include those bodies. Strictly increasing automatic reference mass prevents cycles; ambiguous references stay flat. Relation may change; freeze chosen subsystem membership during a navigation transition and re-evaluate afterward with an explanatory UI change. All translating frames remain root children.

If a grouped view later proves valuable, name it **Derived navigation grouping**, retain an Ungrouped section, surface reference changes and break explicit-reference cycles in display only. It is not necessary to complete Phase 3.5.

## 12. Deterministic high-speed time-warp method comparison

No propagation decision is described as probability. Methods below are deterministic given explicit initial conditions, parameters, step schedules and evaluation order; deterministic does not mean accurate or valid for every N-body system.

| Method | Advantage | Accuracy/architectural costs | Decision |
| --- | --- | --- | --- |
| More identical baseline fixed-step KDK | Same Phase 3 trajectory per tick; current replay/history already work; cached one force pass | Work ∝ requested simulation span/h × N²; scheduling limit and long exact replay remain | **Phase 3.5 authority** |
| Discrete larger fixed symplectic timestep ladder | Simple KDK reuse, fewer steps; constant h on a branch retains symplectic structure | Changes trajectory, phase error O(h²), shortest moon/periapsis/encounter limits; history/tick/replay need explicit branch change | First modest future candidate after measurement/convergence; not needed now |
| Adaptive timestep integrator | Resolves eccentric encounters with local error control | Naive state-dependent variable h KDK is generally not symplectic; needs suitable integrator and deterministic accepted/rejected schedule; extra force passes/state/history | Later robustness/integrator evaluation, not a warp-rate multiplier |
| Hierarchical/multi-rate stepping | Fast moons substep while slow planets use longer intervals | Mutual force/reaction coupling, kicks/synchronization/encounters/reverse require rigorous Hamiltonian split; asynchronous full world publication invalid | Later dedicated performance phase only if moon-limited work dominates |
| Analytic Kepler/two-body propagation | Near-Kepler motion over huge spans at low cost; good preview candidate | External perturbations ignored/approximated, nested COM transport/reaction and changing validity need explicit model; mass ratio alone insufficient | Later **preview first**; authoritative approximate mode requires separate evidence |
| Wisdom–Holman/near-Kepler symplectic mapping | Exact dominant Kepler drift plus perturbing kicks, full chosen Hamiltonian split; bounded long-run error in appropriate regimes | Canonical heliocentric/Jacobi coordinates, central-star motion, binary/nested moons, resonances/encounters and correctors; still timesteps, no unlimited warp | Preferred later experiment for near-Kepler planetary authority, compare with ladder |
| Visual-only extrapolation while exact authority lags | Responsive future/configuration view without asserting full simulation | Two times/states must remain separate; preview invalidity and authority lag visible; cannot count preview as achieved simulation | Later isolated preview; never silently enabled by rate |

Wisdom–Holman is not just independent ellipses: a validated splitting can retain the full Newtonian Hamiltonian with numerical splitting error rather than discard perturbations. Neither it nor symplecticity proves phase accuracy or close-encounter safety. RK/adaptive alternatives can be justified for encounters, but shouldn't inherit “Exact Phase 3” labeling.

No one method spans short moons, perturbed binaries, encounters and geological-scale preview. First preserve exact baseline and measure **simulated seconds per real second at accepted accuracy**, not just faster step throughput.

## 13. Chosen Phase 3.5 playback work

### 13.1 Exact fidelity and bounded scheduling

Retain baseline h10/h60, KDK equations, pair order, chi≤0.02 guards, per-step commits, history/replay and overload admission semantics. Selecting any speed changes only requested counts. Approximation never turns on automatically at 100000x or overload. No live physical timestep slider is added.

Add a narrow `pump_with_work_limit(system, max_work_units, after_commit)` API to runner, validating `1..=config.work_limit`; existing `pump` delegates to configured cap. Limits bound scheduling **including private replay and restores**, not numerical method. App breaks a drawable work opportunity into chunks (initial 1 to estimate cost, then at most 32; total at most 512), checks monotonic work time **between** chunks and stops after a configurable interactive budget (initial 4 ms). Sum counters across chunks; retain final runner metadata. No physics kernel reads wall time. One step remains indivisible: a 13 ms step may exceed budget, so do not claim preemption or guaranteed 60 FPS for N1024.

This fixes a count cap permitting seven-second blocking work without adding threads. Time-dependent chunk choices can alter how quickly debt drains, **not physical trajectory for equal committed tick targets**. Repeated zero-duration wakeups and surface retries do not create demand; extra pumps are work opportunities only. Caller-controlled limit tests prove baseline `pump` and chunked pumping yield identical state/history/trail cadence after draining identical admitted targets.

Keep cap/headroom diagnostics separate: pending demand, count-limited, CPU-budget-limited and numerical failure. Classify limitations using observed work and scheduler metrics; do not assert physics is slow when count admission is the limiter. N1024 UI stress remains an optional headless/synthetic scheduling workload, not a required normal scenario or new broad fixture system.

### 13.2 User-facing status

Prominent compact bar, detailed engineering section below:

```text
Requested 10000x | Achieved 9970x (10 s window)
Baseline exact • full N-body KDK • physical step 60 s
Authority 2.34 days | requested 2.35 days
Pending 14 ticks / 840 s • lagging, CPU budget limited
```

Also show paused/suspended/measurement warming/origin/replay status, signed reverse rate, rejected interval and explicit cancelled debt. Examples “achieving 10000x using approximate Kepler” or “only 3500x” are future/status scenarios, never hardcoded claims. All values must derive from authority/report/method metadata.

- Include presets 1/100/1000/**10000**/100000/1000000x and a finite custom rate input; high requested speeds stay exact and may lag. Changing rate alone still does not resume; visually obvious Resume control.
- Overload warning explains admitted debt drains at unchanged h and new intervals are rejected; offer Lower Rate, Pause/Cancel Debt and Resume Admission with their distinct semantics. Estimate drain latency from actual measured work cost, label estimate, don't change h.
- Short waits at h60/1x show fractional tick progress and “next committed step in …”; lack of motion is not false overload.
- Reverse reports Snapshot Restore or Exact Positive Replay as the advancement operation while the branch's propagation method remains KDK. Internal replay steps/s is separate from achieved live playback.

### 13.3 Achieved-rate measurement

App-owned `PlaybackMetrics` consumes actual live playback commit deltas and **active accounted wall durations** once per host update. Use rolling ~10 s window by default (bounded history of timing/count aggregates), with a 1 s responsiveness indicator if useful. At rates where h/r exceeds window, show “tick-limited sample” and also since-segment average with elapsed window; never smooth by assuming requested rate was achieved.

Reset measurement on rate/sign/pause/resume/suspension/stall reset/scenario/rebranch. Exclude explicit seek/reset/single/replay completion jumps from **playback** numerator; reset anchors at their boundaries. Live reverse snapshot traversal counts signed committed elapsed, but an outside-retention replay jump is shown as seek/replay completion, not a burst of playback speed. Keep private work throughput, live achieved rate and authority-to-request lag separately. While replay freezes demand, rate reads unavailable/replaying rather than synthetic advancement. Metric windows include pump/render stalls during otherwise active short intervals so overload cannot look faster by excluding its CPU cost.

Quantization means achieved need not equal requested on a finite window even when all full ticks are caught up; display fractional progress and window length. No fake percentages or requested-rate copying.

## 14. Future quality regimes and coherent handoff

Explicit regimes are appropriate **when implemented and validated**, not as four empty enum variants today:

| Future regime | Authority/expectation |
| --- | --- |
| Baseline Exact | Phase 3 h and full N-body KDK; existing accuracy/replay envelopes |
| Controlled Fast | Explicit larger fixed KDK ladder or validated full-Hamiltonian mapping; numerical trajectory changes, documented weaker/different accuracy |
| Extreme Approximate | Explicit specialized perturbation-limited propagation; model omissions and validity span displayed |
| Preview | Separate non-authoritative state/time; no live world/history mutation |

Requested speed and quality are orthogonal controls. Faster rate never selects a regime. Future transitions must be explicit commands with method/h and consequences visible, validated **before** old session is discarded. At a full-time coherent committed boundary capture current f64 positions/velocities/orientations/spins and sample time; initialize new caches/coordinate transformations, verify inverse transform residuals, cancel outstanding demand explicitly and begin a new branch/diagnostic baseline. `BodyId`/properties/instant/state values survive handoff. Position/velocity continuity is possible; accelerations may change when the physical approximation changes, and accumulated past error cannot be undone.

Do not splice variable-h ticks into existing `epoch + tick*h` history or replay a different method as though it were the original branch. The simple first implementation would reset branch history/trails on method/h change, documenting loss of old reverse retention. Cross-method replay eventually needs recorded method/parameter/schedule provenance; that is later scope, not a save-game design here. Failed preflight retains old world and runner. Preview has no promotion API to world.

## 15. Strict future larger-timestep policy

Never use `h = base_h * playback_rate`. Evaluate a ladder, e.g. base h × `[1,2,4,8,16]`, selected only by explicit quality/h action and constrained by the system. Values are candidate experiments, not authorized Phase 3.5 presets.

For every dynamically relevant pair, including moon/planet:

- Bound orbital period T and **periapsis timescale** `tau_p = r_p/v_p` for eccentric ellipses; an outer annual orbit cannot justify a step that underresolves a days-period moon or a brief periapsis.
- Current/all-pair gravitational timescale `sqrt(r³/(G*(mi+mj)))`; maintain existing chi≤0.02 at old/candidate positions. Circular chi criterion implies h/T≤0.02/(2π), but is an envelope, not proof of accuracy.
- Crossing/encounter timescale `r/|v_rel|` (especially unbound/high-speed encounters), pair approach/periapsis forecast and third-body coupling. Zero relative velocity is not infinite guaranteed safety.
- Trial safety requirements such as h≤T/1000, h≤tau_p/100 and h≤0.01*r/|v_rel| are conservative **starting experimental limits**, to refine against convergence/phase/encounter tests. All-pair guard remains; guide automatic references alone cannot screen out dangerous pairs.

Strict refusal on unavailable/ill-conditioned timescale for a required safety check, predicted close encounter, exceeded guard, unsupported perturbed/binary case or unacceptable step-comparison error. Preflight complete candidate and periodically reassess; on validity loss retain last good state and halt visibly. **Do not silently lower h within that branch**: prompt explicit transition to a smaller validated ladder branch, or a later designed integrator with recorded adaptive schedule. An app may suggest a reduced step; it cannot quietly change fidelity to meet requested speed.

One global ladder still takes the shortest important timescale. Very unequal periods reduce its benefit; this is the later multi-rate/mapping justification, not a reason to ignore moons. Maximum safe h and expected speed gain are system-derived diagnostics, not fixed playback-rate thresholds. No guarantee a moon “cannot explode” follows from a single timescale ratio; trial validation/transactionality and honest refusal are required.

## 16. Error feedback and analytic-propagation evaluation

### 16.1 Technically justified feedback

Future controlled modes can combine:

- Occasional **one h step versus two h/2 steps** from identical private full-time state; for second-order KDK, fine local error is approximately difference/3 in the asymptotic regime. Compare scaled position/velocity for every body/pair, especially moon relative motion. Discard both diagnostic trajectories; do not commit them as extra history. More subdivisions/checkpoints test whether asymptotic estimate applies.
- Energy/P/L/COM drift and local orbital phase/separation versus a smaller-step baseline. Bounded energy can coexist with significant phase error; huge star energy can hide lunar error. Conservation alone is insufficient.
- Timescale/periapsis ratios and differential perturbation diagnostics for analytic modes.

Publish “within validated indicators”, “degraded indicators”, “model validity failed” only with actual thresholds established by convergence fixtures and their sampled tick/horizon. Do not claim “99.9% accurate”, a guaranteed future position error or uncertainty distribution. Phase 3.5 reports existing diagnostics/guards and baseline method; no new unverified accuracy badge is added.

### 16.2 Near-Kepler pairs need differential external acceleration

For pair b/q measure:

```text
a_pair_rel = -G*(m_b+m_q) * r/|r|³
a_external_rel = Σ(k != b,q) [a_k(x_b) - a_k(x_q)]
eta = |a_external_rel| / |a_pair_rel|
```

Both accelerations in that difference use the same external body's mass/current position. Uniform external acceleration cancels; raw acceleration at a moon includes the star but is not by itself its relative perturbation. Also monitor per-perturber magnitudes to expose cancellation and spatial/tidal variation over periapsis/apoapsis and the **proposed propagation horizon**. A small current eta, a large mass ratio or a binding negative energy alone is insufficient; resonances, secular drift, changing encounters and accumulated phase all matter.

Extreme analytic candidacy requires well-conditioned bound elements, small differential perturbation over a validated limited horizon, no predicted encounter/reference switching, acceptable comparison with full N-body and documented maximum horizon. Set numeric propagation thresholds through benchmarks/tests; the eta≤0.05 navigation-reference heuristic is not permission for authoritative analytic propagation.

For nested planet/moon systems: transport internal relative Kepler pair about their **combined barycentre**, then propagate outer barycentre/star pair with appropriate combined mass and reacting star, reconstruct both bodies mass-weightedly from barycentre and relative coordinates. Do not freeze star or simply add independent moon motion to a planet ellipse without accounting for recoil. Independent dropped interactions remain an approximation. A full correction scheme needs an explicit Hamiltonian/perturbation split; binaries and overlapping dominance may be unsupported. Preserve orientation/spin coherently at the same sampled instant.

Initial research should use this in isolated **preview**, because nested coupling and accumulated perturbation validity are not solved by the present runner. A later authoritative approximate mode must label omissions, record branch method, and stop on invalidity. Returning to full N-body initializes its workspace from the preview-independent authoritative approximation's current **full-time** f64 state, retaining positions/velocities/IDs and recomputing all forces. It continues from an approximate state; it does not recover the exact baseline future. Continuity guarantees are position/velocity/spin state within conversion roundoff at one instant, not unchanged acceleration/energy derivative or erased accumulated error.

## 17. Multi-rate and preview boundaries

### Multi-rate

Potential value: a days-period moon sets global h while outer planets take decades. A deterministic recursive fast/slow force split could substep internal pairs and couple slower interactions at synchronized boundaries. Mutual reactions, angular momentum, close encounters, hierarchical changes, Hamiltonian splitting and history restoration require careful design. Bodies cannot simply evolve asynchronously then be published with a shared timestamp pretending coherence.

**Placement: later dedicated orbital-performance phase.** Compare a global safe ladder and near-Kepler mapping first. Require representative 3–20-body multi-timescale workloads to show an accuracy-qualified benefit over their added complexity; N1024 force cost alone is unrelated evidence. No per-body timestep fields or unused scheduler framework in Phase 3.5.

### Preview

A future fast preview can inspect an approximate future configuration/orbit-guide evolution and choose a target time. Own `PreviewState` separately from `CelestialSystem`/live projection/runner, tagged with source branch/time, preview time/method/validity. Use a separate **derived projection** for that state within the same coordinate conventions, never a disconnected scene or extra authoritative universe.

Render a persistent “PREVIEW — approximate, authority remains at …” banner and distinct visual treatment. Preview movement is not achieved playback; preview samples never enter world revision, snapshot history or TrailHistory. Exiting returns the same live authority. Commit **target time**, not preview state: exact baseline must advance/reset/replay through real steps, reporting work/latency/cancel; far-future accuracy may remain impractical. Do not imply geological-scale exact replay becomes cheap because preview is analytic. Physical editing invalidates preview.

**Design-only in Phase 3.5**: no timeline editor, future scrubber, prediction overlay, preview promotion or unimplemented UI quality choices. Geological-scale exploration would use specialized explicit approximations/model time, not fabricate trillions of integrated steps.

## 18. Rendering ownership and polyline boundary

Keep renderer → math only. App retains BodyId/name/style mapping; renderer outputs transient request indices and domain-free prepared screen geometry. The selected identity is app-owned BodyId even when the renderer uses a request index internally.

```text
world immutable current state
  → simulation pure two-body element query (no mutations)
  → app guide reference, tessellation, system bounds, selection and history
  → renderer generic f64 frame-tagged sphere/polyline/overlay preparation
  → checked camera-relative/clipped data
  → GPU + app label/marker UI
```

Introduce a narrow generic celestial polyline request with frame-tagged f64 points, per-point opacity/age-derived color, width in physical pixels and line style (`Solid`, `Dashed`). App owns semantics: history/guide/axes are separate request groups/counts, not inferred in GPU code. Guide computation/references are outside renderer. Avoid naming all arbitrary lines `append_historical_lines` or counting selected axes as history.

Use one explicit renderer-owned content viewport rectangle (physical-pixel origin and nonzero size) for projection, ray unprojection and overlay coordinates. `CelestialProjection` retains its f64 FOV and that rectangle; GPU celestial pass sets matching viewport/scissor, and projected egui positions add the rectangle origin once. Fitting consumes the same rectangle. Existing full-window callers use origin zero. Docking panels must not leave camera fit using a smaller aspect while the GPU still projects across the whole window; letterboxing, high-DPI and off-centre picks are part of the headless projection tests.

For useful portable line widths and dashes, use renderer-owned segment quads, not native line-list width assumptions. Convert/clip endpoints in f64 through existing source-centred path; calculate screen-space tangent, bounded pixel offsets, join caps and accumulated dash length **after projection**. Generate camera-relative/clip data with the same reverse-Z endpoint depth; no astronomical absolute f32 positions. Near-plane crossings are clipped before normalization; discard zero-projected-length segments without hiding invalid source numbers. Preserve screen-error budget, finite input validation and frame-poisoning/full upload contract. Use alpha blending for age fade, no depth writes, defined overlap/join behavior; test widths/dash continuity/DPI headlessly. Retain ordinary Phase 1 debug lines separately.

Physical sphere preparation stays current 642 vertices/1280 triangles/source-centred normals/debug shading. No terrain mesh detail, surface texture, lighting or renderer-selected gravity parent. Marker fade is driven by actual projected sphere size and fallback state, not altered physical radius. Document useful rendering range separately from navigation bounds; farther bodies may be marker/bearing-only.

## 19. Safe interactive elapsed-time policy

Default **simulation pauses with the interactive app**; no background workers or automatic elapsed-time catch-up. Keep existing detected-hidden behavior and add shared host-clock classification in app, separate from `TimeController` and physical runner.

| Situation | Required behavior |
| --- | --- |
| Minimized, occluded, zero-size, OS suspension | Suspend work/admission, explicitly cancel debt/replay, remember user pause/rate, reset capture. First restored capture admits zero hidden time. |
| Sleep/resume without lifecycle event | Treat elapsed >250 ms as discontinuity by default; reject **entire interval**, cancel prior pending debt explicitly, suspend automatic playback until Resume; set fresh timestamp. |
| Debugger pause / several-second stall / rendering stopped | Same conservative discontinuity policy, with “interactive clock gap; no catch-up requested”. No distinction based on guessed cause. |
| Brief stall ≤250 ms while drawable | Account full elapsed; runner cap/debt remains honest. Include cost in rate sample. |
| Recoverable surface retry | No double commit/admission. If retries/redraw gap trigger discontinuity, use gap policy; lost frame itself is not a physical rollback. |
| User pause | Cancel debt explicitly; camera/UI still work; resume fresh active segment. |

250 ms is an initial documented interactive policy, not integrator dt or universally detected sleep. A very slow drawable machine may pause often: show cause and allow an explicit session threshold change, never hide debt by clamping elapsed to 250 ms. This is safer than treating hours of laptop sleep as simulation demand. Record excluded wall interval, reason and cancelled requested seconds separately from overload **rejected simulation interval**. Camera animations/input also reset on discontinuity; no giant free-flight jump.

Factor native event classification in `main.rs` and pure `InteractiveClock` into app library. Deliver hidden transitions immediately on resize/occlusion/suspend and again at `about_to_wait`; use idempotent state changes. Apply host capture reset to all modes; analytic Phase 1/2 producers remain explicit samples with their existing limits. No long-wait unit test uses actual sleeping.

An eventual explicitly configured “continue elapsed-time catch-up” policy may admit measured time under the existing debt cap, with UI warnings/status and separate background execution design. It is **not implemented** in Phase 3.5 and never the default.

## 20. Representative Rust API sketches

These are semantic sketches, not implementation, compile-ready signatures or a general framework. Checked types keep private fields; expose only real app/test/benchmark callers. Module-private helpers use `pub(crate)` where possible. App public facade is limited to useful headless navigation/guide/metrics types; errors use existing anyhow at composition and focused library errors.

```rust
// mundaris_app: BodyId is selection/focus identity; frame roles are disposable.
pub enum CameraMode { SystemOrbit, BodyOrbit, FreeFlight }
pub enum OverviewScope {
    WholeSystem,
    SelectedSubsystem(BodyId),
    ExplicitBodies(Vec<BodyId>),
}
pub enum FocusTarget { Overview, Body(BodyId) }

pub struct SystemViewBounds {
    // private: f64 anchor + offset centre/extents, included IDs,
    // guide/history contribution and omitted-fit-extent diagnostics
}
pub struct NavigationInput {
    // f64 drag/look/log-zoom values, normalized translation command,
    // manual speed multiplier; no physical thrust
}
impl CelestialCamera {
    pub fn transition_to(
        &mut self, target: FocusTarget, pair: &CoherentCelestialView<'_>,
        bounds: Option<&SystemViewBounds>,
    ) -> Result<()>;
    pub fn update_navigation(
        &mut self, pair: &CoherentCelestialView<'_>,
        input: &NavigationInput, elapsed: Duration,
    ) -> Result<()>;
    pub fn pose(&self) -> FramePose;
    // Re-expression/remapping remain Phase 1/body-role operations.
}

pub struct BodySelection { /* private Option<BodyId> */ }
pub struct CelestialVisualBody {
    // app-only: BodyId + domain-free renderer request + style/name association
}
pub struct BodyHitTarget {
    // app-only: BodyId, screen marker/ring, displaced label rect,
    // f64 observer-relative physical sphere, occlusion metadata
}
pub fn pick_body(
    targets: &[BodyHitTarget], pointer_pixels: [f64; 2],
    projection: CelestialProjection,
) -> PickResult; // none, unique BodyId, or ordered overlapping candidates
```

```rust
// mundaris_simulation: pure f64 derivation, no FrameId, rendering or mutation.
pub enum ConicClass { Elliptic, Hyperbolic, NearParabolic, Degenerate }
pub struct TwoBodyElements { /* checked private a/e/p, P/Q/normal, classification */ }
pub fn osculating_elements(
    relative_position_m: DVec3,
    relative_velocity_m_s: DVec3,
    first_mass_kg: f64, second_mass_kg: f64,
) -> Result<TwoBodyElements, OrbitalElementError>;
impl TwoBodyElements {
    pub fn class(&self) -> ConicClass;
    pub fn elliptic_position_m(&self, eccentric_anomaly_rad: f64)
        -> Result<DVec3, OrbitalElementError>;
}
impl FixedStepRunner {
    pub fn pump_with_work_limit(
        &mut self, system: &mut CelestialSystem, max_work_units: u32,
        after_commit: impl FnMut(u64, &CelestialSystem),
    ) -> Result<SimulationAdvanceReport, SimulationPumpError>;
}

// mundaris_app: policy and identity wrapping, never stored in world BodyState.
pub enum OrbitGuideReference { Automatic, Explicit(BodyId), None }
pub struct OrbitGuide {
    // BodyId, chosen reference BodyId/source, sampled instant/revision,
    // checked elements/class/caution, f64 relative visualization vertices
}
pub struct TimeWarpStatus {
    // requested rate, measured achieved rate/window or unavailable reason,
    // immutable method label "full N-body KDK", baseline h and exact-baseline flag,
    // authoritative/requested instants, admitted debt/fraction/status,
    // separate replay, budget limitation, excluded gap/rejected/cancelled values
}
// No fake Fast/Extreme implementation variants or unused propagator traits.
```

```rust
// mundaris_renderer: domain-free visual requests; app maps indices to BodyId.
pub enum CelestialLineStyle { Solid, Dashed }
pub struct CelestialPolyline<'a> {
    pub points: &'a [FramePosition], // all same source, f64 math wrappers
    pub colors: &'a [[f32; 4]],      // derived visual opacity, never physics
    pub width_pixels: f32,
    pub style: CelestialLineStyle,
}
impl CelestialFrame<'_, '_, '_> {
    pub fn append_polylines(
        &mut self, lines: &[CelestialPolyline<'_>],
    ) -> Result<(), RenderPreparationError>;
    // Existing sphere/marker output stays transient and view-bound.
}
```

Renderer `CelestialMarker` can add apparent diameter/fallback/fade-ready metadata but no world dependency/BodyId. Semantic `OrbitGuide`/`HistoricalTrail` ownership stays app; both map to explicit generic styled lines. No guide/history API accepts `&mut CelestialSystem`. Physical evolution remains runner-only through existing world validation.

## 21. Exact future implementation file plan

This is the complete planned change surface for the **chosen** Phase 3.5 implementation. It authorizes no source edit during this design task. No new crate/dependency version is needed. Each row identifies visibility, owner/dependencies and verification; new test files use public app-library or simulation APIs, private edge cases stay colocated.

### App

| File / operation | Responsibility, API/ownership/dependencies | Expected tests |
| --- | --- | --- |
| `crates/app/src/celestial_camera.rs` — modify | `CelestialCamera`, public useful `CameraMode`/`FocusTarget`, private transition/orbit/free-flight states; one observer; app→math/world, existing glam | Focus every ID, no world mutation, continuity, frame remap, scale zoom, free-flight invariance |
| `crates/app/src/system_view.rs` — new | App-library scoped bounds/fitting helpers and `OverviewScope`/`SystemViewBounds`; read-only world/math and guide/display bounds, no renderer mutation | Geometric fit, outlier, history cap/full fit, subsystem, aspect/DPI, empty/single |
| `crates/app/src/celestial_selection.rs` — new | `BodySelection`/hit target/results exposed for actual headless callers; private gesture arbitration; app owns BodyId, renderer projection/math inputs | Sphere/marker/label/list equivalence, occluded/overlap/offscreen/DPI, duplicate names, stale IDs |
| `crates/app/src/celestial_labels.rs` — new | Deterministic layout on measured screen rects; public small layout input/output for benches, private candidates/cache; app UI policy | Priorities, nonoverlap, hiding, leaders, stability and displaced hits |
| `crates/app/src/orbit_guides.rs` — new | Reference policy, `OrbitGuideReference`/guide metadata/cache, relative tessellation and subsystem associations; app→simulation/world/math/renderer requests | Reference selection/override/ambiguity, tidal ratio, cache instant, no authoritative mutation |
| `crates/app/src/playback_metrics.rs` — new | `PlaybackMetrics`/`TimeWarpStatus` facade, private bounded wall/commit aggregates, exclusions and replay metrics; simulation reports and std Duration | Quantization, signed reverse, windows/rate resets, seek/reset/jump exclusion, status honesty |
| `crates/app/src/interactive_clock.rs` — new | Pure drawable/gap clock policy and reason/status; app-only monotonic captures, no physics; shared by modes | Minimize/occlude/zero-size/sleep/debug gap/stall/restore, once-only admission |
| `crates/app/src/trails.rs` — modify | Keep synchronized f64 real history; expose read-only timestamp/sample access sufficient for bounds/display, private simplification/style assembly and BodyId reference; no simulated-history fabrication | Cadence/eviction/invalidation, presentation change retains records, selected fade, relative subtraction/precision |
| `crates/app/src/gravity_orbits.rs` — modify | Compose modules, ordered command queue, list/toolbar, guides/trails/marker toggles, bounded chunk pumping and reports; world/runner ownership stays here | End-to-end commands, chunks/callbacks, selection/focus, replay/overload/lifecycle and no visualization mutation |
| `crates/app/src/lib.rs` — modify | Deliberate module declarations/minimal headless app exports; Rustdoc semantic boundary | All app integrations compile; docs contracts |
| `crates/app/src/main.rs` — modify | Native lifecycle delivery/capture routing, existing flags; integrate clock events without duplicate elapsed | Native Windows/Linux lifecycle and headless clock wiring |
| `crates/app/src/reference_frames.rs` — modify | Consume shared safe elapsed/reset policy; preserve analytic samples/approach/precision and mode UI | Existing replay/handoff tests plus hidden/gap exclusion |
| `crates/app/src/celestial_model.rs` — modify | Shared safe elapsed/reset policy only; retain analytic bounded time/edit/re-expression | Existing deterministic sampling/edit tests plus hidden/gap exclusion |
| `crates/app/tests/celestial_navigation.rs` — new | Public headless app/world/frame math integration; no GPU | Section 22 camera/fit/selection round trips |
| `crates/app/tests/celestial_system_view.rs` — new | Headless layout/guide/trail/read-only assembly integration | Semantic separation, references, labels, marker fading, no mutation |
| `crates/app/tests/playback_reporting.rs` — new | Headless clock/metric/runner composition | Requested/achieved/debt/gaps/replay exclusion/chunk limits |
| `crates/app/benches/celestial_navigation.rs` — new | Real view bounds, selection, labels and transitions in reused buffers; Criterion | Verify outputs before timing; N3/16/64/256 and astronomical fixtures |
| `crates/app/benches/orbit_guides.rs` — new | Reference/element/tessellation/bounds prep separately; immutable systems | Bound/circular/eccentric/unsupported correctness outside timing |
| `crates/app/benches/trail_history.rs` — modify | Keep historical baseline groups, add bounded display/reference/highlight preparation and exact h60 end-to-end throughput | Actual commits and source provenance checked outside timing |
| `crates/app/Cargo.toml` — modify | Register two new Criterion targets; existing dependencies suffice | Locked all-target compile |

### Simulation and renderer

| File / operation | Responsibility, API/ownership/dependencies | Expected tests |
| --- | --- | --- |
| `crates/simulation/src/orbital_elements.rs` — new | Pure checked `TwoBodyElements`/classification/error/evaluation public API, private robust math; simulation→glam/math, existing G | Independent conic oracle, numeric degeneracy/overflow/conditioning; no stepping |
| `crates/simulation/src/lib.rs` — modify | Small element-query exports/Rustdoc, no integrator trait/regime scaffolding | Rustdoc and compile/API use |
| `crates/simulation/src/runner.rs` — modify | Validated caller-supplied bounded pump limit, existing `pump` delegation/getter as needed; same private numerical runner | Exact chunk equivalence, bad budget transactionality, replay/restore work accounting |
| `crates/simulation/tests/orbital_elements.rs` — new | Independent circular/eccentric inclined element/conic answers, unsupported classification | Section 22 numerical cases |
| `crates/simulation/tests/fixed_steps.rs` — modify | Work-limit scheduling regressions while retaining all baseline history/replay/FPS tests | Same target bits for different chunks, callback counts, cap preservation |
| `crates/simulation/benches/fixed_steps.rs` — modify | Exact throughput with chunks plus representative moon-limited planetary fixtures; retain scale groups | Record h/body/accuracy context, committed/history overhead |
| `crates/renderer/src/celestial.rs` — modify | Sphere output apparent diameter, generic polyline integration and separate prep counters; existing public sphere/marker types, private GPU resources | Fades/fallbacks and no physical radius mutation, poisoning; keep existing precision |
| `crates/renderer/src/celestial_view.rs` — modify | Explicit content viewport origin/size and matching f64 projection/ray unprojection/projected sphere bound/query helpers and clipping inputs; domain-free | Viewport-offset/DPI agreement, large sphere off-centre hits, behind/near/zero cases, error budgets |
| `crates/renderer/src/celestial_lines.rs` — new | Public small domain-free polyline/style requests, private clipped quad/dash/width/packing preparation/GPU resources; math/glam/wgpu | Width/dash/fade/DPI, near clipping, joins/zero-length/invalid/poison |
| `crates/renderer/src/shaders/celestial_lines.wgsl` — new | Explicit observer/clip-relative quad and alpha/dash shading contract; no physics/IDs | Matching naga parse/validation/layout tests |
| `crates/renderer/src/lib.rs` — modify | Export used requests/helpers; renderer wiring/polyline draw before egui, preserve smoke/debug paths | Shader resource/lifecycle compile and native rendering |
| `crates/renderer/tests/celestial_precision.rs` — modify | Maintain source-centred sphere/view envelope, add apparent diameter/fallback/ray helpers and styled lines | ≤0.05 px narrowing, no absolute f32, physical radius/poison/layout |
| `crates/renderer/benches/celestial_preparation.rs` — modify | Separate marker/sphere/styled line preparation, retain baseline workload sizes | Valid nonconstant inputs/output observation, preallocated warm storage |

Keep `celestial_trails.wgsl` and its existing compatibility line method for Phase 3/reference-axis callers initially; route new semantic guide/history requests through generic styled polylines. Do not delete unrelated source. `gravity.rs`, `integrator.rs`, `history.rs`, world/math/core source, `gravity_fixtures.rs`, `redraw.rs`, root manifests/lockfile and current CI require no changes in this chosen scope. If implementation discovers a genuine need outside this exact list, record the reason and revise the plan before changing it.

### Documentation at implementation time

| File / operation | Responsibility / verification |
| --- | --- |
| `MUNDARIS_PHASE_3_5_CELESTIAL_NAVIGATION_SYSTEM_VIEW_AND_TIMEWARP.md` — update | Actual APIs/status/acceptance evidence; all claims traced to tests/records |
| `README.md` — modify | Implemented navigation controls/mode/quality/status; current links |
| `MUNDARIS_ENGINE_DESIGN.md` — modify | Phase 3.5 between celestial foundation and Phase 4; unchanged precision/domain principles |
| `docs/architecture.md` — modify | Derived guide/selection/navigation and clock ownership/API boundaries |
| `docs/engine-invariants.md` — modify | Explicit guide/history/quality separation, preserve fixed-baseline statement |
| `docs/roadmap.md` — modify | Insert Phase 3.5, later orbital-performance direction, Phase 4 terrain still distinct |
| `docs/performance.md` — modify | Actual distributions for Section 23 workloads, baseline comparisons and limitations |
| `docs/phase-3-5-validation.md` — new | Actual numerical/headless/native/CI evidence and open platform criteria |
| `docs/adr/0005-celestial-navigation-system-view-and-timewarp.md` — new | Accepted implementation decisions/alternatives, exact-only warp scope, history presentation, gap policy, measurements and revisit triggers |

Existing Phase 3 specification/validation factual clarifications in this audit stay linked; do not rewrite earlier evidence as proof of a better explorer. All new/modified documentation gets UTF-8/LF/newline/fence/link/diff review. No new dependency or Cargo.lock change is planned merely for API sketches.

## 22. Automated verification matrix

Headless tests exercise independent answers and public semantic contracts; no GPU pixel test is required. Shader/layout tests validate actual data contracts. Preserve all Phase 1–3 numerical tolerances and existing long-run release tests.

| Coverage | Objective assertions |
| --- | --- |
| Arbitrary identity selection | Each body including index 0, planet and moon: list/label/marker/sphere converge on the same BodyId; same names don't alias, wrong namespace rejected; rebuild changes no selection |
| Marker/label/sphere hits | Physical-pixel boundary/tie/depth/overlap candidate reachability; displaced label rect maps to true BodyId; off-centre visible sphere/behind/offscreen/occluded behavior; markers off still allow list/sphere |
| Gesture routing | Click versus >4-point drag; select+focus ordering; input consumed by egui cannot issue camera commands; multiple events do not lose selection through overwrite |
| Body focus | Every body/role, first/revisited focus, interrupted transition, next/previous/unfocus; exact initial/final pose, no target-centre crossing |
| Transition continuity | Paused and moving-frame fixtures: same pose at cancel/mode boundary; shortest quaternion path, finite monotone log progress; 30/60/144 Hz equivalent total admitted wall duration matches endpoint |
| Precision | Local source-centred ≤1e-9 m, body-local/rotated handoff ≤1e-7 m, astronomical independent root handoff ≤1e-3 m at 1.5e11 m; orientation basis ≤1e-12; defined kinematic migrations retain applicable Phase 1 velocity budgets |
| Framing | Every included sphere and guide extremum inside padded viewport at wide/tall aspects; uncapped all-history fits; cap reports omitted extent; outlier never silently excluded; subsystem/empty/single body; resize/DPI margins |
| Zoom | Known scroll factor/round trip away from clamp; at AU many decades reachable, at R+1 m small clearance increments, no centre crossing; real R copied unchanged, overflow/invalid input rejects |
| Overview→body→overview | Paused return restores saved orientation/scope with all meaningful bounds fitted; authority-advanced return refits current positions rather than initial COM |
| Free flight | Normalized diagonal motion, distance/manual speed scales, no input release inertia; root/local carrier migrations preserve pose and system-stationary behavior as body moves; no inherited foreign spin |
| Labels | Selected first, no accepted-rectangle overlap/protected-area intrusion, leader links/candidate fallback, deterministic priorities/hiding/hysteresis and DPI; all hidden labels' bodies remain selectable |
| Marker fade | Smooth endpoints and intermediate opacity at 8/24 px; selection retained; fallback keeps marker visible; opacity affects neither physical radius nor sphere scale |
| Elliptical guides | Independently authored circular/e=0.3/inclined pair reconstruct a/e/p/plane and known peri/apo radii; reference at focus, current r on conic, finite exact extrema; circular peri direction explicitly undefined |
| Conic edge cases | Known escape/hyperbolic and near-parabolic states classify without ellipse; radial/zero separation/overflow/ill-conditioning produce unavailable diagnostic rather than fake curve |
| Guide reference | Automatic moon chooses planet under differential tides, star root unreferenced; equal-mass/ambiguous cases unavailable unless explicit; changes stable/hysteretic, no force hierarchy |
| Guide/history separation | Empty paused epoch has guides but zero history segments; guides never change sample counts; committed states only refill history; relative old body/reference same tick; seek/reverse invalidation retained |
| Read-only visualization | Snapshot body IDs/properties/state/time/revision before/after every fit/selection/focus/layout/guide/trail/fade preparation; exact unchanged copies; no mutable world reference required |
| Rate measurement | Synthetic real commit deltas/windows reproduce signed rate; no commits is zero/tick-limited, never requested-rate copying; pause/rate/rebranch reset; seek/replay/reset/single jumps excluded |
| Debt/overload | Requested/achieved/method/h/debt displayed from reports; over-cap interval atomic rejection, no hidden dt change; limits distinguish count/budget, resumed retained debt vs cancelled pause |
| Pump chunks | Equivalent tick targets/state/history bits/callback cadence for pump512 versus 1/8/32 variable chunk schedules, including replay init/final publication/restore; ≤512 total per UI opportunity |
| Lifecycle | Synthetic hidden durations, 10-hour gap, 3-second stall, exact 250 ms boundary, multiple repeated events, restore zero elapsed; no debt/sleep movement and no double-admission after surface retry |
| Quality transitions | **No new propagation mode implemented**: selecting high/custom rate retains baseline KDK/h. Later modes require full-time state continuity/refusal/branch/replay-provenance tests before enablement |
| Styled-line rendering math | Clipped near/frustum/astronomical lines stay finite within ≤0.05 px position budget; width/dash/alpha/stride offsets, zero-length/failed batch poisoning; matching WGSL validates |

Tests of error classification/layout tolerances must reference actual policy constants and independently constructed geometry, not merely mirror internal helper calls. No screenshot suite, physical spacecraft velocity tests or terrain acceptance hidden in navigation tests.

## 23. Performance plan and targets

Criterion remains stable/CPU-only/outside normal CI. Headless fixtures use explicit deterministic state, realistic noncoincident orbital conditions, nonconstant inputs and complete observed outputs. Preallocate warm storage; separate cold setup/allocation from steady work. Record CPU/OS/compiler/profile/glam/lockfile, N/h/pairs/sample/segment counts, viewport/DPI, medians/distributions/CI, allocation evidence and numerical envelopes. Do not claim allocation profiler evidence from inspection alone.

| Benchmark | Workloads / meaningful separation |
| --- | --- |
| Selection/hit preparation | 3/16/64/256 bodies, spread and coincident-marker layouts, physical large sphere vs subpixel; projection/preparation separate from repeated picks |
| Label layout | 3/16/64/256, realistic measured text sizes, selected/hover priorities, dense clusters; measurement/cache changes separate from collision resolution; no egui window bootstrap timer |
| System framing | Whole/subsystem, physical bounds only versus 64/512 guide extrema and displayed 1024/8192 history; extreme outlier/tall viewport; no spatial index for N3 |
| Orbit guides | 3/16/64 normal and 256 probe; reference scoring/differential eta, elements, 64/512 tessellation and projected refinement independently; paused reuse versus changing revision |
| History | 3/16×1024/8192 retained, inertial/relative, selected width/fade, bounded vertex display and full visible worst case; record sampling/request/clip/narrow/pack/upload-size separately |
| Camera transitions | Prepare ordinary same-body, planet→moon and AU→near-radius transition; repeated frame re-expression/navigation samples independently; no benchmark of one enum getter |
| Physical spheres/markers | Existing 3/16/64 sphere and 256/1024 marker-heavy groups with apparent-size metadata; maintain source-centred precision checking; GPU frame timings native separately |
| Exact high-warp | 3-body h60 real spins and plausible 8/16/20-body systems with short moons; h10 circular; 512/full commits/history/trails/projection versus scheduler chunks; 1/100/1000/10000/100000/1000000x admitted-duration runs with actual debt |
| Stress budget | Retain N64/256/1024 force/kernel/512 groups as labelled scale probes; measure one-step overshoot and chunk responsiveness, not promise an interactive 1024-body product |
| New propagator | **None in Phase 3.5.** Later compare equal simulated horizon/position+phase tolerances, initialization/handoff/check cost and validity rejection, not unequal-h steps/s alone |

On the existing documented baseline host, investigate these implementation goals:

- Normal hierarchy at 100000x and 1000000x exact with h60: sustained requested advancement within fixed-tick quantization, no growing debt over 60 active wall seconds and responsive navigation. Record actual rate window/debt/presentation cadence in both debug and release; release CPU evidence alone is insufficient.
- Normal N3–20: combined navigation/labels/guides/sphere/**default bounded** trail CPU preparation ideally <4 ms at 1280×800, exact pump budget ~4 ms, leave renderer/UI headroom for a 60 Hz goal. Record failures instead of weakening precision. Full 8192 display is measured separately; old 3-body 2.39 ms line result is the comparator, not an impossible zero-cost expectation.
- Label cost at 256 may be O(N²); report distributions and counts. No hardware timing assertions in tests, no kd-tree/BVH/cache/job infrastructure unless real normal-count evidence calls for it.
- Throughput table reports `h * committed_forward_steps / wall_s`, authority advancement, debt/rejection, replay costs and method/accuracy context. Exact requested ceiling depends on CPU **and opportunities/count budget**. N1024 h0.001 probe's low playback estimate is not transferable to the h60 explorer.

## 24. Native validation workflow

Extend existing `cargo run --locked -p mundaris_app -- --gravity-orbits`; no disconnected scene or separate explorer executable. Run at 1280×800 and one high-DPI/tall viewport on each supported desktop, recording OS/GPU/backend/driver, build/profile/revision, all results and unresolved problems:

1. Open paused system overview: all three bodies identifiable, true sizes, legend and guides immediately visible.
2. Select planet by marker, displaced label, list and physical sphere when large; verify same ID/highlight. Exercise crowded planet/moon overlap picker.
3. Smooth focus planet, orbit, exponential-clearance zoom from astronomical separation to reference-radius near-surface clearance; verify debug-reference-sphere qualification and no centre crossing.
4. Select/focus moon, orbit; selected subsystem fits moon separation. Return to current Whole System without changing any body state.
5. Free-fly at metres/km/radius/moon/AU speed scales, release movement, change frames, refocus a body; no attachment/snap surprise.
6. Force label overlaps; selected label remains readable, displaced leader/hit remains accurate; hidden labels still selectable from list.
7. Toggle guides/trails/markers independently. Advance real simulation: actual trails appear/refill, solid age fade differs from dashed guides. Explicit reference-relative history and reference guide show their semantic labels.
8. Resume/run 1x/100x/1000x/10000x/100000x, then 1000000x or higher; record active full N-body KDK/h and measured window/authority/request/fraction/debt. At 1x wait a whole h60 tick or single-step separately; do not confuse no substep rendering with overload.
9. Deliberately exceed exact admission/throughput; status honestly shows lag/overload/rejected/cancelled demand. Lower-rate/Resume Admission versus Pause/Cancel Debt behave distinctly; no fidelity switch.
10. Retained reverse, older private replay/seek/cancel/reset/edit/rebuild remain coherent; no guide point enters history and playback metrics exclude seeks.
11. Minimize/restore, occlude, zero-size/suspend where available, long debugger pause and sleep/resume: no hidden giant request or camera leap; clock-gap status and Resume behavior verified.
12. Resize/surface recovery/normal close; old smoke, `--reference-frames`, `--celestial-model` modes still present and their existing mathematical/interactive checks work.

High-speed approximate simulation is design-only: validation demonstrates excellent exact UX plus honest limits and documents the future explicit regime boundary. An operator sequence report is visual evidence, not a GPU timing/physics-error oracle.

## 25. Objective Definition of Done

Checked implementation/automated items below are supported by the subsequent
[validation record](docs/phase-3-5-validation.md). Combined operator/platform and
complete profiling criteria remain open; the original design alone completed none.

### User experience and semantics

- [ ] Clean FOV/aspect/content-aware current system fit, visible star/planet/moon identification and immediate orbit guides; outliers/fit-history limitations explicit.
- [ ] Arbitrary BodyId selection through sphere/marker/label/list, overlap reachability and highlights; selection distinct from focus.
- [ ] Smooth interruptible body focus/overview/next/previous/unfocus; no astronomical f32 transition, no authoritative mutation or universe movement.
- [ ] System Orbit, Body Orbit and useful scale-aware Free Flight share Phase 1 observer/reference-frame architecture, work paused and preserve documented frame continuity.
- [ ] Multiplicative clearance zoom spans AU to near reference radius with no centre crossing/near-surface huge steps; physical radius never enlarged.
- [ ] Subpixel bodies remain visible/selectable via derived markers; smooth marker/sphere transition, selected ring, readable labels/leader lines/collision handling and observer distances.
- [ ] Historical trails remain actual synchronized committed samples, age-faded/highlighted and camera-relative; presentation switches do not fabricate/reset physical history unnecessarily.
- [ ] Dashed orbit guides remain unmistakably separate from solid/fading history; conic math/ref selection/classification honest, perturbations update guides; invalid/unbound states never masquerade as ellipses.
- [ ] Flat reference-aware list/subsystem overview does not add authoritative orbit parents, spin ancestry or force switching.

### Time and ownership

- [ ] Requested/achieved rate, window/quantization status, integrator/propagator label, h, exact-baseline fidelity, authority/requested time and simulation debt are visible and derived honestly.
- [ ] Exact overload/count/CPU-budget behavior is understandable, previous admitted debt/rejection/cancellation distinct; baseline KDK/h unchanged for every rate.
- [ ] Bounded chunk work preserves deterministic tick results/history/callbacks and avoids an unconditional 512-step blocking pump; indivisible-step overshoot reported.
- [ ] Minimize/occlusion/suspend/sleep/debugger/long stall cannot accidentally admit enormous catch-up or navigation movement; shared host timing tested/native-validated.
- [x] No approximate authoritative or preview propagator implemented; any subsequently approved addition requires explicit mode/method/error/handoff evidence and a revised scope/ADR before enablement.
- [x] World/frames/renderer/selection/guide/history ownership and one-instant read-only contracts hold, BodyId distinct from FrameId, camera-relative conversion precedes narrowing, no global floating-origin mutation.
- [x] Clean focused observer/body interface available for Phase 4 without terrain/LOD commitments.

### Quality, measurements and platforms

- [x] Stable Rust/Rust 2024, six `publish=false` crates, no project unsafe/cycles/unrelated dependency upgrades.
- [x] `cargo build --locked --workspace` and `cargo check --locked --workspace --all-targets --all-features` pass.
- [x] `cargo fmt --all -- --check` passes.
- [x] `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` passes.
- [x] `cargo test --locked --workspace --all-features` passes with meaningful headless navigation/conic/lifecycle/reporting and old lifetime tests.
- [x] Focused optimized workspace-domain/app/renderer tests and unchanged ignored long-run orbital release tests pass; actual numerical maxima recorded.
- [x] `cargo doc --locked --workspace --all-features --no-deps` passes with `RUSTDOCFLAGS=-D warnings` set in native shell.
- [x] Headless WGSL/layout/precision tests pass; no unnecessary GPU pixel suite introduced.
- [ ] Section 23 benchmark targets executed with hardware/compiler/profile/error context and comparison evidence; no ordinary-CI benchmark execution.
- [ ] Windows native build/check/tests/release and full Section 24 interactive/lifecycle validation recorded with backend evidence.
- [ ] Linux native build/check/tests/release and full Section 24 interactive/lifecycle validation recorded with backend evidence; Windows/WSL availability is not substituted for unperformed tests.
- [ ] Current implementation-revision remote Linux-quality and Windows-compatibility CI pass via normal repository workflow. Missing/unavailable evidence remains open.
- [ ] All Phase 1–3 validation modes and numerical envelopes still work; deferred prerequisite platform/visual criteria resolved or explicitly reviewed without marking missing evidence complete.
- [ ] README/architecture/roadmap/performance/new validation record and ADR 0005 describe actual implementation, consequences/revisit triggers and outstanding gates accurately.
- [ ] `git diff --check`, UTF-8/LF/final-newline, Markdown links/fences and source-only intended changes reviewed; correctness/architecture/performance review completed.

## 26. Exclusions and implementation recommendations

No planetary terrain, cube-sphere terrain patches, planetary LOD, procedural mountains, terrain editing, biomes, vegetation, oceans, atmosphere, clouds, planetary surface textures, buildings, spacecraft physics, gameplay, collision, relativity, tides, geological simulation, save-game format or galaxy generation. This design does not specify those systems in depth. Phase 4 begins planetary representation; reference-sphere clearance is navigation only.

Also defer authoritative analytic/extreme warp, adaptive/multi-rate/mapping implementations, preview/timeline editing, cross-method retained replay, permanent orbit ownership, N-body approximation/parallel/GPU gravity, general UI layout/camera frameworks, jobs and spatial indices.

Before implementation:

1. Review this exact-only scope, gap threshold/resume UX, frame/carrier semantics and guide/reference thresholds; accept ADR 0005 decisions with the actual implementation, not speculative performance claims.
2. Resolve or explicitly review outstanding Phase 1–3 Linux/current-CI and Phase 2 full Windows visual gates. The existing ADR 0004 sequencing deferral is not blanket acceptance for every future phase.
3. Establish headless camera/fit/selection/math contracts first; add element queries and independent conic tests; then readable UI/polyline rendering; finally exact metrics/chunk timing/native lifecycle. Keep all current modes runnable throughout.
4. Measure normal small-system exact throughput/preparation before reconsidering fidelity. When extreme warp is an actual requirement, run a separate ladder-versus-near-Kepler mapping/preview experiment with equal-horizon error metrics and nested-moon workloads.
5. Keep read-only BodyId/observer interface clean for Phase 4. No orbital hierarchy or rendering convenience may become authoritative celestial state.

## 27. Documentation audit corrections

The companion corrections to existing Phase 3 documents record current behavior rather than improve code:

- Current overview/focus/markers/trails exist, but overview is an initial COM fit, arbitrary focus is an instant explicit action, and label/sphere area clicks are not implemented.
- Relative trail checkbox refers to fixed `ids[1]`, not arbitrary selected body.
- Detected hidden-time exclusion already exists in gravity mode; unreported long drawable gaps remain a real hole.
- UI lacks 10000x preset but rate API supports it. Requested/achieved reporting exists, with finite-window quantization/control-jump caveats.
- Earlier Windows operator-reported passing sequence remains preserved evidence; it does not establish the proposed smooth transitions/clean layout/system fit or sleep protection.

The design-only change is documentation. No Phase 3.5 source, new propagator, benchmark implementation, commit or push is part of this audit.

## 28. Implementation record — 2026-10-02

The earlier audit/design narrative remains historical. The subsequent explicit
implementation request authorized the chosen exact-only phase; it did not waive
platform acceptance or authorize Phase 4. Concrete APIs, independent headless
contracts, performance distributions, native observations and remaining criteria
are recorded in [validation](docs/phase-3-5-validation.md),
[performance](docs/performance.md#phase-35-baseline--2026-10-02) and
[ADR 0005](docs/adr/0005-celestial-navigation-system-view-and-timewarp.md).

The Section 21 surface gained only shared app benchmark fixtures in
`crates/app/benches/common/mod.rs` and a focused empty/single/outlier integration
test in `crates/app/tests/celestial_bounds.rs`. These reduce fixture duplication
and keep bounds acceptance focused; no runtime crate, dependency, physics kernel,
math/world ownership change or generalized framework was needed. Rust API sketches
remain semantic: projection origin is a checked builder, generic styled curves
pack clip-relative quads after f64 checks, and app status composes existing runner
reports with rolling metrics. Interrupted focus uses pose-preserving Free Flight.

Objective acceptance has evidence for implemented behavior and Windows quality;
the combined Section 25 boxes are not all marked complete because full human,
real OS sleep/high-DPI, Linux, remote CI, allocation-profiler and presentation-cadence
evidence remains incomplete. Measured 16-body full-retention preparation misses
the 4 ms goal. Numerical tolerances are retained. No approximate authoritative
propagator, preview timeline or Phase 4 feature has been implemented or enabled.
