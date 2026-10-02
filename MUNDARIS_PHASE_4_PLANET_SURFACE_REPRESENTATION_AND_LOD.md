# Mundaris — Phase 4: Planet Surface Representation & LOD

> **Status:** smooth-sphere implementation present; Windows numerical/benchmark/directed native evidence recorded. Complete operator/platform and performance acceptance remain open — see [validation](docs/phase-4-validation.md) and [ADR0006](docs/adr/0006-planet-surface-topology-and-lod.md).
>
> **Audit:** 2026-10-02, initially clean repository at `bf4c1f9ced75d11181cc9b716d599b9698053b18`.
>
> **Targets:** stable Rust/Rust 2024, native Windows x86-64 and Linux x86-64, existing six crates.
>
> **Prerequisites:** [bootstrap](MUNDARIS_PROJECT_BOOTSTRAP.md), [engine design](MUNDARIS_ENGINE_DESIGN.md), Phases [1](MUNDARIS_PHASE_1_REFERENCE_FRAMES.md), [2](MUNDARIS_PHASE_2_CELESTIAL_MODEL_AND_TIME.md), [3](MUNDARIS_PHASE_3_GRAVITY_ORBITS_AND_CELESTIAL_RENDERING.md), [3.5](MUNDARIS_PHASE_3_5_CELESTIAL_NAVIGATION_SYSTEM_VIEW_AND_TIMEWARP.md), and ADRs [0002](docs/adr/0002-reference-frames-and-precision.md), [0003](docs/adr/0003-celestial-domain-and-time.md), [0004](docs/adr/0004-gravity-integration-and-playback.md), [0005](docs/adr/0005-celestial-navigation-system-view-and-timewarp.md).
>
> **Evidence:** validation records for [Phase 1](docs/phase-1-validation.md), [Phase 2](docs/phase-2-validation.md), [Phase 3](docs/phase-3-validation.md), [Phase 3.5](docs/phase-3-5-validation.md), and [performance](docs/performance.md). Outstanding platform/operator evidence remains outstanding.

## 1. Outcome, scope and decision summary

Phase 4 proves the structural planetary rendering problem: representing, addressing, subdividing, bounding, selecting, culling, preparing and transitioning a spherical surface across extreme scales. It does not choose what mountains exist.

The required end-to-end experience is:

```text
whole-system overview → select Aurelia → focus Aurelia
→ approach from millions of kilometres away
→ far physical sphere yields to hierarchical surface patches
→ continue through orbit, kilometre and metre scales
→ inspect the curved horizon at ~2–10 m clearance
→ look toward the independently simulated Luma/Solace
→ depart to the same system overview
```

There is one authoritative `CelestialSystem`, the same `BodyId`, the same body state and one observer. No scene switch, second planet, separate surface world, body teleport, or global origin mutation is permitted. Rendering can change representation without changing world identity.

| Concern | Phase 4 decision |
| --- | --- |
| Base geometry | Perfect sphere at the authoritative reference radius; zero height. |
| Topology | Six normalized-cube faces, `normalize(N + u U + v V)`; no equal-area/spherified mapping. |
| Addressing | Compact checked `(face, level, x, y)` render patches; body-fixed direction for surface locations. |
| Hierarchy | Computable quadtree, flat complete covering leaves plus a visible subset; no owned recursive nodes. |
| Quality | Conservative projected spherical approximation error in physical pixels. |
| Stability | Split above 0.125 px, merge below 0.0625 px; previous split decisions are explicit inputs. |
| Seams | Edge-neighbor difference at most one level; 16 shared stitched index variants, canonical boundary samples. |
| Transitions | Atomic complete sibling replacement with subpixel error limits; no geomorph in the smooth-sphere default. |
| Generation | Synchronous CPU `f64` evaluation and checked view narrowing; shared topology, packed sample storage, at most 16 instanced index draws per surface batch. |
| Coordinates | Body-fixed samples, source-centered conversion; explicit co-rotating surface inspection. Regional math, no automatic tree-node allocation. |
| Depth | Existing infinite-far reverse-Z/Depth32Float, shared celestial depth; near policy extended for at/inside diagnostics. |
| Integration | Extend `--gravity-orbits`; app decides body capability and representation handoff. |
| Resources | Bounded metadata cache and reused frame buffers; no persistent per-patch CPU mesh cache or general jobs. |

These are choices for implementation and measurement, not claims of implemented capability. Revisiting a choice requires updating this contract and recording the reason; it must not silently change permanent identity or numerical guarantees.

## 2. Complete repository audit and fresh evidence

The audit read every tracked file: specifications, ADRs, validation/performance/architecture documents, all six crate manifests and source, shaders, tests, benchmarks, workspace manifest/lockfile, CI and repository configuration. No repository `AGENTS.md` exists. Ignored `target/` products and external dependency source are not repository source.

The actual project links remain:

```text
app → math + world + simulation + renderer
world → math
simulation → math + world
renderer → math
core: documentation-only
```

The locked stack is wgpu 27.0.1, naga 27.0.3, winit 0.30.13, glam 0.30.10, egui 0.33.3 and Criterion 0.8.2. Renderer requests no optional GPU features and uses default device limits. World/simulation do not depend on graphics.

Fresh audit execution on Windows: `cargo test --locked --workspace --all-features` passed **96 runtime tests and seven compile-fail documentation tests**; two long-run tests were ordinarily ignored. This is baseline evidence only. The new topology, LOD, depth-range and surface-precision tests specified below do not exist yet. This audit performed no new native visual run or benchmark. Historical measurements are retained with their original scope; there is no Phase 4 timing result. GitHub CLI is unavailable on this host, so current remote CI was not inspected.

Documentation completion also passed `cargo build --locked --workspace`, `cargo check --locked --workspace --all-targets --all-features`, `cargo fmt --all -- --check`, warnings-denied locked workspace Clippy and warnings-denied locked workspace Rustdoc. Tracked and new-document whitespace checks pass, and the source/manifests/lockfile/CI diff is empty. These checks validate the unchanged foundation and documentation change scope, not Phase 4 implementation acceptance.

A standalone integer-domain calculation checked the proposed grid16 boundary-collapse rule for all 16 masks: signed domain area was exactly one, no triangle had reversed winding, nondegenerate counts ranged 512→480, and maximum triangle diameter was `sqrt(5)` grid steps. This supports the conservative `4h` bound; it does not replace implementation tests of edge incidence, overlap, spherical winding or rendered cracks. Sample-memory calculations in Section 23 were checked independently. No prototype source or new engine test was created during this audit.

### 2.1 Physical sphere and renderer findings

| Implementation | Verified behavior and Phase 4 consequence |
| --- | --- |
| `renderer/src/celestial.rs::Icosphere` | `Vec<DVec3>` unit vertices and `Vec<u32>` indices, 20 base triangles, three midpoint-subdivision rounds with undirected-edge deduplication using `BTreeMap`. 642 vertices, 1,280 outward triangles, 3,840 indices. This is a reusable debug primitive, not an existing planetary hierarchy. |
| Sphere generation lifetime | App keeps one CPU `Icosphere`. GPU renderer creates another deterministic copy during initialization to upload its indices once. No per-frame subdivision or mesh asset manager. |
| Radius path | `BodyProperties::reference_radius_m()` → app `CelestialRenderBody.reference_radius_m: f64` → `radius * unit` in `f64`. Radius never comes from a marker, camera fit, frame scale or gravity kernel. |
| Draw preparation | Each drawable body streams 642 view-relative position/normal pairs at 32 bytes/vertex; a 32-byte color/flag uniform occupies a 256-byte dynamic slot. One indexed draw/body. GPU draw slices hardcode 642 vertices/3,840 indices/1,280 triangle reporting. Those assumptions are local to this path, not world identity. |
| Precision | `PreparedView::prepare_source` converts the observer into the source using Phase 1 LCA math. `PreparedRenderFrame::view_displacement` subtracts locally before camera rotation. Sphere preparation uses this `f64` helper with its own range/radius/0.05-pixel budget, not the 10 km `near_debug()` narrowing policy. |
| Whole-mesh fallback | All vertices are checked, including offscreen/back/near-plane-crossing vertices. A single range/precision failure drops that body's mesh and returns `PrecisionMarker`; later invalid `f64` geometry is still checked. At near radius, offscreen vertices can fail the very strict near-plane budget. Surface refinement must not inherit whole-body all-or-marker behavior. |
| Visibility | Sphere selection uses forward depth plus radius, distance range and approximate projected diameter. There is no patch frustum or horizon culling. Centre projection is not a sufficient visibility test when a planet fills the screen. |
| Apparent size | Current `2 R focal / max(-center.z, near)` is a small-object estimate, not exact near-sphere angular diameter or off-axis silhouette. It becomes unsuitable for surface handoff/selection rings near radius. |
| Marker fade | App `marker_opacity` fades from 8 to 24 physical px only for `PhysicalSphere`; fallback remains opaque. Selected/focused rings persist, capped at 2,000 physical px radius; labels require a projected centre. Surface readiness must count as available geometry even when the old sphere draw is suppressed. |
| Depth | `CelestialProjection` uses right-handed, -Z-forward infinite reverse perspective; depth `near/(-z)`, clear 0, `GreaterEqual`, `Depth32Float`. Opaque spheres write depth; curves test without writes. Phase 1 debug lines keep separate forward depth. No logarithmic depth, depth partitioning, multipass cameras or depth bias. |
| Pass ownership | `CelestialRenderer::draw` currently owns the clear/depth pass, followed by history/styled curves, then egui without depth. Phase 4 must insert opaque surface draws into that same depth ownership, not clear depth between planets and patches. |
| Reusable abstractions | Borrowed prepared view/source, checked projection/content rectangle, f64 line clipping, safe explicit byte packing, poisoned full-frame staging, lazy pipelines, growable buffers, resize/suspend/surface recovery and WGSL validation are suitable foundations. No generic mesh renderer needs to be invented. |

The current chordal bound is `0.005 R`: up to 31,855 m for Aurelia. Unit vertices on the reference sphere do not make the planar triangles a metre-accurate surface. Existing tests check the bound and ordinary near/far reverse-depth monotonicity, not simultaneous metre/horizon/moon/star visibility.

### 2.2 Navigation, frames, coherence and performance findings

| Implementation | Verified behavior and Phase 4 consequence |
| --- | --- |
| `app/src/celestial_camera.rs` | One checked `FramePose` plus derivative metadata; System Orbit, Body Orbit, Free Flight and smooth focus/overview transitions. Body Orbit is in the body's translating frame unless fixed-spin debugging is explicitly selected. |
| Clearance zoom | `distance = R + exp(log_clearance)` with 80 ms response, factor 1.25/notch, minimum `max(1 m, 64 ulp(R))`, disclosed maximum navigation distance `1e15 m`. No remaining 1.05R body floor prevents metre approach. Pitch ±1.5 rad is an orbit-control limit, not a surface-coordinate singularity. |
| Focus path | First focus targets 4R in 0.9 s, via pull-back/transit/log approach. It does not itself demonstrate a slow astronomical descent. Add explicit approach/clearance controls and scripted inspection, keeping current Focus available. |
| Look direction | Orbit pose couples viewing direction to the body centre. Rotating orbit yaw changes position by approximately `(R+h) delta_angle`; at 0.005 rad/input point this is ~32 km on Aurelia. It cannot serve as metre-scale look-around. |
| Free Flight | System-stationary by default, even on a translating numerical carrier: committed body translation is compensated. Enters below 32R/leaves above 64R. No default radial clearance guard; it can enter the sphere. No co-rotating free inspection exists. At h60 a body moves ~1.8e6 m/tick, so stationary free flight is unsuitable for holding near a moving surface. |
| Existing re-expression | Debug re-expression is restricted to the same completed focus body, translating/fixed roles. Generic Phase 1 pose/kinematic conversion remains available. Focus/attachment deliberately changes future tracking, while coordinate re-expression preserves the instantaneous defined state. |
| Frame projection | Every body has a translating root child and a fixed rotating child. The moon has its own root anchor and never inherits planet spin. Projection exposes only `&FrameTree`, not arbitrary insertion/mutation. Automatic regional nodes would require a new projection lifecycle contract. |
| Coherence | World owns properties/state/time/revision; `CoherentCelestialView` validates namespace/body topology/revision/exact instant and retains both borrows. App publishes after pumping; projection failure suppresses mixed draws. Geometry metadata must not be regenerated on every kinematic world revision. |
| Near plane | App chooses `max(0.1 m, 0.01 * nearest_positive_clearance)`, ignoring nonpositive clearances. This is sufficient for default outside clearance ≥1 m but can choose a distant body's clearance when inside/at a nearer body. Extend that policy explicitly. |
| Picking/overlays | f64 reference-sphere picking already handles an offscreen centre; markers/labels remain no-depth navigation overlays. Near-surface sphere picking can stay analytic in this phase. Planet centre labels, huge rings and centre-to-1.2R debug axes need surface-aware presentation, not deletion of BodyId picking. |
| Tick motion | Constant spin and gravity publish full-time h60/h10 states; no render ephemeris interpolation. Co-rotating inspection cancels common motion for nearby geometry. Independent moon/star observations still change at actual committed ticks; high-warp jumps are not LOD jitter. |
| Performance | Existing N3 sphere preparation ~0.134 ms; N3 full-retention bounded history display ~1.74 ms; N16 full-retention display ~9.42 ms. Checked 65,536-vertex near preparation ~2.15–2.17 ms. These motivate batching and careful patch counts, not a claim CPU patch evaluation is free. All are CPU-only measurements. |

The Phases 1–3.5 foundation does not require an ownership rewrite. The identified limits are representation/control policies that must be extended deliberately. Existing Linux, remote-CI, Phase 2 full Windows visual and Phase 3.5 operator/high-DPI/sleep acceptance remain open; ADR 0004's earlier deferral is not blanket acceptance for Phase 4.

## 3. Topology comparison and selection

| Parameterization | Hierarchy, sampling, seams and future use | Assessment |
| --- | --- | --- |
| Normalized cube | Six square charts with direct dyadic quadtree identity, four deterministic children, local refinement and regular grids. Sampling density varies across a face; seams need signed axis permutations. Direction queries, tangent math, sparse region intersections and GPU evaluation are straightforward. | Select for the smallest complete implementation. Distortion is bounded and reflected in local error/bounds, not hidden by altitude thresholds. |
| Spherified cube | Polynomial/square-root mapping reduces density distortion; often confused with simple normalization. Still six quadtrees, but inverse/domain metric, edge evaluation, conservative derivative/bounds and shader precision are more involved. | Revisit only if normalized-cube measurements show materially uneven quality/work. No such measurements yet. |
| Equiangular cube | Replace linear chart axes by tangent of uniform angles, then normalize. Better angular spacing along chart axes, extra transcendental evaluation and different nested-domain/error math. | Viable alternative; its costs solve no measured Phase 4 blocker. |
| Icosahedral subdivision | Near-uniform triangular sampling, twenty roots, deterministic four triangle children, no polar collapse. The existing icosphere is not addressed this way. Triangular render grids and orientation/neighbor/stitch tables are less natural for future 2D region queries and shared quad topology. | Strong sampling alternative; reject for initial implementation complexity, not because triangle addressing cannot support edits/vegetation. |
| Octahedral sphere | Eight triangular roots or folded-square encoding; compact directions/GPU mapping, dyadic refinement possible. Axis/corner distortion and fold adjacency/winding need explicit rules. | Fewer roots do not outweigh folded/triangular transition complexity here. |
| Latitude/longitude | Familiar coordinates and rectangular grids, but polar degeneracy, anisotropic cells, seam and shrinking polar rings complicate uniform error/local refinement and stitching. | Reject as render hierarchy; latitude can still be a future environmental-field input. |
| HEALPix/equal-area cells | Strong equal-area field/statistical sampling and stable cells; unusual boundary geometry and mesh adjacency. | Materially relevant to future global fields, unnecessary as first surface mesh. |
| Geodesic/local clipmaps | Good near-observer regular GPU work, but moving chart origins, global coverage and spherical seam/persistence separation need an additional representation. | Possible later hybrid; not the initial global topology. |

All direction-based topologies can support future height, biome, hydrology, vegetation and sparse edits if identity is kept independent of render allocations. No topology makes caves a heightfield feature. Normalized cube wins on computable hierarchy, fixed shared grids and explicit edge mathematics; the prior cube-sphere preference alone is insufficient justification.

For `q = N + u U + v V`, `u,v ∈ [-1,1]`, use exactly:

```text
n(u,v) = q / sqrt(q.x² + q.y² + q.z²)
p(u,v) = R n(u,v)
```

Here `N,U,V` are signed cardinal axes. Area Jacobian is `(1+u²+v²)^(-3/2)` on the unit sphere: centre-to-corner density ratio is `3 sqrt(3) ≈5.196`. This is meaningful distortion, not equal-area sampling. Maximum mapping derivative norm is at most 1 because `|q|≥1`; local conservative error and tests handle it. Record patch counts/error versus face centre/edge/corner before changing mapping. No final texture parameterization is implied.

## 4. Spaces, face convention and winding

Body axes retain Phase 1's right-handed Cartesian convention. Face enumeration/traversal order is `PositiveX, NegativeX, PositiveY, NegativeY, PositiveZ, NegativeZ`.

| Face | N | U, increasing face u | V, increasing face v |
| --- | --- | --- | --- |
| +X | +X | -Z | +Y |
| -X | -X | +Z | +Y |
| +Y | +Y | +X | -Z |
| -Y | -Y | +X | +Z |
| +Z | +Z | +X | +Y |
| -Z | -Z | -X | +Y |

For every face `U × V = N`. Mesh patch coordinates `(s,t)` lie in `[0,1]²`, increasing along U,V, row-major `index = j*(n+1)+i`. Grid resolution is initially **16×16 cells**, 17×17 =289 sample vertices, 512 regular triangles. This balances depth/patch metadata against per-patch work; benchmark 8 and 32 cells only as comparisons, without heterogeneous runtime grids.

Each regular cell uses `(a,b,d)` and `(a,d,c)`, where a is lower-left, b lower-right, c upper-left and d upper-right. This is outward CCW before camera projection. Use `FrontFace::Ccw`, back-face culling in normal view, no reflections or negative radius. Shader normals are analytic radial directions rotated into camera axes. Diagnostic underside view uses a separate no-cull pipeline option; ordinary view remains opaque exterior rendering.

The face UV domain is explicit, never silently mirrored to make a texture look right. Across a face edge a 2D chart may rotate/reverse; stable direction evaluation is the common surface field coordinate. No global continuous tangent/UV chart exists on a sphere.

## 5. Stable identity: location versus render region

### 5.1 Surface location

A mathematical location is a checked **body-fixed `Direction3`**, optionally plus a radial offset in metres when a 3D location is needed. World binds it to `BodyId`. At zero displacement its point is `R * direction`. It is independent of observer, grid size, face assignment, LOD, FrameId, cache slot and GPU handle.

`BodyId` is currently stable within an append-only session, **not a persistent project ID**. Future save data needs an explicit persistent-body resolver; this design must not claim existing namespaces survive reload. A floating direction is a spatial coordinate, not a hashable generated-object identity: no equality/hash of rounded directions defines future tree or edit IDs.

Future persistent region/candidate identity can use a versioned, fixed-resolution layer partition or authored body-local coordinates. Its resolution and topology version are independent of the current render LOD. Terrain edits can reference body-fixed spatial support plus layer/persistence identity; generated vegetation can use layer region key + candidate index + generator version/seed. Those encodings are deferred until they have authoritative callers.

### 5.2 Render patch

`CubePatchAddress { face: CubeFace, level: u8, x: u32, y: u32 }` is a checked, ordered, copyable mathematical address. Fields are private; reject level >30 or coordinates ≥`2^level`. There are six roots at `(level=0,x=0,y=0)`.

The maximum **representable** level is 30, not a forced detail level. Use u64 intermediates for grid numerators and shifts. Phase 4 cannot claim arbitrarily small geometric precision at this level; the numerical error floor and maximum-level report constrain usable refinement.

App's runtime resource association is `(BodyId, topology version, patch address)`. Renderer receives an opaque session surface key or dense request index instead of importing BodyId. Cache addressing contains no FrameId. Geometry metadata also keys the fixed grid/topology version; body radius scales dimensionless metadata and is copied per current view, never stored as a second physical radius.

A patch ID is stable for its region **within this topology convention** and can key disposable caches. It is not permanent terrain/edit/vegetation/hydrology identity. Replacing this rendering topology changes render-region addressing without moving an authoritative surface location.

## 6. Computable hierarchy and exact adjacency

For N=`2^level`, patch face bounds are:

```text
u0 = -1 + 2*x/N; u1 = -1 + 2*(x+1)/N
v0 = -1 + 2*y/N; v1 = -1 + 2*(y+1)/N
u = u0 + (u1-u0)*s; v = v0 + (v1-v0)*t
```

Child order is lower-left `(0,0)`, lower-right `(1,0)`, upper-left `(0,1)`, upper-right `(1,1)`, encoded `child = dx + 2*dy`. Children have `(level+1, 2*x+dx, 2*y+dy)`; parent has `(level-1,x/2,y/2)`; root has no parent. Traversal is face order then this child order (depth-first), with iterative stack/reused scratch. Logical interior domains are half-open; shared geometric boundaries are closed. Canonical location-to-face assignment chooses largest absolute Cartesian component, ties X before Y before Z, then sign, so a seam/corner belongs to one chart for point lookup.

Same-face edge neighbors adjust x/y by one. An edge query returns a same-level neighbor address, its shared edge and whether along-edge coordinate is reversed. Edges are `UMin, UMax, VMin, VMax`. At a face boundary:

1. Adjacent face normal is `-U,+U,-V,+V` respectively.
2. On the shared cube edge, map q using exact signed-axis dot products: `u'=dot(q,U')`, `v'=dot(q,V')`; `dot(q,N')=1` there. One of u',v' is ±1 and identifies the neighbor edge.
3. Let source along-edge tangent be V for a U edge, U for a V edge. Dot it with the neighbor along-edge tangent; +1 preserves interval index k, -1 maps to `N-1-k`.
4. Set the neighbor fixed cell coordinate to 0 or N-1 according to its edge; use the mapped k for its varying coordinate.

This constructs all 24 directed edge transitions from the face table using integers, without float direction sampling or guessed cube-net orientation. Unit tests additionally carry an independently authored 24-entry expected table, reciprocal edge/reversal checks and corner fixtures. A leaf-neighbor query first tests the same-level address, walks its parents for a coarse cover, or visits only descendants touching the edge for fine covers. Corner-only contact does not impose an edge-level constraint.

No node has heap-owned children. A sorted vector of complete covering leaves, previous split-address set and scratch membership index are sufficient. The mathematical hierarchy is infinite in concept/bounded in encoding; only visited/covering/ready metadata is resident.

### 6.1 Canonical sample evaluation

For a vertex i,j on a patch with n=16 cells, use dyadic face fractions:

```text
face_fraction_u = (n*x + i) / (n*2^level)
face_fraction_v = (n*y + j) / (n*2^level)
```

Reduce dyadic rational numerators/denominators and form a canonical signed Cartesian cube tuple before `f64` conversion. Edge faces and three corner faces therefore construct the **same Cartesian q**, including normalized signed zero and X/Y/Z evaluation order. Matching parent/child samples reduce to the same tuple. Normalize once through the same CPU function; GPU does not independently evaluate another face mapping. Body radius multiplication and source centering likewise use the same order.

Sample evaluation checks `i,j≤cells` and a declared power-of-two grid resolution; production uses 16, comparison fixtures use 8/32. Arbitrary resolutions must not silently break nested edge sampling or cache versioning.

Bit-equal canonical sample positions on one target are required; cross-platform CPU components use numerical tolerances. Final equal edge GPU positions are packed once per canonical boundary key per view and reused by both patches, avoiding differing subtraction/rounding paths. The temporary edge table is bounded by active boundary samples and is discarded/reused after frame preparation. This is shared render data, not world storage.

## 7. Smooth-sphere approximation error

An analytic sphere and its triangulated approximation are not mathematically identical. Let a triangle have unit directions `n0,n1,n2`, outward unit plane normal m, and `c = dot(m,n0)>0`. Its scaled plane is `dot(m,p)=R c`.

Every planar barycentric point is inside the reference sphere by convexity and has `|p|≥R c`. Normalized cube projection maps the planar domain to directions covered by the corresponding spherical triangle. Radial projection from any planar point therefore reaches that analytic spherical region, giving a conservative radial/Hausdorff bound:

```text
E_triangle ≤ R * (1 - c)
```

The foot of the perpendicular need not lie inside the triangle; using the entire plane only makes this bound more conservative. Compute the maximum over all nondegenerate triangles of all allowed stitched variants for a patch. In finite arithmetic use the minimum of all three evaluated dot products minus the dot/unit-normal arithmetic allowance, rather than assuming an exactly common offset from one vertex. Store dimensionless `e_unit` with geometric bounds; scale by the current R. Near-degenerate numerical arithmetic uses a conservative roundoff allowance, not a falsely negative error.

A cheap analytic bound is also available. With face grid step `h = 2/(16*2^level)`, normalization has angular derivative norm ≤1. The angular distance between samples is bounded by their face-domain distance. Regular triangles have domain diameter `sqrt(2)*h`; conservatively every stitched triangle has diameter ≤`4*h` (verify from all index variants). If all triangle directions lie in a cap of radius δ<π/2, every convex combination has norm ≥cos δ:

```text
E ≤ R*(1-cos δ) = 2 R sin²(δ/2) ≤ R δ²/2
```

Use `δ=4h` as the coarse stitched bound, and the plane-based maximum for tighter selection. The familiar chord sagitta `R*(1-cos(theta/2))` explains an edge but must not be substituted for a triangle-wide bound without proof. Stitching can enlarge triangles; error selection must include it.

At deep levels use stable small-angle expressions where possible and add the dimensionless arithmetic allowance. Initial absolute numeric floor is `64*f64::EPSILON*R` (~9.1e-8 m at R=6.4e6 m); validate it against independent dense samples. Report `max_level`/`precision_floor` if a requested pixel tolerance cannot be met, never return zero geometric error merely because `1-c` rounded to zero. No float formula defines patch identity.

Phase 5 adds conservative filtered-field approximation/error and displacement bounds to the sphere error. It cannot simply treat authored maximum height as the mesh approximation error; those quantities have different meanings.

## 8. Patch bounds and metadata

All mapping, bounds, camera conversion, distances, plane tests, error and active-set policy calculations remain `f64`. No culling position is narrowed to body-centred `f32`.

Choose an analytic directional cap `(a, alpha)` with a the normalized face-rectangle centre and alpha the maximum angle to its four corner directions, plus numeric allowance. Evaluate unit-vector angles as `atan2(|a×b|,a·b)`; `acos(dot)` can round tiny deep-level cap angles to zero and produce nonconservative bounds. Positive combinations of corner q vectors cover the face rectangle; a hemispherical cone is convex, so the corner cap conservatively covers the entire analytic patch. Root alpha is about 54.736°.

For a zero-height cap on radius R:

```text
bounds centre = R cos(alpha) a
bounds radius = R sin(alpha)
```

This ball contains the spherical cap and the convex hull of its triangles. Inflate by numeric allowance and future `max(abs(height_min),abs(height_max))`. Metadata is dimensionless until radius is supplied. Bounds need not be tight AABBs or per-vertex culling lists.

For smooth-sphere horizon rejection also retain an envelope of **geometric triangle plane normals**, not shading normals: unit cone `(m_axis,beta)` containing every triangle normal in all 16 variants, and the minimum positive plane offset `c_min`. Build these bounded metadata products once with reusable fixed-size sample scratch, including stitched triangles. Construct m_axis deterministically (start with analytic cap axis; beta is the maximum angle of plane normals to it); expand beta and lower c_min by certified arithmetic allowances. Cross-product normalization amplifies cancellation in deep/skinny triangles: derive a private scalar error bound from operand magnitudes/cross-product norm, never assume all normals have a constant 64-ULP angular error. If that bound cannot certify a useful hemispherical envelope, horizon rejection is unavailable for this candidate; conservative parent/frustum tests still work. Bound construction can enumerate vertices/triangles; per-frame culling does not. The fallback is required in maximum-level diagnostic tests.

Metadata construction cost is real. Build coarse parents before descending, cache only compact results, and budget cold child metadata construction. No high-resolution mesh/GPU payload is generated merely to test an invisible child. The shared index variants are built once, and variant-independent regular triangles need not be recomputed sixteen times. Measure cold/warm bounds independently before optimizing this path.

## 9. Observer-dependent LOD metric

Distance-only or hardcoded altitude bands ignore viewport/FOV/radius/curvature. Angular patch size alone ignores sampling error. Projected edge length is useful for future sampling bands and debug grids but would unnecessarily over-tessellate a perfectly smooth local plane. Choose **screen-space geometric error** as the primary metric; horizon/frustum relevance determines where it is evaluated.

For physical content height H and vertical FOV φ:

```text
f = H / (2 tan(phi/2))                 pixels
tx = tan(phi/2) * width/height
ty = tan(phi/2)
z0 = max(near, -bounds_center_view.z - bounds_radius)
```

E includes spherical/stitch approximation plus numeric allowance. Projection is `f*(x/z,y/z)` with z=-view.z. Its differential operator norm is `f/z * sqrt(1+(x²+y²)/z²)`. For possibly visible points inside the expanded frustum, conservatively use:

```text
if E >= z0/2: split (unless capacity/level requires explicit degradation)
otherwise:
    z_safe = z0 - E
    lever = sqrt(1 + (tx + E/z_safe)² + (ty + E/z_safe)²)
    projected_error_px = f * E / z_safe * lever
```

This is a bound, not an exact silhouette error. Bounds intersecting the near plane use near, never division by zero or negative centre depth. Invisible/offscreen samples must not force whole-planet precision fallback. Tightening z0 by intersecting a conservative bound with the five-plane frustum is a measured follow-up if overly conservative near-plane selection becomes material; it cannot weaken containment.

The metric scales quality naturally with physical resolution/FOV, body radius and actual camera pose/altitude. Same pixel threshold means similar pixel-sized approximation error; a larger viewport legitimately requires more detail. No hard activation altitude or fake surface render distance is present. Future field scale-band/query-resolution requirements may add another conservative criterion; they must not change surface truth with camera distance.

Initial settings: split **>0.125 physical px**, merge **<0.0625 px**, maximum representable level 30. Equality retains the prior decision. These deliberately subpixel values support atomic smooth-sphere transitions; they are tunable renderer quality policy, with measurements and recorded changes. Selection alone does not enforce one-metre edge spacing: at 2 m clearance a smooth sphere may require tens/hundreds of metres between samples while still meeting curvature error. A separate 1 m diagnostic primitive validates local precision; debug patch spacing is not terrain detail.

## 10. Hysteresis, selection and deterministic history

A rebuild-from-roots traversal is simpler to verify than a highly stateful incremental tree. Choose a hybrid: rebuild desired coverage each update from six roots, consulting a compact previous split set. The output covering/visible leaves are flat, deterministic, sorted by the defined traversal/address order. Reuse vector/stack storage.

```text
validate coherent body/view/settings
→ roots / conservative bounds
→ coarse horizon + frustum relevance
→ obtain compact error/normal metadata for relevant candidates
→ tighter conservative horizon test
→ apply split/merge hysteresis
→ descend only relevant refinement candidates
→ balance covering leaves (including a neighbor guard)
→ form ready replacement transactions
→ validate actual active cover / seam masks / visible subset
→ prepare samples only for visible active leaves
```

Culled regions retain a coarse logical cover, not a hole or generated fine mesh. Visibility alone does not erase persistent world state or invalidate location identity. Guard neighbors needed for balance may require compact metadata even when culled; no invisible high-detail GPU payload is requested. On a metadata miss, cheap cap bounds and the analytic `4h` error bound can form provisional desired requests; a child becomes ready only after its full metadata validates. Bound visited candidates and pending addresses as well as final leaves. If a cold desired traversal reaches the scratch/work limit, report an incomplete desired estimate and keep its unresolved subdomains covered by ready ancestors; never label a truncated request list as achieved quality.

An unsplit node splits only above the upper threshold. A previously split node may merge only when the **parent's hypothetical stitched error** falls below the lower threshold and sibling replacement/balance permit it. Otherwise retain its split and traverse children. Keep split ancestors while descendants need them; remove obsolete history when their covering branch is merged or evicted. Stationary inputs settle with no repeated splits/merges. Slow radial/lateral movement changes limited regions; rapid zoom requests progressive refinement without holes; orbiting across faces uses the same adjacency rule.

Hysteresis intentionally makes state part of the input. Determinism means identical coherent camera/body/settings, previous decisions, readiness, resource budgets and explicit update schedule produce identical sorted results. Camera pose alone cannot uniquely determine a hysteretic set. A reset starts from six roots deterministically. No unordered map iteration or host timing decides the desired topology; generation caps are operation counts for replayable tests.

## 11. Neighbor restriction and balancing

Choose `abs(level_a-level_b)≤1` for every edge-sharing active covering pair, including cross-face edges. This allows fixed finite stitching variants. Without restriction a fine edge needs arbitrarily long coarse spans/index patterns; skirts would avoid that constraint but introduce unnecessary artifacts.

Process violations in stable address/edge order; split the coarse leaf, queue affected neighbors, repeat until fixed point. Each split increases finite levels and adds exactly three covering leaves. No merges occur during this balancing closure, so it terminates at a finite configured maximum. Compute a closure proposal before mutating active coverage. If capacity/metadata readiness prevents closure, defer the initiating split and keep the prior valid cover; do not partially refine a chain.

On merge, replace exactly four sibling regions by their parent only when every outside edge neighbor would remain within one level. Retain forced split decisions separately from quality-required splits so balance does not permanently latch excess detail.

No universal small constant multiplier is promised for arbitrary sparse requests: a lone deep refinement can require a graded ring/cascade through many levels. Each closure's exact overhead is `3 * forced_split_count`; record desired-unbalanced, desired-balanced and active counts. Exhaustive small trees and adversarial one-cell/corner refinement through level 20 must measure worst overhead/time. Initial review trigger: balanced visible+guard count >4× unbalanced count in normal scripted views, or >1,024 forced covering leaves for one request. A trigger requires investigation, not ignoring the invariant.

## 12. Crack prevention: shared stitched indices

| Strategy | Tradeoff | Decision |
| --- | --- | --- |
| Skirts | Easy, but hide gaps with extra radial walls, silhouette/underside/depth artifacts and future displacement interactions. Huge skirts mask defects. | Not selected; no skirts in acceptance view. |
| Restricted quadtree + stitching | More edge bookkeeping and 16 variants; exact shared coarse edges, no walls, reusable indices, suitable future heights. | Select. |
| Shared vertices alone | Equal-level seams are exact, but a fine midpoint on the sphere is not on a coarse chord. | Necessary but insufficient. |
| Geomorph alone | Can smooth transitions but does not establish compatible boundary topology without neighbor policy. | Future complementary mechanism. |
| Cross-fade | Double coverage, depth/alpha ordering and ghost silhouettes; does not fix gaps. | Reject as surface seam strategy. |
| Hardware tessellation | Dynamic edge factors can work, but portable current wgpu/WGSL has no required tessellation stage; compute is extra architecture. | Defer. |

Each leaf has four mask bits identifying edges whose neighbor is exactly one level coarser: bits 0/1/2/3 are UMin/UMax/VMin/VMax. Generate 16 index lists once. Start with the regular grid and, on masked edges, remap odd along-edge vertex indices to the preceding even boundary sample; corners remain fixed. Apply both edge rules consistently at corners (U-edge rule first, then V-edge rule), discard degenerate triangles, and validate positive outward domain winding, exact domain coverage, manifold boundary and no overlaps for **every** variant. The integer audit found no reversed triangles or area loss; full adjacency/overlap tests still gate GPU integration. A failure requires correcting and recording the triangulation convention before integration; acceptance never relies on skirts.

The fine boundary then uses the identical coarse sample sequence/chord segments; its unused odd samples are not drawn. Across faces, edge reversal changes sample correspondence, not geometry. Equal-level neighbors use canonical dyadic samples directly. No seam position is independently normalized in a GPU shader. The surface remains watertight up to final raster/numeric validation, including corners and simultaneous four-edge masks.

Future displaced terrain must query the same canonical boundary locations with compatible filtering. Different sample scales cannot produce different heights at a shared coarse vertex. Edge stitching remains useful, but its conservative error/bounds must include displaced geometry.

## 13. Popping and transition policy

For this smooth sphere choose **synchronized replacement without geomorph**. Parent and children sample one analytic sphere, but their planar approximations differ. The strict subpixel error policy, analytic normals and identical debug shading limit the change objectively; “they are identical” is not the justification.

A parent remains active until all four logical replacement children and the balancing closure are ready. Culled children need ready metadata/coverage, not a GPU buffer. At one frame boundary replace the complete covering transaction and derive fresh masks for all affected neighbors. Merge replaces complete sibling coverage similarly. There is never overlapping opaque parent/child coverage in normal surface draws.

Measure projected displacement between representations using matched analytic directions/ray intersections, including stitch-mask changes. With both meshes bounded by 0.125 px plus ≤0.05 px narrowing each, a conservative before/after allowance is **0.35 physical px**. Full GPU projection arithmetic adds at most 0.05 px per representation under Section 15, giving **0.45 px total**. These are the maximum normal ready-transition acceptance bounds, not guarantees under deliberately starved/teleported coarse fallback. Normal view uses analytic radial normals, so LOD does not introduce faceted lighting jumps. Debug borders/colors may change discretely and are excluded from normal appearance continuity.

If native inspection finds conspicuous silhouette/shading changes despite those bounds, or scripted ordinary approach exceeds them, Phase 4 is not done: tighten the quality bound or review an explicit morph addition. Do not accept a visible multi-pixel pop merely because the world identity is continuous.

Reserve a conceptual geometry-evaluation boundary for Phase 5:

```text
fine position + parent-surface position/delta + morph factor
```

Future m=0 evaluates the **actual parent triangle surface**, m=1 the child field sample; normals/bounds/edge constraints must cover the interpolation. Edge morph ownership must be shared, not arbitrary independent patch timers. Stitched parent triangles need a compatible common refinement or explicit transition topology: naïve barycentric vertex interpolation can straddle parent stitch triangles and fail exact parent reproduction. Phase 5 must resolve that detail before displaced geomorph is enabled. No unused morph buffer/job framework is implemented in Phase 4.

## 14. CPU versus GPU geometry and draw strategy

| Approach | Fit to current renderer and precision | Choice |
| --- | --- | --- |
| CPU f64 evaluation | Deterministic, inspectable canonical samples, easy independent error/edge tests, uses existing source centering/checks. Costs evaluations, packing and uploads proportional to visible samples. | Initial implementation. |
| Vertex shader analytic sphere | Shared grid + small instances are attractive, but `f32(R*n - camera_body)` subtracts million-metre values and loses near-surface detail. Tiny face-domain intervals also collapse if evaluated in global f32 UVs. Needs a proven patch-local rationalized mapping/precision scheme. | Not the initial precision path. |
| Compute-generated vertices | Can support future expensive fields, but adds dispatch/resources/readiness and still needs coordinate/edge precision. No CPU readback should be required for selection. | Defer until measured generation work justifies it. |
| GPU instancing of CPU-prepared samples | Reuses index topology and batches fixed sample blocks, without recomputing astronomical/local geometry in WGSL. | Select for Phase 4 drawing. |

CPU evaluates smooth sphere samples into reusable staging, source-centers in f64, rotates into camera axes, rotates analytic normals and checks narrowing. Do **not** allocate a `Mesh` object, separate index buffer, pipeline or full CPU topology for each patch.

GPU contract:

- One shared collection of 16 u16 index variants (289 vertices fit u16).
- One packed storage buffer of sample records: `view_position: vec4<f32>`, `view_normal: vec4<f32>` =32 bytes/sample.
- One compact per-instance storage buffer, planned 64-byte aligned record: sample base, level/face/debug flags, color and reserved explicit padding. Exact layout is tested, not Rust/glam transmutation.
- Shader uses indexed `vertex_index` as the grid-sample index and instance data to fetch that patch's prepared samples. It also derives `(i,j)`/local grid UV from that index for borders. No per-patch vertex topology buffer is needed.
- Sort visible instances into the 16 seam-mask buckets. Up to 16 indexed instanced draws for one surface batch, not thousands of per-patch calls. Use bounded storage bindings supported by default limits; no indirect-draw/compute/meshlet framework or optional GPU feature.
- Surface pipeline uses existing celestial projection/reverse-Z/depth attachment, compatible debug shading and optional borders from interpolated local grid coordinates. Curves follow all opaque geometry; UI follows curves.

Prepare outputs only for the current view. They cannot persist as authoritative or body-space data. Planet motion does not rewrite stored local samples/world content; preparation of visible output for a new camera/evaluation is expected derived work. A later GPU field backend can produce the same prepared sample contract without changing patch addresses, world identity, selection or coverage transactions.

## 15. Precision, near-plane crossings and errors

For body-fixed observer c and sphere point p:

```text
p_body = R*n                         f64, never AU coordinates
delta_body = p_body - c_body          f64 source subtraction
p_view = camera_from_body*delta_body  f64
checked GPU narrowing                f32 only here
```

Use one prepared source per enabled body. Reuse Phase 1 APIs; extend a narrow read-only accessor for observer-in-source/relative rotation only if actual bounds/selection callers need it. Never implement local surface rendering through `transform_to_root`.

Nearby samples must satisfy both physical and projected budgets: ≤1e-5 m component error within 100 m, ≤1e-4 m within 1 km, ≤1e-3 m within 10 km, and ≤0.05 physical px for potentially visible geometry. Far samples use screen/range budgets, not a global millimetre claim. Include GPU matrix multiplication/projection rounding in an additional ≤0.05 px validation allowance; current `narrow` tests alone check positional narrowing, not the full GPU arithmetic path.

Patch frustum culling precedes sample preparation. For a patch straddling near/side planes, validate all mathematical inputs but do not test behind/offscreen vertices with the whole-sphere near-depth lever. Use f64 triangle clipping against the five planes for the **precision proof**; compare projected clip vertices/intersections against emulated f32 transformed triangles. Normal visible patches share index variants. Rare triangles that cannot satisfy the clipped error test are clipped/prepared as a bounded fallback triangle list using the same depth/shading contract; count its bytes/triangles/draw separately. Its shared transient 64-byte vertex layout carries clip position, camera normal, colour and local grid UV/explicit padding, so different patches can share one fallback draw without unique resources. Use a separate vertex entry/pipeline layout sharing the fragment/depth policy; validate interpolation/clip narrowing and layout headlessly. Clipping is representation math, not collision. This prevents invisible large coordinates from turning a visible surface into a marker and preserves usable parents under budget starvation.

A failed preparation poisons the combined frame; no partial buffer reaches GPU. Recoverable per-region readiness failure retains valid coarser coverage, and reports unmet quality. Invalid numeric input, incoherent view, wrong IDs or arithmetic overflow is an explicit error, never a missing patch quietly accepted as culling. Surface rendering cannot replace a near visible planet with only a centre marker on an ordinary path.

## 16. Body-local and regional coordinates

```text
authoritative BodyId + current BodyState/reference radius
→ translating anchor (root child, no body spin)
→ rotating body-fixed child (zero translation)
→ body-fixed surface location/patch samples
→ optional regional tangent pose
→ observer-relative prepared GPU vertices
```

All patches use body-fixed directions/positions. Orbital motion changes the anchor; rotation changes the fixed frame. Surface addressing/data does not change. Neither patches nor future trees are frame-tree children.

Phase 4 establishes generic tangent/anchor math and an **explicit inspection anchor**, represented as an f64 rigid pose relative to the body's fixed frame. It need not be a new `FrameId`: small observer/diagnostic offsets can be stored in that pose and expanded in body f64 at the query boundary, retaining the body-scale ≤1e-7 m envelope. Regional frames become useful for future local physics, dense object offsets or stricter small-delta requirements; their anchor is a chosen stable surface location, not an LOD patch centre.

Do not add automatic regional tree-node creation in Phase 4. Projection currently has private depth-two publication assumptions and append-only frames/no deletion. Creating nodes on every patch/camera boundary would produce unbounded frames and require an unrelated lifetime design. Generic Phase 1 tests can continue exercising real regional nodes independently of the live celestial projection.

If regional integration later needs runtime nodes, add an explicitly owned derived regional projection with bounded reuse/remap/coherence semantics. A camera crossing a render patch edge never inherently changes its physical reference frame. Re-expression and attachment remain different commands.

## 17. Tangent math and inspection navigation

For checked body-local up n and preferred north axis A (initially body +Y):

```text
east = normalize(A × n)
north = n × east
east × north = n
camera/regional basis columns = [east, n, -north]
```

This is right-handed and fits camera +X right/+Y up/-Z forward. A is a documented body-coordinate convention, not inferred from a name or compulsory spin axis. If `|A×n|<1e-6`, choose the least-aligned cardinal axis (ties X,Y,Z) as the fallback reference. At exact poles zero/NaN is forbidden. Static basis convention has chart discontinuities; a globally continuous tangent field is impossible. For a moving inspection anchor, project the previous east onto the new tangent plane, normalize and reconstruct north; fall back only if ill-conditioned. Parallel-transport-like continuity is navigation state, not location identity.

Extend the existing camera with explicit **Surface Inspection** control, using the same observer:

- Enter only by a user command while focused on a surface-enabled body; re-express the current pose into its fixed frame without moving it. Then deliberately select co-rotating tracking/zero relative simulation derivative. Report this attachment policy change.
- Preserve incoming orientation; offer an explicit “look tangent/horizon” action using the local basis. Mouse look rotates orientation without orbiting the observer around the centre. No-input observer stays at the same body-fixed location while the body moves/spins.
- WASD/QE remain editor offsets at clearance-scaled metres per wall second; normalize diagonal movement. Guard the reference sphere by default after movement, retaining ≥`minimum_clearance(R)`; label it navigation clearance, not terrain collision. Look toward moon/star independently of radial camera placement.
- Departure to Body Orbit can deliberately centre-look; ordinary Free Flight retains its system-stationary policy. No automatic switch to co-rotation merely because LOD becomes fine.
- Extend projection remapping by semantic `(BodyId, fixed role, inspection local pose/anchor)`, never old handles or patch IDs.

Default Body Orbit remains translating/nonrotating. A repeatable approach starts at a configurable astronomical clearance while focused and decreases log clearance continuously without reselecting/reloading the body. Show actual measured `|observer_in_body|-R`; camera's stored orbit-distance field is not valid altitude in free/inspection modes.

### 17.1 Scale behavior

| Observer condition | Expected behavior |
| --- | --- |
| 100R, 10R centre distance / millions of km | Actual projected curvature decides whether far sphere is adequate; patches prewarm if needed. |
| Near one R centre distance | Positive clearance controls zoom; no renderer activation by altitude. |
| 1,000 km, 100 km, 10 km, 1 km | Screen error refines relevant patches; tangent/horizon view works without radial-centre look lock. |
| 100 m, 10 m, ~2 m | Same body/fixed coordinates, physical+pixel precision budgets; metre diagnostic is stable. |
| Rapid departure | Coarser valid coverage/then far sphere, bounded hysteresis and cache; no missing region while merging. |

At exactly/below R in a debug override: disable outside-horizon culling, retain frustum/finite bounded LOD, use near 0.1 m, report signed clearance/inside state. Default inspection/Body Orbit prevent this; ordinary Free Flight gains the same optional navigation envelope with explicit debug bypass. Exterior normal rendering may show no underside because of back-face culling. A no-cull diagnostic toggle can show it; collision/walking/interior planetary physics are excluded. Observer at the centre has no surface direction/tangent; return “unavailable”, not an invented up vector.

## 18. Frustum and horizon culling

### 18.1 Frustum

Use normalized f64 view planes for near and four sides from the same content projection. For an inward plane `(normal,offset)` and sphere `(b,r)`, reject only if `dot(normal,b)+offset < -r-epsilon`. No far plane exists. epsilon initially `max(1e-7 m, 64*EPSILON*(|b|+r+near))`, with scaled norm/finite checks. Equality/grazing remains relevant.

Centres are converted by the prepared source; radii remain f64. Ball bounds contain analytic cap, stitched chord geometry and future declared displacement. Test wide/tall/off-centre/high-DPI viewports and near-plane crossings. Per-vertex visibility testing is not the culling strategy.

### 18.2 Smooth-sphere horizon

For analytic smooth surface, outside observer C in body-fixed coordinates with d=|C|>R sees n only if `C·n≥R`. Analytic horizon angle is `acos(R/d)`. A directional patch cap is analytically hidden if `angle(C/d,a)-alpha` is strictly beyond that angle. **Do not apply that formula unexpanded directly to coarse chordal triangles:** their faces lie below R and can still contribute near the analytic horizon.

Use two conservative stages:

1. Optional cheap occlusion against an inscribed opaque sphere whose radius r0 is the guaranteed minimum radial distance of the complete coarse fallback cover. Compute that from root triangle offsets, not a guessed R. For patch ball `(B,b)` and outside C, set `T=B-C`, l=|T| and shadow axis=-C/d. Only reject when l>b, `angle(T/l,-C/d)+asin(b/l)<asin(r0/d)` and `l-b>d+r0`, with numeric margin. These sufficient conditions put the entire ball inside the angular shadow and beyond the occluder. Uncertain cases continue. This stage is deliberately loose near the horizon and need not be implemented if the tighter stage is cheap enough.
2. The cached geometric plane-normal envelope from Section 8 proves all candidate triangles back-facing/hidden when:

```text
theta = angle(C/d, m_axis)
max_dot = d * cos(max(0, theta-beta))
reject only if max_dot < R*c_min - epsilon
```

For every triangle m, `C·m≤max_dot` and its plane offset is ≥`R*c_min`; therefore its outward face cannot be visible to this observer. This is conservative for every stitched variant and avoids an unproved analytic-horizon shortcut. It includes a small chordal horizon expansion automatically. m is the geometric triangle normal, **not** the analytic normal used for smooth shading. Draw-time back-face culling remains consistent. Precision-border cases survive; disable this rejection at/inside R.

Future displacement requires a different conservative occlusion bound. Keep `SurfaceExtent { min_height_m, max_height_m, guaranteed_opaque_radius_m }` as an explicit future input. For an opaque sphere radius r0 and target radius ≤rmax, maximum visible angular separation is bounded by `acos(r0/d)+acos(r0/rmax)` (where valid d,rmax≥r0), plus patch-cap/numeric allowance. Frustum bounds expand by height; normals-only rejection must be disabled for arbitrary terrain slopes unless proven. No assumption that future edited caves leave the whole reference sphere opaque. Phase 4 smooth height margin is zero; its chordal representation is nevertheless accounted for above.

Culling counts include coarse candidates and fine leaves separately. Cache/build work on hidden roots is permitted; hidden high-detail sample uploads are not. Balance guard records remain logical and are not drawn unless relevant.

## 19. Depth strategy and projection policy

Retain one connected celestial reverse-Z view. No log depth, multi-camera partition or second surface depth pass is required by current requirements. Clear once, draw all opaque far bodies and surface patches with `GreaterEqual`/writes, then debug curves without writes, then UI.

Choose near from the same f64 measured outside clearances as today, `max(0.1 m,0.01*nearest_clearance)`, but if an enabled physical body's clearance is nonpositive/within numeric tolerance, use 0.1 m instead of ignoring it in favor of a distant star. For ordinary ~2 m inspection near remains 0.1 m. The existing policy clips geometry closer than 0.1 m; sub-decimetre camera content is not required. Infinite far keeps moon/star physically drawable subject to their existing perceptual representation budget.

For depth D=n/z, approximate error from depth quantization is:

```text
delta_z ≈ z²/n * ulp(n/z)
```

For normal f32 depth values relative depth precision is roughly constant (~1e-7), so z=2 m is sub-micrometre to micrometre order, z=5 km sub-millimetre order, z=1e8 m metre order, z=1.5e11 m kilometre order. These distant errors do not imply metre accuracy for the star, nor prevent correct ordering of widely separated bodies. Camera-relative vertex/projection precision is a separate constraint.

Tests must compare actual f32 adjacent depth values and reconstructed z, not only monotonic depths. Exercise near 0.1 m with surfaces at 2/10/100/1e3/5e3 m, moon 1e8–1e9 m, star 1.5e11 m in one view; prove 1 cm near separations and 1 m separations at 5 km remain ordered where geometry exists. Overlays are not evidence of opaque depth correctness. Native visual validation records horizon/star/moon visibility on each backend. Revisit partitioning only after a reproducible depth-ordering failure that cannot be fixed within this path.

## 20. Far celestial sphere ↔ surface responsibility

Stars and bodies without surface capability keep the simple sphere path. Surface enablement is an explicit app-owned mapping to existing bodies; it does not infer semantics from mass, name, selected status or apparent size. No authoritative shape/configuration component is needed just to enable this validation renderer.

App `PlanetSurfaceSession` decides representation using projected **far-sphere approximation error** and readiness. Renderer returns generic quality/visibility/preparation reports; it does not decide which body is a planet.

For a small front-facing body, far error is roughly `0.005 R*f/z` (about `0.0025*diameter_px`). Use a conservative projected sphere/silhouette bound, including off-axis/near cases; the small-angle estimate is diagnostic only. For a centred outside sphere angular radius is `asin(R/d)`, not R/z. If the observer is near/inside or a silhouette crosses the viewport/near plane, surface responsibility is required irrespective of centre-marker availability.

Initial handoff policy:

- While far error <0.05 px, far sphere can remain sole opaque geometry; no fine patches.
- At far error ≥0.05 px prewarm root/desired surface coverage. Rough centred sizes are 20 px at this threshold and 50 px at 0.125 px; these are derived estimates, not locked diameter switches.
- When a complete relevant surface cover is ready and both representations satisfy ≤0.125 px geometric error, transfer opaque responsibility atomically. Once surface-active, return to far sphere only below 0.05 px with that sphere's own preparation ready; retain surface until then.
- During prewarm both representations may be prepared, but **only one writes opaque coverage for this body**. No coplanar dual opaque draw, z-fighting, duplicate marker or double-filled planet. The overlap is a quality/readiness range, not an altitude switch.
- If rapid approach outruns readiness, switch to a complete ready coarse patch cover as soon as available and refine progressively. Keep the last valid cover; report coarser-than-requested quality. Far representation may be temporarily inadequate, never reclassify a near-body marker as satisfactory surface rendering.

Refactor celestial preparation into generic per-body observation/overlay data and explicitly requested geometry. Preserve one dense body observation/request mapping even when sphere draw is omitted in favor of surface patches. Marker fade uses `physical_geometry_available`/actual fallback status from either representation. Labels/selected inspector stay BodyId-associated; centre-offscreen surface keeps list/picking usable. Hide centre ring/body-length axes in ordinary surface inspection; provide small tangent/metre diagnostics instead. No low-level renderer receives gravity parent or body-name heuristics.

Handoff tests compare silhouette/colour/normal continuity, opaque ownership count exactly one, same radius/source/sample instant and ≤0.35 px ordinary transition displacement. Preparation/readiness never changes authoritative body/frame identity.

## 21. Readiness, active cover, budget and resource lifecycle

Phase 4 needs only `Requested` and `Ready` compact patch metadata states, with **active coverage** represented by membership in the active leaf set. A patch can be ready but inactive in the bounded cache. Do not add unused generating/retiring/cached enum states or a full job system.

Selector owns desired/active covering decisions; app owns one selector session per enabled body/observer. Renderer session owns metadata/staging/GPU derived resources. Metadata completion is synchronous initially, subject to a deterministic construction-count allowance (initially 32 new records/update, including balance dependencies). Select requests by highest screen error, then traversal order. Warm ready leaves need no generation latency; sample preparation remains synchronous per visible active frame.

Replacement transaction includes the four children, balance dependencies and changed boundary masks. Parent remains until all are ready and the proposed cover is verified. Coarsening requires parent metadata ready and all affected seams valid. Cold startup pins six ready roots before declaring surface representation available. Camera reversal reuses recently ready metadata; desired requests no longer needed are dropped. A future worker can produce immutable metadata/sample results keyed by address/topology/generator revision, but worker completion order cannot directly activate one child or mutate world.

Retain old GPU storage until its submitted use is complete; ordinary wgpu buffer ownership/queue ordering handles disposable resources. Do not overwrite allocation ranges still referenced by in-flight submissions if a later persistent sample pool is introduced. Phase 4 uses complete current-frame buffer uploads and no per-patch GPU allocator.

Emergency limits are explicit resource policy, not world cutoffs. Initial soft review point 2,048 visible patches; hard 4,096 visible patches/view and 65,536 covering/scratch addresses, with byte caps from Section 23. If detail/closure exceeds them, retain a balanced coarser valid cover, prioritize highest-error transactions and report requested versus achieved error/count/cap. Never cull a visible region because its quota was exhausted. At maximum representable level report unresolved pixel error rather than looping.

Cold zoom/teleport paths may take multiple updates to reach requested detail; normal continuous operator/scripted paths must meet quality after convergence and cannot routinely hit emergency caps. Metadata construction latency and sample-preparation time must be measured separately. No CPU time reading changes mathematical desired selection; interactive operation-count settings are explicit inputs.

## 22. Cache and invalidation

Choose a small bounded **metadata-only** LRU cache: key `(opaque surface session key, topology/grid version, patch address)`; value dimensionless cap/bounds/error/normal envelope/readiness. Initial limits **4,096 records and 1 MiB actual accounted cache storage**, whichever is reached first. Active/required parent/transaction records are pinned; if pins would exceed the cache, defer replacement and report budget pressure. LRU uses monotonically checked access sequence, deterministic address tie-breaks and no wall clock. Include container allocation/capacity overhead in accounting, not just payload. Clear on session/body replacement; projection rebuild remaps handles without invalidating geometry.

Radius changes rescale dimensionless metadata and refresh view/navigation envelope; translation/orientation/time/name/mass changes do not invalidate geometry bounds. World revision still gates coherent current rendering. Do not key mesh validity solely to the monotonically changing simulation revision. Future generator/edit revision is a separate geometry invalidation input when those fields exist.

Do not retain CPU patch vertex arrays or per-patch GPU meshes initially. Smooth sphere samples can be reconstructed; cache memory is justified by the more costly conservative bounds/error products. Benchmark cold metadata builds, LRU lookups and reversal churn. If the cache does not materially help, simplify it before adding vertex caching. All caches can be discarded without world mutation.

## 23. Count and memory expectations

These are planning envelopes, **not measured results or universal bounds**. Quality, frustum orientation, face-edge location and conservative metadata all matter. Derive a reproducible count table from actual headless selection before GPU work.

Sphere error scales as `E≈C*R*(2/(n*2^L))²` and projected error as `f*E/z`. Thus local required level is approximately:

```text
L ≥ 0.5*log2(4*C*R*f/(n²*z*pixel_error))
```

C depends on triangle/stitched geometry; use the measured conservative `e_unit`, not one optimistic centre value. At R≈6.4e6 m, H800/FOV60°, n16 and the global stitched bound, a downward 2 m view can need roughly level 16; bounds crossing near 0.1 m may reach level 18–19. Ordinary patches can be shallower because actual plane error is tighter. This selects curved-surface accuracy, not terrain frequencies.

For area A at depth z, patch count density is roughly `A/W²`, with `W≈2R/2^L`; integrate over visible curved area and add graded neighbor rings. A close smooth surface does not require uniformly level-18 coverage of the planet. The worst full uniform level would be `6*4^L`, which must never be instantiated.

| Representative view | Planning visible patch envelope / main uncertainty |
| --- | --- |
| Tiny system-view planet | 0 patches; far marker/sphere. Six roots only if prewarm has started. |
| ~100 px full planet | Tens to ~128; tight error usually needs only a few levels. |
| Planet fills viewport | ~128–512; stitched error and silhouette matter. |
| Low orbit | ~128–768; horizon/frustum and view direction dominate. |
| 10 km altitude | ~128–1,024; near-plane-bound conservatism and horizon flight. |
| 100 m altitude | ~128–2,048; graded rings and grazing view, not an altitude cap. |

Also record 2 m views, at least face centre/12 edges/8 corners, narrow/wide/tall viewports and horizon orientations. If these envelopes are exceeded, measure why and revise estimates/settings; do not invent missing benchmark evidence. Count complete covering leaves, visible active leaves, cache records and forced guard leaves separately.

| Memory item | Planned accounting |
| --- | --- |
| Mathematical address | Semantic fields total 10 bytes; native padding may make 12. Test `size_of`, do not promise serialized layout. |
| Compact metadata | Aim ~128–192 bytes/record before container overhead; 4,096×192 is 768 KiB. Cache cap includes overhead, so actual capacity can be smaller. |
| GPU samples | 289×32 =9,248 bytes/visible patch: 512 patches ~4.52 MiB, 2,048 ~18.06 MiB, 4,096 ~36.13 MiB. |
| Instance data | 64 bytes/patch: 32/128/256 KiB for the counts above. |
| Shared indices | At most 16×1,536 u16 indices =48 KiB plus variant ranges; actual degenerate removal reduces it. |
| CPU outgoing bytes | Same sample/instance payload, reused; no permanent full mesh per theoretical patch. |
| Cover/history/scratch | Initial 65,536-address cap plus checked container overhead, budget 8 MiB; temporary canonical boundary table separately capped at 8 MiB. |

Set CPU outgoing staging capacity cap **64 MiB**, GPU surface buffer allocation cap **80 MiB** including growth rounding/instances/shared topology/clipped fallback (depth attachment accounted separately by window size), metadata cache 1 MiB, cover scratch 8 MiB and boundary scratch 8 MiB. Preflight complete allocations, count vector **capacity** and device storage-binding/max-buffer limits. Oversized request degrades covering detail explicitly; buffer growth cannot bypass caps with `next_power_of_two`. No unbounded retention after revisiting millions of patches.

These are aggregate viewport surface budgets, including all enabled bodies, not an independent full allowance for every planet. App partitions cache/scratch quotas among live per-body sessions and prioritizes current visible error; the renderer enforces total outgoing/GPU bytes and the 4,096 visible-patch ceiling. Tiny/far surface-enabled bodies need no fine cover allocation. Test two simultaneous surface bodies and repeated focus changes against aggregate accounting; releasing a session returns its derived quota without altering its body.

## 24. Future surface-query contract

The following is an architectural interface description; **Phase 4 implements only zero height and analytic normals**, without a generator trait hierarchy or noise dependency.

```text
query surface location:
    body-fixed unit direction / explicit body-coordinate convention
    reference radius (metres)
    immutable world generator configuration, seed and version (future)
    filter footprint/query resolution in metres or radians
    requested scale bands / evaluation context (future)
returns:
    elevation metres relative to reference radius
    optional terrain-derived normal/derivatives
    conservative displacement and approximation/error metadata
```

Patch address is optional batching/diagnostic context, never random identity or terrain-algorithm state. Filtering selects approximations of one field; camera distance/LOD must not change the authoritative field. Shared vertices must use a common boundary filter/evaluation rule; future terrain normals come from derivatives/neighbor samples of that field, not arbitrary per-patch face normals.

Multiple scale bands may contribute continents/ranges/hills/ridges/detail later. The selector consumes their conservative approximation error and footprint requirements, regardless of noise, precomputed fields, authored data or tectonic output. This phase does not lock Perlin, Simplex, fBM, a single frequency function or GPU compute.

Sparse edits layer over procedural base, expressed by topology-independent body-local location/support and their own authoritative IDs/version. Changing render LOD regenerates meshes without changing edit meaning. A future edit invalidates intersecting region caches at all relevant LODs; it is not a mutation of currently visible vertices.

Local volumetric regions may later own sparse body-fixed 3D domains and override/replace height-surface coverage inside explicit masks. A future phase must decide mask boundary ownership, conservative bounds, mesher/LOD seam and collision agreement. Global planet voxelization or SDF meshing is not required now; this surface topology is not declared the complete terrain truth.

Tectonic plates/geology feed low-frequency fields through surface queries, never patch topology/boundaries. Biome/hydrology fields may have independent region partitions. Vegetation generation/identity is independent of terrain render meshes: patches can request/display a layer region but trees are not children of transient leaves/buffers. Moving the body transports every layer through body-local coordinates without per-feature world-coordinate updates.

## 25. Ownership and representative Rust APIs

| Data/policy | Owner |
| --- | --- |
| BodyId, reference radius, physical state/time/revision | Existing `mundaris_world`; authoritative. No second Planet entity/radius. |
| Gravity/constant-spin/fixed ticks/history | Existing `mundaris_simulation`; unchanged physical evolution. |
| Cube faces/addresses/canonical directions/parent-child-neighbor/tangent/cap mathematics | `mundaris_math::surface`; meaningful without a body/GPU. |
| Future permanent terrain/seed/edit/layer configuration | `mundaris_world` when implemented; not forced into renderer by Phase 4. |
| Observer-dependent LOD, balanced cover, bounds/error/readiness/cache and preparation | CPU-only modules of `mundaris_renderer`; derived policy. App owns their per-body session instances. |
| Sample/instance/index buffers, pipelines, depth integration | `mundaris_renderer`, disposable. |
| Surface-capability BodyId mapping, handoff policy, one-observer controls, debug UI/scripts | `mundaris_app`. |

No renderer→world dependency or new crate. Phase 4 adds no permanent world-surface component merely to reserve future terrain fields. Generic surface location math is not a planet-world database.

API sketches express semantics/ownership, not immutable exact signatures; private fields and focused error types follow existing style:

```rust
// Math: no BodyId, FrameId, camera or GPU resource identity.
pub enum CubeFace { PositiveX, NegativeX, PositiveY, NegativeY, PositiveZ, NegativeZ }
pub struct CubePatchAddress { /* checked face/u8 level/u32 x,y */ }
pub struct SurfaceLocation { /* checked body-fixed Direction3 */ }
pub struct SurfaceTangentBasis { /* f64 east/up/north */ }
impl CubePatchAddress {
    pub fn try_new(face: CubeFace, level: u8, x: u32, y: u32)
        -> Result<Self, SurfaceMathError>;
    pub fn parent(self) -> Option<Self>;
    pub fn children(self) -> Result<[Self; 4], SurfaceMathError>;
    pub fn neighbor(self, edge: PatchEdge) -> EdgeNeighbor;
    pub fn sample_direction(self, i: u32, j: u32, cells: u32)
        -> Result<Direction3, SurfaceMathError>;
}

// Renderer CPU policy. All world quantities copied/read-only and f64.
pub struct SurfaceViewInput<'view, 'tree> {
    pub view: &'view PreparedView<'tree>,
    pub body_fixed_frame: FrameId,
    pub reference_radius_m: f64,
    pub projection: CelestialProjection,
}
pub struct LodSettings { /* checked pixel thresholds/level/count/byte limits */ }
pub struct SurfaceLodSession { /* previous splits, flat cover, bounded metadata */ }
pub struct LodReport { /* desired/balanced/ready/active/error/culling/budgets */ }
impl SurfaceLodSession {
    // Reuses owned scratch. No mutable world/tree; readiness is explicit input/state.
    pub fn update(&mut self, input: &SurfaceViewInput<'_, '_>, settings: &LodSettings)
        -> Result<LodReport, SurfacePreparationError>;
    pub fn active_visible(&self) -> &[ActiveSurfacePatch];
}
pub struct SurfaceStaging { /* bounded reusable prepared bytes, boundary scratch */ }
pub struct PreparedSurfaceFrame<'view, 'tree, 'storage> {
    /* retains view/tree and staging borrow; complete-frame poison contract */
}
// Append explicit generic surface requests to the existing coherent celestial frame.
// Low-level GPU PlanetSurfaceRenderer is private; app supplies source/radius/style.
```

```rust
// App: body identity/capability and representation responsibility, no GPU handles.
struct PlanetSurfaceSession {
    body: BodyId,
    lod: SurfaceLodSession,
    representation: SurfaceRepresentationState, // far/prewarm/surface policy
}
struct SurfaceInspectionAnchor {
    body: BodyId,
    location: SurfaceLocation,
    body_from_regional: RigidTransform, // f64, derived anchor; no patch identity
}
```

Allocation behavior is explicit: session/cache setup may allocate; ordinary warm traversal/balancing/preparation reuses bounded contiguous scratch; byte packing is safe little-endian. No per-patch `Rc`, `Arc<Mutex<_>>`, recursive pointers, GPU uniform allocation, generalized tree or worker framework. Existing camera transactional clones may allocate; do not broaden their current behavior into a zero-allocation claim without measurement.

## 26. Exact implementation file plan

This is the expected implementation surface for a later authorized change. **No engine source is modified by this design task.** Pure helpers remain private unless an actual app/test/benchmark caller needs them; expose a narrow library facade rather than internal containers.

| File / operation | Responsibility, API/dependencies and verification |
| --- | --- |
| `crates/math/src/surface.rs` — create | Public checked face/address/location/tangent math, private signed-axis tables/canonical dyadic evaluator; math/glam only. Parent/child/neighbor/cap helpers, focused errors; no render thresholds. |
| `crates/math/src/lib.rs` — modify | Declare/re-export only used surface types/functions and conventions. |
| `crates/math/tests/surface_topology.rs` — create | Independent six-face axes, 24 transitions, edge/corner equality, hierarchy/coverage/address overflow/winding tests; headless. |
| `crates/math/tests/surface_tangents.rs` — create | Poles/axis ties/orthogonality/handedness, explicit continuity and invalid-location tests. |
| `crates/renderer/src/planet_surface/mod.rs` — create | Small public CPU session/settings/report/request facade; private GPU renderer composition. Uses math/glam/current wgpu; no world link. |
| `crates/renderer/src/planet_surface/topology.rs` — create | Private fixed grid/16 u16 stitch index variants, domain coverage and GPU ranges; colocated invariant tests. |
| `crates/renderer/src/planet_surface/bounds.rs` — create | Private f64 cap/ball/triangle-plane error+normal envelope metadata, frustum/horizon helpers, future extent input semantics; no generation algorithms. |
| `crates/renderer/src/planet_surface/lod.rs` — create | Public facade's derived selection/hysteresis; private root traversal/flat cover/balance/ready transactions and deterministic operation budgets. |
| `crates/renderer/src/planet_surface/cache.rs` — create | Private metadata-only bounded deterministic LRU, pin/evict/invalidation/accounting; no CPU mesh/GPU allocator. |
| `crates/renderer/src/planet_surface/prepare.rs` — create | Private f64 sample/view/normal/canonical edge preparation, checked clipped proof/fallback, explicit 32/64-byte layout packing and bounded staging. |
| `crates/renderer/src/planet_surface/gpu.rs` — create | Private shared indices/sample+instance buffers, 16-bucket instancing and rare clipped fallback draw, depth-compatible pipelines/resource limits. |
| `crates/renderer/src/shaders/planet_surface.wgsl` — create | Storage-fetched observer-relative samples/projection/debug analytic shading/border view; no body state or height generator. |
| `crates/renderer/src/celestial.rs` — modify | Separate body observation/overlay preparation from explicitly requested sphere geometry; integrate opaque surface staging/draw/report before curves, preserving all current callers and poisoning. Remove single-mesh assumption only at this boundary. |
| `crates/renderer/src/celestial_view.rs` — modify | Checked sphere apparent-size/silhouette bound, five normalized f64 frustum planes and triangle clipping/projection proof; preserve current viewport/unprojection and infinite reverse-Z. |
| `crates/renderer/src/view.rs` — modify if required | Narrow borrowed source observer/rotation accessor for real bounds/LOD consumers; preserve LCA/subtract-first and near-debug contracts. |
| `crates/renderer/src/lib.rs` — modify | Surface facade exports and lazy GPU/pass wiring; one celestial depth owner, resize/suspend/device recreation. |
| `crates/renderer/tests/planet_surface_lod.rs` — create | Headless error/bounds/culling/selection/hysteresis/balance/coverage/readiness/cache/cap tests through used facade. |
| `crates/renderer/tests/planet_surface_precision.rs` — create | Edges/GPU layouts/WGSL/clipped precision/1 m local fixture/astronomical shared ancestry/radius/depth tests. |
| `crates/renderer/tests/celestial_precision.rs` — modify | Preserve existing sphere/curve tests, add geometry/overlay separation and combined representation/depth regressions. |
| `crates/renderer/benches/planet_surface.rs` — create | Cold/warm bounds/selection/balance/culling/cache/sample packing/rapid refinement, counts/bytes and grid-size comparisons; Criterion, no device startup. |
| `crates/renderer/Cargo.toml` — modify | Register actual new benchmark; existing dependencies suffice. |
| `crates/app/src/planet_surface.rs` — create | App BodyId capability/session/handoff, repeatable inspection paths/debug orchestration and altitude helpers; uses world/math/renderer, no authoritative duplicate body. |
| `crates/app/src/celestial_camera.rs` — modify | Same-observer Surface Inspection and explicit fixed attachment/local look/motion/clearance/anchor remapping, continuous approach target; existing free-flight semantics retained. |
| `crates/app/src/celestial_labels.rs` — modify | Fade on either ready physical representation; surface presentation keeps identity, list fallback and optional small diagnostics. |
| `crates/app/src/gravity_orbits.rs` — modify | Enable Aurelia/Luma surface sessions, unified render requests/near policy/UI, scripted path commands/tangent/metre debug, simulation still existing runner. |
| `crates/app/src/lib.rs` — modify | Minimal headless session/path exports required by tests/benches. |
| `crates/app/tests/planet_surface_navigation.rs` — create | Real gravity+spin+moon, same BodyId/coherent projection, approach/inspection/look/departure/remap and no-render-mutation tests. |
| `crates/app/tests/planet_surface_paths.rs` — create | Deterministic radial/grazing/lateral/oscillation/rapid/reversal scripts and metric records, no graphical dependency. |
| `crates/app/benches/planet_surface_approach.rs` — create | Integrated real fixture/readiness/handoff/camera/renderer CPU preparation versus existing explorer baseline. |
| `crates/app/Cargo.toml` — modify | Register actual approach benchmark only; no new framework. |
| `docs/phase-4-validation.md` — create at implementation | Numerical maxima/script outcomes/platform/backend/CI evidence and open criteria. |
| `docs/adr/0006-planet-surface-topology-and-lod.md` — create at implementation | Topology/mapping/address distinction/error/hysteresis/cracks/transitions/handoff/ownership alternatives, measured consequences and revisit triggers. |
| `README.md`, `docs/architecture.md`, `docs/roadmap.md`, `docs/performance.md`, this specification and `MUNDARIS_ENGINE_DESIGN.md` — update at implementation | Actual implemented scope, quality commands, counts/distributions/bytes/remaining evidence; no design-as-performance claims. |

`mundaris_world`, simulation kernels/time/history, core, old analytic modes, main flag parsing, redraw, root manifest/lockfile and existing CI need no feature changes for this chosen design. World still owns future terrain state when it arrives. A discovered need outside this plan requires a documented narrow revision first. No `tectonics.rs`, vegetation/biome/edit/terrain-noise placeholder modules.

## 27. Staged implementation order

Each milestone must keep Phases 1–3.5 tests and all current modes buildable and passing.

1. Resolve or explicitly review outstanding prerequisite/operator/platform gates; record baseline commands. Review face/address conventions and precision before source work.
2. Implement checked surface math, canonical dyadic mapping and independent edge/corner/neighbor/coverage tests, then tangent math. No GPU yet.
3. Generate shared grid/16 stitched variants; prove all domain/corner/winding properties. Implement bound/error/plane-normal metadata and independent containment/error tests.
4. Implement conservative five-plane/horizon math and LOD projected-error tests, then flat covering/hysteresis/balance. Complete headless active-cover/readiness/cap/cache tests before rendering can hide holes.
5. Run preliminary count/cold/warm selection benchmarks at all representative views; investigate unexpected cascades, poor bounds or budget misses. Confirm grid16 before GPU investment.
6. Implement f64 source-centered samples, clipped precision proof, layouts/WGSL and metre/shared-ancestor tests; then shared-storage instanced GPU drawing in the existing celestial depth pass.
7. Add app surface capability/coherent BodyId integration with existing moving/spinning gravity fixture. Separate observations/overlays from sphere geometry and integrate readiness-based far handoff.
8. Add explicit surface inspection/local look/clearance/tangent anchor and repeatable continuous approach/departure; ensure independent moon/star remain observable.
9. Add normal smooth-sphere view and debug visualization, counters/operator controls; verify no ordinary transition exceeds subpixel bounds and no emergency cap hides regions.
10. Run numerical/script/release/benchmark/native/remote validation, document actual measurements, write ADR 0006 and update status only with evidence. Perform correctness/architecture/performance review.

The primary mode is `cargo run --locked -p mundaris_app -- --gravity-orbits`. A separate `--planet-lod` executable mode is unnecessary initially: deterministic torture paths are headless tests and optional controls in the same integrated mode. If a dedicated flag later improves reproducibility, it must configure this same fixture/session/render path, never create a static second universe.

## 28. Debugging and operator validation

Normal appearance is a smooth, solid, analytically shaded sphere, compatible with the existing celestial debug style. LOD colours are never the only normal rendering mode.

Provide toggles for patch borders, LOD/face IDs, hovered patch address, desired versus active/ready state, culling bounds/normal envelope, transition transactions, analytic normals and tangent basis. Show horizon/frustum-rejected counts without rendering hidden terrain; sample a bounded number of debug bounds/labels so debug itself does not create unbounded work.

Diagnostics:

```text
body / sample time / world and projected revision
camera control/frame role / signed actual clearance / near plane
far error / representation responsibility / prewarm readiness
desired unbalanced / desired balanced / active covering / visible active
requested / ready metadata / deferred transactions / splits / merges
max level / requested and actual error / precision floor / budget degradation
horizon culled / frustum culled / balance guards
cache records / hits / misses / evictions / churn / accounted bytes
prepared samples / fallback clipped triangles / upload bytes / draw calls
selection / balance / preparation CPU times; native GPU timing when available
```

Use existing Aurelia (R=6.371e6 m, tilted daily spin, mutual outer gravity) and Luma (independent inner orbit at ~1e8 m, separate spin). Solace stays simple/unlit. Optionally surface-enable Luma for a second approach using the same systems. No mountains or fake static universe.

Record OS/GPU/backend/driver, source revision/profile, content rectangle/DPI/FOV, settings and observations on Windows and Linux:

1. Launch `--gravity-orbits`, paused whole-system overview. Identify Solace/Aurelia/Luma and distinguish guides from history.
2. Select Aurelia by list/marker/label; focus and set astronomical approach clearance using the inspection controls, without moving the body.
3. Approach continuously from millions of kilometres, logging measured clearance/error/responsibility. Observe sphere-to-patch handoff with exactly one opaque planet.
4. Orbit at high altitude, cross every face edge/corner in targeted paths, enable borders/LOD/face display and confirm topology continuity.
5. Descend through 1,000 km, **100 km**, **10 km**, **1 km**, **100 m** with naturally increasing local refinement.
6. Reach **~2–10 m** clearance. Enter explicit Surface Inspection without a pose snap; look along curved horizon, move 1 m and compare metre/tangent diagnostics.
7. Resume physical motion at an inspectable rate or single-step h60. Co-rotating local geometry remains stable; look toward Luma/Solace when above the geometric horizon. Record their actual changing relative observations; do not demand visibility through the planet.
8. Change view orientation at fixed surface position; list/picking remain available with the planet centre offscreen. Hide borders/colours and assess normal-view popping/jitter.
9. Orbit/graze laterally near a seam, rapidly zoom out to Body Orbit/Whole System, reverse approach; monitor readiness/active cover/cache/caps with no cracks/holes.
10. Approach surface-enabled Luma; same identity/coherence rules. Return to Aurelia without scenario replacement.
11. Test temporary starved metadata budget/debug below/at/centre locations: no invalid LOD/crash; coarser valid surface and explicit diagnostic, optional underside view.
12. Resize wide/tall/high-DPI, minimize/restore/occlude/suspend/surface recovery, close normally. No stale view/clear-depth mismatch or hidden-clock catch-up.

Every prescribed checkpoint stays in one connected world. Capture issues/maxima, not only “looks smooth”. Ordinary default paths must show no cracks, holes, catastrophic pops, local jitter or incorrect body motion; debug starvation is an explicit quality-degradation test, not normal-view acceptance.

## 29. Objective numerical tests and tolerances

All ordinary tests are headless; use independent Cartesian/plane/ray/domain answers, not only the same helper twice. Require finite results before tolerance comparisons. Exact equality is appropriate for integer addresses/canonical shared packed bytes/unchanged world state, not every floating formula.

| Contract | Independent fixture and required assertion |
| --- | --- |
| Face mapping | Six face centres equal ±axes, axes U×V=N; dense dyadic sweep and non-grid samples have unit norm within 2e-14. Radius vertices within `max(1e-8 m,32*EPSILON*R)` at R≤1e8 m. |
| Shared edges/corners | All 24 directed edges, 12 undirected edges, 8 corners, levels 0/1/5/16/30 and parent-child matches: canonical q/direction/same-target position bits equal. Independent analytic edge vectors agree within 2e-14 direction /1e-7 m at R6.4e6. |
| Address/neighbor | Exact parent/child ordering/ranges/max-level rejection/overflow, all expected edge/reversal permutations and reciprocal lookups, same-level integer sweeps. No handle/cache/view input in address constructor. |
| Domain coverage | Four child half-open domains partition parent exactly using integer areas; complete small covers have no overlap/missing area, no ancestor+descendant leaves. Closed shared boundaries coincide. |
| Stitched topology | All 16 masks and corner combinations: nondegenerate outward triangles, signed integer/dyadic domain area exactly one, edge incidence and canonical boundary sequences; no overlapping triangle interiors/missing wedges. |
| Approximation error | Independent dense radial ray/triangle intersections plus barycentric sweeps stay within cached E +`max(1e-7 m,64*EPSILON*R)`; include stitched triangles and deepest numerical-floor cases. Error bounds never negative/NaN. |
| Bounds containment | Non-grid analytic cap samples and every stitched vertex/triangle barycentric sample inside ball +same body-scale allowance. Future synthetic extent ±1,000 m expands containment; no terrain generation. |
| Screen error | Compare analytic f64 projection/ray intersection versus triangulated geometry at H600/800/2160, FOV30/60/90°, tall/wide/offset views, clearance2m–1e11m. Actual discrepancy ≤computed bound +1e-6 px (before narrowing). Doubling f doubles error for unchanged geometry/depth within 1e-12 relative. |
| Hysteresis | Stationary settled path makes zero transitions over 1,000 identical updates; values at/between thresholds retain prior state, whole excursions split/merge once. Repeated input/state schedule yields identical address/mask bits. |
| Balance | Exhaustive shallow random-free covers, one-deep-cell/adversarial corner refinement to level20, all cross-face combinations: edge delta≤1, finite closure, exact forced count, no partial closure when starved. |
| Conservative horizon | Independent brute triangle front-face/visible-ray oracle: no rejected patch has a potentially front-facing triangle, with plane margin epsilon; all variants, grazing cases and R+2m. Interior disables rejection. Synthetic future bounds use valid guaranteed-opaque radius or disable test. |
| Conservative frustum | Independent triangle clipping and sphere/plane cases, grazing/near crossings/behind/off-axis/tall/DPI: rejected patch has no clipped triangle; boundary within epsilon survives. |
| Readiness/coverage | Delay one child/dependency indefinitely: parent remains, never 3/4 replacement. Budget 0/debug starvation and rapid reversal preserve complete cover, valid masks and bounded resources. |
| Handoff | Far/prewarm/surface/return has one opaque owner, no duplicate overlay ID, same R/frame/time/BodyId; ordinary before/after displacement≤0.35 px plus separately reported full projection arithmetic allowance. |
| GPU contract | 32-byte samples/64-byte instances/index ranges/alignment/little-endian padding, matching naga WGSL validation, default-device binding sizes, poison-on-partial failure. Shared drawn edge positions have identical bytes and normals agree within 2e-6 after narrowing. |
| Observer precision | CPU source-centred local deltas ≤1e-9 m in a shared local representation, body conversion ≤1e-7 m; final component error≤1e-5/1e-4/1e-3 m at100m/1km/10km and visible narrowing≤0.05 px. Emulate GPU projection allowance separately. |
| Tangent basis | Poles/near-poles/arbitrary directions: dot products and length error≤1e-12; E×N=up and camera basis determinant positive within1e-12; fallback deterministic, transported orientation continuous except documented ill-conditioned case. |
| Camera/frame invariance | Explicit inspection enter/re-expression pose residual≤1e-7 m and basis≤1e-12; any defined velocity-preserving migration≤1e-6 m/s body-local. Tracking change is tested separately, not called velocity preservation. |
| Identity/coherence | Rebuild fresh FrameIds leaves body/address/location unchanged; stale represented revision/time rejects; kinematic world revisions update view but reuse geometry metadata. |
| Read-only render policy | Copy BodyIds/properties/states/time/revision before/after topology/selection/LOD/culling/cache/handoff/render prep: exact unchanged values. No `&mut CelestialSystem` in rendering API. |
| Depth | Actual f32 depth quantization/order at stated ranges, ≤2e-6 near=1 error; positive/decreasing finite depths to star, near separations ordered as Section19; no second depth clear. |

### 29.1 Stress and motion fixtures

Use R≈6.4e6 m, body at ~1.5e11 m, observer clearance2 m, moon1e8–1e9 m. Add a stable 1 m body-local diagnostic line/grid beside the observer, never a procedural surface deformation. Sweep sphere seam/corner positions and independent body orientations.

Repeat local sample/observer configurations while a shared ancestor translates/rotates at 0/1.5e11/1e16 m, retaining local observer/content. Local preparation must remain invariant under common excluded ancestry within the Phase 1 envelopes. If observer and samples only share the body's million-metre basis, use the ≤1e-7 m body-scale budget, not the tighter regional promise.

Live world projection can test an astronomical body centre with observer on its own fixed frame; generic FrameTree stress can add a truly shared moving ancestor. Independent moon/star branches at1e16 m may have source quantization; those observations get angular/distant budgets. Never demand recovered millimetres after deliberately flattening through root. Include that loss as a negative precision test. Translating orbit observer legitimately sees rotating patch patterns; fixed inspection observer sees stable local patterns. No test “passes” by ignoring all body motion.

## 30. Scripted transitions and performance validation

Script explicit camera poses/control samples at fixed admitted navigation durations, with pause and real gravity/spin variants. Use the same production session/select/prepare functions:

- Radial approach/departure: log clearance1e11 m→2 m→1e11 m, includes every operator checkpoint, face centres/edges/corners, 30/60/144 Hz partitions.
- Grazing/horizon flight: tangent-looking near clearance2/100/10,000 m, bounded lateral path across several patch/face edges.
- Fast lateral orbit: rotate view/pivot around body at fixed100km/10km altitude; separate default translating versus fixed inspection.
- Threshold oscillation: 1,000 updates inside hysteresis deadband, then deliberate upper/lower crossings; counts prove no pathological thrashing.
- Rapid zoom/teleport: large desired jump and starvation/reversal, then hold camera until convergence. Parent coverage persists; coarse error and readiness latency remain visible.

Record per sample desired/balanced/active/ready/visible counts, split/merge/forced decisions, maximum projected error, seam/transition failures, new metadata, cache hits/evictions, upload bytes and max level. A normal converged view meets the configured error; normal ready transitions are ≤0.35 px before the separately bounded GPU projection arithmetic (≤0.45 px total). Emergency paths may miss desired error but never coverage/precision/identity invariants. Script output records deterministic count sequences; timings are observations, not correctness assertions.

### 30.1 Benchmark groups

Criterion remains CPU-only/outside ordinary CI, existing stable dependency. Preallocate warm scratch; black-box complete outputs; separate cold allocations/metadata construction from warm selection. Scenarios: tiny planet,100px,viewport-filling,low orbit,10km,100m,2m; R6.4e6, multiple radii/FOV/resolution, centre/edge/corner and normal/grazing orientations.

| Group | Record alongside timings |
| --- | --- |
| Mapping + bounds/error metadata | Grid8/16/32, regular/stitch envelope, visited records, cold construction and warmed reuse. |
| Selection/hysteresis | Candidates/desired/covering leaves, levels, splits/merges, reset versus warm steady view. |
| Neighbor closure | Forced splits/queries, overhead multiplier, cross-face and adversarial cascades; no hidden quadratic all-pairs leaf loop. |
| Horizon/frustum | Candidate/rejection counts, independent stages, bounds-only queries versus metadata miss. |
| Sample/instance preparation | Visible patches/samples, f64 source prepare versus evaluation/narrowing/packing/clipped fallback; bytes and draw buckets. |
| Cache | Hits/misses/evictions/pins/actual allocated bytes; orbit-back and rapid reversal. |
| Integrated rapid approach | Readiness delay/max achieved error/coverage/churn + normal explorer guides/history cost separately. |
| Native GPU | Surface draw count≤16/batch plus counted fallback, triangle/sample/upload bytes and actual frame/GPU durations when available; no device-startup timing masquerading as frame cost. |

Record CPU/OS/GPU/backend/compiler/profile/dependencies/glam features/revision, content rectangle/FOV/DPI/grid/threshold/budget, medians/distributions/confidence intervals and allocation evidence. Existing near-preparation and explorer measurements are comparators, not predicted Phase 4 results. Single-threaded synchronous work is the default; immutable inputs/outputs permit later worker generation without introducing Rayon/jobs now.

On the documented Windows desktop at1280×800, review if warm single-planet LOD+surface CPU preparation median exceeds **2 ms** or p95 exceeds **4 ms**, if cold refinement frequently stalls above4ms despite its operation budget, or if total normal integrated preparation exceeds the existing ~4ms explorer goal. These are measured review triggers in a60Hz/16.7ms budget, not hardware-independent pass/fail nanoseconds. At4K record workload scaling and agree headroom with actual GPU measurements. Revisit culling bounds, grid/batching/CPU evaluation only after evidence; do not weaken local precision or silently impose draw distance.

## 31. Deferred choices and pre-implementation recommendations

Deliberately deferred: terrain algorithms/normal fields/scale-band filtering, displaced geomorph transition topology, persistent body/layer/edit/candidate encodings, volumetric override mesher/seam ownership, automatic regional-frame lifetime, vertex caching/worker jobs/compute/indirect draws, stricter cross-platform bit determinism, final materials/lighting/texture domain, render ephemeris interpolation and depth partitioning absent a demonstrated failure.

Before implementation:

1. Review the face/child/edge/address convention, render versus permanent identity distinction and shared-boundary precision contract together.
2. Resolve or explicitly review old platform/operator gaps, keeping their missing evidence open. This design is not implementation permission or prerequisite acceptance.
3. Prove all16 stitch variants and conservative plane/error/horizon mathematics headlessly first; cache/LOD can then rely on verified metadata.
4. Measure counts and cold/warm bounds at centre/edge/corner/grazing views before committing GPU layouts; adjust only with recorded rationale.
5. Validate shared reverse-Z and clipped narrowing with metre/horizon/moon/star ranges before native appearance is treated as acceptance.
6. Keep explicit co-rotating inspection distinct from ordinary system-stationary Free Flight and default translating Body Orbit.

## 32. Explicit exclusions

Phase 4 does not implement procedural terrain height generation, Perlin/Simplex/fBM terrain, mountains, erosion, rivers/hydrology, terrain editing, voxel/SDF caves, tectonics, biomes, trees/vegetation/grass/rocks, oceans, atmosphere/clouds/weather, settlements/gameplay, collision/walking physics, spacecraft, final materials/textures or physically based planetary lighting.

Prefer the perfectly smooth sphere for every validation. Only if the seam/morph architecture cannot be validated otherwise may a deliberately trivial, bounded analytic diagnostic deformation be proposed and reviewed; it must be labelled/test-only, have explicit error/bounds and never become terrain generation. The selected no-morph Phase 4 strategy does not require it.

## 33. Objective Phase 4 Definition of Done

All boxes remain open. This design/audit completes none of the implementation acceptance below. Unavailable platform evidence stays open.

### Connected capability and ownership

- [ ] Aurelia or an equivalent existing fixture body orbits under Phase 3 gravity, rotates, is selectable/focusable and renders through the surface LOD path; its moon continues independently.
- [ ] Continuous system/astronomical→orbit→kilometre→~2 m approach/departure passes the prescribed operator/script sequence in one connected world, with no scene transition/duplicate authoritative planet/teleport.
- [ ] Same BodyId/state/reference radius/coherent instant throughout; surface render selection mutates no world state, frame ancestry or global origin.
- [ ] Body-fixed source-centered preparation meets stated near/body/shared-ancestor precision; a 1 m local diagnostic remains stable under astronomical translation/spin.
- [ ] Surface Inspection supports local look/horizon/moon/star observations while moving with the rotating body; default orbit/free-flight semantics remain explicit.

### Geometry, visibility and lifecycle

- [ ] Deterministic checked patch addressing, full hierarchy coverage and all 24 directed edges/8 corners/16 stitch variants pass independent tests.
- [ ] Smooth-sphere approximation error/bounds/screen metric are conservative; hysteresis and active sets deterministic for explicit state/schedule inputs.
- [ ] Edge-neighbor restriction holds cross-face; surface is crack-free without skirts, complete ready sibling replacement/balancing creates no holes.
- [ ] Ordinary transitions meet ≤0.35 px geometry/narrowing and ≤0.45 px full projected bounds with no conspicuous normal-view pop; deferred displaced morph insertion/compatibility requirements documented.
- [ ] Conservative frustum/horizon culling handles grazing/near/inside states without false rejection or undefined LOD.
- [ ] One shared reverse-Z view supports near geometry/horizon/moon/star with measured depth/vertex precision; no erroneous second clear or hidden range cutoff.
- [ ] Far celestial/surface readiness responsibility integrates with observation/picking/marker fade, exactly one opaque owner/body and no centre-marker substitute for ordinary near surface.
- [ ] Shared grid/index topology and batched draws/resource counts verified; cache/staging/GPU/scratch remain bounded with explicit no-hole budget degradation.
- [ ] Debug borders/LOD/face/address/readiness/culling/tangent/normal metrics available alongside a normal smooth-sphere view.
- [ ] No procedural terrain, editing, atmosphere or other excluded scope has entered implementation.

### Objective evidence, performance and quality

- [ ] Complete headless topology/error/culling/precision/identity/ownership/numerical test matrix passes with explicit tolerances; all previous tests/modes remain working.
- [ ] Radial/departure/grazing/lateral/threshold/rapid/starvation/reversal scripted paths record counts/error/churn/coverage and satisfy normal/emergency contracts.
- [ ] CPU benchmark groups and native GPU/draw/upload/memory observations are recorded with hardware/workload/distributions and reviewed trigger misses; no fabricated universal FPS guarantee.
- [ ] Stable Rust/Rust 2024, six publish=false crates, no project unsafe/cycles/unrelated dependency upgrades.
- [ ] `cargo build --locked --workspace` passes.
- [ ] `cargo check --locked --workspace --all-targets --all-features` passes, including benchmarks.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test --locked --workspace --all-features` and focused math/renderer/app/domain release checks pass; previous long-run gravity envelopes remain verified.
- [ ] `cargo doc --locked --workspace --all-features --no-deps` passes with `RUSTDOCFLAGS=-D warnings` in the native shell.
- [ ] Windows native visual/operator/high-DPI/lifecycle sequence passes with OS/GPU/backend/profile evidence.
- [ ] Linux native build/headless/release and visual/operator/lifecycle sequence passes with corresponding evidence.
- [ ] Current implementation-revision remote Linux-quality/Windows-compatibility CI passes through normal workflow; compile CI is not native visual evidence.
- [ ] Validation/performance/architecture/roadmap/README status and ADR 0006 record actual topology/LOD/crack/handoff/ownership decisions, measurements, deferred choices and outstanding prior gates accurately.
- [ ] `git diff --check`, UTF-8/LF/newlines/links/fences and intended change scope pass; correctness, architecture and performance reviews completed before Phase 5 expands scope.

## 34. Implementation record — 2026-10-02

The earlier audit/file plan is historical design evidence. The subsequent authorized
implementation adds checked math/address/canonical sample/tangent APIs, complete
flat balanced hysteretic coverage, metadata/error/culling/readiness, source-centred
f64 preparation, shared storage/index instancing, exclusive app handoff and explicit
Surface Inspection in the same physical hierarchy. No procedural displacement or
Phase5 work was introduced.

Concrete APIs and the small private `cover.rs`/optional integrated native route are
recorded in ADR0006. Local closure transactions replaced a cache-churning global
proposal; direct edge queries fixed hypothetical-merge rescanning. Exact compact
temporary sharing arrays are body-batch scoped. Counts/errors/bytes, numerical maxima,
native observations and CPU review misses are in the validation/performance records.
The selected mapping/grid/stitches/thresholds are preserved. Unperformed combined
operator/platform/profiling criteria above remain unchecked; implementation is not
blanket acceptance or permission to begin Phase5.
