# Planet terrain redesign — Slice 1 handoff

This records the initial V1 checkpoint. The subsequent user-directed orbital
refinement is recorded in the [V2 handoff](PLANET_TERRAIN_SLICE_1_ORBITAL_REPORT.md).
Its source, executable, capture package and query records are retained separately.

## Goal

Start the approved [redesign](PLANET_TERRAIN_RENDERING_REDESIGN.md) with its
[Slice 1 contract](PLANET_TERRAIN_SLICE_1.md): reusable upstream moon terrain,
complete height/gradient/material queries and fixed-resolution visual references.
The user remains the visual acceptance authority.

## Result

**PARTIAL.** The reference prototype is implemented. Visual approval, convincing
terrain across the requested scales and later GPU/streaming acceptance remain
open. Automated correctness and quality results are recorded below separately.

## Architecture changes

`world::terrain` owns an independent `MoonLikeV1` definition and complete oracle.
Broad relief, contextual regolith and three seeded impact epochs combine with
bounded convex overprinting within each epoch. Rotated Cartesian cells generate
nearby features; their centres project to the reference sphere. The fixed 27-cell
halo encloses support plus projection displacement. Features have age-dependent
profiles, warped walls, broken rim sectors, ejecta and correlated materials.
Analytic gradients include warping, blending, material context and cutoff terms.
Only an absolute-height envelope is certified; regional derivative/representation
certificates are future tile-builder work.

The app example is a temporary f64 software rasterizer. It subtracts the observer
before projection, clips near-plane polygons and interpolates attributes with
perspective depth. Actual displaced triangles supply lighting normals; raw oracle
normals remain a separate diagnostic. Materials use a separately labelled derived
footprint approximation in coarse views. No renderer/world dependency is added.
The native production path and its legacy definitions retain their semantics.
The new definition is not yet published through `Body::terrain()` or production
clearance/navigation adapters.

## Files changed

- `crates/world/src/terrain/moon.rs`: definition, version, configuration and oracle.
- `crates/world/src/terrain/mod.rs`: module export only; prior crater edits preserved.
- `crates/world/tests/terrain_moon.rs`: deterministic, material, gradient, radius
  and canonical chart tests.
- `crates/app/examples/moon_surface_reference.rs`: reference rasterizer and package.
- `crates/app/Cargo.toml`: reference example feature declaration only in this task.
- Redesign/contract/report, README, architecture and reviewer context: scope,
  reproduction and dated evidence orientation.

All other existing dirty work, including the developer interface and crater
experiment, was preserved. No commit or push is authorized or performed.

## Tests

VERIFIED on Windows, Rust 1.98.1, with the dirty source state identified below:

- `cargo test --locked -p mundaris_world --test terrain_moon --lib`: 21 library
  tests and six moon integration tests pass (`world-focused-tests.txt`).
- `cargo test --locked --release -p mundaris_app --features terrain-capture
  --example moon_surface_reference`: four rasterizer/normal/material tests pass
  (`reference-example-tests.txt`).
- `scripts/ai-check.ps1 -OutputDirectory target/terrain-redesign/slice1/ai-check-final
  -Scene moon-orbit`: PASS. The paired schema-5 PNG/JSON was inspected. This checks
  the production legacy path; at frame 64 it is ready but `quality_pending=true`,
  `settled=false`, source radial LOD 1 versus desired 5. It proves no settled-terrain
  or new-definition visual acceptance.
- The first full-matrix all-feature Clippy run failed on two new example lints
  (manual array chunks and useless conversion). Both were corrected; selected
  format, all-target/all-feature workspace check and warnings-denied Clippy pass
  in `lint-retry/validation.json`. The original failure remains retained.

The initial full matrix is retained in `full-validation/validation.json`:
format, workspace check, default Clippy, all-feature debug workspace tests,
warnings-denied rustdoc and the ignored long-orbit tests pass. Release workspace,
bridge and four explicit GPU targets could not build because Windows denied
replacement of `target/release/mundaris_dev.exe`, held by two pre-existing services.
This is an environment/build failure before those tests, not a failing assertion.

The affected six checks pass on retry with `CARGO_TARGET_DIR` pointing to
`target/terrain-redesign/slice1/validation-build`; existing services remain intact.
Release dependency/build/fingerprint caches were copied, with Cargo freshness
checks and newly linked top-level binaries. See `release-retry-environment.json`
and `release-retry/validation.json` for exact commands and final results. The retry
runs all-feature release workspace tests, six bridge tests, and the ignored GPU
targets `native_close_surface`, `native_full_frame`, `developer_interface` and
`developer_scenarios` (one explicitly ignored GPU test in each target).
All 13 documented quality gates now have passing final results;
`effective-validation.json` indexes each latest result and its retained raw log.
Native window/operator interaction and Linux/remote CI are untested for this slice.

## Measurements

Reference JSON reports per-scene query/build and software rasterization wall time,
sample counts and sampled triangle-centroid radial residuals. These are single
reference runs, not CPU/GPU percentile, native FPS, process RSS or driver VRAM
measurements, and do not establish a production performance improvement.

For the 109 km reference radius, the fixed maximum nominal sample spacings are
1,703.125 m (orbit), 52.0833 m (regional) and 0.333333 m (near). Across seeds 2,
7 and 19, the maximum sampled radial residual is 29.48–33.92 m, 1.043–1.068 m
and 0.0297–0.0497 m respectively. Each scene checks 512 deterministic triangle
centroids; these finite samples are not representation-error certificates.
Source geometry uses f64 heights. Height PNGs are separately normalized to each
view's sampled extrema and quantized to eight bits; JSON records that step.

## Captures

Reproduction uses a new output directory:

```powershell
cargo run --locked --release -p mundaris_app --features terrain-capture --example moon_surface_reference -- target/terrain-redesign/slice1/reference-reproduction
```

The package contains three seeds (2, 7, 19), 960×640 orbit, regional and near views,
and seven diagnostics per scene: lit, lighting-only, height, reconstructed normal,
complete analytic normal, filtered material and raw material. It includes scene
metadata, 495 complete oracle queries and `index.html`. The 109 km radius is a
reference body, not a universal size.
Regional and near views are separate finite fixed-resolution crops, not a
continuous streaming route. The oracle corpus also records other radii and
canonical edge/corner queries for unchanged reuse in Slice 2.

OBSERVED in the reference package: regional relief and local impacts are present,
but depressions remain repetitive and broad surfaces look soft and simplified.
This is a reviewable first prototype, not a claim of convincing moon geology.

## Known failures

User visual acceptance is **OPEN**. Morphology/material realism, resolved shading
detail and lighting remain prototype limitations; no physically complete geology
or shadows are claimed. Sampled extrema/residuals and finite material filtering
are not conservative certificates or proofs of alias-free appearance. No settled
LOD, continuous navigation, tile residency, GPU reconstruction, zero unchanged
uploads, cold convergence, collision/walking or real-time performance gate is
closed. The original terrain Acceptance A and unrelated platform debt remain.

## Evidence

Evidence root: `target/terrain-redesign/slice1/`. Review `reference-final/index.html`
and its per-scene `metadata.json`. `reference-final-run.txt` retains the command
output; source and executable SHA-256 records are inside the package. Earlier
`initial-reference` and `review-reference` packages are intermediate visual
investigations. The preceding `reference` package preserves its own fingerprints.

VERIFIED: `reference-final-repeat-comparison.json` records identical SHA-256 values
for all 63 PNGs and the complete oracle corpus between `reference` and the rebuilt
`reference-final`. JSON timing metadata is intentionally not compared. Capture
runs overlapped validation and are not controlled performance measurements.
`reference-measurements.csv` summarizes the preceding equivalent visual run.

## Git state

`main`, HEAD `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`, dirty working tree.
HEAD alone does not identify the build. Initial status and final source/evidence
fingerprints are retained under the evidence root. `final-git-status.txt` records
the handoff state; `task-files.sha256.json` also identifies the task's documentation.
`git diff --check` passes. No commit or push.

## Reviewer follow-up

Inspect the final lit views together with height, reconstructed/analytic normals,
material diagnostics and metadata. Discuss morphology and materials with the user
before progressing to Slice 2. Correctness checks do not authorize that next slice.
