# Slice 1 orbital morphology refinement

Implementation/evidence checkpoint: 2026-10-06, following the user's 2026-10-05
visual target and moon-variety clarification.

## Goal

Implement the user's orbital morphology target within the approved
[Slice 1 contract](PLANET_TERRAIN_SLICE_1.md): an ancient heavily cratered airless
body, rather than a smooth sphere with isolated bowl stamps. The supplied images
are readability references, not layouts or color/composition templates. User
visual acceptance remains required.

The subsequent clarification requires substantially different moon families.
That requirement is recorded in the [redesign](PLANET_TERRAIN_RENDERING_REDESIGN.md).
This implementation covers the cratered family; irregular, icy, volcanic and
atmospheric families are not implemented or claimed from it.

## Result

**PARTIAL.** The separately versioned V2 field and denser CPU reference are
implemented. Numerical/quality verification and visual observations are recorded
below separately. Convincing morphology across scales and user visual acceptance
remain open. This does not authorize Slice 2.

## Architecture changes

IMPLEMENTED: `MoonLikeV2` adds ten descending-scale impact epochs, two independent
rotated/translated Cartesian layouts per epoch, fewer dominant large impacts,
age-ordered bounded overprinting,
asymmetric degraded floors/walls/rims, ejecta structure, coarse-to-fine highland
ridges and correlated smoother plains. Narrowed excavation influence preserves
older neighbouring rims outside younger excavation; the compact fringe blends
back into them. Relief is concentrated in orbitally readable scales while fine
impacts remain in the complete field. Gradients include contextual masks,
warping, smooth support and composition derivatives.

The field remains deterministic, body-fixed and f64, independent of chart,
camera, footprint, workers and render resources. Features have bounded support
and a fixed one-cell halo, without a whole-body crater catalogue. The V2 envelope
uses basin + twice highland + eight times major + 2.25 times regional + local
relief, plus 1.5 m regolith. Each impact profile is bounded below its epoch
budget, and overprinting is convex. These are absolute-height bounds, not local
derivative or rendering-error certificates.

`MoonTerrainDefinition::new` still selects V1; explicit `with_version` selects V2.
V1 configuration, identity salt, profiles, query path and bound are preserved.
The native production path, `Body::terrain()`, navigation and collision have not
migrated to either new family definition.

IMPLEMENTED: the temporary app reference uses displaced triangles for normals
and f64 ray/triangle shadows accelerated by a median-split BVH. It retains
unshadowed illumination, shadow visibility, complete analytic normals and raw
materials as separate diagnostics. Coarse materials use a labelled 2×2 oracle
footprint average. The illustrative palette distinguishes highlands and basalt;
it is not calibrated lunar reflectance or the reference image's enhanced colors.
Local shadow casters cover only their mesh crop. No production shadow, GPU
residency, adaptive LOD or streaming system was added.

## Files changed

- `crates/world/src/terrain/moon.rs`: explicit version selection and V2 dispatch.
- `crates/world/src/terrain/moon/ancient.rs`: complete V2 impact/history field.
- `crates/world/tests/terrain_moon.rs`: V2 repeatability, bounds, finite-difference
  gradients and canonical edge/corner checks; existing V1 tests retained.
- `crates/app/examples/moon_surface_reference.rs`: selectable versions, denser
  orbital fixture, diagnostics, palette, mesh shadows and evidence settings.
- `crates/app/examples/moon_surface_reference/mesh_shadow.rs`: f64 triangle BVH
  and focused visibility tests, including accelerated versus exhaustive queries.
- Slice contract, redesign, reports, README, architecture and reviewer context:
  scope, reproduction, family diversity requirement and dated evidence index.

Pre-existing developer-interface and crater-experiment edits are preserved.

## Tests

VERIFIED: all 13 named quality stages have passing effective results. Seven
affected stages passed again after the source freeze: formatting, workspace
check, both strict Clippy configurations, debug/release workspace tests and
rustdoc. The six unaffected long-orbit, developer-bridge and native/developer
capture checks retain their initial passing results. This is a combined evidence
matrix, not a claim that the initial full run passed. `effective-validation.txt`
maps each stage to its log.

The initial full run exited 1 with nine passes and four failures. Both Clippy
issues were corrected; the derivative investigation is recorded below. The
existing worker-cancellation assertion passed on the release retry without an
app-source change. Its initial failure remains unexplained; a passing retry does
not establish that the underlying intermittent failure was fixed.

The locked final-source focused checks passed: reference example 14/14, world
library 24/24 and `terrain_moon` integration 8/8. Commands used the isolated
`target/terrain-redesign/slice1/validation-build` Cargo target so existing running
development services could retain their executable locks:

```powershell
$env:CARGO_TARGET_DIR='C:/Users/senne/Documents/GitHub/mundaris/target/terrain-redesign/slice1/validation-build'
./scripts/validate.ps1 -OutputDirectory target/terrain-redesign/slice1-orbital/full-validation -IncludeGpu
./scripts/validate.ps1 -OutputDirectory target/terrain-redesign/slice1-orbital/full-validation-retry -Only format,workspace-check,clippy-all-features,clippy-default,tests-debug,tests-release,rustdoc
cargo test --locked -p mundaris_app --features terrain-capture --example moon_surface_reference
cargo test --locked -p mundaris_world --lib
cargo test --locked -p mundaris_world --test terrain_moon
```

Focused tests cover both versions, independent derivatives at walls/support,
expanded-halo comparisons, contextual grid-plane/overlap derivatives, radius and
configuration failures, normalized materials and canonical chart reuse.
Reference tests cover projection, depth, clipping, seam normals, material
filtering, direct ray cases and multi-node BVH traversal versus exhaustive tests.

VERIFIED: the matched V1 seed-2 replay contains 165 cases whose complete parsed
JSON case fields match the historical corpus using the same serialization.
The source diff independently preserves the V1 numerical path. This replay is
not an exhaustive compatibility proof over all possible queries.

The final V2 query corpus is the numerical checkpoint for later tile reuse.
Subsequent complete-field changes require a new version/corpus; changing a
derived grid, filter or lighting fixture must not reinterpret this checkpoint.

VERIFIED: the repeated seed-2 near fixture produced identical SHA-256 hashes for
all nine PNGs and matching parsed fields for all 165 seed-2 query cases.
`final-build-input-check.json` records zero changed inputs across the 244 frozen
build inputs. These checks establish reproducibility of the named fixture only.

MEASURED: the isolated historical-field investigation reproduced the original
gradient failure exactly at direction index 44, seed 7, radius 1,737,000 m.
At finite-difference step 1e-8, absolute error was 1.22448 m per unit direction;
at 2.5e-9 it was 0.04937, inside the unchanged 0.80747 tolerance. The finer-step
series resolves the coarse-step truncation, with increasing cancellation at the
smallest steps. Final integration checks use 1e-9 and 5e-10, retaining the original
tolerance and requiring agreement between estimates. The historical harness is
ignored evidence only; production numerical code was not changed to make that
derivative failure disappear. See `fd-investigation/final-source-fd-series-output.txt`
and its input hashes/reproduction notes.

## Measurements

Reference metadata records per-scene query counts, material-filter and mesh/BVH/
raster wall times, sampled height ranges, quantization and sampled triangle-centroid
reconstruction residuals. They are single software-reference observations on a
busy machine, not native FPS, GPU timings, performance improvements or certified
error bounds. Fixed-grid spacing does not prove convergence or resolve all fine
impacts; complete analytic normal diagnostics can alias sub-grid detail.

MEASURED, seed 2 final orbit: 3,548,166 vertices, 7,077,888 triangles, approximately
283.85 m nominal spacing; sampled centroid radial reconstruction residual RMS
5.10 m and maximum 23.01 m across 512 deterministic samples. These sampled errors
are not conservative certificates. The sampled vertex range is approximately
-758.66 to 495.83 m, within the declared ±2761.16 m envelope.

## Captures

Final package: `target/terrain-redesign/slice1-orbital/orbital-reference-final/`, with
orbit/regional/near views for seeds 2, 7 and 19, nine diagnostic PNGs per scene and
495 complete queries across the three reference radii. Reproduce in a new directory:

```powershell
cargo run --locked --release -p mundaris_app --features terrain-capture --example moon_surface_reference -- target/terrain-redesign/slice1-orbital/reference-reproduction --orbit-cells 768
```

The orbit uses six complete cube-face grids, approximately 283.85 m nominal
spacing at 109 km radius, and a 960×640 output. Local grids remain 384 cells per
edge. Camera/light settings match across seeds; near eye clearance is 4 m above
the complete field. Local views use higher sunlight for morphology inspection;
the orbital grazing-light fixture remains fixed. This is a reference fixture, not a native navigation
acceptance route. CPU rays test represented triangles with a 1 cm lightward origin
offset and a 0.00001 m positive-hit threshold.

Matched V1 seed-2 orbital package: `v1-matched-reference/`, with the same orbital
grid/camera/light/palette/shadow method. `v1-query-replay.json` records its comparison.
The V1 terrain is unchanged by the later V2 phase-constant lint correction.

The production fast check is `ai-check-final/`: PASS, paired schema-5 PNG/JSON
inspected. At update 64 it is ready, `quality_pending=true`, `settled=false`, source
radial LOD 1 versus desired 5. It samples the existing legacy terrain, not V2;
it preceded the final V2 morphology tuning and is legacy regression evidence only.
No new native interactive evidence was collected.

## Known failures

- Visual acceptance remains OPEN and belongs to the user. Tests and denser meshes
  do not establish a convincing ancient cratered moon.
- OBSERVED, seed 2: broad smoother regions and dense cratered highlands now break
  up the earlier uniform carpet. Basin/plains outlines remain soft, and many
  craters still appear more regular than the supplied reference. The terminator
  remains a narrow jagged shadow band. Stronger degraded overlap readability is
  still needed before accepting the morphology target.
- OBSERVED, seed 2 local fixtures: the fixed +Z crop lies in a smoother region.
  Higher sunlight removes the previous darkness, but regional crater contrast
  remains faint and the near crop has no clearly visible crater interior. These
  views are not convincing close-crater acceptance evidence.
- OBSERVED: all 81 diagnostics were inspected through nine contact sheets, with
  seed-2 and seed-7 lit views also inspected at full resolution. The smoother
  seed-19 local crop is also subdued; seed-7 regional rings are clearer. This
  broader inspection does not close the orbital or close-crater visual gates.
- Sub-grid relief and fine shading can still alias; no whole-view convergence or
  certified representation residual is claimed.
- Local crops omit external shadow casters. The ray reference does not establish
  production shadow correctness or cost.
- The current lunar radial envelope and three material channels do not establish
  irregular, icy, volcanic or atmospheric moon-family capability.
- Initial strict Clippy failed on uneven hex grouping and approximate TAU literals.
  Grouping was corrected without numerical change; TAU corrected the V2 phase
  range slightly. Pre-correction and interrupted captures remain intermediate
  evidence and are not the final frozen query corpus.
- The initial mixed-source matrix failed a derivative check in an intermediate
  V2 field and an existing app worker-cancellation test. Final retries are
  recorded separately. Derivative checks now use two finer finite-difference
  steps with the original tolerance and check agreement between those steps.
  The cancellation failure must not be dismissed as an environment issue.

## Evidence

All paths below are relative to `target/terrain-redesign/slice1-orbital/`:

- `orbital-reference-final/`: final images, metadata, index, manifest and V2 corpus.
- `frozen-source/`, `frozen-source-hashes.json`, `frozen-build-inputs.sha256.json`: exact final field/test/
  reference sources and capture executable retained with SHA-256 fingerprints.
- `full-validation/`, `full-validation-retry/`, `effective-validation.txt`:
  initial quality matrix, frozen-source retries and effective 13-stage mapping.
- `focused-final/`: final-source 14/24/8 focused test logs.
- `capture-completeness.json`, `final-scene-summary.json`, `contact-sheets/`:
  package counts, scene measurements and all nine diagnostic contact sheets.
- `final-near-replay/`, `final-replay-check.json`, `final-build-input-check.json`:
  repeated fixture hashes/query comparison and unchanged frozen build inputs.
- `ai-check-final/`: paired production capture/snapshot and fast-check manifest.
- `fd-investigation/`: isolated historical source, independent step-size sweep,
  exact failure reproduction, commands and hashes.
- `v1-matched-reference/`, `v1-query-replay.json`: matched V1 fixture and replay.
- `initial-git-status.txt`, `head.txt`, `final-git-status.txt`: dirty input state.
- `reference-orbit.png`, `reference-detail.png`: supplied visual references.

`probe-256`, `probe-512`, `ray-probe-256`, `ray-probe-512`, `morph-probe-512`,
`final-orbit-512`, interrupted `final-reference`, superseded `reference-final`,
and `structured-probe-*` are tuning intermediates with
different field/shadow settings. They do not establish final-source acceptance.
The [initial V1 report](PLANET_TERRAIN_SLICE_1_REPORT.md) remains historical.

## Git state

Uncommitted work on `main`, HEAD `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`, with
extensive pre-existing dirty/untracked work. The retained source/executable
fingerprints identify this dirty build; HEAD alone does not. No commit or push.

## Reviewer follow-up

Inspect the actual orbit/regional/near diagnostics and paired settings against
the supplied morphology target. Check source, derivative/bound tests and quality
logs independently. Discuss remaining visual gaps and the next distinct-family
contract with the user; do not automatically advance GPU/LOD or claim all moons
implemented from the cratered prototype.
