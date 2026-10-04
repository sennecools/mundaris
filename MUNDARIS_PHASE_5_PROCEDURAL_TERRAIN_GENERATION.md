# Mundaris — Phase 5: Procedural Terrain Generation

> **Status:** implementation-ready design; no Phase 5 engine implementation.
>
> **Audit baseline:** 2026-10-02, HEAD `3975336f3f4930c30a90149a53ec59fa9a528dcb`, including the current working-tree guidance. Existing workflow changes are preserved.
>
> **Sequencing decision:** Phase 5 development is permitted with Phase 4 architecture frozen. Outstanding Phase 4 platform/operator acceptance must be completed before a later release-quality milestone. The approximately 11 ms full-view preparation target miss is an accepted, measured Phase 4 limitation, not an architectural failure or permission to weaken precision/quality.
>
> **Targets:** stable Rust/Rust 2024, existing six crates, native Windows/Linux.
>
> **Prerequisites:** [bootstrap](MUNDARIS_PROJECT_BOOTSTRAP.md), [engine design](MUNDARIS_ENGINE_DESIGN.md), Phases [1](MUNDARIS_PHASE_1_REFERENCE_FRAMES.md), [2](MUNDARIS_PHASE_2_CELESTIAL_MODEL_AND_TIME.md), [3](MUNDARIS_PHASE_3_GRAVITY_ORBITS_AND_CELESTIAL_RENDERING.md), [3.5](MUNDARIS_PHASE_3_5_CELESTIAL_NAVIGATION_SYSTEM_VIEW_AND_TIMEWARP.md), [4](MUNDARIS_PHASE_4_PLANET_SURFACE_REPRESENTATION_AND_LOD.md), ADRs [0002](docs/adr/0002-reference-frames-and-precision.md) through [0006](docs/adr/0006-planet-surface-topology-and-lod.md), [architecture](docs/architecture.md), [invariants](docs/engine-invariants.md), [coding standards](docs/coding-standards.md), [performance](docs/performance.md), and Phase 4 [profiling](docs/phase-4-profiling.md)/[validation](docs/phase-4-validation.md).

## 1. Outcome and scope

Phase 4 proved a smooth sphere, planetary LOD and connected system-to-surface navigation. Phase 5 adds a convincing, deterministic hierarchy of broad landforms, mountain systems, regional relief, hills, ridges, valleys and resolved local detail:

```text
body-fixed location + immutable terrain definition + explicit footprint
    → filtered height / derivatives / conservative certificates
    → reusable body-fixed patch geometry
    → balanced ready cover + crack-free visual transitions
    → current observer-relative f64 preparation → checked f32 → GPU
```

The final finite-band terrain definition is independent of rendering. Filtering and morphing are approximations of that definition, not changes to the world. Generated meshes remain disposable.

**Excluded:** editing/sparse modifications; voxel, cave, overhang or SDF meshing; tectonic simulation; evolving hydraulic/thermal erosion; rivers/hydrology; oceans; a beach system; biomes/climate; vegetation/trees/grass; rocks as objects; atmosphere/clouds/weather; settlements/roads/buildings; gameplay; collision/walking physics; final materials/textures/PBR. Future compatibility is specified only at boundaries, not as implementations or empty frameworks.

## 2. Audit: actual Phase 4 insertion points

Historical audit narratives in earlier phase specifications describe earlier source states; their subsequent implementation records and the following current source are authoritative for this design.

| Current file / symbol | Implemented behavior | Phase 5 insertion |
| --- | --- | --- |
| `crates/math/src/surface.rs`: `CubePatchAddress::sample_key`, `CubeSampleKey::direction` | Reduced dyadic signed Cartesian cube tuple, fixed normalization order; matching faces/levels share canonical directions | Keep unchanged as patch batch sampling coordinates; evaluate terrain at the resulting body-fixed direction |
| Same file: `SurfaceLocation`, `face_uv`, neighbor/address/tangent helpers | Location is an independent checked direction; charts and render addresses are not world identity | Reuse location in pure world queries; add only generic footprint/cap helpers with real callers |
| `crates/renderer/src/planet_surface/prepare.rs`: `SurfaceStaging::append`, sample loop | Forms `direction * radius` in f64, calls prepared source `view_displacement`, rotates radial normals with `view_direction`, packs sample32 | Consume pre-generated body-fixed positions/normals here; no procedural algorithm in renderer |
| Same file: boundary key collection / `boundary_samples` | Shares final position/normal bytes within one source/radius append batch; every duplicate still executes initial sample math | Extend sharing to reconciled displaced/morphed boundaries; cache generation separately, share the current-view conversion once |
| `planet_surface/bounds.rs`: `PatchMetadata::build`, `error_unit`, `projected_error` | Dimensionless all-stitch sphere error, cap ball and geometric-plane envelope, numeric floor | Preserve sphere metadata; add world-provided terrain certificates and project an explicit total error in metres |
| Same file: `SurfaceExtent`, `ball`, `horizon_reject` | Extent already expands balls; `smooth()` has zero displacement and opaque radius zero; nonzero displacement disables existing horizon rejection | Pass real height bounds; implement separately certified terrain occlusion, otherwise conservatively disable it |
| `planet_surface/lod.rs`: `relevance`, `visible_for`, `SurfaceLodSession::update` | Hardcodes `SurfaceExtent::smooth`; six ready metadata roots, flat cover, hysteresis, balance, local atomic closure, merge checks | Supply bounds/error/readiness through a domain-free derived input; gate both split and merge on generated geometry/transition readiness |
| `planet_surface/cache.rs`: `MetadataCache` | Sorted compact metadata-only LRU, pins/access sequence; no height/mesh retention | Keep independent; add bounded generated geometry cache outside authoritative terrain |
| `planet_surface/gpu.rs`: `PlanetSurfaceRenderer::upload/draw` | Current-frame sample/instance/fallback buffers, 16 index buckets, growth waits for prior submissions; 80 MiB cap | Keep baseline uploads/layout/depth; temporary morph overlay uses a counted triangle stream, not per-patch persistent GPU allocation |
| `crates/app/src/planet_surface.rs`: `PlanetSurfaceSession::update`, `return_to_far_if_ready` | App BodyId capability and exclusive Far/Prewarm/Surface ownership; far error currently sphere-only | Orchestrate pure world generator, cache/work, certificates and terrain-aware far error without changing handoff ownership semantics |
| Same file: `hovered_patch`, `SurfaceInspectionAnchor`; `celestial_camera.rs` | Hover intersects reference sphere; anchor/navigation guard are reference-sphere based | Label hover as approximate; add terrain elevation/clearance diagnostics and navigation-only guard, not collision |
| `crates/world/src/{body,system,frame_projection}.rs` | Runtime namespaced BodyId, one reference radius, coherent motion, disposable separate translating/fixed frames | Attach optional immutable terrain definition through world-owned state, separate terrain revision; no change to gravity or frame identity |

The current dependency graph is app → math/world/simulation/renderer, world → math, simulation → math/world, renderer → math. Core has no domain API. Preserve it: app adapts world terrain outputs to renderer-owned domain-free geometry/certificate records. No renderer → world dependency and no new crate.

**No contradiction requiring a Phase 4 architecture change was found.** Frozen: normalized radial mapping, computed addressing, independent locations, complete flat balanced cover, screen-space error/hysteresis, one-level edge balance, 16 stitched variants, source-centred precision and celestial handoff semantics. Terrain certificates, readiness and temporary geomorph representation extend the intentionally deferred terrain seams; they do not replace those decisions.

### 2.1 Evidence, not new measurements

The closeout source and harnesses confirm the quoted baseline. `renderer/benches/planet_surface.rs` settles each radial view before timing preparation; its cold group times one initial update, not whole convergence. `app/benches/planet_surface_approach.rs` times one real N3/h60 commit, projection, selection and preparation, excluding guides/history. `planet_surface_profile.rs` separately measures short CPU stage distributions under `surface-profile`.

The original full-view scenario is **10,000 km reference clearance, 587.304 px diameter**, not literally viewport-filling: 510 covering / 402 visible patches, level 4, 116,178 samples, 204,800 triangles, nine draws, 3,743,424 payload bytes. The added 6,400 km clearance scenario fills 800 px and has the same sample count. Accounting-corrected closeout preparation median/p95 is **10.7824/11.3209 ms**; evaluation/conversion/narrowing stage median is **9.1432 ms**. This is CPU-only evidence, not GPU/presentation timing. Near downward 2 m has four patches/1,156 samples; horizon views cost more. Terrain changes counts and slopes: do not assume those sphere counts persist.

## 3. Terrain identity, configuration and version

World owns an optional `TerrainDefinition` on a body, separate from physical `BodyProperties`/`BodyState`. It contains an immutable validated configuration, `TerrainSeed(u64)`, explicit `TerrainGeneratorVersion::V1` (conceptual name `mundaris-terrain-v1`), and an explicit `TerrainIdentity(u64)` supplied by the fixture/world authoring boundary. Identity is stable generation salt, **not** the existing runtime BodyId namespace/index. Two bodies may intentionally share a definition/identity/seed to reproduce terrain; normal fixture planets receive distinct explicit identities. Names never seed terrain.

Layer seeds use a specified fixed integer mixer, not `DefaultHasher`: wrapping SplitMix64 finalizer steps `(x ^ (x >> 30))*0xbf58476d1ce4e5b9`, `(x ^ (x >> 27))*0x94d049bb133111eb`, `x ^ (x >> 31)`, with explicitly ordered tagged combinations of seed, terrain identity, version code, band tag and octave index. Tags and combination constants are part of V1's algorithm contract. No random stream consumption depends on sample/request order.

`TerrainRevision` is checked monotonic runtime coherence metadata changed only by an accepted terrain-definition edit. Generator **identity** is version + seed + explicit terrain identity + complete validated config values + reference radius where evaluation uses it. Revision prevents stale result publication; it is not the world identity and is not a generator version. Restoring the same definition restores numeric terrain even if revision increases. Cache keys use exact definition identity or an interned immutable definition with equality verification, never an unchecked hash alone. Normalize signed zero; reject NaN/infinity. Persistence encoding/body resolver is deferred; neither FrameId nor current BodyId is promised to survive save/reload.

Configuration has five band records (amplitude in metres, longest/shortest wavelength or angular mode, octave count), continent bias/contrast, mountain coverage/strength, warp fraction and ridge softness. Fixed V1 amplitude falloff is 0.5 and lacunarity is 2; do not expose hundreds of knobs. Bands can be disabled with zero amplitude. Global amplitude envelopes are calculated from the actual finite sums and transforms, not guessed from UI slider limits. A sea-level hint may be absent; zero height already has an unambiguous future sea-level datum.

Definition/radius editing validates the combined legal radius and derived certificate inputs before commit; reject transactionally otherwise. Radius change invalidates generated terrain geometry and metre-domain evaluations even though Phase 4 dimensionless metadata remains reusable. Name/mass/time/translation/orientation/frame rebuild changes do not invalidate terrain. Terrain-only edits must not rebranch orbital history or mutate celestial kinematics.

### 3.1 Deterministic guarantee

Same finite definition, canonical location and explicit footprint yield same height/gradient bits on the same executable/target, including scalar/batch, cache eviction, request order and scheduling. Fixed operation order; no optional fast-math reassociation or separate SIMD formula silently changing results. Windows/Linux and debug/release satisfy the numerical envelopes in Section 17; cross-platform floating-point bits and GPU pixels are not promised. Integer seed/hash results are exact across supported targets. An algorithm, transform, constants, band order or filtering semantic change that changes terrain requires a new generator version, not a Git hash. Unsupported versions return an error rather than silently using latest.

## 4. Coordinate domain and height semantics

Authoritative domain is **body-fixed unit direction**, with reference radius and metre/angular scale configuration. For metre-based band wavelength `lambda`, sample a rotated/translated 3D noise domain `x = Q * direction * (R/lambda) + offset`; Q and a bounded offset are deterministically derived once per band. V1 uses one fixed documented coordinate-to-noise frequency convention, so lambda is characteristic wavelength, not a guarantee of a single Fourier frequency. Macro angular mode uses dimensionless cycles per body, explicitly separate from metre mode. Doubling R does not double metre features; angular macro features intentionally span comparable fractions of a body.

```text
height > 0 : outward from reference sphere
height < 0 : inward from reference sphere
p_body = direction * (reference_radius_m + height_m)
```

One authoritative reference radius remains. Derived minimum/maximum radial envelopes are bounds, not additional competing physical radii. V1 configuration requires finite bounds and `R + global_min_height > max(1e-3 m, 64*EPSILON*R)`. The initial supported ruggedness envelope additionally requires `max(abs(h_min),abs(h_max)) <= 0.1 R`; reject beyond it rather than clamping samples. This is a validated initial generator envelope, not an engine-wide prohibition on future shapes.

| Domain option | Decision |
| --- | --- |
| Global 3D noise restricted to directions | Selected: continuous at all cube edges/corners, no poles, differentiable primitives and straightforward CPU batching |
| Independent face-local 2D noise | Reject: discontinuities/rotated patterns and chart-dependent truth; UV allowed only for batching |
| Spherical harmonics / spherical regional fields | Good low-frequency alternatives; no face seams, but more machinery/oscillatory basis control than V1 needs |
| 3D domain warping | Selected, bounded and only at macro/range scale; continuous across faces, derivative/Jacobian bounds required |

No authoritative face-specific rotation, face seed or patch UV. Canonical grid points provide exact inputs; nearby non-grid point queries use the same continuous function. A floating direction is a coordinate, not a hashable vegetation/edit identity.

## 5. Small deterministic noise basis

Select **one 3D gradient-noise primitive** with a fixed gradient table, hashed integer lattice corners, quintic interpolation `6t^5-15t^4+10t^3`, analytic first derivatives and private second-derivative bounds. Hash full signed lattice indices; no 256-entry repeating permutation period. Validate finite domain/cell range before conversion to integer. Eight corner dot products per noise evaluation; use a fixed documented order. Scale by a proven envelope to keep output in [-1,1], rather than assuming an empirical range. Derivatives are of the same expression, including domain rotations/warps.

| Candidate | Assessment |
| --- | --- |
| Perlin-style gradient noise | Choose: modest implementation, analytic derivatives, C2 interpolation and auditable bounds; lattice bias addressed by seeded rotations and structured masks |
| Simplex/OpenSimplex | Potentially fewer contributions/isotropy advantages; extra region/gradient conventions and source/license review, not a second V1 basis without measurements |
| Value noise | Cheap but smooth lumps/block-like derivative structure; unnecessary alongside gradient noise |
| Cellular/Worley | Useful feature-distance macro structure but expensive neighbor searches and derivative cusps; defer rather than add redundant primitive |
| Ridged transform / warp | Compose the selected primitive; not independent noise libraries |

Implement from mathematical definitions; any third-party implementation proposed later needs permissive commercial-use license/source review. No noise/random dependency is pre-approved. Hashing is integer exact; noise floating arithmetic has Section 3.1's guarantee. Future SIMD/GPU backends must validate against this reference rather than become authoritative silently.

Use smooth ridge magnitude `abs_e(x)=sqrt(x*x+e*e)` with positive fixed/configured softness and range-normalized ridge transform. Do not use a raw `abs` cusp while claiming a finite Hessian. Product/smoothstep/warp derivatives and certificates are propagated through the same explicitly bounded expression DAG implemented as concrete modules, not a public node-graph engine.

## 6. Explicit multi-scale architecture

The generator sums independently seeded, replaceable **additive output bands**. Masks/control fields are immutable functions of location; resolving another band must not reinterpret an earlier mountain or continent. Each band includes its own control modulation and warp when its effective derivative scale is certified. A finer band cannot feed back into coarse band parameters.

Earth-sized fixture starting ranges (artistic starting values, not measured quality or fixed engine limits):

| Band | Characteristic wavelengths | Typical output envelope | Function / independent seed |
| --- | --- | --- | --- |
| Macro | 4,000–12,000 km, optionally 1–4 cycles/body | -4,000 to +2,000 m | Broad masses/basins/lowlands; `Macro` seed |
| Range | 200–1,600 km | 0 to +5,000 m | Mountain likelihood, coherent broad uplift and range chains; `Range` seed |
| Regional | 8–128 km | approximately ±1,200 m | Peaks/plateaus/ridge-valley structure modulated by fixed range controls; `Regional` seed |
| Local | 128 m–4 km | approximately ±120 m | Hills/gullies/ridges, varying relief masks; `Local` seed |
| Fine geometry | **8–64 m** | approximately ±2 m | Small resolved surface variation; `Fine` seed |

Actual amplitude certificates come from config finite octave sums and mask ranges. No centimetre geometry; wavelengths below **8 m** are outside V1 geometric scope and can later become material normals/objects. Planet presets may change all amplitudes/wavelengths; tests include radii 1e4, 3.74e5, 6.371e6 and 1.2742e7 m with legal relief. Small bodies need explicitly sensible macro/angular presets, not Earth constants copied unchanged.

### 6.1 Continents and basins

Use two or three very-low-frequency rotated 3D fields, unequal weights and a bounded low-frequency vector warp. Apply a smooth biased contrast remap to a broad continental control value; combine a separate broad uplift/lowland field. Signed macro elevation supports broad negative basins without water. Bias affects relative elevated-region coverage, not a shoreline subsystem. Unequal wavelengths, separate uplift and warp avoid evenly spaced identical blobs. Macro angular/physical configuration is explicit; no seeded per-face blobs or mandatory Voronoi plates.

V1 chooses warped low-frequency gradient fields over cellular regional sites or harmonics for minimal basis/batch reuse. If seed surveys still show noise soup, change the macro algorithm under a new version or a reviewed V1 pre-release tuning change; do not compensate with extra high-frequency noise. Report quality against several seeds, not only one favorable view.

### 6.2 Mountain systems, regional and local structure

A broad continuous mountain mask is a smooth threshold of two range-scale controls, biased by macro uplift but not simply every positive continent. A warped anisotropic ridge field (seeded 3D rotation plus unequal axis scales) inside that mask produces elongated ranges. Positive range uplift, smooth ridged regional contributions and bounded negative valley terms create high relief with lower surrounding terrain. Fixed range controls gate regional/local amplitude, so plains and mountains differ without every patch using a planet-wide ruggedness estimate.

Use 2–4 finite octaves per band initially, fixed falloff, with independent tags and bounded warp. Apply smooth powers/remaps to emphasize narrow ridges and broaden valley floors; no hard discontinuity, unbounded exponent or discontinuous cellular nearest-feature switch. All modulation derivatives count toward effective frequency and error certificates. Future ridge-network/tectonic boundary fields can replace the range control inputs, not the renderer.

### 6.3 Gradient shaping / fake erosion decision

Include **stateless erosion-inspired ridge/valley remapping**, not stateful erosion. Analytic derivatives supply slopes/normals/debug evidence. Serious comparison:

| Method | Cost / integration consequence | V1 decision |
| --- | --- | --- |
| Derivative noise | One value+gradient evaluation instead of multiple neighbors; chain rule auditable | Required baseline |
| Finite-difference slope/normal | 4–6 extra full field queries, step-size and face-seam hazards | Independent oracle only |
| Slope-dependent amplitude attenuation | Can smooth steep fine detail; differentiating it needs Hessians, bounding resulting curvature needs higher derivatives | Defer default algorithm; focused optional experiment after sample-cost baseline, no acceptance dependency |
| Warped gradients | Correctly sharpen/place ranges with bounded warp Jacobian | Required chain-rule handling, limited macro/range warp |
| Ridge/valley algebraic remaps | Cheap, C2 soft ridges, controllable relief and replacement boundary | Include as initial fake-erosion approximation |
| Hydraulic/thermal iteration | Neighborhood state, expensive global context/streaming semantics | Excluded |

Do not claim drainage or erosion simulation. A slope-derived experiment may only enter the versioned definition after independent derivatives/bounds and matched quality/cost evidence; visual benefit alone cannot bypass error certification.

## 7. Physical sampling footprint and filtering

`TerrainFootprint` is a validated nonnegative **maximum adjacent sampling distance in metres** at the reference sphere, not camera distance or quadtree identity. Zero requests the complete finite-band field. Arbitrary point queries supply an explicit footprint; queries outside rendering do not need a camera.

For the fixed grid and normalized radial map, a reproducible conservative patch profile is:

```text
rho(level) = R * 2 / (16 * 2^level)
```

The mapping Jacobian norm is at most one, so this bounds a grid step's arc length throughout every face. It is intentionally face-independent: equal-level seams get identical filtering. Record local Jacobian/edge spacing for diagnostics, but do not independently change filter weights per patch to exploit distortion in V1. World batch evaluation receives physical rho, not level; renderer/app derives the profile. Stitched interpolation uses larger triangle diameters in its error certificate, not a secret octave cutoff tied to mask.

For a band/octave with certified **effective wavelength** `lambda_eff` (including warp stretch and modulation), choose:

```text
s = lambda_eff / rho
w = 0                     when s <= 4
w = smoothstep(4, 8, s)   when 4 < s < 8
w = 1                     when s >= 8
```

At rho=0 all permitted finite bands have weight one. Effective scale uses a conservative derivative/stretch factor; no warp can smuggle an unresolved sharp ridge into a supposedly broad band. Derivative envelopes, not a nominal base octave label, determine this factor. V1's two-times-Nyquist safety margin is deliberately more conservative than a 2-rho cutoff.

**50 km/sample:** wavelengths <=200 km contribute zero; 200–400 km fade in; >=400 km are fully allowed. Metre detail is not evaluated. **2 m/sample:** <=8 m contribute zero, 8–16 m fade in, >=16 m fully contribute. The complete 8 m octave becomes fully available at rho<=1 m. A 2 m sample is not permission to add centimetre features.

Compile the active octave list once per profile/batch. Zero-weight output bands are skipped, including their private controls; controls needed by an active coarse band are reused once per sample. Query diagnostics count actual primitive evaluations, including warps/masks, not merely advertised height octaves. Renderer steady reuse invokes no field queries.

### 7.1 Meaning and limits of filtering

This is a smooth scale-truncated procedural approximation, **not an exact convolution filter or a proof that nonlinear noise is band-limited**. Quintic lattice noise/remaps have spectral tails; soft ridges and bounded derivatives prevent arbitrary sharp features. The displacement difference from full terrain is certified as the sum of omitted/attenuated band envelopes (Section 9). Use known waves and high-frequency adversarial fixtures to measure aliasing/shimmer, alongside derivative bounds. If a selected remap creates unacceptable harmonics, broaden softness/increase effective stretch or replace it; do not ignore them.

Abrupt octave cutoff is rejected. Smooth weights are continuous for arbitrary rho. Discrete parent/child profiles still differ: temporal geometry morph handles that change, separately from the field's scale filtering. Analytic convolution is desirable for specific future primitives but not available for the whole warped V1 expression. Mip-like height tiles/precomputed fields remain an optimization alternative only if expensive reused macro evaluations justify them; no global high-resolution map is generated.

Shared low-frequency terms with weight one are bit-identical at the same canonical location, even if finer bands become active. Terms within the cutoff taper have explicitly different weights; that difference is bounded, morphable and must not be falsely called a changed mountain definition. Tests compare individual band terms and the common low-frequency sum, not incorrectly require all filtered totals to agree.

## 8. Normals and derivatives

World's sample returns height and **body-axis tangent derivative with respect to unit direction**, `g = (I-n n^T) * grad_direction(h)`, in metres per unit-direction change. Physical reference-sphere slope vector is `g/R`; slope angle of the displaced surface is `atan(|g|/(R+h))`. Do not confuse a domain-space noise gradient, metre-domain slope and direction derivative.

For radial graph p=(R+h)n, use:

```text
normal_body = normalize(n - g/(R+h))
```

Derive all active weighted terms, smooth masks, anisotropic domains and warp Jacobians analytically. Patch profile weights are constant during a query, so their spatial derivative is zero. If later spatially varying footprint is introduced, differentiating weights becomes mandatory. Allocate nothing for point evaluation. Value-only internal calls may omit gradient arithmetic when bounds/debug callers truly do not need it; renderer batch requests value+gradient together.

Normals for unconstrained final samples follow this formula. Stitched constrained edge normals are copied/interpolated from the same coarse owner's edge record; equal-level edges/corners share records. Interior normals smoothly blend back over two sample rows. During morph, interpolate old/new shading normal vectors and renormalize with an explicit nonzero/outward check. These **visual transition normals** are not advertised as exact physical derivatives of a piecewise-linear mesh. Field queries always return their analytic terrain normal. Mesh-triangle normals remain an independent debug/error oracle, not the default lighting normal. Basic elevation/slope shading is sufficient; no PBR.

## 9. Displacement and terrain approximation certificates

### 9.1 Regional bounds without generating a mesh

For each output band, provide regional value interval, tangent/Cartesian derivative envelope and chart-composed second-derivative bound. V1 computes conservative interval bounds over a directional cap converted to bounded 3D domains: lattice corner dot/interpolation bounds plus interval propagation through masks, remaps and warps. Use fixed bounded subdivisions (initially at most eight interval subregions per query); wide domains fall back to analytic global primitive envelopes, never enumerate all noise cells on a continent. Outward arithmetic margins cover roundoff; a failed tighter certificate falls back to a valid loose one or disables refinement optimization, not to a small guessed error.

Height bounds sum intervals of all configured bands, **including unresolved bands**. These support future queries, pre-generation visibility and handoff. Multiplicative masks also give local residual/derivative envelopes: a cap proven outside a mountain mask can have zero mountain error; uncertain caps use a larger bound. Cache compact certificates keyed by immutable definition/radius + region/profile, separate from view/frame revision. No dense terrain evaluation just to cull a child. Bounds queries and primitive evaluations are timed separately from generation.

Post-generation sampled extrema alone are not safe tighter bounds. Tightening requires sampled extrema plus certified variation over unsampled cells, or intersection with the existing regional certificate; apply this to all allowed stitched/morphed triangles too.

### 9.2 Safe approximation error

Selection must bound **full finite-band terrain versus the actual rendered approximation**, not just filtered samples versus each other. Let n(u,v) be the frozen cube mapping, F(u,v)=h_filtered(n(u,v))*n(u,v), and D the maximum Euclidean face-domain diameter of triangles in the actual allowed stitch/transition topology. Define M explicitly by `||D²F(x)[a,b]||₂ <= M ||a||₂ ||b||₂` throughout each triangle (or an equivalent certified Lipschitz bound on DF for piecewise-C2 functions). For any domain point x, use its actual nonnegative triangle barycentric weights alpha_i: `x = sum_i alpha_i v_i`, `sum_i alpha_i = 1`. Expanding each F(v_i) about x therefore cancels the linear term exactly: `sum_i alpha_i (v_i-x)=0`. Each remainder is bounded by `0.5 M ||v_i-x||²`, so the vector interpolant has error at most `0.5 M sum_i alpha_i ||v_i-x||² <= 0.5 M D²`. Weights from another parameterization do not satisfy this proof. Validate the derivation with independently evaluated affine/quadratic fixtures before using it for LOD.

```text
E_filtered_interpolation <= 0.5 * M * D^2
E_unresolved <= sum_b (1 - w_b) * regional_absolute_band_bound_b
E_total <= E_sphere(R) + E_filtered_interpolation + E_unresolved
           + E_boundary_constraint + E_morph_remaining + E_numeric
```

The formula bounds vector position at the same domain parameter, hence supplies a conservative correspondence/Hausdorff upper bound. Include height-direction coupling in M (derivatives of h*n), not height curvature alone. Sphere error retains Phase 4's all-stitch bound/floor. Regular D=sqrt(2)*grid_step; existing masks have measured D<=sqrt(5)*grid_step, with Phase 4's 4-step analytic bound available. A transition overlay uses its measured domain diameters, never the regular-grid bound blindly.

If M is unavailable/too loose, a first-derivative Lipschitz bound `L_F*D` is safe. An independent height interval gives `range(h_filtered)+H_abs*max_direction_chord` for the displacement-vector interpolation term; take the minimum only of independently valid bounds. Never infer curvature/error solely from a few sampled extrema or slope average. Region bounds for masks/derivatives let flat regions refine less than ranges. Global worst-terrain error is only the provisional fallback when a local certificate is pending.

`E_boundary_constraint` is the maximum displacement between unconstrained fine samples and the reconciled target, propagated through convex interpolation; compute it at target vertices, with field interpolation/residual already covered above. `E_morph_remaining` is `(1-m)*max|P_old-P_new|` on the common refinement (Section 11), certified over its vertices because the difference is piecewise affine there. Include error on both endpoints/cells and any numeric conversion allowance; report target-final error separately from currently displayed error.

Project the total metre error with the existing conservative `bounds.rs::projected_error` formula and expanded ball. Preserve split0.125/merge0.0625 physical px and equality/hysteresis behavior. Large/infinite projected error requests refinement; caps cause explicit quality debt, not falsified certificates. Minimum wavelength is finite; do not endlessly subdivide past numeric/resolution floors. Terrain tail/curvature may cause many more patches than smooth sphere; inspect local certificate conservatism before proposing any quality policy change.

Independent fine-reference samples are regression checks, **not a proof** of a bound. Primitive/chain-rule interval reasoning supplies the certificate. Required test sweep in Section 17 must find no under-bound, even in pathological masks/warps/ridges. A discovered under-bound blocks implementation integration until corrected.

## 10. Displaced seams: canonical ownership

Terrain query truth remains location/profile dependent, not neighbor dependent. Render boundary reconciliation is derived state over the **actual ready cover**, never the desired unready cover.

1. Every regular generated patch retains unconstrained 289 samples at its own physical profile.
2. Create a canonical boundary record per body/definition, reduced dyadic key and current boundary ownership/profile. Pick the **coarsest incident active leaf** at a drawn vertex; tie by frozen address order. Include all incident faces at cube corners and patches meeting at a vertex, not only one edge query.
3. A stitched fine edge uses coarse-owner filtered heights/positions/normals at its drawn even vertices. The existing odd-vertex collapse remains; unused odd samples do not open a second fine curve. Coarse and fine share the exact same displaced chord segments.
4. Equal-level patches use the same profile/location. A corner incident to a coarser third patch obeys that coarsest corner record on every participating patch.
5. Blend the constrained boundary displacement/visual normals toward unconstrained fine samples over the first two interior rows using fixed smooth weights. This avoids a sharp fine-detail wall at the edge. The resulting mesh is derived, not a new terrain query function; its residual belongs in E_boundary_constraint.
6. Share observer-relative position/normal packing once per canonical current boundary record, body-batch scoped as today. Never share a transformed record across bodies/views.

Boundary ownership/profile changes when cover/masks change, so they participate in the **same morph transaction** as patch replacement. Recompute every affected edge/corner owner and include incident neighbors whose target changes; continue the dependency closure until outside boundary geometry is unchanged. This is an extra geometry dependency closure, not a relaxation of one-level balance. If closure cannot fit resources, defer it and retain the old valid cover.

Independent patch timers, independent per-face filters, simply displacing fine samples at the fine profile and skirts are rejected. Geometry and lighting seams are tested separately. A coarser constraint disappears only through a shared transition; changing a stitch bit cannot instantly raise a mountain endpoint.

## 11. LOD morph strategy and representation

Choose **CPU-derived geometric morph on a temporary common refinement of old/new piecewise-linear meshes**, with synchronized closure progress. Frequency taper reduces aliasing/new-detail amplitude; it is not a substitute for reproducing the parent triangulation.

The normal settled target still uses grid16 and the 16 Phase 4 stitch variants. A split first waits for all four children, balance dependencies, affected edge/corner records and transition topology. The old visible mesh (including its actual stitch mask and any reconciled vertices) is captured/pinned. No new transition starts on a region already morphing.

### 11.1 Exact endpoints, including stitched diagonals

In the face domain, overlay the old and proposed new triangle partitions over the transaction's region. Insert their edge intersections and use a deterministic ordered triangulation of each convex intersection polygon. Use integer/dyadic grid coordinates and exact rational predicates/intersections where needed, with checked bounded numerators; convert barycentric weights to f64 once with a numeric allowance. Across face boundaries use the frozen signed-axis adjacency and a shared boundary subdivision. Do not assume interpolating coarse heights at fine-grid vertices reproduces a stitched parent: triangles can cross its diagonal.

At each overlay vertex, retain barycentric references into the captured old triangle and the new target triangle. Then:

```text
P_old = old mesh barycentric position at domain vertex
P_new = new mesh barycentric position at domain vertex
P(m) = P_old + m * (P_new - P_old)
```

Every overlay triangle is inside one old and one new triangle. At m=0 the entire interpolated geometry exactly reproduces the old piecewise-linear surface within numeric allowance; m=1 reproduces the new target, including boundary constraints. **Height-only morph is rejected**: new angular samples also need the actual parent chord position. Old/new normals use captured interpolation and Section 8's shading rule. Full duplicated global vertex sets are unnecessary: cache barycentric references plus optional resolved position/normal deltas only for active transitions.

Draw the temporary overlay through a separate counted CPU-prepared triangle stream sharing opaque reverse-Z/shading and the precision/clipping proof. Extend the current clipped/fallback64 contract for these transient triangles; do not call ordinary morph triangles numerical fallbacks in metrics. There is exactly one opaque representation per region: old regular triangles stop when overlay m=0 begins; new regular triangles take over at m=1. No skirts/alpha overlap/depth fighting or topology switch at intermediate m.

### 11.2 Progress, merges and interruption

One closure morph factor uses accumulated **admitted navigation wall duration**, initially 0.25 s eased smoothstep, independent of simulation time/seed/frame count. Edge/corner records use that shared factor. Outside boundaries are unchanged at both endpoints, so neighboring nontransitioning regions remain compatible. Generation completion order never controls an edge's factor.

Merges morph current siblings and affected neighbors to the actual proposed parent before committing the coarser cover. Logical active coverage is frozen during the morph; distinguish transition draw coverage from target cover in reports. Split commit and balanced target cover activation occur atomically at m=1; the morph overlay owns draw coverage beforehand. Hysteresis/split-history updates follow that commit. Do not coarsen/rerefine beneath a running morph. A reversal finishes the bounded transition then re-evaluates desired work; teleports keep a complete ready coarser cover and enqueue high-error refinement without requiring every obsolete request. Hidden/gap time does not advance morphs by hours; reset admitted capture just like navigation.

Resource emergency may retain/coarsen to pinned roots with explicitly degraded quality. Any non-subpixel representation change in normal paths must morph; a sudden debug teleport/emergency fallback is separately diagnosed and never accepted as a smooth ordinary transition. GPU arithmetic/physical budgets still apply to the coarse fallback.

Initial bounds: max **65,536 transient transition triangles/view**, max **16 MiB accounted transition construction/staging allowance** inside aggregate resource preflight. Six triangle vertices are not needed: three packed64 vertices per triangle imply 12 MiB at the count cap. Barycentric/index construction capacity and simultaneous clipping expansion also count; actual permitted count may be lower. Chunk/defer closure if it does not fit, never truncate overlay area. Standard draws remain <=16 buckets per body batch; the transient stream adds a counted draw, not a replacement topology architecture.

The topology overlay proof, shared ownership closure and both morph endpoints are a **mandatory first integration milestone** using analytic fixtures before artistic terrain tuning. Failure is not permission to fall back to naive height morph or skirts.

## 12. Terrain-aware visibility, far handoff and navigation

### 12.1 Bounds and opaque horizon

Before generation, inflate existing cap ball by regional `max(abs(h_min),abs(h_max))` plus numeric/boundary/transition excursion allowance. Balls containing both morph endpoints contain their convex interpolation. Constrained endpoints may use neighboring region certificates: union those certificates, not just the interior fine patch's bound. Post-generation tighten only with certified cell variation. Frustum rejection keeps Phase 4's normalized planes/grazing margin.

Disable the smooth geometric-plane horizon rejection for nonflat terrain. Enable displaced horizon rejection only with a certified **inscribed opaque sphere for the actual displayed closed cover**, not simply `R+h_min` for the analytic field. Coarse chords/morphs can lie inward. For every displayed triangle, certify positive outward domain/winding and a plane distance enclosing the origin; a lower bound `(R+h_global_min)*cos(delta_max)` is usable only after proving each triangle lies in a directional cap delta_max<pi/2 and the complete radial cover stays star-shaped. Include numeric allowance; overlay morph plane/orientation certificates must cover the entire morph interval. If unavailable use opaque radius zero/disable occlusion.

Compute/update a cover-wide minimum opaque certificate from compact per-patch/transition certificates, not terrain per sample every frame. Never increase r0 before a new cover is certified; retaining a smaller valid r0 is safe. At d=|observer|>r0, rmax=R+regional_max (including transition bounds), all points in a patch cap are hidden if their minimum angular separation from observer direction is strictly greater than:

```text
acos(r0/d) + acos(r0/rmax)
```

with valid d,rmax>=r0 and conservative margins. Alternatively use the sufficient whole-ball shadow cone/beyond-occluder test described in Phase 4 §18.2. Bounds with rmax<r0 or uncertain arithmetic cannot be rejected by this test. This lets high mountains survive beyond the reference-sphere horizon, including negative basins. Coarse-root r0 may make the test loose; that is a performance risk, not grounds for an unsafe reference-radius shortcut. At/inside the opaque envelope or on certificate failure, disable horizon rejection and retain frustum/resource-bounded traversal. Future volumetric ownership can revoke opaque certification.

### 12.2 Handoff and camera

The far icosphere remains a reference-sphere approximation. Its terrain-body error is bounded by its sphere error plus the global absolute displacement envelope, projected using expanded bounds. Prewarm and 0.05/0.125 px ownership thresholds remain, but include terrain error; large mountains can demand earlier patch responsibility. Keep the same BodyId, radius, frames, instant, observations/marker semantics and exactly one opaque owner. At far/surface transfer, ordinary discrepancy must remain within Phase 4's 0.35 px geometry/narrowing and 0.45 px full-projection envelope; otherwise use a compatible bounded transition or transfer earlier, not an unannounced terrain jump. A ready six-root **terrain** cover is required before declaring terrain surface coverage available.

Measured signed reference clearance remains `|C|-R`; add analytic terrain clearance `|C|-R-h(n, footprint=0)` and displayed-mesh clearance only when actually computed. Do not present any one as collision clearance. Surface Inspection's navigation-only guard may query complete finite-band height at the observer direction plus minimum navigation clearance; it moves only the observer and is explicitly not continuous collision/walking. A radial guard alone does not guarantee absence of clipping against coarse mesh approximations during starved refinement; report unresolved geometry and allow debug bypass. Keep default body orbit translating/nonrotating, explicit fixed-frame inspection, independent moon/star motion and current one-depth ownership.

Picking/hover may remain reference-sphere logical-region picking labelled approximate in Phase 5. Accurate terrain ray picking/collision is not silently included. Focus/overview fitting uses expanded physical visualization bounds without changing authoritative radius or gravity.

## 13. Generation strategy comparison and chosen baseline

| Option | Performance, precision, determinism, future compatibility | Decision |
| --- | --- | --- |
| A: CPU procedural evaluation every rendered frame | Adds expensive field work to up to 116,178 already costly samples; easy reference/queries but pointless regeneration on motion | Reject |
| **B: CPU generation on demand + reusable geometry** | Pure f64 truth, scalar/batch testing, CPU derivatives, disposable bounded memory; current precision preparation survives; morph endpoints reusable | **Phase 5 baseline** |
| C: CPU macro + GPU fine displacement | Low upload potential, but CPU/GPU queries/filter/normals/edit consistency and precision/edge ownership become two implementations | Defer fine-detail/material optimization |
| D: GPU procedural geometry + CPU reference | Parallel potential; double algorithms, default-feature/precision constraints, readback/queries and determinism costs | Defer until generation measurements justify |
| E: Cached height tiles / patches | Reusable sampled fields, possible future erosion/edit inputs; separate filtering/interpolation/face seams and tile residency | Use only final patch cache initially; tiles are a later measured alternative |

### 13.1 Once versus every frame

Once per definition/radius/address/profile cache miss: canonical directions, multi-scale height/derivatives, body-fixed f64 positions/normals, certified regional/error data, optional bounded debug contributions. Normal final geometry is unchanged by observer/body motion. A stitch-mask/neighbor change recomputes derived boundary constraints/transition records from cached data, not every height field.

Every displayed frame: observer/source preparation, selection/culling/projected error, boundary ownership resolution if changed, active morph interpolation, source-centred f64 conversion, normal rotation, checked narrowing/precision proof, sample/instance packing and GPU upload. Settled fields regenerate **zero samples**. Retained geometry does not imply retaining old view-relative GPU bytes. The ~11 ms Phase 4 path is not claimed solved: conversion/packing/upload are still O(visible samples), though canonical mapping/noise/normals-in-body are reusable. Separate transform-only timings from field generation.

### 13.2 Patch-local f64 anchors and f32 offsets

Evaluate, but do not require a new GPU transform path for V1. A patch-centre anchor A in body-fixed f64 plus f64 offsets d supports reusable geometry and precision. Per frame compute `R_camera_from_body * ((A-C_body)+d)` using the existing prepared/LCA source semantics; do not flatten through root. Canonical seam records bypass differing anchor evaluation order and are prepared once. Anchor remains unchanged under body rotation/translation; only relative placement changes. Normals rotate without translation.

Small f32 offsets can be attractive at deep local patches, but a level-4 patch spans roughly 800 km at R6.4e6; f32 spacing at 4e5 m is ~0.03125 m. That exceeds the 1e-5 m near budget. Even offset spacing at 100 m is about 7.6e-6 m before rotation/addition error. A uniform f32-local payload cannot be asserted sufficient at all levels/views; adjacent patch anchors also produce different GPU edge rounding.

Baseline retains f64 body-fixed samples (or f64 anchor+offset storage with proven equivalence) and renderer-owned narrowing after centering. Prefer simple cached body positions first: an anchor does not automatically save per-frame work. A later optional GPU local-offset backend must pass total offset/anchor/rotation/addition/projection error and identical boundary tests, with fallback to the frozen CPU path; default optional GPU features/high-low/global f32 subtraction are not needed. No new runtime FrameId per patch.

## 14. Cache, readiness and generation budget

### 14.1 Ownership and keys

World owns pure scalar/batch evaluation and authoritative immutable definitions. App owns a per-body `TerrainPatchCache` and a small `TerrainGenerationQueue` orchestration module, adapting to renderer geometry requests. Cache values use **renderer-owned domain-free** generated patch records: f64 body positions/normals, address/profile token, certificates and optional transition references. App may depend on both world/renderer; neither lower crate depends on app. Cache keys include BodyId association, terrain definition identity/revision, reference radius bits, mapping/grid cache schema and patch address + footprint profile. Definition identity, not the moving world revision, decides data validity. Do not store FrameId in generated data.

This address is a convenient **derived cache key**, not permanent terrain location identity. Eviction removes no world truth/edit/object. Cache final geometry only; do not add a global terrain-sample dictionary, all noise intermediates or whole-planet textures. Share active boundary records and batch control evaluations where useful. Low-frequency field caching awaits sample-cost evidence.

### 14.2 Minimal lifecycle

States are external membership: absent → requested (queue) → building (private cursor/workspace) → ready (validated immutable cache entry). Active and retained cached are memberships/pins, not separate copies. Eviction returns to absent. Failed numeric generation is a visible error; an unavailable budget is pending, not failure disguised as culling.

Retain Phase 4 metadata Requested/Ready semantics and local complete sibling/balance closures. Add a separate **geometry readiness** predicate: metadata alone is insufficient to activate drawable children. Geometry and changed boundary/transition endpoints must be ready for every relevant replacement region. Culled guard children need certified metadata/coverage, not a mesh, but a newly visible leaf missing geometry must display a pinned generated ancestor. Form a balanced drawable ancestor cover before preparation; if ancestor fallback changes neighbor restrictions, defer the closure. Pin six generated roots, active visible entries, their necessary ancestors, current transition endpoints and pending highest-error dependencies. Never evict the only covering representation.

An initially visible generated terrain root cover is produced under budgets while the available far body remains sole opaque owner. Changing seed/config stages the new six roots, then atomically switches definition/render revision; show regeneration pending and do not present old geometry as new terrain. Accepted authoritative config may be published with an explicitly tagged old-revision preview during rebuilding, but baseline UI uses staged apply to avoid mixed revisions. Invalid edits leave old definition/cache/queue unchanged. Old+new root/result memory is preflighted within the same cap, not doubled budgets.

### 14.3 Bounded synchronous generation first

Choose **resumable synchronous CPU batches**, not an indivisible hundreds-of-patches loop and not a general job system. Initial per-update cap: 32 new metadata records (existing), **1,156 unique regular sample evaluations** (four 289-sample patches), and 2 ms generation opportunity checked between batches of at most **32 samples**. At most one 32-sample microbatch can overshoot the time opportunity; report it. Count controls/noise evaluations separately. Certificate/topology construction has its own resumable operation cap (initially eight interval subregions and 256 triangle-overlay operations per chunk); time them within the same generation opportunity. Headless replay uses explicit operation caps/schedule, with wall cutoff disabled.

These numbers are initial scheduling policy, not promises of patch latency/60 FPS. Stop after a chunk on either limit, retain builder cursor and complete parent coverage. Priority is largest projected quality debt, then frozen address order, preserving the existing highest-error pending dependency chain. Desired traversal does not read wall time; the time budget only controls publication availability. Count resumed work correctly: no full patch restart merely because a chunk ends. Cap pending regular requests at 256 and private partial builders at four; prune obsolete unstarted requests, cancel a builder at a chunk boundary when its definition/request is obsolete. A canceled result never activates a child.

Slow approach/lateral flight: reuse stable entries and refine incrementally. Rapid descent/teleport: keep ready ancestors and display quality debt while prioritizing new view. Orbit across faces: same global field, canonical seam ownership and bounded eviction. Rapid zoom out: use ready parents, synchronized merge/morph, then far handoff when valid. Return inward reuses retained patches. World movement/rotation changes neither keys nor queued generation.

### 14.4 Limited worker option, measurement gate

Parallel CPU generation is promising but not required before costs are known. If measured 32-sample/certificate microbatch routinely stalls interactive updates or convergence is unacceptable at normal workloads, introduce the smallest **terrain-only one-worker** adapter: immutable definition/radius/region/profile jobs, bounded input/result queues (initially eight each), cancellation token/revision checked between chunks, no world/frame/GPU access, result publication on app thread in deterministic request order. Reserve result/workspace memory from the cache cap. Do not wait for a worker in the frame loop. Completion scheduling may affect readiness timing, never generated values or partial sibling activation. Join/cancel on shutdown; hidden window suspends admission, publication resumes coherently. Increase worker count only after throughput/contention measurements; no Rayon/Tokio/general job graph is pre-approved.

## 15. Memory and resource accounting

Grid16 is still 289 samples. Baseline cached body position `DVec3` + body normal `DVec3` has **48 semantic bytes/sample**, or **13,872 bytes/patch**. Heights are recoverable from position magnitude for diagnostics and are not a third permanent array by default. Optional debug heights cost 2,312 bytes/patch; keep only selected patches or a bounded debug subset. Canonical 64 boundary keys can be regenerated/retained selectively (u128 keys cost 1,024 bytes/patch). Native layout/capacity must be measured, not assumed from semantic sums.

| Resident patches | CPU position+normal payload | Optional equally sized old/delta payload | Current GPU sample32+instance64 payload |
| ---: | ---: | ---: | ---: |
| 402 | 5.32 MiB | +5.32 MiB | 3.57 MiB |
| 512 | 6.77 MiB | +6.77 MiB | 4.55 MiB |
| 1,024 | 13.55 MiB | +13.55 MiB | 9.09 MiB |
| 2,048 | 27.09 MiB | +27.09 MiB | 18.19 MiB |
| 4,096 | 54.19 MiB | +54.19 MiB | 36.38 MiB |

Old/delta column is a deliberately pessimistic comparison, **not** a requirement to double all entries: pin cached old endpoints and store barycentric transition references; only transitioning geometry may retain resolved deltas. 512 bytes of hypothetical metadata/container allowance per patch adds 2 MiB at 4,096; measure real record/container capacity, including padding. Hundreds/thousands are practical within a bounded desktop budget, but roots/ancestors, debug data and transient construction compete with visible entries.

Initial new **aggregate terrain CPU residency/work cap 128 MiB** across all enabled bodies: final cache, capacities/container slack, pending builders/results, cached certificates, boundary targets and transition references. Max 4,096 generated entries aggregate (not per planet); pins may reduce available refinement. Reserve up to 16 MiB transition work within that cap. Existing renderer caps remain: metadata 1 MiB, cover scratch 8 MiB, boundary scratch 8 MiB, outgoing capacity 64 MiB, GPU allocations 80 MiB (indices/instances/fallback/morph included; depth window resources separate). Do not allocate the 128 MiB eagerly; grow only with complete preflight.

LRU uses checked access sequence and address ties, not wall clock. Evict inactive unpinned entries; active/pending endpoint pins cannot be stolen. If pins/closure exceed byte/count/device limits, defer or retain a balanced ancestor cover and report pressure. Account actual vector **capacities**, retained stale-root replacement, in-flight results and GPU growth overlap; no multiplication by enabled body count. Renderer currently drains prior submissions on GPU growth: preserve safe ownership and measure possible stalls. Persistent GPU patch pools/reupload avoidance are deferred because they require a new precision/in-flight seam contract, not because cached terrain would be invalid.

## 16. Representative Rust interfaces and allocation contract

Sketches show implementation boundaries; checked types keep private fields/accessors, and only real callers receive public APIs. They are not source to paste into the engine during this task.

```rust
// mundaris_world::terrain: no FrameId, camera, render frame or GPU allocation.
pub struct TerrainSeed(u64);
pub struct TerrainIdentity(u64);
pub enum TerrainGeneratorVersion { V1 }
pub struct TerrainRevision(u64);
pub struct TerrainConfig { /* five bands, small macro/range controls; validated */ }
pub struct TerrainDefinition { /* identity, seed, version, immutable config */ }
pub struct TerrainFootprint { /* nonnegative f64 metres; zero = complete field */ }
pub struct TerrainQuery {
    /* SurfaceLocation + TerrainFootprint; body binding belongs to definition owner */
}
pub struct TerrainSample {
    /* f64 height_m; DVec3 tangent_gradient_m_per_unit_direction */
}
pub struct TerrainRegionCertificate {
    /* f64 min/max height, active/tail band bounds, derivative envelopes */
}
pub struct TerrainGenerator<'a> { /* borrowed validated definition + R, compiled bands */ }
impl TerrainGenerator<'_> {
    pub fn evaluate_point(&self, query: TerrainQuery)
        -> Result<TerrainSample, TerrainError>;
    pub fn evaluate_batch(&self, locations: &[SurfaceLocation],
        footprint: TerrainFootprint, output: &mut [TerrainSample])
        -> Result<TerrainEvaluationReport, TerrainError>;
    pub fn bounds_for_region(&self, region: DirectionalCap,
        footprint: TerrainFootprint)
        -> Result<TerrainRegionCertificate, TerrainError>;
}
```

Scalar/batch use the same fixed-order core. Batch precomputes weights/domain constants and uses caller storage; it does not allocate per sample or call an expensive public validating scalar wrapper 116,000 times. A patch-generation adapter builds canonical location batches of up to32 and invokes this batch API. `DirectionalCap` is a generic math region, not a world-specific patch ID. World provides primitive certificates; renderer composes chart/triangle error for its representation. A compact enum for Flat/Analytic/V1 evaluation is sufficient for real fixtures; no plugin/noise trait hierarchy.

```rust
// mundaris_renderer::planet_surface: domain-free derived geometry/certificates.
pub struct GeneratedSurfacePatch {
    /* address, physical profile token, f64 body positions/normals,
       conservative extent/error inputs; no world definition/BodyId/FrameId */
}
pub struct SurfacePatchAvailability<'a> {
    /* borrowed ready geometry/certificates; queried by address, allocation-free */
}
pub struct SurfaceTerrainInput<'a> {
    /* region certificates, total-error inputs and geometry readiness, opaque bound */
}
// Extend SurfaceLodSession::update with explicit optional terrain input;
// existing smooth callers remain a tested zero-height path.
// CelestialFrame::append_generated_surface consumes borrowed ready geometry,
// same current-view lifetime/poisoning contract as append_surface.

// mundaris_app::planet_terrain: orchestration/adaptation, not generation truth.
struct TerrainPatchCache { /* exact keys, bounded LRU, pins */ }
struct TerrainGenerationQueue { /* bounded requests/private resumable builders */ }
struct TerrainTransition { /* closure, old/new barycentric references, wall progress */ }
```

Use concrete borrowed readiness lookup or a narrow static-dispatch callback where required; do not pass world configuration into renderer to make lookup convenient. Cache/query setup may allocate; warm field evaluation and transform preparation reuse bounded storage. Invalid lengths/input/overflow/version/definition mismatch return focused errors. Failed batch may leave scratch partially written; **never publish it**. A generated entry is Ready only after all samples/certificates validate. Full render failure still poisons the combined celestial frame. Recoverable quota exhaustion reports pending/degraded quality and preserves old coverage; nonfinite terrain is not silently replaced by zero.

## 17. Fixtures, objective tests and tolerances

Fixtures use the same scalar/batch generator and integrated rendering path:

- **Flat:** all amplitudes zero; reproduce Phase 4 positions/radial normals/counts/handoff. Preserve the existing flat fast path and its evidence.
- **Diagnostic:** constant displacement; linear direction field `h=A*(n·k)`; smooth polynomial wave/ridge; sinusoidal wave for filtering tests. Supply independently derived gradients/Hessians/envelopes. Do not confuse characteristic 3D wave frequency with a globally uniform geodesic wavelength.
- **Earth-like:** Section 6 hierarchy, fixed explicit identity/seed and several documented alternative seeds.
- **Extreme rugged:** legal near-0.1R relief, high warp/soft ridge extrema and negative basins; deliberately stress certificates/refinement/resources.
- **Smooth moon-like:** smaller R, reduced amplitudes/metre bands; no craters/biome system implied.

All automated tests are headless, independent formulas/fixtures before artistic noise. Require finite values before approximate comparisons. Numerical envelopes apply to supported radii<=1e8 m and configurations within Section 4's radius/relief envelope; extreme unsupported inputs must fail explicitly.

| Contract | Required test / tolerance |
| --- | --- |
| Hash/noise | Fixed signed-cell/seed vectors exact integer output; same-target repeated values/gradients bits identical; fixed seed changes a nontrivial deterministic sample ensemble, not necessarily every single point |
| Primitive derivatives | Central-difference step sweep 1e-3→1e-6 in bounded noise domain; minimum well-conditioned derivative residual<=1e-6 relative-to-max(1,derivative magnitude); independent polynomial derivative checks<=1e-12 |
| Scalar/batch | Identical bits for same canonical inputs/footprint and fixed expression order, including sliced/resumed batches and arbitrary request order |
| Height semantics | Constant±1000m fixture matches `(R+h)n` within max(1e-7m,64EPS(R+abs(h))); invalid total radius/config/footprint/overflow rejected transactionally |
| Face/corner continuity | All24 directed edges/12 edges/8 corners, levels0/1/5/16/30 and non-grid near-edge limits; canonical inputs at same profile have equal height/gradient bits; independent direction residual<=2e-14, height residual<=max(1e-6m,128EPS*height_scale) for equivalent independently reconstructed inputs |
| Parent/child common terms | Same canonical locations: identical fully active band terms/common sum bits; profile difference equals explicitly weighted extra/taper terms within1e-9m plus128EPS*height_scale; no level-dependent seed/domain reinterpretation |
| Filtering | Weights exact zero/full endpoints, monotone C1 taper, bounded height delta by residual envelopes; rho50km/2m cases; zero-weight bands have zero primitive calls; derivative-aware remap/warp tests; known unresolved wave cannot alias into active low-frequency output |
| Morph endpoints | Dense independent domain/ray samples across every old/new stitch-mask pair and corner closure: overlay m0/m1 surface residual<=max(1e-7m,256EPS*R); no parent stitched diagonal crossed without overlay intersection; topology exact domain area/manifold/shared partitions |
| Displaced edges | Every16 mask, reversed cross-face edge, corner/neighbor ownership change and starvation: drawn boundary position/normal bytes identical per current view; coarse/fine segment sequence equal, not just shared endpoint heights |
| Normal/gradient | Constant height radial; linear `A(n·k)` analytic tangent gradient and normal<=1e-10 vector norm; procedural analytic vs well-conditioned finite-difference normal angular error<=1e-4rad; unit length<=1e-12 CPU and <=2e-6 after narrowing; outward/nonzero morph blend |
| Height/region bounds | >=4096 deterministic directions plus extrema/pathological fixtures per selected config; observed heights within certificate +max(1e-6m,256EPS*height_scale); certificate derivation/interval tests, not sampled max called a proof |
| Approximation error | All masks, representative levels0/4/10/18/30, ordinary/pathological bands: >=64 deterministic non-grid points/triangle where tractable plus seeded256-point patch sweeps against full-field reference; discrepancy<=reported E+max(1e-6m,256EPS*R); test morph and boundary residual separately |
| Culling | Independent full-field/actual stitched and transition triangle ray/clipping oracles; no false frustum/horizon rejection, high mountain beyond reference horizon survives, negative basin/opaque-root/morph/inside/uncertified cases |
| Replay/cache | Generate twice, evict/rebuild, reverse order, different request/traversal/update schedules, camera away/back: same final numeric bits; revision/radius invalidation rejects stale geometry/results; unrelated world revisions preserve entries |
| Motion/precision | Body moves >=1e8m and rotates: cached body positions/normals bits unchanged, regenerated count zero; shared ancestor offsets0/1.5e11/1e16 and arbitrary rotation preserve CPU local1e-9m or body1e-7m budgets; no root round-trip promise at1e16 |
| GPU precision | Preserve1e-5/1e-4/1e-3m component error within100m/1km/10km; <=0.05px positional narrowing + separately<=0.05px GPU projection; rare clipping/transition paths same proof; default limits/layout/naga/poisoning tests |
| Readiness/resources | Delay one sibling indefinitely, work0/32/1156, rapid reversal/teleport/two bodies: complete balanced drawable cover, no partial activation/holes, pins/count/capacity/bytes within caps; settled camera1000 updates generates zero terrain samples |
| Ownership | Rendering/query/cache/selection/morph leave body properties/state/time/revision exact unchanged; frame rebuild changes no terrain definition/location/value; config changes do not change celestial identity/orbits |

Statistical evidence: deterministic area-weighted spherical sample set (not unweighted cube vertices), at least8 V1 seeds, min/max/mean/stddev, slope p50/p95/max, broad elevated-region fraction and per-band RMS. Store coarse ranges and reported deltas as versioned regression evidence; avoid fragile golden artistic numbers or label elevated fraction ocean/land truth. Same-version deterministic reference samples protect accidental algorithm changes; intentional algorithm changes require version disposition. Operator checks supplement but do not replace analytic proofs.

## 18. Exact performance benchmark plan and review triggers

Reuse Criterion0.8.2 and the existing opt-in stage probe, CPU-only/outside normal CI. No measurements for Phase 5 are claimed by this design. Run old/new matched scenarios with viewer closed, sequential timing groups, 20 samples/100ms warmup/requested500ms measurement initially; extend slow cases as needed, retain medians/distributions/95% intervals/outliers. Preallocate warm inputs/output; exclude setup/device/shader/window except explicitly cold groups. Observe complete output with black_box and verify correctness before timing.

| Target / group | Exact workloads and reported separation |
| --- | --- |
| `mundaris_math --bench terrain_noise` (new) | One value, value+gradient, 289 points, bounded derivative/certificate primitive; rotations/warps separately; deterministic changing inputs |
| `mundaris_world --bench terrain_generation` (new) | Scalar query; batch289; Flat/Diagnostic/continent-only/fullV1; rho50km/1km/128m/2m/1m/0; height-only vs height+gradient; all actual mask/warp primitive calls; region certificate queries at coarse/mid/fine caps |
| `mundaris_app --bench planet_terrain_generation` (new) | Regular patch289 with coordinates/height/normals/certificates; cache hit vs miss vs eviction/rebuild; 32-sample resumed chunks; one four-child closure; 256-request rapid refinement schedule; old/new stitched transition construction and normal/boundary target cost |
| `mundaris_renderer --bench planet_surface` (extend) | Flat matched Phase4; ready cached geometry transform/proof/pack; unchanged observer vs changed observer/body; morph m0/0.5/1 and clipped crossing; original510/402/116178 sample view remains named/comparable |
| `mundaris_app --bench planet_surface_approach` (extend) | Same real N3/h60 pipeline, Flat and terrain; separate paused reuse vs one physical commit; full-view10,000km, filling6,400km, low orbit1,000km,10km,100m,2m down/horizon; generation/certificates/selection/transform/pack separated |
| Native optional profiling | Default UI/guides/history separately from geometry-only benches; cold approach/refine/lateral/reversal/teleport/seed change; actual upload/GPU/presentation/driver allocations where instrumentation available |

Benchmark centre/edge/corner views, FOV30/60/90°, physical1280×800 and3840×2160, tall content and non100% DPI. Use measured terrain clearance for100m/2m test observer so mountains do not put the camera underground inadvertently; also record reference clearance. Test radii6.371e6 and3.74e5 with presets, at least one extreme rugged case. Cold six-root convergence is a complete update sequence, not the existing single-update cold benchmark renamed.

Every result records seed/identity/version/config/profile/radius, actual footprint and bands/octaves/primitive evaluations, sample count, requested/generated/reused/evicted/pending patches, active/visible/guard counts, morph/fallback triangles, CPU stage times, memory capacity/peak and GPU payload. Always report **cost per terrain sample** and **new terrain samples regenerated per update**; normals cost, certificate-only cost and transform-only preparation are separate. Cached sample use is not counted as fresh generation; boundary/transition extra queries are included in generated counters. Primitive-call count exposes hidden gradient finite-difference multiplication.

Review triggers, not invented hardware-independent timing promises:

- A settled unchanged view must generate zero terrain samples after convergence; body motion/rotation must also generate zero.
- Whole-planet views must not execute unresolved fine bands. Inspect actual primitive counters, not intended cutoff.
- Normal approach/lateral motion must refine incrementally without routine frame stalls or prolonged holes (holes are correctness failures, never permitted).
- If32-sample/certificate microbatch routinely exceeds the2ms opportunity or queue debt grows in normal paths, measure bottleneck and consider Section14.4's worker, not a wholesale Phase4 redesign.
- Terrain cache must stay bounded under repeated focus/seed/radius/revisit changes; pinned pressure is visible.
- Match flat precision/quality/count baselines; report full-view preparation against accepted~11ms, not claim caching automatically meets the old2ms trigger. Terrain generation and steady reuse must be compared independently.
- Conservative interval/tail error may refine excessive patches, especially near-plane/grazing. Investigate bounds/certificate tightness and effective frequency before proposing threshold changes. Any revised threshold needs matched visual/error evidence and explicit review.
- Report cold transition-topology time/memory and GPU growth waits; synchronous overlay construction is also chunked work, not hidden unbudgeted generation.

## 19. Validation UI and operator sequence

Extend existing `--gravity-orbits` and its surface panels/routes. A `--planet-terrain` flag is optional only as a fixture/control preset for the **same** engine/session; baseline needs no second flag. Seed/config draft editing is an explicit staged Apply/New Definition; changing debug visualization never changes terrain revision. Reset restores known fixture/seed without changing BodyId. Restore seed/config verifies reproducibility, even after eviction. No save/editor UX.

Normal view is simple terrain-aware shading. Bounded debug modes: elevation, slope, selected band contribution, continent control, mountain mask, LOD, morph factor, generation readiness/cache, normals. Store/compute diagnostic contributions only for selected bounded patches; changing a visualization must not regenerate the entire planet each frame.

Live panel: requested/generated/reused patches, pending, evaluated samples and primitive/band calls, cache hits/misses/evictions, generation time, transform-only time, CPU terrain bytes/GPU payload, target vs displayed unresolved error, active morph count. Bounded selected-patch height/slope statistics; no GIS package or hundreds of counters.

Record OS/GPU/backend/driver/profile/source revision, viewport/FOV/DPI/config/budgets and observations:

1. Open system overview; identify bodies/guides/history.
2. Focus procedural Aurelia; same BodyId and coherent physical system.
3. Approach from far distance; observe earlier terrain-aware prewarm/handoff with one opaque owner.
4. Verify broad continents/uplifts/basins without cube seams.
5. Descend through low orbit/100km/10km/1km with actual terrain clearance visible.
6. Observe coherent ranges acquiring regional/local detail, not changing mountain identity.
7. Orbit laterally; cross face centres/edges and every targeted corner.
8. Cross LOD split/merge and stitch-mask changes; inspect morph start/end and normals.
9. Inspect ridge/valley silhouettes in normal shading.
10. Reach100m then~2–10m above terrain; fixed inspection/local look keeps terrain attached.
11. Move laterally, cross several patch seams and vary look toward horizon.
12. Zoom rapidly out to far/system, then back in; readiness fallback remains complete.
13. Teleport/debug jump and deliberately starve generation, then hold until converged; quality debt explicit, no holes.
14. Change seed via Apply; inspect coherent staged regeneration; restore prior seed/config and compare known features.
15. Enable elevation/slope/band/mask/morph/readiness/cache/normal diagnostics; disable them and assess normal temporal appearance.
16. Resume/single-step physics; verify moon/star independent motion and unchanged mountain body-local coordinates, including >=1e8m body translation test.
17. Resize wide/tall, non100% DPI; minimize/restore/occlude, actual OS sleep and live clock gap/recovery where available; close.

Look for cracks, LOD/face seams, popping, high-frequency shimmer, terrain swimming, lighting seams, cache holes, generation/topology stalls, jitter, culling flicker and mountains disappearing beyond the smooth horizon. Captures alone do not prove temporal absence; distinguish directed checkpoint evidence from live human signoff. Quality goal is a hierarchy of recognizable broad regions → ranges → mountains → ridges/hills → local variation, not photorealism or uniformly rough noise soup.

## 20. Future compatibility (interfaces only)

- **Editing:** later composition occurs in world terrain query/certificate layer: procedural base + sparse body-fixed modifications → final field. Edits invalidate intersecting derived patches/certificates, not render vertex IDs. No edit storage/index now.
- **Volumetric override:** height terrain remains default base; future explicit body-local ownership regions may replace it and revoke opaque-sphere certification. Mesher, seam/collision ownership are deferred, not assumed solved.
- **Tectonics:** replace macro/range control fields (uplift, basin elevation, mountain likelihood/boundary distance) with versioned immutable inputs and certificates; no topology change. V1's concrete controls are internal replaceable functions, not an empty public tectonic provider trait.
- **True erosion:** replace regional/local bands/control data behind the same height/gradient/bounds contract. Stateful/offline context and its revisions are a later design.
- **Biome/climate:** elevation/analytic slope available from pure world queries; latitude/insolation/moisture/temperature remain independent later inputs.
- **Vegetation/objects:** identity comes from independent fixed world-layer regions/candidates/version/seed, not current LOD patch/cache or morph state. Placement can query complete or explicitly filtered height/slope. No tree system now.
- **Materials/GPU:** sub8m appearance may become material normals/detail, with a separate backend consistency contract. Renderer remains replaceable and terrain query truth remains CPU-accessible.

## 21. Exact file-level implementation plan

This table is for a subsequent implementation task; **no engine/Cargo changes are made by this specification**. Existing files are extended narrowly; new files are created only when their stage has concrete callers.

| File | Planned work / ownership / verification |
| --- | --- |
| `crates/math/src/noise.rs` (new), `src/lib.rs` | Generic hashed3D gradient primitive/value+derivative/private bounds; no planet policy; exact hash, independent derivatives |
| `crates/math/src/surface.rs` | Reuse canonical APIs; generic directional-cap/footprint Jacobian helpers only if required, no changed mapping/address identity |
| `crates/math/tests/terrain_noise.rs`, `benches/terrain_noise.rs` (new) | Exact seed/lattice/derivative/bounds tests and cost baseline |
| `crates/world/src/terrain/mod.rs`, `config.rs`, `generator.rs`, `bounds.rs` (new) | Definition/version/seed/identity/revision/footprint/query; scalar/batch bands/controls/analytic fixtures; region interval/derivative/residual certificates |
| `crates/world/src/{body,system,lib}.rs` | Optional terrain definition association and narrow transactional terrain mutations; keep BodyState/gravity/motion revisions distinct |
| `crates/world/tests/terrain_generation.rs`, `terrain_bounds.rs` (new) | Analytical heights/gradients/filtering/seams/scalar-batch/replay/version/config transaction/error certification |
| `crates/world/benches/terrain_generation.rs` (new) | Sample/patch/profile/normal/control/certificate costs with counts |
| `crates/renderer/src/planet_surface/{mod,bounds,lod,cover}.rs` | Domain-free generated geometry/certificates/readiness, total-error projection, conservative terrain visibility, balanced drawable ancestor fallback; flat path preserved |
| `crates/renderer/src/planet_surface/cache.rs` | Keep dimensionless metadata independent; narrow certificate/readiness lookup support, no authoritative terrain config |
| `crates/renderer/src/planet_surface/terrain_geometry.rs` (new) | Domain-free ready patch records, boundary reconciliation/residual and visual normal helpers; renderer owns representation, not generator |
| `crates/renderer/src/planet_surface/transition.rs` (new) | Exact old/new stitched-domain overlay, barycentric endpoint references, boundary closure inputs, count/cap/error/morph proof |
| `crates/renderer/src/planet_surface/{prepare,gpu}.rs` | Consume cached f64 body geometry, current boundary packing, chunk-produced transition stream, clipped proof and aggregate caps; retain sample32/instance64 normal path |
| `crates/renderer/src/shaders/planet_surface.wgsl` | Basic terrain normals/debug attributes and transition shared fragment path only; no procedural seed/noise ownership |
| `crates/renderer/src/{celestial,lib}.rs` | Borrowed generated-surface append/pass/report wiring, poisoning; depth cleared once |
| `crates/renderer/tests/planet_surface_lod.rs`, `planet_surface_precision.rs` | Preserve flat tests; terrain culling/total-error/readiness/geometry/precision/no-mutation additions |
| `crates/renderer/tests/terrain_transitions.rs` (new) | Every old/new stitch pair/edge/corner, overlay area, displaced seams/normals/endpoints/interior morph bounds |
| `crates/renderer/benches/{planet_surface,planet_surface_profile}.rs` | Flat/cached transform/morph benchmarks, generated/reused counters and stage clocks |
| `crates/app/src/planet_terrain.rs` (new) | Body terrain adapter/cache/exact keys/budget queue and resumable generation; no noise algorithm |
| `crates/app/src/planet_surface.rs` | Generated root/child readiness, terrain-aware far error, staged config publication, transition orchestration |
| `crates/app/src/{celestial_camera,gravity_orbits,lib}.rs` | Terrain-clearance navigation/debug/seed/config/fixtures/UI, same integrated path, semantic frame remap |
| `crates/app/src/{system_view,celestial_selection}.rs` | Expanded derived framing/approximate picking annotations only where required; no collision engine |
| `crates/app/tests/planet_terrain.rs` (new), existing `planet_surface_{paths,navigation}.rs` | Cache/revision/budget/eviction/body-motion/system-to-terrain/lifecycle/read-only tests |
| `crates/app/benches/planet_terrain_generation.rs` (new), `planet_surface_approach.rs` | Cold generation/reuse/closure/rapid schedule and integrated matched matrix |
| Relevant existing crate `Cargo.toml` files | Register actual benchmark targets using existing Criterion; no new runtime dependency planned; no unrelated upgrades |
| `docs/adr/0007-procedural-terrain-generation.md` (at implementation review) | Record these choices/alternatives/actual API deviations, certified seams/errors and measured reuse/cost; proposed until evidence supports acceptance |
| `docs/phase-5-validation.md`, performance/architecture/roadmap/README/engine design/spec | Actual numerical/native/benchmark evidence, versions/resources/open debt and implementation status |

Simulation kernels/runner/history/time, core, reference-frame algorithms, redraw/clock, root manifest/lockfile/CI need no feature change. Main flag parsing changes only if optional shared-path terrain preset is justified. No new crate, renderer→world dependency, generalized asset/job system or unrequested scope. Any necessary file-plan deviation needs a focused documented reason.

## 22. Dependency-ordered implementation stages

1. Record baseline/readiness/resource contracts and sequencing debt; review version/coordinate/precision/error and transition mathematics. Keep all existing modes buildable.
2. Noise/hash/analytic derivatives and independently certified primitive bounds; run sample-cost microbenchmarks before adding many warps.
3. World definition/query/version/footprint APIs and Flat/analytic fixtures; scalar/batch/no-render tests.
4. Region displacement/derivative/residual certificates and representation error integration, analytical heights/normals first; no artistic tuning.
5. Reusable body-fixed patch adapter/cache, resumable budget and geometry readiness; six-root terrain fallback and eviction/body-motion tests.
6. Displaced canonical boundary ownership, every16 stitch variants/corners, bounds/culling with conservative disabled-horizon fallback until certified.
7. Exact stitched old/new overlay morph using analytic fixtures; prove endpoints/coverage/edge/normal/error/resource contracts before native use.
8. Cached generated-surface preparation, physical/projected precision, layouts/WGSL/one-depth GPU integration and terrain-aware far handoff.
9. Implement versioned macro/range/regional/local/fine functions, shared scalar/batch controls, cutoff weights and all error/gradient bounds; incremental benchmarks after each band.
10. Integrated Earth-like/moon/extreme fixtures, terrain navigation/diagnostics/seed/config Apply and quality surveys; no duplicate engine.
11. Matched rapid/steady/moving-body benchmark matrix, investigate bounds/cache/transition cost; choose limited worker only if measured trigger is met and review its deterministic publication.
12. Windows headless/release/operator/platform checks, Linux where available, remote CI through normal workflow, ADR/documentation and independent correctness/architecture/performance review. Missing gates remain open.

## 23. Audit disposition, remaining review and design validation

This design preserves original evidence and does not complete any outstanding Phase 4 checkbox. The sequencing decision supersedes earlier conditional Phase 5 entry wording only: terrain development may proceed; human control-feel/flicker/edge-corner traversal, non100% DPI, actual OS sleep/recovery, Linux native acceptance, current remote CI, external allocation/driver/GPU timestamps/compositor profiling remain debt. The accepted full-view~11ms limitation and cold spikes stay visible comparators. No architecture redesign is justified before terrain workload evidence.

Before implementation review together: exact common-refinement/boundary-owner closure and rational predicates; interval/Hessian certificate derivations (including soft ridges/warp); tail-driven LOD counts versus128MiB pins; displayed-cover opaque radius proof; staged config publication versus current celestial revision/runner ownership; budgeted synchronous chunk cost. These are specific mathematical/implementation verification gates, not permission to leave the architecture undecided or to begin with noise-only visuals.

Deferred: persistent body IDs/config serialization/save format; true erosion/tectonics/edit/voxel ownership seams; accurate ray picking/collision; slope-dependent fake-erosion experiment; exact convolution filtering/global fields/height tiles; SIMD, multiple workers, GPU generation or patch-local GPU transforms; persistent GPU sample pools; final material/lighting/normal maps; cross-platform bit identity. None is a prerequisite for the chosen CPU reuse baseline.

Design validation consists of current-source/spec/ADR/harness audit, independent numerical/cache/performance investigations, memory calculation and documentation-scope/link/encoding/whitespace review. Historical Cargo/native/benchmark results are explicitly attributed to their records, not rerun terrain validation. No source/dependency implementation, commit or push is part of this task.

## 24. Objective Definition of Done for Phase 5 implementation

All boxes are prospective and remain unchecked. Design approval is not implementation or platform evidence. Unavailable platform gates remain open and preclude claiming full release-quality acceptance.

- [ ] Versioned seeded multi-scale terrain and pure scalar/batch queries reproduce from explicit body-fixed location/definition/footprint; eviction/request/traversal/scheduling do not change final values.
- [ ] Metre/angular scales and legal height/radius semantics validated; Flat reproduces Phase4 and analytic fixtures pass independent gradient/normal/error tests.
- [ ] Broad continents/basins/uplift/lowlands, coherent localized mountain systems, regional/local ridge-valley structure and bounded8m-minimum geometric detail demonstrated across documented seeds.
- [ ] Whole-planet views skip invisible fine bands; smooth footprint taper and normal-view transitions show no high-frequency shimmer or terrain swimming.
- [ ] No cube-face/corner seams, no displaced equal/coarse-fine LOD cracks, all16 stitched variants/ownership changes pass geometry and lighting seam tests without skirts.
- [ ] Actual stitched-parent/common-refinement morph endpoints, sibling/balance/edge closure, merges and interruption keep one complete opaque drawable cover without visible ordinary popping.
- [ ] Terrain-aware analytic normals, conservative regional height/derivative/residual/error certificates, constrained/morph error and spatially varying complexity drive trustworthy LOD.
- [ ] Terrain-aware conservative frustum/horizon bounds handle mountains/negative basins/morphs; uncertified opaque envelopes disable rejection instead of losing terrain.
- [ ] Same connected system→planet→surface experience, BodyId/reference radius/instant/frames, terrain-aware exclusive far handoff, independent star/moon motion; no world mutation from rendering.
- [ ] Body translation>=1e8m and rotation regenerate zero terrain samples; frame rebuild changes no terrain; Phase1 precision/narrowing/projection budgets pass at shared astronomical offsets.
- [ ] Bounded cache, work queues/builders, pins/transitions/staging/GPU allocations and no-hole budget fallback verified under rapid descent/lateral/reversal/teleport/config edits/two bodies.
- [ ] Settled camera1000-update test regenerates zero samples; measured cost/sample, regenerated/reused samples per update, normal/certificate/cache/transform-only/rapid refinement costs and memory/upload counts recorded.
- [ ] Benchmark matrix executed against matched Phase4 Flat baseline; actual risks/trigger misses documented, no unsupported speedup/GPU/FPS/allocation claims.
- [ ] Required normal/debug visualization, fixture reset/seed restore/config revision/clean invalidation and full operator sequence recorded; diagnostics do not accidentally change terrain.
- [ ] Phase1–4 regressions/modes/numerical envelopes remain intact; stable Rust/six publish=false crates/no unsafe/new runtime framework/cycles/unrelated dependency upgrades.
- [ ] Locked workspace build/check, formatting, warnings-denied all-target/all-feature Clippy, headless workspace tests, focused release tests/old orbital long runs and warnings-denied Rustdoc pass using README/CI quality commands.
- [ ] Windows native/operator/high-DPI/lifecycle validation recorded with OS/backend evidence; Linux native/headless/release/operator evidence recorded where available, otherwise explicitly open.
- [ ] Current implementation-revision remote Linux-quality/Windows-compatibility CI passes through normal authorized workflow; CI is not native visual evidence.
- [ ] ADR0007 and specification/validation/performance/architecture/roadmap/README report actual algorithms/APIs/version/certificates/resources/measurements and intentionally deferred decisions accurately.
- [ ] Independent correctness, architecture and performance reviews complete; prior Phase4 acceptance debt remains tracked and is completed before the later release-quality milestone.
