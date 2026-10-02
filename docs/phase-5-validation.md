# Phase 5 implementation and validation evidence

## Phase 5.5 gradient-directed erosion checkpoint — 2026-10-02

**Implementation exists; full Phase 5.5 visual acceptance is not established.**
The [Phase 5.5 construction](../MUNDARIS_PHASE_5_5_GRADIENT_DIRECTED_EROSION.md)
records the executable mathematical adaptation, rather than replacing this
validation evidence with another specification. The generic stateless ridge/valley
shaping below is **superseded as the primary mountain erosion model by Phase 5.5**
in V2. V1 and its historical measurements remain intact.

### Implemented architecture

- `world/src/terrain/erosion.rs`: one bounded, analytic, feature-anchored straight
  gully primitive, quintic partition blending, rounded Gaussian creases, regularized
  slope fade and normalized [-1,0] incision. Fixed anchor coefficients avoid an
  invalid frozen-query-gradient derivative approximation.
- `world/src/terrain/generator.rs`: one-to-five recursive octaves. Each smaller
  feature samples gradients modified by earlier erosion at its fixed body-space
  anchor. Different seeded body-space rotations, no face axes or tangent charts.
  Stack fading attenuates subsequent features in existing creases. A bounded
  query/batch-local memo is discarded after evaluation; no persistent flow state.
- V2 replaces the Regional ridge/valley contribution using its existing amplitude
  budget, preserves V1 macro/range salts and domain transforms, and leaves Local
  and Fine as separate additional relief. Definition equality includes erosion
  count/strength and immutable generator version; old cached geometry cannot match.
- Exactly-zero footprint weights skip erosion evaluation entirely. Feature
  coefficients use the complete preceding field regardless of query footprint,
  making the sum of omitted weighted amplitudes a valid residual bound. Complete
  analytic gradients feed the unchanged radial-graph normal calculation.
- The existing cache, readiness and renderer path are reused. Grid16 positions and
  normals still consume 13,872 bytes. No per-patch erosion map is introduced.
- Native opt-in preview uses V2; `MUNDARIS_PHASE55_AB=legacy` selects V1. The same
  seed/body/camera route applies. Uniform covers remain restricted to levels 0–4;
  that physical sampling suppresses the preset erosion hierarchy entirely.

### Numerical validation and acceptance distinction

The original geometry/error assertions remain unchanged. The 163,840-comparison
suite now spans eight seeds, one-to-five erosion octaves, strengths 0/0.25/0.5/0.75/1,
levels 0/4/10/18/30 and all sixteen stitch index variants. Its release rerun passed
with no under-bound. Index-variant tests do not authorize mixed-LOD displaced drawing.

New app tests compare V2 height/gradient and analytic normals at canonical same-face,
cross-face, corner and parent/child shared directions, and prove version/config
cache separation plus deterministic eviction/regeneration. The existing nine
cache/readiness regressions also pass. World tests cover scalar/batch/chunk equality,
full-minus-filtered residuals, axis/diagonal directions and gradient-feedback A/B.

An initial centered-difference oracle used a minimum direction step of 1e-8. Its
2 m footprint test failed even with erosion strength zero, from truncation in the
existing fine-band field. The reference was improved to a centered Richardson
step sweep (1e-9 through 1e-7), without changing runtime derivatives. The new
derivative assertion is stricter than the initial experiment (1e-4 rather than 1%).
Finite differences are test-only. No old assertion was relaxed.

The final sweep contains 10,584 derivative references: maximum scaled error
2.4066099448838917e-5, p95 2.206435503120489e-7, maximum absolute error
0.06780838314443827 m/unit direction. Disabling earlier-octave feedback changes
heights by up to 135.6195430522055 m in the directed comparison. These are numerical
regressions, not a proof of branching morphology; the analytic construction and
certificate derivation supply the mathematical contract.

### Isolated CPU measurements

Windows x86-64 MSVC, Ryzen 7 9800X3D, Rust 1.98.1, optimized bench profile.
Criterion uses 20 samples, 100 ms warmup and requested 500 ms measurement.
Tables use `new/estimates.json` median point estimates, not the console's slope
estimates. Raw runs are `target/phase55-world-final.txt`,
`target/phase55-patch-final.txt` and `target/phase55-live-preset-final.txt`.
No concurrent benchmark runs, GPU timing or integrated-frame speedup is claimed.

Two fixtures must not be conflated. The historical comparison uses identity 17,
seed 0x5eed, R=6,371,000 m, band octave counts 3/3/3/3/2 and default erosion
spacing 64/16/4 km. The actual live preset uses identity 0x415552454c4941,
seed 17, counts 2/3/4/4/4 and spacing 128/32/8 km. All results include height,
analytic derivative and normal; there is no V2 value-only evaluation path.

Historical complete-field (zero footprint) configuration comparison:

| Generator | Scalar µs | Batch32 ms | Batch289 ms |
| --- | ---: | ---: | ---: |
| V1 | 3.747 | 0.115441 | 1.036225 |
| V2, 1 octave | 27.012 | 0.308243 | 2.443458 |
| V2, 2 octaves | 57.940 | 1.019029 | 9.255733 |
| V2, default 3 | 137.863 | 2.728291 | 27.529950 |
| V2, 4 octaves | 332.713 | 6.846013 | 72.115650 |
| V2, 5 octaves | 448.403 | 15.929150 | 175.899050 |

Historical default-three footprint comparison:

| Footprint | Scalar µs | Batch32 ms | Batch289 ms | Active/faded/skipped |
| --- | ---: | ---: | ---: | --- |
| 50 km | 2.500 | 0.075398 | 0.685564 | 0/0/3 |
| 10 km | 3.045 | 0.093840 | 0.839843 | 0/0/3 |
| 1 km | 3.036 | 0.092768 | 0.836693 | 0/0/3 |
| 100 m | 57.907 | 1.007703 | 9.059633 | 2/0/1 |
| 10 m | 138.597 | 2.731583 | 27.614650 | 3/0/0 |
| 2 m | 138.920 | 2.732562 | 27.794300 | 3/0/0 |

Actual live-preset default-three comparison:

| Generator / footprint | Scalar µs | Batch32 ms | Batch289 ms | Active/faded/skipped |
| --- | ---: | ---: | ---: | --- |
| V1 complete | 3.864 | 0.117573 | 1.062392 | — |
| V2 50 km | 2.523 | 0.076541 | 0.691545 | 0/0/3 |
| V2 10 km | 2.857 | 0.085754 | 0.777934 | 0/0/3 |
| V2 1 km | 24.611 | 0.197305 | 1.585532 | 0/1/2 |
| V2 100 m | 119.418 | 2.320493 | 21.725850 | 2/1/0 |
| V2 10 m | 119.830 | 2.334279 | 21.573550 | 3/0/0 |
| V2 2 m | 119.669 | 2.315917 | 21.558650 | 3/0/0 |
| V2 complete | 120.198 | 2.354548 | 21.660475 | 3/0/0 |

Live-preset batch289 actual primitive/feature totals at 50 km, 10 km, 1 km,
100 m, 10 m, 2 m and complete are respectively 10404/0, 11560/0, 21600/2312,
260067/60624, 260645/60624, 261223/60624 and 261512/60624. This counts recursive
anchor dependencies and memo reuse, not merely emitted octaves. Historical
default-three per-sample batch averages are 36/0, 44/0, 44/0, 417.34/58.08,
1207.75/233.80, 1209.75/233.80 and 1209.75/233.80. Zero features at coarse
footprints verify genuine early rejection. Operation counts vary with spatial
coherence, seed, domain transforms and direct-mapped memo collisions.

Complete Grid16 cache misses include 289 geometry vertices, certificates, normals,
queue/publication and cache destruction, with the actual default 8-vertex chunks.
Cache setup is outside timing. They are not equivalent to arbitrary batch directions.

| Level | Footprint m | Historical ms | Live preset ms |
| --- | ---: | ---: | ---: |
| 0 | 796375 | 0.506640 | 0.437406 |
| 4 | 49773.438 | 0.731791 | 0.737861 |
| 6 | 12443.359 | 0.889421 | 0.823050 |
| 10 | 777.710 | 2.130275 | 1.755867 |
| 13 | 97.214 | 3.567136 | 6.402850 |
| 16 | 12.152 | 7.268625 | 6.508888 |
| 18 | 3.038 | 7.399713 | 6.550200 |
| 19 | 1.519 | 7.320350 | 6.589300 |

Chunk measurements traverse 32 patches; median/worst are observed wall-clock
durations, not deadline guarantees or Criterion estimates.

| Vertices | Historical coarse µs | Historical fine µs | Live coarse µs | Live fine µs |
| --- | ---: | ---: | ---: | ---: |
| 8 | 19.6 / 53.7 | 198.3 / 416.6 | 19.7 / 46.6 | 175.7 / 795.2 |
| 16 | 38.7 / 104.3 | 296.9 / 661.5 | 38.9 / 187.4 | 269.9 / 603.2 |
| 32 | 77.1 / 94.0 | 494.6 / 934.6 | 77.4 / 156.0 | 457.6 / 802.0 |
| 64 | 153.4 / 174.4 | 890.1 / 1203.3 | 154.6 / 196.8 | 834.0 / 1402.6 |

### Scheduling and memory

The measured recursive cost supports small chunks, not unrestricted synchronous
generation. Live work uses 8 vertices per chunk, at most 64 below a 5 km footprint;
coarse work retains the prior 1,156-vertex ceiling. The existing 2 ms cutoff is
checked between chunks, so overshoot remains possible. No worker pool was added.
Four/five-octave complete-field costs are a known scalability limitation, not the
production default. No claim of a hard 2 ms generation deadline is made.

Historical 4,096-entry traversal retained 4,096 patches and evicted 256. Resident
and peak accounting were 58,954,756 bytes (56.22 MiB), versus the prior 56.12 MiB.
The byte-cap traversal retained six entries at 250,660 bytes under a 262,144-byte
quota. Aggregate budget remains 128 MiB; geometry payload remains 13,872 bytes.
The live preset single-patch harness reports 153,972 resident/peak bytes including
cache storage. Chunk accounting has median delta zero and worst delta 13,872 bytes
at publication. These are cache-accounting deltas, not heap-allocation counts.
The 256-slot memo is temporary stack-local storage; scalar/world batches allocate
no heap memory by code inspection. No allocator profiler was run.

### Final validation commands

All completed successfully on Windows:

```text
cargo test --locked --workspace --all-features
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked -p mundaris_math -p mundaris_world -p mundaris_renderer -p mundaris_simulation -p mundaris_app --release --test terrain_noise --test terrain_generation --test terrain_bounds --test terrain_bands --test terrain_erosion --test terrain_error --test terrain_geometry --test terrain_identity --test planet_terrain --test terrain_geometry_error -- --nocapture
cargo test --locked -p mundaris_simulation --release --test orbits long_run -- --ignored --nocapture
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps
cargo fmt --all -- --check
cargo bench --locked -p mundaris_world --bench terrain_erosion
cargo bench --locked -p mundaris_app --bench planet_terrain_generation
cargo bench --locked -p mundaris_app --bench planet_terrain_erosion
```

The focused release suite ran 37 tests; both ignored orbital long runs passed.
Runtime validation preceded the separate live-preset benchmark addition; that
benchmark itself compiled and ran successfully. Linux and remote CI were not run.
The inherited ridge transform's behavior at extremely large accepted softness
values has not been investigated here; this checkpoint does not certify that
untested extreme configuration beyond the existing validation contract.

### Visual evidence and retained limitations

`app/examples/erosion_capture.rs` generates deterministic analytic CPU A/B views:
planet, continent, range, mountain face, gully, peak, valley, cross-face direction,
cube corner and the same mountain face at 1 km/100 m/2 m footprints. BMP outputs
under `target/phase55-captures` can be converted to PNG for inspection. These use
20x exaggerated normal lighting and are **not native displaced-mesh captures**.
Final Gaussian inspection shows coherent elongated incisions and intersecting detail,
with prominent local/fine noise; convincing hierarchical branching has not been
demonstrated. Continuity and feedback tests alone do not
establish the intended mountain morphology.

Native Windows captures under `target/phase55-live` exercise V1/V2 through the
existing Vulkan route. Timed-route screenshots are directed operational evidence,
not a guaranteed identical camera snapshot or temporal quality acceptance. At the
supported uniform footprint erosion is absent; these cannot establish live gully
quality. Reference-sphere navigation can enter displaced terrain, producing
uninformative filled close views. No visual seam/axis/peak/valley acceptance is
claimed from those views.

The committed [capture index](evidence/phase55/README.md) contains 24 exact-sample
CPU A/B PNGs and two inspected native orbital PNGs, with parameters and explicit
limitations. The initial periodic cosine prototype was rejected for worm-like
stripes; only the final non-periodic Gaussian evidence is retained. The reused
mountain mask has no explicit ocean exclusion. Pointwise peak preservation,
valley quality, broader seed/pole/axis surveys and matched-camera live fine-terrain
inspection remain open acceptance gates. No bypass of footprint filtering was
introduced to manufacture live detail.

### Checkpoint commits

| Commit | Scope |
| --- | --- |
| `5a6b552` | Certified body-fixed gradient-feedback world implementation, tests and benchmarks |
| `2066246` | App preview/config identity, bounded scheduling, seam/cache tests, benchmarks and capture example |

Construction, measurement and capture documentation follows in a separate closeout
commit. Nothing was pushed. Unrelated existing design/workflow changes remain
excluded. This is a working, numerically validated but **not fully accepted**
Phase 5.5 checkpoint; no subsequent LOD transition work was started.

Mixed-LOD displaced stitching and common-refinement morphing remain unimplemented;
replacement popping and the coarse horizon remain. Terrain certificates still do
not fully drive adaptive selection. No Phase 4 redesign, worker-thread generation,
true hydraulic simulation, per-patch erosion texture or budget increase is included.

## Displaced geometry/cache checkpoint — 2026-10-02

**Implemented, with restricted live integration; not complete Phase 5 acceptance.**
The foundation record below is historical. Its statements about missing bands,
geometry and cache are superseded by this section, not retroactively rewritten.

### Implementation and foundation audit

- Audited identity/revision, normalization, physical footprint, derivative units,
  analytic intervals/error components, outward normals, f64 precision and
  caller-owned batches. No concrete foundation behavior bug was found. Existing
  analytic fixtures/noise/error projection remain intact.
- `world/src/terrain/generator.rs` implements all five additive output bands:
  macro basin/continental contrast and separately seeded uplift; range-scale
  mountain masks and warped anisotropic soft ridges; regional ridge/valley
  remapping; masked local relief; finite fine geometry. Seeds, tagged mixing,
  rotations/offsets, frequencies and transform order are the initial executable
  V1 contract. This completes a previously unevaluated V1 definition, not a
  silent change to an earlier released procedural algorithm.
- Metre scale means domain frequency R/lambda; macro angular scale means
  cycles/TAU. Fixed octave doubling/halving remains explicit in band records.
  Warp Jacobians, mask products, tanh contrast, anisotropy and soft-ridge
  derivatives use analytic chain/product rules. No finite differences, slope
  attenuation, simulation, neighborhood erosion or patch-local procedural state.
- Generator setup validates derived domains and finite derivative/certificate
  arithmetic before definition/radius publication. Scalar and caller-owned
  batch evaluation share the fixed-order core; batch weights are compiled once.
  Inactive profiles skip primitives; reports count actual mask/warp calls.
- `renderer/planet_surface/terrain_geometry.rs` retains only 289 f64 body
  positions/normals, address, reference radius, footprint, extent and six-term
  error record. Construction checks finite/outward samples, canonical radial
  alignment and the explicit radius envelope. No permanent height/direction array.
- `app/src/planet_terrain.rs` owns the exact definition/body/revision/radius key,
  sorted compact LRU, pins, bounded requests and one resumable private builder.
  Grid/schema are fixed to this executable's grid16 representation; footprint is
  derived deterministically from radius/address. Definition equality is checked,
  not an unchecked hash. Movement, rotation, camera and frame lifetime are absent
  from keys. Compilation and topology are reused, not rebuilt per microbatch.
- `TerrainReadyCover` retains the complete old uniform cover until every target
  sample is Ready. Roots remain pinned. Obsolete requests/builders are canceled
  at update boundaries; changed truth cancels stale publication. A new revision
  uses the far representation while new roots are pending, never mixes revisions.
- Renderer staging/`CelestialFrame::append_generated_surface` borrow ready
  geometry and retain source-centred f64 conversion, checked narrowing, clipping,
  GPU layout and frame poisoning. Renderer contains no terrain definition/query.
  Optional elevation colours derive from cached positions in the existing unused
  normal.w slot, including clipped triangles; changing colours generates no terrain.
- `gravity_orbits.rs` exposes an opt-in checkpoint preview and the original sphere
  comparison path. Fixture authoring is an explicit command, not a side effect of
  rendering. Enabling terrain by default initially broke the existing small-radius
  Phase 4 edit regression; fixture authoring was moved behind preview activation,
  and the original assertion passes unchanged. This was an integration correction,
  not a defect in transactional world radius validation.

### Certificates and tests

Current procedural regional queries use the **analytic global fallback**. They
include every configured band regardless of footprint. Tight cap subdivisions and
local mask exclusion are not implemented; these certificates are valid but loose.
Represented/tail envelopes follow the same per-octave weights; omitted detail is
never discarded from total error. Positive product/sum rounding includes subnormal
allowances. Global noise gradient/Hessian operator bounds, warp stretch and all
smooth remap/product bounds are propagated analytically.

For normalized cube mapping, ||Dn||<=1 and ||D²n||<=3. Displacement F=h(n)n has
L<=G+A and M<=H+5G+3A. The patch certificate takes the minimum of independently
valid interval, Lipschitz and Hessian interpolation bounds using the conservative
all-stitch four-step face-domain diameter. Sphere correspondence also uses the
mapping Hessian bound in addition to the existing Phase 4 bound. Numeric margin
is explicit. Boundary/morph contributions are zero **only because these operations
are disabled in the uniform display**, not placeholders for active transitions.

- Eight seeded randomized legal configurations × 4,096 area-weighted directions,
  footprints 0/2/1,000/50,000 m: scalar/batch/chunk identity, height envelopes,
  full-minus-filtered residual bounds and independent tangent derivative sweeps.
- Eight seeds × levels 0/4/10/18/30 × all 16 stitch variants × 256 non-grid
  barycentric references: **163,840** full-field versus filtered triangle checks,
  with no certificate under-bound. Testing topology errors does not authorize
  drawing unreconciled displaced mixed-level edges.
- Nine app cache/cover tests cover cold/resumed generation, hits, LRU refresh,
  eviction/regeneration, seed/radius/address separation, revision cancellation,
  canonical face edges/corners and parent/child samples at the same profile.
  Delayed level-1 replacement retains six complete roots; settled 1,000 updates
  generate zero samples. No logical coverage holes were observed.
- Generated renderer tests check flat packed-data equivalence, actual displacement
  and normal changes, radius/profile/stitch rejection, diagnostic elevation bytes
  and poisoning on missing geometry. Existing Phase 4 assertions are not weakened.

### Scheduling, memory and measurement scope

The synchronous baseline admits at most 1,156 vertices/update and 2 ms, checked
between at-most-32-vertex batches. Headless tests disable wall cutoff. Requests
are capped at 256, entries at 4,096, and partial builders at one. Pin pressure
remains pending and retains old coverage. Metadata/certificates are built once
at patch completion; their cost is included in completion chunks. No worker
threads, SIMD backend, GPU generation, new dependencies or allocator were added.

Accounting measures actual native struct sizes and Vec capacities/Box lengths:
geometry, normals, exact keys, cached metadata, entry slots, requests, inline
compiled builder and shared topology. It is **allocated-capacity accounting**,
not an OS RSS/allocator-header/driver memory profile. The live cache reserves
16 MiB of the aggregate 128 MiB for cover/reference/staging/transition allowance;
the cache itself is limited to 112 MiB. This is the design's aggregate reservation,
not an increase to the 128 MiB budget. No transition payload is currently allocated.

The Criterion groups use 20 samples, 100 ms warmup, requested 500 ms measurement
(slow view groups extend automatically). Viewer closed, sequential groups,
complete outputs observed with black_box, same Windows/Ryzen environment as the
foundation. Patch misses include canonical sampling, full active field, normals,
metadata/certificates, validation and publication; cache construction is excluded
by batched setup. Cache-drop/destruction is included where ownership leaves the
timed closure. Chunk diagnostics are individual wall measurements, not Criterion
confidence estimates. Position-only generation and a value-only V1 specialization
are not implemented: current elevation calls include analytic derivatives.

### Measured checkpoint results

Final Criterion **medians**, with 95% median intervals from `new/estimates.json`:

| Operation | Median | 95% interval | Scope |
| --- | ---: | ---: | --- |
| Complete V1 point, elevation + derivative | 3.615 µs | 3.607–3.631 µs | Five bands, 14 configured octaves |
| V1 batch289, rho 50 km | 682.071 µs | 678.258–690.951 µs | 36 primitives/sample |
| V1 batch289, rho 1 km | 921.719 µs | 919.114–929.676 µs | 47 primitives/sample |
| V1 batch289, rho 128 m | 970.350 µs | 968.282–971.733 µs | 49 primitives/sample |
| V1 batch289, rho 2 m | 1,046.331 µs | 1,041.059–1,050.075 µs | 52 primitives/sample |
| V1 complete batch289, rho 0 | 1,040.120 µs | 1,038.713–1,044.445 µs | 52 primitives/sample; 3.599 µs/sample |
| Full cached patch miss, level 0 | 497.795 µs | 495.402–499.175 µs | 289 positions + normals, rho 796,375 m |
| Full cached patch miss, level 4 | 718.075 µs | 716.551–725.732 µs | rho 49,773.438 m |
| Full cached patch miss, level 10 | 956.264 µs | 954.181–960.891 µs | rho 777.710 m |
| Full cached patch miss, level 18 | 1,081.283 µs | 1,075.772–1,093.528 µs | rho 3.037930 m |
| Warm cache hit | 13.7 ns | 13.6–13.8 ns | Eight-slot cache; not a 4,096-entry latency claim |
| Eviction + two generations | 1,456.900 µs | 1,445.315–1,468.048 µs | Level 10 then regenerated root, one-entry cache |

Benchmark preset: R=6,371,000 m, identity 17, seed 0x5eed, V1;
macro/range/regional/local/fine have 3/3/3/3/2 octaves respectively. The actual
live fixture has its separately documented authoring preset (2/3/4/4/4 octaves),
so the timing table is not a claim of worst-case legal configuration throughput.
All full patch payloads allocate **13,872 bytes**, 48 bytes/vertex; complete
generation includes authoritative analytic normals and certificate validation.

Individual chunk diagnostics generated 32 level-18 patches (9,248 vertices):

| Vertex budget | Chunks | Median | Worst, final run | Completed patches/chunk |
| ---: | ---: | ---: | ---: | --- |
| 1 | 9,248 | 3.9 µs | 34.1 µs | 0 or 1 |
| 8 | 1,184 | 29.3 µs | 71.4 µs | 0 or 1 |
| 16 | 608 | 58.3 µs | 117.3 µs | 0 or 1 |
| **32 (selected)** | 320 | **116.4 µs** | **275.2 µs** | 0 or 1 |

The last chunk can contain fewer vertices and includes completion work. Maximum
allocation delta is 13,872 bytes at builder start, zero for ordinary continuation
and publication. Across the repeated runs the largest observed selected-32 chunk
was 275.2 µs; another 16-vertex run had a 318.3 µs outlier. Full-patch chunking
medians for budgets 1/8/16/32 were 1,031.448/973.260/960.555/959.419 µs.
32 is retained because throughput is comparable to 16, has fewer admission/setup
calls, and measured overshoot is well below the 2 ms opportunity. These are host
observations, not a hard real-time guarantee. A native generation opportunity
capture recorded 803 vertices/three patches in 2.016 ms; settled captures recorded
zero vertices/patches and approximately 0.001 ms generation opportunity work.

Memory/traversal evidence:

- Native entry slot is **464 bytes**, including the exact 264-byte identity,
  120-byte generated record, 64-byte cached sphere metadata, access/pin/padding.
  Box payload adds 13,872 bytes: **14,336 bytes per occupied reserved entry**.
- 4,096-slot empty cache bookkeeping: **2,029,732 bytes**. Traversal generated
  4,352 different level-7 patches: 4,096 retained, 256 evictions; resident and peak
  **58,849,444 bytes (56.12 MiB)**, including all retained capacities. Count limit
  binds before the full 128 MiB byte cap for this representation.
- A 262,144-byte test quota, 64 entry slots, traversed 320 patches: seven retained,
  313 evictions, resident/peak **255,988 bytes**, proving byte-cap eviction rather
  than only entry-count enforcement. Pinned pressure tests preserve covering data.
- Native approach retained **2,046 patches**, reported **29.00 MiB** cache peak,
  then generated zero settled samples. The matched view has 1,542 resident entries
  (six roots plus 1,536 level-4 leaves): cache **23,420,356 bytes**, with cover
  bookkeeping **23,760,668 bytes total (22.66 MiB)**. Actual OS/allocator peak and
  GPU-driver residency were not measured. No reason to increase 128 MiB was found.

Matched camera: 10,000 km reference clearance, 60° FOV, 1280×800. Both comparison
paths draw **the same 1,536 uniform level-4 patches / 443,904 samples**, with no
horizon rejection. Terrain generation is outside timing, pending=0; steady app
group asserted zero generated vertices and accumulated 132,096 cache hits, zero
lookup misses/evictions. Output payload is **14,303,232 bytes**.

| Matched steady workload | Sphere reference median | Cached terrain median |
| --- | ---: | ---: |
| Renderer preparation only | 41.232 ms | 31.833 ms |
| App CPU: selection + readiness + lookup + preparation | 43.679 ms | 34.993 ms |

The app comparison includes identical uniform-cover readiness work in both paths;
physics is paused, with no guides/UI/upload/GPU/presentation timing. These are **not
direct terrain overhead against the accepted 10.7824 ms Phase 4 result**: that
result used 510 covering/402 visible/116,178 samples and horizon rejection. The
interim uniform preview draws much more geometry and misses that baseline. Cached
conversion is cheaper than rebuilding canonical sphere positions at matched counts,
but this does not claim the frozen Phase 4 performance issue solved. Native full
frame distributions, cold-cover activation distributions and a fully adaptive
matched terrain view remain open. Generation opportunity spikes are separately
reported above, not included as steady-state generation.

**289-sample clarification:** the historical ~223.01 ns is the entire optimized
analytic linear batch, not per-sample Criterion normalization. `Throughput::Elements`
only reports throughput alongside the whole-iteration time. It writes 289 reused
caller outputs using the simple linear expression, with zero noise/mask/warp calls,
and black-boxes input/output. About 0.77 ns/output is therefore its interpretation.
No evidence establishes compiler elision, and vectorization was not independently
isolated. The complete V1 batch instead measures approximately 1.040 ms and must
be used for its own workload's capacity planning.

### Validation commands and outcomes

Passed on Windows for this checkpoint:

```text
cargo fmt --all -- --check
cargo check --locked --workspace --all-features
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps
cargo test --locked -p mundaris_math -p mundaris_world -p mundaris_renderer -p mundaris_simulation -p mundaris_app --release --test terrain_noise --test terrain_generation --test terrain_bounds --test terrain_error --test terrain_identity --test terrain_bands --test terrain_geometry --test planet_terrain --test terrain_geometry_error
cargo test --locked -p mundaris_simulation --release --test orbits long_run -- --ignored --nocapture
cargo bench --locked -p mundaris_app -p mundaris_world --bench planet_terrain_generation --bench planet_terrain_view --bench terrain_generation
```

The focused release invocation ran 32 tests; both ignored orbital long runs were
run separately and passed. A release-test attempt while the native executable was
running hit Windows executable locking; rerunning after native shutdown passed.
No test assertion was relaxed to fix this platform artifact. Native route captures
at planetary scale, approach, 10 km and 2 m horizon show broad elevation colouring,
no evident gaps/NaNs in those snapshots, and a visibly coarse faceted horizon.
Earlier thin screen lines aligned with orbital guides; they are not proof of
terrain cracks or of temporal crack absence. Native capture client was 2558×1408;
the benchmark viewport is deliberately different and is not native FPS evidence.

### Live restriction and remaining gates

Only **complete uniform-level covers, levels 0–4, stitch mask zero** are drawn.
The existing Phase 4 desired visible level drives the requested preview level;
it still selects desired patches independently. Expanded terrain balls disable
smooth-sphere horizon rejection. Displayed projected errors use complete terrain
certificates, but those certificates do **not yet drive adaptive LOD selection**.
This interim restriction is deliberately coarse quality debt, not new Phase 4
thresholds or an alternative transition architecture.

Equal-level shared geometry and normals are verified. Mixed-level coarsest-incident
boundary reconciliation, two-row blending, transition closure, common-refinement
morphing and a general balanced ready ancestor cover remain unimplemented. Whole
uniform replacements can pop. No claim of crack-free displaced LOD transitions.
The far error includes the complete height envelope, but full subpixel displaced
far/surface transfer and terrain-clearance navigation are not accepted yet. Picking
and camera clearance remain explicitly reference-sphere approximations.

Windows Vulkan/Radeon RX 9070 XT release startup and an automated approach/
inspection route were exercised. Captures are directed checkpoint evidence,
not temporal/human acceptance, Linux validation or current remote CI. Close-range
uniform terrain is visibly too coarse for range/local inspection; the faceted
horizon and unresolved quality debt are expected limitations. Distinct mountain
systems and all cube-face/corner views have not received visual acceptance.

No Phase 4 architecture redesign or dependency change was necessary. The live
restriction must be removed through the planned canonical ownership/adaptive
readiness milestone, not skirts, patch-local terrain or independent morph timers.

Implementation commits for this checkpoint:

| Commit | Unit |
| --- | --- |
| `ca2f01b` | Certified procedural band composition and focused world evidence |
| `a0709f5` | Borrowed displaced geometry, preparation and diagnostic shader path |
| `a80ddc7` | Bounded cache, resumable work, uniform readiness and benchmarks |
| `aface7e` | Opt-in live terrain preview and terrain-aware far envelope |

Documentation/measurement closeout follows separately. Nothing was pushed.
Pre-existing design/workflow working-tree changes were preserved and excluded
from the implementation commits.

## Historical foundation checkpoint

## Implemented checkpoint — 2026-10-02

This is implementation/validation evidence, not a replacement terrain design.
The approved Phase 5 specification remains authoritative. Its §22 orders analytic
fixtures/certificates and transition proofs before artistic bands/live integration.
Phase 4 mapping, cover, stitching, thresholds, readiness, preparation and GPU paths
remain unchanged. The accepted approximately 11 ms full-view baseline is retained.

Completed foundation units:

- `math/src/noise.rs`: full signed-coordinate SplitMix64-finalizer hashing,
  fixed twelve-gradient table, eight-corner quintic interpolation, analytic first
  derivatives, same-order value-only path and conservative global envelopes.
- `world/src/terrain/{mod,query}.rs`: explicit immutable identity/seed/V1/config,
  five finite band records, normalized signed zero, separate checked terrain
  revision, validated body-fixed query location and physical footprint.
- Optional body terrain association and transactional definition/radius validation.
  Terrain publication does not change celestial revision, properties, kinematics,
  sample instant, frame coherence or orbital replay/history. Radius is an independent
  evaluation/cache-key input; a radius edit need not change definition revision.
- Pure Flat/constant/linear analytic fixture scalar/batch queries, tangent
  derivatives, elevation/slope and radial-graph normals. Caller-owned batches can
  be sliced into 32-sample chunks without changing sample bits.
- Smooth 4–8 footprint taper accepting a **certified effective wavelength**.
  Tests cover 50 km/2 m footprints; no claim yet that V1 controls/warps have been
  compiled or their effective scales certified.
- Generic directional caps and outward-rounded analytic regional height intervals,
  global derivative bounds and zero analytic unresolved tail.
- Renderer-domain explicit six-term metre error accounting, outward-rounded
  `0.5 M D²` interpolation helper and projection through existing bounds math.
  Independent affine/quadratic fixtures test the barycentric cancellation proof.

The initial §22 stages 1–3 have foundation implementations; stage 4 is **partial**.
There is no compiled V1 band evaluator yet. Config amplitude sums are conservative
authoring envelopes for bounded future output transforms, not a completed interval
proof of a procedural expression DAG. Analytic fixtures deliberately have no
procedural detail and ignore footprint; they are not labelled the V1 field.

Remaining, dependency ordered: procedural regional/derivative/residual certificates
and chart-composed error integration; generated geometry/cache/queue/readiness;
coarsest-incident boundaries; exact common-refinement closure/morph proofs; cached
staging/GPU/far handoff; V1 macro/range/regional/local/fine fields and their effective
filter scales; integrated fixtures/diagnostics and matched stress/performance/native
validation. No artistic tuning, workers, editing or persistence was introduced.

## Correctness evidence

Windows headless checks passed:

```text
cargo test --locked -p mundaris_math -p mundaris_world
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked -p mundaris_math -p mundaris_world -p mundaris_renderer -p mundaris_simulation --release --test terrain_noise --test terrain_generation --test terrain_bounds --test terrain_error --test terrain_identity
cargo test --locked -p mundaris_simulation --release --test orbits long_run -- --ignored --nocapture
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps
cargo fmt --all -- --check
```

The ordinary workspace suite leaves two orbital long-run tests ignored; the focused
release command ran both successfully. No Linux, remote CI or native terrain evidence
is claimed. Focused release terrain integration binaries contain 16 tests; a separate
world unit test checks terrain-revision overflow.

Noise tests cover four independently checked integer hash vectors, repeated sample
bits, value-only/gradient value bit agreement, seed separation, finite/envelope
checks over 16,384 deterministic points, first derivative step sweeps (1e-3–1e-6),
numerical Hessian checks and three-axis lattice/arbitrary-point continuity.
The exact integer vectors were independently reproduced with arbitrary-precision
integer arithmetic, not assumed correct solely because the implementation emitted them.

Analytic tests cover all directed face edges/corners, levels 0/1/5/16/30,
parent/child canonical points, 4,096 area-weighted directions, independent numerical
derivatives/normals, invalid-input transactions and deterministic orbital replay
after repeated terrain publications. Regional sweeps contain 307,200 cap samples
over signed/zero amplitudes, cap sizes and centres. These samples are regression
evidence; the analytic spherical dot extrema supply the interval proof.

Independent review found an underflow issue in the original analytic interval
margin. Endpoint/margin operations now widen outward, with positive/negative
least-subnormal regressions. Interpolation products and error sums also round
outward. Review of the correction found no remaining blocking issue in that scope.

Primitive derivation: interpolation weights form a convex partition. Normalized
table dot magnitudes are at most sqrt(2); final scaling by 1/sqrt(3) leaves a strict
sqrt(2/3) envelope below one, including roundoff slack. With fade derivative at
most 15/8 and second derivative at most 6, the published component bounds 4.5/20
dominate first/second derivatives. These are not yet regional warped-field bounds.

## Measured CPU microbenchmarks

Windows x86-64 MSVC, AMD Ryzen 7 9800X3D, Rust 1.98.1, optimized Cargo bench profile;
Criterion 20 samples, 100 ms warmup, requested 500 ms measurement. Sequential
CPU-only runs, no window/device/GPU setup, preallocated world batch output. Table
uses Criterion median estimates and 95% bootstrap intervals from `new/estimates.json`.
Raw distributions/outliers remain under ignored `target/criterion/`.

| Workload | Median | 95% median interval | Samples / primitive calls |
| --- | ---: | ---: | --- |
| Noise value only | 47.22 ns | 47.07–48.24 ns | 1 / 1 |
| Noise height + derivative | 61.11 ns | 61.00–61.27 ns | 1 / 1 |
| Noise scalar loop, 289 | 17.548 µs | 17.432–17.763 µs | 289 / 289 |
| Noise scalar loop, 32 | 1.930 µs | 1.925–1.945 µs | 32 / 32 |
| Analytic linear query, rho 50 km | 10.62 ns | 10.58–10.69 ns | 1 / 0 |
| Analytic linear query, rho 2 m | 10.69 ns | 10.65–10.71 ns | 1 / 0 |
| Analytic normal only | 23.69 ns | 23.58–23.86 ns | 1 / 0 |
| Analytic cap certificate | 29.03 ns | 28.96–29.14 ns | 0 / 0 |
| Flat batch, 289 | 83.23 ns | 82.97–83.99 ns | 289 / 0 |
| Analytic linear batch, 289 | 223.01 ns | 222.64–225.48 ns | 289 / 0 |
| Analytic linear chunk, 32 | 30.52 ns | 30.16–30.93 ns | 32 / 0 |

Noise loop costs are approximately 60.7 ns/sample (289) and 60.3 ns/sample (32);
linear batches approximately 0.77/0.95 ns/sample. Simple analytic batches permit
compiler optimizations unavailable to scalar validating queries. Noise workloads
use seed 17 for loops, changing scalar seeds and bounded 3D coordinates; no terrain
definition, footprint, bands or patch geometry is involved. World fixtures use
R=6,371,000 m, A=1,000 m, axis normalize(1,2,3), cap half-angle 0.1 rad, no V1
bands/warps/masks or primitive calls, and no generated patches/uploads.

The certificate median increased from approximately 23 ns to 29 ns after the
correctness fix. This is accepted, not optimized away. The measured 32-noise-call
loop is **not** the full generation microbatch: real V1 controls, normals,
certificates/topology and queue publication are not yet timed. No worker decision,
generation latency or integrated-frame speedup follows from these numbers.

## Memory, live integration and remaining risks

Measured native layouts: `TerrainDefinition` 232 bytes, `TerrainConfig` 216 bytes,
`TerrainSample` 32 bytes; caller output for 289 samples is 9,248 bytes. Immutable
definitions live with bodies; scalar queries allocate nothing and batches write
caller storage. No retained geometry/cache exists: terrain cache residency is
currently **0 bytes**, not validation of the future aggregate 128 MiB cap. Future
cache/container/builders/certificates/boundary/transition accounting remains mandatory.
No heap-allocation profiler was run.

Terrain is **not integrated into the live planet renderer**. It still displays
the Phase 4 smooth sphere. Existing no-hole/stitch/navigation regression tests
pass, but displaced terrain transitions/morph closure are **unimplemented and
unvalidated**, not claimed crack-free. No terrain visual scenes were run. Integrated
frame impact versus the accepted 10.7824 ms median Phase 4 preparation baseline
has **not been measured**. GPU payload/upload is unchanged by these query-only tests.

No architecture contradiction or Phase 4 redesign was required. The existing
workspace `glam` dependency was promoted from world dev-only to runtime for f64
query derivatives, without adding a package/version or changing Cargo.lock.
Future V1 setup must still validate all derived domain/warp/remap certificate
inputs before publication, not rely only on today's finite authoring controls.
Opaque horizon, revision-staged render apply, full generation-chunk cost and exact
morph topology/closure remain integration gates. Existing Phase 4 platform/operator
acceptance debt remains open.
