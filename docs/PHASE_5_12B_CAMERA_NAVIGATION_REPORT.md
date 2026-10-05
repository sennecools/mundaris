# Phase 5.12B — continuous camera/navigation handoff

Date: 2026-10-04. This is an **uncommitted PARTIAL implementation handoff**, not
navigation-feel acceptance, terrain acceptance, or authorization for a next phase.

## Goal

Implement [the agreed task](tasks/PHASE_5_12B_CAMERA_NAVIGATION.md): ordinary controls
from overview to complete-terrain metre scale; explicit pose-preserving modes;
projection/clearance-aware response; body-fixed horizon-stable editor motion;
separate speed components; input lifecycle ownership; canonical diagnostics and
reproducible evidence. The user retains navigation/visual acceptance authority.
Scope exceptions explicitly approved: the minimal native-event hook in
`crates/app/src/main.rs` and schema assertion 1 → 2 in `scripts/ai-check.ps1`.

## Result

**PARTIAL.** IMPLEMENTED: camera/input policy, focused controls and schema 2 diagnostics.
VERIFIED: the retained controller fixtures meet their numerical assertions, including
Earth/Moon 2 ± 0.05 m approach. **FAILED: the synthetic native route did not complete.**
Physical-device interaction and human approval are UNTESTED. No settled-terrain or
performance-improvement claim is made.

| Task criterion | Verdict | Evidence and remaining limitation |
| --- | --- | --- |
| 1. Ordinary-control route | PARTIAL | Controller wheel/mode/tangent/vertical route passes on Earth/Moon; paired route captures pass the 2 m numerical gate. Synthetic native runs fail before completing the route. |
| 2. Entry without approach | PASS, automated fixture | Entries at 100 km, 10 km and 2 m preserve pose. Separate rolled/polar fixture preserves the entry instant, then reconciles roll smoothly. Native entry/control feel is unverified. |
| 3. Pose preservation | PARTIAL | Same-body entry/exit ≤1 mm and ≤1e-6 rad verified; largest logged positional exit residual 6.51e-11 m. Interrupted inter-frame fixture logs 3.146e-5 m position residual; transition starts preserve pose. A comprehensive astronomical 64-ULP conversion matrix and interruption angular-residual matrix are not retained. |
| 4. Sensitivity matrix | PARTIAL | 60 body/local rows plus 12 overview rows: 30°/60°/90°, physical heights 720/1080/1620/2160, DPI 1/1.5/2. Near precision gate passes. Repeated turning is mechanically possible but 366 swipes for ~180° at 2 m is a usability risk, not practical-turn acceptance. |
| 5. Wheel continuity | PASS, exercised controller fixtures | Fractional inward/outward wheel before/after explicit body/surface/free-flight changes; scale traversal from 100 km to 2 m; unchanged FOV/user multiplier, finite signed response. No physical high-resolution device evidence. |
| 6. Timing equivalence | PASS, retained controller fixtures | Timestamp-boundary replay at 30/60/144 Hz, zero-time deltas, no-input smoothing, exact equal targets and mixed endpoints within unchanged bounds. Live native event/clock timing equivalence is not independently replayed. |
| 7. Basis/attachment | PARTIAL | Tangent/local-up motion, polar-adjacent transport, remap and real-gravity body translation/rotation tests pass; advanced carrier system-stationarity passes. Explicit comparative time-warp and fully quantified pole-crossing trajectory coverage remain incomplete. |
| 8. Input lifecycle | PARTIAL | Panel-owned presses, outside releases, text/focus cancellation, fractional scroll and app focus/resize/hidden-resume hooks pass headlessly. Native text/panel/shortcut/DPI/minimize sequence is not reached by the failed recorder. |
| 9. Speed/snapshot correctness | PASS, exercised tests and source | Base × user × boost, source labels, optional/null diagnostics, schema 1 decode/schema 2 output, body/frame associations and prepared PNG/JSON pairing verified. Static captures intentionally have null navigation. |
| 10. Human acceptance | UNTESTED / open | Neither automated tests, synthetic messages nor screenshots establish physical-device feel or user approval. |

These verdicts refer to the named evidence, not every possible navigation trajectory.
Phase 5.12A human UI approval and the existing terrain/Acceptance A blockers stay open.

## Architecture changes

- The app retains one frame-local f64 observer and read-only coherent world/frame input.
  No authoritative body, origin, terrain generator, renderer threshold or cap changes.
- Internal `SurfaceInspection`/`FreeFlight` names and serialized mode strings remain;
  UI labels are **Surface Navigation**/**Advanced Free Flight**. Explicit mode entry
  and exit re-express the instantaneous pose. Body orbit retains a look offset.
  Surface attachment follows body translation/rotation; advanced flight compensates
  numerical carrier motion to remain system-stationary. Simulation velocity is not
  relabelled as commanded wall-navigation speed.
- Surface WASD is tangent, Q/E radial. Wheel is view-forward/recede, not tangent
  movement, FOV adjustment or a speed adjustment. Default protection is 1 m sampled
  complete-terrain radial clearance, labelled sphere fallback where unavailable;
  this is neither swept-volume collision nor protection against pending drawn mesh.
- Surface heading/pitch and radial-up transport avoid a near-vertical singularity.
  Incoming roll is retained at entry and reconciles over time. Explicit horizon look
  remains separate. Input interrupts focus at the displayed pose, retaining surface
  policy when applicable or acquiring an orbit pivot, never implicitly advanced flight.
- Native events become app-owned host-timestamped navigation commands; wall integration
  splits at event boundaries. Viewport gestures and keyboard ownership are explicit.
  Focus loss, hidden/resume, excluded gaps and resize/DPI cancel input/targets. A new
  viewport must match the prepared projection before native input resumes.
- A late cancellation regression was reproduced after the first quality matrix:
  body-orbit cancellation froze sphere altitude instead of complete-terrain clearance,
  producing 273.109 m unintended motion in the independent probe. Cancellation now
  retains the displayed target quantity, including focus cancelled before its first
  sample. The new regression logs ≤2.911e-11 m motion and zero angular residual.
- Complete-clearance samples are reused by body, exact terrain definition, radius
  and radial direction. Snapshot collection copies the recorded controller sample;
  it does not query terrain or schedule work. Sample timing/count diagnostics cover
  the new cached controller-query path, not every pre-existing debug query.
- Canonical snapshot **schema 2** adds optional navigation diagnostics, target meaning,
  response values, speeds, safeguard quantity/minimum, native ownership and query
  counters. Schema 1 JSON lacking navigation still deserializes; readers must branch
  on version. Forward destinations are not falsely exported as radial targets.
- Capture route uses production controller wheel/mode APIs. Static seeding is limited
  to separate fixtures; the ordinary route does not use direct pose/clearance setters.
  Its GPU checkpoints are fresh prepared captures, not a live native terrain history.

Equations, units, clamps and time constants are in
[RESPONSE_POLICY.md](evidence/phase512b/RESPONSE_POLICY.md); controls/reproduction in
[CAMERA_NAVIGATION.md](CAMERA_NAVIGATION.md). Policy was recorded before curve edits;
later clarifications correct representative arithmetic without relaxing test gates.

## Files changed

Task edits, including edits layered on pre-existing dirty interface files:

- `crates/app/src/celestial_camera.rs`: response, explicit pose-preserving modes,
  transported heading/pitch, wheel destinations, complete samples and diagnostics.
- `crates/app/src/gravity_orbits.rs` and `gravity_orbits/navigation_input.rs`:
  native translation/timing, lifecycle cancellation, viewport ownership and snapshot wiring.
- `crates/app/src/gravity_orbits/developer_ui.rs`: focused mode/recovery/speed controls.
- `crates/app/src/main.rs`: approved event forwarding and consumed-shortcut hook.
- `crates/app/src/developer_snapshot.rs`, `developer_capture.rs`,
  `crates/app/examples/developer_capture.rs`: schema/interface and paired route capture.
- `crates/app/tests/camera_navigation_512b.rs`, `developer_interface.rs`,
  `celestial_navigation.rs`: numerical, schema and superseded-wheel semantics tests.
  `crates/app/examples/navigation_profile.rs`: focused controller/query profile.
- `scripts/ai-check.ps1`: approved single schema-version assertion.
- Navigation/interface documentation, this report, reviewer-context addendum and evidence.

Pre-existing terrain/renderer, dependency manifests, native full-frame tests and workflow
changes were preserved. The task-start `lib.rs` fingerprint is unchanged. Start status,
six captured baseline source files, their hashes and the task-start/current diff are
retained in the evidence package. The baseline fingerprint list covers only those six
files; it is not a full-checkout before-state manifest. Final fingerprints cover Cargo
inputs and crate sources; HEAD alone does not identify this dirty candidate.

## Tests

Windows 11 Pro, x86_64 MSVC, Rust/Cargo 1.98.1. Commands use
`CARGO_TARGET_DIR=target/phase512b-build`; the user's existing release app was left alone.

- `cargo test --locked --release -p mundaris_app --test camera_navigation_512b -- --nocapture`:
  exit 0, **7 passed**, final matrix in `navigation-matrix-final3.txt`; includes the
  late terrain-orbit/unstarted-focus cancellation regression.
- `cargo test --locked --release -p mundaris_app --lib celestial_camera::tests -- --nocapture`:
  exit 0, **4 passed**, in `navigation-matrix-final.txt` (before cancellation fix);
  rerun with the final all-feature app library tests below.
- `cargo test --locked --release -p mundaris_app --lib navigation_input::tests -- --nocapture`:
  exit 0, **2 passed**, in that log.
- `cargo test --locked --release -p mundaris_app --test celestial_navigation -- --nocapture`:
  exit 0, **5 passed**, in that log.
- Mistyped `--test surface_navigation`: exit 101, no such target, retained rather than
  hidden. Corrected `--test planet_surface_navigation -- --nocapture`: exit 0,
  **3 passed**, `surface-navigation-final.txt`.
- `./scripts/ai-check.ps1 -OutputDirectory docs/evidence/phase512b/fast-check-final`:
  exit 0; formatting, interface tests, capture and association checks pass. Paired
  Earth image/JSON inspected; this is not full validation or settled-terrain proof.
  After the cancellation fix, `fast-check-final2` also passes and its pair was inspected.
- `cargo build --locked --release -p mundaris_app --all-features`: exit 0,
  `native-build-final2.txt`; final executable SHA256 retained.
- `./scripts/validate.ps1 -IncludeGpu -OutputDirectory docs/evidence/phase512b/full-validation-final`:
  exit 0, all **11 commands pass**, including debug/release workspace tests, both
  warnings-denied Clippy configurations, warnings-denied rustdoc, ignored long orbit
  tests and three GPU groups. This was before the late cancellation fix; affected
  checks are rerun separately on the final source rather than repeating unrelated
  long-running terrain/world suites. See raw `validation.json` and rerun evidence.
- Final affected rerun selects app library, `camera_navigation_512b`,
  `celestial_navigation`, `planet_surface_navigation` and `developer_interface` in
  debug/release, all features, plus script selections format/workspace check/both
  Clippy configurations/rustdoc/three GPU groups. Both test commands exit 0, **56 pass
  and 1 GPU test is ignored per profile**; the ignored GPU group is then run explicitly.
  All **8 affected quality commands pass**, exit 0. Exact commands/results in
  `affected-tests-*.txt` and `affected-validation-final/validation.json` are separate
  from the earlier full matrix. The app tests use `cargo test --locked -p mundaris_app
  --all-features --lib --test camera_navigation_512b --test celestial_navigation
  --test planet_surface_navigation --test developer_interface -- --nocapture`, with
  `--release` added for the second profile. Quality rerun uses the full-validation
  script with `-IncludeGpu -Only format,workspace-check,clippy-all-features,
  clippy-default,rustdoc,native_close_surface,native_full_frame,developer_interface`.
- `git diff --check`: exit 0, `diff-check-final.txt`; final rerun accompanies handoff.

No Linux/remote-CI or physical-device/human navigation acceptance is claimed. Earlier
failed/intermediate tests remain indexed; none were deleted to manufacture a pass.

## Measurements

MEASURED: `navigation-matrix-final3.txt`, gameplay fixture (Earth radius 400 km),
60° vertical FOV/1080 logical height/200 logical pixels:

| Complete clearance | Local angle | Body-pivot angle |
| --- | ---: | ---: |
| 2 m | 0.492422° | 0.027400° |
| 10 m | 0.501821° | 0.061266° |
| 100 m | 0.606526° | 0.193719° |
| 10 km | 6.370964° | 1.913636° |
| 100 km | 11.182518° | 5.479680° |

The 2/10 m local and pivot values are nonzero and below 1°. Equal logical heights
at DPI 1/1.5/2 give identical measured values. Overview's same swipe is 12.251753°.
Repeated 2 m local turning takes **366 × 200 = 73,200 logical pixels** for 180.226°
cumulative turn. This potentially cumbersome behavior needs user review; an eventual
coarse-look policy would require explicit agreement, not a quiet gate relaxation.

Timestamp fixtures have exactly equal requested overview targets
342155071997.18097 m at all three rates. Smoothed distances differ by ≤467.1 m at
~3.444e11 m (≤1.36e-9 relative). The ~51.533 m mixed trajectory's maximum logged
endpoint residual is 1.130e-5 m, clearance residual 1.251e-6 m and quaternion
orientation discrepancy 0 at the reported precision (bound 1e-4 rad). Same-body
entry/exit has zero reported angular discrepancy. Rolled entry has 0 entry rotation,
0.001997 rad change after 1 ms and ~1.322e-4 residual right/up dot after 3 s.

Controller CPU microprofile: Ryzen 7 9800X3D, release, gameplay Earth, nominal 100 m
complete-clearance setup; 20 samples × 1,000 calls, 1 ms admitted time per call.
Identical named idle/look/tangent inputs and sampling protocol; policy/trajectory
semantics intentionally differ. Baseline is the task-start dirty controller, not HEAD.

| Fixture | Baseline median µs/call | Final median µs/call | New counted complete queries / 20,000 calls |
| --- | ---: | ---: | ---: |
| Idle | 0.0925 | 0.6098 | 0 |
| Look | 0.0990 | 0.5511 | 0 |
| Tangent | 0.1193 | 148.8960 | 20,000 |

Final tangent queries total 2,963,475.4 µs (~148.174 µs/query). Separate complete-query
mean is 149.719 µs versus baseline 147.467 µs; this variability is not a speedup claim.
Final idle sample range is 0.5467–4.4156 µs/call, look 0.5421–0.7677 and tangent
146.8943–152.0164. The pre-cancellation run is also retained (142.0666 µs tangent);
normal host variability and policy changes preclude attributing this difference to
the cancellation fix.
Baseline query count was not instrumented; baseline source with the optional guard
disabled performs no procedural query in those surface-update loops. Final results
exclude rendering, terrain preparation, UI, GPU, readback, encoding and file I/O.
Native input-translation-only CPU work and native event overhead were **not separately
profiled**. The added complete-terrain motion cost is real, not zero-cost navigation.

Raw logs: `baseline/controller-profile-retry.txt` (UTF-16) and
`controller-profile-final2.txt` (UTF-8). `environment.json`, `candidate-final.json` and final
source/binary hashes identify the machine/candidate. An existing user app may add
background load; no exclusive-machine benchmark or native FPS result is asserted.

## Captures

`captures-final2/` contains six prepared PNG/JSON checkpoints and `navigation-route.json`.
Command: `cargo run --locked --release -p mundaris_app --features
terrain-capture,surface-profile --example developer_capture -- navigation-route
docs/evidence/phase512b/captures-final2` (exit 0). RX 9070 XT/Vulkan; 960×640 physical
and logical viewport, DPI 1, FOV 60°, natural layers, no LOD colors/borders/markers at
near checkpoints; 64 terrain updates × 16 ms, zero workers per isolated capture.

| Checkpoint | Requested complete clearance | Actual complete clearance | Reference altitude | Drawn-mesh clearance | Navigation time |
| --- | ---: | ---: | ---: | ---: | ---: |
| Earth | 2 m | 2.0000013886601664 m | -90.852337 m | 243.818781 m | 5.0 s cumulative |
| Moon | 2 m | 2.0000013886747183 m | 72.285685 m | 63.359471 m | 10.2 s cumulative |

The target is route intent recorded from wheel calibration, not a controller radial
setter; controller snapshots correctly report `view_forward_destination` and null
requested radial clearance. Navigation time excludes capture/GPU/I/O wall time. Both
bodies satisfy 2 ± 0.05 m, independent of drawn LOD. Tangent/up inputs follow each
checkpoint; the final overview recovery is captured.

OBSERVED: both near PNGs are largely uniform dark images. Earth source/desired radial
LOD is 1/22, Moon 1/19; both `quality_pending=true`, `settled=false`. Their positive
drawn-mesh clearance does not establish visible whole-view quality. The captures do
not demonstrate terrain acceptance or continuity of a live native refinement history.
The final fast-check Earth-orbit pair visibly shows Earth and is also quality-pending.

Native evidence is separate in `native/run1`–`run5`: owned-window screenshots and
canonical prepared snapshots, **not same-prepared-frame PNG/JSON pairs**. Recorder
messages are synthetic, not real mouse/keyboard operation. All owned runs requested
graceful close; runs 2–5 record process exit 0, while run 1 records null/unavailable
exit status. Graceful process exit does not turn route failures into passes. Run 2 reaches body
orbit but stops at 959,988.137527 m rather than 100 km. Runs 1/3/4/5 fail Earth focus.
Final run 5 reports `window_focused=false`, keyboard/gesture ownership false, and no
wheel received. INFERRED: unavailable foreground/focus prevents that synthetic route;
this does not establish a physical-input engine bug or a successful native route.

## Known failures

- **FAILED:** synthetic native route; physical native route/control feel unavailable.
- **UNTESTED:** human approval, native panel/text/shortcut/minimize/DPI sequence and
  independent live native timing replay. Some full quantitative coverage is partial
  as identified in the criterion table (astronomical conversions, time warp, poles).
- Near-surface practical turning may be too cumbersome despite numerical precision.
- Complete-terrain queries substantially increase active tangent-controller CPU work.
  Optional pre-existing debug guards also have queries outside the new counted cache.
- Near images are dark/uniform and terrain quality pending; useful visible surface
  inspection, drawn-mesh convergence and Acceptance A remain separate unresolved work.
- The first full quality matrix passed but predates the cancellation fix; final
  affected validation also passes and must be read separately, not inferred from
  that earlier pass. Native interaction and broader coverage gaps remain open.

## Evidence

[Evidence index](evidence/phase512b/README.md) links final and intermediate runs,
response policy, baseline state, hashes, logs, pairs and the native failures.
Final source fingerprint: `evidence/phase512b/fast-check-final2/source-files.sha256.json`.
Native binary fingerprint: `evidence/phase512b/native-binary-final2.sha256.json`.
Native run 4 and the first full matrix predate the cancellation fix; their prior
source manifest remains in `fast-check-final`. Native run 5, the second fast check,
route/profile reruns and affected validation use the same final source fingerprint.
`artifact-association-final2.json` verifies zero source-hash mismatches and six PNG/JSON
filename/schema associations; visual observations are separately reported above.
Source presence is not measurement
or visual/native acceptance; the report is an index, not a substitute for raw evidence.

## Git state

Branch `main`, HEAD `d81bb2a17ad4f8c1d06eaa1619b6281b946641c3`, substantially dirty
at task start and handoff. **Nothing committed or pushed.** Baseline status is
`baseline/git-status.txt`; final status is `git-status-final.txt`. Captured task-start
source copies and `task-baseline-diff-final2.patch` distinguish these controller edits from
pre-existing work where the baseline was captured. No unrelated user process was
terminated, and no neighboring terrain/renderer refactor was undertaken.

## Reviewer follow-up

Inspect the baseline-relative diff and final paired source/snapshots, then reproduce
with actual viewport-owned physical controls on Windows. Prioritize the native focus/
route gate and precise-versus-practical near look before accepting the policy. Review
the query-cost tradeoff and remaining coverage gaps separately. The user decides
control-feel acceptance; do not automatically begin terrain or another camera phase.
