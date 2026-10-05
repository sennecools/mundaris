# Phase 5.13C — cinematic procedural distant sky

## Goal

Implement the user-approved deterministic cinematic decorative sky without changing
celestial authority, navigation, terrain or dependencies. Clear the mandatory native
playback/focus/attachment prerequisite first. Present representative sky captures to
the user **before final polish, warm profiling and full validation**.

## Result

**PARTIAL — native prerequisite cleared; rejected first look revised, awaiting review.**

The user rejected version 1 as bland compared with the desired cinematic references,
then explicitly approved fixing the visual recipe. Version **2** replaces the soft
grey-brown stripe with brighter structured stellar clouds, contrasting fine dust,
cooler blue-white populations and a warmer core. The
[before/after comparison](evidence/phase513c/visual-comparison-20261005-10/before-after.png)
uses identical camera state and unaltered raster pixels: rejected version 1 left,
revised version 2 right. This is implemented visual revision, **not user acceptance**.

- VERIFIED: the pre-sky readiness-gated native run retained **8/8** unobstructed,
  focused-foreground checkpoints and app/observer exit **0**. All eight PNGs and
  associated snapshots were inspected. This is scripted native evidence, not human
  control-feel acceptance or native validation of the subsequently added sky.
- IMPLEMENTED: immutable app-owned seeded/versioned finite stars; renderer-owned
  directional galactic texture, dust lanes, mip filtering, finite-star projection,
  restrained halos, bounded static residency and developer appearance controls.
- VERIFIED: focused debug/release sky/preset tests, independent f64 projection
  oracle fixtures, shader validation, cache/toggle/occlusion/invalid-state GPU test,
  day/night GPU comparison and existing analytic/focus-cancellation regressions.
- MEASURED: rendered centroid maximum error **0.001645105 physical pixels** for the
  identified 960×640 adapter-required fixture. It does not prove every resolution,
  FOV, pose or catalogue entry on the GPU.
- UNTESTED: user visual approval, real-scale sky capture matrix, complete current-sky
  native navigation/lifecycle validation, warm raw profiles and full quality matrix.

No next universe/addressing/streaming phase is started. Terrain Acceptance A,
camera/control UX and developer UI acceptance remain open.

## Architecture changes

`crates/app/src/sky_definition.rs` now authors **48,000** decorative points with explicit
preset version **2** and seed `0x6d756e6461726973`. Coordinates are galactic-local; their
fixed orientation is `rotation_z(0.4) * rotation_x(0.7)`. The galactic normal is +Y,
the band lies in X/Z, and the anchor is the system inertial origin. No `BodyId`,
gravity source, selectable destination, authoritative galaxy or ambient RNG exists.

The range/envelope was declared before sky implementation: finite stars are at
**1e18–2e19 m**, supported observers within **1e14 m** of the anchor. Renderer
preparation uses the coherent source-relative observer and direction APIs in f64.
Static star positions and per-frame observer offsets are then represented in
1e18-m GPU units; the shader subtracts the observer before rotating/projecting.
Outside the envelope the layer is explicitly unavailable, without clamping camera
motion or narrowing an unsupported observer into an otherwise valid foreground.
This is not a universe-scale precision contract.

The revised visual recipe uses authored cloud complexes plus multi-scale directional
turbulence and variable-width, fragmented optical-depth dust ridges. Stellar color
populations vary spatially instead of tinting the whole band brown. The catalogue
includes clustered disk clouds, more isotropic stars and a wider brightness hierarchy.
The static background is **4096×2048** with linear-light mip construction, bounded
at those dimensions; the finite-star admission bound is **65,536**. No point stars
were moved into the directional texture, and no per-frame noise/twinkling was added.

The main reverse-Z pass draws a directional mipmapped galactic texture and instanced
Gaussian stars **before opaque bodies/terrain**, with no sky depth writes. The
renderer retains one catalogue/background definition by `Arc` identity; camera,
playback and appearance changes upload only a **112-byte uniform**. Texture generation
and uploads occur on definition replacement, not on ordinary pose/time changes.
The background is fixed directional content, not per-frame procedural screen noise.

Early sky-on/off fixtures demonstrated daytime leakage through the existing scalar
LDR atmosphere composition. The bounded shader correction suppresses decorative sky
only at **zero-depth pixels near an illuminated surface**. Foreground-pixel atmosphere
composition remains unchanged. This is not physical stellar radiometry or an HDR
atmosphere rewrite. Night/airless visibility is retained in the tested fixtures.

Canonical diagnostics are schema **4**, with an optional `sky` record containing
definition, controls, actual draw flags, envelope and separate resource/timing scopes.
Old snapshots deserialize with no sky record. Native resources/GPU observations can
describe an earlier submission/completion; deterministic captures identify same-frame
observations. Cold/unavailable sky timestamps are null, never presented as zero wins.

## Files changed

Task-owned additions:

- `crates/app/src/sky_definition.rs`: decorative preset and deterministic tests.
- `crates/app/src/sky_capture.rs`, `crates/app/examples/sky_capture.rs`: first-look
  fixtures and adapter-required day/night regression.
- `crates/renderer/src/sky.rs`, `sky_background.rs`, `sky_gpu.rs`,
  `shaders/sky.wgsl`: validated inputs, precision boundary, static generation,
  residency, rendering and focused tests.
- `crates/renderer/tests/sky_capture_513c.rs`: real-adapter numerical/depth/cache test.
- Phase-local runners, raw evidence, report/index and dated reviewer context.

Focused integration edits to previously dirty files: app manifest/module export,
`gravity_orbits.rs`, its `analytic_validation.rs` and `developer_ui.rs`, developer
capture/snapshot/contract test; renderer `celestial.rs`, `gpu_profile.rs`, module
exports, `terrain_capture.rs` and `shaders/planetary.wgsl`. Also updated schema checking
in `scripts/ai-check.ps1` and `docs/AI_DEVELOPMENT_INTERFACE.md`.

The native gate was separately user-approved: an optional
`MUNDARIS_ANALYTIC_VALIDATE_READY` marker and native focus gate **startup only**.
Without it, legacy route behavior is unchanged. The runner confirms foreground/focus
for 0.5 s before signaling readiness. Original route commands, thresholds and eight
criteria are unchanged; later focus loss still uses existing cancellation safety.

No dependency/world/simulation/terrain/controller edits belong to this task. The
starting dirty source fingerprints, current fingerprints and explicit edited-path
allowlist distinguish this work from pre-existing user work. Overlapping integration
files are not asserted byte-identical; untouched baseline files are hash-checked.
No cleanup, commit or push was performed.

Relative to the separately recorded rejected-version visual baseline, this revision
changes only **three production files**: `sky_definition.rs`, `sky_background.rs`,
and `sky.rs` (bounded star/texture admission). Pipelines, shaders, atmosphere,
navigation, world/simulation, dependencies and the capture/snapshot schema are
unchanged by this visual revision. Fresh deterministic generation is tested with
independently built definitions; session `Arc` residency remains the existing cache.

## Tests

Windows, AMD Radeon RX 9070 XT / Vulkan; uncommitted source at HEAD
`d81bb2a17ad4f8c1d06eaa1619b6281b946641c3`. Exact locked commands and logs are retained
in the [evidence index](evidence/phase513c/README.md).

- `native-ready-20261005-03`: observer exit 0, stages 0–7; readiness binary hash
  `FF4D2A429B9C74F35CB86B8BC787D0440B77A2414AFC1EE6D6E1178817F8C7C9`.
- `sky-focused-20261005-08/commands.json`: all 11 focused Cargo commands exit 0,
  including debug/release sky and preset tests, GPU query accounting, planetary WGSL,
  schema contract, readiness/focus-cancellation and analytic regressions.
- `gpu-focused-20261005-06`: explicit release GPU test exit 0; actually rendered
  finite-star centroids, reuse after toggling, opaque occlusion, exact return image
  and rejection of invalid frames without partial upload.
- `day-night-gpu-20261005-07`: explicit release GPU test exit 0, matched foreground
  snapshots, day sky-on/off maximum channel difference **1/255**, night **193/255**.
- `sky-fast-20261005-08/validation.json`: `ai-check` exit 0; paired snapshot/PNG
  inspected. Its terrain remains `quality_pending=true`, `settled=false`.
- `clippy-20261005-08`: focused app/renderer all-target/all-feature Clippy exit 0
  after replacing the capture helper's complex tuple/many arguments with a fixture
  struct. The earlier failed lint result remains retained.
- `affected-final-20261005-08/commands.json`: both explicit GPU tests, affected-crate
  all-target/all-feature Clippy and workspace formatting rerun against the checkpoint
  source, all exit 0. Centroid and day/night results are unchanged.
- `checkpoint-verification-20261005-08/summary.json`: 176/189 baseline source files
  remain byte-identical; the other 13 are explicit task integration paths, with eight
  new source files. All 121 historical 5.13B artifacts remain byte-identical. Parser,
  whitespace, pre-sky native prerequisite and first-look return-image checks pass.

Version-2 checks:

- `visual-focused-20261005-10/commands.json`: all 11 locked focused checks exit 0.
  Added background tests verify contrasting warm/cool populations, absorbing dust,
  bright-cloud/dark-pocket hierarchy and the below-display-code polar cutoff.
  Preset tests cover fresh repeatability, finite bounds, clustered population,
  color variety and brightness distribution.
- `visual-affected-20261005-10/commands.json`: explicit GPU centroid/cache/occlusion
  and day/night tests, affected-crate Clippy and formatting all exit 0. Centroid
  maximum remains **0.001645105 physical px** in the named small fixture. Day
  sky-on/off maximum remains **1/255**, night is now **239/255** with matched
  foreground snapshot state.
- `visual-fast-20261005-10/validation.json`: fast check exit 0; production Earth
  PNG/schema-4 snapshot inspected, still quality-pending/unsettled.
- `visual-build-20261005-10/command.json`: locked release all-feature app build
  exit 0; current executable hash retained. No current-sky native route is claimed.
- `visual-verification-20261005-10/summary.json`: separately records this revision's
  three changed production paths and untouched rejected-version source fingerprints,
  current version-2 capture association, return equality and historical preservation.

The CPU oracle tests cover 960×640, 2560×1440 and 3840×2160; 10/30/60/120° FOV;
nonzero viewport origin; several observer baselines/rotations. The named GPU centroid
fixture covers 960×640, 60°, at origin, 1 AU and 9e13 m offsets. Full GPU precision
coverage at the other resolutions and attached-motion fixture matrix remains open.

## Measurements

No accepted p95 sky profile exists. The required **≥30 warmups / ≥200 raw samples**
and on/off baseline comparison have not run before the visual checkpoint. Planning
targets remain CPU preparation **≤0.1 ms p95** and GPU sky **≤0.5 ms p95** at
**2560×1440** on RX 9070 XT. Individual capture timings are not target acceptance.

The rejected-version fast 960×640 capture reported one cold renderer representation generation
of **189.3303 ms**, upload API work **2.5258 ms**, CPU sky preparation **0.0029 ms**;
its sky GPU timestamp is unavailable. Owned GPU payload capacity is **11,709,212 B**,
CPU definition payload **480,152 B**. This is a single capture, not a warm distribution
or FPS measurement. App preset construction, transient generation peak/RSS and driver
allocation are unmeasured. Renderer `generation_ms` includes catalogue packing and
background/mips, not app preset construction; uploads/submission/readback are separate.

The revised version-2 `visual-fast-20261005-10` single cold capture reports renderer
generation **989.6462 ms**, upload API **5.1842 ms** and sky preparation **0.0029 ms**;
its sky GPU timestamp is unavailable. Owned GPU payload is **46,836,508 B** (~44.7 MiB),
CPU definition payload **2,304,152 B** (~2.2 MiB). This is the explicit quality cost of
the denser catalogue and doubled texture dimensions: approximately four times the
prior resident GPU payload and about one second of cold generation on this fixture.
It is **not a warm p95 or performance win**; optimization/target acceptance remains
open after user visual review. Terrain's existing cap and foreground state were not
changed. Transient generation memory/RSS/driver allocations remain unmeasured.

The first-look shared-renderer sequence records catalogue/background upload counts
staying at **1/1**. Subsequent enabled frames have **0 static bytes / 112 frame bytes**;
disabled frames upload no sky data. Return-to-core PNG matches the original exactly.
This verifies exercised reuse, not a stationary/moving-camera performance budget.

## Captures

The revised visual checkpoint is `docs/evidence/phase513c/visual-recipe-20261005-10/`:

- `sky-toward`, `sky-along`, `sky-away`, `sky-rotated`, `sky-viewport-origin`,
  `sky-return`: authored band/dust, sparse away sky, rotation, viewport and return.
- `earth-day-on/off`, `earth-night-on/off`: identical foreground states with the
  atmosphere actually drawn; near-surface outward sky observations.
- `earth-silhouette`, `moon-silhouette`, `moon-airless`: opaque depth and airless sky.

All are gameplay-preset 960×640 PNG/schema-4 JSON pairs through production renderer
paths. Pure sky and low-level silhouette/atmosphere fixtures intentionally admit
**no terrain/ocean/clouds**; they are not settled terrain evidence. The separate fast
Earth capture exercises production terrain and planetary shells, but is unsettled.
Representative images and diagnostics were inspected; user visual acceptance is open.

Earlier first-look `-05` captures had incorrectly ordered fixture lighting, so their
day/night atmosphere was **not drawn**. `atmosphere-check-...-06` corrected that
fixture and reproduced leakage. Retain these as intermediate/failure evidence, not
the corrected visual checkpoint. No historical artifact was overwritten.

## Known failures and limitations

- Native runs 01 and 02 failed complete-route observation (6/8 and 1/8) before the
  approved startup gate. Foreground arrived too late; no engine defect was established.
  The successful pre-sky run supersedes the prerequisite blocker, not those raw logs.
- The first fast sky check failed its old schema-3 association check; the updated
  schema-4 checker and later fast runs pass. The initial focused runner also stopped
  on harmless Windows PowerShell native-stderr wrapping; its retained replacement
  records Cargo exit codes correctly.
- No user visual approval yet. Band detail/brightness, star density/color/halos and
  temporal filtering remain subject to review; static images alone cannot establish
  slow-motion stability. No final cinematic acceptance claim is made.
- Warm profiles, 1440p/4K captures, real-scale system coverage, fuller near-surface
  sky/terrain comparisons, current-sky native playback/lifecycle and full quality
  matrix remain UNTESTED. Linux/remote CI and human physical controls are UNTESTED.
- Resident capacity diagnostics exclude transient generation and driver allocation.
  Non-sRGB-target additive-color equivalence has no focused overlap comparison;
  retained Vulkan captures use the sRGB path. No universal photometric claim is made.

## Evidence

See [phase evidence index](evidence/phase513c/README.md). Starting baseline, source
fingerprints, command logs, intermediate failures and PNG/JSON pairs remain separate.
The checkpoint verifier records untouched-source/historical identity, explicit task
paths, script parsing, whitespace, native prerequisite and return-image equality.

## Git state

Branch `main`, HEAD `d81bb2a17ad4f8c1d06eaa1619b6281b946641c3`, substantial pre-existing
dirty/untracked work plus these uncommitted task changes. Current source fingerprints
identify dirty builds; HEAD alone does not. No commit/push is authorized.

## Reviewer follow-up

Ask the user to review the current **toward / along / away** captures and the Earth
example. Do not treat this handoff as approval. After visual direction is agreed,
continue only the remaining approved 5.13C polish/evidence/profile/validation scope.
Do not close earlier terrain, camera or UI gates or start universe/streaming work.

Version-1 `first-look-20261005-07` is now explicitly **rejected visual evidence**,
not the current proposal. `visual-recipe-20261005-09` is an intermediate revised
capture preceding the final polar cutoff guard. Current directional, night, daytime
and production Earth images were reinspected after that guard in the `-10` runs.
