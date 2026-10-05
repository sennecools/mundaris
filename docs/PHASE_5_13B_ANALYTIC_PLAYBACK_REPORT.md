# Phase 5.13B — default analytic playback integration

## Goal

Integrate the Phase 5.13A prescribed-motion foundation into ordinary solar-system
usage with responsive direct-time playback/seeking and coherent navigation/rendering.
Preserve the Newtonian hierarchy/circular solver and replay. Do not change terrain,
camera-control tuning, dependencies, solver architecture or start Phase 5.13C.
The user retains visual/UX and product acceptance authority.

## Result

**PARTIAL overall:** the integration is implemented; focused numerical/behavioral
checks and all 11 quality commands pass using the initial matrix plus affected
reruns. Native scripted validation has not passed; physical-device interaction and human visual/UX approval are
**UNTESTED**. Prior terrain, camera and UI acceptance gates remain open.

| Criterion | Verdict and evidence |
| --- | --- |
| Ordinary solar presets use analytic motion; Newtonian scenarios remain explicit | PASS / IMPLEMENTED: `GravityOrbitsDemo::solar_system`, app CLI routing and mode-specific tests. `GravityOrbitsDemo::new()` remains Newtonian. |
| Approved content policy and explicit frozen pacing | PASS / VERIFIED: solar authoring and original-initialization period/epoch regressions in `tests/solar_system.rs`; orbital inventory in `ANALYTIC_PLAYBACK.md`. |
| Signed rates, fractional/negative seeks, pause/resume, reset, documented single-step | PASS / VERIFIED: five playback/failure tests in both debug and release; single-step directly samples ±60 seconds. |
| History-independent direct times, including ±1000 years and return-to-T | PASS / VERIFIED: complete-state bitwise return tests for both presets; paired deterministic epoch/return PNG hashes match. Not a cross-platform bitwise promise. |
| Sampling/invalid-time/stale-binding failures preserve authority and pause | PASS / VERIFIED: invalid targets and rates, forced solver failure and stale revision regressions retain states/time/revision. |
| Coherent publication and failure recovery | PASS / VERIFIED: complete world sample precedes frame publication and camera/render work; app library publication-failure regression checks authority retention, draw gating and recovery. |
| Supported edits retain authored periods; derivative velocity is not editable | PASS / VERIFIED: checked metadata/property rebinding and period-preservation regressions; velocity edit disabled in analytic UI and rejected by command API. |
| Authored guides and successful-publication-only bounded trails | PASS / VERIFIED: guide/reference geometry, seek/reset/reverse trail and zero-rate regressions; existing caps retained. |
| Earth/Moon translating/body-fixed attachment precision | PASS / VERIFIED for numerical criteria: real-scale ±1000-year millimetre offsets retain local rotation semantics to <1e-15 m; attached camera pose survives seek. Native/human experience is separate. |
| Mode-aware diagnostics and shared default-authoring captures | PASS / VERIFIED: schema 3, coherent requested/published time checks, shared session captures at epoch/±day/return and distant fractional seeks. |
| At least 30 raw timing samples per fixture and explicit scopes | PASS / MEASURED: six preset/time fixtures ×30 repeats, separate complete sample/commit, frame publication and app-update scopes. Not FPS or a Newtonian speedup. |
| Native playback/focus/attachment route | PARTIAL: final observer retained 6/8 checkpoints (stages 1,2,3,4,6,7) and app exit 0. FAILED complete-route gate: Earth/Moon completed-focus stages 0/5 were not observed. All observed snapshots are unfocused/far away; physical-device input and human control approval UNTESTED. |
| Full locked quality matrix including GPU | PASS / VERIFIED across initial full matrix plus affected reruns: all 11 commands have passing final-source evidence. Four original build-blocked command results remain preserved. |
| Whitespace/source identity | PASS / VERIFIED: `git diff --check` exit 0; all 189 fast source fingerprints match final checkout, no source path added/deleted since fast validation. |
| Human visual/UX approval and settled terrain | UNTESTED / not claimed. All offscreen snapshots report `quality_pending=true`, `settled=false`. |

## Architecture changes

IMPLEMENTED: app-owned concrete `MotionSession::{Newtonian, Analytic}` selects the
producer; world and simulation remain domain owners. The analytic session uses
`TimeController` for requested time and `AnalyticMotionProducer` for complete
transactional samples. Sampling/commit, coherent frame publication, navigation and
render preparation remain ordered. Requested time and last successfully published
frame time are distinct on failures. Unchanged paused/zero-rate targets avoid
resampling. Hidden time and >250 ms interactive gaps retain existing exclusion and
pause policy rather than silently producing enormous jumps.

Sampling failure does not mutate authoritative world state/time/revision. A frame
publication failure retains the complete successful world sample, pauses playback
and suppresses drawing until a coherent projection is rebuilt. This is deliberately
different from rolling world authority back after a successful sample.

Supported name/mass/radius edits validate immutable definitions before atomic world
mutation and explicitly reconstruct the binding against the compatible world
namespace/topology. Periods, phases, spin and orbit references do not change. Arbitrary
external revisions are diagnosed as stale, not silently adopted. Terrain remains
body-fixed; reference translations never inherit parent spin.

Analytic dashed guides use immutable authored ellipses and the current translating
reference anchor, not mass-derived osculating conics. Unsupported alternate references
have no curve and an explanatory diagnostic. Analytic trails store synchronized
all-body successful publications; discontinuities clear/reseed without synthesizing
intermediate samples. The original Newtonian history/guide behavior remains separate.

### Approved solar policy

Approved 2026-10-05: stationary Sun at the existing epoch position; planets reference
Sun; Moon references Earth. Removing Earth's original inner wobble moves Earth and
Moon together by **292,894.815 m gameplay / 4,665,082.161 m real-scale** at epoch.
Solar reflex motion and zero total COM/momentum are not preserved-mode requirements.
This is an approximate authored catalogue, not a physical ephemeris.

Periods are frozen numeric inputs matching the original circular initialization
pacing, including Moon mass in Earth's initial period. Gameplay keeps its faster
~0.250568 scaling. Phases, inclinations, rotation convention, radii, colors and terrain
are retained. The full inventory and controls are in [ANALYTIC_PLAYBACK.md](ANALYTIC_PLAYBACK.md).

## Files changed

Task-owned production edits relative to the **dirty starting checkout**:

- `crates/app/src/motion_session.rs`: concrete selection, analytic time/failure/edit
  handling and motion diagnostics.
- `crates/app/src/gravity_orbits.rs`, `gravity_orbits/developer_ui.rs`,
  `gravity_orbits/analytic_validation.rs`: composition, mode-aware controls,
  publication/navigation, developer UI and opt-in native command route.
- `crates/app/src/solar_system.rs`: shared approved analytic preset authoring;
  original Newtonian constructor retained.
- `crates/app/src/orbit_guides.rs`, `trails.rs`: authored guides and publication history.
- `crates/app/src/system_view.rs`: authored guide/reference bounds in existing derived overview calculations.
- `crates/app/src/developer_snapshot.rs`, `developer_capture.rs`,
  `examples/developer_capture.rs`: schema 3 and integrated shared-session captures.
- `crates/app/src/lib.rs`, relevant app tests and
  `examples/analytic_playback_profile.rs`: exports, regressions and scoped timing evidence.
- Documentation: playback/interface guides, architecture integration note, this
  handoff, evidence scripts/index and dated reviewer context.

Many other dirty files predate this phase, including world/simulation foundation,
renderer, terrain, camera and dependency changes. Those are **not Phase 5.13B edits**.
No reset, cleanup, commit or push was performed. Final source-change manifest records
the exact changed/added paths relative to the starting fingerprints.

## Tests

Windows 11 / Rust 1.98.1 / x86_64 MSVC. Exact focused commands, exit codes and durations
are in [focused/commands.json](evidence/phase513b/final/focused/commands.json):

- `cargo fmt --all -- --check`: exit 0.
- `cargo clippy --locked -p mundaris_app --all-targets --all-features -- -D warnings`: exit 0.
- Debug and release `analytic_playback_513b` + `analytic_failure_513b`: exit 0,
  **5 tests each**.
- Affected app regressions: exit 0, **64 passed** including library tests.
- World/simulation: exit 0, **75 passed including doctests**, 2 long tests ignored
  here (explicit long-orbit command is in the full matrix).
- `./scripts/ai-check.ps1 -OutputDirectory docs/evidence/phase513b/final/fast`: passed;
  paired Earth epoch snapshot/image inspected. Fast checks are not the full matrix.

The one initial full matrix command was:

```powershell
./scripts/validate.ps1 -OutputDirectory docs/evidence/phase513b/final/full-quality -IncludeGpu
```

It passed formatting, workspace check, both Clippy configurations, workspace debug
tests, rustdoc and explicit long orbits. Workspace release tests and the three
explicit GPU commands exited 101 **before testing**, because the concurrent native
retry had `target/release/mundaris_app.exe` open and Windows denied Cargo's removal
of that file (os error 5). These original logs are preserved. Native runner now
copies the executable outside Cargo's output path. After the window closed, all
four affected commands **passed**, including each explicit GPU test actually
executing one test, in `final/quality-retry/`:

```powershell
./scripts/validate.ps1 -OutputDirectory docs/evidence/phase513b/final/quality-retry -IncludeGpu -Only tests-release,native_close_surface,native_full_frame,developer_interface
```

The initial matrix is not represented as all-pass; the combined evidence supplies
a passing result for each of the 11 commands on unchanged final source.

Linux/remote CI and physical native input are UNTESTED.

## Measurements

MEASURED: release ten-body solar app on Ryzen 7 9800X3D, Windows 11; dirty source
identified by the final fast fingerprints, HEAD
`d81bb2a17ad4f8c1d06eaa1619b6281b946641c3`. Two presets ×epoch/+1000/-1000-year
start fixtures, five warmups and **30 repeats each**. Each measured update advances
1 ms at 1×; fixtures are successive updates, not 30 identical seeks.

| Scope | Median range across fixtures | Largest observed sample |
| --- | ---: | ---: |
| Complete candidate evaluation + world commit | 0.5–0.7 µs | 0.7 µs |
| Frame publication + coherence validation | 0.1–0.2 µs | 0.2 µs |
| App update, including guides/trails/navigation | 1.3–1.6 µs | 25.8 µs |

Raw [profile.json](evidence/phase513b/final/runtime/profile.json),
[statistics](evidence/phase513b/final/profile-summary.json) and
[environment](evidence/phase513b/final/runtime/environment.json) identify scopes,
samples and hardware. Timer quantization is visible at these submicrosecond values.
App-update scope excludes render-time terrain admission/preparation, GPU and
presentation. No native FPS, memory/RSS/VRAM, terrain performance improvement or
equal-fidelity Newtonian solver speedup is claimed.

## Captures

OBSERVED/VERIFIED: deterministic offscreen captures use the shared gameplay solar
authoring/session, RX 9070 XT/Vulkan, 960×640, 64 terrain updates ×16 ms,
zero terrain workers and 150 ms morph duration. Epoch, +86400 s, -86400 s and return
are one session sequence; epoch/return PNG hashes match. Distant paired Earth/Moon
captures publish exactly ±31,557,600,000.25 s, schema 3, analytic mode, no motion
failure and matching world/published time. Images and snapshots were inspected.

All seven retained offscreen pairs report **quality_pending=true, settled=false**.
They establish capture/session coherence and visible bodies in these fixtures,
not settled terrain, useful surface convergence or human visual acceptance.
Original distant commands used the wrong argument order and failed; corrected
commands and fresh outputs are retained separately under `final/capture-retry/`.

Native evidence uses ordinary commands in an actual window, not physical devices.
Snapshots and screen images are observational and are **not guaranteed same-raster-
frame pairs**.

Final independent native observer retained **6/8 checkpoints** in
`final/native-observations/`: forward 1000×, reverse -1000×, paused fractional
+1000-year seek, resumed Earth body-fixed inspection, paused -1000-year Moon
inspection and epoch reset. Each observed snapshot has coherent analytic
world/published time and no motion failure. The app exited **0**, stderr is empty;
the **runner exited 1** because completed Earth/Moon fit stages 0/5 were missing.
This is partial observation, not a passed route.

All six native snapshots report `window_focused=false`, ~9.446–9.508e11 m body
distance, no active terrain, and null terrain quality/settled fields. The six
screen images were inspected: a foreground terminal obscures most of the native
window; only portions of developer UI and viewport/guide are visible. Playback,
signed rate and seek/reset text are visible where unobscured, but these screenshots
do not establish useful planet/surface rendering or human UX. Changing attachment
role while far from a body is not near-surface acceptance.

## Known failures

- First native attempt exited while exporting `latest.json`: os error 32, 0/8
  checkpoints. Its reader denied concurrent writing. A `FileShare.ReadWrite`
  reader retries incomplete JSON; the failed attempt remains under `final/native/`.
  Its summary exit code is null: no successful exit is claimed.
- Initial full release/GPU commands were build-blocked by the concurrent running
  executable. Their failures are preserved rather than overwritten; all four
  affected reruns passed after the native window closed.
- First non-blocking-reader retry (`final/native-retry/`) did not crash, queued all
  eight route stages and logged clean shutdown, but recorded 0/8 checkpoints and
  a null process exit code. The sequential observer waited for a missed first
  checkpoint. Final snapshot is unfocused, Moon body-fixed at time 0 with no motion
  failure, but **944,628,060,567.7701 m** from the Moon and no active terrain.
  INFERRED: focus-related cancellation prevented the fit transition from finishing;
  source queues navigation cancellation while native/egui focus is absent. This is
  not proof of a physical-device regression or near-surface acceptance.
- Final native observer retained stages 1/2/3/4/6/7 and clean app exit 0, but still
  failed the complete-route gate. Earth/Moon completed-fit checkpoints 0/5 are
  missing. OBSERVED: snapshots remain unfocused; screen images show a terminal
  obscuring most of the app. No focus-safety bypass or camera retuning was made.
- Human input, control feel and visual/UX approval remain UNTESTED.
- Prior terrain Acceptance A, convergence/headroom/performance, camera UX and
  developer UI human approval are not closed by analytic integration. Unsettled
  captures cannot close them.
- Prescribed circles omit N-body perturbations, solar reflex and Earth's inner
  wobble by approved policy. Time/cycle envelopes and checked failure behavior
  remain finite; no clamping, gravity fallback or arbitrary precision is added.

## Evidence

See [evidence index](evidence/phase513b/README.md). Starting dirty Git state and
184-file foundation match are retained in `prerequisite-20261005-000610-388/`.
Final fast fingerprints identify the tested source; `final/verification-final/`
compares all 189 inputs, records capture hashes and retains Git/whitespace results,
combined quality-command outcomes and native missing-stage summary. All 17 added/
changed source paths relative to the starting dirty checkout are app-only; no
source deletions, renderer/world/simulation/dependency changes occurred in this phase. Evidence
is indexed by fixture and validation scope, not inferred from this summary.

## Git state

Branch `main`, HEAD `d81bb2a17ad4f8c1d06eaa1619b6281b946641c3`; all work remains
**uncommitted** on the pre-existing dirty checkout. HEAD alone does not identify
the build. Start/final status and source fingerprints are retained in the evidence.
No commit/push was authorized or performed.

## Reviewer follow-up

Inspect task-relative source changes, full matrix plus affected reruns, raw profiles,
paired capture snapshots and the native checkpoint evidence. Review content policy
and failed-publication behavior against the actual source, not just this report.
Physical input and human visual/UX assessment remain separate. Discuss priorities
with the user; do not automatically start visible-universe/streaming Phase 5.13C,
terrain optimization, camera retuning or UI redesign.
