# Phase 3.5 validation record — 2026-10-02

## Status and evidence boundaries

Phase 3.5 celestial exploration, exact playback scheduling/reporting and shared
interactive-clock handling are implemented. Requirements remain
[the Phase 3.5 contract](../MUNDARIS_PHASE_3_5_CELESTIAL_NAVIGATION_SYSTEM_VIEW_AND_TIMEWARP.md).
Decisions and API choices are in [ADR 0005](adr/0005-celestial-navigation-system-view-and-timewarp.md).
Phase 4 has not begun and no approximate authoritative propagation exists.

Windows automated checks, optimized numerical tests, unchanged long-run tests,
benchmarks and directed native rendering/input/lifecycle checks have been executed.
**Complete Definition of Done is still open:** the full human Section 24 sequence,
actual OS sleep/resume and high-DPI checks, Linux native acceptance and current
remote CI have not been established. No earlier operator report is repurposed as
Phase 3.5 acceptance. The explicit implementation request permits this milestone's
work while the Phase 1/2/3 platform and Phase 2 full visual gates remain unaccepted.

## Implemented explorer

- Current geometric whole-system bounds, physical radii, analytic guide extrema,
  display-history cap/full-fit policy, actual dock-excluded viewport and padding.
  Explicit membership and one-generation automatic-reference subsystem scopes.
  Startup orbital-plane direction, smooth visual-centre tracking, grow-only
  auto-fit hysteresis and user-input disablement of automatic growth.
- BodyId list/label/marker/reference-sphere picking. Marker overlap returns every
  stable candidate; repeated clicks cycle them. Labels use final displaced
  rectangles, leaders and deterministic bounded layout/hysteresis. Names with
  duplicates receive a secondary index. Selection and focus remain separate.
- One checked observer with System Orbit, Body Orbit and Free Flight. All bodies
  can be focused with interruptible camera-only transitions. Saved body views and
  paused system orientation/fit are retained; current bounds are refitted when
  authority advances. Normal focus does not inherit body spin.
- Physical-radius/clearance zoom with a 1 m/ULP floor and logarithmic wall response;
  scale-aware normalized editor flight, manual speed, translating carrier
  compensation and explicit unfocus. The debug reference sphere is labelled;
  metre clearance does not claim metre-accurate surface mesh detail.
- Minimum derived markers, sphere-aware fade, selected/focused rings, short
  observer distances and a persistent curve-semantics legend. Physical mesh radius
  remains copied from immutable authoritative state.
- Real committed synchronized history, 1.5/2.5 px solid curves, timestamp age fade,
  selected emphasis and bounded existing-vertex simplification. Arbitrary explicit
  relative history presentation retains records. Seek/branch/direction invalidation
  remains separate from presentation changes.
- Read-only osculating two-body guides in current translating reference frames,
  automatic/explicit reference diagnostics, perturbation cautions, elliptic/
  hyperbolic/near-parabolic/degenerate classification, exact bounds and bounded
  screen-refined dashed curves. Unavailable curves leave bodies/history usable.
- Ordered UI commands, F/double-click focus, Home overview, Tab selection,
  next/previous focus and WASD/QE flight. Camera navigation works while paused.

## Headless and numerical evidence

All tests avoid GPU/display creation. Existing Phase 1–3 physics tolerances remain
unchanged. New coverage is in `app/tests/celestial_navigation.rs`,
`celestial_system_view.rs`, `celestial_bounds.rs`, `playback_reporting.rs`, app
session tests, simulation `orbital_elements`/`fixed_steps`, and renderer precision/
styled-line tests.

| Contract | Evidence |
| --- | --- |
| Read-only visualization | Before/after complete body copies and world revision for bounds, guides, label/pick preparation, focus/transitions and visual/history assembly; exact unchanged state |
| Exact scheduling | Old pump512 versus variable 1/8/32 chunks: every public callback state, final BodyStates, retained history and restore/private replay targets agree; invalid work budgets are transactional |
| Conic math | Independent circular/e=0.3 inclined construction recovers a/e/p/normal/peri/apo with 1e-12 relative/basis and 1e-7 m reconstruction envelopes; analytic extrema enclose 1000 samples |
| Unavailable guides | Escape/positive-energy/radial/zero-separation/overflow cases never evaluate a closed ellipse; equal masses require an explicit reference |
| Reference policy | Hierarchy planet→star, moon→planet, star unreferenced, checked differential perturbation; explicit unbound pair classified honestly |
| Picking/layout | Physical sphere edge with hidden markers and offscreen centre, behind-camera exclusion, displaced rectangles, stable overlap cycling, no accepted label overlap, selected priority and stale-namespace rejection |
| Bounds | Whole-system outlier included; local subsystem omits outer guide; empty finite default and single real radius; capped/full displayed history; wide/tall offset viewport projection |
| Camera | Every fixture body focus, retarget continuity, 30/60/144 Hz transition/zoom endpoints, 1 m clearance and small notch behavior, frame rebuild and no-input/no-inertia flight |
| Carrier | Real gravity commits under free flight preserve system-stationary observer within the existing 1e-3 m astronomical conversion budget; numerical carrier is not focus |
| History | Cadence/eviction/reverse/seek tests retained, presentation switches preserve ticks, paused epoch has guides and zero history segments |
| Reporting/clock | Synthetic signed commit windows, tick quantization, metric resets, exactly 250 ms accepted, larger/10-hour gaps rejected, hidden/restore capture and existing-debt cancellation with unchanged physical state/observer |
| Renderer | Physical-pixel width at two viewport scales, dashed quads and fading color, WGSL validation, offset projection/unprojection, near/astronomical curves ≤0.05 px narrowing, invalid styled batch poisoning |
| UI routing | Headless actual egui pointer/wheel events reach the same observer; ordered select/focus is not overwritten by viewport commands |

The 60 **accounted** wall-second matrix runs the actual app update/pump at 16 ms
opportunities for both fixtures and 1/100/1000/10000/100000/1000000x. It is synthetic
demand, not a 60-second presentation benchmark. Hierarchy h60 reaches every full
requested tick with no rejected demand/debt. Its final 10-second samples are
6/102/1002/10002/100002/1000002x respectively: the 1x value is a tick-limited window,
while the since-segment average is 1x. Circular h10 reaches through 100000x;
1000000x honestly exceeds the 512-opportunity ceiling and latches overload.

Unchanged release long runs were rerun:

| Run | Max relative energy | P/Qp | L/Ql | COM residual |
| --- | --- | --- | --- | --- |
| Circular h10, 1000 periods | 1.1213312e-11 | 1.6505902e-13 | 1.3096737e-13 | 9.5498088e-7 m |
| Eccentric e0.3 h10, 1000 periods | 3.6288931e-6 | 1.1342715e-13 | 2.9628381e-13 | 6.5660692e-7 m |
| Hierarchy h60, two outer periods | 1.5336759e-13 | 1.0756820e-13 | 7.6754689e-14 | 6.4013909e-7 m |

Circular radius drift is 3.3374781e-6, accumulated phase 0.0139805959 rad,
return-period error 2.2249811e-6 T. Hierarchy lunar separation remains
[99,958,433.1291, 100,020,034.9111] m. These reproduce Phase 3's maxima; no new
guide/navigation calculation changes physical evolution.

## Windows quality checks

Stable Rust 1.98.1 (`48a229cea`), LLVM 22.1.8, Rust 2024,
`x86_64-pc-windows-msvc`; six `publish=false` packages, original dependencies and
unsafe prohibition. Commands executed:

```text
cargo build --locked --workspace
cargo check --locked --workspace --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked -p mundaris_simulation -p mundaris_world -p mundaris_math -p mundaris_renderer -p mundaris_app --release --all-features
cargo test --locked -p mundaris_simulation --release --test orbits long_run -- --ignored --nocapture
cargo doc --locked --workspace --all-features --no-deps
```

Rustdoc uses PowerShell `$env:RUSTDOCFLAGS='-D warnings'`. Final source checks are
recorded through navigation hardening milestone `7e5be8d`: **96 runtime tests, seven compile-fail
documentation tests and two ordinarily ignored long tests**. Both long release
tests pass separately. Debug/release numerical envelopes are unchanged, and build,
all-target checks, formatting, warnings-denied Clippy and Rustdoc pass. Criterion target execution and
actual distributions are in [performance](performance.md#phase-35-baseline--2026-10-02).
No ordinary-CI benchmark/GPU job was added.

## Directed native Windows evidence

Windows 11 Pro 10.0.26200, Ryzen 7 9800X3D (8 reported logical processors), Radeon
RX 9070 XT driver 32.0.31041.1004, **Vulkan**. Debug startup logged adapter and
successful initialization with empty stderr; existing missing-validation-layer,
registry-manifest and duplicate-AMD-layer loader warnings remained.

Actual native windows were exercised using directed window keyboard/pointer
events and window captures. Captures were inspected, not committed as repository
assets. Observed: immediate whole-system circle/three separated names/list;
planet and moon selection/focus; physical sphere area and list picking; camera
unfocus/free movement/refocus; current overview; labels and solid recorded arcs
distinct from dashed guides; 1280×800 content and a tall window; minimize/restore;
normal window close. This establishes these directed observations, not a complete
human operator signoff or GPU frame-timing measurement.

The original smoke, `--reference-frames` and `--celestial-model` release modes were
also opened, captured and closed normally after the native explorer run. Their
full mathematical suites pass; their old unperformed operator gates remain open.

| Profile / request | Active run / measured window | Captured achieved | Pending |
| --- | --- | --- | --- |
| Debug 10000x | short startup sample / 2.0 s | 9993.9x | 0 |
| Debug 100000x | ≥60 s / 8.5 s window before timing-cap expansion | 100002.1x | 0 |
| Debug 1000000x | ≥60 s / 10.0 s | 999999.6x | 0 |
| Release 100x | ≥10 s / 10.0 s | 102.0x | 0 |
| Release 1000x | ≥10 s / 10.0 s | 1001.7x | 0 |
| Release 10000x | ≥10 s / 10.0 s | 9999.1x | 0 |
| Release 100000x | ≥60 s / 10.0 s | 100004.0x | 0 |
| Release 1000000x | ≥60 s / 10.0 s | 999998.8x | 0 |

All show baseline exact/full N-body KDK/h60. Requested and achieved are not copied;
the finite-tick deviations are visible. Historical retention reached 8192 real
samples and useful outer arcs after advancement. No growing debt was observed in
these captures. No captured presentation-FPS distribution is claimed.

Suspending/resuming the native process for three seconds produced the visible
diagnostic **“Interactive clock gap 3.025 wall s; no catch-up requested. Pending
demand cancelled. Resume explicitly.”** Playback remained paused with zero debt
after restore. This validates an actual drawable scheduler discontinuity; actual
OS sleep and delivered OS-suspend events remain separate unperformed checks.

## Remaining acceptance and API review

1. Full Section 24 human workflow at both normal and high DPI, including continuous
   mouse orbit/near-surface wheel feel, all disambiguation/labels, authoring,
   restore/private seek/cancel and surface-recovery interactions. Synthetic wheel
   delivery did not establish native near-surface wheel behavior; headless actual
   egui routing and clearance mathematics pass.
2. Actual OS sleep/resume, OS suspension/zero-size/occlusion where available, and
   captured presentation cadence. A three-second process stall is not renamed OS
   sleep evidence. Native startup/directed captures do not close the full Windows
   operator checkbox.
3. Linux native quality/release/desktop sequence. WSL Ubuntu still reports kernel
   6.18.33.2-microsoft-standard-WSL2 with no cargo/rustc/gcc/pkg-config on the login
   PATH; no Linux success is inferred.
4. Current-revision Linux-quality/Windows-compatibility remote CI. Nothing was
   pushed; local Windows quality is not remote CI.
5. Complete Section 23 distribution/allocation/presentation coverage: measured
   CPU groups exist and ran, but no allocation profiler or presentation-FPS capture
   was collected. N16 full-retention default preparation exceeds the 4 ms goal;
   maintain this measured limitation rather than loosen precision.

API sketches were semantic: projection origin is a checked builder preserving
old callers; generic styled quads pack clip-relative coordinates after f64 checks;
app status composes the existing runner report and rolling metrics. Transition
interruption deliberately chooses pose-preserving Free Flight. New benchmark
common fixtures and a focused bounds test are small justified file-plan additions.
Debug frame re-expression is restricted to translating/fixed roles of the same
completed focused body; arbitrary generic pose/kinematic conversion remains in
Phase 1 math. This prevents a numerical frame change from silently replacing the
orbit pivot. Refocus after looking away in free flight uses outward radial pull-back
and an external transit arc, tested against the old/target envelopes. Radius growth
around a focused observer reports its camera-only clearance adjustment. Visual
preparation timing now includes app curve preparation before renderer narrowing.
No world/math/kernel change, dependency upgrade, framework, preview or Phase 4
terrain/LOD feature is introduced.

## Milestones

- `2b89434` — bounded exact runner pumping and pure osculating conics.
- `317db49` — content viewports and renderer-owned styled curves.
- `2d85603` — connected app navigation/selection/labels/guides/history/playback UI.
- `4295923` — final bounds/carrier/UI/history/viewport regressions and representative
  normal-count/scale-probe benchmarks.
- `7e5be8d` — outward focus transit, focused-identity debug migration and explicit
  reference-envelope adjustment/reporting hardening.
- The subsequent documentation milestone records architecture/acceptance and its
  hash is reported in the completion summary.
