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

No benchmark scaffolding or optimization infrastructure is included in the bootstrap.
