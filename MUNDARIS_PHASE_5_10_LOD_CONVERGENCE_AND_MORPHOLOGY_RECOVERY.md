# Phase 5.10 — LOD convergence and morphology recovery

**Status: partial implementation; responsiveness acceptance A is not met.
Morphology recovery is blocked, not accepted.** This work starts from Phase 5.9
commit `c25e173`. The worker/convergence foundation is an intermediate checkpoint,
not Phase 5.10 completion. Nothing has been pushed. Existing
unrelated documentation and workflow changes are preserved.

## 1. Acceptance and sequencing

Interactive convergence must be demonstrated before substantial morphology tuning.
Complete coarse coverage, low app-thread latency, a high radial LOD, and passing
unit tests are different properties; none alone establishes acceptance A. The
current terrain appearance remains rejected. No morphology parameter change is
present in this checkpoint.

## 2. Reproduced before state

Fresh committed-Phase-5.9 native offscreen captures were made before implementation.
They are distinct from the later serial timing reference using the new probe.
That serial reference reproduced a 1,279.9088 ms terrain-update stall. Offscreen
captures and headless production-route probes do not establish human UI acceptance.
The retained evidence index identifies each baseline separately.

## 3. Worker architecture

The app owns a terrain-specific pool with at most four workers, one admitted
calculation per slot, and a bounded app request queue. Inputs contain immutable
terrain identity, address or complete-cover geometry, never mutable world/frame/UI
state. Workers produce disposable CPU patches, stitched surfaces and exact morph
meshes. Generation remains on CPU; no new general task framework or dependency is
introduced. Domain truth remains in world, geometry policy in app, and presentation
in renderer.

## 4. Worker-count selection

Serial operation-budget fixtures retain `TerrainPopulation::new()`. Native
construction uses `TerrainPopulation::interactive()`, defaulting to four workers;
`MUNDARIS_TERRAIN_WORKERS=0..4` permits serial/1/2/4 comparisons. The retained
final 64-patch benchmark measured approximately 141/126/243/477 fine Grid16 patches per
second for serial/1/2/4 respectively. Coarse results were 1,115/636/1,252/2,627;
regional results 508/326/659/1,308. Initial worker-count selection evidence is
retained separately. One-millisecond polling affects these results;
they are not native frame rates or hard deadlines.

## 5. Determinism and identity

Worker results use the full body/terrain-definition/revision/reference-radius
binding. Body association is not generation salt. Serial and 1/2/4-worker raw
samples/certificates match bitwise for Earth, Moon and Mars at coarse, regional
and fine addresses. Separate worker-cover tests compare stitched endpoints,
normals, elevations, references and intermediate morph samples with serial output.
Observer movement and frame rebuilding cannot change authoritative terrain.

## 6. Atomic readiness and publication

Complete roots and complete balanced sibling/balance closures remain prerequisites
for publication. An incomplete job never removes valid source coverage. Useful
active patch results publish in admitted sequence order, independent of worker
completion order. Cover construction is off-thread; publication installs either
a complete destination or its common-refinement morph, not partial geometry.

## 7. Cancellation and switching

Revision/body changes invalidate inaccessible old entries and cancel reconstructible
jobs. Worker-held raw inputs remain resident/accounted until their final handles
drop. Canceled calculations cannot head-of-line block useful newer-body publication.
Source/destination coverage and pins remain valid through active morph completion.
Cancellation checks occur between generation microbatches and between major cover
stages; transition construction is not preempted in its inner exact-overlay loop.
This remains a cancellation-latency limitation for large jobs.

## 8. Aggregate memory ownership

The inherited 128 MiB aggregate CPU cap is unchanged. Admission includes retained
cache/queue capacities, raw geometry, worker stacks/fixed scratch, output envelopes,
derived surfaces/morphs, selector scratch and the renderer's full outgoing/boundary
staging allowances. Invalid worker-held cache entries cannot be evicted prematurely.

An admitted cover reservation owns the shared source-surface charge; the coordinator
does not charge the same source again while construction is pending. A capacity-four
completion queue retains that reservation even for cancellation acknowledgement.
Consumption restores coordinator accounting before new construction admission.
Explicit cover abandonment releases completed reservations or leaves in-flight
charges until acknowledgement. Tests cover serial/1/2/4 ownership and abandonment.
These are capacity/reservation measurements, not external allocator/RSS/driver peaks.

## 9. Exact stitching reuse

Stitch construction reuses unchanged patch data only when raw input identity and
stitch constraints prove it is the same result. Cross-patch displaced boundaries
still use canonical dyadic ownership. Reuse is an optimization of reconstructible
derived data; it neither changes the generator nor approximates seam geometry.

## 10. Common-refinement morphs

Differing topology still uses exact triangle overlay, barycentric old/new capture
and canonical boundary endpoints. Conservative spatial bins restrict candidates;
identical topology can use the exact identity template. No skirts, approximate
cross-fades, GPU terrain generation or loosened interpolation targets are used.
Exact integer separating-edge tests reject touching-only candidates before
rational clipping. Rejection is checked against exact clipping across all 16×16
stitch-mask pairs; full every-face split/merge comparisons against the unoptimized
path preserve output and endpoint bits.
The native morph duration remains 150 ms. A measured 32 ms experiment did not solve
cold convergence, so a shorter duration was not adopted.

## 11. Transition-budget recovery

The former first 16 MiB rejection permanently froze the private ready target.
Construction now retries at 16, 24 and 32 MiB, always preflighted inside the same
128 MiB aggregate cap. Source coverage and the private target remain frozen during
the retry; another selector transaction cannot advance beyond the source.
Successful publication resets the next transaction's allowance. Exhausting the
bounded retries still reports explicit deferral; changing duration or identity
resets it. Admission pressure is reported rather than publishing unbudgeted work.
The frozen private target retains/reconstructs its raw dependencies, preventing
eviction from leaving the retry waiting for geometry nobody requests again.

## 12. Certificates and culling

Generated patch height intervals intersect the global certificate with a proven
center-height ± global-gradient × cap-angle interval, plus omitted-band and numeric
allowances. This is not a sampled extrema bound. Displaced cap balls are centered
at the interval midpoint and conservatively contain the complete displaced cap.
Interpolation/curvature bounds are not reduced. Terrain horizon occlusion remains
disabled without a separate proof. Global gradient/Hessian envelopes still drive
excessive requested refinement; `TerrainGenerator::bounds_for_region` does not yet
use its regional cap to tighten derivative bounds.

## 13. Live diagnostics

The inspection panel separately exposes desired radial certificate LOD, highest
ready radial raw LOD, rendered **source** LOD, local error contributions, queued and
running work, reservations, cancellation count and stage costs. A morph destination
is not reported as a fully published source. Radial LOD is not screen-wide quality.
Certificate-probe cost is measured separately rather than hidden in worker timing.

## 14. Timed convergence

The probe samples 0/100/250/500/1,000/2,000/5,000 ms using the ordinary population,
selection, generation, publication and render-preparation paths at 768×512, 60°
vertical FOV and a 0.1 m near plane. Camera direction, complete terrain height and
target clearance are explicit. Headless frame opportunities are approximately
16 ms; optional offscreen readback costs alter that schedule. Actual sample times
are retained alongside requested times.

Final four-worker cold-2 m measurements reached source LOD0/0/1/3/5/9 at
100/250/500/1,000/2,000/5,000 ms, against radial requested LOD25. One/two workers
reach source LOD7/8 at five seconds; four workers with 32 ms morphs reach LOD12,
still failing the gate. Final descent reaches LOD19 at 2 m instead of the former
fixed-budget LOD7 freeze, with no transition deferral. Every view is still
quality-pending. Full timelines are in the evidence index; readiness must not be
described as convergence.

## 15. Moving-camera and multi-body routes

The probe supports descent, cold-2 m, approach/tangent movement/look reversal/retreat,
and repeated Earth→Moon→Mars→Earth teleports. It asserts exclusive body ownership,
valid published coverage and aggregate-cap compliance. A scripted camera route is
not human control-feel validation, and the motion route is not proof of arbitrary
high-speed cache cancellation or of complete quality convergence.
Final switch/motion probes recorded zero cancellations; forced unit tests exercise
current cancellation and source-ownership paths. The retained intermediate
switch route's 19 cancellations are historical, not final timing evidence.

## 16. Stage timings and throughput

Per-frame CSVs distinguish app update, selection, raw result publication/scheduling,
stitch/morph submission and completed worker CPU from render preparation. Exact
overlay profiles break down mapping, clipping, barycentric capture, canonicalization
and emission. Worker CPU totals are not frame latency; CPU render preparation is
not GPU time/FPS. Intermediate serial and worker captures must not be conflated
with the exact committed before-state matrix.
Final cold-four update median/worst is 0.677/2.408 ms; update plus CPU render
preparation is 1.947/28.973 ms. Descent total preparation has a 97.082 ms outlier
and peaks at 127.993 MiB accounted, only 6,828 bytes below the cap. Main-thread
render preparation during morphs, regional certificates and slow worker morphs
remain convergence/performance blockers; moving construction off-thread is not
proof that frame stalls are eliminated.

## 17. Morphology recovery and multiscale appearance

No macro/regional/local morphology tuning has been performed because acceptance A
fails. Branching erosion, eight-seed visual robustness, grounded/horizon readability,
and multi-scale Earth/Moon/Mars morphology acceptance remain unvalidated. Numerical
eight-seed certificate stress is not visual eight-seed acceptance. Readability/LOD
captures are diagnostic evidence, not a substitute for accepted planetary landforms.

## 18. Automated validation

Focused worker, publication-pin, cancellation/source-accounting, population,
adaptive/morph and renderer bound regressions have passed. The evidence index
records final formatting, workspace check, warnings-denied Clippy/Rustdoc,
debug/release workspace tests, eight-seed/all-stitch error stress, long orbits and
native offscreen regression outcomes separately. A debug stress invocation exceeded
its 120 s command budget; this is not counted as a passed run.
Final workspace debug and release each pass 224 tests with zero failures and
three ignored across 60 result suites, including the complete numerical stress.
The two ignored long-orbit tests and native close-surface regression pass
separately. Formatting, locked all-target/all-feature check, warnings-denied
Clippy and Rustdoc pass. These successful safety/quality checks do not turn the
failed responsiveness or morphology gates into acceptance.

## 19. Native/platform acceptance and remaining work

Native offscreen readback validates the renderer/capture path, not the exact
interactive Earth inspection workflow. Human camera-control feel, full native
frame/GPU/compositor distributions, high-DPI, actual OS sleep/recovery, Linux native
and current-revision remote CI remain open. The next substantive work is reducing
exact-overlay cost and regional certificate conservatism without weakening proofs,
then repeating acceptance A before any morphology recovery.

## 20. Delivery and repository state

Primary implementation: `crates/app/src/planet_terrain/{workers,adaptive,certificate}.rs`,
`crates/app/src/planet_terrain.rs`, `crates/app/src/terrain_population.rs`,
`crates/app/src/gravity_orbits.rs` for the inspection panel,
renderer stitching/transition/bounds/priority code, focused tests and two examples.
No new dependencies or changes to orbital integration are needed. Reproduction and
retained results are indexed in [Phase 5.10 evidence](docs/evidence/phase510/README.md)
and [Phase 5 validation](docs/phase-5-validation.md). The Phase 5.10B continuation
requests an intermediate foundation commit before further Acceptance-A recovery.
No push is authorized. This is not Phase 5.10 completion or morphology acceptance.
