# ADR 0014: Regional resident terrain scheduling and boundary publication

- Status: Implemented prototype; acceptance requires the Slice 2C evidence
- Date: 2026-10-06
- Contract: [Slice 2C](../PLANET_TERRAIN_SLICE_2C.md)
- Prerequisites: ADR 0012 and ADR 0013

## Decision

Keep the regional selector, immutable derived CPU tile cache, asynchronous build
admission, upload admission, physical GPU slots and drawable cover separate.
Exact Slice 2A content keys identify tiles; epochs identify requests and slot
generations protect physical publication. Desired detail does not imply readiness.
A missing child retains complete resident ancestor coverage. Queue, payload,
upload, publication and transition limits are independently configurable prototype
settings, with structured pressure and quality-lag observations.

The regional fixture starts from one explicitly built cube patch. It does not
construct a six-face planetary cover. Canonical cube adjacency also supports
focused cross-face boundary tests. The original terrain and fixed hierarchy paths
remain available. World definition, generation and navigation authority remain
unchanged.

## Mixed edges and transitions

Use a complete regional 2:1 balanced cover. Retain every regular-grid edge vertex,
but constrain fine boundary points to linear interpolation of the coarser rendered
edge. Canonical dyadic sample keys select the coarsest incident tile, with address
ordering breaking ties. A common owner supplies positions, raw normal varyings and
materials, including corners. There are no skirts or new stitch diagonals.

Prepare immutable boundary endpoints when local topology changes. The GPU blends
them using compact per-frame metadata. This is perimeter dependency data, not a
per-frame CPU transition mesh. Boundary transfers are accounted separately from
tile content and presentation metadata. Parent triangle reconstruction consumes
the outgoing parent's corrected boundary vertices, preserving its actual indexed
surface and raw attributes at fraction zero. Its interior uses the same resident
shader reconstruction as Slice 2B.

The publication unit is a four-child split or merge plus the neighboring patches
whose shared boundary endpoints change. One fraction controls that local closure.
Overlapping closures wait; disjoint closures can transition concurrently. A merge
reverses the interior morph while its boundary fraction moves from outgoing to
target edges. Retain the parent, child and boundary dependencies until completion.
No global hierarchy barrier or synchronous build wait is required.

## Resource lifetime

Use bounded CPU tile and byte caches with dependency pins. The GPU pool records
content identity, slot generation, active draw dependencies and last submitted use.
Queue completion callbacks advance a nonblocking watermark. Reuse requires both
dependency release and completion of prior submitted use. Stale publication is
rejected before mutation; unsafe uploads defer while the previous cover remains.
Logical residency is reconciled with actual renderer slots after eviction.
CPU payload eviction also clears core logical residency. A renderer report can
restore it only when the current CPU payload and local slot match the full content
key; an orphan GPU copy cannot suppress future build or upload admission.
Legacy tile/hierarchy slots and regional slots occupy separate physical ranges;
their independent generation counters cannot overwrite each other's content.
Shrinking a logical pool retains physical generation history. Authority changes
require an available safe slot before replacing the prior drawable state.

Coarsening schedules the missing parents of complete drawable sibling groups,
including parents previously evicted from GPU residency. Under a CPU capacity
that cannot hold every competing split frontier, reserve one complete quartet
plus roots, drawable, transition and upload dependencies. Other partial desired
groups can be cancelled or evicted. If even one complete group cannot fit,
report CPU pressure and retain the valid cover without repeatedly rebuilding
payloads that cannot be retained.

GPU admission independently reserves a complete publishable split quartet when
competing frontiers cannot all fit. The reservation includes the peak footprint:
outgoing drawable tiles, roots, live morph parents and all four incoming children.
Eligible trial covers must satisfy completeness and 2:1 balance. Keep a chosen
quartet until publication or demand changes; suppress other tile uploads while
retaining their CPU payloads. If no quartet fits, expose the minimum required peak
slots and capacity pressure, retaining the cover without cycling incomplete groups
through the pool. Restore at most one missing balanced merge parent in a
merge-only admission phase before competing split uploads. Re-evaluate splits
after coarsening releases capacity; count restoring parents in the peak footprint.
When the pool cannot hold every address in the configured finite region, also
filter uploads while the current frontier fits: admit only base dependencies and
the complete current frontiers. CPU preparation may run ahead, but future
descendants must not evict siblings before their drawable parent is published.
An unrestricted upload path is available only when the configured pool can hold
the entire finite region. This calculation describes the fixture's configured
domain, not a permanent planetary tile limit.
Pin every admitted frontier's children while waiting for its publication, including
when all current frontiers fit. Retained slot LRU ages use a separate monotonic GPU
use clock preserved across reconfiguration. Per-configuration capture frame numbers
may restart without making new siblings older than retained cache content.
An upload packet cannot overwrite another upload's newly selected slot.

Worker cancellation interrupts queued work and injected delay. Finite canonical
builds may finish; their obsolete results cannot replace current requests. Frame
operations use nonblocking admission and result drains. Reconfiguration refuses
busy pools rather than joining workers on the frame thread. Disabled pools drain
cancelled work before replacement.

## Limits and evidence

### Geometry and future field products

The regional selector and geometric topology use canonical shape/height and
geometric representation error. The inherited Slice 2A tile-build/cache path
carries geometry and material attributes together, as described below; this
does not make material settings inputs to authoritative height. Biome,
temperature, humidity, colouring, vegetation and material lookup rules must not
control height, refinement targets or geometric detail. Preserve the existing
physical-scale shape/province/regional/local/fine hierarchy instead of replacing
it with view-dependent noise or material-driven residuals.

`SurfaceLocation`, complete `SurfaceGenerator::evaluate_point` queries and the
separate terrain/normal accessors are the boundary for future derived data.
Complete truth is independent of LOD, cube face, query order and worker thread.
Future slope, curvature, flow/drainage, erosion/deposition, temperature/moisture
and variance products must be able to choose their own resolution, cache key,
revision, filter and LOD policy. Reusing a cube chart does not require matching
geometry tile level or sampling density. Specify units and aggregation semantics
per field; averaging normalized material weights is not a general reduction for
physical fields or their statistical moments. Compare full defining geometry
configuration in caches; `geometry_identity()` is only an identity namespace.

The inherited Slice 2A tile combines radial offsets and four material weights
with one filter/key for render parity. Carrying these attributes through shared
edges and morphs is not an appearance decision or a future environment payload
contract. Do not append climate channels to `TileTexel` or make independent field
cache publication depend on that combined material revision. Introduce separately
versioned optional products when a later phase needs them. A physical process
that changes terrain must enter through an explicit deterministic generation
stage, never through a biome-to-extra-height-noise rule.

This boundary follows the user update after reading the supplied
[Terra Firmer transcript](../../transcript.whisper.reviewed.timestamped.md),
especially geometry/climate separation (14:14), reusable simulation data and
consistent meanings (18:44–22:50), downstream rules (27:42–35:00), variance
(35:00–37:25) and cube-sphere/CPU generation (37:25–39:40). Those sections provide
architectural context; no climate solver, library format or statistical field
implementation is adopted in Slice 2C.

### Validation scope

Projected selection error is an explicitly stated approximation for the regional
prototype, not a certified complete-world error bound. Do not interpret matching
desired and drawn covers as geological or visual acceptance. Capacity pressure
defers quality without changing the requested definition or silently weakening the
stationary target. Requested buffer bytes are not physical VRAM.
Refinement debt is the positive difference between drawable and desired projected
error means, weighted by patch area and normalized to the configured region.
It is expressed in pixels. A zero value can coexist with pending coarsening when
the current cover is more detailed than the requested cover.
Selection memoizes scores within one exact view. An unchanged view can reuse its
cover only after a repeated selection establishes a fixed point, including its
capacity-pressure state; camera position and velocity are not quantized. A bounded
native-frame history separates regional advance, local publication, renderer
preparation, and asynchronous GPU timing. Frame intervals include diagnostic and
presentation overhead that those CPU stage timers do not cover.

Inspect numerical edge/attribute parity, actual GPU reconstruction, stale-slot
transactions, delayed and constrained native routes, frame intervals, cache reuse
and debt convergence. Preserve failed candidates. Stop after Slice 2C; planning
whole-planet streaming requires the user's next decision.
