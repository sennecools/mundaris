# Native Moon Surface Integration Handoff

Date: 2026-10-06. This is a focused handoff for the latest Moon refinement-demand
and compositional coarsening changes. Code validation completed; native visual and
performance acceptance remain open.

## Goal

Route the Solar System Moon through the new compositional terrain authority using
the existing native terrain architecture, preserving the historical crater recipe
and requiring the native clearance query to evaluate the same complete field.

## Result

**PARTIAL.** Both Solar System presets select the Moon's `RockyV5`
`SurfaceDefinition`; native positions still sample the complete field and clearance
queries the same authority. Compositional candidate priority uses the highest
projected representation demand across eligible surfaces instead of absolute
observer-radial distance. Demand uses the nearest point of each patch's conservative
ball to avoid chart-center undersampling. The same representation demand drives
stale/coarsening checks; prefetch follows the selector's actual outstanding requests.

The user rejected the latest screenshots for center detail, coarse edges and slow
retained cover. Visual acceptance is **FAILED** for those images; the subsequent
implementation has no accepted final visual evidence. Earlier preliminary cadence
measurements predate the latest selector changes and final normal stencil. They are
not a performance claim for current source. Final performance evidence remains
pending.

## Architecture changes

`NativeTerrainDefinition` adapts either a legacy `TerrainDefinition` or a
compositional `SurfaceDefinition`. Generation evaluates the complete selected
surface at each emitted LOD sample; caches remain keyed by body, full definition,
revision and reference-radius bits. Complete clearance uses the world query and
includes shape radius and gradient. The preset selects `RockyV5` for Moon; the
legacy `terrain_definition` and `cratered_terrain_definition` functions remain
available for their earlier fixtures.

The adapter keeps the existing renderer's ten-percent radial support limit. The
safe compositional patch certificate remains the complete radial amplitude bound
`2H` plus sphere-correspondence error, with zero profile-footprint delta. It still
does not provide a finite derivative certificate or prove error decreases with LOD.
The separate projected-sample-spacing demand is explicitly non-certifying and
cannot mark the global target certifiable. Refinement selection, stale/coarsening
checks and prefetch use the same representation-demand calculation; the selector's
outstanding requests determine what prefetch does. Compositional coarsening batches
up to 32 merges per update (legacy coarsening remains one merge and refinement
remains one step). Each cover tracks all 32 merge parents in a fixed inline array.
A queued or building cover is cancelled if any changed parent makes its merge
invalid. A regression test covers the multi-parent case that exposed the earlier
singular-parent tracking gap. The nearest-point test on a
patch's conservative bounding ball accounts for the most demanding part of the
represented surface rather than relying on the chart center. The snapshot
distinguishes `projected_sample_spacing` from `certified_error` and reports
`target_certifiable` separately.

Surface positions remain bit-exact evaluations of the complete field. Normals use
finite secants at the mesh footprint along three fixed body axes, avoiding a hard
basis switch; seven complete-field queries per vertex run on workers to construct
the approximate shading normal.
Terrain population retains no compiled generator heap for this optimization: it
caches identity and height bound in eight fixed slots keyed by body, revision,
reference-radius bits and exact definition, avoiding repeated compilation during
admission and active updates.

This can avoid demand underestimation and inconsistent stale/prefetch decisions, but
it does not refine the global certificate.
Native material presentation and shadow treatment still do not match the reference;
the renderer's mesh-normal treatment also remains approximate. No GPU tile redesign,
four-channel material shading, final-binary performance result, or visual acceptance
is claimed.

## Files changed

Implementation files in the shared dirty worktree:

- `crates/app/src/solar_system.rs`, `crates/app/src/terrain_population.rs`,
  `crates/app/src/gravity_orbits.rs` — Moon selection, surface admission and
  session discovery.
- `crates/app/src/planet_terrain.rs`,
  `crates/app/src/planet_terrain/native_surface.rs`, related worker/cache/LOD code,
  and heap-accounting support in `crates/world/src/terrain/surface.rs` plus
  province/hierarchy code — native dispatch and conservative geometry.
- `crates/app/src/developer_snapshot.rs` and capture filters — optional algorithm
  and certificate diagnostics.
- `crates/app/tests/native_moon.rs` and related tests — exact serial/worker parity
  with complete queries and legacy/surface cache dispatch.

`crates/world/src/body.rs`, `crates/world/src/system.rs`, and
`crates/app/src/terrain_inspection.rs` were already dirty; the Moon-selection task
did not edit them. Their body-authority and complete-clearance changes remain in
the shared integration state and are not attributed to that task.

Documentation updated for this handoff:

- `README.md`
- `docs/architecture.md`
- `docs/REVIEWER_CONTEXT.md`
- `docs/AI_DEVELOPMENT_INTERFACE.md`
- `docs/NATIVE_MOON_INTEGRATION_REPORT.md`

## Tests

- Historical focused results for the earlier adapter revision include
  `cargo test --locked -p mundaris_app --test native_moon` (2/2), plus the following
  suites after the legacy-only Moon unwrap fix:
  `cargo test --locked -p mundaris_app --test terrain_inspection` (3/3),
  `cargo test --locked -p mundaris_app --test developer_interface` (8/8),
  `cargo test --locked -p mundaris_app --test solar_system` (7/7), and
  `cargo test --locked -p mundaris_app --test native_moon` (2/2).
- A prior focused native-terrain rerun passed 12 tests across native, adaptive and
  population suites, plus one render test and its infinite-range regression. These results
  predate the latest demand-selector changes.
- Focused tests for the latest changes passed: app library 44/44,
  `native_moon` 4/4, `adaptive` 3/3,
  `population` 6/6, `terrain_workers` 2/2, renderer library 44/44 and renderer
  demand 1/1. The latest regression exercises invalidation when any of multiple
  merge parents changes.
- Four final-lint gates passed. Full workspace debug and release tests, rustdoc,
  long-orbit tests, bridge tests and all four GPU/native capture checks passed.
  The matrix's two initial `useless_vec` lint failures are retained in its logs
  and superseded by clean strict reruns, giving 13 passing latest gates in
  `target/moon-lod-fix/20261006/completion.json`.
  The debug workspace run began before the final all-parent invalidation change;
  current focused debug tests and the current full release workspace run passed.
- A prior `cargo check --locked --workspace --all-targets --all-features` is recorded
  in `target/native-moon/20261006-014148/full-validation/workspace-check.txt`.
- A prior `cargo fmt --all -- --check` is recorded in
  `target/native-moon/20261006-014148/full-validation/format.txt`.
- Both standalone strict workspace Clippy commands pass after fixing the initial
  `manual_range_contains` finding:
  `cargo clippy --locked --workspace --all-targets -- -D warnings` and
  `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`.
  `cargo test --locked -p mundaris_world --lib memory_accounting_tests` passed 1/1.
- The initial full-matrix debug and release suites failed because
  `terrain_workers` still used a legacy `terrain().unwrap()` fixture after surface
  authority migration. The migrated fixture passes both focused and workspace runs.
- The initial Clippy run reported `manual_range_contains`; this was fixed and both
  standalone strict Clippy commands pass. These do not establish a full-matrix pass.
- Earlier focused passes and preliminary measurements refer to preceding source
  states; do not treat them as final validation or performance evidence for the
  latest demand-selector implementation.

## Captures

Evidence directory: `target/native-moon/20261006-014148/` (HEAD
`ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`, dirty source; the JSON records the
capture fixture and adapter).

- `ai-check/` — the script commands and paired files passed artifact checks, but
  the snapshot reports terrain inactive. Retain this as failed integration
  evidence: that run did not exercise the selected Moon surface.
- `ai-check-v2/` — paired `moon-orbit.png` and `moon-orbit.json` report Moon /
  `RockyV5`, `complete_amplitude_bound`, source and ready radial LOD 1, desired LOD
  30, `ready=true`, `quality_pending=true`, and `settled=false`. The PNG shows a
  coarse whole-body surface. It is offscreen evidence, not native interaction or
  acceptance of the visual target.

The pre-fix live capture is retained in
`native/captures/12764-1791251339393283700-4-moon-orbit/`; it reports source/ready/
desired radial LOD 16/17/30, budget-constrained and quality-pending at 327,278 m
clearance. The user rejected that visual result and reported roughly 10 FPS. A
later native sampling lease was interrupted at 20/100 samples. The script stopped
before requesting a capture, and none was requested after that interruption.

## Measurements

The pre-fix read-only cadence record at `native/cadence-observations.json` contains
16 samples over 5.311 s at 1,000 m clearance: 106 submitted frames (19.957 Hz), CPU
frame 44.1013–52.7459 ms, terrain update 13.3312–20.3507 ms, terrain preparation
29.1342–31.405 ms, and unavailable GPU timings. Submitted-frame cadence is not GPU
FPS and does not establish a GPU bottleneck.

Earlier post-fix evidence is under `target/moon-lod-fix/20261006/`. Its `ai-check`
pair remained visibly coarse (ready LOD 1 versus desired LOD 7, target certification
false). Its preliminary 18-sample, 6.812 s native cadence at 1,000 m recorded 437
submitted frames, CPU frame 13.0889–16.7866 ms and 624 to 552 mutable leaves. Those
measurements predate the final normal stencil and the latest selector, nearest-ball,
stale/coarsening and prefetch changes. They are historical development observations,
not current-source performance evidence. New full-validation and paired screenshot
evidence is pending. GPU timings remain unavailable; stale/minimized values in
`native/readonly-cadence.json` have zero counters and provide no FPS evidence.

## Known failures

- The user rejected the pre-fix appearance and reported roughly 10 FPS. The updated
  visual/performance result has not been accepted.
- The global `2H` certificate remains nonconvergent; target certifiability remains
  false even when projected sample spacing guides refinement.
- The latest user-reviewed screenshots show center detail, coarse edges and a slow
  retained cover; the user rejects them. Visual acceptance is **FAILED** for those
  images, and no final visual package has been accepted.
- A later pre-batch native orbit capture at 15 seconds reports source LOD 4 and
  desired LOD 7 and still appears coarse. Human input ended the observation lease
  during the close-in phase, before zoom-out. This is not final visual acceptance
  or a final-source performance measurement.
- Earlier post-fix offscreen/cadence evidence predates the current demand selector
  and cannot establish current visual quality or performance.
- The later native sampling lease ended at 20/100 samples; the script did not
  request a capture afterward.
- The initial debug and release workspace test suites failed on an obsolete
  `terrain_workers` `terrain().unwrap()` fixture. This was repaired; the subsequent
  workspace suites and current focused tests pass, with the source-scope limits above.
- Initial `manual_range_contains` Clippy errors were fixed; standalone strict Clippy
  and the latest matrix results are recorded separately from the retained failures.
- Native material and shadow presentation remain unlike the reference; secant mesh
  normals are approximate.

## Evidence

- Paired current offscreen capture and metadata:
  `target/native-moon/20261006-014148/ai-check-v2/moon-orbit.png` and
  `target/native-moon/20261006-014148/ai-check-v2/moon-orbit.json`.
- Capture command summary:
  `target/native-moon/20261006-014148/ai-check-v2/summary.md` and
  `validation.json`.
- Earlier inactive fixture pair:
  `target/native-moon/20261006-014148/ai-check/`.
- Live capture pair:
  `native/captures/12764-1791251339393283700-4-moon-orbit/`.
- Read-only cadence record: `native/cadence-observations.json`.
- Pre-fix read-only follow-up: `native/readonly-cadence.json`.
- Earlier post-fix paired offscreen evidence: `target/moon-lod-fix/20261006/`.
- Pre-batch native orbit capture: `native-final/captures/18696-1791253169899094200-5-orbit/`.
- Preliminary post-fix cadence values:
  `target/moon-lod-fix/20261006/preliminary-performance.json`.
- Final source fingerprints, release binary hash and latest gate results:
  `target/moon-lod-fix/20261006/completion.json`; matrix logs in `final-validation/`,
  repaired lint logs in `final-lint/`, and current focused app logs in
  `focused-final.txt`. The final fast capture pair is in `ai-check-batch-final/`.
- Workspace check, format and validation logs:
  `target/native-moon/20261006-014148/full-validation/`.
- Focused post-fix test and lint results:
  `target/native-moon/20261006-014148/focused-validation-followup.json`.

## Git state

No commit or push was requested. The evidence uses HEAD
`ef40ed3c81c2a4b66f3cd359c508c8944cc5183d` with extensive dirty and untracked
source/documentation state; HEAD alone does not identify the tested source.

## Reviewer follow-up

Review the actual source and offscreen/live capture pairs. Keep implementation
correctness, complete-field parity, quality-pending state, code validation,
runtime observations, pre-fix user rejection and post-fix acceptance as distinct
claims. The next review should assess a native representation/certificate that
supports useful LOD convergence, then presentation that matches the reference. Do
not accept the current coarse/uncertifiable visual result or start another terrain
phase without reviewer and user discussion.
