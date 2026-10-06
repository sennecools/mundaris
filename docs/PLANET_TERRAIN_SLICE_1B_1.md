# Mundaris Planet Terrain Redesign — Slice 1B.1
## Geological Province Director & Multi-Scale Family Morphology

Proceed with **Slice 1B.1**.

This is a focused continuation of Slice 1B.

Read and preserve the decisions in:

- `docs/PLANET_TERRAIN_RENDERING_REDESIGN.md`
- `docs/PLANET_TERRAIN_SLICE_1B.md`
- `docs/PLANET_TERRAIN_SLICE_1B_REPORT.md`
- ADR 0009 and the current surface-authority implementation
- the existing RockyV3 / IcyV1 / VolcanicV1 definitions
- the current reference/evidence workflow

Do not begin Slice 2.

Do not commit or push unless explicitly authorized.

The repository is intentionally dirty. Preserve unrelated existing work.

---

# 1. Goal

Slice 1B successfully established:

- compositional body-surface authority,
- separate shape / geology / material / atmosphere ownership,
- rocky, icy and volcanic procedural families,
- irregular/star-shaped body support,
- production body authority,
- deterministic query corpora,
- production clearance integration.

However, visual morphology is still insufficient.

The current families are numerically distinct, but at regional and especially near-surface scales they do not yet read as convincingly different geological systems.

This phase must add a **geological province/director layer** so that:

1. a single body contains substantially different geological regions,
2. different seeds produce different geological histories rather than only different feature coordinates,
3. rocky, icy and volcanic families remain recognizable in **geometry alone**,
4. family identity survives from orbit down to near-surface scale.

The primary objective is:

> Replace globally uniform family recipes with deterministic, low-frequency geological control fields that direct which terrain processes dominate each region.

---

# 2. Core concept

Each body should generate a coherent body-level phenotype, then one or more low-frequency regional control fields, then family-specific geological provinces, and finally local terrain processes.

Conceptually:

    Body seed / physical properties
                 ↓
          Body phenotype
                 ↓
      Geological control fields
                 ↓
       Regional provinces
                 ↓
    Family-specific processes
                 ↓
      Local terrain features
                 ↓
    Height + gradient + material

This must not become:

    low-frequency noise
         ↓
    arbitrary biome color
         ↓
    same terrain everywhere

The director fields must influence actual morphology.

---

# 3. Terminology

Do not call all regional terrain variation a "biome."

Use terminology appropriate to geological generation.

Prefer concepts such as:

- geological province,
- terrain province,
- structural province,
- resurfacing province,
- tectonic province,
- impact province.

Ecological/climate biomes are future higher-level systems and must remain separate.

---

# 4. Preserve current architecture

Do not replace the Slice 1B authority model.

Preserve:

- `CelestialBody` ownership,
- `SurfaceDefinition`,
- shape definition,
- terrain/geology definition,
- material definition,
- atmosphere separation,
- body/frame identity separation,
- deterministic versioning,
- complete f64 world queries,
- canonical cube-sphere coordinates,
- production clearance,
- existing RockyV3 / IcyV1 / VolcanicV1 semantics unless versioned forward,
- irregular star-shaped representation contract,
- current evidence/capture tooling.

If authoritative numerical semantics change, create an explicit new version.

Do not silently reinterpret existing accepted numerical checkpoints.

---

# 5. Geological director fields

Add deterministic, very-low-frequency body-fixed control fields appropriate to each family.

These fields are **not direct terrain height**.

They describe geological conditions.

Possible shared fields include:

- crust age,
- structural activity,
- regional roughness potential,
- resurfacing intensity,
- impact preservation,
- relief potential,
- material/composition tendency.

Possible family-specific fields include:

Rocky:
- ancient crust age,
- basin influence,
- bombardment exposure,
- uplift/structural roughness,
- resurfacing/fill tendency.

Icy:
- crust thickness,
- internal heat,
- tidal stress,
- fracture activity,
- relaxation strength,
- resurfacing age.

Volcanic:
- thermal activity,
- emplacement age,
- volcanic-centre potential,
- crust age,
- flow accumulation,
- resurfacing fraction.

Do not require these exact names.

Choose a compact, coherent set.

---

# 6. Director-field requirements

Director fields must be:

- deterministic,
- body-fixed,
- independent of camera/render footprint,
- independent of worker order,
- independent of GPU state,
- continuous or intentionally bounded across cube-face boundaries,
- stable under repeated queries,
- versioned as part of authoritative family semantics.

Use very-low-frequency fields or an equivalent hierarchical regional mechanism.

Avoid noisy province boundaries.

Avoid obvious Voronoi/patch blobs unless they are intentionally transformed into plausible geology.

---

# 7. Province generation

Convert the body-level fields into overlapping or weighted geological provinces.

Do not use abrupt hard switches unless physically intentional.

Province weights should transition smoothly where appropriate.

The system may use:

- smooth weighting,
- hierarchical masks,
- process activation thresholds with blended support,
- correlated multi-channel selectors.

Do not merely evaluate three full terrain functions and blindly `lerp` between them everywhere if a more process-oriented implementation fits the architecture better.

The preferred model is:

> regional controls alter which geological processes are active and how strongly they operate.

---

# 8. Rocky morphology requirements

The rocky family must support visibly different provinces such as:

## Ancient highlands

Expected character:

- high crater saturation,
- overlapping degraded impacts,
- rough inter-crater terrain,
- incomplete rims,
- old relief remnants,
- stronger local roughness.

## Basin / basin-margin province

Expected character:

- major basin-scale morphology,
- fractured/rough basin margins,
- broad structural deformation,
- regional relief beyond the crater cavity itself.

## Resurfaced plains

Expected character:

- partially erased older terrain,
- lower surviving crater density,
- smoother large-scale surface,
- later younger impacts over the resurfaced region.

## Disturbed / uplifted / structurally rough province

Expected character:

- non-crater ridges or structural relief,
- terrain that does not reduce to crater stamps.

The rocky family must no longer read as:

> one crater recipe applied globally.

---

# 9. Icy morphology requirements

The icy family must be recognizable from geometry without relying on blue color.

Add a geological grammar including several of:

- long fractures,
- intersecting fracture systems,
- ridges,
- troughs/chasmata,
- warped resurfaced bands,
- smoother young ice,
- old cratered ice,
- locally chaotic/reworked terrain,
- impact relaxation / softened old structures.

Director fields should affect:

- fracture density,
- fracture orientation,
- ridge/trough amplitude,
- resurfacing,
- impact preservation,
- relaxation.

Fracture systems must not look like repeated evenly spaced decorative lines.

Use deterministic structural orientation fields or correlated stress-like directions where useful.

At regional scale, the viewer should be able to identify fracture-dominated and resurfaced regions.

At near scale, icy terrain must not become a nearly planar sheet.

---

# 10. Volcanic morphology requirements

The volcanic family must be recognizable from geometry without relying on orange/red materials.

Add a geological grammar including several of:

- broad resurfaced plains,
- shield-like constructional terrain,
- caldera/depression systems,
- flow/emplacement fronts,
- ridge or vent chains,
- partially buried older terrain,
- boundaries between young and old terrain,
- locally reduced crater survival.

Director fields should affect:

- volcanic-centre density,
- emplacement intensity,
- resurfacing age,
- terrain burial,
- caldera scale,
- flow orientation/spread,
- old terrain survival.

Volcanic structures must stop reading as repeated ellipsoidal stamps.

Use regional history and process interaction so that features modify surrounding terrain coherently.

At near scale, volcanic terrain must show constructional and/or emplacement structure, not just uniform slopes.

---

# 11. Within-family variation

Different seeds within one family must produce different geological histories.

Do not merely reposition the same provinces.

Each body phenotype should deterministically derive correlated high-level controls.

For example, rocky worlds may vary in:

- highland fraction,
- basin dominance,
- surface age,
- resurfacing fraction,
- impact saturation,
- degradation strength.

Icy worlds may vary in:

- fracture activity,
- young resurfaced fraction,
- old crater survival,
- stress orientation families,
- internal heat.

Volcanic worlds may vary in:

- resurfacing fraction,
- old terrain survival,
- shield/caldera prevalence,
- flow scale,
- thermal activity.

Record these phenotype parameters in capture metadata.

---

# 12. Multi-scale requirement

Each geological family/province must remain meaningful at multiple scales.

At minimum evaluate:

- orbital,
- regional,
- near-surface.

A valid system must not behave like:

    orbit: distinct family
    regional: weak distinction
    near: generic smooth terrain

That is the current failure mode and must be corrected.

---

# 13. Near-surface is the primary gate

For this phase, prioritize **near-surface morphological identity**.

For each family choose representative province-specific locations.

At minimum capture:

## Rocky

- ancient highlands,
- basin rim or heavily disturbed impact region,
- resurfaced/smoother plain.

## Icy

- major fracture/ridge zone,
- young resurfaced ice,
- old cratered/reworked ice.

## Volcanic

- caldera/rim or constructional centre,
- flow/resurfaced plain,
- old/new terrain boundary.

Do not rely only on arbitrary +Z crops.

Keep unbiased fixed crops as regression evidence, but add deterministic province-targeted captures.

---

# 14. Multi-distance local captures

For selected representative provinces, generate local captures at multiple scales.

Use appropriate repeatable scales, for example approximately:

- tens of kilometres,
- kilometres,
- hundreds of metres,
- tens of metres.

Exact values may be chosen to fit body radius and the reference renderer.

The purpose is to prove that morphology remains coherent as the camera approaches.

Avoid claiming continuous LOD/streaming acceptance; these remain fixed reference crops.

---

# 15. Geometry-only family recognition gate

This is a critical acceptance test.

Create a comparison set where:

- all families use the same neutral grey material,
- identical or matched lighting is used,
- no family-specific color cues are present.

The viewer should still be able to distinguish:

## Rocky

- impact saturation,
- degraded highlands,
- basin structures,
- crater overlap.

## Icy

- fractures,
- ridges,
- chasmata,
- smooth resurfaced zones,
- relaxed/reworked impact terrain.

## Volcanic

- constructional swells,
- plains,
- flows,
- calderas,
- resurfacing boundaries.

If family identity disappears when color is removed, Gate E remains failed.

---

# 16. Material integration

Materials should continue to correlate with world geology.

Province fields may influence materials, but materials must not create fake morphology.

Examples:

Rocky:
- highland regolith,
- basin/plain deposits,
- fresh ejecta,
- exposed substrate.

Icy:
- clean young ice,
- older dirty/irradiated ice,
- fractured/reworked ice,
- exposed substrate/contaminant.

Volcanic:
- fresh flows,
- old plains,
- altered terrain,
- exposed old substrate.

Do not use material contrast to hide weak geometry.

---

# 17. Process interaction

Where practical, allow later geological processes to modify earlier ones.

Examples:

- resurfacing partially buries old craters,
- volcanic plains suppress old micro-relief,
- fractures cut through older icy terrain,
- younger impacts cut across older structures,
- relaxation softens old icy impact morphology.

Do not simply sum every terrain process indefinitely.

Use bounded, deterministic compositing.

Preserve valid global relief/envelope bounds.

---

# 18. Spatial scalability

Maintain bounded regional query work.

Do not create whole-body feature catalogues that must be scanned for every sample.

Continue using scalable techniques such as:

- regional cells,
- deterministic hierarchical seeds,
- bounded supports,
- support halos,
- province-local candidate generation.

Record representative:

- cells visited,
- candidate features,
- accepted features,
- family/province query costs.

Do not introduce unexplained fixed global limits.

---

# 19. Control-field diagnostics

Add diagnostic output for the geological director itself.

For each family, provide visualizations of the relevant control fields/province weights.

These are debugging views, not user-facing final rendering.

Examples:

- province assignment/weight,
- age/resurfacing,
- activity/stress,
- impact retention,
- volcanic activity.

The diagnostics should make it possible to answer:

> Why did this terrain process appear here?

Do not collapse all controls into one unreadable RGB image only; provide individually inspectable channels or clear labelled composites.

---

# 20. Province metadata

Each reference scene must record:

- body family/version,
- seed,
- radius,
- body phenotype,
- queried geological controls,
- dominant province weights,
- selected family processes,
- selected landmark identity where applicable.

This allows visual results to be traced back to authoritative world data.

---

# 21. Deterministic identity namespaces

Preserve or extend separate deterministic salts for:

- body phenotype,
- shape,
- geological director,
- province structure,
- family-specific features,
- material composition.

Changing presentation must not move terrain.

Changing material-only controls must not move geometry.

Changing atmosphere must not move terrain.

Camera/render footprint/worker order must not affect world generation.

---

# 22. Testing

Preserve all existing Slice 1 and Slice 1B tests.

Add focused tests for the director/province system.

At minimum:

## Director determinism

- same definition + seed = same control fields,
- different seed = deterministic difference,
- camera/worker/render state has no effect.

## Continuity

- cube-face edges,
- corners,
- province boundary continuity,
- support halos,
- finite derivatives.

## Province behaviour

- each family can produce at least the intended province classes,
- dominant province weights are normalized/bounded where applicable,
- transitions do not create discontinuous height jumps unless intentionally defined.

## Process correlation

Examples where practical:

- high icy fracture activity increases fracture structures,
- strong resurfacing reduces retained old relief,
- high volcanic activity increases construction/emplacement structures,
- old rocky highlands retain stronger degraded impact history.

Do not overfit tests to exact artistic values.

Test invariants and deterministic relationships.

---

# 23. Numerical corpus

Extend the authoritative corpus with:

- province centres,
- province boundaries,
- mixed-transition regions,
- strong family-specific process regions,
- multiple radii,
- multiple seeds,
- canonical cube edges/corners.

Record:

- shape result,
- terrain height,
- gradient/normal,
- material weights,
- geological control values,
- province weights,
- family/version identity.

This corpus becomes part of the future Slice 2 oracle.

---

# 24. Reference capture package

Generate a new Slice 1B.1 package.

At minimum include:

## Orbital

- labelled 12-body contact sheet,
- unlabelled 12-body contact sheet,
- neutral-grey geometry-only 12-body contact sheet,
- normal/material/director diagnostic sheets.

## Regional

For each family:

- representative province views,
- geometry-only,
- lit,
- height,
- normal,
- material,
- geological-director diagnostics.

## Near

For each family:

- at least three province types,
- multiple approach scales,
- neutral-grey geometry,
- normal,
- material,
- lit.

Also retain unbiased regression crops.

Do not hand-select only flattering locations without recording the selection method.

---

# 25. Reference renderer scope

Do not significantly redesign the software reference renderer unless required to expose the authoritative field correctly.

It is an evidence tool.

The current reference workflow is expensive and is not the production renderer.

Do not spend this phase optimizing it as if it were shipping code.

Do not interpret software-reference timings as production FPS.

---

# 26. Visual acceptance criteria

## Gate D — Cross-family orbital diversity

Still required.

The unlabelled sheet must not look like:

> one procedural recipe recolored three ways.

## Gate E — Regional / near identity

This is the primary phase gate.

A viewer should be able to identify geological family from **neutral geometry alone** at regional and representative near scales.

## Within-body province diversity

At least one body from each family should visibly contain multiple different geological provinces.

A single body must not look globally homogeneous.

## Within-family diversity

Different seeds must produce different body histories, not merely shifted feature layouts.

---

# 27. Failure criteria

Do not accept the phase if:

- icy identity depends mainly on blue material,
- volcanic identity depends mainly on orange/red material,
- near-surface views remain nearly planar,
- volcanic features remain repeated ellipsoidal stamps,
- icy fractures remain regular decorative bands,
- rocky terrain remains a globally uniform crater carpet,
- geological province boundaries appear as obvious noise blobs,
- different seeds retain essentially identical province statistics/history,
- dramatic lighting is required to reveal terrain,
- tests pass but the geometry still looks generic.

---

# 28. Out of scope

Do not begin:

- resident GPU terrain tiles,
- GPU displacement,
- replacement LOD,
- GPU morphing,
- tile streaming,
- mesh shaders,
- indirect terrain selection,
- sparse GPU residency,
- GPU terrain generation,
- production shadows,
- caves,
- overhangs,
- volumetric terrain,
- atmosphere rendering,
- vegetation,
- final Earth climate/ecology.

This phase is about authoritative procedural morphology.

---

# 29. Required final handoff

Report using explicit labels:

- IMPLEMENTED
- VERIFIED
- MEASURED
- OBSERVED
- PARTIAL
- FAILED / OPEN

Provide:

## Architecture

- control-field/province data flow,
- family-specific process composition,
- identity/version changes,
- files changed.

## Visuals

- neutral 12-body orbital sheet,
- labelled/unlabelled sheets,
- province diagnostic sheets,
- regional province comparisons,
- multi-distance near comparisons.

## Numerical evidence

- extended corpus,
- deterministic replay,
- boundary tests,
- director/province invariants.

## Performance observations

- cells/features/query work,
- reference generation timings,
- clearly labelled non-production scope.

## Visual assessment

For each family state explicitly:

- what is now visually convincing,
- what remains weak,
- whether geometry-only recognition succeeds,
- whether near-scale identity succeeds.

Do not declare Gate D or E passed without inspecting the actual final images.

---

# 30. Stop condition

When the implementation and evidence package are complete:

**STOP within Slice 1B.1.**

Do not begin Slice 2 automatically.

Present the final results for user review.

Slice 2 may begin only when the user is satisfied that:

1. the three families look genuinely different,
2. individual bodies contain visibly different geological provinces,
3. different seeds represent different geological histories,
4. regional and near-surface terrain retain family identity,
5. neutral geometry alone communicates the family,
6. the terrain no longer collapses into generic smooth heightfields when approached.