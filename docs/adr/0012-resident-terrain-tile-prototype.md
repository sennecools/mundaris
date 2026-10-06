# ADR 0012: Resident terrain tile prototype

- Status: Slice 2A implementation decision; acceptance requires current evidence
- Date: 2026-10-06
- Contract: [Slice 2A](../PLANET_TERRAIN_SLICE_2A.md)
- Continuation: [gated Slice 2B](../PLANET_TERRAIN_SLICE_2B.md)

## Decision

Prototype a single persistent terrain tile alongside the existing CPU-prepared
terrain path. The world retains the complete `SurfaceDefinition` and
`SurfaceGenerator` authority. The app derives disposable filtered radial offsets
and four material weights. The renderer consumes domain-free data and owns a
keyed GPU slot, a reusable regular grid and displacement shaders. The prototype
is opt-in through developer tools; it does not replace whole-body selection.

The baseline format uses ordinary storage buffers. Each grid node has a scalar
radial offset from the complete f64 radius at the patch centre and normalized
material weights, packed into two GPU vec4 records. One node of halo surrounds
the closed grid. Normals come from secants of reconstructed displaced geometry,
including the halo. Sampled bounds describe this payload, not a conservative
certificate for unsampled complete terrain.

The derived filter is separately versioned. A centre sample has weight 1/4;
six samples displaced along positive and negative projected body X/Y/Z axes
each have weight 1/8. Projected axes are not normalized, so the stencil remains
smooth when an axis projects to zero. Its angular width is one quarter of the
nominal grid spacing. This small symmetric filter is a reproducible prototype,
not a final production low-pass filter. Geometry and materials share it.

Exact definition words, radius bits, stable body identity, surface/material
revisions, chart address, cell count and tile/filter format versions identify
content. Diagnostic hashes alone cannot establish equality. This baseline
intentionally couples geometry and material payload invalidation. Camera,
runtime frame identity, slot index, submission, illumination and debug mode are
absent from the content key. Request epochs and slot generations protect
publication separately from deterministic generation.

## Precision

Let the normalized cube vector at the patch centre be `n0`, and let `t` be the
patch coordinate offset divided by the centre cube-vector length. Define
`s = 2 dot(n0,t) + dot(t,t)` and `k = 1/sqrt(1+s)`. The direction difference is
`t*k - n0*s*k/(1+sqrt(1+s))`. This rational form retains small differences
without subtracting nearly equal planetary coordinates in f32. Local position
is `anchor_radius * direction_difference + direction * radial_offset`.

Patch-anchor subtraction and body-frame conversion remain f64 in the existing
prepared-view boundary. Only observer-relative translation, local mapping and
rotation are narrowed for GPU use. One-shot diagnostic GPU readback uses the
same reconstruction function as the vertex shader. Ordinary frames require no
terrain build, completion fence, or mandatory terrain readback.

Before measuring the prototype, the focused reconstruction target is 1 mm local
positional error for an approximately 256 m footprint at reference, Earth and
large-planet radii. The derived-normal parity target is 1 mrad. Complete-world
approximation and authoritative-normal disagreement are measured separately;
neither is hidden inside the CPU/GPU parity number. These local limits do not
certify arbitrary footprint size or distant observer narrowing.

## Gates and limits

Keep cold publication, warm reuse, presentation-only changes and authoritative
invalidation observable in paired captures and structured diagnostics. Track
terrain payload uploads separately from compact camera/instance metadata and
topology initialization. Requested wgpu allocation sizes are not physical VRAM
measurements; upload API time is not GPU elapsed time.

Complete the 2A checkpoint before adding any hierarchy. Continue only if every
specified 2A continuation gate passes. Slice 2B is limited to one parent and four
children and actual parent-triangle reconstruction. Stop after its proof or at
the first unresolved gate; no whole-body streaming or Slice 2C is authorized.
