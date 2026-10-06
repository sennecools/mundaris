# Mundaris Planet Terrain Redesign — Slice 1B
## Procedural Moon Families, Body Identity & Surface Representation Proof

Proceed with **Slice 1B** of the planet terrain redesign.

This is a new implementation phase following:

- `docs/PLANET_TERRAIN_RENDERING_REDESIGN.md`
- `docs/PLANET_TERRAIN_SLICE_1.md`
- `docs/PLANET_TERRAIN_SLICE_1_ORBITAL_REPORT.md`
- the existing `MoonLikeV1` / `MoonLikeV2` work
- the user's visual review of the Slice 1 captures

Read those documents and the relevant current source before modifying code.

Do not rely on HEAD alone to identify the working state. The repository is intentionally dirty and contains pre-existing terrain, developer-interface and evidence work that must be preserved.

Do not commit or push unless explicitly authorized.

---

# 1. Goal

Slice 1 proved a reusable deterministic **cratered lunar terrain family** and fixed-resolution visual reference pipeline.

Slice 1B must prove that Mundaris is capable of generating **substantially different procedural moon/world identities**, rather than merely producing different seeds of one Moon-like recipe.

The core objective is:

> Establish a compositional, versioned body-surface architecture in the authoritative world layer that separates:
>
> 1. body shape,
> 2. terrain/geological structure,
> 3. material/color structure,
> 4. atmosphere state,
>
> and demonstrate that multiple genuinely different moon families can share the same authoritative query/rendering boundary.

This is still **pre-Slice-2** work.

Do not implement the replacement GPU terrain renderer yet.

The result should make the future tiled renderer largely agnostic to whether a tile represents:

- cratered rocky highlands,
- fractured ice,
- volcanic plains,
- resurfaced terrain,
- or another supported height-field-compatible surface.

---

# 2. Why this phase exists

The existing `MoonLikeV2` work has improved the cratered family, but different seeds still inherit the same broad geological grammar.

Changing a seed must eventually be capable of changing more than:

- crater locations,
- basin locations,
- local feature placement.

Mundaris needs bodies with distinct **geological identities**.

The generator should conceptually operate more like:

    body seed / physical properties
                ↓
       body surface identity
                ↓
    ┌───────────┼────────────┐
    ↓           ↓            ↓
   shape     geological    material
             history       system
    ↓           ↓            ↓
        regional provinces
                ↓
       individual features
                ↓
          local structure

Atmosphere remains a separate layer and must not become terrain-generator state.

Do not build a single giant `MoonType` switch that creates a handful of hard-coded templates.

Prefer compositional definitions with independently versioned deterministic parameters.

---

# 3. Architectural requirement: separate surface concerns

Establish or refine an authoritative world-side surface definition that clearly separates at least:

## Shape

The low-frequency/base geometry of the body.

Examples:

- near-spherical,
- oblate,
- triaxial,
- bounded irregular/star-shaped.

Shape is not the same thing as terrain detail.

A triaxial or irregular body must not require pretending that its entire departure from a sphere is "terrain noise."

## Terrain/geological structure

The history and morphology applied to the base shape.

Examples:

- ancient bombardment,
- resurfacing,
- impact degradation,
- ice tectonics,
- volcanic construction,
- fracture systems,
- plains emplacement.

## Material/composition

Authoritative body-fixed material classification correlated with geological history.

Examples:

- regolith,
- exposed rocky substrate,
- basaltic plains,
- fresh ejecta,
- old highlands,
- clean ice,
- dirty ice,
- fractured ice,
- volcanic deposits.

Materials must not merely be unrelated shader noise.

## Atmosphere

Atmosphere must remain a separate body property/definition.

Slice 1B does **not** need to implement production atmospheric rendering.

It must merely avoid designing terrain, shape or material APIs in a way that conflates them with atmosphere.

---

# 4. Avoid a template-based "moon type" architecture

Do not solve this as:

    MoonType::Cratered
    MoonType::Icy
    MoonType::Volcanic

with all properties implicitly hard-coded behind the enum.

Families may have explicit algorithm/version identifiers, but body identity should be derived from composable deterministic parameters.

For example, a generated body's identity may include characteristics such as:

- shape elongation,
- surface age,
- impact exposure,
- impact-flux history,
- basin frequency,
- resurfacing fraction,
- highland fraction,
- relief scale,
- crater degradation strength,
- ejecta retention,
- regolith development,
- fracture activity,
- tectonic directionality,
- volcanic activity,
- plains coverage,
- material composition.

Do not independently randomize dozens of unrelated knobs.

Generate **correlated parameter groups** that describe a plausible coherent surface history.

The objective is controlled procedural diversity, not visual chaos.

---

# 5. Preserve existing terrain versions

`MoonLikeV1` and `MoonLikeV2` are evidence-bearing numerical definitions.

Do not silently reinterpret them.

If their complete authoritative numerical terrain semantics change, create a new explicit version.

Existing V1/V2 deterministic query corpora and regression behaviour must remain valid unless a separately documented migration is explicitly justified.

New families require their own:

- family/version identity,
- configuration identity,
- deterministic salts,
- authoritative query behaviour,
- test corpus.

Derived rendering changes must not alter authoritative complete-field identity.

---

# 6. Required family prototypes

Implement enough authoritative functionality to demonstrate **three substantially different surface families**.

These are not final art-complete systems.

They are architectural and procedural diversity proofs.

## Family A — Ancient battered rocky moon

Reuse and refine the existing cratered work.

Do not discard `MoonLikeV2`.

This family should be capable of varying internally between, for example:

- highly saturated ancient highlands,
- basin-dominated surfaces,
- partially resurfaced rocky terrain,
- smoother impact plains,
- varying crater degradation histories.

It must not imply that every rocky moon has Earth's Moon's exact statistical appearance.

Do not spend the entire phase perfecting this family while leaving the others superficial.

---

## Family B — Fractured icy moon

Create a genuinely different geological grammar.

Potential authoritative structures include:

- large fracture/chasm systems,
- long ridges,
- intersecting tectonic bands,
- broad smoother resurfaced regions,
- displaced or disrupted older terrain,
- ice-specific impact degradation/relaxation,
- regional differences between old battered ice and younger resurfaced ice.

The result must not look like:

> rocky Moon + blue/white material.

Its **geometry itself** must communicate a different geological process.

Fractures and tectonic structures should be body-fixed, deterministic and spatially queryable without scanning every feature on the body.

Use bounded support or another scalable regional query mechanism.

---

## Family C — Resurfaced volcanic moon

Create a surface history dominated by emplacement/resurfacing rather than impact saturation.

Potential structures include:

- broad volcanic plains,
- partially buried old terrain,
- shield-like regional swells,
- caldera/depression structures,
- flow-like lobes or emplacement fronts,
- volcanic ridge systems,
- younger regions with visibly lower crater survival,
- isolated older terrain remnants.

Again, this must not look like:

> cratered Moon with darker colors.

The terrain morphology must change.

The system should be able to express different volcanic histories rather than one fixed layout.

---

# 7. Irregular-shape representation stress test

In addition to the three terrain families, implement a deliberately non-spherical **shape stress-test body**.

This is not required to be a final polished moon family.

Its purpose is architectural.

At minimum test a bounded:

- triaxial,
- asymmetric,
- visibly irregular

body shape.

Determine whether the planned representation:

    body-local direction → base shape radius → terrain displacement

remains sufficient for the range of irregular bodies Mundaris currently intends to support.

Test:

- silhouette,
- canonical cube-face traversal,
- local gradients/normals,
- large scale precision,
- terrain displacement on top of the shape,
- observer-relative reconstruction assumptions.

Explicitly document the representational boundary.

If a surface remains **star-shaped** relative to the body origin — one valid surface radius per direction — state that clearly.

Do not claim this representation supports arbitrary concave geometry, caves, overhangs or every possible contact-binary topology.

If important intended bodies cannot be represented by one radial surface per direction, stop and document the conflict before Slice 2 rather than hiding it.

Do not implement a volumetric solution in this phase.

---

# 8. Production authority integration

The Slice 1 reference terrain currently exists upstream, but the new family definitions must now be connected to the actual production world authority sufficiently that the future renderer has one authoritative source.

Do not maintain:

    reference-generator terrain

and

    production terrain

as independent truths.

Establish a clean production path conceptually similar to:

    CelestialBody
         ↓
    SurfaceDefinition
         ↓
    ShapeDefinition
    TerrainDefinition
    MaterialDefinition
    AtmosphereDefinition

Exact Rust type names are not prescribed.

Choose names that fit Mundaris's existing architecture.

The important properties are:

- world owns authoritative body identity,
- renderer remains domain-free,
- renderer does not construct procedural geology,
- camera position is not generation salt,
- runtime `FrameId` is not generation salt,
- workers do not alter results,
- derived tiles later depend on stable world identities,
- navigation/clearance queries can query the same authoritative surface.

Integrate this with `Body::terrain()` / the relevant existing production world path rather than leaving all new definitions isolated inside examples.

Do not rewrite unrelated navigation/collision systems.

Where full collision support does not exist, preserve that limitation honestly.

---

# 9. Surface query contract

The family architecture must expose enough authoritative information for Slice 2 to build derived tiles later.

At minimum establish deterministic queries for:

## Shape

Given body-local surface direction / canonical coordinate:

- base surface position or base radius,
- shape normal/gradient information sufficient for correct reconstruction,
- conservative global shape envelope.

## Terrain

- complete displacement/elevation relative to the base shape,
- authoritative gradient or equivalent normal information,
- family/version identity.

## Material

- normalized material weights/classification,
- material-family/version identity,
- correlation with geological features.

The exact API may combine these where existing architecture makes that cleaner.

Do not make derived LOD footprint filtering authoritative.

The complete world query remains truth.

---

# 10. Procedural scalability requirements

Every family must use scalable spatial queries.

Do not generate every crater/fracture/volcano/etc. on the body and linearly scan the full catalogue for each sample.

Use appropriate mechanisms such as:

- body-local deterministic spatial cells,
- hierarchical regional seeds,
- bounded feature support,
- support halos,
- region-local feature generation.

Record the query complexity and representative features/cells considered.

Avoid arbitrary whole-body feature-count ceilings introduced only to make the implementation terminate.

A safety bound may exist, but it must have an explicit reason and must not masquerade as scalability.

---

# 11. Body-level procedural identity

Introduce a deterministic body-level parameter generation stage.

For each body/family, derive a coherent set of geological controls before individual feature placement.

Examples for rocky bodies:

- surface age,
- bombardment intensity,
- basin frequency,
- resurfacing fraction,
- relief strength,
- degradation rate.

Examples for icy bodies:

- tectonic activity,
- fracture orientation families,
- resurfacing age,
- relaxation strength,
- impact retention,
- ice purity/material mixture.

Examples for volcanic bodies:

- emplacement age distribution,
- resurfaced fraction,
- volcanic centre density,
- flow scale,
- old terrain survival,
- impact survival rate.

Do not require all these exact parameters.

Use whatever physically/coherently fits the implementation.

The critical requirement is:

> Different seeds must be capable of producing different geological histories, not merely different coordinates for identical feature distributions.

---

# 12. Deterministic seed separation

Use stable, explicit deterministic salts/namespaces for conceptually separate generation stages.

For example:

- body phenotype,
- shape,
- regional provinces,
- terrain epochs,
- materials,
- family-specific feature systems.

Changing material visualization should not unexpectedly move major terrain features.

Changing unrelated rendering settings must never alter authoritative terrain.

Document the identity composition.

---

# 13. Visual reference workflow

Continue using the fixed-resolution reference renderer and existing capture/evidence workflow for visual evaluation.

Temporary rendering infrastructure remains allowed.

Do not create temporary authoritative terrain generators.

For each implemented family, capture at least:

- orbit,
- regional,
- near-surface,
- height/geometry,
- reconstructed normal,
- analytic/reference normal where applicable,
- material diagnostic,
- lit view.

Use lighting that reveals morphology rather than hides it.

Also include at least one neutral/broad-light view where appropriate so terrain quality is not dependent on dramatic grazing illumination.

---

# 14. Cross-family comparison sheet

Create a primary diversity acceptance package.

Generate at least **12 bodies total**, preferably:

- 4 rocky/cratered,
- 4 icy/fractured,
- 4 volcanic/resurfaced,

using different deterministic seeds/phenotypes.

Create:

1. a labelled sheet for debugging,
2. a second **unlabelled orbital contact sheet** for visual diversity review,
3. a mapping file that records which hidden body corresponds to each definition/seed.

The unlabelled sheet is an important acceptance artifact.

The user should not need seed labels to notice that different bodies come from genuinely different geological families.

Avoid hand-selecting only unusually good seeds without recording that selection.

If seeds are rejected for technical reasons, record why.

---

# 15. Within-family variation test

Each family must also show meaningful variation internally.

Four icy bodies must not be copies with fracture lines moved around.

Four volcanic bodies must not be the same plains percentage with different volcano positions.

Four rocky bodies must not all have the same crater saturation.

Record each body's generated high-level phenotype parameters in its metadata so visual differences can be traced back to deterministic world identity.

---

# 16. Material/color requirements

This phase should improve the conceptual material architecture, but final physically calibrated planetary rendering is out of scope.

Materials/colors should:

- belong to authoritative body-fixed surface data,
- correlate with geological history,
- differ appropriately between families,
- support regional and local variation,
- remain deterministic.

Examples:

Rocky:
- regolith,
- highland/exposed substrate,
- basaltic/resurfaced plains,
- ejecta/disturbed material.

Icy:
- relatively clean ice,
- dirty/contaminated ice,
- fractured/reworked ice,
- older irradiated/rough terrain.

Volcanic:
- fresh volcanic deposits,
- old plains,
- exposed old substrate,
- ejecta/impact disturbance where retained.

Do not represent family identity solely through dramatic color changes.

Geometry must remain distinct when viewed in neutral grayscale/height/normal diagnostics.

---

# 17. Atmosphere boundary

Do not implement atmosphere rendering in Slice 1B.

However:

- atmosphere presence must remain separate from shape/terrain/material identity,
- an airless moon family must not require special terrain API assumptions,
- future atmospheric bodies must be able to use the same body-surface authority.

If useful, define or preserve a minimal atmosphere descriptor in world/domain architecture.

Do not build clouds, scattering, weather or atmospheric GPU passes.

---

# 18. Testing requirements

Preserve all existing Slice 1 tests and evidence.

Add focused tests for the new architecture.

At minimum test:

## Identity/versioning

- identical definition + seed → identical result,
- different seed → different result,
- different family → distinct identity,
- changed relevant configuration → changed identity,
- rendering/camera settings do not alter identity.

## Shape

- finite values,
- legal radius/envelope,
- canonical face edges,
- cube corners,
- deterministic cross-chart reuse,
- correct triaxial/irregular reconstruction,
- finite gradients/normals.

## Terrain

For each family:

- deterministic scalar/batch agreement,
- bounded heights,
- finite gradients,
- spatial support boundaries,
- representative overlap/intersection cases,
- several radii where applicable,
- canonical face boundaries.

## Materials

- deterministic material queries,
- finite normalized weights,
- family identity consistency,
- feature correlation checks where practical.

## Production authority

- body selects the intended surface/family definition,
- world identity survives frame projection,
- navigation/clearance queries use authoritative world definition where integrated,
- renderer-facing adaptation does not own geology.

Do not alter tests merely to make them pass without understanding the failure.

---

# 19. Numerical evidence corpus

Create a reusable Slice 1B authoritative query corpus for later Slice 2 validation.

Include representative queries from:

- all three families,
- multiple seeds,
- different body radii,
- cube-face interiors,
- cube-face edges,
- cube corners,
- family-specific feature boundaries,
- irregular shape extremes.

Record:

- complete authoritative surface result,
- material result,
- relevant shape result,
- family/version/configuration identity.

Slice 2 must be able to use this corpus to prove that derived GPU tiles approximate the same authoritative world rather than creating a new terrain definition.

---

# 20. Performance observations

This is not a renderer-performance phase.

Still record representative generation/query costs so no family architecture is accepted while obviously scaling poorly.

Record where practical:

- query count,
- cells/regions visited,
- candidate features considered,
- accepted features,
- scratch memory,
- wall time for reference generation,
- family-specific expensive stages.

Do not claim native FPS or GPU speedups from the software reference renderer.

Do not optimize prematurely.

---

# 21. Preserve the existing repository architecture

Do not redesign unrelated systems.

Preserve:

- reference frame architecture,
- body identity versus frame identity,
- cube-sphere canonical topology,
- f64 world authority,
- observer-relative narrowing,
- camera/navigation ownership,
- developer interface,
- capture infrastructure,
- existing terrain versions,
- old production renderer,
- existing dirty unrelated work.

Do not combine this phase with:

- camera redesign,
- orbital-system generation redesign,
- ocean rendering,
- clouds,
- vegetation,
- local volumetric terrain,
- caves,
- persistent terrain editing,
- multiplayer/network architecture.

---

# 22. Explicitly out of scope

Do not implement Slice 2 features yet.

That includes:

- resident GPU terrain tiles,
- tile atlas residency,
- replacement terrain LOD,
- GPU displacement,
- parent/child GPU morphs,
- GPU terrain streaming,
- indirect GPU terrain selection,
- mesh shaders,
- sparse GPU resources,
- GPU terrain generation,
- production terrain shadows,
- final performance optimization.

Do not use future renderer work to compensate for weak procedural terrain.

---

# 23. Acceptance gates

Slice 1B is complete only if all of the following are true.

## Gate A — Architectural composition

Shape, geological terrain, materials and atmosphere concerns have clear authoritative ownership boundaries.

The renderer does not own procedural world generation.

## Gate B — Three distinct terrain families

Rocky/cratered, icy/fractured and volcanic/resurfaced families all exist as deterministic authoritative world definitions.

They must differ in geometry, not merely color.

## Gate C — Internal procedural variation

Different seeds/phenotypes within the same family produce meaningfully different geological histories.

## Gate D — Cross-family visual diversity

The 12-body unlabelled orbital sheet clearly does not look like twelve seeds from one Moon generator.

## Gate E — Regional and near-surface identity

At regional and near scales, each family retains its own terrain grammar.

Example failure:

- orbit looks icy,
- but close surface becomes the same generic rolling noise terrain as every other family.

That does not pass.

## Gate F — Shape representation proof

A visibly irregular/triaxial body works through the authoritative query system, or the limitation is explicitly demonstrated and documented before Slice 2.

## Gate G — Production world authority

The real body/world definition can own/select the new surface identity.

Do not leave the new architecture entirely inside a reference example.

## Gate H — Determinism and tests

Focused correctness tests pass without silently altering existing V1/V2 semantics.

## Gate I — User visual review

Visual acceptance still belongs to the user.

Automated tests and contact-sheet generation do not grant visual approval.

---

# 24. Required final evidence

At handoff, provide:

## Architecture

- final surface-definition ownership diagram,
- concrete world types/modules introduced or changed,
- identity/version composition,
- representation boundary for irregular bodies.

## Visuals

For each family:

- orbit,
- regional,
- near,
- height,
- normal,
- material,
- lit diagnostics.

Additionally:

- 12-body labelled contact sheet,
- 12-body unlabelled contact sheet,
- irregular-shape stress-test captures.

## Procedural metadata

For every contact-sheet body record:

- family,
- seed,
- radius,
- shape parameters,
- high-level geological phenotype,
- material identity,
- atmosphere state,
- generator/configuration version.

## Numerical

- deterministic authoritative query corpus,
- test results,
- canonical edge/corner coverage,
- family-specific feature tests.

## Performance observations

- representative query/generation work,
- cells/features visited,
- reference render wall times,
- clearly labelled as non-production performance evidence.

## Repository state

- files changed,
- preserved unrelated dirty files,
- source/evidence fingerprints where existing workflow expects them,
- validation logs,
- final `git diff --check`.

---

# 25. Handoff classification

Use explicit statuses:

- IMPLEMENTED
- VERIFIED
- MEASURED
- OBSERVED
- PARTIAL
- FAILED / OPEN

Do not convert visual observations into verified claims.

Do not claim:

- all moons implemented,
- production renderer upgraded,
- streaming complete,
- GPU terrain complete,
- atmosphere complete,
- caves/volumes supported,
- arbitrary concave bodies supported,

unless those statements are actually proven by this phase.

---

# 26. Stop condition

When Slice 1B implementation, evidence and captures are complete:

**STOP.**

Do not automatically begin Slice 2.

Present the results to the user for review.

The user must be able to answer two independent questions:

1. **Does each individual procedural body look convincing for its intended family?**
2. **Do different generated bodies genuinely feel like different worlds rather than variations of one procedural recipe?**

If either answer is no, continue iterating within Slice 1B.

Only after explicit user approval should the project proceed to the resident GPU tile / displaced-grid prototype of Slice 2.