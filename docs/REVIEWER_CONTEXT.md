# Mundaris reviewer context

Read this first after `AGENTS.md`, then check Git status and drill into references
only as needed. This is an orientation index, not a substitute for current source,
runtime evidence, or the user's direction. **Last checked: 2026-10-06.**

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
4. The user approved a procedural crater terrain foundation on 2026-10-05,
   changing the morphology sequence. Review its shape and unresolved LOD evidence
   separately; see the dated Phase 5.15 note below.
5. The approved Slice 1B task now establishes compositional body surfaces and
   three procedural families. Review its fixed-seed orbital and local diagnostics
   with the user before any Slice 2 work; the dated checkpoint below supersedes
   the preceding Slice 1 authority limitations only where explicitly described.
6. The approved Slice 1B.1 successor adds geological directors and province/process
   authority. Review its final grey geometry and retained unbiased near crops:
   the inspected near portion of Gate E remains FAILED, and within-body volcanic
   separation remains weak. See the dated handoff below; no Slice 2 is authorized.
7. Slice 1B.2 adds hierarchical geometric successors down to metre-scale process
   structure. Review its same-anchor approaches and unbiased grey views using
   the [dated report](PLANET_TERRAIN_SLICE_1B_2_REPORT.md). Numerical success does
   not accept its still-soft fine appearance; no Slice 2 implementation is authorized.
8. The separately authorized native Moon integration now selects RockyV5 in both
   Solar System presets. Review the current paired fixture and certificate limitations
   in the [dated handoff](NATIVE_MOON_INTEGRATION_REPORT.md); this does not itself
   accept visual quality or authorize another terrain phase.
9. The user explicitly authorized Slice 2A and gated continuation into fixed Slice
   2B on 2026-10-06. The [Slice 2A checkpoint](PLANET_TERRAIN_SLICE_2A_REPORT.md)
   passes its resident-tile architecture gates with preserved source attribution,
   native/offscreen captures, historical bitwise replay, and the repaired full
   quality matrix. The [fixed parent/four-child checkpoint](PLANET_TERRAIN_SLICE_2B_REPORT.md)
   now passes B1–B12 with actual GPU seam/endpoint measurements, independently
   ready children, native delay/reversal/stale-result proof, preserved authority,
   and all 13 final quality commands. Work stopped after Slice 2B. Earlier
   statements withholding Slice 2 authorization are dated context; they do not
   override this request. Whole-body LOD, streaming, and Slice 2C remain outside
   the authorization. Existing visual/UX and global-quality blockers remain open.
10. The user separately authorized Slice 2C on 2026-10-06. Its adaptive regional
    desired/resident/drawable scheduler, bounded caches and local mixed-level
    transitions are implemented in the dirty tree. The [dated Slice 2C report](PLANET_TERRAIN_SLICE_2C_REPORT.md)
    records a PARTIAL result: numerical GPU checks and final cold/warm pressure
    routes pass, while C15 fails on native CPU publication/advance hitches (maximum
    measured wall interval 336.30 ms on the identified delay-route binary). C4 and
    native original-Moon verification remain partial. The recommendation is to
    hold in Slice 2C for contained publication/scheduling work. This later request supersedes the preceding
    statement withholding Slice 2C authorization. Whole-planet streaming still
    requires a new user decision; existing art, camera UX and Acceptance A
    limitations are not waived by regional numerical success.
11. The user authorized Slice 2D planetary-runtime integration on 2026-10-06.
    Current dirty source connects the six-face regional resident runtime to the
    ordinary `--solar-system` and `--real-solar-system` presets, with
    `--legacy-terrain` retained for comparison. This is IMPLEMENTED architecture;
    native acceptance is PENDING the [dated Slice 2D report](PLANET_TERRAIN_SLICE_2D_REPORT.md)
    and reviewer assessment. The uncertified projected-relief/sagitta proxy can
    remain `quality_pending`; implementation does not establish visual quality,
    native UX, or performance acceptance. Slice 3A is not authorized.

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
| Developer UI / AI observability | Phase 5.14 adds a live loopback developer protocol, CLI/MCP bridge, native paired capture, bounded diagnostics, reusable scenarios, and owned rebuild/replay. The interface is IMPLEMENTED and has current-phase native/MCP/scenario evidence; Windows cancellation, capture, scenarios and fresh-Codex operation have been demonstrated. The final Windows matrix passes; Linux native and adapter-fault gates remain open. See the dated Phase 5.14 note and its report. |
| Native Moon surface | Both presets select RockyV5 through the existing tile path. The latest demand selector prioritizes projected demand at the nearest point of each patch ball, and the same demand drives stale/coarsening checks and actual-selector prefetch. The user rejected the latest screenshots for center detail, coarse edges and slow retained cover; visual acceptance is FAILED for those images. The global certificate remains nonconvergent, and final performance/validation evidence is pending. See the [2026-10-06 report](NATIVE_MOON_INTEGRATION_REPORT.md). |
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
  [developer interface guide](AI_DEVELOPMENT_INTERFACE.md),
  [snapshot source](../crates/app/src/developer_snapshot.rs),
  [native capture source](../crates/app/src/developer_capture.rs),
  [developer service](../crates/app/src/developer_service.rs),
  [scenario/process runner](../crates/app/src/developer_scenarios.rs), and
  [fast check](../scripts/ai-check.ps1). The guide documents implementation and
  commands; it does not establish user acceptance or close the terrain/camera gates.
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

## Phase 5.14 AI engine development interface — 2026-10-05

The current dirty source implements an app-owned loopback interface for AI-assisted
inspection and bounded control. IMPLEMENTED: session-bound opaque body handles,
typed actions, exclusive expiring control leases, freshness-tagged snapshots,
bounded terrain/performance/error/event diagnostics, a stdio MCP adapter and CLI,
native paired evidence capture, parameterized scenario files, deterministic fixed-step
offscreen execution, native wall-paced replay, and owned stop/build/copy/launch/replay
with semantic checkpoint comparison. See the [interface guide](AI_DEVELOPMENT_INTERFACE.md)
and current [protocol](../crates/app/src/developer_protocol.rs),
[service](../crates/app/src/developer_service.rs),
[CLI/MCP bridge](../crates/app/src/developer_bridge.rs), and
[scenario/process runner](../crates/app/src/developer_scenarios.rs).

VERIFIED: the complete 13-check Windows matrix passed in
`target/phase514/final-validation/validation.json`, including debug/release workspace
checks and four explicitly selected GPU test targets. A subsequent test-only
ambiguity regression passed six bridge tests on Windows and Linux; the selected
format/Clippy/release test rerun is `final-ambiguity-retry/validation.json`.
`final-ai-check-retry` passed; its paired 960x640 Earth image was inspected with
schema 5, `quality_pending=true`, `settled=false`. This is offscreen evidence.

Actual Windows native evidence covers paired full-client/crop captures,
resize/minimize/restore, stale queries, busy/cancel/retry, lease expiry, failure
retention, disconnect and human wheel interruption. Actual stdio MCP exercises
structured actions, matching PNG content, immediate argument rejection and
cancellation. All four scenarios passed natively; final deterministic offscreen
repeats compare checkpoint values and decoded pixels. Owned rebuild/replay produced
a new session, equivalent semantic checkpoints and stale-handle rejection. The
fresh Codex run in `fresh-codex-final.jsonl` completed discover/launch/inspect/focus/
settings/capture/release/stop. A later retry using final binaries was interrupted
by an external service capacity error after inspection; do not count that retry
as a completed workflow. Its process absence and registry state were checked.

Linux WSL headless workspace tests passed; final focused tests and warnings-denied
all-target/all-feature Clippy also passed. Windows used Rust 1.98.1 and Linux 1.99.0;
the repository selects `stable`, not a pinned compiler. Native Linux and real
unsupported-adapter/forced surface-device-loss demonstrations remain UNTESTED.
Measured native process costs and capture cost have small raw sample budgets and
identified binaries; no universal overhead or speedup is asserted.

Use the [Phase 5.14 implementation report](PHASE_5_14_AI_ENGINE_DEVELOPMENT_INTERFACE_REPORT.md)
for exact commands, fingerprints, retained failures and final evidence paths.
The interface is IMPLEMENTED with exercised acceptance paths and PARTIAL overall
acceptance. It does not close historical 5.11E Acceptance A, terrain headroom,
visual approval, camera/navigation UX or unrelated platform/performance gates.


## 2026-10-05 terrain direction and redesign request

The user rejected the crater experiment, including its higher-detail result, and
requested a plan for a replacement LOD/rendering approach. The crater experiment
remains paused. The user subsequently requested starting
[the redesign](PLANET_TERRAIN_RENDERING_REDESIGN.md), authorizing the
[Slice 1 contract](PLANET_TERRAIN_SLICE_1.md) on 2026-10-05: a reusable world-owned
moon field and fixed-resolution visual references. Visual approval and later
GPU/streaming slices remain gated; do not treat this request as acceptance of
the rejected experiment or automatically advance to Slice 2.
The user also retired the old 128 MiB constraint and authorized spending machine
resources while pursuing excellent visuals and real-time performance.

IMPLEMENTED in the dirty Phase 5.15 experiment: footprint-independent crater
landmarks, sharper profiles, 128 seeded feature sizes and a demand-driven 512 MiB
CPU terrain ceiling. This is not visually accepted or fully validated. The latest
seed-2 regional pair reaches radial LOD 11 with about 4.84 m spacing, but remains
unsettled and visibly artificial. Its 2,046-leaf cover approaches the separate
2,048-leaf population limit despite RAM headroom. See the [report](PHASE_5_15_PROCEDURAL_CRATER_TERRAIN_FOUNDATION_REPORT.md)
and `target/phase515/recovery-512/` for scope and limitations. Passing numerical
checks and a larger memory budget did not close the visual gate.

## 2026-10-05 terrain redesign Slice 1 prototype

IMPLEMENTED: a separate world-owned `MoonTerrainDefinition` / `MoonLikeV1`
complete height, tangent-gradient and material oracle, with seeded spatial cells,
three impact epochs, bounded overlap composition, warped walls and broken rims.
See [source](../crates/world/src/terrain/moon.rs), the
[contract](PLANET_TERRAIN_SLICE_1.md) and [handoff](PLANET_TERRAIN_SLICE_1_REPORT.md).
The native path and `Body::terrain()` have not migrated to this definition.

The app's temporary f64 software reference renderer records orbit/regional/near
views for seeds 2, 7 and 19, seven diagnostic PNGs per scene and 495 complete oracle
queries for later reuse. Evidence is `target/terrain-redesign/slice1/reference-final/`.
All 63 PNGs and the query corpus match the preceding equivalent capture by SHA-256.
Focused world and example tests pass. All 13 documented Windows quality gates
have passing final results, including the four explicit GPU targets. Initial lint
and active-binary file-lock failures plus the successful focused/isolated retries
are retained in the handoff evidence. The final production check is offscreen,
ready but quality pending and
unsettled, and samples the legacy definition.

OBSERVED: the reference terrain remains soft and repetitive. Acceptance is PARTIAL;
convincing morphology/materials and user visual review remain OPEN. Discuss those
views before further Slice 1 refinement. Do not advance to Slice 2, claim native
GPU/LOD/performance acceptance, or resume the rejected experiment automatically.

## 2026-10-06 Slice 1 orbital target and moon variety

The user supplied lunar images as an orbital morphology/readability target:
dense multi-scale impacts, degraded overlap, rough highlands, smoother regions,
structured inter-crater terrain and terminator relief. They are not crater-layout
or composition templates. The user also clarified that other moons must support
substantially different appearances, including Phobos/Deimos, Europa, Io and
Titan-like families. See the [variety requirement](PLANET_TERRAIN_RENDERING_REDESIGN.md#variety-across-moons--user-clarification-2026-10-05).

IMPLEMENTED: explicit `MoonLikeV2` selection alongside preserved V1, with ten
impact epochs, two seeded rotated/translated layouts per epoch, bounded
age-ordered composition, broad highland/plains context and analytic derivatives.
The temporary reference adds fixed denser orbital geometry, represented-mesh CPU
ray shadows and separate unshadowed/visibility diagnostics. Local morphology
fixtures use higher sunlight, recorded independently from the orbital fixture.
The [orbital handoff](PLANET_TERRAIN_SLICE_1_ORBITAL_REPORT.md) indexes current
source, tests, captures and remaining gates; the preceding V1 section is historical.

At that checkpoint, the new field was the cratered family only. Generic family dispatch, irregular
shape support, ice/volcanic materials and atmospheric presentation are not
implemented. The current radial limit and three material weights must not become
universal moon assumptions. Body role, shape, structural family, materials and
optional atmosphere remain distinct design choices for future scoped work.
The native path and production clearance have not migrated. Slice 2 and visual
acceptance remain gated by user review; numerical/quality results cannot replace it.

## 2026-10-06 Slice 1B compositional surfaces

IMPLEMENTED: world-owned `SurfaceDefinition` separates shape, geological history,
material channel/composition and atmosphere descriptor. `RockyV3` wraps the
preserved V2 field with phenotype/resurfacing composition; independent `IcyV1`
and `VolcanicV1` fields use bounded local fracture/emplacement features. Sphere,
triaxial and asymmetric irregular shapes share complete radial queries and
analytic gradients. See [ADR 0009](adr/0009-compositional-body-surfaces.md),
[source](../crates/world/src/terrain/surface.rs), the
[contract](PLANET_TERRAIN_SLICE_1B.md) and
[handoff](PLANET_TERRAIN_SLICE_1B_REPORT.md).

IMPLEMENTED: actual celestial bodies can select the new authority transactionally,
and production radial clearance/camera safeguards query its combined shape and
relief. The native terrain renderer remains legacy-specific. There is no new
GPU tile system, atmospheric rendering or general solid collision. The radial
graph supports one positive radius per direction, with the explicit non-star-shaped
topology boundary in the ADR.

The evidence package is rooted at `target/terrain-redesign/slice1b/`: twelve fixed
seed bodies, unbiased orbital/regional/near views, separate selected landmark
views, grayscale height/shape/normal/material diagnostics, irregular-shape captures,
full definitions and reusable query corpus. The intermediate probe exposed weak
local geometry and regular icy bands, prompting the bounded morphology iteration.
Use `frozen-source-final/` and `reference-final-fixed-384/` plus the handoff for
current validation and visual observations; intermediate PNGs do not identify
the final field. The final package contains 49 scenes/588 per-scene PNGs and a
2,577-record corpus whose complete parsed replay matches exactly. V1/V2 historical
replays also match exactly for 165 seed-2 cases each.

VERIFIED: `full-validation-final/validation.json` records all 13 quality stages
passing on the frozen final source, including the four ignored native/GPU
regressions. The paired `ai-check-final-source/earth-orbit.json` on RX 9070 XT /
Vulkan is still quality-pending/unsettled legacy Earth; it is not compositional
rendering or settled terrain proof. See the report for scoped software timings
and process peak working set; there is no native performance improvement claim.

Acceptance is PARTIAL. The inspected near-view portion of Gate E is FAILED:
icy/volcanic near views do not yet convey convincing family grammar, even though
selected features exist numerically. User visual review remains OPEN; repetitive
regional material patches and similar bodies within each row also need review.
Stop in Slice 1B and discuss the captures; tests and generated contact sheets do
not grant approval or authorize Slice 2.

## 2026-10-06 Slice 1B.1 geological provinces

IMPLEMENTED: explicit `RockyV4`, `IcyV2` and `VolcanicV2` definitions add correlated
body histories, body-fixed geological directors, four normalized provinces per
family and family-specific regional/local process composition. Control gradients,
support windows, burial and bounded overlap participate in the complete world
query. New scalar diagnostics expose the same authority. Historical algorithms
remain separately selectable. See [ADR 0010](adr/0010-geological-province-directors.md),
the [contract](PLANET_TERRAIN_SLICE_1B_1.md) and
[handoff](PLANET_TERRAIN_SLICE_1B_1_REPORT.md).

VERIFIED: `target/terrain-redesign/slice1b1/reference-review-384/` contains twelve
fixed bodies plus an irregular stress fixture, 88 scenes / 2,288 paired scene
PNGs, four provinces per family at 20 km, 2 km, 256 m and 32 m, individual
director/process maps and 63 comparison sheets. Its 3,601-record complete corpus
matches the repeated numerical replay exactly. The prior 2,577-record family
corpus and both 165-case MoonLike replays are also exact; the twelve historical
near PNGs per Moon version retain their SHA-256 hashes. Source inputs are frozen
in `frozen-source-final/` with 234 fingerprints and no subsequent source drift;
the baseline audit records no unrelated file changes. Ignore older/interrupted
directories carrying `final` in their names when selecting this phase's evidence.

VERIFIED with an intermittent failure retained: the final quality matrix has
12/13 initial stage passes; the release workspace stage fails an unchanged worker
test (`adaptive.rs:1939`, expected pending job `Some(12)`, observed `None`). A
focused reproduction and exact full release-stage retry both pass, so all thirteen
stages have passing runs. This is not proof the intermittent assertion is fixed.
The report indexes both the failure and retry, 431-pass workspace suites,
46 focused reference tests, seven province tests and native/GPU checks.
The inspected RX 9070 XT / Vulkan Earth fast-check pair is still legacy,
quality-pending and unsettled (source/ready LOD 1 versus desired 14).

MEASURED: one contended optimized software reference process takes 1,142.489 s
with a 397.4 MiB peak working set. Complete queries visit 810 cells for RockyV4
(including retained Moon history) and 270 for ice/volcanic. These are bounded
software-reference observations, not production FPS, GPU performance, a speedup
or acceptance of continuous native approach. Generator heap/scratch and rocky
accepted-history support counts remain unavailable.

Acceptance is PARTIAL. OBSERVED: orbital impacts, icy lineaments and volcanic
roughness differ, but grey geometry is muted and some bodies remain similar;
Gate D is PARTIAL / OPEN for user review. Selected 256 m crops show real process
forms, while several 32 m and unbiased standing-height crops are smooth. The
near recognition portion of Gate E is FAILED; within-body volcanic province
separation is weak. The scalar maps and passing tests do not accept the visuals.
Stop within Slice 1B.1 for user discussion; do not begin Slice 2.

## 2026-10-06 Slice 1B.2 hierarchical authority checkpoint

IMPLEMENTED: `RockyV5`, `IcyV3` and `VolcanicV3` preserve their explicit province
parent and add bounded 256 m / 32 m / 8 m residual regimes. Fine continuous
processes contain 2 m joints, stress troughs/shoulders and stepped emplacement
fronts. Signed parent morphology, normalized preceding residuals and regional
lineage orientation correlate detail with larger geology. Analytic derivatives
and derived decomposition come from the same authority. Fixed search work adds
162 cell visits; no GPU tiles, streaming, LOD replacement or micro-detail renderer
is introduced. See [ADR 0011](adr/0011-hierarchical-geological-residuals.md) and the
[contract](PLANET_TERRAIN_SLICE_1B_2.md).

VERIFIED: frozen v4 independent complete corpora match exactly (7,085 records),
as do the historical 2,577-record family and 3,601-record province corpora and
both 165-record MoonLike corpora/near PNG sets. All fifteen focused hierarchy
tests pass. Keyed v3/v4 comparisons preserve all inherited/regional/local/context/
work values exactly. All thirteen frozen-v4 quality stages pass, including both
workspace suites, strict lint/rustdoc, long orbits and explicit GPU/developer
regressions. The complete-package audit passes: twelve bodies, 106 scenes, twelve
five-scale approaches, 36 nine-channel decomposition-map scenes, 3,080 scene PNGs
and 63 comparison sheets. Full capture/replay also matches exact float bits;
all 3,143 PNG encodings verify. The final sheet helper includes all five approach
columns; its packaging-only hash change is recorded separately from the unchanged
compiled v4 inputs.
The final native Earth fast-check pair passes its commands
but remains ready, quality-pending and unsettled; it is not new-family or native
interaction acceptance. Evidence is under `target/terrain-redesign/slice1b2/`.

OBSERVED: a first 8 m candidate was broadly featureless despite nonzero relief
and less than 0.1 mm measured mesh error. Fine-process corrections now reveal
icy troughs and unequal shoulders, volcanic steps/fronts, and rocky joints.
Some selected views are dominated by steep parent slopes; the appearance remains
soft and rounded, and rocky 8 m family recognition is weak. Unbiased 8 m crops
have relief; the separate 128 m standing views show represented mesh/shadow
facets in steep terrain. These observations do not establish polished visual
acceptance. Final neutral review finds strong family identity at 256 m and
generally at 32 m. At 8 m, rocky impact-derived recognition is FAILED; ice and
volcanic recognition are PARTIAL, with weak/rounded forms. Unbiased crops are
useful relief fixtures but do not accept their family recognition. The maximum
sampled 8 m triangle-centroid error across selected crops is below 0.854 mm; this
is an uncertified finite sample, not a global error bound.

MEASURED: matched optimized complete-query medians are 20.841 microseconds for
RockyV5, 12.038 for IcyV3 and 11.959 for VolcanicV3, with paired parent cost ratios
1.260 / 1.572 / 1.587. Each successor adds 162 fixed cell visits. Complete capture
runtime is 1,650.092 seconds and process peak working set 399.1 MiB; some capture
work overlaps validation. These are software reference observations, not native
FPS or a production speedup. Raw fixtures, counters and limitations are in the
[Slice 1B.2 report](PLANET_TERRAIN_SLICE_1B_2_REPORT.md).

Recommendation: **A — READY FOR SLICE 2**, with visual acceptance PARTIAL. No
concrete fundamental representation/architecture blocker was found. Weak fine
profiles remain authoring work, especially rocky 8 m; this does not accept the
unmet visual target. The package is complete. Stop for reviewer/user discussion;
do not start Slice 2 automatically.

## 2026-10-06 Native Moon integration checkpoint

IMPLEMENTED: gameplay and real-scale Solar System presets publish `RockyV5` as
the Moon's `SurfaceDefinition`. The native definition adapter dispatches the exact
selected complete field through existing workers, cache, adaptive LOD,
stitch/morph, and the matching complete-query camera-clearance path. The legacy
`terrain_definition` and `cratered_terrain_definition` helpers remain separately
available for old fixtures. No GPU tile redesign or material-channel shading was
added.

The initial full-matrix debug and release suites failed because `terrain_workers`
still used legacy `terrain().unwrap()` fixtures after surface-authority migration.
Those fixtures require migration before the matrix can establish current status.
Initial strict Clippy identified `manual_range_contains`; this was fixed, and both
standalone strict Clippy runs pass. These and earlier focused passes do not establish
a full-matrix pass. Final matrix evidence is pending.

OBSERVED: the first `ai-check` pair exits successfully but terrain is inactive; it
predates the fixture's recognition of the new surface authority and is retained as
failed integration evidence. The subsequent
[`ai-check-v2` pair](../target/native-moon/20261006-014148/ai-check-v2/summary.md)
reports Moon / RockyV5, `complete_amplitude_bound`, ready radial LOD 1 versus desired
30, and `quality_pending=true`, `settled=false`. Its PNG shows a coarse whole-body
representation. This does not establish useful convergence, polished morphology,
performance improvement, or visual acceptance.

OBSERVED: the pre-fix native Moon pair is recorded in
`native/captures/12764-1791251339393283700-4-moon-orbit/` (`viewport.png`,
`snapshot.json`, `complete.json`). It reports source radial LOD 16, ready LOD 17,
desired LOD 30, budget-constrained and quality-pending at 327,278 m clearance.
The user rejected the latest screenshots for center detail, coarse edges and slow
retained cover; visual acceptance is **FAILED** for those images. Final screenshots
and current-source validation evidence remain pending. A later native sampling lease
was interrupted at 20/100 samples; the script stopped before requesting a capture,
and no capture was requested after interruption.

Older preliminary cadence data in
`target/moon-lod-fix/20261006/preliminary-performance.json` predates the latest
selector and final normal stencil; it is not a performance claim for current source.
Stale/minimized data in `native/readonly-cadence.json` has zero counters and provides
no FPS evidence. No current performance claim is made.

IMPLEMENTED: compositional refinement prioritizes the highest projected demand using
the nearest point of each conservative patch ball, avoiding chart-center
undersampling. Stale/coarsening checks use the same representation demand, and
prefetch follows the selector's actual outstanding requests. This does not change
the global `2H` certificate plus sphere-correspondence bound or certify its target.
Native material/shadow presentation remains different from the reference; secant mesh
normals are approximate. See the
[implementation handoff](NATIVE_MOON_INTEGRATION_REPORT.md).

### 2026-10-06 Native Moon compositional coarsening follow-up

IMPLEMENTED: compositional coarsening batches up to 32 merges per update, while
legacy coarsening remains one merge and refinement remains one step. Covers track
all 32 merge parents in a fixed inline array; queued/building work is cancelled if
any changed parent invalidates the merge. A regression now covers the singular-
parent tracking gap found during review.

VERIFIED: focused results for this update are app library 44/44, native Moon 4/4, adaptive 3/3,
population 6/6, terrain workers 2/2, renderer library 44/44 and renderer demand
1/1. Four final-lint gates, full workspace debug/release tests, rustdoc, long-orbits,
bridge tests and all four native/GPU capture checks passed. Two initial matrix lint
failures were repaired and superseded by strict reruns: 13 passing latest gates in
`target/moon-lod-fix/20261006/completion.json`. The debug workspace run began before
the final all-parent invalidation change; current focused debug and full release
tests passed. Code checks do not establish visual or performance acceptance.

OBSERVED: the latest pre-batch native orbit capture, at 15 seconds, reports source
LOD 4 and desired LOD 7 and remains coarse. Human input ended the observation lease
during close-in, before zoom-out. Visual acceptance remains **FAILED** for the
user-rejected center-detail/coarse-edge/slow-retained-cover images; there is no
final visual acceptance or current final-source FPS claim. Evidence is in
`native-final/captures/18696-1791253169899094200-5-orbit/`. See the
[integration handoff](NATIVE_MOON_INTEGRATION_REPORT.md) for implementation limits
and earlier evidence history.
