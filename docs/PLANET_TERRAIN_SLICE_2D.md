# Planet terrain redesign — Slice 2D implementation plan

Date: 2026-10-06. Inspected baseline: `bb4e8ee15a926fc1e3e0fd7e54db9ba6bb4f69f3`, clean checkout.

## Objective and inspected problem

Make the accepted resident tile representation the ordinary planetary terrain
backend, including the actual Solar System Moon, and remove synchronous expensive
publication construction from native frames. This user-authorized slice supersedes
2C's finite-region restriction. It does not authorize Slice 3A.

IMPLEMENTED in the baseline: `TerrainPopulation::update` feeds the ordinary
`gravity_orbits.rs` route and `append_stitched_surface` repacks legacy CPU geometry.
The resident route is nested under the developer-only single-tile fixture.
`RegionalTerrain` already supports multiple roots and exact-key CPU caching.
`RegionalFixture::publish` constructs full-cover boundaries before its final morph
conflict check. Its advance loop can retry several expensive candidates in one
frame. These are source findings, not new performance measurements.

Historical C15 evidence records publication/advance maxima of 208.84/286.96 ms
and native intervals up to 336.30 ms, with acknowledged final-source drift. The
user's legacy Moon observations at roughly 436/243 km are comparison context;
fresh matched viewpoints are required for a current speedup claim.

## Ownership and implementation order

1. **Selector and scheduling** (`crates/app/src/regional_terrain.rs`, its focused
   integration tests): retain a complete six-face balanced desired cover, but use
   conservative horizon/frustum relevance to suppress invisible demand. Reuse
   `PatchMetadata`, `SurfaceExtent`, and `CelestialProjection` bounds. Keep the
   existing approximate relief/sagitta projected-error policy explicitly labeled;
   it is not a certified unsampled error. Add a separate optional body-fixed view
   orientation/projection API so existing finite fixtures keep their contract.
   Bound selector changes per tick and measured CPU time; use indexed neighbor
   lookup rather than full pair scans where needed. Preserve highest-error-first
   priority, hysteresis, quartet reservation, cancellation, and exact cache keys.
2. **Publication runtime** (`gravity_orbits/regional_fixture.rs`): reuse the same
   resident runtime outside developer-only fixture gating. Add asynchronous
   six-root configuration/bootstrap without sampling on the frame thread. Move
   target-boundary construction and subdivided-parent endpoint construction to
   one dedicated coordinator with at most two parallel boundary or endpoint tasks,
   one in-flight batch, one completion, and eight retained prepared products.
   Preflight local replacement/neighbor conflicts and skip blocked candidates.
   Tokenize exact tile keys and outgoing local dependencies; validate current
   drawable membership, demand, balance and affected boundary expectations before
   adopting each product. Independent topology changes may proceed during work.
   Pin dependencies until result acceptance/discard; retain transiently blocked
   products without rebuilding their geometry.
   Retain drawable parents through deferred split, and children through merge.
   Commit compact local prepared state with a measured 2 ms admission budget,
   1.5 ms scheduling headroom, at most eight adoptions and sixteen active groups.
   Prioritize current visual error with age protection. Report budget overruns,
   pipeline backlogs, useful-detail proxy arrival and exhaustive convergence.
3. **Planetary orchestration and GPU preparation** (primary ownership:
   `gravity_orbits.rs`, new planetary orchestration module as needed, renderer
   resident transaction/preparation files): select the terrain body using existing
   observer admission, compile immutable published world definitions and launch
   six-root work. Keep the ordinary far sphere until complete resident root
   coverage is ready. Support upload-only transactions during bootstrap. Derive
   observer-relative transforms in f64, cull only presentation/demand and retain
   topology/dependency coverage. Skip the legacy terrain population and stitched
   geometry preparation for the resident-owned body. Expose an explicit legacy
   comparison launch toggle; enable the resident backend in ordinary Solar System
   launches. Do not modify world truth to enable terrain fixtures.

   Inspection established one necessary presentation-boundary generalization:
   regional staging hardcodes a 10 km / 1 mm observer-relative narrowing budget,
   which rejects Moon root anchors and orbital views before rendering. Planetary
   transactions explicitly use a chart-footprint-scaled budget (1/262144 of the
   footprint, with a 1 mm floor). Fine charts within the original 10 km envelope
   and all original finite fixtures retain the 1 mm gate. Beyond 10 km, planetary
   anchor presentation additionally permits distance/2^23 per component: the
   concrete 100000.013 m retreat anchor narrows with 2.625 mm error and would
   otherwise reject the entire transaction. This is a representation-scale allowance,
   not an error certificate, and does not change tile reconstruction or formats.
4. **Diagnostics and reproducible routes** (after shared APIs settle): expose
   backend, CPU/GPU/desired/drawable/fallback counts, transitions, job states,
   refinement debt, throughput, selector/publication/GPU-preparation times,
   per-frame/cumulative content uploads and accounted memory. Record real-Moon
   checkpoints near 1000, 500, 250, 100, 25, 5, 1 km and close inspection, plus
   approach/retreat/reversal/region changes using existing leased native controls.
   Preserve paired PNG/JSON, normal worker delays, hardware, executable/source
   fingerprints, timings and pending/settled state.

These file owners are disjoint during concurrent implementation. No source or
documentation commits/pushes are authorized. No general-purpose job system,
tile format/topology rewrite, new morphology/material/climate payload, GPU
generation or environmental work is included.

## Acceptance and validation budget

Initial configurable planetary settings: 32 cells, six roots, maximum level 24,
512 desired leaves, 1024 GPU slots, 1536 CPU tiles, four generation workers,
bounded job/completion queues, four uploads and 8 MiB content upload admission
per frame, 150 ms transitions. These are inspectable capacity settings, not a
quality definition. Report capacity pressure and requested quality honestly.
Tune only against recorded hardware evidence; do not lower demand to claim
convergence. Initial publication CPU admission target is 2 ms; no tile generation
or boundary construction may execute on the frame thread. Native terrain stage
maxima and total frame p50/p95/max must be recorded, including cold refinement.
Use the historical 100 ms C15 stall gate as a failure bound, not a desirable frame
target; assess a tighter limit from the fresh hardware baseline before acceptance.

The preliminary native sweep filled the initial desired capacity at 250 km while
retaining valid coverage and unresolved target quality. At a 510-leaf view it
accounted 16.9 MB cached tile payload, 16.2 MB endpoint edges, 45.2 MB GPU tile
buffers and 13.1 MB GPU boundary buffers (690 allocated slots); these intermediate
figures omit some later accounting additions. Tune the production capacities to
2048 desired leaves, 4096 GPU slots and 6144 CPU tiles, with lazy GPU allocation
and the unchanged 0.15/0.075 px split/merge target. This raises bounded headroom;
it does not establish close-view convergence or final performance acceptance.

The later 1938x1334 terrain viewport reached 2046 desired leaves at 25 km with
capacity pressure and unresolved quality (`native-final-v9`). Current bounded
capacities are therefore 4096 desired leaves, 8192 GPU slots and 12288 CPU tiles,
with unchanged projected-error thresholds. Close-view convergence and frame
pacing remain acceptance gates; this capacity change does not establish either.
The large-window regression subsequently converged at 5 km (3273 leaves) but
reached 4095 leaves and capacity pressure at 1 km. Current production limits are
8192 desired leaves, 16384 logical GPU slots and 24576 CPU tiles. Allocation stays
lazy and bounded; the error thresholds remain unchanged. Final close-view native
memory and frame measurements must validate this larger headroom.
The close inspection regression reached 8190 leaves before capacity pressure,
then reached 9921 leaves below the unchanged threshold with larger headroom.
Production bounds are now 16384 desired leaves, 32768 logical GPU slots and 49152
CPU tiles. These are capacity bounds, not allocated memory or accepted frame cost.

Required focused tests: existing 2A/2B/2C reconstruction, shared/cross-face/corner
edges, parent fallback, split/merge, exact identity/stale rejection, memory/upload
accounting; new six-face coverage, conservative culling, spatially varied projected
LOD, rapid reprioritization, asynchronous bootstrap, bounded publication/stale
transaction rejection, and sustained stationary zero generation/content upload.
Run focused locked package targets first, `scripts/ai-check.ps1` and inspect its
paired evidence, then README/CI quality commands (format, strict Clippy, full
debug/release workspace tests, rustdoc, required native/GPU gates).

Run a fresh graphical executable and normal-delay Moon route. For each checkpoint
record useful-coverage and settled-quality time or explicit remaining pressure,
queue/debt, requested/displayed quality, per-stage and total CPU distribution,
memory and upload/generation deltas. Compare legacy/resident at matched cameras.
Require a sustained warm interval with zero tile generation, tile uploads and
boundary content uploads; presentation metadata may change. Increasing resident
count must not trigger every-tile CPU geometry rebuilding. Passing software or
offscreen tests alone does not accept native responsiveness or visuals.

## Completion boundary

Update reviewer context and deliver `PLANET_TERRAIN_SLICE_2D_REPORT.md` using the
repository handoff format, with exact production call path, default/toggle policy,
commands, current fingerprints, measured distributions/convergence/resources,
settled deltas and remaining failures. Slice 2D is incomplete if the real Moon
still uses legacy stitched meshes or terrain refinement produces unbounded
synchronous frame stalls. Remain within 2D; no automatic Slice 3A continuation.
