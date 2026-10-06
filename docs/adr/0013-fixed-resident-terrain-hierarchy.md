# ADR 0013: Fixed resident parent/child transition proof

- Status: Accepted for the fixed Slice 2B prototype; [dated evidence](../PLANET_TERRAIN_SLICE_2B_REPORT.md)
- Date: 2026-10-06
- Contract: [gated Slice 2B](../PLANET_TERRAIN_SLICE_2B.md)
- Prerequisite: [accepted Slice 2A checkpoint](../PLANET_TERRAIN_SLICE_2A_REPORT.md)

## Decision

Extend the proven resident renderer to a fixed five-slot pool: one pinned parent
and its four canonical children, with one shared regular UV/index grid. Retain
exact world-content identity separately from physical slots and request epochs.
Each child may finish building and upload independently; the parent remains fully
drawable until all four children are drawable. Child draw ownership then publishes
atomically. This prototype avoids a mixed-level outer-edge topology while proving
asynchronous generation/upload and uninterrupted parent coverage. It is not a
whole-body selector or streaming cache.

Evaluate the actual parent's `[a,b,c,b,d,c]` triangles at child grid coordinates.
Morph positions from those barycentric points to the child's own derived samples
on the GPU. Express both endpoints in the same parent-local anchor: subtract child
and parent body-fixed anchors in f64, then narrow the local difference. Use the
parent's prepared observer-relative transform for the final position. No large
absolute anchors are subtracted in f32.

Preserve the parent's raw interpolated vertex-normal varying and material weights
at the coarse endpoint. Blend those attributes toward the child's derived vertex
attributes, and normalize normals after raster interpolation. Normalizing a coarse
normal at each newly inserted vertex would change the smooth-shading field during
subdivision; the coarse endpoint must preserve the actual parent attributes too.

Retain parent content while any child needs fallback, reconstruction, or transition.
Change only compact presentation/morph metadata during stable transitions. No
unchanged tile upload, CPU transition mesh, skirt, or ordinary-frame geometry
readback is required.

## Readiness, cancellation, and reversal

Use a bounded four-worker prototype pool over immutable published definitions.
Ordinary frames drain completed work without blocking. Request epochs reject late
results, while per-slot content generations protect renderer publication. Missing
or delayed children preserve parent coverage. Delays are explicitly injected on
workers, and frame delivery is measured separately from builder time.

Reverse a transition by moving its existing common fraction toward the new target;
retain the same resident endpoints. Zero, ordinary, and slow durations are debug
parameters, not final engine policy. Retain ready children across repeated demand
so transition repetition cannot cause a hidden rebuild/upload cycle.

## Evidence and scope

Check parent and child endpoints, shared edges/corners, intermediate fractions,
normal/material attributes, large radii and view offsets, missing children,
artificial delays, stale results, reversal, upload bytes, and resource lifetime.
Distinguish diagnostic GPU waits from ordinary frames. Preserve paired captures,
source/executable hashes, request events, and frame measurements. Stop after the
fixed hierarchy proof or an unresolved gate. No Slice 2C is authorized.
