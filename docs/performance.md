# Performance policy

- Profile representative workloads before optimizing. Complexity, custom allocators, unsafe code, caching, multithreading, or GPU compute require a concrete workload and evidence.
- Keep future hot paths optimizable: avoid needless clones and hidden allocations, favor contiguous processing of large homogeneous data, and avoid unnecessary dynamic dispatch or lock-heavy designs.
- Inspect per-frame allocation intentionally once frame-loop work exists. Do not build an allocator or allocation framework now.
- Keep CPU and GPU responsibilities separable. Move suitable work to portable GPU compute only after profiling demonstrates a reason.
- Data-oriented/SoA layouts may help very large homogeneous collections; do not impose them on every domain type preemptively.
- Caches are disposable derived state, never the only copy of authoritative edits or simulation state. Keep future caches bounded and rebuildable.
- Add focused, reproducible benchmarks when performance-sensitive algorithms exist. Do not run benchmarks in normal CI, and compare against meaningful baselines while recording hardware context.
- Preserve headroom for streaming, observer-relative detail, and compact procedural world descriptions rather than assuming all world objects remain resident as heap objects.
- Deterministic generation must remain reproducible while optimizing. Prefer explicit seeds/coordinates and deterministic local derivation over ambient random state.
- Avoid hidden `O(n^2)` work in systems expected to scale. Make expected complexity visible and validate it against realistic input sizes.

## Phase 1 baseline — 2026-10-01

Host: AMD Ryzen 7 9800X3D, 8 reported cores/logical processors; Windows 11 Pro x86-64, build 26200. Rust 1.98.1 stable (`48a229cea`, LLVM 22.1.8), `x86_64-pc-windows-msvc`. Cargo's unmodified optimized bench profile; no native-CPU flags, custom allocators, parallel conversion, caching, LTO or unsafe project code. `glam 0.30.10` uses default/std features. Lockfile selections: Criterion 0.8.2 (default features disabled), naga 27.0.3, wgpu 27.0.1, egui 0.33.3, winit 0.30.13. Benchmark implementation is in the Phase 1 commits through `402a046`; app-only follow-up changes do not change timed library operations.

Reproduce:

```bash
cargo bench --locked -p mundaris_math --bench reference_frames
cargo bench --locked -p mundaris_renderer --bench view_preparation
```

Both targets ran locally. Each benchmark uses 20 samples, 100 ms warm-up and a requested 500 ms measurement period (Criterion extends it for long batches). Graph/point/output allocation, device startup, shader compilation and logging are outside timing. Inputs are nonconstant, passed through `std::hint::black_box`; outputs/tree publication are observed. Renderer batches reuse initialized output. Math prepared batches include one preparation per timed batch; direct queries prepare once per point. Direct/prepared equality is verified before timing. Reports/raw distributions live under ignored `target/criterion/`, not in normal CI.

### Repeated point conversion: depth-8 cross branches

Latest sample median and bootstrap 95% confidence interval, measured in microseconds unless labelled ms. Throughput is calculated from the median, not a hardware-independent guarantee.

| Points | Direct median [95% CI] | Prepared median [95% CI] | Direct / prepared throughput | Speedup |
| --- | --- | --- | --- | --- |
| 1,024 | 335.296 [334.027, 338.430] µs | 18.878 [18.750, 19.120] µs | 3.05 / 54.24 M points/s | 17.8× |
| 65,536 | 22.031 [21.831, 22.264] ms | 1.204 [1.189, 1.216] ms | 2.97 / 54.45 M points/s | 18.3× |

The long direct batch had two high-severe outliers (mean 22.967 ms versus median 22.031 ms). These are recorded rather than replaced by a timing assertion. Adding per-batch preparation to the 1,024-point measurement increased its central estimate by about 2.7% from the earlier preprepared measurement, consistent with the extra preparation work. No implementation optimization was introduced.

### Preparation

All same-frame, sibling, root/descendant and cross-branch groups ran at depths 1/4/8/32, separately for pose and motion. Representative Criterion central time estimates (these are regression/mean estimates, not medians):

| Pair / depth | Pose | Motion |
| --- | --- | --- |
| Same / 1–32 | 18.9–19.0 ns | 34.6–35.0 ns |
| Sibling / 1–32 | 43.4–43.7 ns | 89.6–91.9 ns |
| Root→descendant / 1 | 31.3 ns | 64.1 ns |
| Root→descendant / 4 | 97.0 ns | 133.2 ns |
| Root→descendant / 8 | 172.1 ns | 228.3 ns |
| Root→descendant / 32 | 632.4 ns | 806.9 ns |
| Cross branch / 1 | 44.1 ns | 90.9 ns |
| Cross branch / 4 | 174.5 ns | 233.0 ns |
| Cross branch / 8 | 316.8 ns | 415.1 ns |
| Cross branch / 32 | 1.240 µs | 1.568 µs |

Shared depth does not add work to sibling/same-frame composition. Cross-branch and root queries grow with walked depth as designed; no quadratic scaling or path allocations appeared in inspection.

### Renderer preparation and checked narrowing

View/source preparation central estimates were 55.3–56.6 ns across local 100 m, 10 km, shared `1.5e11 m`, and shared `1e16 m` fixtures. Preparation and conversion are measured separately. Batch medians/95% CI below are for 65,536 vertices:

| Fixture / budget | Median [95% CI] ms | Median throughput |
| --- | --- | --- |
| 100 m / `1e-5 m` | 2.169 [2.153, 2.193] | 30.21 M vertices/s |
| 10 km / `1e-3 m` | 2.166 [2.154, 2.178] | 30.26 M vertices/s |
| Shared `1.5e11 m`, local 100 m | 2.158 [2.147, 2.185] | 30.37 M vertices/s |
| Shared `1e16 m`, local 100 m | 2.148 [2.125, 2.169] | 30.51 M vertices/s |

The 1,024-vertex central estimates were 33.81–34.05 µs (about 30 M vertices/s). The `1e16` long batch had two outliers; its broader mean/slope interval does not indicate a structural offset-dependent cost. Checked `hypot`, rotation and round-trip error checks are deliberately retained. No performance threshold or speculative optimization follows from these measurements.

### State publication and fixed visible subset

`F64` and `F4096` denote **non-root** frames (65/4,097 total nodes). A 64-node identity-root tree has only 63 editable edges; this explicit naming permits the requested 64-edge publication workload without editing the root. Updates are preallocated/index ordered. Both 0 and 65,536 app-owned attached points exist in the workload; publication never traverses them. Preparation of a fixed eight-frame subset follows publication, measured separately.

| Non-root frames / updates | Update median, 0 attached | Update median, 65,536 attached |
| --- | --- | --- |
| 64 / 64 | 91.894 ns [91.670, 92.216] | 92.325 ns [91.292, 93.089] |
| 4,096 / 64 | 98.788 ns [98.517, 100.117] | 99.909 ns [99.092, 100.862] |

One-edge central estimates were 3.66–3.78 ns (roughly 265–273 M updates/s). Sixty-four-edge publication yields roughly 641–696 M edges/s from these medians. The eight-source preparation estimates were 215–224 ns total (about 36–37 M prepared pairs/s), independent of attachment count. Subnanosecond differences and Criterion comparisons from short runs are not evidence of an attachment-dependent engine cost.

The very small publication measurements were investigated by explicitly observing the tree with `black_box` after each update and rerunning the publication group. Timings remained similar. Inspection confirms two bounded U-entry loops and no node/attachment traversal or allocation; the small fixed subset remains hot in CPU cache. The slightly higher 4,096-frame result does not grow proportionally to F, and no quadratic behavior was found. No counting allocator or external allocation profiler was installed; allocation expectations are supported by code inspection, not an invented profiler measurement. Topology edits intentionally allocate O(F) temporary metadata; UI/command objects and first-use GPU/staging allocation are outside the conversion promise.

### Numerical tradeoffs and platforms

Benchmarks use valid bounded inputs; they never bypass precision validation. Source coordinates already rounded through astronomical root space cannot recover their low bits. Debug geometry has a finite representation budget, not a universal world cutoff. See [validation evidence](phase-1-validation.md) for numerical maxima and native Windows results. Linux build/release/interactive and current-revision remote CI remain unverified; no Linux timing baseline is claimed.

## Phase 2 baseline — 2026-10-01

Host: AMD Ryzen 7 9800X3D, 8 reported cores/logical processors; Windows 11 Pro
x86-64 10.0.26200. Rust 1.98.1 stable (`48a229cea`, LLVM 22.1.8),
`x86_64-pc-windows-msvc`. Unmodified optimized bench profile, Criterion 0.8.2;
no dependency upgrades, native-CPU flags, custom allocator, cache or parallelism.

```bash
cargo bench --locked -p mundaris_world --bench celestial_system -- --sample-size 20 --warm-up-time 0.1 --measurement-time 0.5
cargo bench --locked -p mundaris_world --bench frame_projection -- --sample-size 20 --warm-up-time 0.1 --measurement-time 0.5
```

Both targets ran. Values below are Criterion central estimates and 95% confidence
intervals, **per complete batch**, in microseconds. Short measurements are a local
baseline, not an engine performance guarantee. Raw reports are under ignored
`target/criterion/`. Inputs/outputs and post-edit/publication systems are observed
through `black_box`. Lookup measures ID validation/address lookup, not reading
every physical field. Property edits replace validated mass/radius without name
allocations. Full-state batches are prepared once and reuse duplicate scratch.
Projection republishing updates all 2B edges, not a dirty subset. Build includes
tree/mapping/staging allocations and destruction. Live append uses Criterion
batched setup outside timing, then one body insertion plus full projection
publication (including vector capacity growth); returned systems are destroyed
outside the timed routine. Live-append throughput is not an O(1) insertion claim.

| Bodies | Lookup µs [95% CI] | Property edit µs [95% CI] |
| --- | --- | --- |
| 64 | 0.03135 [0.03120, 0.03150] | 0.03718 [0.03696, 0.03745] |
| 1,024 | 0.5129 [0.5067, 0.5187] | 0.7394 [0.7341, 0.7446] |
| 16,384 | 8.143 [8.103, 8.180] | 15.046 [14.944, 15.156] |

| Bodies | Full state µs [95% CI] | Projection build µs [95% CI] | Full republish µs [95% CI] | Live append µs [95% CI] |
| --- | --- | --- | --- | --- |
| 64 | 0.1191 [0.1179, 0.1206] | 2.050 [2.032, 2.076] | 0.7237 [0.7144, 0.7332] | 6.814 [6.590, 6.980] |
| 1,024 | 2.383 [2.373, 2.395] | 43.993 [43.560, 44.411] | 12.726 [12.633, 12.825] | 37.523 [32.789, 40.614] |
| 4,096 | 10.763 [10.671, 10.859] | 518.06 [515.42, 521.31] | 58.062 [57.833, 58.273] | 269.42 [260.24, 282.27] |

Full state and republish show approximately body-linear work with working-set
effects. Construction grows more sharply at 4,096 bodies; it includes increasing
allocations/reallocations and destruction, unlike hot republishing. No allocator
profile was collected, so the cause is not established. Code inspection finds
bounded linear passes plus amortized vector appends, no nested body traversal or
quadratic algorithm. Live-append estimates have broad allocation/batching-sensitive
intervals. Outliers were present in lookup/edit/state/build/append groups and are
retained. No optimization or timing threshold was introduced to hide them.

Allocation expectations are based on code inspection, not a counting allocator:
state batches use pre-existing duplicate flags and caller updates; full projection
republish reuses its vector once topology is prepared. Neither has per-body heap
allocation in the hot path. Body insertion/name authoring, projection build/growth,
UI drafts/markers and renderer first use can allocate. No clock microbenchmark was
added. Linux timings remain unverified.
