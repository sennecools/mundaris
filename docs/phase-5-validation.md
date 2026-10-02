# Phase 5 implementation and validation evidence

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
