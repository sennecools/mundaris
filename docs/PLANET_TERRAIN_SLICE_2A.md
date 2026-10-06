# Mundaris Planet Terrain Redesign — Slice 2A
## First Resident GPU Terrain Tile & CPU/GPU Reconstruction Proof

Proceed with **Slice 2A** of the planet terrain/rendering redesign.

This is the first implementation step of Slice 2.

Before changing code, read and preserve the decisions/evidence in:

- `docs/PLANET_TERRAIN_RENDERING_REDESIGN.md`
- `docs/PLANET_TERRAIN_SLICE_1B.md`
- `docs/PLANET_TERRAIN_SLICE_1B_REPORT.md`
- `docs/PLANET_TERRAIN_SLICE_1B_1.md`
- `docs/PLANET_TERRAIN_SLICE_1B_1_REPORT.md`
- `docs/PLANET_TERRAIN_SLICE_1B_2.md`
- `docs/PLANET_TERRAIN_SLICE_1B_2_REPORT.md`
- ADR 0009
- ADR 0010
- ADR 0011
- current `SurfaceDefinition` / `SurfaceGenerator`
- existing renderer terrain path
- current precision/view preparation code
- current developer/capture infrastructure
- current GPU profiling infrastructure

The repository is intentionally dirty.

Preserve unrelated existing work.

Do not commit or push unless explicitly authorized.

Do not reinterpret historical terrain versions.

Do not begin Slice 2B, whole-body LOD, streaming or global terrain replacement.

---

# 1. Goal

Slice 1B.2 established an authoritative deterministic multi-scale surface field and recommended proceeding to Slice 2.

Slice 2A must prove the smallest viable **persistent GPU terrain representation**.

The objective is:

> Take one deterministic authoritative surface region, derive one reusable GPU-resident terrain tile from it, render it through a reusable regular grid with GPU displacement, validate the result against the CPU authority, and prove that unchanged terrain content remains resident without being rebuilt or re-uploaded every frame.

This is a renderer architecture milestone.

It is not a visual-art milestone.

It is not a whole-planet LOD milestone.

It is not a streaming milestone.

---

# 2. Core architecture to prototype

Implement this path:

    SurfaceDefinition / SurfaceGenerator
                  │
                  │ complete authoritative f64 query
                  ↓
           Derived Tile Builder
                  │
                  ├── height/displacement data
                  ├── required material data
                  ├── optional derived normal/gradient data
                  ├── min/max bounds
                  └── identity/version metadata
                  ↓
            CPU Derived Tile
                  │
                  │ changed content only
                  ↓
             GPU Tile Slot
                  │
          persistent residency
                  │
                  ↓
         Reusable Regular Grid
                  │
                  ↓
       GPU Vertex Displacement
                  │
                  ↓
               Draw

The world remains authoritative.

The renderer must not contain a procedural geology implementation.

The shader must not secretly become a second terrain generator.

---

# 3. Scope

Implement **one-region / one-tile GPU displacement** only.

The first accepted prototype should deliberately avoid unrelated complexity.

Required:

- one selected body,
- one selected surface family,
- one known deterministic region,
- one derived terrain tile,
- one GPU-resident tile slot,
- one reusable regular grid,
- one patch instance,
- GPU vertex displacement,
- CPU/GPU reconstruction comparison,
- persistent reuse across frames,
- changed-only terrain upload,
- precision validation,
- observability.

Optional only if necessary for the proof:

- a tiny fixed slot pool larger than one,
- one parent-independent metadata structure,
- explicit debug modes.

Not required yet:

- parent/child refinement,
- morphing,
- global quadtree selection,
- whole-body coverage,
- tile eviction policy under real pressure,
- predictive streaming,
- high-speed descent,
- horizon culling,
- terrain-aware occlusion,
- cross-body switching.

Those belong to later slices.

---

# 4. Authority rule

The tile builder must consume the existing authoritative world-side surface definition.

The data flow must remain:

    CelestialBody
        ↓
    SurfaceDefinition
        ↓
    SurfaceGenerator
        ↓
    authoritative complete query
        ↓
    derived render tile

Never:

    renderer
        ↓
    separate terrain formula

Do not duplicate:

- rocky generation,
- icy generation,
- volcanic generation,
- province logic,
- hierarchical residual logic,
- material history logic

inside shaders or renderer code.

---

# 5. Tile identity

Define a stable derived tile identity suitable for later caching.

The exact Rust representation is not prescribed, but it must distinguish at least:

- stable body identity,
- complete surface definition/configuration identity,
- reference radius,
- terrain/surface revision,
- material revision or compatible material identity,
- tile format version,
- chart / cube face,
- level,
- tile x/y address.

Where relevant, include:

- shape definition identity,
- derived-filter version.

Do NOT include as generation identity:

- runtime `FrameId`,
- camera position,
- worker ID,
- GPU slot number,
- render submission ID,
- current lighting,
- current visual debug mode.

GPU residency identity and world/content identity must remain separate.

---

# 6. Deterministic derived tile builder

Add a derived tile builder that samples the authoritative surface over a fixed tile footprint.

Requirements:

- deterministic,
- independent of worker order,
- independent of rendering frame,
- independent of GPU slot assignment,
- independent of camera,
- stable across repeated builds from identical world identity.

For each tile, derive at minimum:

- surface displacement/height representation,
- min/max displacement,
- enough information for correct GPU surface reconstruction,
- material representation sufficient for the prototype,
- metadata required to interpret the tile.

Do not redefine the authoritative surface through tile sampling.

A tile is a finite render approximation of the complete field.

---

# 7. Tile resolution is a measured prototype parameter

Do not treat the historical Grid16 or any previous fixed density as permanently correct.

For Slice 2A, choose one or a small number of candidate tile resolutions sufficient to establish the architecture.

Document:

- tile texel dimensions,
- border/gutter strategy if any,
- patch-grid vertex dimensions,
- resulting spacing over the test footprint,
- payload bytes.

The selected values are prototype choices, not frozen engine constants.

Avoid premature optimization.

---

# 8. Reusable regular patch grid

Create one reusable regular grid topology.

The grid must not contain unique terrain heights.

Its vertices should encode only reusable local patch coordinates/topology.

Per-patch information should come through compact instance/patch metadata.

Conceptually:

    reusable grid UV/local coordinate
              +
        patch transform
              +
         resident tile
              ↓
        GPU displacement

Do not rebuild a complete terrain vertex buffer per patch.

Do not upload a new copy of the reusable topology for every patch.

---

# 9. GPU displacement

Implement GPU-side displacement from the resident terrain tile.

The vertex shader must reconstruct the surface from:

- local reusable-grid coordinates,
- patch/cube-sphere mapping,
- resident derived tile data,
- observer-relative / patch-relative positioning.

Do not subtract enormous absolute positions in `f32`.

Do not evaluate the authoritative procedural geology directly in the shader.

---

# 10. Precision architecture

Preserve Mundaris's precision model.

World truth remains f64.

GPU rendering must receive values that avoid catastrophic large-coordinate subtraction.

The intended conceptual flow is:

    f64 world surface
          ↓
    f64 patch anchor / body-relative frame
          ↓
    f64 observer-relative subtraction
          ↓
    checked narrowing
          ↓
    GPU local reconstruction

Test at minimum:

- the current small reference body scale,
- a much larger planetary radius,
- substantial observer/frame offsets,
- different cube faces,
- one cube-face edge,
- one cube corner if the first tile path can exercise it without expanding scope excessively.

The local terrain displacement must survive those coordinate scales.

---

# 11. Tile format experiment

Do not assume the final representation prematurely.

Evaluate a minimal sensible candidate.

Possible baseline:

- height/displacement texture,
- material texture or channels,
- reconstructed normals from height/gradient.

If the existing authoritative gradient can be efficiently stored/derived, evaluate whether:

    height + gradient + material

is preferable to:

    height + explicit normal + material

or:

    height + reconstructed normal + material.

For Slice 2A, choose one baseline and document why.

Do not turn this into a large format research project.

Measure:

- payload bytes,
- GPU allocations,
- reconstruction error,
- shader sampling cost where measurable,
- obvious filtering artifacts.

---

# 12. Persistent GPU residency

The terrain tile must remain resident across unchanged frames.

Do not implement a path that:

    frame N:
        build tile
        upload tile

    frame N+1:
        build same tile
        upload same tile

    frame N+2:
        build same tile
        upload same tile

After warm-up, stable content should remain resident.

The intended lifecycle:

    ABSENT
       ↓
    BUILT_CPU
       ↓
    UPLOADED
       ↓
    RESIDENT
       ↓
    REUSED FOR MANY FRAMES

An unchanged view/definition must not cause terrain-content upload again.

---

# 13. Terrain-content dirty tracking

Explicitly distinguish:

- terrain tile content,
- instance/camera state,
- lighting uniforms,
- debug presentation state.

The acceptance statement:

> zero unchanged terrain-content uploads after warm-up

does NOT mean every GPU write in the renderer becomes zero.

Camera and small instance/uniform updates may still occur.

Instrument terrain-content bytes separately.

---

# 14. GPU tile slot abstraction

Even though one tile is enough for visual proof, implement the resident representation using a reusable tile-slot concept suitable for later extension.

A slot should have enough metadata to safely track:

- current content key,
- slot generation,
- residency state,
- GPU resource location/layer,
- last upload/publication generation,
- submission/in-flight state where relevant.

Do not yet build a sophisticated LRU.

Do not yet implement production eviction heuristics.

The abstraction should simply avoid baking world identity into physical GPU slot index.

---

# 15. No synchronous terrain wait in the frame

Even in Slice 2A, establish this architectural invariant:

> Rendering must not require waiting for terrain refinement/build work.

For the initial static prototype, it is acceptable to explicitly prepare the tile before beginning the visual comparison fixture.

However, once the renderer has a valid resident tile, ordinary frames must not rebuild or block on that tile.

If asynchronous tile construction is introduced during 2A, parent/fallback refinement behavior is still out of scope.

Do not add unnecessary threading complexity solely to satisfy a future requirement.

But do not introduce a design that fundamentally requires synchronous per-frame terrain building.

---

# 16. CPU reference stages

Keep three concepts distinct:

## A. Authoritative complete surface

The current `SurfaceGenerator` world truth.

## B. Derived CPU tile reconstruction

What the selected tile format/grid actually represents.

## C. GPU reconstruction

What the shader produces from that tile.

Measure errors separately:

    world → derived tile

and

    derived tile → GPU

Do not report only one combined number.

This distinction is critical.

If geometry differs, we must know whether the fault is:

- terrain sampling/filtering,
- tile quantization,
- coordinate reconstruction,
- shader math.

---

# 17. CPU/GPU comparison

Create a deterministic comparison fixture.

At selected known grid/sample points, compare:

- authoritative complete position,
- derived CPU tile reconstruction,
- GPU-reconstructed position if practical to read/validate directly.

If direct GPU vertex readback would distort the architecture or require excessive machinery, implement a focused validation/debug path rather than per-frame production readback.

No mandatory GPU readback may exist in the shipping path.

Record errors in physical units:

- radial/height error,
- local positional error,
- normal/angular error if applicable.

Define acceptance tolerances from Mundaris precision needs rather than simply observing that values "look close."

---

# 18. Visual comparison mode

Create a developer/debug fixture that can show:

- old/reference CPU representation,
- new GPU tile representation,
- side-by-side or toggle comparison,
- height diagnostic,
- normals diagnostic,
- material diagnostic,
- tile UV/border debug,
- patch/grid overlay.

Prefer using the existing developer/capture infrastructure.

Do not build an unrelated second control system.

---

# 19. Cross-family sanity check

The main implementation may use one family/region.

Before declaring Slice 2A complete, run the same tile-builder/GPU path over representative regions from:

- RockyV5,
- IcyV3,
- VolcanicV3.

Do not write family-specific renderer branches.

Renderer/tile code must not contain logic like:

    if rocky { ... }
    if icy { ... }
    if volcanic { ... }

except debug labels/tests.

The tile builder consumes generic authoritative surface/material queries.

This proves the rendering path is family-agnostic.

---

# 20. Use one canonical acceptance region

Choose a deterministic test region with:

- visible relief,
- nontrivial gradient,
- meaningful material variation,
- no pathological vertical wall requiring unsupported topology.

Prefer a known Slice 1B.2 anchor from the existing fixed evidence.

Record:

- body family,
- seed,
- radius,
- exact anchor,
- cube/chart address,
- tile footprint,
- complete definition identity.

This becomes the stable Slice 2A reference fixture.

---

# 21. Borders and gutters

Even though multi-tile adjacency belongs mainly to Slice 2B, the first tile format must not make future border continuity impossible.

If texture filtering samples outside the tile interior, define a border/gutter policy.

Possible approaches include:

- duplicated canonical border texels,
- explicit gutters,
- clamped interior sampling,
- analytically matched boundary samples.

Do not introduce a tile format that will obviously create seams when adjacent tiles arrive.

Add focused boundary tests.

---

# 22. Sampling/filtering semantics

The authoritative complete field may contain finer detail than the tile can represent.

Do not simply point-sample at texel centres and imply that the result is a correct coarse representation if aliasing is obvious.

For Slice 2A, establish a clearly defined derived sampling/filtering method.

This does not need to be the final production low-pass filter.

It must however be:

- deterministic,
- documented,
- reproducible,
- separable from authoritative terrain,
- versioned as part of derived tile format semantics.

Record approximation error.

Do not silently change the complete world definition.

---

# 23. Material representation

Carry enough material data through the derived tile path to prove generic surface rendering.

Do not attempt final material rendering.

At minimum:

- preserve authoritative normalized material information,
- define the derived material sampling/filtering rule,
- make tile identity include material-format/version compatibility,
- show a diagnostic rendering.

Do not allow material-only changes to invalidate geometry unnecessarily unless the cache format intentionally couples them.

Record that choice.

---

# 24. Normals

Normals are important enough to test now because GPU displacement is useless if lighting disagrees with geometry.

Choose one prototype approach:

- derive normals from resident height/displacement,
- store gradient,
- store normal.

Whichever is selected:

- compare against authoritative/reference normals,
- test precision,
- test chart/border behavior,
- ensure normal reconstruction matches displaced geometry.

Do not let the old CPU-prepared normal path hide a new GPU geometry mismatch.

---

# 25. GPU resource strategy

Use ordinary portable wgpu features first.

Prefer:

- texture arrays,
- ordinary textures,
- ordinary buffers,
- indexed instancing.

Do NOT make Slice 2A depend on:

- mesh shaders,
- bindless descriptor indexing,
- native sparse residency,
- experimental unsafe wgpu features,
- raw Vulkan-only paths,
- GPU compute terrain generation.

Those remain future optimization options requiring measurements.

---

# 26. Allocation policy

Avoid repeated allocation during stable rendering.

Prefer creating/reusing:

- grid vertex/index buffers,
- tile texture/array resources,
- staging resources,
- instance buffers.

For the prototype, capacities may be configurable constants.

They must be explicitly labelled prototype capacities.

Do not introduce another unexplained permanent engine budget.

Do not recreate the old 128 MiB mistake with a new number.

---

# 27. Resource accounting

Expose at minimum:

## CPU

- derived tile payload bytes,
- builder scratch where known,
- retained tile bytes.

## GPU

- tile payload bytes,
- actual allocated resource bytes where observable,
- grid/topology bytes,
- instance bytes,
- staging/upload bytes,
- in-flight bytes if tracked.

Keep these categories separate.

Do not conflate:

- CPU tile cache,
- GPU residency,
- staging,
- world authority,
- process RSS.

---

# 28. Upload metrics

Record per frame or per relevant submission:

- terrain-content upload bytes,
- upload count,
- tile key uploaded,
- slot uploaded,
- reason for upload.

Acceptance fixture:

1. start cold,
2. build/upload tile,
3. render repeated identical frames,
4. verify terrain-content upload bytes become zero,
5. alter camera only,
6. verify terrain-content remains resident,
7. alter lighting/debug mode only,
8. verify terrain-content remains resident,
9. change authoritative surface identity/revision,
10. verify the tile is correctly considered stale and rebuilt/reuploaded.

---

# 29. Invalidation

Prototype correct invalidation now.

Changing relevant authoritative content must not reuse stale GPU terrain.

Test at least:

- body/surface definition change,
- radius change,
- terrain/surface revision,
- material revision where applicable,
- derived tile format revision.

Changing these must not unnecessarily invalidate geometry:

- camera,
- illumination,
- debug mode,
- runtime `FrameId`.

Record exact behavior.

---

# 30. GPU slot generation safety

Implement a slot-generation or equivalent stale-publication guard.

Scenario:

    slot 3 contains tile A
        ↓
    tile A invalidated
        ↓
    slot 3 reused for tile B
        ↓
    delayed completion for A arrives

The delayed A result must not overwrite or publish as B.

A full streaming system is out of scope, but the ownership model must already make this class of bug impossible or detectable.

---

# 31. Precision tests

Add focused tests/fixtures for:

- small body radius,
- Earth-scale or larger radius,
- large world/frame translation,
- different body rotation where existing architecture permits,
- cube face centre,
- cube edge,
- cube corner if applicable.

Compare reconstructed local geometry in physical units.

Do not merely compare clip-space output.

---

# 32. Developer diagnostics

Add focused debug visualization for Slice 2A.

Useful modes include:

- derived tile height,
- tile layer/slot ID,
- tile UV,
- grid wireframe,
- CPU-vs-GPU residual visualization,
- normal visualization,
- material weights,
- resident/nonresident state.

Do not turn this into a broad developer-UI redesign.

Extend existing typed developer controls if required.

---

# 33. Tests

Preserve all existing tests.

Add focused tests for:

## Tile identity

- deterministic same key,
- relevant definition changes alter key,
- camera/frame state does not alter key.

## Tile building

- deterministic output,
- finite values,
- known bounds,
- repeated scalar/batch authority agreement.

## Filtering

- deterministic derived samples,
- edge/corner consistency,
- no uninitialized gutters.

## Resource lifecycle

- one cold upload,
- repeated warm frames do not reupload terrain content,
- invalidation causes exactly the expected rebuild/upload.

## Precision

- reconstruction error within defined tolerance.

## Family independence

- rocky/icy/volcanic all pass through identical generic tile code.

## Historical preservation

- existing world terrain corpora and previous Slice 1 replay evidence remain unchanged where expected.

---

# 34. Performance measurements

This is not yet a whole-renderer performance phase.

Still measure:

- one tile CPU build time,
- tile payload size,
- one tile upload bytes/time where available,
- stable repeated-frame terrain upload bytes,
- new-path CPU render preparation cost,
- GPU terrain draw time where timestamp support allows,
- memory allocated for the prototype.

Use multiple repeated measurements where appropriate.

Do not claim production FPS or whole-planet speedup.

---

# 35. No old-path removal

Keep the existing terrain renderer intact.

Slice 2A is a comparison path.

Do not remove:

- CPU-prepared terrain,
- old stitching,
- old transition machinery,
- existing production terrain path.

The old path remains useful for:

- regression comparison,
- visual checks,
- debugging.

Removal happens only after the replacement passes later slices.

---

# 36. No whole-body selector

Do not connect the new path to the complete existing adaptive cover yet.

The prototype may use a fixed patch address/fixture.

Do not spend Slice 2A modifying:

- global leaf limits,
- whole-view selection,
- balanced whole-body cover,
- global transition scheduling,
- horizon culling.

Those are later concerns.

---

# 37. No parent/child morphing yet

Slice 2A proves one resident displaced tile.

Do not implement full parent/child transitions unless a minimal piece is strictly necessary to validate the chosen tile reconstruction.

The next phase will explicitly handle:

- parent,
- four children,
- border continuity,
- parent reconstruction,
- morphing,
- independent child readiness.

Keep Slice 2A narrow.

---

# 38. No asynchronous streaming system yet

Do not build the final request scheduler.

However, do not design APIs that require tile generation every frame or require the render thread to synchronously generate the tile.

A simple explicit prebuild/warm-up fixture is acceptable for 2A.

The architecture must be compatible with later asynchronous generation.

---

# 39. Out of scope

Do not implement:

- whole-planet tile cover,
- adaptive GPU tile selection,
- parent/child LOD transitions,
- high-speed streaming,
- predictive prefetch,
- terrain refinement debt,
- horizon occlusion,
- whole-body cache eviction,
- multi-body residency,
- GPU compute tile generation,
- mesh shaders,
- sparse textures,
- rocks/boulders,
- micro-displacement,
- parallax,
- volumetric terrain,
- survival gameplay,
- atmosphere redesign,
- terrain shadows beyond what is strictly required for the debug fixture.

---

# 40. Acceptance gates

## Gate A — World authority preserved

The new renderer consumes the current authoritative surface definition.

No duplicate procedural terrain implementation exists.

## Gate B — Deterministic derived tile

A fixed world/tile identity produces bitwise or explicitly equivalent deterministic tile content according to the selected representation.

## Gate C — GPU reconstruction

GPU displaced positions reproduce the derived CPU tile representation within an agreed measured physical error.

## Gate D — World-to-tile approximation measured

Error between complete authoritative terrain and derived tile is measured separately.

## Gate E — Precision

Large radii/frame offsets do not destroy local terrain detail or violate existing precision expectations.

## Gate F — Persistent residency

After initial upload, repeated unchanged frames reuse the resident tile.

## Gate G — Zero unchanged terrain-content uploads

Stable terrain content causes zero repeated terrain-content upload bytes after warm-up.

## Gate H — Correct invalidation

Relevant world/tile-format changes rebuild content; camera/lighting/debug-only changes do not.

## Gate I — Generic family path

RockyV5, IcyV3 and VolcanicV3 all render through the exact same tile/displacement architecture.

## Gate J — Observable resources

CPU tile memory, GPU tile payload/allocation and upload bytes are visible separately.

## Gate K — Old renderer preserved

The existing production terrain path remains available for comparison.

---

# 41. Required evidence package

Produce a dedicated Slice 2A evidence root.

Include:

## Source/build identity

- initial/final git state,
- relevant input hashes,
- executable hashes where existing workflow expects them,
- preservation audit.

## Tile metadata

- exact tile key,
- source surface identity,
- radius,
- address,
- tile format version,
- dimensions,
- payload bytes,
- bounds.

## CPU comparison

- authoritative complete samples,
- derived CPU tile samples,
- world→tile errors.

## GPU comparison

- GPU reconstruction validation,
- tile→GPU errors,
- normal/material validation where applicable.

## Residency

A recorded cold/warm sequence proving:

- initial upload,
- repeated warm reuse,
- zero repeated terrain-content uploads.

## Visuals

At minimum:

- new GPU terrain lit view,
- height view,
- normal view,
- material view,
- grid/tile debug view,
- old-vs-new comparison.

## Performance observations

- tile build time,
- upload bytes,
- GPU draw timing if available,
- CPU preparation timing,
- memory accounting.

---

# 42. Handoff report

Create a Slice 2A handoff/report that clearly separates:

- IMPLEMENTED
- VERIFIED
- MEASURED
- OBSERVED
- PARTIAL
- FAILED / OPEN

Explicitly state:

- selected tile format,
- selected grid density,
- world→tile error,
- tile→GPU error,
- persistent-residency behavior,
- upload behavior,
- resource usage,
- precision results,
- known limitations.

Do not describe prototype choices as final renderer architecture unless actually frozen by evidence.

---

# 43. Slice 2A success condition

Recommend **READY FOR SLICE 2B** only if:

1. the authoritative world feeds the tile builder cleanly,
2. the derived tile is deterministic,
3. GPU displacement reconstructs it correctly,
4. local precision is acceptable,
5. stable content remains resident,
6. unchanged terrain is not re-uploaded,
7. invalidation is correct,
8. all three current geological families can use the generic path,
9. no architectural defect is found that would make parent/child refinement unsafe.

If the main remaining problems are:

- tile density tuning,
- tile format tuning,
- artistic terrain quality,
- optional normal/material packing improvements,

those do not necessarily block 2B.

---

# 44. Slice 2A stop condition

When the first resident GPU tile prototype, validation and evidence package are complete:

**STOP.**

Do not proceed automatically to Slice 2B.

Do not add parent/child refinement "while already in the code."

Present the results for review.

Slice 2B will separately address:

- parent + four children,
- canonical same-level borders,
- child/parent coordinate agreement,
- independent readiness,
- coherent parent fallback,
- actual parent-triangle reconstruction,
- GPU morphing,
- transition watertightness.