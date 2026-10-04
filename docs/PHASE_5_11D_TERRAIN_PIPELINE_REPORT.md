# Phase 5.11D — terrain CPU/GPU measurement checkpoint

## Status

**PARTIAL. Acceptance A: FAIL.** Production instrumentation and release evidence
are extended. The requested rebalance is not implemented: no new keyed renderer/GPU
terrain geometry cache, GPU morph interpolation, accelerated display progression, or
operational headroom recovery is claimed. The ~114 ms outlier was not reproduced
and therefore is **not fixed or causally explained**. Following the measurement-first
rule, no terrain-pipeline redesign or morphology work follows this checkpoint.

The overnight working tree and unrelated design/workflow changes were preserved.
Work remained uncommitted throughout measurement and validation, before the
subsequent user-requested commit. No push, cap increase, error-threshold change,
terrain-generator change, continental tuning or new planetary feature was made.

## Root cause

### Slow visible convergence — confirmed display gate

An exact complete destination must construct before display. One successor can
construct from the active morph's immutable destination, but it cannot display
until that endpoint is promoted. Every displayed replacement still captures the
150 ms duration. Twenty-three such intervals alone imply 3.45 seconds, before
raw readiness, construction and app opportunities. This gate is source-confirmed;
occupancy timings are not an additive causal percentage of total convergence.

### 114.533 ms descent outlier — stages identified; event unproved

The old matching CSV/profile at 10 km / 4,349.477 ms records:

| Stage | ms |
| --- | ---: |
| Update | 88.8164 |
| Selector parent | 87.4671 |
| Selector split/readiness parent | 35.9403 |
| Selector visibility/masks | 50.0437 |
| Selector merge / desired / final | 0.3871 / 0.7170 / 0.1467 |
| Preparation | 25.7167 |
| Regular boundary / samples / proof / packing | 19.9670 / 4.3104 / 0.2151 / 1.0877 |
| Update + preparation | **114.5331** |

There are zero selector capacity growths and one 48-byte dependency vector. The
preparation profile has zero growths and no transition triangles on this frame.
Nearby old frames instead spike in regular samples/packing or transition sample/
clipping work. These records reject an attribution to transition append or GPU
buffer growth **on the 114.533 ms frame**, but do not distinguish descheduling,
certificate callbacks, scans or another CPU event within selection.

Six fresh descents have worst totals 24.0821 / 50.3362 / 44.0923 / 24.9656 /
25.6681 / 35.1627 ms; the last two are the final rebuilt source.
None reproduces >100 ms. Windows thread accounting is too coarse for exact
short-stage attribution, and the scheduler trace attempt was denied by the host
profiling policy. No algorithmic or scheduling cause is promoted from suspicion
to fact; non-reproduction cannot satisfy the elimination gate.

### Zero memory headroom — confirmed reservation/ownership pressure

All six fresh descents still reach **134,217,728 bytes**, exactly the unchanged
128 MiB CPU admission cap. The existing 75,530,240-byte renderer staging allowance
and selector/construction/transition envelopes coexist with raw cache residency,
workers and source/destination geometry. Stitched reuse still copies sample grids;
source, destination and successor can own distinct complete copies. Source charges
transfer into worker reservations during construction; adding those charges again
would double-count ownership. Measurements still report no operational headroom.

## CPU/GPU architecture before

- CPU workers: deterministic raw samples/certificates, complete-cover stitching,
  exact common refinement. CPU app: admission, clearance, selector/publication,
  150 ms morph bookkeeping, culling, every-frame coordinate conversion,
  classification, proof lookup/calculation, clipping and packing.
- GPU: projection/rasterization/materials and existing ocean/cloud/atmosphere.
  Index topology and buffer capacities persist; all active terrain payload content
  is rewritten. No keyed immutable GPU terrain residency.
- Capture timing: host encode and render/wait/readback, **not GPU elapsed time**.

## CPU/GPU architecture after this checkpoint

Geometry ownership and display progression remain the same. Measurement changes:

- `surface-profile` reports selector setup/pinning, request scan/sort, balance
  scans and work counts; adaptive and population profiles distinguish admission,
  selection, prefetch, construction/publication, visibility and reservations.
- Regular sample work is patch-batched into conversion, generated classification
  and boundary narrowing, with timers outside vertex loops. f64 calculations,
  boundary reuse, proofs, packed layouts and thresholds are unchanged. Proof
  lookup and calculation, and actual vector-growth intervals, are separate.
- Workers report submit/start/finish/cache reception waits and consumed-job stages,
  with coordinator wait measured at consumption rather than summing retained
  last-completion timers. Abandoned in-flight acknowledgements contribute cancelled
  measurement records without retaining their source allocation.
- An optional safe thread-clock dependency distinguishes scheduled CPU from wall
  time; it introduces no unsafe project code. Windows resolution is explicitly
  reported, not interpreted as exact per-stage CPU cost.
- Supported adapters request wgpu timestamps. One bounded native query/readback
  slot skips busy samples without waiting. Capture resolves queries with its existing
  readback opportunity. Written-scope masks prevent absent scopes returning old data.
- CPU upload reports bytes, buffer capacities, growth/wait events and API duration.
  Native logs label asynchronous GPU values as latest-completed, not current-frame.

Transition interpolation/conversion/classification remain one CPU parent scope;
clipping and packing are separate. Fine interpolation-only timing, comprehensive
allocator/RSS tracing, and precise CPU scheduler attribution remain incomplete.

## Visible convergence

Four workers, cold tangent 2 m view, unchanged desired L23 and 150 ms setting:

| Requested time | Previous source LOD | Final source LOD | Desired | Final ready |
| ---: | ---: | ---: | ---: | ---: |
| 1 s | 5 | 5 (1,009 ms) | 23 | 7 |
| 2 s | 8 | 8 (2,009 ms) | 23 | 10 |
| 5 s | 18 | 18 (5,010 ms) | 23 | 19 |

The final cold run completes 685 raw patches / 197,965 samples, with 166
completions useful to the current local closure. This is not a speedup or a
claim that the remaining completions are wasted. All checkpoints remain pending;
camera-radial agreement is not whole-view useful quality.

## Frame latency

Milliseconds, median / nearest-rank P95 / worst. Totals are quantiles of per-frame
sums, not sums of independent quantiles. No optimized after architecture exists.

| Descent evidence | Update | Prepare | Update + prepare |
| --- | --- | --- | --- |
| Previous overnight | 3.470 / 5.700 / 88.816 | 8.056 / 12.870 / 85.925 | 11.201 / 16.883 / 114.533 |
| Fresh unchanged before, run 1 | 3.327 / 5.434 / 10.708 | 7.722 / 12.270 / 19.968 | 10.779 / 16.148 / 24.082 |
| Fresh unchanged before, run 2 | 3.318 / 5.441 / 35.194 | 7.779 / 12.353 / 19.751 | 10.861 / 16.287 / 50.336 |
| Instrumented first | 3.403 / 5.748 / 21.613 | 8.210 / 12.827 / 22.480 | 11.388 / 17.093 / 44.092 |
| Corrected instrumented repeat | 3.381 / 5.589 / 11.444 | 8.334 / 12.679 / 20.721 | 11.345 / 16.885 / 24.966 |
| Final rebuilt source, run 1 | 3.358 / 5.744 / 18.430 | 7.870 / 12.509 / 19.798 | 10.948 / 16.404 / 25.668 |
| Final rebuilt source, run 2 | 3.362 / 5.619 / 18.332 | 7.931 / 12.649 / 30.134 | 11.031 / 16.714 / 35.163 |

`instrumented/` rows are retained intermediate snapshots. `final/` follows the
cover-publication timer correction and removal of temporary sample-key storage;
its source/executable fingerprints are retained with the exact build/run commands.

The first instrumented worst frame is 10 m / 3,281.039 ms: update 21.6125 + prepare
22.4798 = 44.0923 ms. Selection is 20.8508 ms, including request scan 4.8050 and
balance scan 12.9905. Regular append is 20.8936 ms: boundary 0.5692, sample parent
18.0227 (conversion 3.2703, classification 2.4040, narrowing 12.3095), proof 0.4370,
packing 1.7904; transition samples/clipping are 0.4341/0.8511 ms. No growth, proof
calculation or worker completion occurs. The thread clock reads zero in those
short intervals; its coarse accounting cannot prove an exact scheduling event.

The final repeat's worst frame is 2 m / 3,894.356 ms: update 5.0289 + prepare
30.1338 = 35.1627 ms. Regular append is 27.5701 ms: boundary 3.5577, sample parent
21.0663 (conversion 16.4852, classification 2.1772, narrowing 2.3588), proof 0.4444
and packing 2.0649. Transition samples/clipping take 1.1702/0.7315 ms, with zero
emitted triangles. No proof calculation, capacity growth or worker completion
occurs. Preparation's scheduled thread delta is 15.625 ms, update's is zero;
quantization cannot establish exact wait time or a scheduling cause.

Final run 1's worst frame is 100 m / 354.550 ms: update 18.4298 + prepare 7.2383.
Selection takes 10.0254 ms, with seven balance passes taking 6.4994 ms;
cache scheduling/reservation takes 5.5920/5.5896 ms. Seven dependency vectors total
2,832 B; no capacity growth occurs. Three raw patches are consumed. This is a
different event from the overnight 10 km outlier.

The intermediate cold route's worst total is 44.2506 ms; final cold CPU/capture
routes peak at 18.7364/38.6678 ms. Sub-100 ms hiccups still exist; this is not a
claim of a hitch-free pipeline.

## GPU evidence

RX 9070 XT / Vulkan supports `TIMESTAMP_QUERY_INSIDE_PASSES`. The 10 km all-layers
final fixture's **last of eight unchanged repeats** reports the following actual GPU
query intervals; these are not per-layer medians or native presentation latency:

| Scope | ms |
| --- | ---: |
| Main celestial pass | 0.54744 |
| Regular terrain | 0.35428 |
| Transition/fallback | unavailable — no draw |
| Ocean | 0.02408 |
| Clouds | 0.16416 |
| Atmosphere pass/layer | 0.23412 |
| Remaining celestial spheres | 0.00068 |

Pass and draw scopes are not added to each other; GPU pipeline timestamp boundaries
need not produce additive per-stage costs. Disabled layers report `None`, not stale
values. Eight repeats remain pixel-identical. Host encode median is 0.3714 ms;
render/wait/readback median 1.8326 ms is still separately labelled host evidence.
All 17 final static BMPs also match the overnight fixture byte-for-byte
(`final/capture-previous-comparison.csv`). Intermediate GPU measurements remain
in their original manifests; differing intervals do not demonstrate an optimization.

The same fixture's regular CPU append takes 13.8484 ms: conversion/canonical sample
setup 3.1054, classification 2.6561, narrowing/boundary lookup 3.3028, byte packing
2.9730, proof lookup/calculation 0.2463/0.3689 ms. Repeated CPU preparation is much
larger than this fixture's terrain GPU interval; this does not identify the old
114 ms outlier or measure cold active-morph GPU cost.

## Upload/residency

| Descent | Median / P95 / worst terrain payload B/frame |
| --- | --- |
| Previous overnight | 3,985,696 / 5,583,760 / 8,563,360 |
| Fresh before run 1 | 3,929,952 / 5,583,760 / 8,563,360 |
| Corrected instrumented repeat | 3,888,144 / 5,490,784 / 8,563,360 |
| Final rebuilt run 1 | 3,929,952 / 5,555,888 / 8,563,360 |
| Final rebuilt run 2 | 3,929,952 / 5,583,760 / 8,563,360 |

Different wall-clock publication timelines change payloads; these values do **not**
demonstrate dirty-update savings. No persistent geometry cache exists; hit/miss rate
is therefore not applicable. The 10 km fixture retains 8,108,688 GPU terrain-buffer
bytes and uploads 8,055,072 B including the 64-byte lighting uniform each repeat,
with zero growth/waits on that repeat. The separate cold capture's 5 s frame retains
5,155,392 B and uploads 3,330,768 B, recording two growth events, one 0.0168 ms
growth wait and 0.1410 ms upload API duration. These are host API times, not GPU
transfer completion. Terrain draws are nine in the static 10 km fixture, plus
three planetary layer draws. Query buffers are bounded fixed resources, not an
unbounded VRAM cache. Driver/query object overhead is not measured.

## Memory

Every fresh descent peaks at exactly 128 MiB: **zero free accounted headroom**.
The final cold run leaves only 3,147 B, not meaningful operating margin.
No simultaneous geometry allocations were eliminated and no accounting bytes
were hidden. `memory.csv` retains raw/bookkeeping, pinned subset, worker envelopes/
fixed allowance, completed-unpublished, stitched source, transition mesh, selector
scratch, outgoing staging and boundary/proof capacities with each sampled frame.

The overnight memory audit remains relevant: class maxima include 10,180,316 B
raw/bookkeeping, 38,104,748 B worker reservations, 8,102,003 B stitched source,
21,357,200 B transition, 13,422,717 B outgoing capacity and 6,039,355 B boundary/
proof capacity against the 75,530,240 B staging envelope. These overlap and peak
at different times; they are not an additive peak breakdown. Independent active
destination/successor allocation categories, exact reservation-trigger ownership
snapshots and external RSS/VRAM remain to be instrumented before a recovery claim.

## Investigated architectures and rejected shortcuts

These are evaluation results, **not implemented or validated alternatives**:

1. **Displacement-driven bounded progression.** Exact overlay vertex displacement
   bounds the entire affine triangle field by convexity. A projected duration needs
   a lower depth bound over all affected geometry and the entire interpolation.
   Geometry crossing the near plane must not use an optimistic centre-depth bound.
   Shortening small-displacement positive-duration morphs is a candidate; a subpixel
   geometry bound alone does not prove normals/materials can hard-switch invisibly.
2. **Coalesce/retarget continuous surfaces.** Retargeting must capture the actually
   displayed complete piecewise-affine surface at one fraction, construct common
   refinement with a newer complete balanced target, and publish only if still
   current. The current builder is one-level endpoint-based and has no intermediate
   continuous-surface input. Merely replacing a ready destination would violate its
   captured source endpoint. This needs a bounded new representation, not a queue
   slot edit or an arbitrary hierarchy skip.
3. **Patch-local resident vertices.** Candidate `p = origin_f64 + offset_f32` must
   include offset narrowing, CPU relative-origin error, GPU rotation-coefficient/
   arithmetic error and final addition in its physical/projected bound. Coarse
   patches and near-plane crossings need the existing f64 clipping fallback.
   Independently reconstructed shared edges also need canonical GPU agreement;
   two patch origins with individually acceptable error do not guarantee identical
   boundary arithmetic. Body-scale f32-only positions are rejected.
4. **Resident transition endpoints.** Convex endpoint errors alone are insufficient
   without bounding every intermediate depth and GPU interpolation arithmetic.
   A whole-transition proof must invalidate on view/projection changes, retain exact
   endpoints and fall back conservatively. GPU interpolation of classifications also
   differs from classifying the current f64 interpolated position/normal; diagnostic
   endpoint and interior semantics need tests, not assumptions.
5. **Memory sharing.** Arc-backed unchanged stitched samples could remove real copies;
   safe admission would count shared allocations once across cache/coordinator/
   worker lifetimes, including cancellation acknowledgements. Reducing the existing
   worst-case staging reservation without enforcing a corresponding allocation bound
   would merely hide possible bytes and is rejected.
6. **Procedural GPU generation, higher cap or weaker thresholds.** Rejected in this
   checkpoint: measurements isolate substantial repeated CPU preparation, while
   generator determinism/certificates remain valuable. No generator port is justified
   by raw completion throughput alone.

## Correctness and validation

See `docs/evidence/phase511d/validation/` for commands and results. The matrix covers
the requested renderer/app suites through locked all-feature workspace tests in
debug/release, format, all-target check, warnings-denied all-feature/default Clippy,
warnings-denied Rustdoc and separately selected native close-surface readback.

| Final quality gate | Result |
| --- | --- |
| Format / locked all-target, all-feature check | PASS / PASS |
| All-feature / default-feature warnings-denied Clippy | PASS / PASS |
| Locked all-feature workspace debug tests | 248 passed, 0 failed, 3 ignored |
| Locked all-feature workspace release tests | 248 passed, 0 failed, 3 ignored |
| Warnings-denied Rustdoc | PASS |
| Separately selected release native readback | 1 passed, 0 failed |

Counts include workspace doc-tests and come from the final logs, not historical
reports (`validation/test-counts.json` retains per-suite results). Both modes ran
`planet_surface_lod` (8), `planet_surface_precision` (3), `view_precision` (3),
`terrain_error` (4), `terrain_geometry` (2), `terrain_stitching` (4),
`terrain_transitions` (4), `terrain_transition_cancellation` (1),
`terrain_adaptive_transitions` (7), `terrain_geometry_error` (1),
`terrain_real_transitions` (1) and `terrain_workers` (2), alongside the other
terrain/world/math suites. Worker unit fixtures also check deterministic serial/
1/2/4-worker endpoints and abandoned acknowledgement accounting. Native readback
records 0 inside-culled pixels versus 19,200 inside-no-cull and outside pixels.
The initial failed matrices remain in `validation/first-quality/` and
`validation/second-quality/`; no failed log was relabelled as passing.
`validate.ps1` sets `RUSTDOCFLAGS=-D warnings` for the Rustdoc gate and restores
the prior value afterward.

Quality results are recorded separately from performance acceptance; they cannot
override the failed convergence/headroom/outlier gates.
These are local Windows results, not a Linux, remote-CI, high-DPI or human
interactive-acceptance claim. The two ignored long-run orbital tests were not
selected for this terrain checkpoint; native readback is selected separately.

## Acceptance A

**FAIL.** Final source remains L5/L8/L18 rather than the desired L23 in the cold CPU route,
every destination still waits on the serial captured display progression, memory
headroom remains zero, and the old >100 ms event is unresolved. No trustworthy
whole-view useful-quality, human control-feel or completed rebalance claim is made.
Real-scale ocean precision and washed-out medium appearance were not changed or
accepted; shell precision shaders were not touched.

## Remaining bottlenecks

Measured: repeated regular CPU conversion/classification/narrowing/packing, CPU
transition append, exact destination construction, the fixed display endpoint gate,
and cap-full reservation/ownership pressure. Unresolved rather than guessed:
which exact event produced the overnight selection/preparation wall stalls.

Before claiming the phase complete, reproduce the old outlier under a recorded
workload with a permitted scheduler/CPU trace (or equivalent sufficiently resolved
instrumentation), then attribute and remove its cause. The current host's denied
profiling policy is a measurement blocker, not evidence that scheduling caused it.
After that gate, implement and validate bounded refinement progression, conservative
resident geometry/interpolation paths and allocation sharing. Retest whole-view
quality, native control feel and headroom under the same cap and thresholds; passing
the existing Rust tests alone is insufficient.

## Files changed in this checkpoint

- `Cargo.lock`, `crates/renderer/Cargo.toml`
- `crates/renderer/src/gpu_profile.rs` (new)
- `crates/renderer/src/{lib,celestial,terrain_capture,planetary}.rs`
- `crates/renderer/src/planet_surface/{mod,cpu_profile,lod,prepare,gpu}.rs`
  (`cpu_profile.rs` new)
- `crates/app/src/{gravity_orbits,terrain_population,planet_terrain}.rs`
- `crates/app/src/planet_terrain/{adaptive,workers}.rs`
- `crates/app/examples/{terrain_convergence_probe,solar_system_capture}.rs`
- `docs/ENGINE_MECHANICS_REFERENCE.md`
- This report and `docs/evidence/phase511d/`.

The working tree also contains preserved overnight and unrelated changes. Git's
whole-tree status is retained separately; it is not a list of just this checkpoint.

## Evidence paths and commands

[Evidence index](evidence/phase511d/README.md), `summary.json`, per-frame CSVs,
profiles, GPU manifests/readbacks and quality transcripts provide reproduction.
Exact commands are in the evidence index and validation JSON. No timed probe was
run alongside Cargo validation. `wpr -status` / `wpr -start CPU -filemode` establish
the scheduler-trace blocker, not a recorded CPU trace. Precommit Git state is retained
as `docs/evidence/phase511d/git-status-short.txt`.

Final reproduction entry points (use a fresh destination for measurements):

```powershell
./docs/evidence/phase511d/validate.ps1
./docs/evidence/phase511d/measure-final.ps1 -Destination docs/evidence/phase511d/final/NEW_RUN
./docs/evidence/phase511d/summarize.ps1
git diff --check
git status --short
```

`final/commands.json` records the locked release build, exact sequential executable
invocations and UTC boundaries. `final/source-sha256.csv` fingerprints crate Rust,
WGSL, manifests, lockfile and toolchain; `final/executable-sha256.csv` identifies the
measured binaries. Those source hashes were checked again after measurements with
no mismatches. `validation/quality-results.json` records all eight passing gates.
