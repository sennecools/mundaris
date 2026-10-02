# Phase 5 foundation evidence

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
