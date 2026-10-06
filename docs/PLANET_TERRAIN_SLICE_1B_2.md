# Mundaris Planet Terrain Redesign — Slice 1B.2
## Hierarchical Multi-Scale Geological Detail & Feature-Preserving Surface Fields

Proceed with **Slice 1B.2**.

This is the final planned procedural-terrain implementation phase before Slice 2.

Read and preserve the decisions and evidence in:

- `docs/PLANET_TERRAIN_RENDERING_REDESIGN.md`
- `docs/PLANET_TERRAIN_SLICE_1B.md`
- `docs/PLANET_TERRAIN_SLICE_1B_REPORT.md`
- `docs/PLANET_TERRAIN_SLICE_1B_1.md`
- `docs/PLANET_TERRAIN_SLICE_1B_1_REPORT.md`
- ADR 0009
- ADR 0010
- current `SurfaceDefinition`
- current RockyV4 / IcyV2 / VolcanicV2 implementations
- current director/province implementation
- existing deterministic query corpora and fixed reference workflow

Do not begin Slice 2.

Do not implement GPU terrain tiles, replacement LOD, streaming or renderer architecture in this phase.

Do not commit or push unless explicitly authorized.

The working tree is intentionally dirty. Preserve unrelated existing work.

---

# 1. Goal

Slice 1B.1 successfully established:

- deterministic body phenotypes,
- geological director fields,
- multiple provinces per body,
- different rocky / icy / volcanic process grammars,
- world-owned authoritative surface queries,
- deterministic materials,
- irregular star-shaped bodies,
- production body authority and radial clearance,
- reproducible multi-scale reference captures.

However, **Gate E remains FAILED**.

The reference geometry already resolves the queried near field accurately, yet several 32 m and unbiased near views remain visually smooth.

Therefore this is no longer primarily a reference-mesh-resolution problem.

The authoritative terrain field itself lacks enough meaningful structure at local geometric scales.

Slice 1B.2 must solve that problem.

The objective is:

> Add deterministic, geology-specific hierarchical surface detail so that the same authoritative world contains meaningful structure from planetary scale down to approximately metre / tens-of-metres scale without degenerating into generic multi-octave noise.

This phase must demonstrate that:

1. large geological features survive across scale,
2. approaching them reveals additional correlated structure,
3. rocky, icy and volcanic terrain remain recognizably different at local scale,
4. local terrain structure derives from geological context,
5. the complete authoritative terrain field is suitable for future feature-preserving LOD.

This is the last planned terrain-authoring gate before Slice 2.

---

# 2. Current diagnosed failure

The current fixed-reference system has already demonstrated very small reconstruction residuals in the smallest targeted crops.

Do not attempt to solve this phase simply by:

- increasing reference mesh resolution,
- increasing raster resolution,
- dramatically changing lighting,
- increasing material contrast,
- adding stronger normal-map-like presentation,
- inserting arbitrary high-frequency noise.

The missing information must exist in the authoritative world terrain itself.

---

# 3. Core architecture

Preserve the current high-level flow:

    Body definition
          ↓
    Body phenotype
          ↓
    Geological director
          ↓
    Province weights
          ↓
    Regional geological processes
          ↓
    NEW: hierarchical geological detail
          ↓
    Complete authoritative surface

Conceptually, terrain should become:

    H_complete =
        H_shape
      + H_macro
      + H_regional
      + H_local
      + H_fine_geometric

These terms are conceptual.

Do not force this exact API if a cleaner representation fits the current code.

The important requirement is that different scale ranges have identifiable geological purpose rather than being an undifferentiated octave stack.

---

# 4. Hierarchical residual principle

Prefer a hierarchical / residual interpretation.

Conceptually:

    H0 = base body shape

    H1 = H0
       + planetary-scale geology

    H2 = H1
       + regional geology

    H3 = H2
       + local geology

    H4 = H3
       + fine geometric geology

Each finer level adds information rather than redefining unrelated terrain.

This must preserve feature identity.

Example:

    100 km crater/basin
        ↓
    basin remains present at every scale
        ↓
    approach reveals major rim
        ↓
    then broken rim sections
        ↓
    then craterlets / ejecta / local structure
        ↓
    then fractured ground / debris-scale terrain

Do not generate a new unrelated random surface every time spatial frequency increases.

---

# 5. This is not "add more noise"

Do not solve the local-scale problem with:

    height += noise(p * f1) * a1
    height += noise(p * f2) * a2
    height += noise(p * f3) * a3

unless those fields are explicitly modulated by geological context and have a clear process interpretation.

Generic high-frequency noise causes every terrain family to converge toward:

> noisy procedural ground.

That is a failure.

Each family's local geometry must remain distinct.

---

# 6. Scale-aware morphology

Large geological structures must not be uniformly scaled versions of small ones.

Feature morphology must be allowed to change by physical size regime.

For example, rocky impacts may progress conceptually through:

    tiny impact
        ↓
    simple bowl
        ↓
    complex crater
        ↓
    terraced / central-structure crater
        ↓
    basin-scale system

Do not use one crater equation multiplied by radius for every scale.

Equivalent scale-regime thinking should apply to:

- fractures,
- ridges,
- volcanic structures,
- flows,
- calderas,
- constructional terrain,
- collapse features.

Record the chosen regime boundaries or continuous transitions.

They need not be physically perfect, but they must be intentional and deterministic.

---

# 7. Target geometric scale

For this phase, target meaningful authoritative geometric structure down to approximately the **1–10 metre regime**, where practical.

Do not attempt centimetre-scale planetary geometry.

The intended future representation boundary is approximately:

    planetary → metres:
        authoritative terrain geometry

    metres → centimetres:
        future micro-displacement / relief techniques
        future explicit rocks / debris
        future material microstructure

    very small:
        future normals / roughness / shading detail

Exact production boundaries will be measured later.

Slice 1B.2 must only prove that the authoritative field remains useful at player-scale geometry.

---

# 8. Rocky local geological hierarchy

Improve RockyV4 through an explicit successor version if complete semantics change.

Preserve historical versions.

Rocky terrain should develop local structure according to province and history.

Potential local processes include:

## Ancient highlands

- dense small degraded impacts,
- broken rim remnants,
- inter-crater hummocks,
- irregular ridge fragments,
- shallow overlapping depressions,
- old ejecta remnants,
- fractured high-relief ground.

## Basin margins

- fractured rim relief,
- radial / concentric structural lineaments,
- slumping or terraced morphology,
- smaller secondary impacts,
- local ridges and troughs,
- coarse ejecta-related terrain.

## Resurfaced plains

- genuinely smoother broad terrain,
- but not mathematically empty,
- partially buried old structures,
- sparse younger craterlets,
- subdued regional undulation.

## Structural uplands

- directional ridges,
- broken relief,
- non-crater morphology,
- locally enhanced roughness.

Local rocky detail must depend on context.

A crater floor should not receive the same local relief grammar as a broken ancient rim.

---

# 9. Icy local geological hierarchy

Improve IcyV2 through a successor version if necessary.

At near scale, icy terrain must not collapse into smooth rolling heightfields.

Add hierarchical ice-specific structure such as:

## Primary tectonic structures

- large chasmata,
- ridges,
- troughs,
- warped bands.

## Secondary fracture hierarchy

- smaller fractures branching from or responding to larger stress structures,
- intersecting cracks,
- fracture shoulders,
- offset / distorted segments.

## Reworked / chaotic terrain

- broken plate-like or block-like relief,
- irregular depressions and ridges,
- disrupted older terrain.

## Young resurfaced ice

- smoother than old terrain,
- but capable of subtle flow / freezing / pressure structure,
- sparse recent fractures,
- partially buried old relief.

## Old ice

- retained impacts,
- softened / relaxed crater morphology,
- old fracture remnants.

Do not represent the hierarchy as parallel uniformly spaced decorative grooves.

Structure should correlate with:

- stress direction,
- director fields,
- province,
- age,
- resurfacing,
- nearby larger structures.

---

# 10. Volcanic local geological hierarchy

Improve VolcanicV2 through a successor version if necessary.

Near terrain must no longer become a generic gentle slope.

Potential local structures include:

## Flow fields

- nested or overlapping flow lobes,
- emplacement fronts,
- pressure ridges,
- flow channels,
- small collapse structures.

## Volcanic centres

- constructional swells,
- irregular summit depressions,
- nested caldera structures,
- vent chains,
- radial / directional structures.

## Old/new terrain boundaries

- partial burial of old relief,
- flow fronts crossing older ground,
- isolated old-terrain remnants.

## Young plains

- smoother large-scale form,
- but with local flow texture and bounded structural relief.

Do not make every volcanic feature an ellipse, cone or radial stamp.

Use process history so surrounding terrain records emplacement.

---

# 11. Parent-feature context

Fine structures should inherit context from larger structures.

Examples:

Rocky:

    large crater rim
        ↓
    fractured sectors
        ↓
    smaller impacts / grooves / debris-like relief

Icy:

    major fracture
        ↓
    shoulder ridges
        ↓
    secondary cracks
        ↓
    local broken ice relief

Volcanic:

    major flow
        ↓
    flow lobe
        ↓
    pressure fronts
        ↓
    local crust-break / collapse relief

This does not require storing explicit object trees if deterministic spatial queries can derive equivalent relationships.

The important requirement is geological correlation across scale.

---

# 12. Bounded process composition

Do not indefinitely sum detail fields.

Preserve bounded relief.

Use deterministic bounded composition such as:

- masked residuals,
- convex / normalized blending,
- bounded additive budgets,
- burial / replacement rules,
- context-sensitive suppression.

Later processes may modify earlier ones.

Examples:

- resurfacing suppresses old fine terrain,
- volcanic burial removes old small craters,
- ice relaxation smooths older impact detail,
- younger fractures cut through old ice,
- recent impacts disturb older local relief.

All envelope changes must remain defensible analytically or conservatively.

---

# 13. Spatial scalability

Maintain spatially bounded generation.

Do not introduce a world-wide catalogue of every 5 m feature.

Use the established approach:

- deterministic regional/cell generation,
- nested cell levels where useful,
- support halos,
- bounded support,
- local feature candidates,
- stable salts.

Fine detail must be queryable locally without constructing the complete body.

Record work counts for the added hierarchy.

---

# 14. Stable deterministic identity

Preserve independent salts / namespaces for:

- body phenotype,
- director fields,
- province layout,
- macro geology,
- regional geology,
- local geology,
- fine geometric geology,
- materials.

Changing renderer state must not affect terrain.

Changing material-only parameters must not affect geometry.

Camera position must not affect authoritative terrain.

Worker scheduling/order must not affect terrain.

---

# 15. Diagnostic decomposition

Add a diagnostic method to inspect which scale bands/processes contribute to the final surface.

This must be diagnostic / derived metadata, not a second authoritative terrain definition.

For representative samples/crops expose enough information to distinguish, for example:

- macro contribution,
- regional contribution,
- local contribution,
- fine-geometric contribution.

If implementation structure makes direct component output unsafe or misleading, expose equivalent process-strength / contribution diagnostics instead.

The purpose is to answer:

> What geological scale created this feature?

---

# 16. Feature-preserving future-LOD preparation

Do not implement LOD yet.

However, structure the authoritative field so Slice 2 can later derive filtered tiles without destroying important large features.

The future intended relationship is:

    parent representation
        = meaningful coarse surface

    child representation
        = parent-compatible surface
        + finer residual information

Do not make child detail depend on random state unavailable to the parent/tile builder.

Where practical, add tests or diagnostic probes showing that removing fine bands leaves the major feature identity intact.

This is **not** a requirement to build production low-pass filters or GPU reconstruction yet.

---

# 17. Required reference scales

Retain the established scales:

- 20 km
- 2 km
- 256 m
- 32 m

Add at least one smaller local scale appropriate to the new detail target.

Preferred:

- 8 m

Optionally add:

- 4 m

if the reference system can capture it without disproportionate runtime.

Do not chase sub-metre terrain in this phase.

Each scale remains a separate fixed-resolution reference crop, not continuous LOD evidence.

---

# 18. Multi-scale same-feature tracking

For representative features, do not merely choose unrelated "interesting" points at each scale.

Track the **same geological feature** while approaching.

For example:

Rocky:

    basin/rim from 20 km
        ↓
    same rim at 2 km
        ↓
    same sector at 256 m
        ↓
    same sector at 32 m
        ↓
    local structure at 8 m

Icy:

    major fracture
        ↓
    same fracture system
        ↓
    same shoulder/intersection
        ↓
    secondary cracks
        ↓
    local fractured ground

Volcanic:

    flow/caldera complex
        ↓
    same structure
        ↓
    same emplacement front
        ↓
    pressure/collapse structures
        ↓
    local volcanic terrain

Store the authoritative anchor identity/location used for the approach.

---

# 19. Geometry-only acceptance

Continue using uniform neutral-grey geometry diagnostics.

Family identity must not depend on:

- blue ice,
- red/orange volcanic colors,
- dramatic material contrast,
- special family lighting.

Use matched neutral lighting where possible.

At 256 m, 32 m and the new 8 m scale, the family should remain recognizable from morphology.

---

# 20. Near-surface gameplay readability

Judge local terrain as terrain a future player could traverse.

Ask:

- Are there ridges to walk around?
- Are there gullies/troughs worth navigating?
- Are crater rims actually terrain rather than smooth bumps?
- Do fractures create meaningful local relief?
- Do volcanic flows create recognizable fronts/ridges?
- Does the ground provide scale cues?

This phase does not implement collision gameplay or vehicles.

But the geometry should begin to support future decisions such as:

- landing,
- walking,
- driving,
- building,
- resource exploration.

Avoid terrain that is mathematically non-flat but perceptually featureless.

---

# 21. Do not put rocks into the terrain just to pass the gate

Large terrain-scale blocks / relief are allowed where geologically appropriate.

However, do not fake future explicit rock populations by covering every surface in tiny radial lumps.

Future phases should handle many:

- boulders,
- debris,
- loose rocks,
- ice chunks,
- volcanic blocks

as deterministic explicit geometry / scatter systems.

This phase is about the continuous terrain surface.

---

# 22. No micro-detail rendering yet

Do not implement:

- parallax occlusion mapping,
- relief mapping,
- micro-displacement shaders,
- tessellation purely for material detail,
- virtualized rock textures,
- explicit boulder rendering,
- procedural debris meshes.

Those are future rendering/detail layers.

The current raw geometry must become strong enough before those techniques are allowed to enhance it.

---

# 23. Testing requirements

Preserve all earlier tests and historical exact replays.

If successor versions are introduced, old versions must remain numerically unchanged.

Add tests for:

## Determinism

- scalar/batch agreement,
- repeat query agreement,
- thread/order independence,
- chart-edge reuse.

## Detail hierarchy

- local/fine bands actually contribute in intended contexts,
- resurfacing/burial suppresses old fine bands,
- families use distinct local processes,
- scale-regime changes remain bounded.

## Support boundaries

- nested/fine feature cells,
- support halos,
- cube-face edges,
- cube corners,
- finite derivatives.

## Bounded geometry

- legal combined envelopes,
- no negative combined radius,
- finite normals/gradients,
- bounded local process composition.

## Process correlation

Representative assertions such as:

- rocky broken highlands contain more local relief than resurfaced plains,
- icy fracture zones contain stronger structured local relief than young smooth ice,
- volcanic flow/construction zones contain stronger local relief than quiet resurfaced plains.

Do not hard-code artistic screenshots into tests.

Test invariant relationships.

---

# 24. Numerical corpus

Extend the successor authoritative corpus with:

- same-feature multi-scale anchors,
- local detail support boundaries,
- nested feature intersections,
- representative process-regime transitions,
- 8 m crop centres,
- multiple seeds,
- multiple radii,
- canonical cube edges/corners.

Record:

- complete height,
- gradient,
- normal,
- materials,
- geological controls,
- province weights,
- process strengths,
- scale/detail diagnostics where available,
- complete definition identity.

Require exact deterministic replay.

---

# 25. Reference package

Produce a new Slice 1B.2 review package.

At minimum include:

## Orbital

Retain existing:

- labelled 12-body geometry sheet,
- unlabelled 12-body geometry sheet,
- lit sheets.

The purpose is regression.

Do not spend the majority of the phase on orbit views.

## Regional/local

For each family:

- representative same-feature approach sequence,
- 20 km,
- 2 km,
- 256 m,
- 32 m,
- 8 m,
- neutral geometry,
- lit,
- normal,
- height,
- material,
- relevant geological/process diagnostics.

## Cross-family

Create neutral-grey comparison sheets at:

- 256 m,
- 32 m,
- 8 m.

These are primary acceptance artifacts.

## Unbiased views

Retain unbiased local crops.

Selected landmarks may not replace representative evidence.

---

# 26. Visual acceptance targets

## Rocky

At 32 m and 8 m, terrain should contain recognizable rocky/impact-derived morphology rather than smooth undulation.

Ancient highlands should not resemble resurfaced plains.

## Icy

At 32 m and 8 m, fracture/chaos provinces should retain recognizable ice-specific structural terrain.

They must not simply look like rounded ridged noise.

## Volcanic

At 32 m and 8 m, construction/flow provinces should retain volcanic morphology.

Flow terrain should contain coherent fronts/ridges/collapse or equivalent structure, not gentle generic slopes.

## Cross-family

With identical grey material and matched lighting, the three families should remain visibly different at local scale.

---

# 27. Failure criteria

Slice 1B.2 remains failed if:

- 8 m crops are still broadly featureless,
- local terrain is only differentiated by material,
- all families converge toward generic rough noise,
- increasing detail merely increases bumpiness,
- fine features ignore parent geology,
- large features disappear or change identity when approaching,
- rocky detail becomes random crater speckle everywhere,
- icy detail becomes decorative parallel grooves,
- volcanic detail becomes repeated bumps/cones,
- province context stops influencing local terrain,
- reference-mesh resolution is increased to hide an empty field,
- dramatic lighting is required for recognition.

---

# 28. Performance observations

This is still not a production performance phase.

Record:

- cell visits,
- candidate local/fine features,
- accepted features,
- reference query time,
- full reference runtime,
- memory observations already supported by the evidence workflow.

Compare successor query work against Slice 1B.1.

If fine detail multiplies per-query work excessively, redesign the spatial hierarchy rather than accepting unbounded cost.

Do not claim native FPS improvements.

---

# 29. StarEngine / external technique boundary

Do not implement external-engine techniques merely because they are modern or impressive.

For future renderer research, record the following design questions separately if useful:

- camera-relative large-world rendering,
- fixed/local GPU terrain chunks,
- on-demand terrain LOD,
- feature-preserving orbit-to-ground appearance,
- multi-scale terrain normals,
- dedicated cliff/detail layers,
- geometry-distributed detail,
- texture/mesh streaming,
- terrain-integrated object scattering.

These topics are relevant to future Slice 2+ research but are not acceptance requirements for Slice 1B.2.

Do not expand this phase into a StarEngine clone or renderer rewrite.

---

# 30. Explicitly out of scope

Do not implement:

- Slice 2 GPU resident tiles,
- tile streaming,
- replacement adaptive LOD,
- GPU displacement,
- GPU parent/child transitions,
- indirect terrain rendering,
- mesh shaders,
- compute terrain generation,
- sparse resources,
- production terrain shadows,
- parallax / relief materials,
- rock scatter/rendering,
- caves,
- voxel terrain,
- persistent terrain edits,
- atmosphere rendering,
- vegetation,
- survival gameplay.

---

# 31. Required final handoff

Use explicit labels:

- IMPLEMENTED
- VERIFIED
- MEASURED
- OBSERVED
- PARTIAL
- FAILED / OPEN

Report:

## Architecture

- successor family versions,
- hierarchical detail flow,
- scale regimes,
- feature/context relationships,
- bounded-composition strategy.

## Visuals

- 20 km → 8 m same-feature approaches,
- cross-family 256 m / 32 m / 8 m neutral sheets,
- unbiased near views,
- relevant diagnostic decomposition.

## Numerical

- exact corpus replay,
- historical replay preservation,
- detail hierarchy tests,
- boundary/derivative/envelope tests.

## Work cost

- query/candidate/accepted-feature measurements,
- reference runtime,
- clearly identified non-production scope.

## Assessment

For each family state:

- whether 256 m recognition passes,
- whether 32 m recognition passes,
- whether 8 m recognition passes,
- whether unbiased near terrain is useful,
- remaining visual weaknesses.

Do not declare success from numerical nonzero relief alone.

---

# 32. Final Slice 1B gate

This is the last planned procedural-terrain pass before Slice 2.

Do not continue inventing more terrain architecture indefinitely.

At the end, explicitly recommend one of:

### A. READY FOR SLICE 2

Use this only if:

- the authoritative field contains meaningful multi-scale structure,
- the families retain local identity,
- major features survive approach,
- no fundamental terrain representation problem remains.

Minor artistic weaknesses may remain for later refinement.

### B. BLOCKED BY SPECIFIC FUNDAMENTAL ISSUE

Use this only if a concrete representation or architectural flaw remains that would make building Slice 2 premature.

Do not block Slice 2 merely because terrain could always be made prettier.

---

# 33. Stop condition

When implementation, validation and the final review package are complete:

**STOP.**

Do not begin Slice 2.

Present the results to the user.

If the multi-scale terrain architecture works and remaining weaknesses are primarily artistic/detail-layer issues, recommend proceeding to Slice 2 rather than starting another broad procedural redesign.