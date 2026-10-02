# ADR 0006: Planet surface topology, LOD and opaque handoff

- **Status:** accepted implementation; complete operator/platform acceptance open
- **Date:** 2026-10-02

## Decision

Implement the [Phase 4 contract](../../MUNDARIS_PHASE_4_PLANET_SURFACE_REPRESENTATION_AND_LOD.md)
in the existing math, renderer and app crates. World and simulation retain their
authoritative bodies, reference radii, state, time, gravity and constant spin.

Math owns the six specified right-handed normalized radial cube charts, checked
addresses through level 30, computed hierarchy and integer cross-face adjacency.
Canonical reduced dyadic Cartesian samples establish exact shared boundaries.
Addresses sort in face/depth-first child order using a computed Morton prefix;
this is render traversal, not a persistence encoding. A `SurfaceLocation` is a
checked body-fixed direction independent of patches, frames, caches and GPU slots.
Static and transported tangent bases use the specified pole fallback.

Renderer owns dimensionless bounds/error/geometric-normal envelopes, five-plane
frustum and chord-aware horizon tests, a complete flat balanced leaf cover, and
metadata readiness. Split >0.125 px and merge <0.0625 px remain the defaults.
Complete four-child and balancing transactions activate atomically. Coarsening
checks outside edge neighbors directly; it does not rescan every leaf for every
sibling candidate. Desired coverage is rebuilt from roots for diagnostics; missing
subdomains are explicitly incomplete estimates. `quality_pending` and `settled`
distinguish met quality from completed structural coarsening.

The fixed grid has 16×16 cells/289 samples and 16 shared boundary-collapse stitch
variants. Fine odd edge samples are unused against a one-level-coarser neighbor.
No skirts, morph buffer, procedural displacement, mesh cache or worker system is
introduced. CPU f64 evaluation reuses Phase 1 prepared source subtraction before
narrowing. Packed storage has 32-byte samples and 64-byte instances; up to sixteen
indexed instanced draws share the celestial reverse-Z pass. Rare uncertain regions
use the counted, clipped 64-byte transient triangle path. Normal shading is radial.

App owns explicit BodyId capability and handoff. Overlap preparation never implies
two opaque draws. Far error starts prewarm at 0.05 px; ordinary transfer waits for
surface error ≤0.125 px; rapid approach can use complete ready coarse coverage.
Return below 0.05 px waits for the far representation's preparation. Observations,
selection and overlays retain one dense body association throughout.

Surface Inspection re-expresses the same observer into the body's fixed role,
preserves incoming orientation, then explicitly adopts zero relative simulation
derivative. Local look/movement, tangent orientation, a reference-sphere navigation
guard and f64 regional anchors replace centre-orbit controls for local inspection.
No regional FrameIds, collision/walking controller or second universe are created.

## Measured corrections and API choices

- A whole-cover replacement proposal could pin two regions and churn a small cache
  after a large viewport/focus change. Highest-error **local** ready closures fixed
  this while preserving the computed flat-cover design. The 2,048-record large-view
  reversal test now settles without steady metadata construction/eviction.
- Sorted contiguous address sets and reused scratch implement the chosen baseline.
  Hypothetical merge validation originally exposed a quadratic rescan; direct
  edge-child queries substantially reduced the measured warm cost.
- Temporary sharing keys are exact compact dyadic values, scoped to one body batch.
  Shared-key/value vectors replace per-boundary map nodes and fit the 8 MiB cap.
  A two-body regression prevents one body's boundary samples being reused by another.
- GPU growth drains prior submitted use before destroying/replacing allocations;
  growth rounding and default device limits cannot bypass the 80 MiB aggregate cap.
- `SurfaceExtent::smooth` declares zero displacement but no independently certified
  opaque occluder radius. The selected geometric-plane horizon proof is used directly;
  the optional inscribed-sphere pretest is omitted.
- `Requested` metadata is represented by pending addresses; `Ready` by validated
  cache records. Active membership is separate. No unused job-state enum is needed.
- Public sketches remain semantic: prepared surfaces append to the existing poisoned
  `CelestialFrame`, which already retains the view/tree/staging lifetimes. The small
  private `cover.rs` module supports contiguous membership/accounting.
- Native reproducibility uses an optional UI route or `MUNDARIS_PHASE4_VALIDATE=1`
  with the same `--gravity-orbits` implementation. No second `--planet-lod` renderer.

## Alternatives and consequences

Spherified/equiangular cubes, triangular hierarchies, latitude/longitude and local
clipmaps were considered in the contract. Normalized cube mapping keeps dyadic
addressing and signed-axis transitions simple, with bounded density distortion.
Skirts, opaque cross-fades and GPU analytic million-metre subtraction were rejected.
Compute, persistent vertices, automatic regional frames, displaced morphing, sparse
edit persistence and volumetric seams remain deferred.

[Validation](../phase-4-validation.md) and [performance](../performance.md#phase-4-baseline--2026-10-02)
record counts, precision, native observations and review misses. The ordinary matched
radial handoff fixture measures 0.03705784 px exact displacement. Near 2 m CPU work
is comfortably below the local review target, but the 402-visible-patch full-planet
preparation remains roughly 11–13 ms and misses the 2 ms target. Review CPU sample
evaluation/packing and conservative near-plane error bounds before Phase 5; do not
silently weaken thresholds, introduce draw distance, or discard physical precision.

Full human control-feel/high-DPI/recovery, Linux native and implementation-revision
remote-CI evidence remain open. Earlier prerequisite acceptance is not retroactively
completed by the new Windows route.
