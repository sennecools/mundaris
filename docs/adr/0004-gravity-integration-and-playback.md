# ADR 0004: Gravity, integration and playback

Status: accepted for the Phase 3 implementation; Linux/current remote-CI acceptance remains open.

## Prerequisite gate review — 2026-10-01

The operator explicitly approved proceeding with Phase 3 while retaining the
unverified Phase 2 Windows visual sequence and Phase 1/2 Linux native, interactive
and current-revision remote-CI criteria. This is a reviewed sequencing deferral,
not acceptance of missing evidence. Existing Phase 2 work was preserved in a
separate baseline commit after Windows formatting, workspace tests and Clippy.

## Gravity decision

Use CODATA 2018 G=6.67430e-11, unsoftened inertial SI f64 point masses,
lexicographic dense i<j pairs and ordinary serial component accumulation.
Both members react. Robust hypot distance and checked arithmetic reject
coincidence/unrepresentable forces; physical radius is absent from the kernel.
The chi<=0.02 session envelope uses square-root-scaled pair magnitudes without
forming r cubed or a potentially overflowing sum of masses.

O(N²) forces and O(N) scratch are appropriate for 3–16 bodies. Approximation,
parallel reductions and GPU gravity require a separate measured scope review.

## Integration and publication

Select concrete full-time KDK leapfrog (velocity Verlet for position-only gravity):
half kick, drift all bodies, one fresh force pass, final half kick. Reuse old/next
acceleration and separate checked candidates. Only successful world commits promote
scratch/cache/tick/history. System-axis constant spin is left-multiplied checked
axis-angle transport, with no torque/inertia or rotational-energy model.

Euler was rejected for secular orbital heating; symplectic Euler remains first
order and asymmetric; RK4's four force passes and non-symplectic long-run behavior
are unnecessary for these resolved orbits. No integrator trait/plugin hierarchy.

World remains authoritative. Projection publishes once after pump, then
`coherent_view` checks namespace/topology/revision/instant and borrows both sides
through rendering. Projection failure retains good physics, pauses and suppresses
mixed drawing. Rebuild remaps observer role through BodyId, preserving identity.

## Time, overload and lifecycle

Use one positive h per branch, integer u64 ticks up to 2^53−1, reconstructed times
and explicit ULP/adjacency guards. The selected fixtures use 10/60 s. Playback
changes admitted counts, never h. Retain cumulative exact Duration per signed-rate
segment; floor forward demand and ceil reverse demand. Quantized seek ties lower.

The 512-work-unit cap includes integration, restoration, replay initialization and
final replay publication. At more than 65,536 ticks of candidate debt, reject the
entire new interval and latch overload; previous debt still drains. Explicit
admission resume or an explicit lower rate preserves debt/fraction, unlike paused
Resume's authority anchor. Headless requests also respect the latched admission halt.
Pause/0x cancel demand; same-sign changes retain it; direction changes cancel and
start a fresh segment. Reports retain the latest signed rejected interval and latest
nonzero cancelled interval, not potentially overflowing lifetime totals. Reset and
physical edits report actual pending cancellation, not already achieved elapsed time.
Non-drawable/suspended intervals produce no work/demand, clear debt explicitly and
refresh the app timestamp. Retry of a surface acquisition cannot repeat a commit.

## Reverse and seek

One immutable O(N) branch baseline plus a preallocated ring of up to 2048 whole
snapshots, sharing one 16 MiB actual payload ceiling with private replay retention.
Account for BodyState padding, ticks and ring metadata. Each ring reserves half
the byte budget at setup, so cancel keeps live world and retained history intact
without a temporary over-cap allocation. The UI reports both rings; their sum is
at most 16 MiB, with separate O(N) baseline/scratch. Normal 3–16-body live history
still retains 2048 ticks; larger systems reduce whole-snapshot capacity. There is no
persistent timeline/checkpoint database or per-step allocation.

Retained reverse restores full stored state exactly and recomputes acceleration in
fixed order, discarding later snapshots. Otherwise reset private scratch to baseline
and positively replay under the work cap, holding the last live coherent view and
freezing additional wall admission. Completion commits only the full target and
swaps prepared history/cache; cancel/error leaves the live world. Runtime revisions
stay monotonic. Same-target/build replay reproduces physical components, not old
revision values, cross-platform bits or pixels. Signed negative-dt integration is
not user reverse and no exact mathematical reversibility claim is made.

Physical edits preflight proposed force/diagnostic arithmetic, generation and
capacities before the world mutation, then promote a paused new branch. Name-only
edits retain trajectories/history/trails. Reset restores the edited branch baseline;
original fixture loading deliberately replaces the scenario and identity namespace.

## Rendering and history

Domain-free renderer requests use a reusable indexed level-3 icosphere, physical
f64 radius, source-centred Phase 1 conversion, view normals and debug shading.
Reverse-Z uses Depth32Float, clear 0, GreaterEqual; spheres write depth and history
lines only test it. Layouts are explicit 32-byte vertices/32-byte draw uniform and
64-byte projection; dynamic uniform slots are padded to 256-byte GPU alignment.
Narrowing checks actual radius-relative error and ≤0.05 physical pixels. Markers,
labels and selected axes/rings are derived aids, without authoritative exaggeration.

App TrailHistory stores synchronized committed centres/tick/time in bounded f64
arrays, at stride 8/64 and at most 8192 samples/8 MiB. Relative history subtracts
body/reference at each same historical tick. Reset/edit/seek/direction/mode changes
seed separate strips; repeated same-direction single steps retain sampling cadence.
Private replay is not shown as observed motion. Renderer clips f64 segments before
narrowing; failed frames cannot upload partial buffers. No prediction/conics.

## Concrete API choices

- A borrowed `CoherentCelestialView` makes the explicit world/projection gate
  reusable without linking renderer to world. Compile-fail coverage preserves both
  borrows, beyond ordinary paired revision checks.
- IntegrationWorkspace keeps full checked BodyState arrays for spin/state payload,
  with separate numeric position/acceleration/half-velocity/candidate staging, rather
  than splitting every semantic field into its own array. This avoids redundant
  ownership while preserving O(N) scratch and full transactional candidates.
- Runner owns transactional edit/preflight APIs instead of requiring app to promote
  a previously prepared branch manually. Clonable read-only dense world iteration
  permits shared compensated diagnostic logic without allocation/identity forks.
- `request_forward_to_tick` supplies exact headless forward demand without Duration
  rounding or seek replay. It uses the same work/admission/time guards.
- Pump failures box their large progress report only on error; earlier successful
  work remains reported and committed. No hot success-path allocation follows.
- An app library target in the existing package exposes validation camera/fixtures/
  trail state to headless tests and Criterion. It adds no workspace crate/framework.
- CelestialFrame uses PreparedView's f64 centring/direction helpers with its own
  representation/pixel budget. Existing near-debug `try_position` budgets remain.

## Evidence, consequences and revisit triggers

[Validation](../phase-3-validation.md) records debug/release analytical bounds,
1000-period circular/eccentric and two-outer-period hierarchy maxima, command/FPS/
rollback/coherence/history tests, WGSL/layout/precision, Windows Vulkan execution
and the operator-reported full visual sequence. No tolerance was relaxed.
[Performance](../performance.md#phase-3-baseline--2026-10-01) records separated
force/kernel/publication/history/projection/render/history workloads and actual
distributions. The hierarchy's 512-step update including history/trail sampling/
projection has median 0.110 ms on the baseline host. Fully visible three-body,
8192-sample line preparation is 2.39 ms and exceeds the 2 ms goal; retain this
measured result rather than weaken pixel checks. Allocation claims are inspection,
not allocator-profile evidence.

Revisit exact-loop optimization when normal sustained N≥256 consumes more than
25% of the agreed CPU budget at required throughput, or N≥1024 becomes normal.
The slow 1024 probe alone authorizes no approximation/parallelism. Longer checkpoints
require measured replay-latency workflow evidence. New integrators/forces/adaptive
steps require their own numerical decision. Instancing/interpolation/depth changes
need measured workloads and coherent source-centred guarantees. Terrain, LOD,
generation, atmosphere, physical lighting, collisions and persistent timelines
remain outside this decision.
