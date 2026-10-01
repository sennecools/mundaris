# ADR 0002: Reference frames and observer-relative precision

- **Status:** Accepted for the Phase 1 implementation; Linux/remote-CI acceptance evidence remains open
- **Date:** 2026-10-01

## Context

The same hierarchy must transport attached human-scale content through astronomical translation and rotation. Flattening object and observer independently into root f64 coordinates loses low bits before subtraction; root f32 coordinates are substantially worse. Velocity is a derivative of a point relative to a moving frame, not a displacement whose basis merely rotates. Frame state and observer queries must describe one coherent instant.

## Decision

Use concrete checked f64 value wrappers, normalized DQuat unit rotations and rigid transforms only, in `mundaris_math`. Keep runtime IDs opaque (caller-allocated nonzero namespace plus append-only u32 index) and separate from persistent/domain identity. Publish index-ordered batches transactionally in O(U), retaining unlisted state by caller agreement. Reparent with explicit new local state, validate before publication and rebuild depths in O(F).

Borrow a tree immutably for evaluation. Relative preparation walks only source/target branches below their LCA, retaining separate branch origins and cancelling common pose/motion. Kinematic preparation composes origin/angular derivatives separately; missing branch derivatives fail explicitly. App observer re-expression computes pose and velocity before replacing state, distinct from intentional attachment.

Renderer depends on math, receives a borrowed evaluation and observer pose, converts the observer into each source frame, subtracts locally before rotation, and validates range/actual f32 round-trip error. App owns the abstract fixture and requests. Renderer owns projection, safe little-endian packing, GPU resources and the disposable view-bound debug frame. A failed frame is poisoned and cannot upload partial data. No root matrix, view translation or authoritative motion crosses the GPU boundary.

## Concrete API choices relative to the sketches

- `FrameEvaluation` is Copy and its query methods take it by value. Copying a single immutable borrow is simpler than tying prepared values to the short-lived wrapper. Prepared values retain that tree borrow and cannot coexist with tree edits; compile-fail examples prove both math and renderer invalidation rules.
- State lookup is `Result<&FrameState, FrameError>` rather than an optional lookup: all current callers require a valid handle, so wrong-tree/missing-frame diagnostics remain explicit. Fields and constructors prevent invalid numeric state, eliminating redundant publication-time scalar validation without weakening the contract.
- DebugFrame is a view-bound builder over reusable staging, taking f64 line requests and preparing their source internally. This prevents mixing owned relative outputs from different views and centralizes indexed failures/full-frame validation. Raw relative-vertex append and world-position projection overloads are absent.
- Reparenting rebuilds depths through a temporary parent-first traversal but retains no traversal cache: evaluation only needs parent/depth. The operation remains transactional O(F); state publication remains allocation-free O(U).
- Benchmark F sizes denote non-root frames so the 64-update case has 64 editable edges plus the immutable root. This resolves the one-node counting ambiguity without weakening root immutability.

The ordered update API remains a current implementation choice: measurements confirm its straightforward bounded hot path. No evidence warrants replacing it now; future producer/API changes may do so while preserving coherent transactional publication and measured costs.

## Alternatives and consequences

Absolute root subtraction was rejected for nearby rendering because it provably loses millimetre detail at `1e16 m`. Conventional global rebasing, high/low GPU pairs, arbitrary precision, transform caches, dirty-subtree systems, owned snapshots, arenas and parallel evaluation are unnecessary for the measured Phase 1 workload. Runtime dynamic frame tags require checked operations, not one phantom type per allocated frame. SI-unit names and distinct wrappers provide the needed semantics without a full units library.

Root queries remain available for diagnostics/coarse work; root round trips have an astronomical tolerance and are not a precise local path. Already quantized independent branches cannot recover detail. The 10 km line budget and forward depth are fixture decisions; distant representation/depth/terrain choices remain open. Borrowed evaluation intentionally requires update/evaluate phases rather than shared mutation.

## Evidence and revisit triggers

[Validation](../phase-1-validation.md) records checked math/topology/kinematic tests, lifetime compile failures, renderer layout/projection/WGSL validation, Windows debug/release agreement, the 72,002-sample replay, approach residuals, and Windows visual/runtime evidence. [Performance](../performance.md) records the complete CPU groups and an approximately 18× prepared-batch advantage over direct point queries. Current code review found no per-path/per-point successful conversion allocation or quadratic publication work.

Revisit addressing/precision when measured independent-branch or cross-system requirements exceed f64's source envelope; deletion/handle generations when real frame lifecycles need removal; interpolation/concurrency when coherent sampled snapshots have an actual caller; caching/LCA metadata only after relevant tree workloads demonstrate a bottleneck; physics derivatives beyond velocity only with physics requirements; distant/depth representations in their own phase. Linux and remote CI evidence must be completed before declaring full Phase 1 acceptance or beginning Phase 2.
