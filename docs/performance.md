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

## Phase 3 baseline — 2026-10-01

Host: AMD Ryzen 7 9800X3D (8 reported cores/logical processors), Windows 11 Pro
x86-64 10.0.26200; Rust 1.98.1 (`48a229cea`), LLVM 22.1.8,
x86_64-pc-windows-msvc. Unmodified optimized bench profile, glam 0.30.10 default/std,
Criterion 0.8.2, naga 27.0.3, wgpu 27.0.1, egui 0.33.3 and winit 0.30.13.
No dependency versions, native CPU flags, LTO, unsafe, parallel reduction or force
algorithm were changed. Benchmarks ran sequentially without simultaneous timed
groups. Resource-budget groups were rerun through `a45f713`, then initialization
and normal-count history/replay/hierarchy groups after the `dcfe0e2` boundary
follow-up. Earlier groups with unchanged force/vertex operations were retained.

```text
cargo bench --locked -p mundaris_simulation --bench gravity
cargo bench --locked -p mundaris_simulation --bench fixed_steps
cargo bench --locked -p mundaris_renderer --bench celestial_preparation
cargo bench --locked -p mundaris_app --bench trail_history
cargo bench --locked -p mundaris_world --bench celestial_system -- --sample-size 20 --warm-up-time 0.1 --measurement-time 0.5
cargo bench --locked -p mundaris_world --bench frame_projection -- --sample-size 20 --warm-up-time 0.1 --measurement-time 0.5
```

All groups ran. Default new targets use 20 samples, 100 ms warmup and requested
500 ms measurement. Criterion extends large batches: initial 1024-body 512-step
groups each required about 140 s for 20 iterations. After full-output observation
and shared live/private memory accounting were finalized, affected groups were
rerun with 10 samples (about 70 s each at N1024):

```text
cargo bench --locked -p mundaris_simulation --bench fixed_steps -- 'committed_512_history|private_replay_512' --sample-size 10
cargo bench --locked -p mundaris_app --bench trail_history -- hierarchy_committed_512_trails_projection
```

Black-box complete workspace/system/runner/staging/history outputs, not just counts.
Successful hot work reuses storage. Allocation/setup, topology/device/window/shader
startup are outside warm timings except the explicitly named initialization/build
groups. Raw distributions and bootstrap estimates are under ignored
`target/criterion/`. No timing assertion runs in correctness tests or ordinary CI.

### Forces and fixed steps

Scale probes use deterministic noncoincident index-derived 16×16×layers lattice,
spacing 1e9 m, masses 1e18+(index%7)·1e16 kg and small index-derived velocities;
h=0.001 s resolves every pair. These measure scale, not long-run orbital accuracy.
Pure force excludes chi/integration; warmed KDK includes chi and checked full-time
candidates. Gather copies dense IDs/masses/full checked states into reused staging.
Initialization separately includes allocation, gather and first resolved force.
History-off batches use KDK/full world commit directly; history-on additionally
uses runner/tick validation/ring. They are not an isolated ring-only comparison.
Private 512-work batches include baseline initialization and 511 private KDK steps;
final live publication is outside this restarting probe. Restore roundtrip includes
one forward step and one retained restoration/force recomputation/publication.

Median and bootstrap 95% confidence interval below; µs unless labelled ms/s.
Steps/s and h·steps/s are CPU-only estimates at the actual scale-probe h=0.001.
Publication/history are included in the history-on batch; projection/render are not.

| N / pairs | Force median [95% CI] µs | 512 committed + history median [95% CI] | Steps/s / sustainable scale-probe rate |
| --- | --- | --- | --- |
| 3 / 3 | 0.04852 [0.04802,0.04903] | 68.127 [67.527,68.330] µs | ≈7.52 M / 7520x |
| 16 / 120 | 1.4535 [1.4318,1.4650] | 1.4001 [1.3926,1.4115] ms | ≈365.7 k / 365.7x |
| 64 / 2016 | 29.967 [29.519,30.361] | 22.130 [22.008,22.335] ms | ≈23.14 k / 23.14x |
| 256 / 32640 | 510.82 [502.96,513.95] | 365.47 [363.86,367.52] ms | ≈1401 / 1.401x |
| 1024 / 523776 | 9730.88 [9690.47,9810.00] | 7.0403 [6.9521,7.1774] s | ≈72.72 / 0.07272x |

The force-pass pair throughput is approximately 54–83 M pairs/s on these probes.
Ordinary serial checks and robust hypot remain enabled; no softening, radius force,
fixed star, or parallel reduction bypasses are timed.

Supplemental groups below are Criterion central slope/mean estimates, not medians,
per complete operation. µs unless explicitly labelled. All N groups ran.

| N | Warm candidate µs | Initialization µs | Reused gather µs | New-time world batch µs | 512 no-history | Private replay 512 | Restore roundtrip µs | Diagnostics µs |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 3 | 0.1082 | 0.2770 | 0.01514 | 0.00946 | 62.128 µs | 64.041 µs | 0.3103 | 0.07637 |
| 16 | 2.676 | 2.3561 | 0.08065 | 0.03426 | 1.3979 ms | 1.3816 ms | 4.914 | 0.7526 |
| 64 | 43.898 | 42.134 | 0.3233 | 0.1235 | 22.629 ms | 22.361 ms | 88.231 | 18.944 |
| 256 | 708.06 | 692.98 | 1.3807 | 0.5768 | 369.30 ms | 356.79 ms | 1480.3 | 331.37 |
| 1024 | 13294 | 13819 | 5.6031 | 2.3943 | 7.1209 s | 7.0648 s | 27014 | 7193.2 |

Reported confidence intervals/outliers were retained. For example the final N1024
history-on central interval was [6.9922,7.1269] s and Criterion reported approximately
4.8% slower than the preceding run with a larger private-ring allocation; the cause
is not established. The force/candidate algorithm was unchanged. Smaller history-on
versus history-off differences, including occasional reversed central ordering,
are not evidence of negative history overhead. High-severe outliers appeared in
several 64/256/replay/restore groups; distributions, not a best iteration, are used.
The final normal-count rerun used 20 samples: N16 history-on had two high-severe
outliers and mean 1.4255 ms versus median 1.4001 ms. Initialization now also
preflights finite constant-spin displacement; these setup groups were rerun for
every N. The added O(N) spin check in private replay initialization is negligible
beside the large-N 512-step force workload; retained large-N replay measurements
are a baseline, not a claim to resolve that tiny setup difference. Code inspection
confirms O(N²) forces/diagnostics and O(N) gather/commit/ring work.

### Actual hierarchy throughput and resource policy

The physical h60 hierarchy (real spins) measured **512 committed steps + full
snapshot ring + stride64 actual trail sampling + one coherent frame publication**:
median **110.346 [109.808,111.810] µs**, 20 samples. Central slope 111.719 µs,
mean 111.078 µs and sample standard deviation 1.661 µs.
This is roughly **4.64 M steps/s** CPU-only, far below the host's 8 ms pump goal.
It is not a promise to present that many steps/s or a GPU frame-rate measurement.

At h60, 1000x requests 16.67 steps/s; 1,000,000x requests 16,666.67 steps/s.
With 60 updates/s, the 512 cap admits execution of at most 30,720 work units/s
(1,843,200x h60 before CPU/render limits). This host's measured physics/publication/
sampling headroom supports those small-system rates; the operator reported the
high-rate outer-orbit and overload sequence passed. The UI measures achieved rate
from actual authoritative advance/wall duration, rather than copying the preset.
No captured 60 Hz presentation/GPU timing result is claimed.

One combined 16 MiB live+private replay ring budget is reserved at setup, with
BodyState layout/tick/metadata accounting and at most 2048 ticks per ring. Normal
3–16-body live retention is 2048; larger probes reduce capacity (e.g. N1024: 78
complete slots per ring on this target). Native `size_of::<BodyState>()` was
independently checked as **104 bytes** using the compiled world library. Baseline
and numeric scratch are separate O(N). Trail payload is independently bounded at
8 MiB/8192 complete samples.
No successful step/snapshot/trail sample allocates after setup, by code inspection;
no allocator profiler/counting allocator was installed. Topology/branch setup,
UI formatting/egui and first-use/growing GPU/staging buffers can allocate.

### World/frame publication

Pure world benchmarks deliberately retain valid coincident centres; they are not
gravity fixtures. Full publication and full 2N-edge projection are separated from
build/append. Central estimates and 95% intervals, per complete operation, µs:

| Bodies | New-time world batch | Full frame republish | Frame build |
| --- | --- | --- | --- |
| 3 | 0.00926 [0.00920,0.00932] | 0.03244 [0.03229,0.03260] | 0.2047 [0.2038,0.2058] |
| 16 | 0.03284 [0.03269,0.03300] | 0.1772 [0.1761,0.1786] | 0.6781 [0.6723,0.6842] |
| 64 | 0.1199 [0.1185,0.1214] | 0.7351 [0.7261,0.7445] | 2.339 [2.321,2.356] |
| 256 | 0.6899 [0.6861,0.6947] | 3.114 [3.096,3.131] | 8.297 [8.210,8.381] |
| 1024 | 2.471 [2.457,2.486] | 12.305 [12.220,12.408] | 44.010 [42.277,46.896] |
| 4096 retained build probe | same-time 10.142 [10.068,10.208] | 57.232 [56.811,57.689] | 503.21 [498.69,509.24] |

Existing lookup/edit and live-append groups also ran: live append central estimates
were 0.537/0.874/5.924/12.548/28.627/250.31 µs at 3/16/64/256/1024/4096.
They include insertion/capacity growth plus full publication, not O(1) appends.
The 64-body build result was about 14% above the older Phase 2 run, while hot
republish remained around 0.7 µs. Allocation/cache/host variation is not isolated
by those short runs. No nested body traversal was introduced; current full hot
publication remains bounded linear without per-body allocation.

### Celestial and trail preparation

1280×800, 60° FOV, reusable storage, no device/GPU. All-visible sphere batches
prepare source-centred f64 vertices/normals, check radius/pixel precision and pack
bytes. Marker-heavy batches use subpixel physical radii, not sphere exaggeration.
The deliberate marker occlusion probe is O(N²) ray/reference-sphere work, small at
normal counts; it is not a gravity or collision acceleration structure.

| Group | Median [95% CI] | Central view/source or marker estimate |
| --- | --- | --- |
| 3 physical spheres | 132.718 [131.158,133.533] µs | view/sources 0.2896 µs |
| 16 physical spheres | 701.315 [696.503,709.478] µs | view/sources 1.4850 µs |
| 64 physical spheres | 2.8144 [2.7886,2.8417] ms | view/sources 5.9003 µs |
| 256 subpixel markers | central 166.22 [165.62,167.00] µs | view/sources 23.636 µs |
| 1024 subpixel markers | central 2.0191 [2.0103,2.0269] ms | view/sources 94.595 µs |
| 3×8192-sample synthetic visible lines | 2.3897 [2.3753,2.4303] ms | central 2.4341 ms; high-severe outlier |
| 16×8192-sample synthetic visible lines | 12.7885 [12.6766,12.8742] ms | central 12.7778 ms |

The line probe is explicitly synthetic nonconstant polyline geometry, not fake
orbital history shown by the validation mode. It isolates f64 clip/narrow/pack from
sampling. Smaller 3/16×1024 groups had central 298.19 µs / 1.5804 ms.
App line-request construction from committed stationary probe samples separately
measured central 7.428/60.172 µs (3×1024/8192) and 40.426/342.53 µs
(16×1024/8192). Commit+sample bookkeeping was around 14 ns at 3 bodies and
45–46 ns at 16; it includes world publication, whose separate cost is above.

The fully visible three-body 8192-sample line case exceeds the 2 ms small-preparation
goal before adding spheres (~2.52 ms combined from separated medians). Inspection
finds linear clipping, robust distance/actual pixel checks and reused byte storage,
not hidden per-segment allocations. This is recorded as a goal miss, not a failed
numerical tolerance or permission to drop actual-history/precision checks. Normal
views clip many segments, but no faster full-workload claim follows. Representation
or preparation optimization requires a separate measured change. Large 16-body
full retention and 1024 probes are extensions, not normal Phase 3 workloads.

Numerical precision/conservation and Windows visual evidence are in
[Phase 3 validation](phase-3-validation.md). Linux timings and current-revision CI
remain unverified. The N1024 result alone authorizes no Barnes-Hut/FMM/GPU/Rayon/job
system or ECS/cache change; normal-workload thresholds remain those in ADR 0004.
