# Planet terrain redesign — Slice 1 contract

**Authorized 2026-10-05:** start the [redesign](PLANET_TERRAIN_RENDERING_REDESIGN.md)
with its convincing-surface sample. This contract governs the first slice only.

## Objective and ownership

Build a reusable, deterministic moon-like terrain definition in `world::terrain`,
with broad basins/plains, highlands, varied overlapping impacts and correlated
regolith/rock materials. Give the user fixed-resolution orbit, regional and near
reference views across seeds before tuning streaming or replacing the renderer.
Visual acceptance belongs to the user and remains open after automated checks.

The user's subsequent clarification requires substantially different moon
families, including Phobos/Deimos, Europa, Io and Titan-like appearances. This
slice implements the ancient cratered airless family only. Shape, structural
generation, material/color definitions and optional atmosphere must remain
separate design choices; other families cannot be claimed by recoloring this
field. See the [variety requirement](PLANET_TERRAIN_RENDERING_REDESIGN.md#variety-across-moons--user-clarification-2026-10-05).

### Orbital morphology refinement — 2026-10-05

The user supplied two lunar reference views as morphology/readability targets,
not composition or crater-layout templates. Orbit must read as an ancient,
heavily cratered body: dense multi-scale overlapping/degraded impacts, rough
highlands, appropriate broad smoother regions, structured inter-crater terrain
and strong terminator relief, without repeated perfect bowls. The first V1
prototype does not meet this visual target.

Refinement uses the separately versioned `MoonLikeV2`; preserve V1 numerical
semantics and its recorded corpus. V2 adds intervening impact scales, multiple
independent seeded cell layouts per epoch, degraded asymmetric profiles and
correlated highland/plains structure. Complete analytic gradients and a revised
absolute-height envelope remain upstream. New V2 queries are recorded separately
for later unchanged reuse; old records remain historical comparisons.

The temporary reference may use a denser fixed grid and CPU cast-shadow queries
against that represented mesh, with matched unshadowed diagnostics. This is
reference-only illumination, not production shadow architecture or advanced
GPU/LOD. Record grid spacing, ray tolerances, representation residuals and remaining aliasing. Keep
matched camera/light fixtures across seeds and retain raw terrain diagnostics.

Use a distinct `MoonLikeV1` definition and generator API alongside the legacy
`TerrainDefinition`/`TerrainGenerator`. The new complete height/analytic gradient
and material query is authoritative; reference rendering and subsequent tiles
must consume it unchanged. Do not reinterpret V1, V2 or CrateredV1. Inputs include
explicit identity, seed, version, configuration and reference radius, independent
of camera, chart, runtime frame, worker ordering and render slots.

Generate local features from bounded body-fixed spatial cells at several scales.
Query support halos, including at cube-chart boundaries, without a planet-wide
catalogue scan. Apply deterministic older-to-younger replacement/blending, with
bounded displacement rather than unlimited crater-depth addition. Gradients and
materials must follow the same profiles and regional structure.

The first definition uses explicit scale epochs: broad impacts precede finer
impacts, with seeded ages and stable cell-key ordering inside each epoch. Each
epoch blends bounded local displacement profiles, then applies its residual to
the older terrain. Small impacts preserve the broad bowl beneath them. The finite
sum of epoch envelopes bounds the field; this is a versioned simplified impact
history, not a simulation of every possible geological chronology.

The app owns the temporary fixed-resolution reference renderer and diagnostics.
It may use a software reference rasterizer; label it accordingly. It does not
establish native performance, production GPU parity, settled LOD or convergence.
Keep f64 body-local coordinates through sampling and observer subtraction.
Record fixture camera/light settings, coverage, spacing, quantization, and sampled
representation residuals; sampled residuals are not conservative certificates.
Record canonical edge/corner and representative height/gradient/material queries
with identities for reuse in Slice 2.

## Scope and migration decision

The new path intentionally supersedes the frozen grid16, CPU preparation and
common-refinement choices of Phase 4/5 and ADR 0006 when later GPU slices implement
their replacement. Slice 1 introduces only a reference representation and makes
no change to the existing production path. Preserve upstream authority, checked
precision, balanced adjacency, transactional readiness and single opaque-owner
handoff for the future replacement.

GPU residency, adaptive selection, transitions, shadow architecture, terrain
occlusion, whole-body streaming, camera redesign, celestial assembly, oceans,
atmosphere, sparse-edit persistence and collision/walking are outside this slice.
Preserve existing dirty developer-interface and crater experiment work.

## Acceptance and validation

- Repeatable complete samples and materials; scalar/batch and reordered queries
  agree, seeds and identities affect the definition, invalid radii/configuration
  fail explicitly, heights remain inside the declared radial envelope.
- Analytic tangent gradients agree with independent finite differences, including
  impact overlaps and spatial-cell boundaries. Canonical cube-edge/corner samples
  agree. Material weights remain finite, bounded and normalized.
- Reproducible orbit/regional/near lit and height/normal/material diagnostics for
  at least three seeds, paired with the settings and oracle query records above.
- User review of convincing morphology/materials remains required. No automated
  result authorizes Slice 2 or declares the visuals accepted.

Run focused locked world tests and the reference capture, then `scripts/ai-check.ps1`
and inspect its PNG/snapshot for production regression evidence. Run the documented
full locked quality matrix before completion claims. Preserve raw evidence under a
new target directory and report limitations using `REVIEW_HANDOFF.md`.
