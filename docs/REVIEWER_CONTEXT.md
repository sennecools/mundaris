# Mundaris reviewer context

Read this first after `AGENTS.md`, then check Git status and drill into references
only as needed. This is an orientation index, not a substitute for current source,
runtime evidence, or the user's direction. **Last checked: 2026-10-05.**

## Mundaris vision

Mundaris is a native desktop world-authoring and planetary simulation platform:
editable procedural worlds, physically coherent celestial motion, and continuous
navigation from astronomical views toward human-scale surfaces. The
[engine design](../MUNDARIS_ENGINE_DESIGN.md#1-purpose) is the long-term north star,
not a claim that every envisioned feature exists. The user owns product direction.

## Current development priorities

These are review priorities from the checkpoint below, subject to user decisions,
not authorization to begin another phase:

1. Establish trustworthy reviewer/implementation handoffs and directly inspectable
   runtime/capture evidence; review the in-progress developer/AI tooling separately.
2. Resolve Acceptance A with useful visible convergence and measured CPU preparation,
   upload/reuse, and memory headroom evidence; compare architectures before editing.
3. Assess visual quality, camera/navigation, and developer UI in narrow separate
   reviews, including human native interaction rather than only offscreen tests.
4. Revisit morphology only after the existing responsiveness prerequisite is met or
   the user explicitly approves a changed sequence.

## Current unresolved blockers

**2026-10-04 checkpoint:** [Phase 5.11E report](PHASE_5_11E_TERRAIN_RECOVERY_REPORT.md)
and [evidence index](evidence/phase511e/README.md) describe an uncommitted partial
recovery. This checkout also contains newer dirty developer-tooling work, so those
measurements are checkpoint evidence, not automatically measurements of today's
entire working tree. Recheck source/fingerprints before claiming a current result.

| Area | Dated checkpoint / limitation |
| --- | --- |
| Acceptance A | Not passed in 5.11E. Cold rendered radial LOD 5/8/17 at 1/2/5 s versus desired 23 is supporting evidence, not whole-view useful quality. See report “Cold quality” and “Outstanding acceptance requirements”. |
| CPU terrain preparation | Reported stationary planetary preparation median 13.939 ms over 20 repeats; CPU conversion and morph evaluation/clipping remain. See “Uploads and memory”; not native FPS or evidence of a repaired bottleneck. |
| GPU residency | 5.11E reports no keyed terrain residency, dirty unchanged uploads, or GPU morph interpolation. Persistent buffer capacity is not resident geometry reuse. See “Partial implementation” and “Measurements and validation”. |
| Terrain headroom | Reported descents peak at the unchanged 128 MiB accounted cap with zero headroom. Allocation sharing does not prove unique-allocation accounting or operational margin; RSS/driver VRAM unmeasured. |
| Visual quality | Whole-view quality and shell visual acceptance are not established. Before/after admitted covers differ; unsettled images are not settled terrain proof. |
| Camera/navigation | Scripted native route passed its exercised paths on RX 9070 XT/Vulkan; human control feel and desired navigation UX remain unaccepted. |
| Developer UI / AI observability | Dirty source includes `developer_snapshot.rs`, UI snapshot use in `gravity_orbits.rs`, developer capture modules/example, and `scripts/ai-check.ps1`. These are in-progress inputs, not a validated/accepted interface here; inspect current paths, rendered-state coupling, tests, and paired evidence. |
| Morphology | 5.11E makes no generator/morphology change. Responsiveness-before-morphology sequencing remains in the Phase 5.10 acceptance record. |
| Hitch / platform evidence | Historical 114.533 ms event lacks established reproduction/root cause. Non-reproduction is not a fix. No 5.11E Linux, remote-CI, or universal adapter acceptance is claimed. |

This workflow setup does not validate the engine or advance these gates. Refresh
this dated index after an accepted handoff changes the evidence; retain references
and distinguish IMPLEMENTED, MEASURED, VERIFIED, and still-open claims.

## Architectural principles

- Authoritative world state stays upstream; grids, covers, meshes, transitions, and
  GPU resources are rebuildable derived representations. Rendering does not rewrite truth.
- Precision first: retain meaningful frame-local f64 state and narrow only at checked
  observer-relative rendering boundaries; do not flatten large coordinates first.
- Deterministic terrain where required, with explicit stable inputs; preserve seams,
  conservative error/precision proofs, transactional publication, and safe cancellation.
- Use GPUs for suitable massively parallel/render work and bounded CPU workers for
  suitable irregular work. Ownership follows correctness and measurement, not slogans.
- Minimize repeated work and data movement; no fake wins from hidden work, lower
  quality, changed caps/thresholds, or incomparable fixtures.
- Diagnostics must describe actual rendered state and identify stale/asynchronous
  measurements. Ready, rendered, desired, settled, and quality-pending differ.

## Review philosophy

Discuss first; keep implementation phases narrow. Prefer implementation truth over
planning documents, evidence before optimization, and honest partial results over
unsupported completion claims. Human experience matters: visual correctness requires
visual evidence; performance requires measurements; native UX requires interaction.
Apply the hierarchy/claim vocabulary in `AGENTS.md`, [handoff](REVIEW_HANDOFF.md), and
only relevant portions of [the checklist](REVIEW_CHECKLIST.md).

## Important reference documents

- [ENGINE_MECHANICS_REFERENCE.md](ENGINE_MECHANICS_REFERENCE.md): section index for
  ownership/frame flow (§1), LOD (§4–5), generation (§6–7), reuse/preparation/GPU flow
  (§8–10), camera (§14), memory/workers (§16–17), diagnostics (§18), evidence (§19).
  Its source snapshot and 5.11D continuation predate parts of the dirty 5.11E/tooling
  work; historical line ranges and “latest” statements must be rechecked.
- Latest located checkpoint: [5.11E recovery](PHASE_5_11E_TERRAIN_RECOVERY_REPORT.md)
  and [evidence](evidence/phase511e/README.md); predecessor:
  [5.11D pipeline](PHASE_5_11D_TERRAIN_PIPELINE_REPORT.md).
- Acceptance A sequencing: [Phase 5.10 §1](../MUNDARIS_PHASE_5_10_LOD_CONVERGENCE_AND_MORPHOLOGY_RECOVERY.md#1-acceptance-and-sequencing);
  recorded useful-quality gate: [5.10B §17](phase-5-10b-acceptance-a.md#17-acceptance-a-and-retained-captures);
  latest located requirement verdict: [5.11E outstanding requirements](PHASE_5_11E_TERRAIN_RECOVERY_REPORT.md#outstanding-acceptance-requirements).
  Reports record outcomes; they do not authorize silently redefining the user's gate.
- [Architecture](architecture.md), [invariants](engine-invariants.md),
  [coding standards](coding-standards.md), [precision ADR 0002](adr/0002-reference-frames-and-precision.md),
  [surface ownership ADR 0006](adr/0006-planet-surface-topology-and-lod.md).
  Some milestone inventories are historical; source wins over stale “not implemented” text.
- AI/runtime inspection: [mechanics diagnostics](ENGINE_MECHANICS_REFERENCE.md#18-diagnostics-controls-and-what-they-actually-diagnose),
  in-progress [snapshot source](../crates/app/src/developer_snapshot.rs),
  [capture source](../crates/app/src/developer_capture.rs), and
  [check script](../scripts/ai-check.ps1). No dedicated accepted AI-interface document
  was located at this check; add its link when one exists. Do not run these checks
  merely to answer a discussion question or assume unfinished tooling passes.
- [OpenCode role selection and permissions](opencode-workflow.md#reviewer--plan-mode).

## External rendering research

[Modern Rendering Optimization Reference](references/MODERN_RENDERING_OPTIMIZATION.md)
extracts timestamped external recommendations, current-source applicability and
candidate profiling questions. It is an idea/research source, lower authority than
current Mundaris source, current measurements, and current tests/runtime/capture
evidence. Consult it when renderer/performance architecture is relevant; it is not
required reading for every reviewer session and does not authorize implementation.

## Phase 5.12A interface handoff — 2026-10-04

The newer [developer-interface guide](AI_DEVELOPMENT_INTERFACE.md) and
[implementation report](PHASE_5_12A_DEVELOPER_INTERFACE_REPORT.md) now index the
canonical snapshot, named PNG/JSON fixtures, fast check and separate full quality
command. [Current evidence](evidence/phase512a/README.md) includes native UI
observations and recorded quality checks/retries. This updates the in-progress
interface pointers above, not the 5.11E terrain/performance blockers or Acceptance A.
The user/reviewer still evaluates visual/UX acceptance from the underlying evidence.

## Phase 5.12B camera handoff — 2026-10-04

The uncommitted [camera/navigation handoff](PHASE_5_12B_CAMERA_NAVIGATION_REPORT.md)
and [evidence index](evidence/phase512b/README.md) are **PARTIAL**, not accepted UX.
IMPLEMENTED: projection/clearance-aware controls, pose-preserving Surface Navigation,
transported heading/pitch, tangent/local-up movement, view-forward wheel destinations,
native ownership/cancellation and canonical schema 2 navigation diagnostics. Review
[current controls](CAMERA_NAVIGATION.md) and [interface semantics](AI_DEVELOPMENT_INTERFACE.md).

MEASURED: final controller captures reach complete-terrain 2.000001389 m on both
Earth and Moon. Both near captures are dark/uniform and `quality_pending=true`;
this does not establish visible surface quality. Final sensitivity/timing fixtures
pass their numerical bounds, but ~180° at 2 m takes 366 repeated 200-pixel swipes;
practical turning and human control feel remain open. Tangent controller microprofile
adds complete queries and rises from 0.1193 to 148.8960 µs/call under the documented
fixtures; not a speedup, terrain-preparation measurement or native FPS result.

FAILED: the retained synthetic native route does not complete. Final run 5's snapshot
reports unfocused/no viewport keyboard ownership before Earth focus. This is not proof
of a physical-device regression; physical interaction, native lifecycle sequence and
human approval are UNTESTED. A late terrain-orbit cancellation bug was independently
reproduced (273 m unintended motion) and fixed with a retained regression (≤2.911e-11 m
motion). The first full quality matrix passed before that fix; final-source affected
checks also pass (56 debug and 56 release app tests plus eight quality commands),
with fresh paired route/profile evidence. Follow the report's per-criterion limitations and final
quality-command evidence. These newer dirty-source measurements supersede no terrain/
Acceptance A blockers, and do not close Phase 5.12A human UI approval. Discuss reviewer
reproduction and user priorities before authorizing any follow-on work.

## Phase 5.13A analytic foundation handoff — 2026-10-05

User-approved [specification](../MUNDARIS_PHASE_5_13A_ANALYTIC_CELESTIAL_MOTION.md),
[ADR 0007](adr/0007-prescribed-celestial-motion.md),
[handoff](PHASE_5_13A_ANALYTIC_MOTION_REPORT.md) and
[raw evidence](evidence/phase513a/README.md) describe the uncommitted foundation.
IMPLEMENTED: immutable world-domain complete orbital/spin definitions, namespaced
hierarchy validation and a concrete simulation producer that samples direct times
with bounded solving/reusable candidates and transactional publication. Periods are
explicit inputs independent of body properties; satellite translation never inherits
parent spin. Existing Newtonian integration and ordinary launch are unchanged.

VERIFIED against local focused Windows debug/release evidence: independent numerical
oracles, derivatives, tilted/stationary/retrograde spin, reordered hierarchy,
history-independent samples, +/-1000-year direct seeks, failure rollback, independent
systems and sampled shared-ancestor millimetre attachments. This is not arbitrary
precision or cross-platform bitwise determinism. MEASURED: 1000-body complete sample+
commit Criterion slope ~156–157 microseconds for the retained near/distant fixtures
on Ryzen 7 9800X3D; not native FPS or equal N-body fidelity. VERIFIED: all 11 final
Windows quality commands pass against the fingerprinted source, including both
Clippy configurations, workspace debug/release tests, long orbits and explicit GPU
regressions. The report records scoped PASS and limitations; Linux/remote CI remain
UNTESTED and reviewer acceptance remains separate.

The fast paired capture remains `quality_pending=true`, `settled=false`; no terrain,
camera/native human UX or visual gate is closed. Suggested subsequent sequence is
default producer/time-control integration, visible-universe distant content (clearly
distinguishing decorative/addressable systems), then addressing/bounded streaming.
This roadmap is not authorization to implement those phases automatically.

## Phase 5.13B default playback handoff — 2026-10-05

The uncommitted [playback handoff](PHASE_5_13B_ANALYTIC_PLAYBACK_REPORT.md),
[controls/content policy](ANALYTIC_PLAYBACK.md) and
[evidence](evidence/phase513b/README.md) integrate analytic motion into ordinary
gameplay/real-scale solar presets. IMPLEMENTED: concrete app-owned motion selection,
signed fractional direct-time controls, checked property/name rebinding, coherent
publication/draw gating, authored guides, publication trails and schema 3 diagnostics.
Newtonian hierarchy/circular integration and replay remain explicit and separate.

User approved stationary Sun/planet-about-Sun/Moon-about-Earth circles, frozen original
initialization pacing and removal of Earth's inner wobble (Earth/Moon shift together
~293 km gameplay /4,665 km real-scale at epoch). This is authored approximate motion,
not an ephemeris or conserved barycentric N-body system.

VERIFIED: focused debug/release controls/failure/return tests, supported-edit periods,
guides/trails, millimetre Earth/Moon attachments at ±1000 years; all 11 Windows quality
commands pass across the initial full matrix plus four affected reruns. Original
release/GPU commands were build-blocked by a concurrently running executable; preserved
logs and final-source fingerprints distinguish that from failed test assertions.
MEASURED: six ten-body fixtures ×30 repeats; median complete sample/commit 0.5–0.7 µs,
frame publication 0.1–0.2 µs, app update 1.3–1.6 µs on Ryzen 7 9800X3D, release.
Not native FPS, terrain work or equal-fidelity Newtonian speedup.

Overall **PARTIAL**: native complete-route validation has not passed. A snapshot reader
sharing crash was corrected; the next attempt queued all stages but observed no
checkpoints because the sequential observer waited for a missed first focus criterion.
Final observer retained 6/8 checkpoints and app exit 0, but completed Earth/Moon fits
0/5 are missing. All observed snapshots are unfocused/far from bodies, with no active
terrain; inspected screenshots show a foreground terminal occluding most of the app.
This is limited playback/seek/reset evidence, not a passed native route or useful
surface view. Final verification matches all 189 fast source fingerprints and
`git diff --check` passes. Physical input/human visual/UX approval remain UNTESTED.
All offscreen captures remain quality-pending/unsettled; previous Acceptance A,
terrain/camera/UI blockers are unchanged. Review actual evidence and discuss user
priorities; this handoff does not authorize Phase 5.13C or unrelated optimization.

## Phase 5.13C prerequisite checkpoint — 2026-10-05

The user approved cinematic procedural decorative sky scope with a mandatory native
route prerequisite. The [5.13C handoff](PHASE_5_13C_DISTANT_SKY_REPORT.md) and
[new evidence](evidence/phase513c/README.md) are **PARTIAL / BLOCKED before sky
implementation**. VERIFIED: all 189 starting source fingerprints match 5.13B;
production source and historical evidence remain unchanged. A new phase-local
observer retains all route criteria and requires actual foreground/native focus.

OBSERVED: 6/8 focused foreground checkpoints (2/3/4/5/6/7), including completed Moon
fit at 281,757.9338625927 m and subsequent body-fixed seek/reset. Earth fit and forward
playback 0/1 remain missing; native foreground/focus was only observed after those
stages. App exit 0, empty stderr; observer exit 1. All six inspected images are
unobstructed, but PNG/JSON are observational, not guaranteed same-frame. Moon terrain
is quality-pending/unsettled. No engine defect is established. Operator-assisted
unchanged-route verification is required; see the report's fresh-run command.
Sky architecture, precision, visual checkpoint, performance and acceptance remain
UNTESTED. Earlier terrain/camera/UI gates are unchanged; no universe/streaming work.

## Phase 5.13C first-look checkpoint — 2026-10-05

The earlier prerequisite blocker above is superseded by
[native-ready run 03](evidence/phase513c/native-ready-20261005-03/summary.json):
**8/8 focused foreground checkpoints**, app/observer exit 0, all eight unobstructed
images inspected. After late foreground arrival in another failed retry, the user
approved an opt-in validation-only startup readiness gate; original route commands,
thresholds, all eight criteria and later focus-cancellation safety remain unchanged.
The passing binary predates sky implementation; this is not current-sky native UX.

IMPLEMENTED: app-owned versioned seeded decorative finite-star preset; renderer-owned
directional galactic/dust texture, bounded one-definition residency, finite parallax,
Gaussian stars and restrained halos, with schema-4 sky diagnostics and controls.
Declared range **1e18–2e19 m**, observer envelope **1e14 m**; unsupported observers get
explicit sky-unavailable state, not a camera clamp. No world/simulation/terrain or
dependency changes belong to 5.13C. Daytime leakage was reproduced with atmosphere
actually drawn, then corrected only at zero-depth near-surface illuminated pixels.

The [handoff](PHASE_5_13C_DISTANT_SKY_REPORT.md) and
[evidence index](evidence/phase513c/README.md) are **PARTIAL at user visual review**.
MEASURED: adapter-required rendered centroid maximum **0.001645105 physical px** in
the 960×640/60° origin/1 AU/9e13 m fixture; day sky-on/off maximum **1/255**, night
**193/255**, with matched foreground snapshots. VERIFIED: focused debug/release
tests, exercised cache/toggle/occlusion/invalid-state rejection, latest fast check,
and affected-crate Clippy retry. These are not a full acceptance matrix or p95 profile.

Review [toward](evidence/phase513c/first-look-20261005-07/sky-toward.png),
[along](evidence/phase513c/first-look-20261005-07/sky-along.png),
[away](evidence/phase513c/first-look-20261005-07/sky-away.png) and their paired JSON.
They are low-level gameplay sky fixtures with no admitted terrain; the separate
fast Earth capture remains quality-pending/unsettled. User visual approval, warm raw
profiling, real-scale/1440p/4K matrix, fuller attached/native checks and full validation
remain UNTESTED. Pause for discussion; do not start final polish or another phase,
and do not close terrain/camera/UI gates from this checkpoint.

## Phase 5.13C visual revision — 2026-10-05

The user **rejected** the first look as bland relative to the desired cinematic
references, then explicitly approved fixing it. Version 2 is IMPLEMENTED: 48,000
finite stars with clustered disk/color/brightness hierarchy; a 4096×2048 cached
background with authored luminous complexes, fine directional turbulence and
contrasting variable-width dust. Only `sky_definition.rs`, `sky_background.rs` and
renderer admission bounds in `sky.rs` changed relative to the visual baseline.
No camera/foreground/atmosphere/dependency/world/simulation rewrite occurred.

Inspect the [unaltered before/after](evidence/phase513c/visual-comparison-20261005-10/before-after.png)
and [fresh version-2 pairs](evidence/phase513c/visual-recipe-20261005-10/).
The [current handoff](PHASE_5_13C_DISTANT_SKY_REPORT.md) remains **PARTIAL at renewed
user visual review**. VERIFIED: focused debug/release recipe and regression tests,
explicit GPU centroid/cache/depth/day-night checks, fast capture, affected Clippy,
formatting and release app build pass. Named GPU centroid maximum stays
0.001645105 physical px; day on/off stays 1/255, night is 239/255. These are narrow
fixtures, not arbitrary GPU precision, native controls or cinematic acceptance.

MEASURED single cold fast capture: representation generation ~989.6462 ms,
upload API ~5.1842 ms, owned resident GPU payload 46,836,508 B (~44.7 MiB), CPU
definition payload 2,304,152 B. More detail increases static cost; no warm target,
speedup, RSS/driver allocation or full quality validation is claimed. User approval,
warm raw profiling and the remaining scale/native/full matrix stay open. The fast
Earth terrain is still quality-pending/unsettled; previous acceptance gates remain.

## Phase 5.13C-R reconstruction checkpoint — 2026-10-05

The user rejected version 2 as blurry/repetitive and supplied the four references:
Mundaris baseline, Space Engineers 2 structured dust, Space Engine fine stars and
Space Engine isolated coloured nebulae. The
[new handoff](PHASE_5_13C_R_SKY_RECONSTRUCTION_REPORT.md) and
[evidence](evidence/phase513cr/README.md) remain **PARTIAL**, not visual acceptance.

IMPLEMENTED: app-owned four connected cloud/dust graphs with cavities; renderer
2048×1024 diffuse base plus four filtered 1024-pixel focal charts; 131,072 finite
stars with distinct foreground/background extinction and pixel-integrated cores.
Range/envelope and world/simulation/terrain/camera/atmosphere ownership are unchanged.
VERIFIED: 156 matched offscreen 1440p/4K pairs, exact return/initial-motion images,
static reuse, focused debug/release checks, explicit sky/day-night GPU checks,
affected Clippy/formatting, release app build and final fast check. Maximum named
centroid error is 0.002139788 physical px; new cached-mip seam/pole error is
0.001413084 linear channel; day/night maximum on/off is 1/255 and 240/255.

MEASURED single 1440p cold capture: generation 1047.5287 → 2300.9269 ms; owned GPU
payload 44.67 → 36.00 MiB. Cold work regresses; no warm speedup/target acceptance is
claimed. Separately calculated generation-payload bound is 51.00 MiB, not peak RSS.
Inspect [unscaled viewer](evidence/phase513cr/review.html) and original PNG/JSON.
BLOCKED: three baseline native attempts fail foreground ownership; full native 4K
exceeds the connected 5120×1440 display. Current-sky native route, user art/motion
approval, raw warm profiles, first-visible latency and full quality matrix remain
open. Do not treat offscreen pixels as native evidence or close prior terrain/UI/
camera gates. Stop for user review before further profiling or another phase.

## Off-band sky composition revision — 2026-10-05

The user approved breaking up the galactic backbone, moving structures outside it,
adding off-band clusters and preserving dark directions. The
[composition handoff](PHASE_5_13C_R_SKY_COMPOSITION_REPORT.md) and
[viewer](evidence/phase513cr/composition-20261005/review.html) are the current
version-4 candidate, superseding version 3 without asserting visual acceptance.

IMPLEMENTED: six app-authored compact diffuse disk regions; three nebulae at
+25.2°/−28.6°/+18.3° latitude; 60% disk / 10% local clusters / 30% isotropic star
authoring. Exact catalogue counts are 82,084 within ±10° and 41,933 outside ±15°.
GPU representations, star count, finite ranges/envelope and upload/draw ownership
are unchanged. VERIFIED: 166 matched offscreen settings/dimension pairs, static
reuse, exact return/motion-zero images, focused debug/release checks and explicit
GPU tests, Clippy/formatting, release app build and final fast check. MEASURED
single 1440p cold capture: generation 2227.7919 → 2457.6315 ms; GPU payload remains
37,749,052 B. No warm performance win is claimed. Existing native blockers, user
art/motion approval, raw warm profiles and full quality matrix remain open; stop
for visual review. Final fast Earth terrain remains quality-pending/unsettled.

## Sky visual acceptance and commit request — 2026-10-05

The user accepted version 4 as "good enough for now" and requested a commit and
completion marking. **DONE FOR NOW** applies to the visual/composition iteration;
the [handoff](PHASE_5_13C_R_SKY_COMPOSITION_REPORT.md) and
[acceptance record](evidence/phase513cr/composition-20261005/acceptance.json) retain
native/performance/precision/latency gates as outstanding, not passed. Do not
automatically start another phase. The user explicitly confirmed **whole current
checkpoint** commit scope, including terrain recovery, developer interface/camera,
analytic motion and sky integration with their retained documentation/evidence.
No push is authorized. Historical reports keep their original revision/limitations.

VERIFIED before the requested checkpoint commit: the
[final full Windows matrix](evidence/phase513cr/composition-20261005/full-validation-final/validation.json)
passes all 11 locked commands, and
[separate post-approval sky GPU checks](evidence/phase513cr/composition-20261005/post-approval-sky-gpu/commands.json)
pass both commands. The first full attempt timed out at 120 s, not a failed test;
the retained successful retry removes that limit (debug suite 432.89 s). Native
interaction, warm profiles and Linux/remote CI are not established by these checks.
