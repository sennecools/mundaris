# Phase 4 CPU closeout — 2026-10-02

## Scope and reproducibility

Recovered HEAD `ef9ab84`, following `5a7215c` and renderer `d9707a4`. Existing
`.gitignore`/untracked workflow changes were preserved separately. No history,
terrain, Phase5, topology, dependency, threshold or ownership change is included.
Host/compiler/profile match the [baseline](performance.md#phase-4-baseline--2026-10-02).
Closeout source milestone: `3c79011` (profiling, packing/grouping, capacity fix and
regressions); measurements were made on its corresponding working source.

The original **11.143 [11.086,11.208] ms** benchmark is precisely
`planet_selection/prepare/h10000000/cover510/visible402/level4` in
`crates/renderer/benches/planet_surface.rs`. It prepares a settled radial planet:
510 covering/402 visible patches, level4, 116,178 sample32 records, 204,800 triangles,
nine stitch draws and 3,743,424 payload bytes, no fallback. It excludes selection,
simulation, window/device startup and GPU work. Its diameter is **587.304 physical
px**, not viewport-filling; the new6,400km-clearance case is800px.

```text
cargo bench --locked -p mundaris_renderer --bench planet_surface -- 'prepare/h10000000/cover510/visible402/level4'
cargo bench --locked -p mundaris_renderer --features surface-profile --bench planet_surface_profile
cargo bench --locked -p mundaris_renderer --bench planet_surface
cargo bench --locked -p mundaris_app --bench planet_surface_approach
```

The opt-in `surface-profile` feature adds coarse existing-boundary clocks; ordinary
builds have none. The separate probe warms10 iterations then measures100 individual
updates per view, reporting median/empirical p95 (sorted index95)/maximum and workload.
These are short CPU distributions, **not** Criterion intervals or presentation p95.
Raw probes are temporary; Criterion reports remain in ignored `target/criterion`.
No correctness timing limits or allocator dependency/framework were added.

## Full-view decomposition

First instrumented before-change run, median/p95 milliseconds per update:

| Stage | Median | p95 | Included work |
| --- | ---: | ---: | --- |
| Selection total | 1.3612 | 1.4042 | Complete warm selector and source setup |
| Merge decisions | 0.2748 | 0.2850 | Parent error/culling, siblings/outside-edge balance checks, pins |
| Split/readiness/balance | 0.0977 | 0.1072 | Requests/relevance/order; no steady splits/builds |
| Visible set/stitch masks | 0.4379 | 0.4532 | Cache/culling/error, same/cross-face neighbors, ancestor cover |
| Desired cover traversal | 0.2817 | 0.2925 | Root stack, relevance/hysteresis, flat set |
| Final report | 0.0972 | 0.1090 | Repeated cover culling and capacity accounting |
| Preparation total | 10.9970 | 11.6013 | Visible patches only |
| Boundary processing | 0.6208 | 0.6367 | Keys, sort/deduplicate, shared-value initialization |
| Source/buffer setup | 0.0003 | 0.0006 | Preflight/reserve, source, prior batch records |
| Evaluate/convert/narrow | 9.0551 | 9.5651 | Canonical keys, f64 sphere/radius, local subtraction/rotation, checked normals, f32 casts, shared lookup, finite checks |
| Precision proof | 0.1777 | 0.1920 | Error/depth pass, whole-patch projected/GPU arithmetic proof |
| Samples/metadata packing | 1.0972 | 1.1838 | Little-endian sample32/instance64, record append |
| Batch/draw grouping | 0.0075 | 0.0077 | Buckets/instance copy/draw/payload accounting |
| Combined CPU | 12.3603 | 12.9829 | Selection + preparation, not complete app frame |

Stage medians need not add; total includes clock/loop/setup gaps. Internal selector
total starts after source setup. Bound scaling, cache lookups, horizon/frustum and
projected LOD error are interleaved, not assigned invented exclusive durations.
`relevance` checks the cached geometric-plane horizon envelope, scales the cap ball,
source-centres it, frustum-tests and evaluates projected error. Cross-face/stitch
processing is in `visible_for`; balance closure is inside split transactions.
Steady builds/misses/evictions/splits/merges are zero. Reported cache hits2,413 exclude
the final510 culling-report lookups, because the existing snapshot precedes them.

Isolated public-operation probes on116,178 samples measured keys+f64 mapping
**3.1046ms**, and source-position conversion+checked normal rotation **4.7433ms**
medians. They exclude production loop fusion, shared lookup and narrowing/finite
checks: they are not additive stage times. Existing Criterion metadata/neighbor/
bounds/culling groups supply independent primitive measurements; cached patch bounds
are not rebuilt per sample. Full view has402 whole-patch proofs, no triangle clipping
or fallback. At2m horizon,28 patches use whole proof/four clipped proof, zero fallback.

## Representative settled views

Recorded post-cleanup run, before the accounting-only correction below. R6.4e6m,
1280×800 physical content/FOV60°/near0.1m/grid16/default0.125/0.0625px.
Each view independently starts at six roots, not a radial-history continuation.
All settle unconstrained, warm cache misses/evictions zero; balanced=active and
rendered=visible. Tiny is a probe only: the app uses far representation, no patches.

| View / clearance | Desired / balanced / visible | LOD | Samples | Draws | Bytes | Select median ms | Prepare median / p95 ms | Combined median / p95 ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| A tiny / 1e11m | 6 / 6 / 5 | 0 | 1,445 | 1 | 46,560 | 0.0031 | 0.1292 / 0.1535 | 0.1323 / 0.1652 |
| B ~100px / 83,000km | 81 / 84 / 62 | 2 | 17,918 | 1 | 577,344 | 0.1354 | 1.6397 / 2.0648 | 1.7815 / 2.1956 |
| Original full / 10,000km | 483 / 510 / 402 | 4 | 116,178 | 9 | 3,743,424 | 1.3624 | 11.0652 / 11.4732 | 12.4764 / 13.0503 |
| C viewport fills / 6,400km | 471 / 510 / 402 | 4 | 116,178 | 8 | 3,743,424 | 1.3444 | 11.0640 / 11.5677 | 12.4157 / 12.9348 |
| D low orbit / 1,000km | 303 / 351 / 98 | 6 | 28,322 | 3 | 912,576 | 0.8585 | 2.6215 / 2.9646 | 3.4915 / 3.9003 |
| E 100km | 105 / 189 / 24 | 8 | 6,936 | 3 | 223,488 | 0.3313 | 0.6240 / 0.7822 | 0.9589 / 1.2307 |
| F 10km | 117 / 129 / 8 | 10 | 2,312 | 3 | 74,496 | 0.2268 | 0.2067 / 0.2936 | 0.4352 / 0.6756 |
| G 1km | 177 / 333 / 16 | 14 | 4,624 | 3 | 148,992 | 0.8353 | 0.4189 / 0.5807 | 1.2648 / 1.4998 |
| H 100m | 201 / 213 / 8 | 17 | 2,312 | 3 | 74,496 | 0.5310 | 0.2062 / 0.2253 | 0.7373 / 0.9968 |
| I 10m down | 213 / 225 / 4 | 18 | 1,156 | 1 | 37,248 | 0.5789 | 0.1031 / 0.1366 | 0.6843 / 0.7843 |
| I 2m down | 213 / 225 / 4 | 18 | 1,156 | 1 | 37,248 | 0.5714 | 0.1029 / 0.1209 | 0.6751 / 0.9541 |
| I 2m horizon | 213 / 225 / 32 | 18 | 9,248 | 4 | 297,984 | 0.6416 | 1.0257 / 1.5779 | 1.6736 / 2.2076 |

Original full masks0→15: `[286,24,28,0,24,5,3,0,28,3,1,0,0,0,0,0]`;
viewport-filling: `[298,23,25,0,23,2,3,0,25,3,0,0,0,0,0,0]`.
Full CPU staging capacity3,776,704 bytes; recorded boundary690,413; selector136,720;
metadata678 records. Memory movement scales with visible samples, not the theoretical
tree. Driver staging allocation/upload bandwidth/time is outside this CPU probe.

Cold full convergence needs23 updates: selection median4.1302ms/p958.7052ms/
max8.7832ms. Viewport-filling23/max8.8230ms; low orbit16/max5.2078ms;
1km21/max8.9376ms; 2m19/max2.1444ms. These include metadata/balance, not sample uploads,
and are cold jumps, not continuous warmed approach. The32-record allowance bounds
work, not milliseconds. Cold spikes remain a useful future review workload.

## Allocation observations and changes

All13 selector containers/cache and all7 preparation vectors are observed. Across
100 warm updates in every recorded view: **zero owned capacity growth and zero
explicit temporary dependency vectors**. First preparation grows six vectors.
Cold full convergence creates189 dependency vectors/9,600 cumulative capacity bytes;
1km153/31,392; 2m72/3,648. These are container events, not global allocator call counts.

Inspection found stable instance sorting before an already stable16-mask grouping
pass: unnecessary work and potential stable-sort allocation. Removed it without
changing within-mask/cross-batch order. Sample packing now uses one32-byte stack
record/append instead of eight4-byte appends. Full-byte/interleaved-bucket regression
checks cover both. Fallback and precision behavior are unchanged.

Uninstrumented Criterion before **11.024 [10.979,11.070]ms**, immediate after
**10.949 [10.891,11.011]ms**: **no statistically detected total improvement** (p0.06).
Do not claim an overall speedup or resolved11ms issue. Packing median1.0972→0.7994ms,
grouping7.5→3.4µs in immediate instrumented after; final0.8189ms/3.3µs.
These remove measured substage waste, not structural cost.

The later complete uninstrumented suite measured full preparation
**10.695 [10.636,10.764]ms**, warm selection1.3049 [1.2978,1.3120]ms,
viewport-filling preparation10.693 [10.643,10.745]ms. Criterion reported a2.31%
improvement versus the immediate after run; short host variation and packing effects
are not isolated by that comparison. Integrated N3/h60 full batch was
**12.192 [12.137,12.246]ms**, 10km474.82 [472.26,478.18]µs,
100m783.77 [779.16,788.95]µs, 2m724.07 [718.55,730.33]µs.
Both complete registered Phase4 targets executed with the viewer closed. The remaining
roughly11ms result is still far above the review target, not a resolved bottleneck.

Fresh primitive central estimates: all-stitch metadata root/level5/16/30
15.792/14.618/14.568/15.335µs per record;768 neighbor queries4.0405µs;
192 cached-record ball scaling1.4568µs, horizon9.2746µs, frustum3.6840µs.
These are separate fixed primitive workloads, not exclusive full-frame stage shares.

Inspection plus stable capacities supports successful warm CPU buffer reuse, not
whole-app zero allocations. Allocator internals, egui/camera clones, error paths,
driver/queue staging and GPU growth/wait remain uncounted. External heap/GPU allocation
profiling remains open. Cold dependency churn is small here; no scratch-pool rewrite
is justified merely to remove it.

Closeout inspection also corrected boundary capacity accounting: Rust `Vec<bool>`
is byte-addressed, not bit-packed. The recorded690,413-byte full-view report
undercounted readiness capacity; the corrected report is700,523 bytes. No buffer
allocation, payload or geometry changed; the existing8MiB aggregate cap remains.

The final accounting-corrected probe reran all views: every warm container-growth/
dependency count remained zero. Full preparation median/p9510.7824/11.3209ms,
selection1.3566/1.6355ms; sample stage9.1432ms, proof0.1787ms, packing0.7898ms,
boundary0.6213ms, grouping3.3µs. Corrected700,523-byte boundary capacity was directly
reported, not estimated. These separate runs preserve rather than replace the
before/after evidence above.

## Interpretation and freeze decision

Classification: **ordinary expensive full-planet steady view**, mainly expected
linear CPU f64 sampling/checked conversion at high patch quantity. Not primarily
debug instrumentation, clipping, cold metadata, allocation or N3 gravity. Most
expensive measured settled view, not a global worst-case proof (4K/grazing/corners
can differ). It still misses2ms median/4ms p95 review triggers and consumes60Hz
headroom. Cheap close views do not make it universally acceptable.

Strict thresholds produce402 full-view visible patches versus4 downward at metre
clearance. Patch quantity dominates sample work. Stitched curvature/near-plane bounds
deliberately overestimate error; deep close LOD is not metre terrain spacing. No
false bound, pathological oscillation or unusable default quality was established.
Changing thresholds requires future matched visual/transition quality-cost A/B
evidence; defaults remain unchanged.

**Freeze Phase4 architecture unchanged.** No measured structural bottleneck requires
new topology, GPU generation, persistent vertices, jobs, indirect rendering or weaker
precision before Phase5. The derived sample contract remains optimizable later.
Architecture readiness is not acceptance: outstanding operator/high-DPI/sleep,
Linux/current CI and detailed GPU/presentation gates must be completed or explicitly
dispositioned before declaring Phase5 unconditionally cleared. Phase5 has not begun.
