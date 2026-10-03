# Phase 5.7: adaptive displaced terrain

## Scope and status

The connected terrain preview now uses a balanced adaptive cover, canonical
displaced stitching and a temporary common-refinement morph. This is an
implementation checkpoint, **not interactive-performance or complete Phase 5
acceptance**. See [validation](phase-5-validation.md#phase-57-adaptive-displaced-terrain--2026-10-03)
and [capture index](evidence/phase57/README.md).

The Phase 4 normalized radial mapping, computed addresses, grid16, sixteen stitch
variants, screen-space thresholds, observer-relative precision and exclusive
far/surface ownership are preserved. Terrain truth, filtering, erosion and lighting
remain the Phase 5/5.5/5.6 definitions. No worker, GPU generation, skirts, opaque
cross-fade, shadow, atmosphere or material system is added.

## Selection and readiness

`SurfaceGeometryPolicy` is renderer-domain-free: it provides extent/error
certificates, geometry readiness, replacement admission and a transaction callback.
The smooth path uses the existing policy-free implementation. App-owned
`AdaptiveTerrainCover` has a terrain-aware `SurfaceLodSession`; its report drives
the connected `PlanetSurfaceSession` handoff rather than a second smooth-sphere
selection deciding terrain quality. Picking still uses the smooth selector's
address diagnostics and is not a terrain-intersection oracle.

One complete balanced split/merge transaction is admitted at a time. Old covering
leaves stay active while child, parent and balance geometry is incomplete. Ready
dependencies are pinned immediately at publication, including multiple siblings
completed in one generation opportunity. Requests and partial builders outside
the current blocking closure are discardable. All six roots are retained as
readiness dependencies; incomplete terrain does not suppress the far sphere.

The selector commits its private target before overlay construction. App draw
coverage remains the source until the shared morph completes; selector advancement
is frozen meanwhile. A fixed transition-budget rejection leaves the published
source intact, freezes the unpublished private target and visibly defers it rather
than failing the native frame or attempting a larger old/new level jump. This
deferred target retries only on identity invalidation or an explicit morph-duration
change (including static debug mode). Other construction errors propagate to the
existing preparation/error boundary, not a partial draw.
There is no new transition beneath an active transition.

## Static displaced ownership

`StitchedSurface::build` requires a complete sorted nonoverlapping six-face cover,
correct stitch masks and adjacent edge-level difference at most one. It reconciles
the **complete** ready cover before frustum visibility filtering.

Canonical dyadic physical sample keys identify edge and corner records across face
orientation reversals. The lowest-level incident patch owns each shared sample;
address ordering resolves equal-level ties. Referenced fine even vertices copy the
owner's position and analytic normal. Collapsed odd edge vertices remain unreferenced
by the frozen stitch topology; no second edge curve or skirt is introduced.

Boundary radial displacement and normal corrections blend over the first two
interior rows (weights 2/3 and 1/3, with bounded combination at intersecting rows).
Raw generated cache entries are unchanged. Constrained geometry is disposable.
Raw generated renderer input remains restricted to uniform diagnostic covers;
production mixed terrain must use validated stitched input.

## Certificates and culling

World's filtering-profile difference bound conservatively compares the selected
level with level `max(L-2,0)`, covering the coarsest possible incident owner in a
balanced corner neighborhood. The selector adds that allowance and numeric error;
stitched geometry records the actual maximum constrained-vertex displacement.
Convex interpolation propagates this residual over drawn triangles. This does not
replace the existing interpolation or unresolved-terrain error terms.

The terrain interval is unioned with the actual constrained vertex radii rather
than inflating an already-global interval by an unrelated maximum. Morph remaining
error is `(1-t) * max_vertex |P_old-P_new|`. The common refinement captures both
endpoints, and displayed interpolation remains in their convex hull. Unaffected
patches use their constrained extent. Morph triangles are individually clipped
without a target-only patch-ball rejection.

Terrain horizon occlusion remains disabled: no inscribed opaque sphere certificate
for the actual stitched/morphing closed cover is asserted. Reference-sphere
navigation and the separate smooth-path picking diagnostics are unchanged.

## Common refinement and normals

`SurfaceTransition` overlays each overlapping old/new stitched triangle pair in
local grid16/grid32 coordinates. Reduced checked `i128` fractions drive half-plane
clipping, exact domain orientation and zero-area rejection. Old/new leaf changes
larger than one level are rejected; they must proceed through individual readiness
transactions. Triangles record barycentric references and resolved endpoint data.

The affected group includes removed/added leaves and common leaves whose masks or
constrained samples changed. Unchanged source triangles remain drawn. Changed
source triangles stop at overlay fraction zero; destination triangles replace the
overlay only at completion. All group edges, corners and incident neighbors share
one fraction, including changes driven only by ownership/masks.

Positions interpolate full captured Cartesian triangle positions, not height
alone. Analytic normal **fields** and elevation diagnostics are captured
barycentrically from both endpoints. Normalizing each overlay vertex would change
the original affine normal field and introduce endpoint lighting differences;
the existing fragment shader instead renormalizes after spatial interpolation.
CPU interpolation rejects nonfinite, near-zero and inward intermediate vectors;
construction checks resolved endpoints. These are visual transition normals, not
physical piecewise-linear mesh derivatives.

There is one morph group per displayed terrain body. The live default is **150 ms
linear interpolation**, configurable from 0 to 1000 ms in the existing panel or
`MUNDARIS_TERRAIN_MORPH_MS`; zero is the independent static checkpoint. This differs
from the original design's suggested 250 ms eased duration. Captured duration is
fixed for an active morph. Progress consumes admitted navigation wall time, not
simulation time. Reversal finishes the current bounded morph before reconsidering
the camera; identity invalidation drops obsolete geometry and pins.

## Rendering and resource accounting

Common-refinement triangles use the existing clipped 64-byte transient vertex
layout, reverse-Z pass, terrain lighting and checked 0.05-pixel homogeneous clip
projection. `morph_triangles` is distinct from `fallback_triangles`. There is no
shader terrain query or precision architecture change.

Generation stays synchronous/resumable: eight-sample live microbatches, at most 64
new samples per update and a 2 ms cutoff checked between chunks. The generation
cutoff does **not** bound selector, full-cover stitching or overlay preparation;
measurements show that these remain substantial stalls.

One shared cache is bounded to 4096 entries, 256 pending requests and 128 MiB
aggregate CPU accounting. Preflight includes actual container capacities, pinned
raw patches, current/destination stitched surfaces, selector scratch growth,
stitch-construction scratch and up to 16 MiB transition construction. Transition
address lists are count-preflighted; boundary records and fixed predicate scratch
are charged before allocation, and triangle growth uses explicit bounded capacity.

Renderer staging can retain capacities from other body batches/previous frames.
Live admission therefore reserves its complete existing 64 MiB outgoing plus
8 MiB boundary caps and 32 KiB fixed preparation stack allowance, not a measured
typical payload. Cache accounting also reserves 64 KiB fixed generation/query stack
scratch. This deliberately reduces
available terrain refinement. Transition staging charges retained record capacity
and preflights fallback capacity growth. GPU allocation retains its existing
80 MiB cap. Reported peaks are capacities/reservations, **not process RSS**; the
single preview body is the only enabled terrain body in the connected app.

## Retained limitations

- Full-cover stitching has no incremental derived-geometry reuse.
- Overlay construction retains triangle triplets rather than a deduplicated mesh
  and compares many candidate triangle pairs. It misses interactive preparation
  targets by a large margin; a 150 ms morph duration does not bound startup cost.
- Loose interpolation/unresolved terrain certificates exhaust resources before
  requested pixel quality. `quality_pending` is not hidden or called settled.
- Enabling morphs on a saturated static cover can lack reservation headroom and
  retain that valid old cover. This is not successful refinement convergence.
- The captured checkpoint field can appear low-contrast at the chosen footprint;
  these images are not morphology/seed acceptance or proof of operator usability.
- Existing elevation diagnostics normalize by each patch's extent envelope. Mixed
  filtered profiles can therefore show albedo/profile blocks even when positions
  and normals are watertight; this is not a material/colour-seam acceptance claim.
- No displaced-horizon certificate, terrain-aware walking/collision/navigation,
  broad real-seed transition fuzzing, Linux native or current remote CI acceptance
  is supplied by this checkpoint.
