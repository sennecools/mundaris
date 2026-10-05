# Phase 5.13A — Analytic celestial motion

## Objective and boundary

Add a bounded producer for **authored, prescribed** celestial motion. World owns
the immutable motion definitions; simulation owns the concrete producer that
samples those definitions and commits complete world states. This is neither an
integrator nor a replacement for gravity, playback/replay, or the app's composition
and presentation responsibilities.

This phase does not change ordinary application launch, change rendering/sky behavior, add
streaming, refactor N-body simulation, or claim a visible-universe implementation.
Cross-system motion remains a separate roadmap item. Camera and terrain acceptance
criteria remain unchanged.

## Authored model

Each body in a system has exactly one complete motion definition:

- Translation is either a fixed system-local position or an ellipse relative to a
  named body in the same system.
- An ellipse specifies a positive semi-major axis, eccentricity in `[0, 1)`, a
  plane-to-system orientation, an explicit positive period, mean anomaly at epoch,
  and epoch. The orbital plane uses +X as periapsis. Mean anomaly at epoch is
  finite and has absolute value at most `TAU` radians.
- Spin is specified independently by epoch orientation, a body-local axis, signed
  angular rate, and epoch. Negative rates allow retrograde spin. An orbit reference
  supplies translation and velocity only; it never supplies axial rotation.

Definitions are immutable after construction. Setup validates that every system
body appears exactly once, all IDs belong to that system, and the reference graph
is acyclic. Definition/body insertion order does not determine evaluation order.
The model uses the existing system-local coordinates and checked math types; it
does not imply astronomical identities or a globally meaningful epoch.

## Sampling contract

Sampling is direct and history-independent: the same definition and requested
instant produce the same complete states regardless of prior sample calls. Every
body is evaluated in reference-before-child order. Position and velocity are
relative orbital state plus the reference body's position and velocity; attached
features remain frame-relative and do not need individual updates. Orientation
and angular velocity come from the body's independent local-axis spin.

The producer reuses candidate/scratch storage. It validates the binding, computes
and validates every candidate, then performs one complete transactional world
state update at the requested instant. Any error, including solver exhaustion or
unrepresentable arithmetic, leaves authoritative state, instant, and revision
unchanged. An external celestial revision change makes the producer binding stale;
callers must construct a new producer to bind again. The producer tracks its own
successful commits; terrain-only revisions do not invalidate it. Independent systems have
independent definitions/producers and cannot reference one another.

This contract is bounded numerical authoring, not a fidelity promise for arbitrary
calendars or an alternative N-body solution. The supported envelope bounds the
absolute requested instant, each epoch, and each elapsed interval to `2^36` seconds
(about 2178 years), and limits elapsed orbital/spin phase to `2^32` cycles. Seconds
are represented as `f64`; arbitrary calendar or phase precision is not promised.
Elliptic motion uses a safeguarded solver with default maximum 64 iterations and
configurable limits from 1 through 128. Solver limits bound work; no solver fidelity
claim beyond the acceptance criteria below is implied.

The numerical envelope is not a universal error bound: subtraction of represented
working-epoch seconds, parent/child addition and extreme eccentricity conditioning
remain f64 operations. Nearly parabolic inputs may exhaust the iteration limit;
overflow, vanished orbital scales/velocity intermediates and unsupported time/cycles return errors rather
than clamping eccentricity or publishing invalid states. Attached local detail uses
shared frame ancestry; it cannot recover detail already lost in independently
flattened body centers. Numerical tolerances apply to the identified oracle fixtures.

## Required invariants and exclusions

- No ambient clock, history integration, retained prior sample dependency, or
  partial publication.
- No accidental inheritance of the reference body's spin.
- No changes to gravity/KDK semantics, runner scheduling/replay, existing world mutation semantics,
  renderer, app startup, terrain, camera, sky, streaming, dependencies, or the six
  crate boundaries.
- Preserve shared-ancestry/LCA cancellation in precision-sensitive frame
  conversion; do not flatten huge system translations before subtracting common
  ancestry.
- Keep motion definitions in world and the concrete sampling producer in
  simulation, following existing ownership and error-handling conventions.

## Acceptance and verification evidence

Focused tests must establish:

1. Independent numerical-oracle position error at most `1e-11` and velocity error
   at most `1e-10` in normalized units, across representative eccentricities,
   orientations, periods and phases.
2. Independent signed spin and hierarchy behavior, including that child motion
   does not inherit reference spin.
3. History independence, including a `T` sample, intervening samples, then replay
   of `T`; and epoch-offset checks at zero and plus/minus 1 and 1,000 years within the
   supported envelope.
4. Invalid/incomplete/duplicate/foreign/cyclic definitions, stale bindings,
   solver exhaustion and arithmetic/time failures. Failed samples preserve the
   exact pre-call world state, instant and revision.
5. Multiple independent systems, and millimetre-scale attached offsets converted
   through shared LCA ancestry despite a huge common translation.
6. Existing world, simulation runner/replay, frame precision and relevant app
   regressions remain preserved.

Run focused tests in debug and release. Benchmark 10, 100 and 1,000-body systems,
including near-reference and distant cases; retain build profile, hardware,
workload and statistics (including solver iterations). Benchmarks are evidence,
not an acceptance substitute or a performance claim without results. Run the fast
`ai-check` and the repository's full quality set; report commands and outcomes
separately. Provide a review handoff with changed files, reproducible evidence,
limitations, and unresolved decisions. Do not claim completion or measurements
without corresponding evidence.

## Universe boundary and next phases

Universe location → system-local state → body-local content → observer-relative
rendering is the intended boundary. Evaluation never requires galactic placement;
each namespaced system samples near its own origin. This phase does not claim that
the per-system frame projection implements a navigable universe. Sector/address
formats will be decided against real cross-system requirements, not invented here.

Next: integrate the producer with existing default composition, time controls,
guides and navigation; then a visible-universe phase with spatially defined distant
stars/readable galactic background and deterministic captures (decorative content
clearly distinguished from addressable systems); then universe addressing and
bounded loading/streaming for exploration. Black sky is a missing distant-content
layer, not evidence of a coordinate-size limit. These are roadmap items, not scope
authorized by this phase. Terrain responsiveness and camera acceptance remain open.
