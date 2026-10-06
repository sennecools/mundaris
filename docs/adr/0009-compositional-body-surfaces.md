# ADR 0009: Compositional body-surface authority

- Status: Slice 1B implementation decision; visual acceptance remains separate
- Date: 2026-10-06
- Contract: [Slice 1B](../PLANET_TERRAIN_SLICE_1B.md)

## Decision

The world owns immutable shape, geological history, material composition and
atmosphere definitions. The app adapts complete queries to disposable geometry;
the renderer does not generate geology. Body/frame runtime handles, camera state,
workers and rendering footprints cannot enter generation salts.

```text
CelestialSystem / CelestialBody (BodyId, radius, terrain revision)
  └─ selected authority: legacy terrain OR SurfaceDefinition
       ├─ ShapeDefinition: sphere / ellipsoid / bounded irregular radial graph
       ├─ SurfaceTerrainDefinition: algorithm version + correlated body history
       ├─ SurfaceMaterialDefinition: channel version + composition / contrast
       └─ SurfaceAtmosphere: airless / descriptor (no atmosphere rendering)
            ↓ immutable compiled SurfaceGenerator
       complete shape + displacement + gradients + material weights
            ├─ production radial clearance / navigation queries
            └─ temporary fixed-resolution reference mesh / diagnostics
```

Legacy definitions, `MoonLikeV1` and `MoonLikeV2` retain their complete numerical
semantics. `RockyV3` is a new wrapper algorithm, so history/province composition
cannot silently reinterpret V2's evidence. Icy and volcanic algorithms have
separate namespace/version identities. Material algorithms interpret geological
channels; incompatible channel versions are rejected instead of relabelled.

Body publication is exclusive and transactional: selecting a compositional
definition clears legacy terrain, and vice versa. Definition equality controls
terrain revision. Surface publication does not change simulation time, celestial
revision or frame projection. Radius edits validate the selected surface first.
The existing native renderer remains legacy-specific until a separately approved
renderer integration. An authored compositional surface is queryable authority;
its presence alone does not promise matching native geometry or solid collision.

## Identity and determinism

The body phenotype stage uses explicit algorithm and phenotype namespaces to
derive correlated normalized age/activity, resurfacing/retention, relief, feature
scale and directional controls. These are procedural history controls, not a
physical geological simulation or calibrated dates. Feature placement uses a
separate geology namespace from the stable authored identity and seed. Shape
orientation and material composition/variation use independent salts.

Geometry identity combines terrain algorithm/configuration/seed and shape identity.
Material identity adds material algorithm/configuration and its independent seed
namespace. Atmosphere contributes only to the full definition identity; it cannot
change complete shape, height or materials. Camera/frame/render settings are absent.
Hashes are diagnostic namespaces, not collision-proof equality: derived cache keys
must also compare the complete definition and reference-radius bits. Future tile
format/filter/edit versions remain separate derived cache inputs.

## Representation proof and boundary

For unit body-local direction `n`, the surface is
`p(n) = n * (shape_radius(n) + terrain_height(n))`. Both derivatives are tangent
gradients in metres per unit direction. The outward normal is proportional to
`n - (shape_gradient + terrain_gradient) / total_radius`. Shape envelopes and
complete displacement envelopes add outwardly; a nonpositive inner radius is
rejected. These global radial bounds are not local derivative or tile error
certificates and do not prove represented-triangle convergence.

The irregular fixture uses axes `[1.35, 0.92, 0.70]` and bounded asymmetric/quartic
modulation. This is a star-shaped radial graph with one positive radius per
direction. Some concave star-shaped surfaces fit; arbitrary concave bodies,
undercuts, caves, overhangs, enclosed cavities, and all possible contact-binary
topologies do not. The long-term sparse local-volume direction remains open;
no volumetric representation is implemented in Slice 1B. Intended non-star-shaped
body topology requires a separate design decision before applying radial tiles.

## Validation boundary

Canonical cube directions feed the same complete query at shared edges/corners.
Analytic derivatives, bounded support, overlap cases, scalar/batch repeatability,
body publication and shape-aware clearance require focused correctness tests.
Reference captures retain fixed-grid residuals, complete and reconstructed normals,
materials, represented-mesh shadows and unshadowed diagnostics separately.
Software reference timings are not native FPS or GPU performance. Visual diversity
and convincing morphology at all scales remain the user's acceptance decision.
Slice 2 cannot begin automatically after tests or capture generation.
