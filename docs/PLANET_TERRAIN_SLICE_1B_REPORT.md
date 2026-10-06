# Slice 1B: compositional body surfaces and procedural moon families

Implementation and evidence checkpoint: 2026-10-06.

## Goal

Implement the approved [Slice 1B contract](PLANET_TERRAIN_SLICE_1B.md): separate
world-owned shape, geological history, materials and atmosphere; preserve the
existing MoonLikeV1/V2 numerical definitions; connect the new authority to real
bodies and clearance queries; and provide fixed-seed visual and numerical
evidence for rocky, icy and volcanic families plus an irregular shape.
The user owns visual acceptance. Slice 2 is not authorized by this handoff.

## Result

**PARTIAL.** Compositional authority and the three versioned query algorithms are
IMPLEMENTED. Visual gates remain OPEN pending review of the final package.
The existing native terrain renderer remains on its previous authority path.

| Gate | Checkpoint |
| --- | --- |
| A: compositional ownership | VERIFIED in current types and body publication tests; shape, geology, material and atmosphere ownership are separate. |
| B: distinct families | IMPLEMENTED and numerically VERIFIED as three different complete fields; convincing visual identity remains OPEN. |
| C: internal variation | VERIFIED deterministic seed/control differences in the fixed corpus and tests; meaningful visual history/diversity remains OPEN. |
| D: cross-family orbital diversity | OPEN for visual review of the unlabelled 12-body sheet. |
| E: regional/near identity | FAILED in the inspected near-view identity/readability criterion; selected features exist, but the views do not yet establish convincing family grammar at that scale. User assessment remains OPEN. |
| F: shape proof | VERIFIED bounded irregular/triaxial queries, derivatives, envelopes and production clearance; arbitrary concave/volumetric topology is explicitly unsupported. |
| G: production authority | VERIFIED real body selection, transactional publication and camera/clearance tests; matching native rendering is outside this checkpoint. |
| H: determinism/tests | VERIFIED: all 13 final quality stages pass, and exact new/legacy corpus replays pass within their named fixture scope. |
| I: user visual review | OPEN; no explicit user approval has been received. |

## Architecture changes

IMPLEMENTED: the ownership and identity decision is recorded in
[ADR 0009](adr/0009-compositional-body-surfaces.md). The concrete boundary is:

```text
CelestialSystem / CelestialBody: stable body identity and reference radius
  selected legacy terrain OR SurfaceDefinition
    ShapeDefinition: sphere / ellipsoid / bounded irregular radial graph
    SurfaceTerrainDefinition: algorithm + explicit correlated geological controls
    SurfaceMaterialDefinition: channel version + composition / contrast
    SurfaceAtmosphere: airless / descriptor
      SurfaceGenerator: complete f64 shape, displacement, derivatives, materials
        production radial clearance and camera safeguard
        disposable fixed-resolution reference geometry and diagnostics
```

`RockyV3` composes the preserved V2 crater field with a separate body history and
resurfacing mask. `IcyV1` and `VolcanicV1` have independent algorithm namespaces
and region-local feature queries. Body phenotype age, activity, resurfacing,
impact retention, relief, feature scale and orientation are correlated controls,
not calibrated geological dates or a physical process simulation. Material
channels correlate with geological features; independent composition/contrast
controls do not move geometry. Incompatible channel semantics are rejected.

Shape, geology, phenotype, province and material salts are separate. Geometry
identity excludes atmosphere and material composition. Full definition identity
includes atmosphere; it does not change the complete geometric or material
field. Runtime body/frame handles, camera, render footprint and worker order are
absent from generation salts. Hashes identify diagnostic namespaces; caches must
also compare the exact definition and reference radius.

Body publication validates before mutation, keeps legacy/compositional authority
exclusive, and advances the terrain revision without changing celestial/frame
revision or time. Radius edits validate the selected authority first. Clearance
uses combined shape radius plus relief and the combined normal. The camera cache
includes the complete definition and radius, and its compositional safeguard
uses the conservative outer envelope plus the one-metre clearance margin.
This radial safeguard is not general solid collision. Its repeated generator
construction cost has not been measured as native camera performance.

The representation is `p(n) = n * (shape_radius(n) + terrain_height(n))`, with
analytic tangent derivatives in metres per unit direction. Normal reconstruction
uses the sum of both derivatives divided by the combined positive radius.
Global shape and relief envelopes combine outwardly; they are not local tile
error certificates. The shape fixture has axes `[1.35, 0.92, 0.70]`, asymmetric
amplitude `0.16` and lobe amplitude `0.12`. It is star-shaped about the origin:
one positive radius per direction. Arbitrary concavity, caves, undercuts,
overhangs, enclosed cavities and every contact-binary topology are unsupported.
The current shape/geology validation range caps reference radius at `1e8 m`.

The app reference adapter consumes the world field. Its chart rotation forwards
queries into body-fixed directions and rotates derivatives/normals back; it owns
no geological recipe. Mesh filtering, represented-triangle normals, BVH shadows,
palette and lighting are derived presentation. Atmosphere rendering, replacement
native rendering, GPU tiles, residency, streaming and adaptive LOD are absent.

## Files changed

- `crates/world/src/terrain/surface.rs` and `surface/shape.rs`, `surface/geology.rs`:
  compositional definitions and complete shape/geology/material queries.
- `crates/world/src/body.rs`, `system.rs`, `terrain/mod.rs`: actual body ownership,
  validated publication and public API.
- `crates/app/src/terrain_inspection.rs`, `celestial_camera.rs`: shared authority
  for radial clearance, shape-aware safeguard and exact-definition cache reuse.
- `crates/app/src/gravity_orbits.rs`: surface-presence checks preserve an already
  selected compositional authority when enabling the legacy preview controls.
- `crates/world/tests/surface_shape.rs`, `surface_families.rs`, `surface_authority.rs`,
  `crates/app/tests/surface_clearance.rs`: focused numerical and integration checks.
- `crates/app/examples/surface_family_reference.rs`,
  `moon_surface_reference.rs`, `moon_surface_reference/surface_adapter.rs`,
  `crates/app/Cargo.toml`, `scripts/surface-family-sheets.py`: shared reference
  adaptation, corpus, fixtures and contact/diagnostic sheets.
- README, architecture, ADR 0009, Slice 1B contract/report and reviewer context:
  reproduction, ownership and dated acceptance/evidence boundary.

These task edits are separate from the extensive pre-existing developer-interface,
renderer, crater and Moon V1/V2 work. No commit or push is part of this task.

## Tests

Focused final-source checks passed: world library 36/36, family invariants 7/7,
shape invariants 6/6, publication authority 1/1, application clearance/camera 2/2,
legacy reference runner 17/17 and family reference runner 24/24. Strict focused
Clippy and owned-file format checks passed. The full final matrix and replay
results are recorded with the evidence package below.
Automated checks establish only their named numerical/runtime criteria; they do
not accept the visual design or native interaction quality.

VERIFIED: the fresh final matrix ran **13/13 stages, all exit 0**. This includes
format, all-target/all-feature workspace check, strict Clippy with all features
and defaults, debug/release workspace tests, warnings-as-errors rustdoc, ignored
release long-orbits, developer bridge, and the four ignored native/GPU regression
stages (`native_close_surface`, `native_full_frame`, `developer_interface`,
`developer_scenarios`). Debug took 476.936 s and release 99.273 s. The long-running
geometry-error/erosion tests completed; no service or test process was interrupted.
These automated stages do not constitute human native interaction acceptance.

```powershell
$env:CARGO_TARGET_DIR='C:/Users/senne/Documents/GitHub/mundaris/target/terrain-redesign/slice1/validation-build'
./scripts/validate.ps1 -OutputDirectory target/terrain-redesign/slice1b/full-validation-final -IncludeGpu
./scripts/ai-check.ps1 -OutputDirectory target/terrain-redesign/slice1b/ai-check-final-source -Scene earth-orbit
```

The isolated target directory avoids the pre-existing live service executable
locks; those unrelated services were preserved. `full-validation-final/validation.json`
records exact commands, start times and durations. The paired native developer
snapshot identifies **AMD Radeon RX 9070 XT / Vulkan**; this is the observed local
adapter, not a portability claim or compositional GPU rendering proof.

The native developer evidence loop passed at `ai-check-final-source/`, with
paired `earth-orbit.json` and `earth-orbit.png`. OBSERVED: this legacy Earth scene
reports `ready=true`, `quality_pending=true`, `settled=false`, source/ready radial
LOD 1 versus desired 14 and nine source leaves. Its warnings are
`terrain_quality_pending` and `source_below_desired`. It establishes a working
diagnostic/capture path; it does not demonstrate new-family native rendering or
settled terrain.

## Measurements

Reference query work and wall times belong to fixed software fixtures, not native
FPS or GPU speedups. Windows process peak working set is sampled for the complete
reference capture process; its mesh, raster and shadow allocations are included.
It is not a query scratch measurement, allocation census, process RSS time series
or driver VRAM measurement. Inline Rust type sizes are recorded separately and
are not total memory. Capture may overlap validation, so wall times are observations
under that load, not clean benchmarks or speedup claims.
Rocky accepted-feature counts are unavailable from the preserved V2 oracle and
are represented as null, never as zero.

MEASURED on the final 2,577-record corpus:

| Algorithm | Cells/query | Candidate features/query | Accepted support centres/query |
| --- | ---: | ---: | ---: |
| RockyV3 | 540 | 540 | unavailable |
| IcyV1 | 108 | 1–18 | 0–5 |
| VolcanicV1 | 216 | 8–40 | 0–8 |

Accepted counts describe centres passing spherical support, not guaranteed
nonzero profile values. Cell work is bounded independently of reference mesh
resolution; it is not a measured asymptotic runtime guarantee. Query counters
include the explicitly documented world traversal only. Mesh-vertex work is
reported separately from filtering, centroid, camera and landmark queries.

The 384-cell orbit/local run took **574.231 s** end to end and exited 0 on an
AMD Ryzen 7 9800X3D, Windows x86_64, Rust 1.98.1 release build. Maximum reported
process `PeakWorkingSet64` was **413,143,040 bytes (394.00 MiB)** over 564 one-second
polls. This is one contended software-reference run, with final validation running
concurrently. `reference-process-measurement.json` records the exact scope.

| Family | Orbit mesh/query time, four bodies | Regional mesh/query time | Near mesh/query time |
| --- | ---: | ---: | ---: |
| Rocky | 39,058.904–47,746.920 ms | 5,926.829–6,626.346 ms | 1,091.424–1,205.987 ms |
| Icy | 12,278.200–12,655.269 ms | 1,936.463–2,203.690 ms | 401.207–435.595 ms |
| Volcanic | 13,068.982–13,513.133 ms | 2,036.077–2,485.976 ms | 413.474–474.733 ms |

These phase times include complete queries and presentation footprint filtering;
they are not single-query latency. Separate raster times and every scene's
512-centroid representation residual are in `analysis.json` and
`metrics-summary.json`. The greatest sampled orbital residuals were 165.972 m
rocky, 9.501 m icy and 28.478 m volcanic; regional maxima were 4.576/6.525/9.653 m
and near maxima 0.061860/0.000332/0.000217 m respectively. Small residuals on
almost planar near crops do not establish useful morphology or LOD convergence.

## Captures

The package retains unbiased +Z regional/near views for every body, neutral orbit
views, complete height/shape/material diagnostics, represented and analytic
normals, unshadowed illumination and shadow visibility. Additional geological
landmark views are explicitly selected diagnostics, not replacements for weak
unbiased views. Local shadow casters cover only the crop. Fixed-grid residuals
are sampled measurements, not conservative convergence certificates.

Height and shape grayscale ranges are normalized per scene and recorded in its
JSON; sheet brightness alone cannot compare absolute relief between bodies.

The full fixed package has 49 scenes: twelve bodies at orbit/regional/near,
three first-seed neutral orbital views, six first-seed geological landmark views,
and four irregular-shape stress views. Each scene has twelve PNG diagnostics,
including separate fourth-channel material images. The contact-sheet population
is seeds 0/1/2/3 in each family with radii 80/109/180/260 km respectively. This
intentional fixed fixture selection is reproducible, but radius and seed vary
together, so visual differences are not an isolated seed experiment.

Landmark selection checks 64 fixed Fibonacci anchors in order, stops at the first
world-owned feature-probe set and chooses its greatest terrain-gradient probe.
Rocky has no such feature API and uses a documented 1024-direction maximum-gradient
fallback. Selection/query cost and labels are recorded. Local lighting stays fixed
in capture-chart axes, with its true body-frame direction recorded; selected
features cannot disappear solely because their location lies on an unlit body
hemisphere. These selected views do not establish global feature coverage.

No external textures were required or imported. Palette and four material weights
are derived from the world query; texture availability does not resolve weak
geometry or morphology.

VERIFIED package completeness: 49 metadata scenes, all 588 expected per-scene
PNGs and 52 additional comparison-sheet PNGs, with no missing scene images.
`package-audit.json` records feature selection: icy first anchor 0/shoulder-a,
volcanic anchor 5/caldera-rim, and the rocky fallback. The query corpus contains
16 boundary and 16 landmark audit records; 10 landmark records find active icy
or volcanic features. These include repeat 80 km first-seed probes, so counts
are audit records rather than 16 distinct bodies. Rocky/irregular records are
expected to report no geological landmark API.

OBSERVED: the final neutral irregular orbit has a visibly elongated, asymmetric
silhouette through the same complete query and reference adapter. The height-only
and analytic-normal sheets show different numerical fields across the families,
while the lit unlabelled sheet retains strong visual similarity within each row.

## Known failures

Visual gates D/I remain OPEN and the inspected near-view portion of E is FAILED.
The intermediate probe showed soft rocky crater
readability, overly regular icy orbital bands and nearly flat icy/volcanic near
views. That evidence motivated the bounded fracture/emplacement iteration; it
must not be treated as the final numerical checkpoint.

OBSERVED in final first-seed captures: rocky orbital relief remains soft and the
regional view reads as repetitive granular relief; the unbiased rocky near view
is muted. Icy orbital/regional material patches repeat as conspicuous block-like
shapes. The selected `icy-fracture-epoch3-shoulder-a` is visible as a curved crease
in lit and analytic-normal near diagnostics, but relief/readability is still weak;
the unbiased icy near view is almost planar. These are unresolved morphology and
presentation limitations, not acceptance established by the feature tests.

OBSERVED in final first-seed volcanic captures: orbital/regional ellipsoidal
shields and calderas remain dominated by repeated stamp-like patches. Both the
unbiased near view and the selected `volcanic-epoch2-caldera-rim` near view are
visually almost uniform sloping planes; the analytic-normal diagnostic records
variation but the lit view does not convey convincing volcanic surface identity.
This explicitly fails the inspected near-view portion of Gate E.

The final package requires user assessment of convincing individual bodies and
diversity across worlds. Passing tests or a selected landmark do not close those
gates. The next morphology work, if requested after review, remains in Slice 1B.

No matching native compositional terrain rendering, general collision, native
control-feel acceptance, GPU speedup or atmospheric rendering is claimed.

## Evidence

All new raw evidence is rooted at `target/terrain-redesign/slice1b/`.
`initial-git-status.txt`, `initial-input-hashes.json` and `head.txt` identify the
starting dirty checkout. Frozen build inputs, executable fingerprints, validation
logs, final captures and query replay checks identify this implementation rather
than HEAD alone. Historical Slice 1/1-orbital evidence remains in place.

| Final evidence | Path below `target/terrain-redesign/slice1b/` |
| --- | --- |
| Exact build inputs | `frozen-source-final/`, `frozen-build-inputs-final.sha256.json` (233 inputs) |
| Frozen executables | `surface_family_reference-final.exe`, `moon_surface_reference-final.exe`, `frozen-executables-final.sha256.json` |
| Final captures and browse index | `reference-final-fixed-384/index.html`, `sheets.html` |
| Orbital review | `reference-final-fixed-384/orbital-contact-unlabelled.png`, `orbital-contact-labelled.png`, `orbital-contact-mapping.json` |
| Complete corpus | `reference-final-fixed-384/surface_queries.json`, `reference-corpus-replay/surface_queries.json`, `replay-check.json` |
| Completeness and measurements | `reference-final-fixed-384/package-audit.json`, `analysis.json`, `metrics-summary.json`, `reference-process-measurement.json` |
| Artifact fingerprints | `reference-final-fixed-384/artifact-hashes.json` |
| Native paired developer loop | `ai-check-final-source/earth-orbit.json`, `earth-orbit.png`, `validation.json` |
| Full matrix | `full-validation-final/validation.json`, per-stage `.txt`, `full-validation-final-run.log` |
| Preservation and final state | `preservation-check-final.json`, `preserved-moon-inputs.json`, `final-git-status.txt`, `git-diff-check-final.txt` |

VERIFIED replay: the entire parsed new corpus (definitions, boundary/landmark
audits and 2,577 complete shape/height/gradient/normal/material/work records) is
identical between the full run and `--corpus-only` from the same frozen executable.
Outer run timings are excluded. Coverage includes canonical face corners/edges,
interiors, multiple radii, sampled extrema and actual feature support/core probes.
This is exact evidence for the named corpus, not exhaustive proof over all inputs.
The two historical MoonLikeV1/V2 seed-2 replays are also identical for 165 complete
cases per version; their preserved source/test hashes remain unchanged.

The earlier `reference-probe-192/`, `reference-final-384/` and `frozen-source/`
packages are intermediate/candidate evidence. The first source-overlapping full
matrix ran all 13 stages with 3 passes and 10 failures during field edits; it is
retained at `full-validation/` and does not identify final-source acceptance.
Use only the `-final` frozen inputs and `reference-final-fixed-384/` for this
checkpoint. No repeated-PNG identity claim is made.

## Git state

Branch `main`, HEAD `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`, uncommitted task
changes over a broadly dirty baseline. No commit or push was requested or made.

VERIFIED: `git diff --check` exits 0. Hash comparison of 360 recorded initial
paths finds no missing paths or unexpected edits; twelve changed recorded paths
are explicitly task-owned. All 233 final frozen build inputs still match their
hashes after the full matrix and captures. The preservation record distinguishes
intentional task edits from untouched pre-existing work rather than attributing
the entire dirty tree to this phase.

## Reviewer follow-up

Inspect the current source, frozen inputs, underlying PNG/JSON diagnostics and
test logs using the relevant [review checklist](REVIEW_CHECKLIST.md). Review both
the unlabelled orbital sheet and unbiased local views, then discuss convincing
morphology and variation with the user. Keep the user visual gate OPEN until
explicit approval. Stop within Slice 1B; do not automatically begin Slice 2.
