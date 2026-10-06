# ADR 0010: Versioned geological province directors

- Status: Slice 1B.1 implementation decision; visual acceptance is separate
- Date: 2026-10-06
- Contract: [Slice 1B.1](../PLANET_TERRAIN_SLICE_1B_1.md)
- Extends: [ADR 0009](0009-compositional-body-surfaces.md)

## Decision

Keep the compositional surface authority and add explicit `RockyV4`, `IcyV2`
and `VolcanicV2` algorithms. `RockyV3`, `IcyV1`, `VolcanicV1`, `MoonLikeV1`
and `MoonLikeV2` retain their numerical definitions. New material channel
versions are selected explicitly with the new geology algorithms.

An immutable body phenotype biases low-frequency, body-fixed control fields.
Smooth normalized province weights direct family processes, including impact
retention, structural relief, fractures, construction and resurfacing. These
are procedural geological history controls, not ecological biomes or a calibrated
physical history simulation. The world owns both controls and complete morphology.

Separate salts identify phenotype, director, province structure, family features,
shape and material variation. Camera, worker order, footprint, GPU state and
atmosphere are absent from terrain generation. Material edits affect material
identity and weights without moving geometry. Complete-definition equality and
reference-radius bits remain necessary for future cache reuse.

Features use immutable regional cells, bounded supports and support halos.
Compact windows must vanish smoothly at support edges; overlap composition must
remain bounded even where several features coexist. Height derivatives include
the derivatives of morphology, controls, windows and compositing. Conservative
relief envelopes require algebraic bounds; sampled extrema are diagnostic only.

## Evidence boundary

The reference driver can select the new versions with `--provinces`. It retains
unbiased fixed crops and records its deterministic province/landmark selection
method. Multiple local scales are fixed reference crops, not continuous LOD or
streaming evidence. Uniform grey geometry uses matched lighting independently
of the family material palette.

Director maps expose individual controls, province weights and process strengths.
Their projection is labelled separately from the perspective terrain capture.
Corpus records extend complete shape/height/normal/material queries with controls
and province centres, transitions and boundaries. Historical replay remains a
separate preservation gate.

This decision does not start Slice 2 or integrate a new production renderer.
The existing production clearance adapter consumes any selected valid surface;
the native adaptive renderer retains its earlier terrain path. Passing numerical
tests does not establish geometry-only recognition or user visual acceptance.
