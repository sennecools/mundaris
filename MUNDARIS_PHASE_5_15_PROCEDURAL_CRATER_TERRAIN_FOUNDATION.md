# Phase 5.15: Procedural Crater Terrain Foundation

> **Status:** focused implementation and acceptance contract. Implementation,
> validation, capture results, and user visual acceptance are recorded separately.

## Product intent

The user wants smaller bodies to have larger, recognizable surface landmarks. The
terrain recipe must generate varied moon-like surfaces procedurally and be reusable
across bodies through explicit seed and radius inputs; it must not encode a hand-made
map of our Moon.

The long-term product direction includes infinite varied planetary systems with
multiple moons. That direction informs reusable content and deterministic inputs,
but procedural system assembly is outside this terrain phase. The existing Solar
System preset is a regression and integration fixture only, not a product target
that this phase recreates or expands.

## Focused scope

The user's subsequent image reference rejects the blurred appearance and adds a
visual requirement for dense crisp crater detail, rugged relief and richer mineral
variation relative to the sharp background sky. The user further clarified that
actual planet-scale depth and LOD recovery take priority over surface noise/material
polish. Cosmetic relief cannot substitute for authoritative height, collision
geometry or successful LOD convergence. Numerical and visual acceptance remain separate.

Add a versioned `CrateredV1` world terrain path (generator code `3`) with a compact
seeded crater field, analytic gradients, persistent landmark profiles, and conservative
height/error certificates. Keep generation authoritative in `mundaris_world`; app
content supplies immutable identity, seed, configuration, and body radius. Renderer
and app terrain representation continue using the existing boundaries and resource
accounting.

The reusable app recipe creates 128 seeded crater landmarks per definition. Its
largest crater radius is `min(0.13 × R, 24,000 m)`, each crater's depth is
`0.085 × its radius`, and rim height is `0.020 × its radius`. Centers and sizes are
derived from the stable seed and identity, never authored as diagnostic locations.
The Solar System Moon selects this recipe using its existing terrain identity and
seed. Its gameplay reference radius remains **109,081.7768 m**. Other preset bodies,
orbital data, body counts, and system setup are not part of this change.

## Required behavior and evidence

- Repeated generation from the same version, identity, seed, configuration, radius,
  locations, and footprint produces the same catalogue, values, gradients, and
  certificates. Changing the seed changes the generated catalogue.
- Crater profiles and analytic gradients remain continuous across profile joins and
  canonical terrain seams. Landmarks retain their complete height at every
  footprint; full curvature drives interpolation error and refinement. Background
  noise remains filtered and its omitted contribution stays certified.
- Regional certificates account for crater support: a cap excludes a crater whose
  full support is outside it, while enclosing all intersecting crater contributions
  and derivatives. No LOD, culling, or readiness threshold is relaxed to hide bound
  uncertainty.
- Height, gradient, radius, and radial-envelope tests cover representative and
  boundary inputs. Crater work and retained data are included in existing resource
  accounting. On 2026-10-05 the user explicitly retired the old 128 MiB terrain
  constraint; the demand-driven CPU terrain ceiling is now 512 MiB. Production
  work budgets and renderer buffer caps stay unchanged. Static
  diagnostic captures declare their larger serial work budgets explicitly.
- Focused world/app checks and required full validation pass before implementation
  claims are made. Record exact commands, fixture, seed, radius, revision/dirty
  state, and limitations.
- Provide paired deterministic terrain evidence at orbit, regional, and crater-rim
  scales. Include diagnostic snapshots that identify `settled` versus
  `quality_pending`; an unsettled capture is not evidence of settled terrain.
- Numerical correctness and passing tests do not establish visual acceptance. The
  user reviews the paired captures and decides whether the landmarks and resulting
  appearance meet the product intent.

## Out of scope and open acceptance

This phase does not recreate or extend the authored Solar System, add procedural
planetary-system assembly, change gameplay, rewrite Earth or its environment,
redesign camera/navigation, or redesign GPU residency. It does not invent a
performance target or claim a speedup; report measurements only when collected
against a matched fixture. Existing quality thresholds and readiness semantics
remain in force. The new memory ceiling must be measured and reported separately
from actual retained memory; it is not an eager allocation.

Acceptance A remains **open**. This terrain slice does not close the whole-view
convergence, responsiveness, or visual-quality gate. Keep that verdict separate
from crater algorithm tests and the user's visual review.

## References

- [Phase 5 terrain generation contract](MUNDARIS_PHASE_5_PROCEDURAL_TERRAIN_GENERATION.md)
- [Architecture and ownership](docs/architecture.md)
- [Engine invariants](docs/engine-invariants.md)
- [Terrain validation record](docs/phase-5-validation.md)
- [Implementation handoff format](docs/REVIEW_HANDOFF.md)
