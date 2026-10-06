# Planet terrain redesign — Slice 1B.2 report

## Goal

Complete the last planned procedural-authoring pass before Slice 2: deterministic,
context-related regional, local and fine geometry for rocky, icy and volcanic
bodies, with meaningful family identity at 256 m, 32 m and 8 m. The user remains
the visual acceptance authority. The contract is [Slice 1B.2](PLANET_TERRAIN_SLICE_1B_2.md).
No Slice 2 renderer, streaming, adaptive-LOD replacement or micro-detail renderer
is part of this work.

## Result

**PARTIAL — implementation and evidence package complete; the 8 m visual target
is not fully met.** Successor authority and diagnostics are IMPLEMENTED. Current
and historical replay, the complete capture audit and all thirteen quality stages
are VERIFIED. Neutral geometry shows family identity at 256 m and generally at
32 m. At 8 m, rocky terrain remains too soft for convincing impact-derived
recognition; icy and volcanic profiles retain some channels, shoulders and fronts
but also remain rounded. These are partial/failed visual gates, not a full PASS.

**Recommendation: A — READY FOR SLICE 2**, as a technical direction under the
contract's final gate and stop condition. The multi-scale field and bounded
composition work; no concrete representation or architectural blocker was found.
The remaining weakness is fine-process authoring, particularly rocky detail.
This recommendation does not accept the unmet visual criteria or replace the
user's visual judgment. No Slice 2 work has started.

## Architecture changes

**IMPLEMENTED:** `RockyV5`, `IcyV3` and `VolcanicV3` retain the corresponding
province field (`RockyV4`, `IcyV2`, `VolcanicV2`) with the explicit successor
phenotype, seed and radius. Complete height is inherited height plus three
bounded residuals. The existing shape, material-channel formats and atmosphere
descriptor keep their ownership. Historical version dispatch and formulas remain
available; successor material choices cannot change geometry.

The new cell edges are 256 m, 32 m and 8 m. Their height budgets are the lesser
of 30.72 m / 3.84 m / 0.96 m and
`radius * relief_fraction * 1.2 * [0.16, 0.085, 0.05]`. Each compact profile is
bounded by one, accumulated with a cubic compact window and divided by
`1 + sum(windows)`. A bounded family process network supplies relief between
compact supports; compact and network contributions are averaged. The total
height envelope adds the three budgets to the preserved parent envelope and
rounds outward. This is a height bound, not a newly claimed production LOD-error
certificate.

Two translated cell layouts each visit a fixed 3×3×3 halo per band: 54 visits
per band, 162 additional visits per complete query. Feature centres have at most
0.16-cell component jitter and 0.30-cell radial shell projection. The support
radius is 0.52 cells; `1 - 0.16 - 0.30 = 0.54 > 0.52` bounds the search. There
is no planet-wide catalogue of fine features. Immutable centre-sampled controls
modulate feature amplitudes; material, camera, worker order and mesh resolution
are absent from geometry inputs.

The parent morphology signal sums signed physical contributions from the first
three inherited compact-process levels, normalized by their physical budgets.
It excludes the broad elevation datum. Local detail inherits this signal and
the normalized regional residual; fine detail also inherits the normalized local
residual. Regional lineage keys correlate child orientations. Analytic derivatives
include compact windows, profile algebra, normalization, controls and inherited
context. Parent controls and their derivatives are reused at the same query,
while feature-centre controls retain their own location.

The final fine continuous processes use a 2 m sub-feature wavelength inside the
8 m cell regime. Rocky joints have unequal breccia lips, icy stress troughs have
unequal pressure shoulders, and volcanic fronts have a pressure lip and trailing
channel. Icy fine contrast uses `tanh(1.65 * profile) / tanh(1.65)` for a profile
bounded in `[-1, 0.82]`; it stays in `[-1, 1]`. The volcanic fine profile is bounded
in `[-0.64, 0.92]`. Process, resurfacing and lineage gains remain bounded by one.
The fixed fine budget and height envelope therefore remain unchanged. A keyed
comparison verifies exact inherited/regional/local/context/work values in all
7,085 v3/v4 records; query-record ordering changed, so comparing by row index was
incorrect and its failed audit is retained separately.

Rocky regimes progress from complex impact/rim structure through local bowls and
ejecta to fractured breccia. Icy regimes use troughs, unequal shoulders, crossing
faults and chaos. Volcanic regimes use emplacement fronts, pressure structures
and construction/collapse. The implementation decision is
[ADR 0011](adr/0011-hierarchical-geological-residuals.md).

`SurfaceGenerator::detail_diagnostics` exposes inherited, regional, local and fine
height/gradient contributions, parent morphology and derivative, parent process
strengths, and per-band work. These are derived from the authoritative evaluation.
The rotated reference adapter transforms all derivatives into the capture chart.

## Files changed

Task changes are recorded against the startup file hashes, rather than treating
the repository's broad pre-existing dirty state as this task's diff:

- `crates/world/src/terrain/surface.rs`: successor dispatch and diagnostics.
- `crates/world/src/terrain/surface/hierarchy.rs`: bounded geological hierarchy.
- `surface/provinces.rs`: extract parent context without changing historical
  height accumulation; `surface/geology.rs`: exhaustive successor dispatch.
- `crates/world/tests/surface_hierarchy.rs`: public numerical contracts.
- `crates/app/examples/surface_family_reference.rs` and its `provinces.rs`:
  explicit hierarchy captures, replay corpora and diagnostic maps.
- `moon_surface_reference.rs` and `surface_adapter.rs`: shared capture names and
  rotated diagnostic forwarding; historical defaults preserved.
- `crates/app/examples/surface_hierarchy_cost.rs`: matched parent/successor cost
  observations; `scripts/surface-family-sheets.py`: labelled comparison package.
- README, architecture, contract, ADR, this report and reviewer context: scope,
  reproduction and dated evidence orientation.

## Tests and numerical evidence

Evidence root: `target/terrain-redesign/slice1b2/`. Focused locked checks on the
frozen v4 candidate passed seven internal hierarchy tests and eight public
hierarchy tests (`v4-hierarchy-internal.txt`, `v4-hierarchy-public.txt`). They cover deterministic scalar/batch/
thread/order queries, component recomposition, canonical chart boundaries,
analytic derivatives, support boundaries, conservative envelopes at several
radii, material/atmosphere independence and process suppression.
The `v4` evidence label denotes the fourth review build, not a common terrain
version: its algorithms are `RockyV5`, `IcyV3` and `VolcanicV3`.
The public recomposition check tolerates less than `1e-10 m` height difference
and `1e-7 m per unit direction` gradient difference. Exactness claims below refer
to complete replay records and preserved parent values, not that tolerance check.

**VERIFIED:** The frozen final candidate's two independent complete hierarchy
corpora match exactly: 7,085 records including geometry, gradients, materials,
controls, work and decomposition. The historical family corpus (2,577 records),
province corpus (3,601 records), and MoonLikeV1/V2 corpora (165 records each)
match their preceding evidence exactly. The full capture corpus also matches the
independent replay. `strict-replay-audit.json` verifies complete parsed records
with exact types and IEEE-754 float bits, including signed zero; only wrapper
timings are excluded. Both retained MoonLike near PNG sets also match SHA-256
exactly. See `v4-replay-audit.json` and `fine-scope-audit.json`.

The initial complete quality matrix is preserved in `full-validation/`. Debug
workspace tests passed. Initial strict lint failed and was corrected. The release
suite's tests ran, but its app doctest failed because a referenced world RLIB was
missing during overlapping builds in the same target directory. This is a failed
run, not a passed release matrix or a proven product bug. Later candidate lint
passed both configurations in `accepted-validation/`; that matrix was explicitly
interrupted after the visual failure. **VERIFIED:** the isolated frozen-v4 matrix
passes all thirteen stages in `v4-validation/validation.json`: format, locked
workspace check, both strict Clippy configurations, debug/release workspace
tests, strict rustdoc, long-orbit tests, developer bridge, and explicit ignored
GPU checks for close surface, full frame, developer interface and scenarios.
Commands and individual outputs are retained. Native human interaction remains
separate from those automated regressions.
Both workspace profiles pass 446 tests, with nine ignored tests in their ordinary
invocation. The matrix separately runs its named ignored GPU and long-orbit
targets; it does not silently count ignored cases as passed.

## Measurements

The fixture uses Windows 11 Pro build 26200, Ryzen 7 9800X3D, Rust/Cargo 1.98.1 and optimized
software reference builds. `environment.json` records the environment. Query
cost compares matched explicit phenotypes, seeds 0/7/19, radii 80 km / fixture /
1,200 km, 4,096 fixed equal-area directions, warm-up and three alternating-order
repeats. It excludes construction, rasterization and diagnostic-map sampling.
**MEASURED:** `query-cost-v4-quiet.json` was recorded after the owned capture and
quality processes completed. Each algorithm has 27 scans of 4,096 directions.
`query-cost-summary.json` retains all summaries; these are medians, not worst-case
latency bounds. The existing unrelated developer service remained running.

| Algorithm | Median complete query, µs | Cell visits/query | Median candidates/query | Median accepted/query |
| --- | ---: | ---: | ---: | ---: |
| RockyV4 | 16.554 | 810 | 603.402 | unavailable |
| RockyV5 | 20.841 | 972 | 641.497 | unavailable |
| IcyV2 | 7.661 | 270 | 63.307 | 5.079 |
| IcyV3 | 12.038 | 432 | 101.410 | 8.127 |
| VolcanicV2 | 7.524 | 270 | 63.447 | 5.081 |
| VolcanicV3 | 11.959 | 432 | 101.547 | 8.144 |

Median paired successor/parent cost ratios are 1.260, 1.572 and 1.587 respectively.
The parent side uses the current preserved parent implementation and the same
explicit successor phenotype; this is not a historical-binary performance replay.
The hierarchy adds exactly 162 cell visits, with bounded candidates. Counters
omit noise-interpolation arithmetic; wall time includes all complete-query work.
Rocky's retained historical field does not expose accepted-feature counts, so
its complete accepted total remains unavailable rather than guessed.

The nine contribution maps supply separate per-band support observations. Across
the four selected 8 m crops per family, each sampled at 256×256, mean candidate /
accepted counts per diagnostic query are:

| Family | Regional | Local | Fine |
| --- | ---: | ---: | ---: |
| Rocky | 13.767 / 1.000 | 13.147 / 0.899 | 13.743 / 1.164 |
| Icy | 11.500 / 0.750 | 13.488 / 0.712 | 12.189 / 0.797 |
| Volcanic | 12.481 / 0.500 | 13.115 / 0.848 | 13.187 / 0.988 |

Every band visits 54 cells per diagnostic query. These local-map means are a
different fixture from the equal-area complete-query table; the continuous
process network is not counted as an accepted compact feature.

**MEASURED:** the complete software reference process takes **1,650.092 s
(27.50 min)**, exits zero and reports a **399.1 MiB** process peak working set;
200 ms sampling observes 394.3 MiB. `reference-v4-384.run.json` records binary
hash, arguments, timing and memory. Some capture time overlaps validation and
intermediate preview work. The package is larger than Slice 1B.1, so total runtime
is not a comparable performance regression measurement. Generator heap/scratch
allocation was not instrumented. No native FPS or GPU improvement is claimed.

## Captures and visual assessment

The fixed reference mesh uses 384 cells, unchanged from the preceding phase.
Five crops reuse each immutable body-fixed parent anchor at 20 km, 2 km, 256 m,
32 m and 8 m. Capture order prioritizes small acceptance views; declared scale
indices and the same anchor remain unchanged. Nominal local vertex spacing is
52.083 m, 5.208 m, 0.667 m, 0.0833 m and 0.02083 m respectively.

Neutral geometry uses constant grey material and represented mesh normals with
matched broad fill across families. Analytic normals, height, lighting, shadow,
material channels and independently mapped detail/process maps remain separate
diagnostics. A contrast-stretched height map alone cannot establish recognizable
geometry. The +Z 32 m and 8 m crops are unbiased oblique crops; the original near
view supplies the separate standing-height fixture.

**VERIFIED:** `reference-v4-384/` contains all twelve bodies, the irregular-shape
stress fixture, **106 scenes**, **3,080 scene PNGs** and **63 comparison sheets**.
There are twelve five-scale approach sequences and 36 selected 256/32/8 m scenes
with nine decomposition maps each. All 3,143 PNG encodings verify. The capture
audit checks scene coverage, 384-cell local meshes, exact immutable anchors and
configurations across all five scales, unbiased anchors, metadata, map provenance
and dimensions, and complete corpus replay. A final sheet-only correction adds
the missing fifth column to the family approach sheets; its mapping now verifies
all twenty province/scale cells per family. It changes no compiled Rust input.

Each regular orbital scene already contains matched constant-grey geometry, so
the twelve-body grey atlas uses those images. The irregular stress fixture also
retains its separate orbit-neutral scene. No redundant seed-zero neutral scene
is required. Director maps are 512×256 at orbit and 256×256 locally; the base
perspective rasters are 960×640. An audit's initial wrong universal-size assumption
and coverage assumption are retained as failed helper attempts, not product bugs.

**OBSERVED:** the final neutral 256 m sheets show rocky impact forms, icy faults
and chaos, and volcanic lobate/constructional patterns. At 32 m the families
remain distinguishable, but some panels resemble rounded pitting or meandering
ridges. At 8 m the fine profiles are visible, yet rocky terrain mostly reads as
soft undulation; some icy troughs and volcanic steps are identifiable, with
weaker family identity than at 32 m. The fixed +Z crops confirm local relief away
from selected features, but do not establish robust 8 m family recognition.

The twelve-body orbital grey atlas retains rocky impacts, sparse icy lineaments
and volcanic roughness. Several broad-fill views are muted. The lit atlas uses
materials and stronger illumination, so it cannot rescue neutral recognition.
Same-anchor approach sheets retain parent terrain and add smaller structures;
they do not establish native continuous-approach or camera interaction acceptance.

| Family | 256 m | 32 m | 8 m | Unbiased near |
| --- | --- | --- | --- | --- |
| Rocky | PASS for impact-bearing highlands; smooth plains expected | PARTIAL: pitted/fractured relief, often too rounded | FAILED convincing impact-derived recognition | Useful relief fixture; 8 m recognition FAILED |
| Icy | PASS fracture/chaos identity in stronger provinces | PARTIAL: irregular troughs/chaos, some rounded panels | PARTIAL: some channels/shoulders; robust recognition OPEN | Useful fracture/relief fixture; rounded at 8 m |
| Volcanic | PASS coherent fronts and constructional bands | PARTIAL: coherent ridges/fronts, some generic meanders | PARTIAL: low fronts/lobes, flow identity weak | Useful flow-relief fixture; recognition still weak |

These are technical image assessments, not user acceptance. Cross-family
distinction is strongest at 256 m, generally present at 32 m and partial at 8 m.
The contract's full local visual target is not VERIFIED.

**MEASURED:** maximum sampled triangle-centroid height error across each family's
four selected 8 m crops is 0.569 mm rocky, 0.617 mm icy and 0.854 mm volcanic
(`final-package-metrics.json`). This finite 512-centroid test is not a certificate
for every triangle or derivative. It nevertheless supports the inference that
rounded 8 m forms come from the field, rather than an empty/coarse reference mesh.
The 128 m standing-height ice view shows mesh/shadow facets on a steep inherited
slope; fixed finite reference representation remains separate from production UX.

The earlier broadly smooth rocky candidate is retained in `reference-accepted-384/`.
Its fine map was 0.01678–0.13529 m and sampled error 0.0000766 m. It FAILED despite
nonzero relief. Fine-profile changes improved visible detail, but the final rocky
8 m gate above remains failed; the earlier evidence is not relabelled as passed.

Start review at [the complete sheet index](../target/terrain-redesign/slice1b2/reference-v4-384/sheets.html).
Relevant neutral evidence:

- [256 m](../target/terrain-redesign/slice1b2/reference-v4-384/province-cross-family-scale-2-geometry.png),
  [32 m](../target/terrain-redesign/slice1b2/reference-v4-384/province-cross-family-scale-3-geometry.png),
  [8 m](../target/terrain-redesign/slice1b2/reference-v4-384/province-cross-family-scale-4-geometry.png).
- [Unbiased crops](../target/terrain-redesign/slice1b2/reference-v4-384/unbiased-cross-family-geometry.png).
- Same-anchor five-scale approaches:
  [rocky](../target/terrain-redesign/slice1b2/reference-v4-384/province-rocky-geometry.png),
  [icy](../target/terrain-redesign/slice1b2/reference-v4-384/province-icy-geometry.png),
  [volcanic](../target/terrain-redesign/slice1b2/reference-v4-384/province-volcanic-geometry.png).
- [Twelve-body orbital grey](../target/terrain-redesign/slice1b2/reference-v4-384/orbital-geometry-labelled.png)
  and [lit atlas](../target/terrain-redesign/slice1b2/reference-v4-384/orbital-contact-labelled.png).

Each scene's `metadata.json` provides map ranges, units, controls, work, timings
and exact anchor/configuration. Contribution maps independently stretch their
observed min/max, so compare numeric ranges rather than grayscale contrast.

## Known failures and evidence limits

The first detail-map run panicked from eager evaluation of an irrelevant process
index in metadata; lazy evaluation corrected it. Precomputed rotations enlarged
the field enum; boxing the successor field corrected strict lint. Intermediate
compiler errors, failed runs and interrupted captures remain in the evidence
root. They are not final acceptance evidence.

The final `ai-check-v4/earth-orbit.png` and JSON pair was inspected. The commands
PASS on RX 9070 XT / Vulkan, but the existing Earth scene is ready, quality-pending
and unsettled (source/ready LOD 1, desired LOD 14). This exercises the legacy native
scene and proves neither settled successor terrain nor human navigation UX.
No native FPS, GPU residency, streaming or production continuous-approach claim
is made here.

## Git state and evidence

Branch `main`, HEAD `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`, with pre-existing
dirty/untracked work. `baseline.json` records startup status and hashes;
`preservation-audit.json` finds no unrelated changes. Retained source freezes and
executable hash manifests distinguish intermediate candidates. The 240-entry
v4 build manifest contains 239 unchanged inputs and the sheet helper; only that
helper changed afterward to include the fifth approach column. Its original and
final hashes are recorded in `final-packaging-source.json`. The frozen Rust code
therefore remains the code tested and captured. Final documents are not compiled
inputs. The reference executable SHA-256 is
`74BBDA74797619D35CA8F4D86FB41E3C94BDD45B5E70E6E2A305516E65B55094`.
No task commit or push was requested or performed.

## Reproduction and evidence

The README records fresh-output capture, corpus, sheet and query-cost commands.
Validation uses locked Cargo commands, with separate target directories to avoid
overlapping builds. Final commands include:

```powershell
$env:CARGO_TARGET_DIR = 'target/terrain-redesign/slice1b2/lint-build'
cargo test --locked -p mundaris_world terrain::surface::hierarchy::tests --lib
cargo test --locked -p mundaris_world --test surface_hierarchy
$env:CARGO_TARGET_DIR = 'target/terrain-redesign/slice1b2/final-build'
./scripts/validate.ps1 -OutputDirectory target/terrain-redesign/slice1b2/v4-validation -IncludeGpu
```

All exit statuses are zero. Exact individual quality commands and outputs are in
`v4-validation/validation.json` and its logs. The focused reference example has
29 passing tests in `reference-fine-focused.txt`; the final full matrix also tests
the frozen example. Fast-check commands, statuses and pair are in `ai-check-v4/`.

Evidence index beneath `target/terrain-redesign/slice1b2/`:

- `v4-source-inputs.json`, `frozen-source-v4/`, `v4-executables.json`,
  `final-packaging-source.json`: build/package identity.
- `strict-replay-audit.json`, `v4-replay-audit.json`, `fine-scope-audit.json`:
  complete/historical determinism and preserved coarse/context values.
- `capture-audit.json`, `final-package-metrics.json`: coverage, anchors, maps,
  sheet cells, PNG integrity, sampled representation errors and local band work.
- `v4-validation/summary.json`, `validation.json` and logs: thirteen-stage matrix.
- `query-cost-v4-quiet.json`, `query-cost-summary.json`,
  `reference-v4-384.run.json`: measured work, cost, runtime and memory.
- `baseline.json`, `preservation-audit.json`: distinguish task edits from the
  pre-existing dirty tree. Intermediate failures/interrupted runs are retained.

## Reviewer follow-up

**A — READY FOR SLICE 2.** The authority supplies bounded multi-scale geometry,
correlated context and analytic derivatives; family-specific structure survives
the approach, most clearly through 256/32 m. Fine icy/volcanic forms retain some
identity. No evidence establishes a fundamental radial representation, search,
composition or continuity defect that would make renderer work premature.

**INFERRED:** the weak rocky 8 m morphology and rounded fine forms are profile/
artistic-authoring limitations within the existing field. The final images and
small sampled mesh errors support this diagnosis; they do not make the failed
rocky recognition gate pass. Refine those profiles as focused authoring work,
without starting another broad terrain architecture phase. Reviewer/user visual
discussion remains necessary. This is a readiness recommendation, not a claim
that every Slice 1B.2 criterion passed. The package is complete; stop here.
