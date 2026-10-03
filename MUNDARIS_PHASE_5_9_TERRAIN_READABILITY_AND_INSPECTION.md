# Phase 5.9 — Terrain readability and close-surface inspection

## Scope and checkpoint

Phase 5.8 was already committed at `7a9fda6` before this work began. Its gameplay
Earth radius remains 400,000 m. Unrelated pre-existing design/workflow changes are
not included in these checkpoint commits. Nothing is pushed.

Implementation checkpoints: `670fdc3` (palette/payload/content and cache regression),
`4a02744` (infinite-error LOD priority), and `7987473` (inspection/camera/guard).
The capture harness and retained evidence/documentation are a separate checkpoint.

This checkpoint adds diagnostic colour and developer navigation, not final
materials, biomes, water geometry, terrain physics, or gameplay collision.
Implementation and visual/quality acceptance are separate claims.

## Palette and content convention

`solar_system::reference_sea_level_m` supplies the reference datum independently of
the terrain definition, seed, revision, and geometry cache key. Gameplay Earth uses
**+350 m relative to the reference sphere**; other rocky bodies use zero. Below
datum means a blue **water-region diagnostic**, not an actual ocean surface. On
Moon/Mars it does not imply liquid water. The inspection panel can override the
datum without editing world terrain or generating samples.

The original Elevation, Lit, Normals and Diffuse modes retain their values.
Readability, Slope, SeaMask and RockWeight are added. Gameplay launch defaults to
Readability unless `MUNDARIS_TERRAIN_MODE` overrides it.

Readability uses moderate display-authored colours:

- Below sea level: blue `(0.10, 0.30, 0.57)`, independent of rock classification.
- Gentle lowlands: green `(0.22, 0.43, 0.20)`.
- Highland blend: brown `(0.42, 0.34, 0.24)`.
- Rock: grey `(0.43, 0.45, 0.46)`.
- Very high: pale grey `(0.82, 0.81, 0.78)`; not simulated snow.

Earth highland blending is smooth from sea+60 m to sea+250 m; pale blending from
sea+350 m to sea+650 m. Other rocky height intervals scale with their authored
relief and radius. Sea classification uses strict `height < sea_level`; the
height intervals and the rock interval are independent.

Slope is the angle between the analytic terrain normal and local radial direction,
`atan2(|radial × normal|, radial · normal)`. At direct queries this equals
`atan(|tangent_gradient| / (reference_radius + height))`. It is never patch level
or mesh resolution. Grey rock blends with smoothstep over **8–16 degrees**.
Initial 12–35 degree thresholds exposed very little rock in the measured preset;
the revised interval follows the sampled slope distribution, not amplitude tuning.
Stitch-reconciled and morph-interpolated normals are visual estimates; the UI's
complete-footprint slope remains independent terrain truth.

Colours are decoded to linear space, multiplied by the existing Phase 5.6 ambient
and directional diffuse lighting, then encoded only for non-sRGB targets. There
is no self-shadowing. LOD colours bypass terrain classification/lighting and use
twelve distinct cyclic hues with a numeric legend; hues repeat every twelve levels.

Transient GPU samples grow from 32 to 48 bytes, clipped/morph vertices from 64 to
80 bytes, and the lighting uniform from 32 to 64 bytes. Existing aggregate caps
still apply. Cached f64 geometry, topology, stitch indices and generator identity
are unchanged. Height subtraction occurs in f64 before GPU narrowing.

## Clearance, inspection and readiness

The app converts the observer into the selected body's body-fixed frame and uses
that body's radius, terrain definition/version and seed:

```text
r_camera = length(camera_body_fixed)
direction = normalize(camera_body_fixed)
height = terrain_query(direction, COMPLETE footprint)
surface_radius = reference_radius + height
terrain_clearance = r_camera - surface_radius
```

This is an allocation-free direct point query, not Grid16 generation, nearest-point
distance, or collision. The panel reports centre distance, reference-sphere altitude,
terrain height, displaced radius, signed terrain clearance, direction, slope and
query cost. Negative values display **INSIDE TERRAIN** explicitly.

Complete terrain and currently published geometry can differ materially. A second
radial probe intersects the relevant stitched triangles, or the current local
common-refinement morph. It reports **drawn mesh clearance**, its source patch
identity/LOD, physical footprint, and **INSIDE DRAWN MESH** separately. Source cover
counts, visible minimum/maximum LOD, pending work, ownership and active morphs are
exposed. A missing/not-admitted ready mesh is explicitly unavailable, not zero height.
Terrain-under-camera direction/elevation/slope and patch identity are the minimum
CPU terrain-aware inspection/picking path. Pointer patch addressing remains the
older reference-sphere approximation and is not claimed as displaced cursor picking.

Body Orbit clearance presets resample complete terrain at the current direction
and smooth clearance relative to its displaced radius. Translating attachments
convert the radial direction into body-fixed axes before querying. Co-rotating
inspection presets preserve look orientation and place the same observer radially.
The complete query does not depend on mesh readiness. If the ready mesh lies above
the requested pose, its separate penetration warning makes that limitation explicit.

Optional guard choices are **Disabled / 2 m / 10 m / 100 m**. The app pushes outward
above `max(complete terrain radius, published mesh radius) + minimum`. It changes
only observer state, preserves orientation, and respects the current morph. Exact
complete-terrain clearance is not promised while a coarser mesh forces extra space.
After a guard correction, the published cover is re-culled for the final camera
pose and mesh-constrained near plane without selection, generation or another
publication/morph update. Rendering never uses the previous pose's visible subset.
It is not swept collision, slope-normal collision, walking, or rigid-body physics.
Unattached system Free Flight has no terrain guard; attached inspection is the
supported close-surface workflow.

## Proven close-range failure and bounded correction

The retained Phase 5.8 capture already recorded a capture-only ready-mesh correction.
Phase 5.9 retains unadjusted analytic and legacy sphere-relative poses, plus separately
named guarded captures: no correction is silently hidden in the primary matrix.

At the deterministic Earth direction
`(0.04592207301441123, 0.4595519490375417, 0.886962890625)`, complete terrain elevation
is **205.185151 m**. In the original-priority bounded cover the drawn radial elevation
is **347.631504 m**. A legacy 100 m sphere-altitude camera is consequently **105.185 m
inside complete terrain and 247.632 m inside the mesh**. Its retained normal-cull
capture is blank while ready coverage and surface ownership remain present.

An independent native GPU regression holds geometry, ownership and near plane fixed:
10 m inside a +500 m displaced shell gives **0/19,200 occupied pixels** with backface
culling, **19,200/19,200 with no-cull**, and **19,200/19,200 after placing the camera
10 m outside**. This proves the backface/penetration mechanism; absence of a ready
cover, horizon rejection and non-finite projection are not the cause in this repro.
The actual generated-scene exact-camera no-cull images provide the corresponding
intervention on the gameplay terrain.

Corrections are terrain-relative approach, explicit truth/mesh penetration diagnostics,
and the optional guard against both surfaces. Ordinary backface culling is retained;
the existing no-cull diagnostic remains available when deliberately inside terrain.
This bounds inspection reliably without pretending an inside camera is a rendering
success or implementing final navigation collision.

The investigation also found an LOD priority defect: many conservative errors become
infinite near the eye; address ordering then starved the radial camera patch while
unrelated patches reached high levels. Infinite ties now prefer the radial camera
leaf, then source-space patch-centre distance, then address. Finite ordering and its
pending completion policy remain unchanged. Certificates, thresholds, balancing,
readiness transactions, stitch topology and morph construction are not weakened.
At the exact body centre no radial leaf exists; distance/address fallback remains
valid instead of treating an undefined radial direction as a preparation error.

## Near plane

Previously: `max(0.1 m, 0.01 * minimum_reference_sphere_altitude)`, falling back to
0.1 m inside a sphere. Now complete terrain clearance and ready-mesh clearance
constrain the same rule. The **0.1 m floor is unchanged**. This prevents a mountain's
sphere altitude from choosing a clip plane larger than its actual close clearance.
The root-cause reproduction already uses 0.1 m and is not fixed by a tinier plane.

The existing infinite reverse-Z celestial depth path is retained. No depth-format,
projection topology or reversed-Z rewrite is introduced. A smaller contextual near
plane allocates more depth range to close geometry at that view; distant precision
is still subject to the existing f32 depth/projection budgets. No GPU precision or
performance benefit is claimed from CPU timing.

## Evidence and validation

See [Phase 5.9 evidence](docs/evidence/phase59/README.md) for exact-camera images,
six-height LOD/clearance tables, CPU preparation/query measurements, morphology
observations and Windows validation results. The harness uses a native graphics
adapter with offscreen readback; it is not an interactive control-feel test.

The final five supplemental targets add deterministic above-sea highland/plain/
steep/erosion samples and a shore crossing checked in complete and filtered fields.
An explicitly labelled local side light makes their classifications inspectable;
the baseline matrix still uses the authored Sun. The steep target visibly blends
green to grey and the sea mask crosses blue/green. Highland/mountain silhouettes,
branching gullies and interesting coastline morphology are **not visually accepted**:
even the strongest sampled erosion contribution remains visually subtle. A high
radial LOD is not a promise of screen-wide fine detail, especially in oblique views.
No terrain amplitude was changed to disguise this evidence gap.

Final Windows quality checks passed: formatting; locked all-target/all-feature
workspace check; warnings-denied Clippy and Rustdoc; **215 debug and 215 release
tests** (three ignored in each ordinary run); both separately selected long orbits;
and the separately selected native GPU regression. Exact commands and exclusions
are in [Phase 5 validation](docs/phase-5-validation.md). Compilation and offscreen
readback are not human control-feel, OS recovery, Linux or remote-CI acceptance.

## Retained limitations

- Phase 5.7 synchronous morph-construction performance and aggregate resource pressure.
- Quality convergence is not certified by complete ready coverage.
- Terrain horizon certification remains open; uncertified horizon occlusion stays disabled.
- Fine erosion/branching, mountain silhouettes and coastline morphology are not accepted by these captures.
- Final materials, actual ocean rendering, biomes and self-shadowing remain absent.
- Collision/gameplay navigation, swept camera movement and unattached flight guards remain absent.
- Inspection movement speed still scales from reference-sphere altitude; near-ground control feel is not certified.
- Windows is the tested native platform; no Linux or remote CI claim is made here.
