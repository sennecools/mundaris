# Phase 5.12B — Continuous Camera and Navigation

## Task status and objective

This is an implementation task, not an implementation or acceptance report.
Creating this document does not begin the coding phase. Implementation requires
a separately authorized coding session.

Deliver predictable navigation from solar-system overview to meter-scale terrain
inspection through ordinary controls. Provide distance/FOV-aware rotation and
zoom, explicit pose-preserving camera modes, horizon-stable planet-relative editor
flight, and understandable automatic speed and user adjustments. Keep unrestricted
system-stationary free flight as a distinct advanced mode.

The user owns visual and navigation-feel acceptance. Phase 5.12A's remaining human
UI approval is separate; this task neither closes it nor accepts terrain quality.

## Governing references and starting state

Read `AGENTS.md`, `docs/REVIEWER_CONTEXT.md`, `docs/architecture.md`,
`docs/engine-invariants.md`, `docs/coding-standards.md`, and
`docs/adr/0002-reference-frames-and-precision.md`. Consult camera section 14 of
`docs/ENGINE_MECHANICS_REFERENCE.md`, but verify historical descriptions against
current source. Use `docs/AI_DEVELOPMENT_INTERFACE.md` for capture/snapshot semantics
and `docs/REVIEW_HANDOFF.md` for the final report.

Planning baseline: main, HEAD `d81bb2a17ad4f8c1d06eaa1619b6281b946641c3`.
The checkout contains substantial pre-existing dirty/untracked terrain, renderer,
developer-interface, and reviewer-workflow work. Before editing, record current
Git state and inspect relevant diffs. Preserve that work, including edits within
shared task files. HEAD alone does not identify a dirty build. Do not commit or
push unless explicitly requested.

## Current source-established behavior

These observations establish the starting problem, not measured UX or performance:

| Behavior | Source anchor |
| --- | --- |
| Four modes: `SystemOrbit`, `BodyOrbit`, `FreeFlight`, `SurfaceInspection`. | `crates/app/src/celestial_camera.rs`, `CameraMode` |
| Orbit and local look use fixed `0.005 rad/pixel` sensitivity. | `navigation_checked`, `free_flight`, `inspect_motion`, `orbit_zoom` |
| Ordinary orbit zoom changes logarithmic reference-sphere clearance; explicit clearance targeting enables terrain-aware approach. | `target_clearance`, `navigation_checked` |
| Body-orbit clearance targets smooth toward a destination, while inspection clearance targets immediately reposition the observer. | `target_clearance` |
| Surface motion speed derives from reference-sphere altitude, not complete-terrain clearance. | `inspect_motion` |
| Wheel input in local modes changes the manual speed multiplier and does not reach the controller as zoom. | `crates/app/src/gravity_orbits.rs`, central viewport input handling |
| Active movement input during a focus transition enters free flight. | `navigation_checked` |
| Surface inspection explicitly attaches to the body-fixed frame; free flight uses a system-stationary policy with numerical carrier compensation. | `enter_surface_inspection`, `enter_free_flight`, `free_flight` |
| Current snapshots record semantic pose frames and distinct sphere/terrain/mesh clearances, but not the full speed decomposition or navigation response state. | `crates/app/src/developer_snapshot.rs`, `CameraSnapshot` |

The fixed sensitivity, differing wheel meanings, transition interruption policy,
and reference-altitude speed are potential sources of inconsistent control feel.
Treat that explanation as inference until native interaction establishes it.

## Desired UX and camera mode semantics

Keep modes explicit internally and visibly understandable. Do not automatically
switch modes because an altitude, distance, or clearance threshold was crossed.
Internal enum names may remain if appropriate; present Surface Navigation and
Advanced Free Flight clearly in the UI.

| Mode | Control and attachment policy |
| --- | --- |
| System orbit | Orbit the overview anchor; wheel changes logarithmic target distance. |
| Body orbit | Orbit a focused body; wheel changes logarithmic target clearance, terrain-aware where complete terrain is available. |
| Surface Navigation | Horizon-stable body-fixed editor flight, following the body's translation and rotation. Wheel performs scale-aware forward/recede navigation without leaving this mode. |
| Advanced Free Flight | Unrestricted editor flight, intentionally system-stationary rather than co-rotating. A numerical carrier frame must not silently change this physical navigation policy. |

Selection is independent of camera focus and attachment. Provide clear Focus,
Frame Selected, Overview, Surface Navigation, and Advanced Free Flight actions.
Overview and Frame Selected must remain reliable recovery paths.

### Entering Surface Navigation

- Switching into Surface Navigation changes attachment/reference/control policy,
  not camera position. Preserve instantaneous position and orientation through
  correct frame re-expression.
- Do not teleport, automatically approach terrain, or reset the view to the horizon.
- Entry is valid at 100 km, 10 km, 2 m, and other valid distances. The user can
  continue navigating continuously from the incoming pose.
- If an incoming rolled orientation conflicts with horizon stabilization, preserve
  the entry pose and reconcile roll smoothly afterward; no immediate roll snap.
- Keep explicit Look Toward Horizon separate from entering the mode.
- Preserve instantaneous pose when leaving the mode as well. Attachment changes
  may intentionally change subsequent simulation-relative motion; document that
  policy separately from pose preservation.

Focus transitions may deliberately travel to a destination. Their start and
interruption must not jump the displayed pose. User navigation must interrupt
them according to an explicit documented policy, not silently enter Advanced
Free Flight. Clear stale transition/zoom targets as needed without losing the
incoming view or user intent.

## Zoom and rotation response policy

Before coding response curves, write down equations, units, clamps, smoothing time
constants, and representative numerical outputs in the implementation evidence.
Use actual prepared projection/viewport information rather than assuming a fixed
60-degree FOV or full-window viewport. Keep policy finite, continuous, monotonic
where appropriate, and usable across the tested distance range.

### Wheel semantics

- System/body orbit: wheel changes logarithmic target clearance/distance.
- Surface Navigation: wheel performs scale-aware forward/recede navigation along
  the view direction while remaining in horizon-stable Surface Navigation. A
  downward-looking wheel approach is intentionally distinct from tangent WASD
  movement. Apply the documented navigation safeguard, if enabled.
- Wheel must not alter FOV or the movement-speed multiplier. Provide separate
  multiplier controls. Keep this rule consistent in Advanced Free Flight too.
- Do not silently change wheel behavior because an altitude threshold was crossed.
- Preserve approach/recede intent across explicit mode changes. Avoid locked
  targets, dead zones, stale accumulated zoom, reversals, and extreme jumps.
- Use scale-aware increments, not a universal fixed-meter step. Account for FOV
  and proximity while retaining useful fine control at meter scale.

### Rotation semantics

- Replace fixed angular sensitivity with distance/clearance- and FOV-aware policy.
  Distinguish pivot orbit rotation from local camera look; they have different
  geometry and must not share an unexplained universal coefficient.
- Normalize pointer displacement consistently across viewport sizes and DPI.
  FOV changes must not make a gesture disproportionately aggressive in a narrow
  view. Do not add wheel-controlled FOV as a workaround.
- Improve near-surface precision as clearance decreases: a normal mouse swipe
  must not rotate the view several degrees. Do not achieve precision by making
  rotation effectively zero or unusable.
- Use transported local up/tangent directions for surface yaw/pitch. Avoid
  accumulated unintended roll, singular pole handling, and reanchor snaps.
- Sensitivity changes must be smooth, including changes of scale within one mode.
  Preserve continuous experience across explicit mode changes even when the
  mathematical response differs between orbit and flight.

## Surface-navigation movement and terrain clearance

- WASD moves in the local tangent plane; Q/E moves along local up. Looking down
  must not turn ordinary forward movement into an unintended dive.
- Transport the horizon/tangent basis as the observer travels around the body,
  including polar travel and local reanchoring. Keep motion and look predictable.
- Navigate using admitted wall time, not simulation time or time-warp multiplier.
  Both paused and running simulation must support editor movement.
- Through ordinary wheel/movement controls, reach **2 m requested complete-terrain
  clearance** on Earth and Moon, independently of whether displayed terrain LOD
  is settled. A diagnostic Approach button, direct pose assignment, or a test-only
  clearance setter is not a substitute for this route.

Keep these quantities distinct:

1. Reference-sphere altitude: radial distance minus reference radius.
2. Complete-terrain clearance: radial distance minus the complete procedural
   terrain radius at the observer's current radial direction.
3. Drawn-mesh clearance: clearance from the currently published rendered mesh.

Use complete terrain for the requested approach criterion, not an idle queue,
rendered LOD level, or coarse mesh. Do not clamp navigation to coarse displayed
geometry in a way that prevents reaching the complete-terrain target. If the
displayed mesh intersects the observer while terrain is pending, report it
honestly; neither conceal it nor claim settled visual quality.

Where complete terrain is unavailable, use an explicitly labelled reference-sphere
fallback. Never fabricate terrain clearance. Navigation safeguards must describe
their protected quantity and limitations. Radial/sample clearance protection is
not general collision physics or a swept-volume guarantee. Reuse existing queries
where coherent; do not introduce duplicate procedural work merely for UI reporting.

## Speed model and UI reporting

Automatic/base surface speed should derive from meaningful complete-terrain
clearance, with a documented fallback and smooth scale response. Preserve useful
manual fine/coarse control. Apply user and temporary boost multipliers separately;
do not fold them irreversibly into the automatic/base speed.

Show all four components with correct units:

- Automatic/base speed in m/s, with its terrain/fallback source.
- User multiplier, dimensionless and controlled separately from wheel navigation.
- Temporary boost multiplier when active.
- Effective speed in m/s.

For example: `Base 2 m/s × User 0.5 × Boost 4 = 4 m/s`.
Do not label a dimensionless multiplier simply "Flight speed." Report effective
commanded navigation speed separately from simulation-frame velocity; they are
not interchangeable measurements. Surface wheel displacement must not secretly
change any multiplier.

## Input timing, ownership, and focus lifecycle

### Timing contract

- Continuous held-key movement and smoothing must be timestep-independent.
  Integrate over admitted wall time; use time-based rather than per-frame damping.
- Pointer and wheel input are event/delta driven. Equivalent timestamped
  pointer/wheel deltas must produce equivalent camera response at 30/60/144 Hz
  render frequencies.
- **Do not multiply raw pointer or wheel deltas by frame dt.** Consume each delta
  once, without dropping or duplicating it because redraw frequency changed.
- Preserve event chronology when response depends on evolving state. Distinguish
  discrete target updates from time-based smoothing toward those targets.
- Normalize mouse-wheel and high-resolution scrolling units explicitly.

### Ownership and lifecycle contract

- UI clicks, drags, scrolling, text editing, and focused widgets must not navigate
  the scene or accidentally trigger navigation shortcuts.
- A viewport gesture has explicit ownership. Releasing outside the viewport ends
  it; moving over a panel must not start a scene gesture from a panel-owned press.
- Window-focus loss clears active movement/gesture state. Resume must not replay
  stale deltas, stuck keys, or a cancelled navigation target as catch-up motion.
- Minimize/resume and excluded interactive clock gaps produce no navigation jump.
- Resize/DPI changes keep input, content viewport, projection, and picking aligned.

## DeveloperSnapshot and capture additions

Extend the canonical app-owned snapshot only as needed to explain this behavior:

- Explicit mode and attachment policy, distinct from numerical carrier frame.
- Focus-transition state.
- Base speed, user multiplier, boost multiplier, effective speed, and base source.
- Requested clearance/zoom target with its meaning, plus actual complete-terrain
  clearance where available. A forward/recede operation must not be falsely
  serialized as a radial clearance target when it has no such target.
- Active safeguard and its protected clearance quantity.
- Effective response-policy values necessary to diagnose rotation/zoom sensitivity.

Keep existing sphere/terrain/mesh distinctions, body associations, semantic f64
pose frames, null/unavailable values, and prepared-frame PNG/JSON association.
Snapshot collection remains observational: no terrain queries or work submission.
Document schema compatibility/version decisions and update serialization tests and
interface documentation together. Do not create a competing high-level snapshot.

Scripted navigation evidence must use the same controller/input path as native
interaction. Retain paired checkpoint images and snapshots. Static fixture setup
may seed test states, but must not masquerade as a successful ordinary-control route.

## Implementation ownership and allowed files

The app owns input interpretation, navigation policy, and the single observer.
Retain coherent frame evaluation, frame-local f64 state, common-ancestor precision
cancellation, source-local subtraction, and checked renderer narrowing. Do not
change authoritative body state or the global world origin to move the camera.

Primary task paths:

- `crates/app/src/celestial_camera.rs`: modes, response, motion, transitions.
- `crates/app/src/gravity_orbits.rs`: input, command, lifecycle, projection wiring.
- `crates/app/src/gravity_orbits/developer_ui.rs`: focused camera/speed controls.
- `crates/app/tests/celestial_navigation.rs` and focused app camera/input tests.
- Minimal updates to `crates/app/src/developer_snapshot.rs`,
  `crates/app/src/developer_capture.rs`, its example, and
  `crates/app/tests/developer_interface.rs` for navigation evidence.
- New focused app modules/tests if they clarify ownership; minimal app exports
  only as necessary. Avoid new dependencies or broad public abstractions.
- Navigation/interface documentation, a phase report, evidence under
  `docs/evidence/phase512b/`, and a dated `docs/REVIEWER_CONTEXT.md` update.

Keep tightly coupled controller/input changes under one editor. Delegated editing
requires explicit disjoint file ownership, including tests and documentation.
Follow repository subagent/authentication rules; the primary owns shared APIs,
numerical reasoning, integration, and verification. Do not rewrite neighboring
dirty terrain or renderer work. If implementation requires production changes
outside this ownership, explain the prerequisite and obtain approval first.

## Explicitly out of scope

- Terrain morphology or procedural generator changes.
- LOD/refinement redesign, LOD visualization/debugging, or convergence repair.
- GPU terrain residency, generation, or morph interpolation.
- Acceptance A work or changes to its criteria.
- Walking-character controllers, gravity, or general collision physics.
- Broad UI redesign or coordinate-system rewrite.
- Terrain memory-cap, error/quality-threshold, or acceptance-threshold changes.
- Unrelated refactors, speculative optimization, and automatic follow-on phases.

Report out-of-scope blockers rather than silently expanding the task or changing
quality/cap settings to obtain a pass.

## Quantitative tests and acceptance criteria

Define concrete numerical assertions before implementation and retain their
outputs, not just pass counts. Engineering tolerances below are validation
guardrails, not a replacement for the user's navigation-feel approval. Document
any necessary adjustment with evidence and obtain approval before relaxing a gate.

1. **Ordinary-control route:** overview → Earth orbit → explicit Surface Navigation
   → 2 m requested complete-terrain clearance → Moon → overview. After approach
   inputs stop and smoothing completes, measured complete-terrain clearance must
   be within 0.05 m of 2 m on both bodies. Record the requested target/route,
   actual clearance, terrain state, and elapsed navigation time. Cover wheel
   approach and tangent/vertical movement through production controls. Pending
   displayed LOD does not invalidate this clearance criterion or prove visual
   terrain acceptance.
2. **Entry without approach:** enter Surface Navigation at 100 km, 10 km, and 2 m
   complete-terrain clearance. Confirm no positional approach or orientation reset
   on entry; then continue navigating. Include an incoming rolled view and check
   that stabilization is smooth rather than an entry snap.
3. **Pose preservation:** measure mode-entry/exit and interrupted-transition
   residuals in an appropriate common/local frame. For same-body near-surface
   re-expression, require position residual ≤1 mm and orientation residual
   ≤1e-6 rad. Use explicit justified scale-aware tolerances for astronomical
   inter-frame conversions; root-coordinate round trips are not local proof.
4. **Sensitivity matrix:** include system overview, body orbit, and 2/10/100 m,
   10 km, and 100 km complete-terrain clearance; FOVs 30°/60°/90°; multiple content
   viewport sizes and DPI scales. Report angle per normalized swipe and distance/
   clearance change per wheel input. As a near-surface precision guardrail, a
   200-logical-pixel horizontal swipe at 60° vertical FOV in a 1080-logical-pixel
   high viewport, at 2 m and 10 m clearance, must produce a nonzero rotation below
   1°. Check both local look and body-orbit behavior where valid. Also verify
   practical full-view turning through repeated gestures/native use; a tiny
   coefficient alone is not sufficient acceptance.
5. **Wheel continuity:** test inward/outward wheel input before and after explicit
   body-orbit/surface transitions and while traversing scale thresholds within a
   mode. FOV and user multiplier remain unchanged; mode changes only by explicit
   command. Verify direction, finite response, no locking/dead zone, and no stale
   target replay. Include fractional/high-resolution wheel deltas.
6. **Timing equivalence:** replay identical timestamped pointer/wheel events and
   held-key intervals at 30/60/144 Hz. Compare target changes and smoothed endpoints
   at the same admitted wall time. For isolated pointer/target-update fixtures,
   require angular discrepancy ≤1e-6 rad and relative target discrepancy ≤1e-6
   with a documented near-zero absolute tolerance. For mixed continuous-motion
   trajectories, require endpoint displacement and zoom discrepancy ≤1% of the
   fixture's intended travel/change, with a 1 mm near-zero absolute tolerance;
   record orientation discrepancy and its declared fixture-specific bound.
   Include frame partitioning, no-input smoothing, and zero-admitted-time delta
   handling. Tests must detect accidental dt multiplication of raw deltas.
7. **Surface basis/attachment:** test tangent WASD with downward look, local-up Q/E,
   pole crossing, regional reanchoring, paused simulation, running body rotation,
   and different time-warp settings. Check unintended roll and pose continuity
   against declared numerical bounds. Separately verify advanced free flight's
   system-stationary policy despite carrier changes.
8. **Input lifecycle:** cover panel-owned interactions, text focus, shortcuts,
   outside-viewport release, window-focus loss, resize/DPI, and minimize/resume.
   Assert no unintended navigation or catch-up motion after input is cancelled.
9. **Speed/snapshot correctness:** verify base × user × boost decomposition,
   labelled fallbacks, wheel invariance of multiplier/FOV, null semantics,
   serialization, camera/body associations, and PNG/JSON prepared-state pairing.
10. **Human acceptance:** the user reviews native control feel. Passing automated
    tests or scripted native routes does not close this gate. Mark it open until
    explicitly approved.

If a sensitivity/timing criterion cannot be achieved within scope, report the
specific failed fixture and reason; do not weaken it silently. Existing tests
that encode superseded control semantics should be updated with explicit rationale,
not deleted to hide regressions.

## Native UX validation

Exercise real mouse/keyboard interaction on Windows through overview → Earth →
surface → Moon → overview. Include entry at high altitude, meter-scale wheel
approach, fine look, tangent movement while looking down, speed adjustment/boost,
interrupted focus, recovery, panel scrolling, focus loss, and minimize/resume.
Record build/profile, hardware/backend, viewport/DPI, FOV, mode, clearance, and
terrain readiness. Retain native observations separately from offscreen captures.

Assess both precise near-surface control and practical astronomical travel. Do
not claim native input correctness solely from headless tests, human approval
from screenshots, or settled terrain from `quality_pending=true` evidence.

## Validation budget and evidence requirements

- Iterate with focused locked app camera/navigation, input, and snapshot/interface
  tests. Use test filters and avoid repeated full-workspace matrices during tuning.
- Follow the repository evidence loop: focused change → `./scripts/ai-check.ps1`
  → inspect its paired PNG/JSON → focused tests. Use fresh output directories.
  The fast check is not the final quality suite or navigation-feel proof.
- Preserve one final sensitivity/navigation matrix and native route, plus targeted
  reruns for discovered boundary failures. Add checkpoint capture support only
  where necessary; do not redesign the developer tooling.
- Measure comparable before/after controller/input CPU work and incremental terrain
  query counts/costs. Separate them from terrain preparation, UI, GPU, readback,
  encoding, and file I/O. Record units, sample counts, fixtures, revision/dirty state,
  profile, and raw results. No unsupported speedup or zero-cost claim.
- Before completion claims, run `./scripts/validate.ps1 -IncludeGpu` once on the
  final candidate. Rerun affected validation after later changes. Retain failures,
  environmental blockers, and successful retries. Include `git diff --check`.
- Retain exact commands, logs, test outputs, response equations/constants, numeric
  matrices, paired captures/snapshots, source fingerprints, Git state, and a concise
  evidence index. Preserve earlier phase evidence; do not overwrite packages.

Terrain convergence, displayed-mesh issues, and Acceptance A remain separately
classified. Neither hide them nor treat this phase as their acceptance.

## Final handoff requirements

Use the headings in `docs/REVIEW_HANDOFF.md`: Goal, Result, Architecture changes,
Files changed, Tests, Measurements, Captures, Known failures, Evidence, Git state,
and Reviewer follow-up.

Report PASS/PARTIAL/FAIL against each named criterion. Distinguish IMPLEMENTED,
MEASURED, VERIFIED, OBSERVED, INFERRED, UNTESTED, BLOCKED, and FAILED where material.
Separate task edits from pre-existing work and implementation from validation.
Record schema decisions, response policy, attachment/interrupt semantics, exact
commands/platform, evidence paths, and every unresolved gate. Human navigation
approval and Phase 5.12A UI approval remain open unless the user accepts them.

Refresh the reviewer context with dated source/evidence references, without
claiming terrain acceptance or erased blockers. Leave changes uncommitted and
unpushed. Stop after the handoff; do not start another engine phase automatically.
