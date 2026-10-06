# ADR 0011: Hierarchical geological residuals

- Status: Slice 1B.2 implementation decision; visual acceptance is separate
- Date: 2026-10-06
- Contract: [Slice 1B.2](../PLANET_TERRAIN_SLICE_1B_2.md)
- Extends: [ADR 0010](0010-geological-province-directors.md)

## Decision

Add explicit `RockyV5`, `IcyV3` and `VolcanicV3` successors. Each retains the
corresponding province algorithm as its inherited surface, using the successor's
explicit phenotype parameters, then adds three bounded residual bands. Historical
algorithms remain selectable with their original numerical semantics.
Existing material channel formats remain compatible; material parameters cannot
change geometry. Shape and atmosphere ownership remain unchanged.

The added physical cell regimes are 256 m, 32 m and 8 m. Rocky profiles change
from regional impact/rim structure to simple local impacts and fine fractured
breccia. Icy profiles use stress-oriented troughs, unequal shoulders and crossing
fractures. Volcanic profiles use lobate emplacement fronts, pressure ridges and
construction/collapse morphology. Continuous family process fields fill compact
support gaps. They are bounded, context-modulated process contours rather than
an unqualified additive octave stack.

The fine continuous processes contain 2 m sub-features inside the 8 m cell
regime: connected joints and breccia lips, stress troughs with unequal shoulders,
and stepped fronts with a pressure lip and trailing channel. Their bounded
profiles use the existing fine height budget. This refines the process regime
without introducing a centimetre-scale geometry band or enlarging the envelope.

Larger compact process contributions provide a signed, normalized morphology
signal separate from broad elevation. Finer residuals inherit that signal and
the already evaluated coarser residuals. Derivatives include support, blending
and continuous control modulation. Finer bands do not replace the inherited
height; diagnostics expose contributions from the same evaluation. Removing
residuals recovers the inherited field, not a newly seeded coarse world.

Fine cells are generated locally with independent band salts and shared regional
lineage orientation. Two translated layouts, a one-cell Cartesian halo, bounded
shell projection and smooth compact support bound candidate work. No catalogue
of every local feature is constructed. Camera, mesh resolution, worker order,
GPU state and material settings are absent from the generator inputs.

## Evidence boundary

The reference example's explicit `--hierarchy` mode retains historical modes.
Same-anchor fixed crops span 20 km, 2 km, 256 m, 32 m and 8 m, with separate
unbiased views, neutral geometry and process diagnostics. These are independent
fixed meshes, not continuous production LOD or streaming evidence.

The handoff must identify envelope algebra, query work and timing, exact current
and historical corpus replay, source fingerprints, numerical failures/retries
and inspected visual limitations. Nonzero detail and passing tests do not
establish family recognition or user acceptance. This decision introduces no
GPU tiles, LOD replacement, micro-detail renderer or explicit rock populations,
and does not authorize starting Slice 2.
