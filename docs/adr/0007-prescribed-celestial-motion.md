# ADR 0007: Prescribed analytic celestial motion

- **Status:** accepted for Phase 5.13A implementation

## Decision

World owns immutable definitions of system-local authored motion. Simulation owns
the concrete bounded producer that evaluates those definitions and transactionally
commits complete body states. The producer is a prescribed-motion source, not a
gravity backend, integrator, replay facility, or app/rendering feature.

Each system body has one complete definition: fixed system-local translation or
an ellipse relative to a body in that same system, plus independent axial spin.
An ellipse carries its reference, positive semi-major axis, eccentricity in `[0,1)`,
plane orientation, positive explicit period, mean anomaly at epoch, and epoch. Its
plane uses +X periapsis; the reference contributes position and velocity, never
rotation. Spin carries its own epoch orientation, body-local axis, signed rate and
epoch. Setup rejects incomplete, duplicate, invalid/foreign and cyclic definitions
and establishes reference-before-child evaluation independent of insertion order.

Sampling is direct, deterministic and history-independent. Reusable scratch holds
all candidate states; all candidates must validate before the one complete
transactional commit. Failure preserves world state, time and revision. A successful
external celestial revision change invalidates the producer binding; rebinding
constructs a new producer. Its own commits update the binding; terrain-only edits
do not invalidate it. Separate systems remain independent.

The analytic domain is deliberately bounded: absolute requested time, epochs and
elapsed intervals are at most `2^36` seconds; elapsed phases are limited to `2^32`
cycles. Time is represented in floating-point seconds, not arbitrary calendar-time
or phase precision. Mean anomaly at epoch has absolute value at most `TAU`; eccentricity
is elliptic only. A safeguarded Kepler solver defaults to 64 iterations and permits
limits 1–128. These limits bound work and do not assert general-purpose propagation
fidelity.

## Rationale and consequences

Explicit authored periods make the prescribed model independent of mass and any
assumed gravitational parameter. Independent spin prevents a reference hierarchy
from silently becoming a physical rotation hierarchy. Complete direct sampling
supports seeking and deterministic replay of a requested instant without storing
synthetic history. Transactional publication preserves the existing coherent-world
contract, while revision-based stale bindings avoid hidden synchronization or
implicit rebinding.

This decision does not alter existing world mutation semantics, gravity/KDK, runner history/replay,
rendering, app launch, sky, terrain, camera, streaming, dependencies, or crate
boundaries. Visible-universe behavior and cross-system motion are separate work.
Existing shared-LCA precision behavior remains required for attached local detail.

## Acceptance boundary

Phase acceptance requires independent oracles within normalized position error
`1e-11` and velocity error `1e-10`; signed spin and hierarchy checks; replay of `T`
after intervening samples; epoch offsets 0 and +/-1 and 1,000 years; transactional failure,
stale binding and multi-system checks; and millimetre attached offsets through shared
LCA with a huge common translation. Preserve world/runner/frame regressions. Run
focused debug/release tests and 10/100/1,000-body near/distant benchmarks with
build/hardware/statistical context, fast `ai-check`, and full quality validation.
These are required evidence, not claims of completed implementation or measured
performance. See the [Phase 5.13A specification](../../MUNDARIS_PHASE_5_13A_ANALYTIC_CELESTIAL_MOTION.md).
