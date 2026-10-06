# Slice 1B.1 geological province director handoff

Date: 2026-10-06. Contract: [Slice 1B.1](PLANET_TERRAIN_SLICE_1B_1.md).
Evidence root: `target/terrain-redesign/slice1b1/`.

## Goal

Add deterministic body histories, continuous geological provinces and distinct
rocky, icy and volcanic processes from orbital to near views. Preserve the
historical numerical definitions. The primary acceptance criterion is family
recognition from neutral geometry at regional and representative near scales.
The user owns visual acceptance and permission to begin Slice 2.

## Result

PARTIAL: the versioned authority and evidence workflow are implemented, historical
numerical replay is exact, and every quality stage has a passing result after a
release retry. The inspected near portion of Gate E is FAILED: several small
and unbiased crops remain too smooth for consistent family recognition from
grey geometry. User visual acceptance remains OPEN. No Slice 2 work is included.

## Architecture changes

IMPLEMENTED: `SurfaceDefinition` remains the complete immutable authority.
`RockyV4`, `IcyV2` and `VolcanicV2` select new geological fields and compatible
`RockyV2`, `IcyV2` and `VolcanicV2` material semantics. Historical `RockyV3`,
`IcyV1`, `VolcanicV1` and MoonLike definitions remain separately selectable.
The version and phenotype feed the terrain configuration identity. Material
and atmosphere identities remain separate from geometry.

The data flow is definition/seed → correlated body phenotype → rotated
low-frequency body-fixed director → normalized province weights and process
controls → bounded regional/local processes → complete shape + relief query.
`SurfaceGenerator::geological_controls()` exposes the same authority for
diagnostics. It returns `None` for historical algorithms. Camera, rendering
footprint, mesh density and worker order are absent from generation.

The phenotype correlates age, activity, resurfacing, impact retention and relief.
Seeded smooth softmax fields bias four provinces per family. Scalar control
gradients participate in the height derivative, including burial and process
activation. The normalized tangent stress direction has critical-point fallback
orientations; feature axes are fixed at their immutable centres so query-side
orientation normalization does not create moving features or support seams.
This is a procedural geological history model, not a calibrated physical
evolution simulation or an ecological biome system.

| Family | Province/process composition |
| --- | --- |
| Rocky | Ancient highlands, basin margins, resurfaced plains and structural uplands. A separately seeded preserved MoonLikeV2 history supplies impacts; province-dependent retention and burial modify it before younger local impacts and structural ridges are added. Fine old impacts and grit retain local relief outside large landmarks. |
| Icy | Old ice, fracture tectonics, young ice and reworked chaos. Retained old relief is softened/buried before warped unequal faults, intersecting shoulders/troughs and reworked blocks are applied. Younger ice suppresses older relief. |
| Volcanic | Old substrate, shields/calderas, flow plains and young resurfacing. Old roughness/impacts are buried before lobed construction, summit depressions, directional emplacement ramps and pressure fronts are applied. |

Five cell levels use edges `0.18R`, `0.045R`, `0.009R`, 180 m and 24 m,
with an 8 m minimum, two deterministic translated layouts and one-cell halos.
Compact support is `0.52 × edge`; jitter and shell tolerances are 0.16 and 0.30.
The excluded-centre margin is `1 − 0.16 − 0.30 = 0.54 > 0.52`.
No body-wide feature catalogue is scanned per query. Windows vanish smoothly at
support edges, and the local blend divides by `1 + sum(window)`.
Analytic derivatives include numerator, denominator, windows and process controls.

Local budgets are `min(0.055 × edge, R × phenotype_relief × allocation)` with
allocations `[1.2, 0.6, 0.25, 0.08, 0.08]`. Fixed fine wavelengths of 600 m,
90 m and 14 m use separate family grammar and bounded allocations. The envelope
is the preserved history bound plus `2.02 × R × phenotype_relief` plus `2.5`
times the sum of the five local budgets, rounded outward. The 2.02 allowance
covers director/regional and continuous fine relief; the local profile cap
covers signed rocky, icy and volcanic composition. This is an algebraic bound,
independent of sampled maxima. Positive radial envelopes remain checked upstream.

The world/reference boundary is unchanged. Existing body publication and radial
clearance can consume a selected valid surface definition. The native adaptive
terrain renderer still uses its earlier terrain path. No GPU generation,
resident tiles, LOD replacement, streaming, collision topology or atmospheric
rendering is added. [ADR 0010](adr/0010-geological-province-directors.md)
records this decision and [ADR 0009](adr/0009-compositional-body-surfaces.md)
retains the positive star-shaped representation boundary.

## Files changed

Task edits within the pre-existing dirty surface/reference work:

- `crates/world/src/terrain/surface.rs`: explicit successor versions, compatible
  materials, optional director API, field dispatch and diagnostic probes.
- `crates/world/src/terrain/surface/geology.rs`: exhaustive historical dispatch;
  old numerical formulas are retained.
- `crates/app/examples/moon_surface_reference.rs` and its `surface_adapter.rs`:
  uniform-grey geometry, individual authoritative director maps and metadata-driven
  local inspection cameras; historical camera defaults are preserved.
- `crates/app/examples/surface_family_reference.rs`: explicit `--provinces`, fixed
  fixture reuse, optional body filtering, body statistics and extended corpus.
- `scripts/surface-family-sheets.py`: neutral orbital, province/scale comparisons,
  director sheets and cell-to-source metadata audit mapping.
- `README.md`, `docs/architecture.md`, `docs/REVIEWER_CONTEXT.md`: reproduction,
  authority and dated review orientation.

New task files: world `surface/director.rs`, `surface/provinces.rs`, external
`tests/surface_provinces.rs`, reference `surface_family_reference/provinces.rs`,
the copied phase contract, this handoff and ADR 0010. Other dirty source,
workflow configuration, developer tooling and earlier reports belong to the
pre-existing work. `preservation-audit.json` compares 5,110 baseline files and
records no changes outside this task's explicit ownership.

## Tests

VERIFIED: the final focused world command
`cargo test --locked -p mundaris_world --test surface_provinces` passes all seven
tests (`provinces-focused-selected.txt`). They cover order/thread/chart-edge
determinism, normalized province coverage, seed history changes, bounded complete
queries across radii, material/atmosphere independence, numerical height gradients,
province/support continuity and activity/resurfacing response. Internal differential
and enlarged-halo tests are included in the world test suite.

The independent finite-difference height check uses a 0.2 mm physical perturbation
to resolve the retained fine rocky primitives. An earlier 2e-7 angular step
was too coarse: at R=109 km, seed `0xcafebabe`, identity `0x511b01`, the tangent
derivative converged from −2966.1954 at that step to −2959.5972 at 2e-9, against
analytic −2959.5970. The same convergence occurs in the isolated preserved
MoonLikeV2 component. This was a finite-step validation limitation, not evidence
that a large-step failure should be discarded without investigation.

VERIFIED: final focused reference tests pass 19/19 Moon example and 27/27 family
example tests (`reference-focused-tests-review.txt`). The final AI check passes
all recorded checks; the paired native PNG and snapshot are inspected below.
VERIFIED: all thirteen documented quality stages have passing results on the
frozen review source. `full-validation-review-source/validation.json` records
12/13 passes initially, including format, all-target/all-feature workspace check,
both strict Clippy modes, debug workspace tests, strict rustdoc, long-orbit tests,
developer bridge and all four ignored native/GPU targets. The release workspace
stage initially fails as described under Known failures; the exact full release
stage passes on retry in `full-validation-release-retry/validation.json`.
The debug suite takes 442.71 s and the release retry 56.93 s in this run.
These durations are validation observations, not terrain speedup measurements.
Each passing debug/release workspace run records 431 passes and nine ignored
tests; the required ignored native/GPU targets run separately in the matrix.
Exact stage commands and exit codes are retained in the JSON and text logs.

Reproduction uses the isolated target directory, with new evidence directories:

```powershell
$env:CARGO_TARGET_DIR='target/terrain-redesign/slice1/validation-build'
./scripts/ai-check.ps1 -OutputDirectory target/terrain-redesign/slice1b1/ai-check-reproduction -Scene earth-orbit
./scripts/validate.ps1 -OutputDirectory target/terrain-redesign/slice1b1/validation-reproduction -IncludeGpu
cargo test --locked -p mundaris_app --features terrain-capture --example surface_family_reference --example moon_surface_reference
```
Intermediate failures are retained separately. The Clippy
redundant-closure failure was repaired in the new test; the current focused
strict check passes. No criterion was relaxed to obtain that result.

## Numerical evidence

VERIFIED: the historical family replay matches all 2,577 complete parsed records
in the retained Slice 1B checkpoint exactly. Both MoonLikeV1 and MoonLikeV2 seed-2
replays match all 165 complete cases each. Geometry, gradients, combined normals,
materials, identities and work counters are included in the comparisons;
wall-clock timing is excluded. See `historical-replay-result.json` and
`moon-replay-results.json` with their raw corpus paths.
All twelve historical PNGs in each Moon seed-2 near replay also match their
retained checkpoint SHA-256 values. The added grey geometry output does not
change those historical rendered images.

The successor corpus contains 3,601 complete queries across thirteen definitions,
including canonical cube duplicates, feature/cell/support boundaries, province
centres, local processes, mixed transitions and equal-weight crossings with
both sides sampled. It spans 80 km, each fixture radius and 1.2 million m.
Every successor record also includes authoritative controls. VERIFIED:
`reference-review-384/package-audit.json` records exact equality of all 3,601
complete corpus records with `directed-review-corpus-replay/surface_queries.json`.

Body statistics use 4,096 fixed equal-area Fibonacci directions per body. All
four seeds in each family are retained; body age/activity and province/process
statistics are available in the manifest. Fixture radius varies with seed,
so visual seed comparisons also vary radius. Director statistics and same-radius
test/corpus queries provide the controlled history evidence; appearance alone
does not isolate that variable.

MEASURED on the fixed 4,096-direction scans: dominant rocky ancient-highland area
varies from 13.21% for seed 0 to 78.52% for seed 1; young-ice area varies from
24.41% for icy seed 0 to 42.36% for seed 1. Volcanic old-substrate area varies
from 75.24% for seed 0 to 20.68% for seed 3, while young-resurfacing area changes
from 21.41% to 47.88%. This is evidence of different sampled province histories,
not merely translated feature centres. The targeted volcanic seed-0 body is
also the oldest of its four fixtures; its dominant shields/flow areas are small.
That limitation is recorded, not used to excuse a failed recognition criterion.

## Measurements

MEASURED: complete query work in the final successor corpus is bounded at 810
cell visits for RockyV4 (270 new + 540 retained Moon history) and 270 for IcyV2
and VolcanicV2. Candidate ranges are 565–620, 28–80 and 32–82 respectively.
Accepted supports range 0–10 for ice and 1–10 for volcanic queries. Rocky's
accepted-support count remains unavailable because the retained history does
not expose it; `null` is not zero.

These ranges describe the corpus fixtures, not global maxima. The fixed halo
establishes the cell bound independently. Reference metadata separately accounts
for mesh query work, material filtering, residual probes, director maps and
camera queries; mesh counters do not pretend to account for every diagnostic query.
Owned generator heap/scratch memory is unavailable; inline sample sizes are
reported as inline sizes only.

MEASURED: the completed process takes 1,142.489 s, exits 0 and records a peak
working set of 416,722,944 bytes (397.4 MiB), with 1,130 process samples at
approximately one-second intervals. The raw measurement is
`reference-review-384-process/measurement.json`.

| Family | Mesh/oracle preparation per scene, ms | Software rasterization per scene, ms |
| --- | --- | --- |
| Rocky, including irregular stress | 1,981.93–85,635.69 | 197.43–1,001.49 |
| Icy | 727.76–40,797.64 | 192.70–662.40 |
| Volcanic | 910.96–32,488.34 | 199.37–610.11 |

These are ranges across different views/radii, not comparable per-query throughput.
`metrics-summary.json` retains their underlying capture metadata. They measure
one contended optimized software reference run on Windows 11 Pro build 26200,
Ryzen 7 9800X3D (8 cores/logical processors reported) and 32 GiB RAM; see
`environment.json`. They establish no production FPS,
GPU timing, CPU speedup, renderer scalability or resident-memory claim. The
reference runs concurrently with quality validation, so these are observations
rather than a controlled before/after benchmark.

## Captures

The final review package is `reference-review-384/`, generated by the retained
`surface_family_reference-review.exe` with `--provinces --orbit-cells 384
--local-cells 384`. It includes twelve bodies × three unbiased views, four
provinces × four scales for the seed-0 body in each family, and four irregular
shape stress views. VERIFIED: the package audit checks all 88 scenes, 2,288
per-scene PNGs, paired metadata and dimensions, with no missing files. The sheet
script completes end to end: 63 sheets and 420 audit-mapped cells are recorded.

Targeted scales are 20 km, 2 km, 256 m and 32 m, with nominal mesh spacings
52.083 m, 5.208 m, 0.667 m and 0.083 m. These are separate fixed crops, not
continuous approach or LOD evidence. Terrain PNGs are 960 × 640. Individual
director maps use their separately recorded map dimensions and projections;
they are not aligned overlays on the perspective image.

MEASURED sampled representation residual maxima for the four targeted crops at
each scale (metres; neither certified nor a global error bound):

| Family | 20 km | 2 km | 256 m | 32 m |
| --- | --- | --- | --- | --- |
| Rocky | 3.0259 | 0.4525 | 0.02415 | 0.001262 |
| Icy | 13.6307 | 1.2938 | 0.18606 | 0.003751 |
| Volcanic | 5.5909 | 0.8573 | 0.12713 | 0.002065 |

Whole-package maxima, including orbital views, reach 103.79 m, 67.77 m and
141.12 m respectively. Orbital fixed meshes do not establish that all fine
authoritative relief is represented. Small near-crop residual samples support
the visual assessment's scope but do not certify absence of all aliasing.

Province selection first maximizes each weight over 4,096 fixed Fibonacci
directions. Local crops select the greatest gradient variation over ±4 m along
two tangent axes among nearby world landmarks with slope ≤1 and at least 85%
of the centre's province weight. Fallbacks and exact crop coordinates are
recorded. This selection exposes morphology; retained unbiased standing-height
crops prevent selected landmarks from replacing representative evidence.
The 20 km crop stays at the province centre; smaller scales share the chosen
local anchor. Targeted near crops use an oblique inspection camera, with actual
complete-oracle clearance recorded. Presentation never moves the world field.

`geometry.png` uses uniform grey independently of material under the same
recorded lighting. Lit, unshadowed, normals, height, shape, four-channel raw and
filtered material, lighting and visibility diagnostics remain available.
Director age, activity, resurfacing, retention, relief, four weights and four
process strengths each have a distinct scalar PNG. Relief maps use a fixed
white=1% reference-radius mapping rather than per-scene stretching.

The sheet/browser entry is `sheets.html`. Orbital labelled/unlabelled lit and
geometry sheets cover all twelve bodies. Per-family geometry/lit/normal/material
grids compare four provinces across all scales; cross-family geometry grids
compare every province at each scale. `sheets-audit-mapping.json` links cells
back to exact source PNGs and metadata. Original unbiased regional/near sheets
remain present.

The paired native `ai-check-review-source/earth-orbit.png` and JSON are inspected
separately. The snapshot reports RX 9070 XT / Vulkan, ready=true,
quality_pending=true, settled=false and source/ready LOD 1 versus desired 14.
This is the legacy Earth path. It supplies developer regression evidence,
not settled successor-family rendering or native interaction acceptance.

## Visual assessment

OBSERVED in the frozen review source's individual geometry PNGs: there are real
family process differences at 256 m, but they do not consistently survive into
the smallest or unbiased near views. These are subjective capture observations,
not a blinded recognition study or user acceptance.

| Family | Convincing observations | Weakness and near identity |
| --- | --- | --- |
| Rocky | Ancient-highland crops show crater/rim relief most clearly at 2 km and 256 m; basin margins have stronger directional lineaments; resurfaced plains are smoother with subdued impacts. | Province differentiation is modest, and structural uplands overlap basin-margin appearance. The 20 km views are muted. Several 32 m crops and the unbiased standing-height view show shallow marks or nearly featureless undulation. Near-scale rocky recognition is not established. |
| Icy | The fracture province's 256 m view has unequal, intersecting curving shoulders and troughs, independent of blue materials. The selected 32 m fracture view and unbiased near view retain some channels. | Old/young ice and the small reworked-zone crops can be smooth; the reworked region does not consistently read as visibly chaotic blocks. Rounded relief can still look melted/noise-like. Selected fracture recognition is stronger than general near recognition. |
| Volcanic | The 256 m construction/emplacement crops contain terrace-like fills and fronts distinct from the deeper icy troughs. They exist in grey geometry independently of red materials. | Large constructional relief is muted under the matched light. Several 32 m crops and the unbiased near view are dominated by gentle slopes. Emplacement edges alone do not reliably communicate volcanic geology; strong near identity is not established. |

FAILED / OPEN: Gate E is not accepted. The field is less uniformly featureless
at selected intermediate scales, but the contract's consistent geometry-only
regional/near recognition criterion remains unmet. Targeted camera/landmark
selection is evidence of local forms, not permission to discard the weak
unbiased views or reinterpret the gate as numerical nonzero relief.

OBSERVED in `orbital-contact-unlabelled.png` and
`orbital-geometry-unlabelled.png`: rocky impacts, narrow icy lineaments and
rougher volcanic construction/emplacement give different orbital patterns.
Old rocky seeds 1/3 are substantially more cratered than 0/2; volcanic seeds
2/3 are rougher than 0/1. Some rocky/volcanic outlines and quiet lit hemispheres
remain similar. Colour increases row readability, while grey geometry is muted.
Gate D remains PARTIAL / OPEN for user review, rather than an unconditional pass.

OBSERVED: there is strongest within-body province separation in the rocky
cratered-versus-plains crops and the icy fracture-versus-young/old crops.
Volcanic province separation is weak in grey geometry; several rows share a
similar emplacement contour texture. Province/control maps show the intended
spatial histories, but coloured weight regions alone cannot prove visible
province diversity. The within-body visual criterion therefore remains PARTIAL.

Final inspection includes the two unlabelled orbital sheets, all three four-scale
province geometry sheets, cross-family 256 m/32 m geometry sheets, age/province
director sheets, selected lit/normal/material grids, individual near PNGs and
the paired native Earth capture. File/dimension audit covers the complete
package; this list describes actual visual inspection rather than claiming
every diagnostic PNG received individual visual acceptance.

## Known failures

User visual acceptance remains OPEN. Native continuous approach, interaction,
successor-family GPU rendering and production performance remain UNTESTED in
this phase. Sampled mesh residuals are diagnostics, not certified approximation
bounds or proof that unresolved fine detail is represented at orbital scale.

FAILED in the first final quality matrix: release app test
`worker_source_charge_survives_cancellation_and_cover_abandonment` expected
construction `Some(12)` and observed `None` at `adaptive.rs:1939`.
The app worker source is unchanged versus the initial file fingerprint. A focused
release reproduction passes (1/1); the full release retry also passes. Both are
separate evidence, not proof the intermittent issue is fixed.
INFERRED: the background worker can finish between the saved pending-job state
and the observer update, making the assertion timing-sensitive. This is not
established as the cause or repaired by this terrain slice. Preserve this known
intermittent failure even if a subsequent full release run passes.

The initial director prototype produced flat local terrain and is superseded.
Later probes exposed shallow regional budgets and steep-wall landmark selection;
both were revised before the frozen review source. The interrupted
`reference-final-384-intermediate/` is incomplete, has exit −1 in its process
log, and is not final evidence. Earlier directories containing `final` in their
name do not supersede `reference-review-384/` and `frozen-source-final/`.
All intermediate evidence is retained.

## Evidence

| Evidence | Path under `target/terrain-redesign/slice1b1/` |
| --- | --- |
| Initial state/preservation | `baseline-status.txt`, `baseline-inputs.json`, `preservation-audit.json` |
| Frozen build inputs | `frozen-source-final/`, `final-source-inputs.json` |
| Retained executables | `surface_family_reference-review.exe`, `moon_surface_reference-review.exe`, `review-executables.json` |
| Final review captures | `reference-review-384/index.html`, `sheets.html`, `manifest.json` |
| Numerical replay | `directed-review-corpus-replay/surface_queries.json`, `historical-replay-result.json`, `moon-replay-results.json` |
| Completeness/measurements | `reference-review-384/package-audit.json`, `metrics-summary.json`, `artifact-hashes.json`, `reference-review-384-process/measurement.json` |
| Final quality | `full-validation-review-source/validation.json`, `full-validation-release-retry/validation.json`, `reference-focused-tests-review.txt`, `release-flake-repro-review.txt` |
| Native fast check | `ai-check-review-source/validation.json`, `earth-orbit.png`, `earth-orbit.json` |

## Git state

Branch `main`, HEAD `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d` with extensive
pre-existing dirty/untracked work. The source input manifest and frozen copies
identify the dirty build; HEAD alone does not. The task remains uncommitted.
No commit or push was requested or performed.

## Reviewer follow-up

Inspect the final neutral and unlabelled sheets, the per-family four-scale
geometry grids, original unbiased crops and their metadata. Compare actual
forms with the contract and this report's scoped observations. The package is
for user review within Slice 1B.1; no result authorizes starting Slice 2.
