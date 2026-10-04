# Phase 5.10B — Acceptance A recovery checkpoint

**Acceptance A: FAIL. Morphology remains blocked and unchanged.** The worker
foundation was committed as `11a16d8` before this recovery. This is an incomplete
recovery checkpoint, not Phase 5.10 completion. Nothing has been pushed.

Latest measured code is the `retained-*` series in the
[evidence index](evidence/phase510b/README.md). The subsequent body-center observer
safety fix skips radial prefetch when no direction exists; none of the measured
routes places the observer at the body center. Earlier `final-*` and `audit-*`
directories are intermediate versions, not the latest results.

## 1. Partial checkpoint commit

Foundation `11a16d8` contains the validated worker/convergence foundation. Recovery
changes described here retain the same terrain, 128 MiB hard cap, exact stitched
coverage/common-refinement topology, 0.125/0.0625 px split/merge thresholds, and
150 ms native morph duration. Unrelated design/workflow edits are not part of the
engine recovery. Delivery hashes are recorded in section 19.

## 2. Why LOD25 was requested

At 60° vertical FOV and 768×512, nadir tangent-plane pixel footprints are
2.255274489 m (1 km clearance), 0.225527449 m (100 m), 0.022552745 m (10 m),
and 0.004510549 m (2 m). LOD25 Grid16 patches are about 2.1 cm wide by 1.9 cm,
with 1.174–1.320 mm neighbor spacing. At 1 km that spacing is approximately
1/1,700 of a pixel footprint, not a physically meaningful universal target.

The old ungenerated certificate used a global height interval, forcing the
conservative displaced-ball projection depth to its 0.1 m near-plane floor at
every tested clearance. Filtered interpolation consequently dominated the fine
LOD error. This was a certificate/projection problem, not missing terrain bands.

## 3. Revised useful target and its limitation

`quality-after/targets.csv` gives the first nadir level meeting the unchanged
0.125 px certificate threshold, including the two-level boundary-owner allowance:

| Clearance | Certificate target | Maximum Grid16 spacing | Total error |
| ---: | ---: | ---: | ---: |
| 1 km | LOD17 | 0.337993447 m | 0.031134176 px |
| 100 m | LOD18 | 0.168996727 m | 0.095798451 px |
| 10 m | LOD20 | 0.042249084 m | 0.106418602 px |
| 2 m | LOD22 | 0.010562268 m | 0.048706367 px |

Before correction, all four targets were LOD25, spacing 0.001320283 m, total
0.044195992 px. These are conservative certificates, not observed pixel errors.
The convergence probe looks tangent to the surface at ≤10 m and requests radial
LOD23 there; its projection is different from the nadir audit. The diagnostic
`useful_target_lod` currently equals the certified radial target, not an
independently established central-visible-view target. Invisible underfoot patches
cannot establish screen-wide quality. That interpretation/evidence gap remains.

## 4. Publication granularity and local scheduling

Readiness already used one local balanced closure, not the entire globally desired
cover. No global desired-cover barrier was removed. Each closure still constructs
and stitches a complete cover, then serially constructs/publishes its exact morph.
The selector now prioritizes the visible camera-radial split and prefetches the
next local balanced closure while the current endpoint is constructing/morphing.
Prefetch never advances topology or publishes incomplete siblings.

For interior LOD9→10/10→11/11→12 splits, generated dependencies are 28/24/32;
edge splits need 16 and corner splits 12. This is local, not a balancing explosion.
Exact boundary ownership, all stitch-touch counts and all morph-touch counts are
not yet separately instrumented; mask-change counts are not substitutes.

## 5. Critical-path corrections

The all-stitch face-domain triangle diameter is bounded by
`D = sqrt(5) * 2/(16 * 2^LOD)`, checked across every triangle in all 16 stitch
masks. Arbitrary-triangle interpolation uses `M D²/6`: Taylor's remainder is
bounded by half the Hessian envelope times the weighted vertex variance, whose
three-weight maximum is `D²/3`. `/8` is invalid for equilateral triangles; a sharp
quadratic regression prevents that unjustified reduction.

Ungenerated and generated patches now share the center-filtered-height ± global
gradient × cap-angle interval, expanded by unresolved/numeric allowances and
intersected with the global bound. No sampled extrema replace proof. Global
gradient/Hessian envelopes and filtering remain unchanged. Certificates are
cached within the current immutable terrain binding, not weakened.

## 6. Worker usefulness and throughput

Latest cold runs produce 325/408/425 raw patches with 2/3/4 workers over about
five seconds: approximately 65/82/85 patches/s. Recorded raw/cover worker CPU is
2.346/2.709/2.861 CPU-seconds respectively. Three and four workers both reach
source LOD16; these single runs do not establish an optimal pool size. The existing
serial/1/2/4 fixed-workload throughput comparison remains in Phase 5.10 evidence;
it is not re-labelled as a new recovery benchmark. Native default remains four.

The zero-headroom descent completes 968 raw patches, of which 26 (2.69%) complete
while classified as useful to the current local closure. Prefetch, results already
ready before that closure, and nonlocal useful work are excluded. This is **not**
evidence that 97.31% of work is wasted. Full mutually exclusive completed-job
categories (current closure/prefetch/other retained/obsolete/discarded) and eventual
usefulness are still missing. Worker CPU time is not app-thread latency.

## 7. Cold convergence timeline

Four workers, 2 m complete-terrain clearance, tangent view, 150 ms morph:

| Requested ms | Actual ms | Rendered source | Highest ready radial | Desired radial |
| ---: | ---: | ---: | ---: | ---: |
| 0 | 3 | none | none | 23 |
| 100 | 113 | 0 | 1 | 23 |
| 250 | 257 | 1 | 2 | 23 |
| 500 | 513 | 2 | 3 | 23 |
| 1,000 | 1,009 | 4 | 5 | 23 |
| 2,000 | 2,002 | 7 | 8 | 23 |
| 3,000 | 3,011 | 10 | 11 | 23 |
| 5,000 | 5,004 | 16 | 18 | 23 |

All samples remain quality-pending and unsettled. Two workers reach source12,
ready14 at 5,004 ms; three reach source16, ready17 at 5,004 ms. Descent benefits
from earlier views and reaches source22 at its final 2 m checkpoint, still against
desired23; it is not cold convergence. Reaching source4/7 at one/two seconds fails
the requested interactive useful-quality gate. Twenty-three serial 150 ms morphs
alone require 3.45 s, before generation/construction and frame opportunities.
No arbitrary LOD clamp or shorter morph was used to manufacture a pass.

## 8. Memory and headroom

All measured routes satisfy the hard 128 MiB accounting cap, but operational
headroom acceptance fails. Experimental soft waterlines of 0/8/12/16 MiB permit
critical pinned work to use the hard quota; operational reservations receive
credit to avoid double reservation. No production waterline was selected.

| Soft headroom | Peak accounted MiB | Raw completed | Evictions | Aggregate cancellations | Worst update + prepare ms |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 0 MiB | 127.999345 | 968 | 236 | 90 | 21.862 |
| 8 MiB | 127.999345 | 968 | 236 | 97 | 27.674 |
| 12 MiB | 127.999345 | 968 | 236 | 93 | 23.983 |
| 16 MiB | 127.996630 | 921 | 202 | 184 | 105.158 |

Zero/8/12 finish only **687 bytes** below the cap. Soft-waterline misses and raw
generation reservation rejections are zero in these runs, not proof that all
publication/retry admission succeeded. The 16 MiB single run is slower and provides
no acceptable headroom improvement; its timing cannot be causally assigned to the
waterline alone. Native/default headroom stays zero, explicitly not accepted.

Zero-headroom descent capacity/reservation class maxima:

| Class | MiB |
| --- | ---: |
| Raw geometry plus bookkeeping | 11.739285 |
| Pinned raw subset | 8.367767 |
| Worker reserved output/input envelopes | 36.776396 |
| Worker fixed/stack allowance | 2.500000 |
| Sampled completed-unpublished cover reservation | 0.000000 |
| Stitched source | 8.006912 |
| Active morph mesh | 21.843643 |
| Selector scratch | 0.170761 |
| Actual outgoing renderer capacity | 12.787548 |
| Actual boundary plus proof-cache capacity | 5.757695 |
| Full renderer staging allowance | 72.031250 |
| Frame-end accounted maximum | 119.994300 |
| Aggregate recorded peak | 127.999345 |

These classes overlap, peak at different times, and must not be added. Zero sampled
completed-unpublished reservation does not prove no such reservation ever existed.
Capacity accounting includes envelopes, not allocator/RSS/GPU/driver residency.
Separate publication/retry rejection counts and external memory peaks remain open.

## 9. Long frames and measured costs

The historical 97.082 ms frame had no renderer substages; its exact cause remains
unproved. A later intermediate 392.4775 ms frame was attributed to 378.2901 ms
transition packing with per-triangle `reserve_exact`; that path now uses
hard-budget-bounded geometric growth. Do not apply this attribution retroactively.

Latest CPU distributions below are median / nearest-rank P95 / worst, in ms.
Totals are computed per frame, not sums of separate quantiles.

| Route | App update | CPU prepare | Update + prepare |
| --- | ---: | ---: | ---: |
| Descent, 0 MiB | 2.804 / 5.547 / 10.008 | 7.038 / 12.403 / 18.916 | 9.759 / 16.089 / 21.862 |
| Cold, 2 workers | 2.442 / 11.690 / 29.208 | 3.667 / 19.125 / 39.320 | 8.157 / 27.251 / 45.032 |
| Cold, 3 workers | 1.076 / 4.177 / 6.422 | 3.518 / 9.587 / 13.081 | 5.428 / 10.898 / 14.908 |
| Cold, 4 workers | 1.182 / 3.879 / 6.119 | 3.844 / 9.826 / 14.071 | 5.621 / 10.868 / 15.805 |
| Movement | 1.208 / 3.316 / 4.045 | 2.422 / 7.971 / 8.822 | 4.693 / 9.889 / 11.823 |
| Body switch | 0.265 / 0.943 / 3.426 | 0.768 / 3.476 / 4.495 | 1.002 / 3.817 / 4.877 |

In zero-headroom descent, selection is 2.022/4.095/8.334 ms; raw publication
0.0008/0.0067/0.0177 ms; cover publication 0/0.0003/0.1050 ms; scheduling
0/0.0160/0.0867 ms; diagnostics 0.0092/0.0109/0.9204 ms; main-thread stitch
input/submission 0/0.4967/1.3676 ms. Main-thread morph construction is zero for
the worker route. These instrumented scopes do not sum to the entire update.

The latest worst zero-headroom frame is at the 100 m view, 1,158.088 ms:
3.2787 ms update + 18.5833 ms preparation = 21.8620 ms. Regular stitched append is
3.8739 ms, transition append 14.6582 ms, with 34,160 input triangles and 20,089
emitted; transition sample/clipping/packing are 4.9704/3.9507/3.3327 ms. There are
zero staging growths, 170 proof-cache hits and zero misses. Remaining cost is not
the old allocation/proof-miss problem. The 16 MiB trial's 105.158 ms worst includes
80.6012 ms update (63.6466 ms selection) and 24.5568 ms prepare. Its wall-time
profile is not evidence of a specifically identified computational root cause.

## 10. Stitching and dependency width

`dependencies.csv` reports generated dependencies / balance children /
mask-changed neighbors:

| Region | 9→10 | 10→11 | 11→12 |
| --- | ---: | ---: | ---: |
| Interior | 28 / 24 / 12 | 24 / 20 / 4 | 32 / 28 / 11 |
| Edge | 16 / 12 / 4 | 16 / 12 / 4 | 16 / 12 / 4 |
| Corner | 12 / 8 / 2 | 12 / 8 / 2 | 12 / 8 / 2 |

Reuse remains exact raw-input/stitch identity based. No immutable split/merge
topology-template cache was added. Complete-cover stitch work remains on the
critical path, though executed off-thread. Local dependency width is not the
total number of stitched or morphed patches.

## 11. Morph construction

Exact overlay, barycentric endpoints and canonical shared boundaries remain.
Integer determinant/GCD fast paths accelerate exact candidate rejection and
rational operations without approximating output. Inner overlay loops now poll
cancellation, returning no partial mesh. Reservations remain until acknowledgement.

Across distinct zero-headroom descent transition profiles, median/P95/worst ms:
validation 2.783/7.666/13.689; mapping/bbox 3.341/12.520/24.510; exact clipping
1.466/8.450/14.954; endpoint capture 3.527/15.893/28.749; canonicalization
1.524/25.799/104.420; allocation/emission 0.792/3.410/6.579. Up to 341,674
candidate triangle pairs are considered and 34,160 triangles emitted per recorded
construction. Worker completion is still a serial cover/morph gate. The
`stitch_worker_ms`/`morph_worker_ms` fields repeat the last completion on subsequent
frames; their frame-weighted quantiles are not independent per-job distributions.

## 12. Renderer preparation and precision-proof reuse

An exact-input proof cache fits within the existing 8 MiB boundary allowance. It
keys source frame, patch address, stitch mask, projection and **all bitwise f64 and
GPU-f32 positions**. It reuses only a precision decision/error envelope; normals,
colors, observer conversion and outgoing packing remain fresh. Signed zero is
distinguished; changed view/viewport/mask inputs miss. No unchanged GPU-payload or
renderer-resident geometry reuse was implemented.

Zero-headroom descent records 532,577 hits / 2,909 misses (99.46% hits). Cold-four
records 19,644/471 (97.66%); continuous movement records 0/13,803, as expected for
changed exact inputs. Regular preparation median/P95/worst ms: boundary
0.397/0.625/1.017, setup 0.023/0.048/1.076, sample/conversion 3.648/5.834/6.751,
proof 0.183/0.310/4.083, packing 1.260/2.000/2.348. Transition scopes are separate:
samples 0.563/2.430/5.002, clipping 0.291/1.716/4.138, packing 0/2.364/5.802.
Fallback growths are zero at median/P95, worst12 with 3,932,160 new bytes and the
same recorded copy upper bound. Full sample conversion/restaging still dominates
many frames. CPU preparation is not GPU execution or presented frame time.

## 13. Movement and cancellation

The scripted approach/tangent motion/look reversal/retreat run completes with cap
and body-ownership assertions intact, 59 aggregate cancellations, and 11.8225 ms
worst update + prepare. Coordinator tests cover running and completed-unpublished
obsolete refinement, merge reversal, retained source charge and resubmission
rollback. The body-center regression now skips radial prefetch without losing
the active transaction. The inner-overlay interruption regression polls 15,000
times and compares uncancelled output bitwise; this is not a cancellation-latency
distribution. Human camera-control feel, arbitrary fast-movement behavior and
precise acknowledgement latency remain unmeasured.

## 14. Multiple bodies

The five-second 350 ms Earth→Moon→Mars→Earth switch route records 14 aggregate
cancellations, peak 119.575548 MiB, and 4.8774 ms worst CPU update + prepare.
Assertions check actual body ownership/reference radius and cap, not merely CSV
labels. This establishes scripted switching safety, not useful-quality convergence
on all three bodies, GPU/compositor responsiveness or arbitrary teleport deadlines.

## 15. Determinism

Serial/1/2/4 raw generation identity and cover/morph comparison regressions retain
bitwise outputs for Earth/Moon/Mars and their existing addresses. Cancellation never
publishes partial meshes. Exact stitch/transition endpoint and common-refinement
tests remain required. Different wall-clock completion timelines and prefetch
counts are not evidence of nondeterministic terrain. No generator/morphology
parameter, source of generation salt or worker-count terrain identity was changed.

## 16. Physical scales and conservative bounds

Earth radius is 400 km. Fine-level global gradient/Hessian envelopes are
4.511701785490×10⁶ m and 6.127260175344×10¹⁰ m. The represented-height amplitude
bound (~13,015.625 m) is not interpolation error. At LOD24 the old interpolation
term is 2.722053662884×10⁻⁵ m but projects to 0.174291744 px under the erroneous
universal 0.1 m depth floor. With corrected diameter/remainder, LOD24 interpolation
is approximately 2.83547×10⁻⁶ m. Unresolved terrain is zero by LOD14; by LOD16
boundary/numeric terms are numerical-only (~4.6977×10⁻⁸ m each at 1 km). Morph
remaining error is zero in the static quality audit, not during active transitions.
All per-term metre/pixel values, regional depth floors and filtering footprints are
retained in the before/after quality CSVs.

| LOD | Maximum patch width m | Minimum Grid16 spacing m | Maximum Grid16 spacing m |
| ---: | ---: | ---: | ---: |
| 10 | 692.086687 | 38.421193 | 43.275812 |
| 12 | 173.035452 | 9.612738 | 10.815992 |
| 14 | 43.262733 | 2.403906 | 2.704001 |
| 16 | 10.815791 | 0.601007 | 0.675992 |
| 18 | 2.703943 | 0.150253 | 0.168997 |
| 19 | 1.351970 | 0.075126 | 0.084498 |
| 20 | 0.675985 | 0.037563 | 0.042249 |
| 21 | 0.337993 | 0.018782 | 0.021125 |
| 22 | 0.168996 | 0.009391 | 0.010562 |
| 23 | 0.084498 | 0.004695 | 0.005281 |
| 24 | 0.042249 | 0.002348 | 0.002641 |
| 25 | 0.021125 | 0.001174 | 0.001320 |

These are actual normalized-cube geodesic neighbor/midline distances at the stated
direction/reference radius, not a planet-average or a heuristic error multiplier.
Conservative regional interval tests cover six radii including large-body paths.
Terrain horizon occlusion remains disabled without an independent proof.

## 17. Acceptance A and retained captures

**FAIL.** Useful visible quality within one/two seconds is not established: cold
source4/7 is far below radial23, every checkpoint remains quality-pending, and the
serial morph floor alone exceeds that convergence window. Nearly cap-full memory,
remaining restaging/overlay costs and unmeasured human/GPU/OS responsiveness prevent
an acceptance claim even on the faster CPU route.

Native offscreen Readability and LOD captures from an AMD Radeon RX 9070 XT are
retained as PNGs with the exact manifest/timeline. Inspection of initial/250/500/
1,000/2,000 ms LOD views shows no terrain initially, coarse flat bands at 250/500 ms,
and blank views at 1,000/2,000 ms. The clean 5,000 ms Readability view is a coarse
blue foreground band, not accepted inspection quality. These images do not prove
missing topology: a tangent view above complete truth can face away from or lie
below the coarse filtered mesh. They positively do **not** establish readiness as
visible-quality convergence. Captures are a separate run (5,009 ms final), not
images of the exact headless timed frames. No morphology work follows this gate.

## 18. Validation

Final Windows validation passes; [transcripts](evidence/phase510b/validation/) are
retained separately from performance probes:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `cargo check --locked --workspace --all-targets --all-features` | PASS |
| `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | PASS, default features |
| `RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --all-features --no-deps` | PASS |
| `cargo test --locked --workspace --all-features` | 230 passed, 0 failed, 3 ignored; 63 result suites |
| `cargo test --locked --release --workspace --all-features` | 230 passed, 0 failed, 3 ignored; 63 result suites |
| `cargo test --locked --release -p mundaris_simulation --test orbits -- --ignored --nocapture` | Both long-orbit tests pass |
| `cargo test --locked --release -p mundaris_app --features terrain-capture,surface-profile --test native_close_surface -- --ignored --nocapture` | Native displaced-shell regression passes |

Both workspace runs include the 163,840-sample eight-seed/all-stitch full-truth
error stress (204.20 s debug, 26.46 s release) and the 360 regional interval/ball
locations. Stress residual assertions retain their existing 1 µm numeric tolerance;
passing is not a zero-error claim. Proof-cache tests verify forced-miss/warm byte
equality, invalidation, signed-zero handling and bounded eviction. Native shell
readback records inside culled/no-cull = 0/19,200 pixels, outside = 19,200.
Cancellation/source accounting, body-center prefetch, serial/1/2/4 determinism,
exact stitching/morph endpoints and large-radius precision regressions pass.
Final default-feature Clippy and Rustdoc were repeated after the last documentation
comment correction. These checks cannot override failed responsiveness/visual
acceptance. Linux native, current-revision remote CI, human/high-DPI/OS sleep-recovery
and GPU timestamp evidence remain open.

## 19. Commits

Foundation: `11a16d8` — worker/convergence foundation, Acceptance A still open.
Recovery implementation: `6216cdc` — certificate and local-work recovery,
Acceptance A still fails. This report and its retained evidence are delivered in
the following documentation commit; identify it with
`git log -1 -- docs/phase-5-10b-acceptance-a.md`. Neither implementation checkpoint
is Phase 5.10 completion.

## 20. Push and next blocker

Nothing pushed. No morphology tuning, threshold relaxation, LOD clamp, terrain-band
removal, reduced draw distance, shortened morph duration or cap increase occurred.
The next required architectural decision is how to avoid the one-level serial
cover/morph critical path while preserving exact continuous coverage and the
unchanged morph duration. Also unresolved: independent central-visible quality
interpretation, effective operational headroom, completed-job categories,
publication/retry counters, precise cancellation latency, split/merge topology
templates, unchanged GPU-payload reuse and GPU/operator/platform validation.
