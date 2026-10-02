# ADR 0005: Celestial navigation, system view and exact time warp

- **Status:** accepted implementation; platform and complete operator acceptance remain open
- **Date:** 2026-10-02

## Decision

Extend `--gravity-orbits` in the existing six-crate workspace. World remains the
authoritative owner of all physical properties, full-time states, time and revision.
Selection uses a validated optional `BodyId`. Renderer request indices and frame
handles are disposable mappings. No force, integration, frame parenting or physical
radius changes are made for navigation or visibility.

App owns geometric bounds, overview membership, labels, selection, one observer,
camera transitions, guide reference policy, recorded history and host timing.
Whole System includes every body, including outliers. Selected Subsystem includes
one generation of accepted automatic guide references. Explicit membership is a
separate action. Bounds include physical extents, analytic elliptical extrema and
the displayed history subset. History contribution is capped at twice the core
radius unless the user explicitly fits all displayed history. Screen padding and
the renderer's physical content rectangle determine FOV/aspect fitting.

System Orbit, Body Orbit and Free Flight control the same checked Phase 1 pose.
Body orbit normally uses nonrotating translating frames. Body-fixed spin remains
an explicit debug attachment. Focus transitions resolve semantic body/frame roles
at the current committed instant, use eased f64 pull-back/transit/log-clearance
approach and quaternion interpolation, and reject reference-sphere intersections.
Retargeting starts from the displayed observer. Input interrupting a transition
enters pose-preserving Free Flight, allowing a subsequent explicit refocus.

Pull-back uses the old body's outward radial direction, so free-flight look cannot
send the observer through its old centre. Transit uses an external arc. Debug
re-expression changes translating/fixed role only for the same completed focus;
generic arbitrary pose/kinematic conversion remains available in math. A coordinate
change cannot silently replace the navigation pivot. Radius-envelope adjustments
move only the observer and report the reason.

Clearance zoom uses real radius plus multiplicative clearance, with minimum
`max(1 m, 64 ulp(R))`, an 80 ms wall response and a disclosed 1e15 m navigation
limit. Free flight uses normalized camera-axis movement, wall duration, logarithmic
scale blending, a manual multiplier and translating numerical carriers with
32R/64R hysteresis. Carrier translation is compensated; ordinary free flight does
not track the previous focus or inherit foreign spin. Wall movement speed is not
passed off as a simulation-time physical velocity.

Simulation owns a pure checked two-body conic query, not a new propagator. App
automatic references require greater mass, dominant attraction, a resolved bound
ellipse and differential external perturbation at current/eight diagnostic points
no greater than 0.05. Valid references have 25% challenger hysteresis. Equal-mass
and ambiguous systems require an explicit pair. These edges never become world
ownership, frame ancestry or force switches. Hyperbolic, near-parabolic, radial and
invalid states never generate a closed ellipse.

Renderer owns source-centred f64 preparation, clipping, checked narrowing and
portable pixel-width segment quads. Generic solid/dashed requests contain no body
identity or history semantics. Dashes are prepared in screen space, with alpha
blending and reverse-Z depth testing/no writes. Markers and labels remain overlays.
Guide tessellation starts at 64 segments, refines against 0.5 physical pixels, and
reports a cap miss at 512/1024. Trails simplify existing committed vertices only,
retain endpoints/extrema, and report cap misses at 1024/2048 vertices.

**History presentation changes no longer clear synchronized absolute records.**
An explicit reference picker subtracts body and reference at the same historical
sample and displays that relative path at the reference now. Branch, direction,
seek and physical-edit invalidation are retained. Guides are never recorded as past
motion. This supersedes ADR 0004's presentation-mode clearing policy only.

## Exact scheduling and host clock

All playback rates use unchanged full N-body KDK and configured h10/h60. The runner
adds validated `pump_with_work_limit`; existing `pump` delegates to the original
configured cap. App starts with one work unit, estimates cost, then runs chunks of
at most 32, at most 512 total, checking a 4 ms opportunity budget between chunks.
Restore and private replay work are included. One expensive step is indivisible;
the N1024 probe exceeds the budget even at one unit. There are no threads, hidden
large steps, analytic authority, adaptive stepping or preview timelines.

Rolling achieved playback measures real live commit deltas over accounted active
wall time. Rate/pause/branch/lifecycle changes reset it. Seek/reset/single and private
replay jumps are excluded. Quantized h60/1x samples also expose segment averages and
next-step wait. Requested rate, authority, target, debt, latest work, overload,
rejection/cancellation and opportunity-count/CPU limitations have separate meaning.

Hidden windows suspend/cancel work and refresh capture, preserving user pause/rate
preference. An otherwise drawable elapsed interval above the default 250 ms
threshold is rejected entirely, pending demand is cancelled, camera progression is
cancelled, and playback requires Resume. The diagnostic reports excluded **wall**
time separately from rejected simulation demand. The session threshold is explicit
and configurable. The same host-clock helper is used by the analytic modes.

## Alternatives, API choices and evidence

COM-based initial framing, enlarged spheres, overlapping painter labels, linear
near-surface distance control and an unconditional blocking pump were rejected.
Approximate authority and the future integrator research directions in the
specification remain deferred. A flat reference-aware list is sufficient; no
authoritative-looking tree or generalized UI/navigation framework is introduced.

`CelestialProjection::with_origin` preserves existing full-window callers. The
same rectangle drives projection, picking and GPU viewport/scissor. Styled curves
use renderer-owned packed clip-relative vertices after checked f64 preparation;
they need no additional GPU projection multiplication. `UiInfo` composes existing
runner reports and `PlaybackMetrics` rather than introducing a second public
simulation-status model. Benchmark fixtures share `app/benches/common/mod.rs`;
empty/single/outlier checks have a focused `celestial_bounds` integration target.
These small file-plan additions avoid fixture duplication and broaden no runtime
framework or dependency. Cargo.lock and numerical kernels are unchanged.

[Validation](../phase-3-5-validation.md) records current headless/numerical/native
evidence and outstanding acceptance. [Performance](../performance.md#phase-35-baseline--2026-10-02)
records workloads, distributions and limits. Native h60 hierarchy captures after
60 active seconds at 1,000,000x show approximately 999,999.6x debug and 999,998.8x
release, with zero pending ticks. This is captured viewer reporting, not GPU timing
or a universal machine guarantee. N16 full-retention display preparation misses the
4 ms goal; accuracy checks and provenance are retained.

Revisit reference scoring when real normal systems make its straightforward
candidate/tidal evaluation material; revisit trail preparation when full retained
history becomes the normal interactive workload. Faster propagators require a new
accuracy/handoff/replay decision. Native Linux, remote CI, real OS sleep/high-DPI
and complete human operator acceptance remain distinct from Windows numerical and
directed-event evidence. Phase 4 has not begun.
