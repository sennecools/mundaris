# Planet terrain and rendering redesign

**2026-10-05 — redesign proposal with Slice 1 authorized.** The user
requested a new LOD/rendering plan after rejecting the current visual result.
Implementation of the crater experiment is paused. Product acceptance belongs to
the user; numerical correctness does not establish convincing terrain.

The user subsequently requested starting this plan on 2026-10-05. Implementation
begins with the [Slice 1 contract](PLANET_TERRAIN_SLICE_1.md): reusable upstream
moon terrain and fixed-resolution visual references. Later slices remain gated
by the evidence and user review below. The audit and paused-state descriptions
that follow describe the preceding checkpoint.

## Direction

Build a procedural moon-like vertical slice with coherent terrain from orbit to
walking distance. The leading architecture to prototype is:
**persistent GPU height/material tiles, instanced regular patch grids, and transitions
evaluated on the GPU**. Tile layout and residency remain hypotheses until the
prototype proves continuity, edit invalidation and measured memory cost. Keep
authoritative generation, edits and collision upstream.
Reuse correct frame and topology foundations rather than rewriting unrelated systems.

### Variety across moons — user clarification, 2026-10-05

"Moon" is an orbital role, not a universal terrain or color preset. The supplied
lunar images define the ancient cratered airless family's readability target.
They do not define the appearance of every generated satellite. The system must
also support bodies with substantially different shape, surface structure,
composition, colors and atmospheric presentation. Recoloring a common crater
field does not satisfy that requirement.

Separate body shape, terrain/process family, material definition and optional
atmosphere/liquid presentation. Seeded variation operates within and across
compatible families; those choices must be explicit versioned definition inputs,
independent of orbital classification, camera or tile residency. Share the
sampling/precision/cache boundaries, rather than forcing every family through
the lunar impact parameters or its regolith/rock/basalt material channels.
Derived tile/cache identity must include the family namespace and generator/
material-format versions; a seed or a version number local to one family is
insufficient to prevent reuse across different surface definitions.

| Reference family | Distinct appearance to support | Consequence for the design |
| --- | --- | --- |
| Ancient cratered airless body | Multi-scale impact history, degraded rims, highlands and smoother plains | Current Slice 1 visual target and first implemented family |
| Phobos / Deimos-like | Irregular silhouette and differing crater/regolith structure | Shape must be a first-class input; the current near-sphere height envelope is not evidence of this capability |
| Europa-like | Ice, fractures, ridges and disrupted terrain | Distinct structural generators and ice materials, not a crater-density preset |
| Io-like | Volcanic resurfacing, vents/flows and contrasting deposits | Distinct resurfacing/landform and material rules |
| Titan-like | Surface terrain/materials plus atmospheric haze | Separate surface and atmosphere acceptance; orbital appearance cannot be established by a surface palette alone |

Physical reference orientation: [NASA Mars moons](https://science.nasa.gov/mars/moons/facts/),
[Europa](https://science.nasa.gov/jupiter/jupiter-moons/europa/europa-facts/),
[Io](https://science.nasa.gov/jupiter/jupiter-moons/io/) and
[Titan](https://science.nasa.gov/saturn/moons/titan/facts/).
These are family/readability references, not requests to duplicate named bodies
or their specific geography.

IMPLEMENTED scope remains the cratered family's `MoonLikeV1`/`MoonLikeV2` complete
oracle. The name identifies this implementation; it is not the eventual generic
body-surface contract. The current generator's spherical reference, displacement
limit and three material weights do not establish support for the other families.
There is no generic shape/material-family definition or family dispatch yet.
Future family contracts must supply their own complete shape/height, derivative,
material and bounds semantics. A strongly irregular radial shape needs validated
bounds and navigation/culling behavior; surfaces outside a single-valued radial
representation need a separate geometry decision. Do not silently expand the
lunar generator's envelope or reinterpret its version to accommodate them.

Keep the current lunar reference work focused. Each additional family needs a
scoped implementation and matched orbital/regional/near evidence, with multiple
seeds and genuinely distinct structure and materials. Future renderer acceptance
must include distinct-family fixtures so tile formats, cache identity and adapters
do not become crater-family assumptions. Oceans, haze/clouds and atmosphere remain
separate implementation work, rather than being claimed from this surface slice.

The product remains infinite varied planetary systems, including differently sized
and spaced moons. The Solar System preset is a regression fixture. Start with an
airless body; Earth, water and atmosphere add separate complexity. Smaller bodies
with larger recognizable landmarks remain the visual direction; the existing
approximately 218 km Moon diameter is a convenient test size, not a universal rule.

RAM and VRAM are resources to spend where they improve quality and responsiveness.
The user explicitly retired the old 128 MiB constraint and authorized using the
machine's resources. The experiment currently allows 512 MiB of accounted CPU
terrain; that is a starting configuration, not the redesign's permanent ceiling.
Measure retained allocations, process memory and GPU memory separately. Preserve
frame work limits, cancellation and graceful coarse coverage under pressure.

**Repository audit completed 2026-10-05; amended proposal, implementation still
paused.** The audit below separates existing mechanisms from proposed ones. The
512 MiB experiment and inherited renderer limits are not permanent policy inputs.
No source implementation or new runtime validation accompanies this amendment.

## What current evidence establishes

Baseline: `main`, HEAD `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`, dirty working
tree including earlier developer-interface work and the crater experiment.

- **OBSERVED:** the procedural crater experiment produces smooth circular bowls
  and largely uniform surrounding terrain, unlike the supplied reference. More
  subdivision does not supply missing surface structure or materials.
- **IMPLEMENTED:** `CrateredV1` now preserves landmark heights across footprints.
  This corrects their former amplitude fade; it does not create realistic geology.
- **MEASURED:** `target/phase515/recovery-512/crater-regional.json`, seed 2,
  960×640, serial generation, 4,096 static updates, RX 9070 XT/Vulkan: rendered and
  desired radial level 11, approximately 4.84 m spacing from its paired console,
  2,046 source leaves, 1,778 visible leaves, 24,778,272 submitted upload bytes.
  Accounted retained terrain was 145,813,642 bytes; the console recorded peak
  aggregate reservations of 242,775,669 bytes. These are not process RSS or VRAM.
- **FAILED:** that capture remains quality pending and unsettled; matching radial
  LOD does not certify the whole view. It reports a resource constraint despite
  substantial RAM headroom. `terrain_population.rs` limits the complete terrain
  cover to 2,048 leaves; 2,046 leaves leave no room for the next split. This
  separate work/topology limit is a concrete constraint, not RAM exhaustion.
- **MEASURED, limited scope:** its single captured frame reports 48.923 ms terrain
  preparation and 88.151 ms total CPU frame work. These are offscreen diagnostic
  observations, not native FPS statistics or a steady-state benchmark. Zero GPU
  timing in these captures is insufficient evidence of GPU cost.
- **IMPLEMENTED:** `planet_surface/gpu.rs::upload` writes supplied sample, instance
  and fallback arrays. Retaining buffer capacity is not keyed tile residency.
  `prepare.rs` prepares stitched/transition geometry on the CPU.
- **IMPLEMENTED:** `bounds.rs::horizon_reject` disables its smooth-sphere test for
  nonzero height extents. Setting an opaque radius alone cannot enable displaced
  terrain occlusion. A new conservative terrain culler needs its own proof.

Evidence paths are diagnostic artifacts, not completed visual acceptance. The
three-view recovery run finished while the user requested this plan; its orbit,
regional and rim pairs remain available. Implementation is now paused.

## Terrain must have structure at several scales

Generate recognizable shapes and their physical history, rather than adding
unrelated noise layers or stamping identical circles:

| Scale | Authoritative terrain and surface cues |
| --- | --- |
| Planet | Large impact basins, highland regions, broad resurfaced plains; distinct regional material coverage visible from orbit. |
| Regional | Varied crater sizes and ages, asymmetric walls, broken rims, overlapping impacts, ejecta and ridged highlands. |
| Local | Smaller impacts, fractured slopes, regolith and exposed rock; sparse larger rocks eventually use separate geometry. |
| Below a pixel | Filtered height-derived normals, roughness and material variation that preserve the larger surface's appearance. |

Use seeded body-local spatial cells and feature hierarchies. Query features near
the requested area, including support halos across cube-face boundaries; never
scan every crater on a planet or construct every planet in the universe. Compose
overlapping impacts with explicit age/overprinting rules and bounded local height,
rather than an unlimited sum of crater depths. Keep repeatability and canonical
shared directions. Avoid baking arbitrary shader craters that disagree with terrain.

Fine relief may use shading when smaller than a pixel, but resolve into the same
height field when approached. Normals, material boundaries and displacement must
remain correlated. Preserve a route for sparse surface edits and later local
volumes; a height-field renderer does not itself implement caves or excavation.

## Proposed representation and ownership

1. **World:** versioned immutable terrain definitions, deterministic region queries,
   material classification and edit revisions. A CPU reference sampler remains the
   authority/oracle for collision, navigation and numerical validation. These
   queries evaluate the complete unfiltered field. Footprint-specific rendering
   is a derived approximation with error against that field; it does not redefine
   the surface used by navigation or collision.
2. **Derived tile builder:** build height, slope/normal and material tiles plus
   conservative min/max and parent-reconstruction error. Tile identity includes
   body, generator/configuration version, seed, edit revision, chart and level.
   Initially use CPU workers; move suitable tile construction to compute only after
   profiling and validating parity. No mandatory GPU readback for each frame.
3. **GPU tile cache:** retain tiles in texture arrays or a paged atlas with explicit
   residency and eviction. Upload changed tiles only. Coarse body coverage stays
   resident while detail streams. Child readiness includes its parent, borders and
   required material data; missing children draw the coherent parent.
4. **Geometry:** instance one reusable regular patch grid over a cube-sphere
   quadtree. Displace it from cached height tiles. Keep balanced adjacency and
   canonical border samples; choose grid density through matched measurements.
   Do not rebuild or upload complete per-vertex meshes each frame.
5. **Transitions:** reconstruct the actual parent triangle surface and blend child
   displacement toward it on the GPU. Blending the two analytic heights alone does
   not guarantee matching geometry. Synchronize geometry, normals and materials;
   remove CPU common-refinement overlays only after watertight transition evidence.
6. **Selection:** begin with a CPU metadata quadtree and compact instance list.
   Select from projected geometric error and visible material detail, with
   hysteresis, locality and parent-first request priorities. Profile before moving
   selection to GPU indirect draws; authoritative/control work stays independent.

### Reuse contract for the fixed-resolution visual prototype

Temporary rendering infrastructure is permitted; a temporary terrain generator is
not. Slice 1 must implement its versioned procedural definition, spatial feature
hierarchy, deterministic complete sampling, material classification and coordinate
conventions upstream in the existing world terrain boundary. Both the temporary
renderer and later tile builder consume that same definition/query implementation.
Feature support halos and age/overprinting rules are part of the definition, never
patch-, camera-, worker- or GPU-slot-dependent. New numerical terrain semantics
require a new generator version; do not silently reinterpret V1, V2 or CrateredV1.

Record representative complete height/gradient/material queries, canonical edge
and corner queries, seeds, radii and definition identities during Slice 1. Slice 2
must reuse them unchanged. Rendering filters, quantization, normal reconstruction
and parent error certificates are separately versioned derived representations.
The CPU oracle remains available for navigation and future collision; current
radial clearance queries are not a complete collision/walking system. Existing
material presentation is renderer-owned elevation/slope classification, so the new
authoritative material field requires an explicit world query extension.

Renderer inputs remain domain-free through the app adapter: no renderer dependency
on world, no generator hidden inside a shader, and no authoritative GPU readback
requirement. Add derived cache namespaces for material and tile-format revisions,
and asynchronous request epochs, without treating runtime FrameId as terrain salt.

The replacement deliberately supersedes the frozen grid16/CPU preparation and
common-refinement rendering choices in Phase 4/5 and ADR 0006 **for the new terrain
path only**. Retain their ownership, precision, balanced adjacency, error budgets,
single opaque-owner handoff and transactional readiness contracts. A focused
implementation contract must record that decision before changing code; the old
documents are historical baseline contracts, not evidence that tiles already exist.

A cube-sphere tile quadtree is the recommended first replacement because it serves
both orbital coverage and local views with one representation. A local geometry
clipmap is a later alternative if measurements justify a second surface regime;
introducing two regimes immediately would add handoff and seam problems.

Use checked f64 patch anchors and observer-relative data. GPU patch evaluation
must avoid subtracting large nearly equal planet coordinates in f32. Verify local
reconstruction against the CPU reference at small and large radii, face edges,
corners, extreme views and moving bodies before accepting displacement shaders.

## Detail, lighting and culling

Geometry LOD and shading resolution need separate controls. Geometry tracks
silhouette, depth and parallax; cached normal/material tiles retain smaller surface
cues. Coarse levels should preserve filtered relief statistics, not turn rough
terrain into a featureless grey sphere. Bound geometric residuals against the
actual parent reconstruction; material filtering also needs aliasing and temporal
tests. Sampled extrema alone are not conservative certificates.

Use body-fixed mineral/regolith variation, roughness and height-derived normals,
with directional sunlight and terrain shadows. Begin with restrained physically
plausible colours, plus a separate enhanced mineral diagnostic if desired; the
supplied image also contains strong colour variation. Tune shadow resolution to
visible terrain scale. Compare under matched illumination and camera positions.

Add terrain-aware horizon occlusion using conservative rendered patch bounds and
a certified opaque inner region. Existing smooth-sphere rejection cannot simply
be reused. Include stitch/morph and GPU precision margins, and keep patches visible
when occlusion cannot be proved. The occlusion proof gate must enclose stitched
vertices, morph transitions, triangle interiors and observer-relative GPU precision;
this culler is unimplemented until that gate passes. Frustum culling and prioritization should reduce
unseen refinement without discarding the complete coarse body cover.

## Implementation order and gates

| Slice | Scope | Evidence required before proceeding |
| --- | --- | --- |
| 1. Convincing surface sample | Ancient cratered airless family: seeded basin/highland/crater structure and correlated materials. Use a temporary fixed-resolution reference render. | User-reviewed convincing orbit/regional/near views against the supplied morphology target before LOD tuning; repeat across several seeds. Separate terrain, normal, material and lighting diagnostics. |
| 2. Resident GPU patch prototype | One region, parent/child tiles, reusable grid, checked displacement and GPU parent reconstruction. | CPU/GPU reconstruction error, border/corner continuity, transition fly-through, resource/cancellation tests; unchanged terrain tile uploads become zero after warm-up. |
| 3. Whole body and streaming | Cube-sphere cover, balanced selection, missing-tile fallback, priorities and terrain-aware occlusion. | Orbit-to-ground route, face crossings, rapid approach/retreat and teleport; no holes, feature disappearance or shading/geometry mismatch. Report whole-view quality and cold convergence. |
| 4. Real-time performance | Profile and optimize tile construction, selection, shadowing and residency; optional GPU compute/indirect work follows evidence. | Native fixed-resolution runs on the user's machine: CPU/GPU p50/p95/p99, upload bytes, cold/warm convergence, RSS/VRAM and movement hitches. Compare matched visual quality; report unlocked FPS rather than declaring 60 FPS sufficient. |
| 5. Integration | Multiple differently seeded bodies, moving/rotating frames, cache switching and edit invalidation. | Stable generation, no stale tiles, no body/material identity leakage, consistent terrain queries and bounded pressure behaviour. |

Identity, definition/radius edits, late-result rejection, material invalidation and
submission-safe slot reuse are prerequisites of Slice 2, tested on its small
prototype; Slice 5 broadens that coverage. Sparse edit persistence and volumetric
editing remain future domain work, and cannot be claimed from today's whole-body
TerrainRevision. Slice 1's fixed-resolution render must label its coverage, spacing,
quantization and approximation error; its orbit/near views need not pretend to be
one accepted streaming route. It must not conceal missing detail behind lighting.

Zero unchanged uploads means **terrain tile content** after warm-up, with a stable
definition and view; small observer/instance/lighting updates are measured separately.
Slice 2 starts with texture-array or atlas residency and ordinary instancing.
Capability inventory and negotiated limits precede optional indirect, bindless or
experimental shader paths. An unavailable capability must retain a correct baseline.

Each slice gets a focused implementation contract and independent review. Keep
the old path as a comparison during migration, then remove it after the replacement
passes its checks. Preserve the dirty developer-interface work. Do not combine this
with procedural orbital-system assembly, camera redesign, oceans, clouds or local
volumetric terrain in one implementation task.

Visual quality and real-time cost are both gates. Agree numeric frame and
convergence targets from the first representative prototype; do not silently
weaken quality thresholds or equate a passing test suite with the user's approval.

## Technique reference

[Asirvatham and Hoppe, GPU-based geometry clipmaps](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry)
supports the reusable-grid, cached-height/normal and shader-transition direction.
The spherical tile quadtree, ownership, precision and staged migration above are
project-specific proposals; that reference is not proof of their implementation,
visual quality or performance in Mundaris.

## Repository-context audit — 2026-10-05

### 1. Files read and evidence scope

The inventory at the end lists every relevant file inspected, including focused
sections/searches rather than implying full-file review. Audit baseline is
`main` at `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`, with pre-existing
dirty/untracked terrain and developer-interface work. Only this proposal is
amended. No builds, tests, live device probes or new performance runs were made.

**VERIFIED:** the Phase 5.14 fast-check manifest lists 212 files; 13 tracked input
files now differ, and newer unlisted crater files exist. Its passing 13-command
Windows matrix is historical validation, not validation of today's crater code.
The recovery-512 JSON/console lack source/executable fingerprints, so they are
identified retained fixtures, not a verified current-binary measurement.

### 2. Current terrain pipeline — source, not the historical diagrams

1. **Definition:** `CelestialBody` holds `TerrainDefinition` and
   `TerrainRevision`; `CelestialSystem::edit_terrain` transactionally changes
   whole-body terrain independently of celestial motion. App
   `solar_system::terrain_definition/cratered_terrain_definition` supplies the
   immutable recipe. `TerrainGenerator::new/compile` builds the world sampler
   from version, identity, seed, configuration and reference radius.
2. **Sampling:** `TerrainGenerator::evaluate_point/evaluate_batch` consumes
   body-fixed `SurfaceLocation` and `TerrainFootprint`. Noise/erosion and compact
   crater profiles share that sampler. Complete queries use footprint zero;
   render patches use `R * 2 / (16 * 2^level)`. Crater landmarks currently keep
   complete amplitude; background bands remain filtered. `TerrainSample::normal_body`
   derives the normal from the same height gradient.
3. **Population and LOD:** `GravityOrbitsDemo::render_host` calls
   `TerrainPopulation::update`, which admits one body's terrain using projected
   far error and maintains far/surface ownership through `PlanetSurfaceSession`.
   `AdaptiveTerrainCover::update_with_elapsed` adapts world certificates through
   `TerrainSelectionPolicy` to `SurfaceLodSession::update_with_policy`.
   The selector prioritizes relevant radial/high-error ready closures, maintaining
   six-face coverage and 2:1 edge balance; missing geometry retains its source.
4. **Generation and publication:** `TerrainPatchCache::request_pinned/generate`
   and `generate_workers`, with `TerrainWorkers` calculations, create 289-sample
   `GeneratedSurfacePatch` grids using canonical
   `CubePatchAddress::sample_direction/sample_key`. The shared
   `terrain_surface_certificate/certificate_for_samples` supplies conservative
   extent and interpolation/unresolved/boundary/numeric allowances.
5. **Stitches/transitions:** `StitchedSurface::build_reusing` establishes
   coarsest-incident boundary ownership over the complete cover, including invisible
   neighbors. Sixteen shared index variants collapse odd fine edge samples.
   `SurfaceTransition::build_cancellable` constructs exact old/new triangle
   common refinements; transition vertices evaluate endpoints with `sample(fraction)`.
   Ready construction is published atomically, with retained source and successor.
6. **CPU preparation:** `CelestialFrame` appends terrain through
   `SurfaceStaging::append_stitched/append_transition`. `PreparedView::prepare_source`
   and `PreparedRenderFrame::view_displacement` subtract the observer in body
   coordinates in f64 before rotation/narrowing. Classification, precision proofs,
   morph interpolation, clipping, packing and mask grouping occur on the CPU.
7. **Upload and draw:** `PlanetSurfaceRenderer::upload` writes sample48,
   instance64, fallback80 arrays and the 64-byte lighting uniform; capacity persists,
   keyed content residency does not. `draw` issues up to sixteen indexed instanced
   draws plus a non-indexed clipped/transition draw in `CelestialRenderer::draw`'s
   reverse-Z pass. `planet_surface.wgsl::vs_main/vs_clipped` projects prepared data;
   it does not generate or displace terrain from resident height tiles.

### 3. Existing constraints

| Category | Current constraint and significance |
| --- | --- |
| Topology/count, not memory | Production visible and complete covers: 2,048 each; raw cache: 4,096 entries; stitch builder: 4,096 patches; address level: 0–30. General selector defaults allow 4,096 visible/65,536 cover leaves, but population overrides them. A split adds three leaves before balancing, so 2,046 cannot admit another split. |
| Grid/metadata counts | Grid16 = 16×16 cells/289 samples; sixteen stitch masks; metadata session 6–4,096 records with a 1 MiB capacity check; certificate ring 256 entries. These bound work/cache capacity, not terrain RAM usage directly. |
| CPU byte admission | 512 MiB aggregate accounted quota, raw/generated allocations plus external reservations; 64 MiB outgoing staging and 8 MiB boundary/proof caps remain separately enforced. Capacity/Arc lifetime and reservations are counted conservatively; unique allocation census and allocator overhead are not established. |
| Other CPU reservations | App stages 72 MiB + 32 KiB, selector scratch 8 MiB plus temporary address vectors, transition reservation 16→24→32 MiB, serial workspace 64 KiB; each worker includes 512 KiB stack + 128 KiB fixed allowance and definition-dependent patch/query workspace. Soft cache headroom defaults to zero. |
| GPU/resource allocation | 80 MiB aggregate surface buffers + topology/lighting; growth rounded to 4 KiB and waits for earlier submissions before replacement. Default device storage binding 128 MiB/buffer 256 MiB are per-resource validation limits, not free VRAM. Depth, sky, UI, driver allocations and upload staging are outside that 80 MiB payload accounting. |
| Per-frame work and queues | Native asks for 64 generation vertices and a 2 ms cutoff; serial microbatches are 8, accepted batch maximum 64. Checks between batches can overshoot; the worker path bounds scheduling, not total background completion time or whole-frame preparation. Metadata work is 32 divided among registered sessions. Raw requests cap at 256; four native worker slots (configurable serial or 1–4), one job/result channel element per slot. |
| Transition/convergence | One admitted replacement per policy update; topology freezes during active morph, with at most one constructed successor. Ordinary 150 ms morph may shorten toward a 64 ms floor when projected displacement is below 8 px. Construction and transition byte limits can defer quality with coherent coverage retained. |
| Upload budget | There is no terrain bytes-per-frame streaming allowance or unchanged-content dirty check: every prepared nonempty payload is rewritten. Staging/GPU allocation caps limit capacity; they are not upload-throughput budgets. |
| Cancellation | Obsolete requests/builders and old-body work cancel; job flags and cancellable overlay loops interrupt work; stale completion is rejected and retained allocations remain charged while referenced. Submission-safe GPU tile eviction/request epochs still need implementation. |
| Culling/visibility | Five-plane frustum bounds and conservative patch balls; no displaced-terrain horizon proof. `horizon_reject` returns false for nonzero height extents, and current certificates supply opaque radius zero. Invisible incident patches still participate in a complete cover/stitch ownership; only relevant visible geometry is prepared. Focus/selection does not equal admitted terrain body. |
| Quality/precision | Split 0.125 px/merge 0.0625 px, conservative error terms and 0.05 px narrowing/projection checks; component budgets 10 µm within 100 m, 0.1 mm within 1 km, 1 mm within 10 km. Uncertain geometry uses clipped fallback. The generic near-debug 10 km budget is not a planetary draw cutoff. |
| Generator envelope | Finite validated inputs, radius ≤1e8 m, relief bound ≤0.1R and positive inner radius; five bands, 1–4 octaves, physical shortest wavelengths ≥8 m, angular scale only in macro; noise lattice magnitude <2,147,483,646. CrateredV1 has ≤128 features, minimum feature radius 64 m, depth fraction ≤0.1/rim ≤0.05; the app recipe caps largest radius at min(0.13R,24 km). These are current algorithm/content restrictions, not RAM policies or permanent planetary requirements. |

### 4. Reusable architecture

**Already compatible:** checked frame tree/LCA conversion, coherent
`CelestialFrameProjection`, translating versus body-fixed roles, `BodyId`
versus `FrameId`, cube-face conventions/canonical dyadic addressing/tangent bases,
one deterministic terrain sampler, terrain definition equality/version/seed/radius
keys, independent terrain revision, transactional world edits, single opaque-owner
handoff and reverse-Z presentation.

Keep `CelestialCamera` and its one f64 observer/attachment/navigation policies.
`terrain_inspection::terrain_clearance/clearance_at_position` queries complete
truth; `surface_probe::ready_mesh_probe` instead intersects drawn triangles and
needs a new representation adapter. Neither is a complete collision system.

Keep developer UI controls, typed commands, leases, shared native/offscreen
`FrameHost`, immutable paired capture, scenario runner and freshness/frame
attribution. Extend diagnostics additively for residency, allocation classes,
capabilities, upload backlogs and explicit constraint reasons. Current schema's
`budget_constrained` conflates byte/count/work constraints; radial equality and an
empty queue cannot establish whole-view quality.

### 5. Systems superseded

Replace the new path's raw body-fixed per-vertex grid cache with derived height/
normal/material tiles; retain upstream query semantics. Replace full-cover copied
stitch geometry, exact-overlay construction/display, CPU per-frame morph/conversion/
packing and unconditional sample/fallback uploads with resident tile/grid draw
data and validated GPU reconstruction. Adapt geometry readiness, raw-request
orchestration and metadata certificates to tile dependency readiness.

Reuse selection concepts and topology mathematics, but revise flat-cover scans,
hard leaf limits, serialized closure/morph scheduling and byte admission where
measurements require it. A GPU cache alone leaves those bottlenecks intact.
Keep the old renderer as a comparison bridge, then remove its superseded terrain
path after parity/acceptance; do not duplicate authoritative generation.

### 6. Compatibility of every major proposal

| Proposal | Classification | Audit consequence |
| --- | --- | --- |
| Airless vertical slice, varied seeds/sizes, existing Solar fixture | Already compatible | App recipes and explicit body identity allow this without orbital/camera redesign. |
| Convincing spatial feature hierarchy and age/overprinting | Requires extension | Current bounded crater catalogue is not the proposed locality-aware multiscale hierarchy; use a new world generator version. |
| CPU complete sampler as oracle | Already compatible | Preserve COMPLETE semantics and deterministic target-specific arithmetic; collision system itself is future work. |
| Authoritative material classification, correlated normals/roughness | Requires extension | Current world sample returns height/gradient; renderer derives elevation/slope weights and palette. |
| Sparse edits, region invalidation, future volumes | Requires extension | Only whole-definition edits/TerrainRevision exist; do not claim regional edits/persistence or caves implemented. |
| Tiles, parent-reconstruction errors and certified extrema | Requires extension | Existing certificates are useful methodology; grid16 bounds are not certificates for new filters, texture formats or parent triangles. |
| Resident GPU tiles/changed-only uploads | Requires replacement | Current GPU path is persistent-capacity, rewritten-content storage. |
| Instanced displaced regular grids | Requires replacement | Instancing/index masks exist; shader displacement and residency layout do not. |
| GPU geometric/material transitions | Requires replacement | Exact CPU overlays establish today's continuity; new topology/diagonals/edge pins and transition endpoints need independent proof. |
| CPU metadata selection, hysteresis/locality/parent-first readiness | Requires extension | Reuse policy concepts; material error, tile dependencies, request epochs and scalable work limits are missing. |
| f64 anchors, observer-relative GPU reconstruction | Requires extension | Retain frame boundary; prove shader normalization/curvature/quantization arithmetic independently. |
| Terrain-aware horizon occlusion | Requires extension | New certified rendered occluder, stitch/morph/interior/precision coverage proof; conservative visible fallback. |
| Directional sunlight and terrain shadows | Requires extension | Body-fixed sun/lighting exist; shadow infrastructure is a separate measured addition. |
| Optional compute/indirect optimization | Insufficient evidence | No current end-to-end comparison or live advanced-feature inventory. |
| Optional local clipmap as second regime | Insufficient evidence | No workload establishes need; defer its handoff/seam complexity. |
| Replacing frozen Phase 4/5 grid/CPU-overlay choices | Conflicts with existing architecture contracts | Intentional new-path supersession must be explicit; ownership/precision contracts remain. |
| Staged migration and preserved developer interface | Already compatible | Add renderer adapters/diagnostics; preserve typed control and paired evidence semantics. |
| Native performance/visual/convergence acceptance | Insufficient evidence | Retained serial captures and historical profiles justify experiments, not new-path acceptance. |

### 7. Modern GPU path

Locked dependencies are wgpu/wgpu-types 27.0.1, wgpu-core 27.0.3 and wgpu-hal 27.0.4.
Mundaris requests supported timestamp features, default device limits and disabled
experimental features. Vulkan in a capture identifies a backend, not direct Vulkan
extension access or support for every feature below.

| Technique | Current exposure and realistic use | Required justification |
| --- | --- | --- |
| Indirect drawing | Single draw/indexed-indirect APIs require an indirect argument buffer and `DownlevelFlags::INDIRECT_EXECUTION`; optional multi-draw/count and nonzero-first-instance capabilities are separate. Current terrain uses direct mask-batched instancing. GPU culling could produce arguments. | CPU selection/encode cost versus compute+indirect total at identical visible work; sixteen current buckets alone are weak motivation. |
| Draw-indirect-count | `MULTI_DRAW_INDIRECT_COUNT` exists, unrequested; permits GPU count-buffer draws without count readback. | Show useful variable GPU-produced lists and lower end-to-end cost, not just fewer API calls. |
| Mesh/task shaders | Experimental Vulkan `VK_EXT_mesh_shader` path exists, unrequested, zero default mesh limits. Enabling the locked API's experimental token requires unsafe code; the current workspace `forbid(unsafe_code)` policy would need an explicitly approved narrow policy boundary or exception, not review alone. | Equal-quality indexed-versus-mesh GPU/CPU cost, primitive/vertex bottleneck, seams/precision and supported-adapter fallback; not a Slice 2 prerequisite. |
| Bindless/descriptor indexing | Binding-array/non-uniform/partially-bound flags exist, unrequested; default binding-array limits are zero. Layered texture arrays are a simpler baseline and do not require bindless descriptors. | Material binding/switch overhead or array-capacity bottleneck, matched against ordinary atlas/array instancing. |
| Compute terrain generation | Compute API/capability exists; no terrain compute dispatch today. Derived tile normals/filtering or bounded sampling may parallelize well. | CPU generation/transfer costs versus GPU dispatch/queue occupancy, cold latency and CPU oracle parity including seed hashing, complete/filtered field and precision. |
| Sparse resources | No public sparse-resource API found in the locked stack. Software-paged atlas residency is feasible without native sparse allocation. | Demonstrate residency/volume pressure dense paging cannot handle before considering a backend change or narrow native boundary. |
| Memory-budget queries | wgpu-hal Vulkan may use `VK_EXT_memory_budget` internally; public `MemoryBudgetThresholds` configure OOM/device-loss guards, not app-visible live usage. No current RSS/VRAM budget diagnostic. | Target-adapter/platform budget telemetry, allocation/headroom traces and pressure tests before residency policy depends on it; retain explicit configured fallback if unavailable. |

The [wgpu 27 release](https://github.com/gfx-rs/wgpu/releases/tag/v27.0.0) and
[versioned render-pass APIs](https://github.com/gfx-rs/wgpu/blob/v27.0.1/wgpu/src/api/render_pass.rs)
are API references, not Mundaris device measurements. The locked local sources
were also checked; newer documentation does not override them.

### 8. Memory-policy audit and amendments

**Origin:** the original Phase 5 §15 specifies a 128 MiB aggregate CPU cache/
stitch/morph/worker/staging budget; earliest located code is commit `a80ddc7`.
No measured hardware-capacity derivation was found. Phase 5.10/5.11 retained it
as a phase constraint. Current dirty Phase 5.15 code raises it to 512 MiB after
the user's 2026-10-05 retirement decision, not after a demonstrated optimal-budget
study. Phase 4's resource table specifies CPU64/boundary8/selector8/metadata1/GPU80
MiB; CPU64/GPU80 occur in `2adb120`, boundary8 by `6216cdc`. Their origin is
baseline resource preflight policy, not measured free RAM/VRAM.

The 4,096-entry/256-request limits originate in the initial bounded terrain cache;
2,048 population leaves are an independent later work/topology admission policy.
Worker stack/fixed allowances, 64 KiB query allowance, 72 MiB staging reservation
and 16–32 MiB transition ladder are explicit conservative implementation
reservations. Generator catalogue/query-workspace charges were added by the dirty
crater work. None is a replacement machine-resource policy.

**MEASURED, retained scope:** 5.11E descents reach exactly 134,217,728 accounted
bytes; the final cold fixture leaves 1,848 bytes. Its 10 km gameplay Earth fixture
(1152×768, 800 serial updates, no morph, 575 visible leaves, dirty `d81bb2a`
checkpoint) uploads 8,013,264 B including lighting on unchanged repeats; 20
planetary preparation repeats have median 13.9391 ms. These do not prove present
native FPS, unique allocations or driver VRAM. Recovery-512 seed-2 Moon regional
(960×640, 4,096 static serial updates) has 2,046 leaves, retained accounted
145,813,642 B, console peak aggregate 242,775,669 B, upload 24,778,272 B and one-frame
terrain preparation 48.9228 ms. The retained orbit/regional/rim images and historical
cold image are **OBSERVED**; all remain quality pending/unsettled. Zero reported
GPU timestamps in the newer captures do not establish zero GPU terrain cost.

Adopt four **separate observable policies**, with no unexplained new fixed ceiling:

| Policy | Admission/pressure behavior | Required observations |
| --- | --- | --- |
| CPU terrain cache | Configurable target derived from available host memory, nonterrain use and measured working set; unique retained allocation accounting plus operational headroom. Evict unpinned detail with hysteresis; never destroy authoritative definitions/edits. Hard emergency guards remain explicit configuration with reason and provenance. | Target/source, actual class capacities, unique/shared ownership, pinned/evictable/reserved bytes, hits/misses/evictions, RSS/host pressure, admission reason and pending quality. |
| GPU residency | Configurable payload target based on negotiated texture/layer/buffer limits and measured device budget/headroom where available; explicitly account other renderer consumers. Keep old/new/in-flight allocations and delayed frees inside policy. Use array/atlas paging and reuse hysteresis. | Committed payload versus allocated/estimated driver memory, budget-source availability, pinned/evictable/in-flight bytes, allocation/growth/fence waits and evictions/reuploads. |
| Transient generation/upload | Separate reservations for bounded job workspace, border/mip scratch, pending CPU results and staging copies; bounded bytes/jobs/time per opportunity and bytes/fence backlog for uploads. Backpressure/cancel obsolete detail, prioritize required parents and borders. | Reserved/live/peak bytes per class, queue age, completed-unpublished bytes, upload bytes/time/backlog, cancellation waste, wait time and reason. Do not count separate class maxima as one simultaneous peak. |
| Complete coarse-body coverage | Calculate and preflight minimum complete six-face fallback (tiles, borders, parent/material dependencies, indices and descriptors) per admitted body before detail. Pin it while that body owns surface draw; budget concurrent handoff minimums explicitly, then reduce optional detail. Other bodies retain valid far representations. | Required/admitted coverage bytes, roots/resident dependencies, opaque-owner count, missing coverage and fallback error. Inability to establish coverage is an explicit admission failure, never holes or a silently accepted lower quality target. |

Count/work safety limits remain independent of byte policy. Derive them from
measured selector/draw/generation capacity and negotiated device limits; expose
which bound prevents quality. The current 2,048 value must not simply become a
larger unexplained constant, nor must larger RAM masquerade as scalable selection.

### 9. Implementation risks and required prevention

- **Duplicate generators:** prototype-only craters/noise/material shaders would
  fork truth; reuse the Slice 1 world definition/hierarchy/query unchanged.
- **Authoritative changes:** filtering, smoothing and GPU approximation must not
  alter COMPLETE navigation/collision queries; new geology needs explicit versioning.
- **Seams:** independent face seeds, inconsistent feature halos, gutters/mips,
  texture filtering, corner rotations, coarse diagonals and adjacent transition
  phases can disagree; test canonical edges/corners and actual parent triangles.
- **CPU/GPU disagreement:** hash/floating order, quantization, normals and material
  filtering need bounded comparisons to the oracle, including full versus rendered
  surface residuals; bit-identical GPU arithmetic is not a current guarantee.
- **Thrashing:** child-first requests, short residency reuse, body-switch oscillation,
  cancelled output publication and upload overproduction need priorities, hysteresis,
  request epochs, backlog/backpressure and explicit cancellation-waste telemetry.
- **Precision:** f32 absolute sphere subtraction/normalization can lose local relief;
  validate reconstruction at large radii and frame offsets during motion/rotation.
- **Edits:** whole-definition revision is not sparse edit persistence; future edit
  dependencies must invalidate parents, borders, normals/materials and collision
  without rewriting intent or silently redefining saved coordinates.
- **Stale content:** include system/body association, definition/version/seed/config,
  radius, terrain/edit revision, material revision, tile format and request epoch.
  Slot generations and submission completion protect evicted/reused GPU slots.
  Names, camera pose, illumination and runtime FrameId are not generation salt.
- **Coverage/proofs:** sampled extrema and smooth-sphere occlusion can cut displaced
  or morphing terrain; retain conservative visible/parent fallback until certified.
- **Migration evidence:** changed morphology/covers/work limits cannot establish a
  renderer speedup; compare both renderers using the same approved definition and
  matched quality, separating historical baseline, native timings and user approval.

### 10. Recommendation

**Amend, then proceed to a separately scoped Slice 1 contract; do not implement
the renderer yet.** Resident height/material tiles and reusable displaced grids
are well motivated by existing restaging/upload mechanisms and retained preparation
costs. The chosen tile size, density, filtering, GPU arithmetic and memory policy
remain hypotheses to measure. The reuse contract, explicit historical-contract
supersession, four memory policies, early invalidation/precision gates and feature
measurement conditions above are required amendments.

Also record rather than silently fix documentation drift: architecture still calls
Phase 5 unimplemented; the interface guide still mentions a 128 MiB warning cap
although source uses the configurable cap value; the crater phase specifies rim
fraction 0.020 while current recipe/report use 0.040. Source wins for this audit.
Those files are preserved, and no old visual/Acceptance A gate is closed here.

### Inspected-file inventory

Tests listed here were inspected/inventoried, not run. Git status/diffs and focused
historical `git show` reads of `a80ddc7`'s terrain cache, `2adb120`'s renderer
staging/GPU source and `6216cdc`'s boundary policy supplemented the named files.

| File | Why it matters |
| --- | --- |
| `AGENTS.md` | Defines audit authority, evidence rules, preservation and delegation constraints. |
| `README.md` | Records current terrain routes and the required locked validation commands. |
| `Cargo.toml` | Fixes workspace ownership, wgpu major version and unsafe-code prohibition. |
| `Cargo.lock` | Identifies the exact graphics dependency versions under review. |
| `crates/app/Cargo.toml` | Defines capture, profiling and developer-tools feature integration. |
| `crates/renderer/Cargo.toml` | Confirms renderer dependencies and feature boundaries. |
| `.github/workflows/ci.yml` | Defines headless CI checks and their platform scope. |
| `.codex/config.toml` | Records project orchestration and developer-interface configuration. |
| `.codex/agents/luna-explore.toml` | Defines read-only discovery boundaries. |
| `.codex/agents/luna-review.toml` | Defines independent read-only invariant review boundaries. |
| `docs/codex-workflow.md` | Specifies repository audit/delegation and subscription verification workflow. |
| `docs/REVIEWER_CONTEXT.md` | Indexes dated terrain/tooling blockers without supplying acceptance authority. |
| `docs/architecture.md` | Defines world/app/renderer ownership and records the dirty crater extension. |
| `docs/engine-invariants.md` | Protects precision, procedural authority, independent layers and sparse-edit direction. |
| `docs/coding-standards.md` | Defines safe Rust and explicit numerical/API conventions. |
| `docs/REVIEW_HANDOFF.md` | Requires revision, raw evidence and honest partial acceptance records. |
| `docs/REVIEW_CHECKLIST.md` | Defines relevant architecture, performance, visual and observability review criteria. |
| `docs/ENGINE_MECHANICS_REFERENCE.md` | Maps historical terrain mechanisms and exposes stale snapshot assumptions. |
| `MUNDARIS_ENGINE_DESIGN.md` | Establishes long-term planetary scale, local edits, GPU and streaming goals. |
| `MUNDARIS_PHASE_4_PLANET_SURFACE_REPRESENTATION_AND_LOD.md` | Supplies the inherited grid/topology/precision and renderer resource contracts. |
| `MUNDARIS_PHASE_5_PROCEDURAL_TERRAIN_GENERATION.md` | Defines deterministic terrain authority, footprints, certificates and original CPU quota. |
| `MUNDARIS_PHASE_5_10_LOD_CONVERGENCE_AND_MORPHOLOGY_RECOVERY.md` | Explains inherited aggregate-cap and useful-convergence acceptance constraints. |
| `MUNDARIS_PHASE_5_14_AI_ENGINE_DEVELOPMENT_INTERFACE.md` | Defines the existing developer-control/capture boundary to preserve. |
| `MUNDARIS_PHASE_5_15_PROCEDURAL_CRATER_TERRAIN_FOUNDATION.md` | Records the experiment's intent, 512 MiB decision and stale rim-height requirement. |
| `docs/PLANET_TERRAIN_RENDERING_REDESIGN.md` | Is the proposal audited and the only document amended. |
| `docs/PHASE_5_11D_TERRAIN_PIPELINE_REPORT.md` | Indexes historical CPU/worker/upload instrumentation and headroom limitations. |
| `docs/PHASE_5_11E_TERRAIN_RECOVERY_REPORT.md` | Records partial sharing/recovery, repeated uploads and unmet acceptance. |
| `docs/PHASE_5_14_AI_ENGINE_DEVELOPMENT_INTERFACE_REPORT.md` | Indexes the developer interface's implementation and historical validation limits. |
| `docs/PHASE_5_15_PROCEDURAL_CRATER_TERRAIN_FOUNDATION_REPORT.md` | Records rejected visuals, changed recipe and incomplete current validation. |
| `docs/AI_DEVELOPMENT_INTERFACE.md` | Defines snapshot meanings and paired-capture workflow while exposing stale cap prose. |
| `docs/performance.md` | Identifies historical preparation/resource measurements and their workload scope. |
| `docs/references/MODERN_RENDERING_OPTIMIZATION.md` | Provides measurement-first residency/transfer candidates rather than architecture authority. |
| `docs/adr/0001-foundation-stack.md` | Confirms portable wgpu and native application stack choices. |
| `docs/adr/0002-reference-frames-and-precision.md` | Preserves local-before-root subtraction and checked narrowing. |
| `docs/adr/0003-celestial-domain-and-time.md` | Separates body identity, world truth and disposable frame projection. |
| `docs/adr/0005-celestial-navigation-system-view-and-timewarp.md` | Protects one-observer navigation and physical-size independence. |
| `docs/adr/0006-planet-surface-topology-and-lod.md` | Defines canonical cube topology, complete cover and frozen CPU renderer choices. |
| `docs/adr/0007-prescribed-celestial-motion.md` | Preserves coherent moving/rotating bodies and terrain-independent motion binding. |
| `docs/adr/0008-development-session-interface.md` | Protects owner-thread control, capture lifetimes and session/frame attribution. |
| `scripts/ai-check.ps1` | Defines focused evidence generation and refusal to overwrite prior captures. |
| `scripts/validate.ps1` | Defines the full locked validation matrix separately from native acceptance. |
| `scenarios/developer/convergence-1-2-5s.json` | Shows that recorded checkpoints test mode and do not certify terrain convergence. |
| `crates/math/src/frames.rs` | Implements LCA-relative checked f64 coordinate conversion. |
| `crates/math/src/surface.rs` | Defines right-handed cube faces, canonical dyadic samples and level-30 addressing. |
| `crates/math/src/noise.rs` | Defines deterministic lattice hashing, analytic gradients and checked noise-domain bounds. |
| `crates/world/src/body.rs` | Separates BodyId, physical state, terrain definition and terrain revision. |
| `crates/world/src/system.rs` | Publishes whole-definition terrain edits transactionally and validates radius changes. |
| `crates/world/src/frame_projection.rs` | Publishes coherent translating/body-fixed frames without storing frame identity in terrain. |
| `crates/world/src/terrain/mod.rs` | Defines versions/configuration and legal algorithm/radius envelopes. |
| `crates/world/src/terrain/query.rs` | Defines COMPLETE footprints, height/gradient samples and analytic terrain normals. |
| `crates/world/src/terrain/generator.rs` | Implements scalar/batch deterministic sampling, filtering and regional certificates. |
| `crates/world/src/terrain/crater.rs` | Implements seeded bounded crater catalogues, support and analytic profiles. |
| `crates/world/src/terrain/erosion.rs` | Implements fixed-anchor stateless erosion features and derivative envelopes. |
| `crates/app/src/solar_system.rs` | Supplies current body recipes, material presentation inputs and actual crater rim fraction. |
| `crates/app/src/planet_terrain.rs` | Owns terrain cache identity, allocation accounting, requests and generation. |
| `crates/app/src/planet_terrain/adaptive.rs` | Owns ready publication, transition reservations, certificate reuse and convergence reporting. |
| `crates/app/src/planet_terrain/workers.rs` | Implements bounded worker jobs, reservations, cancellation and completion ordering. |
| `crates/app/src/planet_terrain/certificate.rs` | Adapts world certificates to conservative grid/interpolation errors. |
| `crates/app/src/terrain_population.rs` | Selects one terrain body and imposes production leaf/work-count limits. |
| `crates/app/src/planet_surface.rs` | Implements far/prewarm/surface handoff and body-fixed inspection anchors. |
| `crates/app/src/terrain_inspection.rs` | Queries complete terrain for radial navigation clearance. |
| `crates/app/src/surface_probe.rs` | Distinguishes intersections with drawn stitch/morph triangles from procedural truth. |
| `crates/app/src/celestial_camera.rs` | Maintains one f64 observer, body attachment and cached complete-terrain navigation queries. |
| `crates/app/src/gravity_orbits.rs` | Composes coherent frames, terrain update, render preparation and snapshots. |
| `crates/app/src/gravity_orbits/developer_ui.rs` | Displays terrain/work/memory state and preserves existing user controls. |
| `crates/app/src/gravity_orbits/developer.rs` | Maps typed development actions to production session and offscreen host operations. |
| `crates/app/src/gravity_orbits/frame_host.rs` | Shares production preparation across native and deterministic offscreen presentation. |
| `crates/app/src/gravity_orbits/visual_controls.rs` | Shares presentation controls between UI and developer commands. |
| `crates/app/src/developer_snapshot.rs` | Defines readiness, accounted memory and freshness/source-frame diagnostic semantics. |
| `crates/app/src/developer_protocol.rs` | Defines typed actions, lease rules and bounded diagnostic/control queues. |
| `crates/app/src/developer_service.rs` | Publishes observations and handles on-demand diagnostics/capture cancellation. |
| `crates/app/src/developer_bridge.rs` | Adapts CLI/MCP requests to the same application protocol. |
| `crates/app/src/developer_scenarios.rs` | Defines bounded scenario execution and owned replay without a second native engine. |
| `crates/app/src/developer_capture.rs` | Defines serial crater fixtures and paired snapshots using production terrain preparation. |
| `crates/renderer/src/lib.rs` | Negotiates adapter/device features and owns native render/UI presentation. |
| `crates/renderer/src/gpu_profile.rs` | Negotiates timestamp support and separates asynchronous GPU measurement scopes. |
| `crates/renderer/src/terrain_capture.rs` | Creates offscreen devices and captures production render work with same-frame readback. |
| `crates/renderer/src/native_capture.rs` | Bounds acquired-surface copy/readback and records submitted capture identity. |
| `crates/renderer/src/view.rs` | Implements observer-local f64 subtraction and checked renderer narrowing. |
| `crates/renderer/src/celestial_view.rs` | Defines reverse-Z projection and the physical content viewport. |
| `crates/renderer/src/celestial.rs` | Composes terrain and far-body draw ownership in the shared depth pass. |
| `crates/renderer/src/planet_surface/mod.rs` | Defines surface API exports, grid constants and representation layout. |
| `crates/renderer/src/planet_surface/topology.rs` | Defines reusable grid and sixteen balanced-edge index variants. |
| `crates/renderer/src/planet_surface/cover.rs` | Defines compact cover/address membership rather than permanent world topology. |
| `crates/renderer/src/planet_surface/cache.rs` | Implements bounded pinned metadata LRU capacity. |
| `crates/renderer/src/planet_surface/lod.rs` | Implements projected-error selection, hysteresis, ready closures and leaf limits. |
| `crates/renderer/src/planet_surface/bounds.rs` | Defines conservative extent/error/frustum tests and displaced-horizon rejection. |
| `crates/renderer/src/planet_surface/terrain_geometry.rs` | Validates disposable f64 sampled geometry and conservative error inputs. |
| `crates/renderer/src/planet_surface/stitching.rs` | Establishes canonical coarse-owned boundaries and bounded stitched construction. |
| `crates/renderer/src/planet_surface/transition.rs` | Constructs cancellable exact parent/child triangle overlays. |
| `crates/renderer/src/planet_surface/prepare.rs` | Implements CPU morph/conversion/proofs/clipping/packing and staging ceilings. |
| `crates/renderer/src/planet_surface/gpu.rs` | Implements allocation preflight, unconditional uploads and direct mask-batched draws. |
| `crates/renderer/src/planet_surface/lighting.rs` | Defines renderer-only modes, sunlight and elevation/slope presentation thresholds. |
| `crates/renderer/src/planet_surface/cpu_profile.rs` | Provides scheduled-thread versus elapsed CPU timing used by terrain preparation. |
| `crates/renderer/src/shaders/planet_surface.wgsl` | Consumes packed geometry and performs current terrain fragment presentation. |
| `crates/world/tests/terrain_generation.rs` | Inventories definition/edit/scalar-batch determinism coverage. |
| `crates/world/tests/terrain_craters.rs` | Inventories crater profiles, seed and certificate coverage. |
| `crates/world/tests/terrain_bounds.rs` | Inventories conservative world-terrain envelope coverage. |
| `crates/world/tests/terrain_profile_difference.rs` | Inventories filtered-versus-complete profile bounds. |
| `crates/math/tests/surface_topology.rs` | Inventories canonical face/edge/corner topology coverage. |
| `crates/app/tests/planet_terrain.rs` | Inventories cache identity, eviction and generation coverage. |
| `crates/app/tests/terrain_population.rs` | Inventories terrain-body admission and ownership handoff coverage. |
| `crates/app/tests/terrain_workers.rs` | Inventories worker determinism, reservations and cancellation coverage. |
| `crates/app/tests/terrain_crater_lod.rs` | Inventories the current crater-specific LOD regression coverage. |
| `crates/app/tests/developer_interface.rs` | Inventories snapshot schema, units, memory-cap and capture coverage. |
| `crates/renderer/tests/planet_surface_precision.rs` | Inventories precision proof coverage without executing it. |
| `crates/renderer/tests/planet_surface_lod.rs` | Inventories complete-cover, balance, hysteresis and bounded selection coverage. |
| `docs/evidence/phase511d/README.md` | Locates historical terrain pipeline evidence and revision caveats. |
| `docs/evidence/phase511e/README.md` | Locates recovery evidence and its unmet acceptance gates. |
| `docs/evidence/phase511d/final/descent-4/memory.csv` | Records actual historical accounted capacity/reservation peaks. |
| `docs/evidence/phase511d/final/descent-4/headroom.txt` | Records historical class headroom and capacity limits. |
| `docs/evidence/phase511d/final/descent-4/repository.txt` | Identifies historical dirty repository state. |
| `docs/evidence/phase511d/final/descent-repeat-4/memory.csv` | Checks that repeated historical descent still reaches the CPU quota. |
| `docs/evidence/phase511e/after-final/descent-4/memory.csv` | Records recovery's actual accounted peaks. |
| `docs/evidence/phase511e/after-final/descent-4/headroom.txt` | Records recovery's unrecovered operational headroom. |
| `docs/evidence/phase511e/after-final/descent-4/repository.txt` | Identifies the recovery descent's dirty source checkpoint. |
| `docs/evidence/phase511e/after-final/descent-4/probe-manifest.txt` | Defines Earth radius, camera, work scheduling and measurement limits. |
| `docs/evidence/phase511e/after-final/descent-4/frames.csv` | Contains raw per-opportunity stage times and terrain quality/memory state. |
| `docs/evidence/phase511e/after-final/descent-4/render-profiles.txt` | Contains raw preparation-stage scopes separate from GPU execution. |
| `docs/evidence/phase511e/after-final/descent-repeat-4/memory.csv` | Checks repeated recovery descent memory pressure. |
| `docs/evidence/phase511e/after-final/cold-4/memory.csv` | Records historical cold-route near-zero headroom. |
| `docs/evidence/phase511e/after-final/motion-4/memory.csv` | Records memory under observer motion rather than stationary reuse. |
| `docs/evidence/phase511e/after-final/switch-4/memory.csv` | Records memory during terrain body switches. |
| `docs/evidence/phase511e/after-final/source-sha256.csv` | Identifies historical build inputs without equating them to current source. |
| `docs/evidence/phase511e/after-final/executable-sha256.csv` | Identifies historical executables separately from source revision. |
| `docs/evidence/phase511e/after-final/views.log` | Records static repeated preparation/upload/GPU scopes and their sample counts. |
| `docs/evidence/phase511e/after-final/views/phase59-10000-manifest.txt` | Defines the exact static Earth fixture and its unsettled cover. |
| `docs/evidence/phase511e/after-final/cold-capture-4/2m-5000ms.bmp` | Shows the limited historical cold endpoint rather than accepted settled terrain. |
| `target/phase515/recovery-512-console.txt` | Records actual peak aggregate reservations and spacing/error diagnostics. |
| `target/phase515/recovery-512/crater-reference.json` | Defines seed, radius, feature catalogue count and serial fixture limitations. |
| `target/phase515/recovery-512/crater-orbit.json` | Records orbital rendered/desired quality, morph and resource state. |
| `target/phase515/recovery-512/crater-orbit.png` | Shows rejected-scale orbital morphology in its paired recorded state. |
| `target/phase515/recovery-512/crater-regional.json` | Records the separate leaf-count bottleneck, byte headroom and frame timings. |
| `target/phase515/recovery-512/crater-regional.png` | Shows the regional crater's visibly smooth/artificial presentation. |
| `target/phase515/recovery-512/crater-rim.json` | Records rim-view unmet target and substantial accounted memory headroom. |
| `target/phase515/recovery-512/crater-rim.png` | Shows near-region relief without implying settled quality. |
| `target/phase514/final-validation/validation.json` | Confirms the historical thirteen-command Windows matrix's recorded exits. |
| `target/phase514/final-ai-check-retry/source-files.sha256.json` | Allows the 212-input checkpoint to be compared with thirteen changed current files. |
| `target/phase514/final-ai-check-retry/summary.md` | States fast-check scope and explicit pending-quality limitations. |
| `target/phase514/final-ai-check-retry/earth-orbit.json` | Shows prior schema-5 paired terrain/memory/timing semantics. |
| `target/phase514/final-ai-check-retry/earth-orbit.png` | Visually checks the prior paired production-frame capture. |
| `C:/Users/senne/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-27.0.1/src/api/render_pass.rs` | Defines locked indirect, indirect-count and mesh/task draw APIs. |
| `C:/Users/senne/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-27.0.1/src/api/compute_pass.rs` | Defines locked compute/indirect-dispatch API availability. |
| `C:/Users/senne/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-types-27.0.1/src/features.rs` | Defines locked optional binding/indexing/mesh/indirect feature flags. |
| `C:/Users/senne/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-types-27.0.1/src/lib.rs` | Defines effective default limits and compute/downlevel capability flags. |
| `C:/Users/senne/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-types-27.0.1/src/instance.rs` | Defines memory-budget failure thresholds rather than live usage queries. |
| `C:/Users/senne/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-types-27.0.1/src/tokens.rs` | Shows experimental-feature activation's unsafe-code requirement. |
| `C:/Users/senne/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-hal-27.0.4/src/vulkan/adapter.rs` | Shows internal optional Vulkan memory-budget extension enablement. |
| `C:/Users/senne/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/wgpu-hal-27.0.4/src/vulkan/device.rs` | Shows internal heap-budget checks without an app telemetry API. |

External primary references inspected: [GPU-based geometry clipmaps](https://developer.nvidia.com/gpugems/gpugems2/part-i-geometric-complexity/chapter-2-terrain-rendering-using-gpu-based-geometry)
(reusable grid/height/normal/transition precedent);
[wgpu 27 release](https://github.com/gfx-rs/wgpu/releases/tag/v27.0.0)
(version-specific API changes);
[wgpu 27 feature source](https://github.com/gfx-rs/wgpu/blob/v27.0.1/wgpu-types/src/features.rs)
(optional feature definitions); and
[wgpu 27 render-pass source](https://github.com/gfx-rs/wgpu/blob/v27.0.1/wgpu/src/api/render_pass.rs)
(indirect/mesh draw contracts). Current online API documentation was consulted
for discovery only; locked local sources govern this audit.
