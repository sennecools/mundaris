# Phase 3 validation record — 2026-10-01

## Status and reviewed prerequisite deferral

Gravity/orbits/minimal celestial rendering are implemented. All locally executable
Windows build, headless, focused release, documentation and benchmark checks pass.
The operator reported the full Windows Phase 3 §19 visual sequence passed. **Linux
native build/headless/release/interactive and current-revision remote CI remain
open. Phase 4 has not begun.**

Requirements: [Phase 3 specification](../MUNDARIS_PHASE_3_GRAVITY_ORBITS_AND_CELESTIAL_RENDERING.md).
Decision/API review: [ADR 0004](adr/0004-gravity-integration-and-playback.md).
Measured workloads: [performance](performance.md#phase-3-baseline--2026-10-01).

The operator explicitly approved retaining the unverified Phase 1/2 Linux/CI and
Phase 2 Windows full visual criteria while proceeding. This reviewed sequencing
deferral does not mark those old criteria accepted. Existing uncommitted Phase 2
source/specifications were preserved in a separate approved, validated baseline
commit. Old smoke/reference-frames/celestial-model modes remain operational.

## Implementation and guarantees

- Gravity: G=6.67430e-11, unsoftened point masses, f64 SI, lexicographic symmetric
  unordered pairs, robust hypot distance, finite/representability/chi≤0.02 guards.
  Radius and display names never enter force; all bodies, including star, react.
- KDK: cached old acceleration, full half-kick/all-body drift/new force/final
  half-kick, checked full-time candidates, constant system-axis spin left transport.
  One warmed force pass; initial/regather force is separate. No adaptive catch-up.
- Runner: integer reconstructed ticks/ULP guards, exact cumulative Duration per
  rate segment, fractional demand, floor/ceil direction rules, explicit pause,
  single steps, overload/debt, origin boundary and lifecycle status. Latest rejected
  interval/cancellation are reported separately from admitted target/live authority.
- History: immutable branch baseline, actual-layout bounded full snapshots and
  separately reported preallocated private replay ring. Retained restoration is
  exact on this build; older seek/reverse uses positive replay under the same cap.
  Live state is unchanged during/cancelled/failed replay; revisions never rewind.
- Publication: one full world transaction per good step, then cache/history and
  callback. Projection publishes after pump and `coherent_view` validates exact
  world/projection namespace/topology/revision/time with borrowed lifetimes.
  Failure suppresses mixed celestial drawing and pauses; rebuilding is derived.
- Rendering: 642 unit vertices/1280 outward indexed triangles, chordal error
  ≤0.005 radius, physical scale, debug shading/unlit star, reverse-Z spheres then
  clipped historical lines and no-depth overlays. Layouts/byte order/WGSL validate
  headlessly. Precision uses f64 observer/source centring before GPU narrowing.
- Camera: selection differs from focus; orbit/zoom/fit/overview and deliberate
  translating/fixed attachment stay in one world. Re-expression preserves
  instantaneous pose/velocity; fresh projections remap body/role, not old FrameId.
- Trails: committed callbacks, synchronized inertial f64 centres/ticks/time, bounded
  cadence/whole eviction; labelled simultaneous relative mode, current endpoint
  separate, branch/seek/direction/mode invalidation, same-direction single-step
  retention. No analytic/predicted ellipse is rendered as actual history.
- Editing: name retains physical history; mass/radius/full-state changes preflight
  solver, diagnostic arithmetic, branch/capacity before world mutation. Good edits
  pause/rebaseline at the current instant and clear derived past/future. Reset
  restores that edited baseline; original fixture loading is explicit replacement.

## Numerical results

All comparisons first require finite results. Tolerances were not changed. Ordinary
tests sample every committed orbital tick. Debug and release pass the same normal
envelopes; long tests are named/ignored ordinarily and explicitly run in release.
The circular period from specified G/masses/R is **24319.53646202179 s**.

| Fixture / steps | Max relative energy drift | Max P/Qp drift | Max Lcom/Ql drift | Max COM residual m |
| --- | --- | --- | --- | --- |
| Circular h10, 100 periods / 243,195 | 1.1190043e-11 | 4.3136246e-14 | 8.4333399e-14 | 2.9348727e-8 |
| Circular boosted/offset, 100 periods | 2.6827176e-13 (total inertial energy scale) | 4.2338165e-14 | 6.6701835e-12 | 6.4871325e-5 |
| Eccentric e0.3 h10, 100 periods | 3.6288924e-6 | 2.4904543e-14 | 3.5407857e-14 | 2.1383846e-8 |
| Circular h10, 1000 periods / 2,431,953 | 1.1213312e-11 | 1.6505902e-13 | 1.3096737e-13 | 9.5498088e-7 |
| Eccentric e0.3 h10, 1000 periods / 2,431,953 | 3.6288931e-6 | 1.1342715e-13 | 2.9628381e-13 | 6.5660692e-7 |
| Hierarchy h60, two outer periods / 1,056,166 | 1.5336759e-13 | 1.0756820e-13 | 7.6754689e-14 | 6.4013909e-7 |

The boost is (100,−200,50) m/s and initial offset (1e9,−2e9,3e9) m; expected COM
is independently straight-line, with no ongoing recenter/boost correction.
Boosted total energy contains common kinetic energy; the unboosted circular oracle
independently constrains orbital energy. Eccentric energy remains bounded:
max_1000≤2·max_100+1e-9, with no endpoint-only or secular-drift substitution.

| Orbital/spin measurement | Result / required bound |
| --- | --- |
| Circular 100/1000-period radius drift | 3.3374782e-6 / ≤2e-5 |
| Boosted circular radius drift | 3.3374839e-6 / ≤2e-5 |
| First 20 measured return period errors | 2.2249811e-6·T (boosted 2.2249842e-6·T) / ≤1e-5·T |
| Circular 100-period accumulated phase | 0.0013986142 rad / ≤0.003 |
| Circular 1000-period accumulated phase | 0.0139805959 rad / ≤0.03 |
| 20-period h10/h5 common final instant 486390 s | 2.7959988e-4 / 6.9900110e-5 rad; endpoint ratio 0.2500004998, required [0.20,0.35] |
| Test-only analytic crossing interpolation arithmetic | max 1.1641532e-10 s; full physics steps are never shortened |
| Hierarchy outer period | 31685000.714886308 s |
| Hierarchy moon separation over two outer periods | [99,958,433.1291, 100,020,034.9111] m / allowed [0.9e8,1.1e8] |
| Constant spin, 100,000 h10 steps, omega=(1e-4,2e-4,−3e-4) rad/s | max squared-norm error 6.6613381e-16 / ≤1e-12; basis error 1.7638750e-13 / ≤1e-9 |

Independent axis/non-axis/three-body force and initial E/L formulas meet the
specified relative 2e-14/5e-14 force envelopes. Empty/single-body/equal-mass cases,
invalid raw slices, coincident/unrepresentable/subnormal/overflow/encounter rejection
pass. Radius×100 produces identical 1000-step translation/velocity bits.
Orientation/spin authoring likewise leaves translation bits unchanged.

## Transactional/time/precision evidence

- Equal exact 10 s host totals at 30/60/144 FPS and irregular 317-partition cadence,
  extra zero-duration UI wakeups, positive 0.1/1/10/100/1000 and negative −10/−1000
  rates: admitted targets and final physical bits agree after debt drains.
- Just-before/exact h, fractional retention, pause/zero work, rate/sign/cancellation,
  work4/debt16 overload refusal/resume, hidden elapsed exclusion, giant epoch/tick/
  configuration/rate overflow rejection pass without early epsilon steps.
- 100 retained states restore exactly; 8-slot history forces private replay;
  10,000-tick reset/replay repeats physical components. Seek cancel retains live
  state/revision. Name edits retain history; rejected edits, including unrepresentable
  kinetic diagnostics or nonfinite configured spin displacement, preserve
  playback/world/history. Appended/external state
  detects stale sessions until explicit paused rebranch. Actual byte caps reduce
  whole-snapshot retention and cannot accept fewer than two slots.
- Failed drift candidate promotes no cache/tick/world/history. Pump error reports
  earlier successful commits. Private failure halts replay and retains live state.
- 1000 gravity/frame publications copy centres/velocities exactly, keep independent
  spin, match revision/time, and rebuild fresh handles without changing world IDs.
  Wrong/stale projection and capacity/revision preflight reject; a new compile-fail
  contract prevents mutation while a coherent paired view is retained. Injected app
  projection failure suppresses drawing, pauses and reports the actual cancelled
  requested clock while preserving already committed work; rebuilding recovers.
- Source-centred sphere/marker precision and ≤0.05-pixel round trips at 1280×800
  and 3840×2160, reverse-Z positive w/decreasing depth, 32/32/64-byte layouts,
  unit topology/outward winding/chordal error, both celestial WGSL shaders pass.
- f64 trail clipping handles near/behind/frustum crossings before narrowing;
  poisoned body/line batches cannot submit. Marker selection uses physical-pixel
  radius, screen distance/depth/stable-order tie breaking; zero/behind projections
  produce no screen marker. Camera operations never mutate world/revision.
- Trail whole-sample eviction, FPS-independent stride, simultaneous subtraction,
  seek/reset/edit/direction/mode seeds and projection-rebuild retention pass.
  Repeated forward single steps retain ticks [0,8,16]; reverse starts its own strip.
  Seek clears/seeds immediately, before private replay, and seeds again on completion.
  Radius edits preserve body kinematics and update the derived camera navigation bound.

## Build, documentation and dependency checks

Windows x86_64-pc-windows-msvc, stable Rust 1.98.1 (`48a229cea`), LLVM 22.1.8,
Rust 2024, six non-publishable packages. All pass:

```text
cargo fmt --all -- --check
cargo build --locked --workspace
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked -p mundaris_simulation -p mundaris_world -p mundaris_math -p mundaris_renderer -p mundaris_app --release --all-features
cargo test --locked -p mundaris_simulation --release --test orbits long_run -- --ignored --nocapture
cargo doc --locked --workspace --all-features --no-deps
git diff --check
```

Rustdoc uses PowerShell `$env:RUSTDOCFLAGS='-D warnings'`. The final ordinary suite
has **75 runtime tests, seven compile-fail documentation tests and two ordinarily
ignored long tests**; both ignored release tests pass. The complete local suite
was rerun at implementation commit `62e2de6` during completion on 2026-10-02,
including formatting, build, all-target checks, workspace Clippy/debug tests,
focused release tests, both long release tests and warnings-denied Rustdoc.
Build/quality checks cover all benches. Dependency inspection confirms simulation→world/math,
renderer→math only, original dependency versions, and no unsafe/cycle/new workspace
crate/prohibited parallel/graphics physics framework. Text/links/fences and LF
metadata were reviewed. Benchmark results are distributions, not CI thresholds.

Completion verified retained Criterion estimate files against every registered
Phase 3 force, fixed-step, celestial preparation, trail sampling/request,
hierarchy and world/projection benchmark case. No missing benchmark group was
found; expensive 1024-body batches were not rerun unnecessarily. Key recorded
medians and confidence intervals match the raw estimates. Previously observed
native runs and the operator's visual report remain the interactive evidence.

## Native Windows and operator evidence

Windows 11 Pro x86-64 10.0.26200; AMD Ryzen 7 9800X3D, 8 reported logical processors;
AMD Radeon RX 9070 XT, driver 32.0.31041.1004; startup selected **Vulkan**.
The `--gravity-orbits` debug executable ran for 15 seconds with its Mundaris window,
presented without shader/pipeline errors, accepted normal window close and logged
clean shutdown. Final smoke, reference-frames, celestial-model and gravity-orbits
startup/presentation/close runs each remained alive for five seconds and closed
cleanly, with empty stderr and the same backend.

The operator answered **“Full sequence passed”** for the requested Windows §19
sequence: initial physical spheres/navigation, all selections, playback/steps,
connected focus/orbit/zoom, high-rate outer orbit, lag/overload, retained/older
reverse and seek/replay/cancel, editing/reset, inertial/relative trails/rebuild,
resize/minimize/restore/close. This is operator-reported visual evidence, separate
from residual tests and measured CPU benchmarks; no GPU pixel comparison or
presentation-rate capture is claimed.

Existing Vulkan loader diagnostics were observed: missing Khronos validation layer,
registry manifest lookup and duplicate AMD switchable-graphics layer. They caused
no pipeline/presentation failure and are not described as a warning-free driver run.

## Remaining Definition of Done items

### Current implementation audit clarification — 2026-10-02

This clarification describes the Phase 3 baseline before the subsequent Phase 3.5
implementation. [Phase 3.5 evidence](phase-3-5-validation.md) records the new
explorer behavior and its remaining acceptance; it does not replace the historical
Phase 3 operator evidence recorded here.

The [Phase 3.5 design/audit](../MUNDARIS_PHASE_3_5_CELESTIAL_NAVIGATION_SYSTEM_VIEW_AND_TIMEWARP.md)
preserves the Windows evidence above while distinguishing harness acceptance from
explorer usability. Arbitrary body selection exists via panel/Tab/marker, with
explicit focus buttons for any selected body. Focus is immediate and selection
alone does not move the camera. Overview reuses
initial COM/extent. Projected label clicking, sphere-area picking, label collision
layout, free flight, smooth focus transitions and orbit guides are not implemented.
The relative-history control always references fixture index 1, not current selection.
Historical trails exist and are enabled; startup is paused with one epoch sample,
and early overview arcs/planet-moon separation can be subpixel.

Detected minimize/occlusion/suspend already excludes hidden demand and clears debt;
the historical pre-implementation timing concern is not the current gravity path.
Sleep/debugger/long drawable stalls without a lifecycle event remain unguarded.
The reported native minimize/restore result and synthetic hidden-duration tests
do not prove that additional clock-gap case. Requested/achieved reporting already
exists, but its 250 ms samples can be tick-spiky or include control jumps. There is
no 10000x UI preset although the rate API supports it. These are source-audit
findings, not a new operator visual report or completed Phase 3.5 functionality.

The existing outstanding acceptance items remain:

1. **Linux x86-64:** native workspace build/check/Clippy/headless/Rustdoc, focused
   release/long-run envelopes, and full §19 desktop visual/lifecycle sequence with
   OS/GPU/backend/driver evidence. Available Ubuntu WSL2 reports kernel
   6.18.33.2-microsoft-standard-WSL2, but cargo/rustc/rustup/gcc/pkg-config are absent
   from its login-shell PATH; only the Windows Rust target is installed. No Linux
   result or timing is inferred from Windows.
2. **Current-revision remote CI:** normal Linux-quality and Windows-compatibility
   jobs must run for these commits. No remote changes were pushed, and local tests
   do not satisfy this criterion.
3. **Deferred prerequisite evidence:** Phase 1/2 Linux/remote acceptance and Phase 2's
   full Windows visual sequence remain as recorded in their own validation files;
   the explicit gate deferral permits this phase's implementation, not retroactive
   acceptance of those criteria.

Correctness review covers independent formulas/convergence, dense long-run maxima,
command/error boundaries and real-history precision. Architecture review confirms
authoritative world/derived frames/renderer separation and single-threaded scope.
Performance review records allocation inspection, actual overhead/scale probes and
the full-trail preparation goal miss without reducing numerical checks.

## Commits

- `22c2bd6` — preserve validated celestial model baseline (approved existing work).
- `0139f75` — deterministic Newtonian gravity.
- `76e02fb` — leapfrog orbital integration/diagnostics/conservation tests.
- `6a48513` — fixed-step playback and history.
- `6aff596` — coherent celestial publication views.
- `283e817` — celestial debug rendering.
- `7f6d0ee` — focus camera and initial conditions.
- `9993c90` — bounded committed-history orbit trails.
- `1758e78` — transactional branch diagnostic preflight.
- `3db2bb2` — gravity-orbits validation mode.
- `8b8b0ee` — benchmarks and boundary regressions.
- `106e70e` — preserve trail cadence across single steps.
- `a45f713` — shared live/private history payload ceiling.
- `dcfe0e2` — admission/spin/navigation/publication failure boundaries.
- `62e2de6` — immediate seek-trail invalidation and common-final-time convergence.
- Documentation acceptance record is the subsequent documentation milestone.
