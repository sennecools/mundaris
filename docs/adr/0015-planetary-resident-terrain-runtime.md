# ADR 0015: Ordinary planetary resident terrain runtime

- Status: Implemented; native acceptance pending the [Slice 2D report](../PLANET_TERRAIN_SLICE_2D_REPORT.md)
- Date: 2026-10-06
- Contract: [Planet terrain Slice 2D](../PLANET_TERRAIN_SLICE_2D.md)
- Prerequisites: ADR 0012, ADR 0013, and ADR 0014

## Decision

Use the existing regional resident scheduler for ordinary `--solar-system` and
`--real-solar-system` terrain presentation. The app owns `PlanetaryTerrain`, binds
it to the selected body's immutable world surface definition, and passes derived
resident draws through the existing celestial renderer. `--legacy-terrain` is an
explicit comparison switch for the earlier adaptive terrain path. World remains
the authority for terrain definitions, reference radius, and terrain revision.

Rebind and draw only when exact tile identity and the bound source revision match.
Definition, radius, or terrain-revision changes invalidate readiness while the
bounded worker pool drains; late results from the old binding cannot become the
current draw. Presentation and residency remain rebuildable derived state.

## Runtime policy

Use six canonical cube-face roots and a bounded four-worker generation pool, with
one separate publication coordinator with at most two background calculation
tasks. Current planetary limits are 16,384 desired
patches, 49,152 CPU tiles, 32,768 logical GPU slots, four tile uploads and 8 MiB per frame,
and eight publication adoptions per frame with sixteen active morph groups.
These values bound the configured runtime;
they do not assert measured performance, physical VRAM use, or acceptance.

Keep cache mutation on the coordinator and parallelize immutable boundary and
endpoint calculations across at most two tasks. Limit work to one in-flight batch,
one completion and eight prepared local products. Retain blocked products and skip
them when independent products can proceed. A batch returns its pre-candidate
logical outgoing boundary map; unadopted topology changes remain in prepared
targets. Captured outgoing overlays are consolidated off the frame thread while
later disjoint overlays remain live. Validate exact source keys, current local
drawable sources, demand, 2:1 balance and affected boundary expectations before
each commit. Global topology revision changes from independent adoptions do not
invalidate a locally unchanged product. Authority changes still invalidate work.
The selector
checks a 2 ms work budget, and publication admission defers against a cumulative
2 ms budget with 1.5 ms dispatch/adoption headroom; both expose overruns. Native
slot selection examines at most 256 entries per upload and defers when none can
be safely reused. Treat budgets as observable gates whose acceptance requires
native measurements.

Retain a current-cover canonical boundary cache on the publication worker.
Immutable tile identity controls payload reuse; changes invalidate affected sample
owners and their boundary dependents, including coarse interpolation endpoints.
Retain full cover validation and the existing boundary arithmetic. Any preparation
failure clears the derived cache; stale target work can be reversed to the
acknowledged cover. Report cache storage separately from aliased tile payloads.
The renderer may reuse validation of exact immutable endpoint objects, while
view transforms, morph state, authority, slots and transaction membership remain
validated for every draw.

Reuse stationary scheduling only after the selector reaches an unchanged fixed
point with complete resident coverage and no pending work. Continue completion
draining and LRU touches; view, residency or dependency changes restore normal
admission. Planetary frame diagnostics export scalar coverage/job/resource data
and bounded history with explicit omission labels, while finite fixtures retain
their detailed arrays. Include snapshot retirement in diagnostic elapsed time.

Keep the one-millimetre observer-relative anchor-narrowing gate through 10 km.
Beyond that envelope, permit the maximum of distance / 2^23, chart footprint /
262,144, and one millimetre. This bounds f64-to-f32 anchor presentation only;
finite fixtures keep their original precision policy, and local tile reconstruction
is unchanged. At 100 km, a representative 100000.013 m component has 2.625 mm of
f32 narrowing error, exceeding the former one-millimetre budget. Frustum and
horizon tests affect visible draws while retaining the complete balanced resident
topology. Selection uses projected relief and sagitta as an uncertified quality
proxy. Report capacity pressure or incomplete convergence through
`quality_pending`; proxy convergence is not terrain-quality certification.

## Preserved boundaries and acceptance

Keep the finite Slice 2A resident-tile, Slice 2B fixed-hierarchy, and Slice 2C
regional fixtures and their numerical/representation contracts. The ordinary
runtime integration does not silently change those fixtures, world generation
authority, or visual acceptance criteria. Slice 3A is not authorized by this ADR.

The source implementation is present, but native acceptance remains pending the
dated Slice 2D report and its reviewer assessment. Passing source-level or
offscreen checks alone does not establish native visual quality, human UX, or
performance.
