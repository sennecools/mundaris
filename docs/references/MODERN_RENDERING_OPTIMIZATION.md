# Modern Rendering Optimization Reference

## Source information

- **Video:** “Tutorial: How To Optimize Almost Every Step In Modern Game Rendering”.
- **URL:** <https://www.youtube.com/watch?v=SnNm7rSSvlg>.
- **Transcript source:** [transcript.whisper.reviewed.timestamped.md](../../transcript.whisper.reviewed.timestamped.md), read in full; preserved as supplied.
- **Audio duration:** **02:01:25**. Last transcribed item: **02:00:58**, explicitly marked uncertain outro audio.
- **Transcript limitations:** reviewed machine speech transcription can still contain errors. Its header references technical-spelling audit files, but those files were not located in this checkout. This reference does not independently validate the audio, footage, pipeline diagram, linked presentations, or the speaker's experiments. Visual demonstrations, diagram-only notes, hardware/resolution slides and bonus slides cannot be reconstructed from speech alone. The speaker initially calls the diagram free and then describes Patreon availability (00:02:36); availability is not resolved here.
- **Applicability inspection:** 2026-10-04, HEAD `d81bb2a17ad4f8c1d06eaa1619b6281b946641c3` **plus pre-existing dirty/untracked work**, including renderer recovery, developer interface and camera/navigation changes. This is a working-tree source inspection, not a clean-revision benchmark.
- **Working-tree limitation:** concurrent camera/navigation and reviewer-context updates were observed during extraction. Renderer applicability describes inspected source, not an immutable whole-checkout snapshot; no fresh runtime timings are claimed.
- **Source-material identity:** Git blob hash at inspection: `b377952ff03e50f283a221fffa0e151412393a9e`. This identifies the supplied transcript, not the video.

## How to use this document

This is **external research**, an idea bank rather than architecture authority or an implementation phase. It does not override current Mundaris source, tests, measurements, runtime/capture evidence, [architecture](../architecture.md), [invariants](../engine-invariants.md), or applicable ADRs. A recommendation must be profiled in Mundaris before implementation; candidate prototypes require a separately approved task. Hardware/API-specific claims require validation on Mundaris' supported backends/hardware. Opinions remain attributed opinions, including strong claims about APIs, Unreal Engine, compute lighting and TAA.

Consult the relevant topic and the cross-reference when renderer/performance architecture is being reviewed. Reading the whole reference is not a reviewer-startup requirement. Recheck symbols and evidence fingerprints as the renderer evolves.

Each topic distinguishes:

| Label | Meaning here |
| --- | --- |
| **SOURCE CLAIM** | What the transcript says, not independent verification. |
| **GENERAL ENGINEERING PRINCIPLE** / `[GENERAL PRINCIPLE]` | A bounded engineering observation stated separately from the video's empirical or universal claims. |
| **MUNDARIS CURRENT STATE** / IMPLEMENTED | Exists in inspected source; does not establish speed, visual quality or acceptance. |
| **REVIEWER ASSESSMENT** / `[MUNDARIS-INFERRED]` | Applicability reasoning; requires evidence before adoption. |
| **CANDIDATE EXPERIMENT** | A narrow question for future measurement, not an instruction to modify the engine. |
| `[MEASURED BY SOURCE]` | Speaker-reported experiment/timing, with only the context available in the transcript. Not reproduced here. |
| `[PROPOSED BY SOURCE]` | Suggested design, workflow or target. |
| `[WIP IN SOURCE]` | Speaker explicitly leaves research/testing/design incomplete. |
| `[OPINION IN SOURCE]` | Preference, interpretation or broad evaluative claim. |
| `[MUNDARIS-MEASURED]` | An identified retained Mundaris measurement; its fixture/revision limitations still apply. |

Relevance means **HIGH**: direct current issue; **MEDIUM**: current mechanism worth a bounded audit; **LOW**: little present leverage; **FUTURE**: relevant only once prerequisites exist; **NOT APPLICABLE**: the specific recommendation has no current counterpart. It is not an acceptance verdict.

### Common evidence contract

For any candidate, retain source/executable fingerprints, dirty state, backend/adapter/driver, scene/seed/body/radius, camera, viewport and resolution, layer toggles, admitted cover, quality/settled state, budgets and feature settings. Compare matched work and quality, not just similarly named fixtures. Use warm repeats and distributions (median/P95/worst) where appropriate; retain cold, moving and body-switch behavior separately. GPU timestamps, CPU preparation, upload API wall time, queue waits and native input-to-present time are distinct. Do not add nested timing scopes together. Inspect paired diagnostic snapshots and captures, including depth/coverage when relevant. Visual acceptance needs user review; tests alone do not establish it.

The same evidence contract applies to future experiments below. Source target timings are not Mundaris budgets: the transcript does not supply the complete target-hardware/resolution slide.

### Transcript chapter map

| Original chapter | Start | Reference topic |
| --- | --- | --- |
| Intro & Abouts, Resources, APIs | 00:00:00 | Starting-frame compute and source framing |
| The Importance Of Resource Clearing Management | 00:08:00 | Resource clearing |
| How To Properly Prepass Content | 00:14:21 | Depth/prepass |
| How To Properly Sort Deferred Basepass Content | 00:28:53 | Basepass ordering |
| Post Basepass Resource Management & Decal Notes | 00:36:44 | Post-basepass management |
| Perspective Shadow Maps & Deferred Stencil Volume Lighting | 00:39:00 | Shadows; lighting; specialized eyes/hair |
| WIP Global Illumination & Hybrid Reflections | 00:49:45 | GI/reflections |
| Screen Space Subsurface Scattering | 00:53:48 | Subsurface scattering |
| Direct Lighting Compositing & Effects Rendering | 00:57:21 | Direct-light compositing; sky/clouds; transparency/VFX |
| Tone Mapping & Exposure Mechanics | 01:04:45 | Tone mapping |
| AA (Non Temporal Aspect) | 01:19:38 | AA spatial component |
| AA (Temporal Aspect) | 01:24:25 | AA temporal component |
| Velocity Compensation | 01:42:40 | Velocity/motion effects |
| Motion Effects, Composite Effects, & UI/Gamma | 01:57:29 | Composite effects/UI |
| Outro & Bonus Slides | 02:00:29 | No recoverable technical bonus-slide content; see uncertainty notes |

## Inspected Mundaris source and retained evidence

These anchors are reused in topic sections to keep exact source ownership visible without repeating the entire inventory.

### S1 — Native frame and UI ownership

[crates/renderer/src/lib.rs](../../crates/renderer/src/lib.rs): `Renderer::new_async` selects a supported **sRGB surface format** (not a fixed native RGBA format), default wgpu instance/backends and default device limits. Requested optional features come from timestamp detection. `Renderer::render_frame` performs scene rendering, then an egui pass which **loads** existing scene color, or **clears** color when there is no scene. UI has no depth attachment. Surface presentation is FIFO. No separate HDR lighting or UI render target exists in this path. Windows/Linux x86-64 are the README's native targets; retained Vulkan results are not universal backend acceptance.

### S2 — Celestial passes, depth and persistent resources

[crates/renderer/src/celestial.rs](../../crates/renderer/src/celestial.rs): `CelestialRenderer::{new,resize,draw}`, `depth`, `pipeline`, `grow`, `bind_sphere_state`, `bind_history_state`, `bind_polyline_state`.

| Pass/resource | Format/sample count | Load/store and use |
| --- | --- | --- |
| Main celestial color | Native selected sRGB format, single sample | Clear dark background, Store; far spheres → terrain → ocean → clouds. |
| Main celestial depth | `Depth32Float`, 1 sample | Clear **0.0**, Store; `GreaterEqual` reverse-Z; no stencil aspect. |
| Atmosphere color | Same scene target | Load, Store; separate pass samples completed depth, no attached depth. Conditional on atmosphere enabled. |
| Guide/history overlay color/depth | Same color and depth | Both Load/Store; alpha-blended lines/curves depth-test without depth writes. Conditional on nonempty data. |
| UI color | Same scene target | Load/Store after a scene; otherwise Clear/Store ([S1](#s1--native-frame-and-ui-ownership)). |

`depth` creates a reusable `RENDER_ATTACHMENT | TEXTURE_BINDING` texture. Resize recreates it and the atmosphere depth bind group. Buffers, pipelines, uniform groups and icosphere indices persist; vertex/line buffers retain capacity but nonempty data is uploaded each frame. The main clear initializes sky, uncovered areas and regions outside the content scissor. No observed clear-only fullscreen draw, G-buffer or transient render-target alias pool exists in this production path.

### S3 — Terrain GPU buffers, uploads and draw grouping

[crates/renderer/src/planet_surface/gpu.rs](../../crates/renderer/src/planet_surface/gpu.rs): `PlanetSurfaceRenderer::{new,upload,draw}`, `pipeline`, `buffer`, `binding`. Reusable u16 indices cover **16 stitch variants**. Current sample storage is **48 B**, instances **64 B**, clipped/morph vertex stride **80 B**; historical Phase 4 sample32/fallback64 text is not today's layout. `upload` writes the 64 B lighting uniform and every nonempty samples/instances/fallback byte vector through `Queue::write_buffer`, regardless of unchanged geometry. Capacity retention is **not keyed GPU geometry residency or dirty-upload avoidance**.

Growth rounds to 4096 B, enforces **80 MiB** aggregate GPU surface capacity/default-device limits, waits for prior submissions with `Device::poll(Wait)`, destroys old buffers, and rebinds changed storage. Regular draws are direct indexed instancing, at most 16 nonempty mask buckets; clipped/morph fallback is a direct non-indexed draw. No indirect-draw or compute-culling path exists here. Pipelines write `Depth32Float` reverse depth, opaque color, one sample. A `STORAGE` usage flag does not imply compute execution.

### S4 — CPU preparation and authority boundary

[crates/renderer/src/planet_surface/prepare.rs](../../crates/renderer/src/planet_surface/prepare.rs): `SurfaceStaging::{clear,append_inner,append_stitched,append_transition}`; [view.rs](../../crates/renderer/src/view.rs): `PreparedView::prepare_source`, `PreparedRenderFrame::view_displacement`; [terrain_geometry.rs](../../crates/renderer/src/planet_surface/terrain_geometry.rs): `GeneratedSurfacePatch`; [transition.rs](../../crates/renderer/src/planet_surface/transition.rs): `SurfaceTransition`.

CPU preparation converts body-fixed f64 samples to observer-relative positions, classifies, reconciles shared packed boundaries, checks precision and packs outgoing bytes. `append_transition` samples the morph fraction, clips and packs triangles on CPU. `clear` resets vector lengths while retaining capacity; this is **not a GPU resource clear**. The precision-proof cache does not cache the entire packed payload. The instance order is stable stitch-mask bucketing, not a camera-depth sort.

[crates/app/src/planet_terrain.rs](../../crates/app/src/planet_terrain.rs): `TerrainPatchCache::{new_with_workers,get,report}` and `TerrainGeometryIdentity`; [planet_terrain/workers.rs](../../crates/app/src/planet_terrain/workers.rs): bounded CPU generation/construction; [planet_terrain/adaptive.rs](../../crates/app/src/planet_terrain/adaptive.rs): `AdaptiveTerrainCover::prepare_visible`; [terrain_population.rs](../../crates/app/src/terrain_population.rs): body-local population/opaque ownership handoff. Adaptive visibility uses expanded bounds/frustum rejection, excludes old geometry replaced by an active morph, and retains conservative terrain behavior. No GPU depth-pyramid occlusion is implemented. CPU raw/stitch sample sharing through `Arc` is separate from GPU residency.

World terrain definitions remain authoritative; disposable GPU caches may not replace them. Follow [ADR 0002](../adr/0002-reference-frames-and-precision.md) and [ADR 0006](../adr/0006-planet-surface-topology-and-lod.md), rechecking their historical inventories against current code.

### S5 — Planetary layers and forward shading

[crates/renderer/src/planetary.rs](../../crates/renderer/src/planetary.rs): `PlanetaryRenderer::{new,upload,draw_shells,atmosphere_group,draw_atmosphere}`, `shell_constant`; [shaders/planetary.wgsl](../../crates/renderer/src/shaders/planetary.wgsl): `vs_main`, `fs_ocean`, `fs_cloud`, `fs_atmosphere`, `factored_hit`.

Ocean/clouds use full-content-viewport triangles with analytic ray/shell intersections and fragment depth output. Ocean writes depth and opaque color; clouds alpha-blend, test depth, do not write it, and discard missing/low-coverage pixels. Atmosphere alpha-blends in a separate pass, `textureLoad`s completed depth, truncates integration at opaque depth, and uses 12 view steps with 4 sun steps each. It does not use hardware depth/stencil rejection in that pass. These are visual shells/single-scattering approximations, not volumetric-cloud grids or transparent-object/VFX systems.

[shaders/planet_surface.wgsl](../../crates/renderer/src/shaders/planet_surface.wgsl): `Sample`, `Instance`, `vs_main`, `vs_clipped`, `fs_main`; [shaders/celestial.wgsl](../../crates/renderer/src/shaders/celestial.wgsl): `fs_main`. Terrain uses forward directional diffuse/ambient and procedural Natural palettes; ocean has a sky-reflection approximation/specular lobe and a local `rgb/(1+rgb)` compression. That local curve is not a frame-wide HDR tone-mapping architecture. No shadow-map resources, deferred shading-model lighting, GI/probes, SSR/world-space reflection tracing, screen-space SSS, motion-vector attachment, AA history, bloom, lens/rain or motion-blur chain exists in these production paths.

### S6 — Projection, capture and profiling

[crates/renderer/src/celestial_view.rs](../../crates/renderer/src/celestial_view.rs): `CelestialProjection::try_new` uses `Mat4::perspective_infinite_reverse_rh`. Reverse-Z is IMPLEMENTED for celestial/terrain; [debug.rs](../../crates/renderer/src/debug.rs): `DebugProjection`, `DebugRenderer::draw` use a separate forward-depth fixture (clear 1.0). Do not conflate them.

[terrain_capture.rs](../../crates/renderer/src/terrain_capture.rs): `TerrainCaptureRenderer::{new_async,render}` owns a persistent `Rgba8UnormSrgb`, single-sample color target plus mapped readback buffers, and invokes production `CelestialRenderer`. Capture readback is not production terrain upload staging. [gpu_profile.rs](../../crates/renderer/src/gpu_profile.rs): `CpuUploadProfile`, `GpuProfile`, `CelestialQueries`, `AsyncTimestampSlot::{available,resolve,map}` provide optional pass/inside-pass queries and a bounded asynchronous native readback slot. Query resolve means **timestamp-query resolution**, not MSAA color resolve. `upload_api_duration` is CPU wall time, not DMA time; latest native queries can be stale and skip busy frames. UI and isolated clear timings are not current named GPU scopes.

### E1 — Retained checkpoint, not fresh working-tree performance

Underlying [5.11E final static manifest](../evidence/phase511e/after-final/views/phase59-10000-manifest.txt), [commands](../evidence/phase511e/after-final/commands.json), [source fingerprints](../evidence/phase511e/after-final/source-sha256.csv), [summary](../evidence/phase511e/summary.json), [evidence index](../evidence/phase511e/README.md) and [report](../PHASE_5_11E_TERRAIN_RECOVERY_REPORT.md) identify the measurements. Source inspection above agrees with the retained architecture limitations; fingerprints have not been asserted identical to today's larger dirty tree.

`[MUNDARIS-MEASURED]` **Static `phase59-10000`, 1152×768, 60° FOV, 400 km Earth radius, 10 km sampled-terrain clearance, RX 9070 XT/Vulkan, 5.11E final dirty checkpoint based on d81bb2a**:

- 575 visible patches, 292,912 terrain triangles, 9 terrain draws + 1 sphere + 3 layer draws.
- Unchanged repeats still upload **8,013,264 B**, including the 64 B lighting uniform. Persistent surface capacity **8,063,632 B**, last-repeat growth/wait counts zero; no keyed residency hit census.
- Matched planetary preparation: **13.9391 ms median**, 20 repeats. This is CPU preparation, not native FPS or total frame time.
- Last of eight all-layer query repeats, **not medians**: main celestial pass **0.43520 ms**, terrain **0.28524 ms**, ocean **0.02112 ms**, clouds **0.12392 ms**, atmosphere **0.18388 ms**, remaining sphere **0.00048 ms**. Terrain/ocean/cloud/sphere scopes are inside the main pass; atmosphere is a separate pass and also the same measurement under its layer name. Do not sum enclosing and enclosed scopes.
- `quality_pending=true`, `settled=false`, 800-update limit reached, one pending patch. Repeat images are internally identical, but not evidence of settled quality. Before/after covers differ; their timings do not isolate a speedup.
- Final descent route medians/P95 terrain payload: **3,874,208 / 5,308,960 B/frame**, worst **8,563,360 B**. The 128 MiB CPU accounted cap is reached with zero headroom. Unique-allocation census/RSS/driver VRAM remain unmeasured.

The report still leaves Acceptance A and the historical 114.533 ms event unresolved. Nothing in this research reference closes those gates or measures a new optimization.

## 1. Starting-frame compute / CPU versus GPU responsibility

Source: **00:00:00–00:07:57**; technical responsibility argument **00:05:16–00:07:57**.

### Source recommendation

**SOURCE CLAIM** `[OPINION IN SOURCE]`: keep starting-frame compute small; do not shift animation/geometry tasks to an already busy GPU merely to compensate for inefficient CPU code. The speaker attributes some GPU-driven trends to CPU bloat, cites Blueprints as “up to 10 times” slower than C++, and treats AC Unity's crowd-heavy design as a special case rather than a general workload. Those ratios and design judgments are not Mundaris evidence.

The introduction says the chart targets a specific scenario, hardware and visual standard. Gray pass boundaries can change ordering/existence with testing; red boxes need custom engine tests; question marks mark unfinished research/video discussion (00:05:16–00:05:50). `[MEASURED BY SOURCE]` At 00:04:44 the speaker reports some DX11 pixel shading about 2× DX12 speed, **only tested in Unreal API implementations**, and explicitly leaves Vulkan comparisons unresolved. It might be an engine implementation problem, not API behavior.

### Why the source says it matters

It argues the GPU has many frame stages and finite throughput; adding work can increase its critical path even when CPUs can handle that work. Its CPU-language and crowd arguments are causal interpretations, not controlled proof of a universal division of labor.

### Important exceptions

The source allows reasonable small compute processes. It does not give their full diagram-only list. Its scope does not establish that all animation, geometry management or compute workloads should stay on CPU.

### Mundaris today

**MUNDARIS CURRENT STATE:** [S3](#s3--terrain-gpu-buffers-uploads-and-draw-grouping) and [S4](#s4--cpu-preparation-and-authority-boundary) show CPU terrain generation, conversion and morph evaluation followed by GPU rasterization. No starting-frame compute dispatch exists in the inspected production renderer. The strong current evidence is repeated conversion/upload work ([E1](#e1--retained-checkpoint-not-fresh-working-tree-performance)), not CPU scripting bloat or GPU overload.

### Relevance

**HIGH** for ownership/data movement; **LOW** for the video's API/Blueprint judgments. **REVIEWER ASSESSMENT** `[MUNDARIS-INFERRED]`: measure elimination of repeated work before choosing its processor. Persistent derived geometry and bounded GPU interpolation can be sensible without moving authoritative terrain truth or astronomical subtraction to GPU.

### Candidate investigation

**CANDIDATE EXPERIMENT C01–C04:** census unchanged payloads; separate repeated CPU classification/conversion from view-dependent work; isolate morph preparation; measure buffer-growth waits. These are Mundaris extrapolations of the responsibility/reuse principle, **not techniques demonstrated for planetary terrain by the video**.

### Required evidence

CPU stage distributions, uploaded bytes/calls, residency/dirty reasons where a prototype exists, GPU scope time, growth stalls, precision/seam proofs, memory peaks, useful-quality convergence and native latency on matched static/moving/descent/body-switch routes.

### Do not do blindly

- Do not use this opinion to veto GPU terrain residency or to move all terrain/world work to GPU.
- Do not downcast global coordinates, weaken precision, hide CPU fallback, or change authoritative determinism to show a faster prototype.
- Do not recommend an API switch from Unreal-only comparisons; wgpu/backend/support evidence is required.

## 2. Resource clearing management

Source: **00:08:00–00:13:59**.

### Source recommendation

**SOURCE CLAIM:** prefer accelerated hardware clears for resources needing initialization; skip a clear when subsequent writes safely replace all relevant contents. It says albedo/normals and fullscreen AO can be overwritten; clearing depth supplies the validity context. Emissive diffuse, dual-normal skin data and possibly motion/MSAA-coverage storage still need clears because only selected materials write them. Specular RGBA16 is proposed for later recycling instead of an initial clear.

`[PROPOSED BY SOURCE]` “**Hijack clear**” (00:10:13): while a last-frame shader writes its main output, write clear-equivalent values to additional outputs for the next frame. Avoid clear-only fullscreen quads/triangles; the speaker says these are *always* slower than hardware clears on modern hardware. Recycle targets for later effects when their lifetimes allow it.

### Why the source says it matters

Overwriting is claimed to remove redundant initialization/write traffic and invocations. `[MEASURED BY SOURCE]` KCD2 RGBA8 clears are reported 2–3× faster than several other engines (00:10:45–00:11:17). The UE/Crysis comparison has **different work**: Crysis integrates filmic processing, bloom resolve and fog upscale (00:11:48). The speaker's “probably minimum” 0.1 ms saving (00:12:55) is an estimate, not a measured Mundaris floor.

### Important exceptions

**GENERAL ENGINEERING PRINCIPLE** `[GENERAL PRINCIPLE]`: every later read/blend must see a valid new value or an explicit validity guard; clearing depth alone does not initialize unrelated color. Partial coverage, sky, scissors, discard, blending, material-specific targets and MSAA subsamples can preserve stale contents. “Almost free” extra outputs still need bandwidth/lifetime measurement. Shadow/effect clear behavior is explicitly left needing more testing (00:08:01).

### Mundaris today

**MUNDARIS CURRENT STATE:** exact pass operations/formats are in [S1](#s1--native-frame-and-ui-ownership) and [S2](#s2--celestial-passes-depth-and-persistent-resources). Main color/depth clear once; atmosphere, overlays and UI load existing contents. Spheres/terrain do not cover every pixel; planetary shaders discard and blend. The initial background clear is necessary under the current coverage contract, and reverse depth must initialize to 0. The existing fullscreen planetary draws **shade/composite layers**, not clear resources. `SurfaceStaging::clear` is CPU vector reuse, not a render-target clear.

### Relevance

**MEDIUM** for clear/load/store audit; **NOT APPLICABLE** to current G-buffer/hijack-clear recipes. **REVIEWER ASSESSMENT:** no redundant production color clear was established by inspection. Persistent depth/capture resources already exist; there is no current postprocess target pool to recycle.

### Candidate investigation

**C06:** build a producer/consumer/coverage lifetime map and isolate actual clear/pass-store cost at fixed resolution. **C04:** distinguish persistent capacity, repeated upload and in-flight growth generations. **F01:** only when postprocess resources exist, compare sequential target reuse and guaranteed overwrite against separate allocation/clear.

### Required evidence

GPU clear/whole-pass timestamps or an external GPU capture, resource lifetimes, target-format/sample-count matrix, corner/sky/scissor/discard captures, resize/recreation behavior and zero stale-pixel artifacts. Existing main-pass queries cannot isolate clear cost.

### Do not do blindly

- Do not remove the main color clear because the frame has a fullscreen triangle somewhere.
- Do not use `Load` on uninitialized/newly acquired resources, or discard depth before atmosphere/guide consumers finish.
- Do not adopt last-frame hijack clears without proving first-frame, resize, skipped-frame, async and blend safety.
- Do not translate the source's “always faster” claim into a wgpu/backend law.

## 3. Depth/prepass strategy

Source: **00:14:21–00:28:42**.

### Source recommendation

**SOURCE CLAIM:** generate early final depth using a **selective/partial, depth-only prepass** of large close opaque occluders; shade prepassed content later with equal-depth testing. Exclude per-pixel alpha texture sampling/displacement and distant tiny geometry. Motion-vector or visibility passes can also act as prepasses, but the speaker strongly criticizes UE mesh-shader/software-raster visibility paths as slow/missing effective hardware depth behavior; that is attributed, workload-specific criticism.

Sub-techniques: hardware depth/stencil rejection rather than shader branching for eligible pixels; inspect **overlap overdraw separately from 2×2 quad overdraw**; choose occluders by useful screen area rather than triangle count alone; reorder triangles and reduce vertex bits according to UV/color/material needs; artist-friendly preprocessing/checkmarks. It allows terrain per-vertex texture displacement as a possible exception based on Fox Engine, not per-pixel sampling. Rare dual-normal/emissive objects may receive dedicated motion/coverage prepass outputs to stay below MRT limits; emissive alpha-tested content may require emissive output in the main basepass instead (00:28:09–00:28:42).

### Why the source says it matters

Depth-only passes are claimed to avoid expensive texture/MRT work while rejecting later hidden fragments. Repeating opacity sampling can erase that advantage; tiny triangles pay helper-quad/edge cost twice while producing little useful occlusion. `[MEASURED BY SOURCE]` Alpha-tested examples report a 0.13 ms draw, ≥13% improvement with prepass removed/depth writes disabled, about 40% efficiency improvement after enabling depth writes, and another 0.6 → 0.44 ms example (00:19:50–00:22:04). The latter could not be made depth-writing in the debugging test. These are examples, not a proof to exclude all alpha-tested prepasses.

### Important exceptions

The source's 70%-surface/three-failed-quad discussion (00:24:51) and <0.3 ms Half-Life: Alyx target (00:26:33–00:27:05) lack complete portable measurement context. It says Forward+ usually needs a full alpha-tested prepass, **then acknowledges consultants discussing alternatives** (00:27:05–00:27:37). Occlusion reuse is not well researched in the source (00:28:56).

### Mundaris today

**MUNDARIS CURRENT STATE:** [S2–S5](#s2--celestial-passes-depth-and-persistent-resources) show a single opaque forward terrain/sphere depth-writing stage, no prepass/stencil. `PlanetSurfaceRenderer::draw` groups stitch variants; `fs_main` includes procedural material ALU. Distant projected triangles, helper quads and expensive material work are relevant diagnostic questions, but no current quad-overdraw/prepass comparison was located. Current shader outputs/body-fixed normals/precision fallback are not the video's compact depth-only asset layout.

### Relevance

**MEDIUM** for triangle/quad and vertex-payload audit; **FUTURE** for introducing a prepass. `[MUNDARIS-INFERRED]` retained CPU preparation dominates the static checkpoint, so duplicating geometry submission is not the first intervention.

### Candidate investigation

**C08/C09:** profile projected triangle size, overlap, ordering and actual fetch/packing bytes without changing LOD thresholds. **F02:** after residency work, compare no prepass against a selective large-occluder prepass on an occlusion-heavy terrain scene, holding cover/shading constant.

### Required evidence

GPU total opaque cost including both passes, CPU encode/preparation cost, fragment/helper-quad or equivalent vendor counters, vertex fetch, triangle-area/depth images and matched seams/near-plane precision. Include low-overdraw scenes where the prepass may lose.

### Do not do blindly

Do not implement a full prepass just because depth exists; do not double CPU conversion/uploads; do not equate pixel-invocation count with quad utilization or polycount with cost. Preserve exactly matching depth geometry/morph state and shared-depth ownership before considering equal-depth reuse.

## 4. Basepass ordering, materials and LOD observations

Source: **00:28:53–00:36:32**.

### Source recommendation

**SOURCE CLAIM:** after the selective prepass, draw close alpha-tested content omitted from it first; then non-prepassed opaque geometry; then remaining alpha-tested content using accumulated scene depth; finally prepassed content with the appropriate outputs. This is a specific proposed deferred ordering, not simply “all foliage first.”

Use **32-bit reverse-Z depth** (00:30:34). Proposed material layout includes RGB10A2 normals with spare flag/channel bits; the G-buffer layout is **undecided**. A **dual material profile (DMP) ID**, distinct from shading-model ID, selects a precreated pair of BRDF/material profiles; repurposed metallic values blend those profiles. Store higher-precision index of refraction in profiles and let specular scale it; preprocess legacy cavity/specular edges or investigate baked bent-normal replacement. The source mentions an 8000-slot limit without enough layout detail to reproduce it.

Use ALU two-position jittered **2×2 Bayer LOD dithering**, including shadow-map draws to prevent shadow popping. Modernize specular anti-aliasing. `[PROPOSED BY SOURCE]` Bake shared flat discontinuity information into vertices to suppress redundant deferred-MSAA lighting at coplanar mesh seams while retaining bent/lone edge coverage from `SV_Coverage`. Quad-friendly retriangulation and tiled-mask material workflows are further ideas, not specified implementations.

### Why the source says it matters

The ordering tries to obtain alpha-test occlusion without paying twice for expensive opacity sampling. Profile specialization is intended to improve expressiveness and throughput; compact flags and coverage masks limit redundant lighting/sample work. Multi-sample rasterization/sample-frequency shading are described as quad-overdraw multipliers.

### Important exceptions

The “10-bit normals” versus “drop in quality using 24-bit normals” wording is technically ambiguous; preserve it as uncertainty rather than choosing a format on its basis. Coverage baking still needs shaders/tools (00:36:01); the G-buffer proposal and bent-normal replacement are unfinished. The source's preference for deferred over Forward+ depends on its MSAA/material scenario.

### Mundaris today

**MUNDARIS CURRENT STATE:** reverse-Z is IMPLEMENTED ([S6](#s6--projection-capture-and-profiling)); terrain is opaque forward shading, with stitch-mask rather than camera-depth buckets ([S3/S4](#s3--terrain-gpu-buffers-uploads-and-draw-grouping)). There is no alpha-tested foliage/asset basepass, DMP G-buffer, `SV_Coverage` pipeline or specular-AA asset workflow. Terrain transitions are **CPU-evaluated common-refinement geometric morphs**, not dithered LOD opacity transitions. Shader classification data is not a deferred material/shading-model ID buffer.

### Relevance

**MEDIUM** for current depth/order diagnostics; **FUTURE** for material/coverage workflows; **NOT APPLICABLE** to replacing current terrain morphs with the video's Bayer transition recipe.

### Candidate investigation

**C08:** compare stable front-to-back ordering within existing mask buckets on matched cover. **F03:** when cutout assets exist, compare the exact staged alpha-test order. **F04:** separately test shading-profile/coverage packing and flat-edge coverage correctness on a future deferred material prototype.

### Required evidence

Sort CPU cost, GPU opaque cost, overdraw/quad counters, overlap heat maps; future material precision/BRDF references, coverage and coplanar-edge images, sample-frequency lighting cost, aliasing during motion and shadow/LOD transitions.

### Do not do blindly

Do not redesign material ownership, switch to deferred, compress normals or dither authoritative terrain seams from this proposal. Reverse-Z existence does not imply depth-equal prepass reuse, shadow maps, stencil or MSAA coverage tracking.

## 5. Post-basepass resource management, resolves and decals

Source: **00:36:44–00:38:57**.

### Source recommendation

**SOURCE CLAIM:** learn from Crysis 3's sample-zero G-buffer extraction and pixel-frequency MSAA stencil, but the proposed pipeline **does not** create non-MSAA G-buffer copies because its target hardware is assumed to tolerate bandwidth. Instead construct sample-frequency coverage/shading-model stencil data and write the information into hardware depth/stencil; assign empty/disabled subsamples and sky a shared stencil value.

The Days Gone-inspired repacking discussion combines hardware depth and displacement before decals, then combines decal buffers with shifted outputs. The proposed chart avoids decal buffers and reuses the upcoming 2× MSAA specular RGBA16 target as temporary depth/data storage. Generate later resources afterward and perform occlusion queries. Baked-lighting-bound workflows are proposed. Fox Engine-style decals write channels except shading-model ID; an artist-defined opaque threshold could override material-expression ID.

### Why the source says it matters

It aims to avoid extra resolve steps/targets and improve sample-selective lighting access through stencil management. Avoiding decal buffers is claimed to save memory and compositing work while preserving base-object shading behavior.

### Important exceptions

**GENERAL ENGINEERING PRINCIPLE:** lifetime-compatible target reuse must preserve format, sample count and every later consumer. The spoken **32/44-bit depth/displacement packing** and “sample frequency stencil” description do not specify a reproducible bit layout or native stencil semantics. Hardware bandwidth tolerance is an assumption. Do not invent the missing diagram details.

### Mundaris today

**MUNDARIS CURRENT STATE:** no G-buffer, displacement-depth resolve, decals or stencil attachment ([S2/S5](#s2--celestial-passes-depth-and-persistent-resources)). `resolve_query_set` in [S6](#s6--projection-capture-and-profiling) resolves timestamps, not scene samples. Depth is retained and sampled by atmosphere; it is not packed into lighting channels. Reusing that depth texture across frames is already implemented, not post-basepass target aliasing.

### Relevance

**FUTURE** for target lifetime/resolve planning; **NOT APPLICABLE** to current stencil/decal repacking.

### Candidate investigation

**F01:** later compare target lifetime reuse without MSAA resolves against separate intermediate targets. **F05:** if decals are approved, compare direct compatible target writes with decal-buffer composition, including material override semantics.

### Required evidence

Resource live-range/peak-memory map, depth precision and MSAA coverage validation, whole resolve/decal GPU time, bandwidth counters, overlap/edge correctness and supported-backend format/usage capability.

### Do not do blindly

Do not introduce depth repacking, stencil resources or decal workflow restrictions just to match the chart. Do not infer hardware stencil support from `Depth32Float`, or equate avoiding a resolve with avoiding all synchronization/bandwidth.

## 6. Shadows

Source: **00:39:00–00:41:12**, **00:43:19–00:47:42**.

### Source recommendation

**SOURCE CLAIM** `[PROPOSED BY SOURCE]`: use **perspective shadow maps** with player/light bounds and FOV scaling to concentrate shadow density; scale map resolution with on-screen demand/distance, reportedly following a “1080p plus 8% rule,” under a preferred 2048² ceiling. Cull casters whose shadows cannot affect the visible light/view. Avoid simply raising resolution globally.

For sun/moon, compare perspective versus properly ratioed orthographic cascades. Consider KCD2's five cascades with farther cascades updated across frames; camera cuts could first render those farther maps smaller and restore the three-frame schedule. Approximate distance shadowing via Days Gone foliage atlases or SDF shadows with native projection; combine techniques through a native-resolution 8-bit projected shadow channel and sample-frequency masks. At sufficient distance, replace per-light shadow projection with noiseless/high-sample screen-space shadows. The speaker prefers barely softened shadows and limiting artist control to reduce bias/leaks.

### Why the source says it matters

Shadow depth draws are individually cheap but repeated across many lights/cascades. More resolution means much more shaded depth area; small/distant alpha-tested or high-poly casters can dominate for little visible benefit. `[MEASURED BY SOURCE]` Crysis 3's six cascades reportedly cost ~1.2 ms; the speaker targets half that. Perspective density is claimed to reduce jaggies/leaks without multiplying total area.

### Important exceptions

`[WIP IN SOURCE]` MSAA shadow viability, precision, sun perspective-versus-orthographic quality, far-field strategy, cascade refresh under racing/cuts and blended approximations all need testing (00:39:03, 00:44:57–00:47:08). The “1080p plus 8%” metric is not defined sufficiently in speech. “Shadows are not expensive if optimized” and mandatory barely-soft artist settings are opinions, not universal requirements.

### Mundaris today

**MUNDARIS CURRENT STATE:** no raster shadow maps/cascades, caster lists or shadow buffers ([S5](#s5--planetary-layers-and-forward-shading)). `fs_atmosphere`'s local `shadow` checks analytic planet occlusion of sunlight, not terrain-shadow-map projection. Directional diffuse and ocean highlights do not establish cast shadows.

### Relevance

**FUTURE**, potentially important for planet-scale terrain/vegetation; no current shadow-pass cost to optimize.

### Candidate investigation

**F06:** compare view/light-fitted shadow density versus an orthographic baseline once terrain cast shadows are needed. **F07:** separately compare caster culling, scheduled far-cascade updates and a bounded distant approximation under rapid travel/body rotation/camera cuts.

### Required evidence

Per-map/cascade GPU distributions, caster/triangle/alpha-test counts, screen-space shadow density, update ages, leaks/bias/popping and traversal/cut/rotation captures across planetary/local scale. Include total memory and update CPU cost.

### Do not do blindly

Do not adopt 2048² or 0.6 ms as Mundaris limits, shadow floors as a blanket exclusion, or soft-shadow workflow restrictions without user visual goals. Perspective warping, stale cascades and approximate distant occlusion must prove correctness in moving planetary frames.

## 7. Lighting architectures and specialized shading

Source: **00:41:12–00:44:25**, **00:47:42–00:49:18**.

### Source recommendation

**SOURCE CLAIM:** stencil-volume lighting first rasterizes cheap light volumes to identify affected pixels; lighting is evaluated only where stencil permits, restoring previous stencil values in the lighting draw. Prefer a fullscreen lighting triangle with stencil restriction rather than reusing discontinuous volume geometry for expensive shading. Separate draws by shading model to specialize BRDF/profile inputs and bandwidth; only skin reads the dual-normal buffer/skin LUT. Use empty/sky stencil rejection before multisampled volume processing.

`[OPINION IN SOURCE]` It reports that every examined compute-all-lights-in-one-draw pipeline had slower singular-light-per-pixel cost than stencil lighting, and criticizes branch-heavy UE lighting/separate shadow evaluation. This is not controlled evidence that compute/clustered lighting is generally inferior.

`[WIP IN SOURCE]` Eyes/hair may use forward or tiled-deferred shading because their screen area is small. “Forward” here can mean mesh-context compositing rather than only lighting. The speaker suggests merging existing forward eye/skin-layer work and adding tear lines, adaptive pupils, pupil flares and iris shadows; workflows/shaders remain unfinished. `[MEASURED BY SOURCE]` additional eye layers in Days Gone/Callisto reportedly cost <0.2 ms, **in addition to** the eye shading model, not total eye cost.

### Why the source says it matters

Hardware stencil rejects irrelevant lighting invocations. Specialized shading avoids inputs/branches that other models do not need, creating room for better BRDFs and PCF. Fullscreen masked lighting is claimed to avoid light-volume mesh-edge quad waste.

### Important exceptions

Cost depends on light coverage/count/overlap, material distribution, MSAA, target formats and CPU draw overhead. Many fullscreen model draws are not inherently cheap. The source explicitly says multisampled depth/stencil makes volume work slower. Mesh-context eye/skin passes do not imply every transparent material should be a dedicated model pass.

### Mundaris today

**MUNDARIS CURRENT STATE:** `fs_main` shades terrain inline with a coherent sun direction; no local-light deferred targets, shading-model stencil or characters ([S5](#s5--planetary-layers-and-forward-shading)). Atmosphere's fullscreen scattering is not deferred fullscreen light accumulation.

### Relevance

**FUTURE** for multiple lights/materials; **NOT APPLICABLE** to replacing today's single-sun forward pass.

### Candidate investigation

**F08:** when many local lights justify it, compare stencil-volume/model-specialized lighting with a forward/clustered-compute baseline at identical lights/BRDFs. Character eye/hair features are retained as research, not a current optimization candidate.

### Required evidence

GPU time by light/shading model and total frame, shaded pixels/samples, bandwidth/input counts, stencil setup/restore cost, CPU draw cost and platform capability. Include sparse and heavily overlapping light scenes.

### Do not do blindly

Do not choose deferred or reject compute from the source's preference; do not add stencil/model passes to a single-directional-light renderer without a measured need. Character quality goals/workflows are product decisions.

## 8. GI and hybrid reflections — WIP

Source: **00:49:45–00:53:39**.

### Source recommendation

**SOURCE CLAIM** `[WIP IN SOURCE]`: the approach is “not concrete enough” and may change. Trace existing shadow information first, then fall back to world-space tracing. Avoid rendering reflective shadow maps (RSM color/normal/depth) every frame; feed indirect evaluation from existing shadow maps plus baked world-space information. Favor baked grids/noiseless probes with screen-space directional occlusion/GI and separate large-scale occlusion; static and dynamic objects can consume the same baked representation.

Use dynamic/ray-traced editor solutions for iteration, then bake suitable shipped content (DDGI is cited). Reflections likewise layer non-RT methods before world-space tracing; world-space RT representation should be near-voxel simplified, with dynamic cubemaps still possible.

### Why the source says it matters

`[MEASURED BY SOURCE]` RSM updates reportedly cost 0.3–0.6 ms/frame; the source contrasts this with **not generating** that extra resource. It proposes noiseless GI layers under 2 ms and less storage than lightmaps. Avoiding RSM construction does not make the remaining trace/bake/invalidation work free.

### Important exceptions

`[OPINION IN SOURCE]` Broad dismissal of lightmaps/noisy GI and claims about studios/influencers remain attributed judgments. Probe/grid artifacts may require additional occlusion/screen-space layers even in the source. It supplies no full reflection algorithm or storage/invalidation design.

### Mundaris today

**MUNDARIS CURRENT STATE:** no RSMs, baked probes/grids, screen-space GI/reflections or RT scene representation. Ocean's sky reflection approximation is not SSR, cubemap tracing or GI ([S5](#s5--planetary-layers-and-forward-shading)). Procedural/editable moving planets create representation/lighting invalidation questions absent from the demonstrated baked-game scenario.

### Relevance

**FUTURE**. **REVIEWER ASSESSMENT:** reuse useful existing information when it exists; do not create a bake requirement for editable terrain from this WIP proposal.

### Candidate investigation

**F09:** once shadow/indirect requirements exist, compare a small body-fixed probe/grid hybrid using shared shadow information with a matched alternative. Separately inspect reflection fallback coverage without assuming a voxel RT format.

### Required evidence

Update/bake/invalidation cost, storage by region, lighting changes/body motion/terrain edits, disocclusion/offscreen failure cases, leakage/noise and total GI/reflection GPU cost including preprocessing.

### Do not do blindly

Do not call reuse zero-cost GI, bake authoritative editable state into immutable lighting, or adopt a 2 ms budget/voxel RT scene from unspecified chart notes. Preserve WIP status.

## 9. Screen-space subsurface scattering

Source: **00:53:48–00:57:14**.

### Source recommendation

**SOURCE CLAIM** `[WIP IN SOURCE]`: diffuse-only blur with profile-ID lookup for scattered color; develop an MSAA-compatible implementation. Prefer conservative scattering that preserves shadows. Consider half-resolution SSS blended with native unblurred diffuse, but only with ID/depth-aware upscale to avoid edge artifacts. Investigate a raster/stencil-limited Crysis-like approach and possible merge with later compositing; lower sample counts may be adequate. Two-position 2×2 Bayer modulation is suggested if banding appears.

### Why the source says it matters

Restricting affected pixels and separating diffuse from specular avoids blurring unrelated shading. `[MEASURED BY SOURCE]` Callisto's half-resolution contribution blended at 80% is reported near-identical and ~3× faster, **but still has upscale artifacts**. Forced-format comparisons claim other ~3× differences; source fullscreen RGBA16 target ~1.4 ms and typical third-person ~0.3 ms are distinct scopes.

### Important exceptions

The speaker cannot recommend half-resolution SSS without the aware upscale. It is uncertain whether normals matter, questions excessive scattering and combined diffuse/specular blur, and leaves native/MSAA/performance design unfinished.

### Mundaris today

**MUNDARIS CURRENT STATE:** no screen-space SSS, skin/profile ID, diffuse/specular targets or MSAA ([S5](#s5--planetary-layers-and-forward-shading)). Atmosphere scattering is a different participating-medium approximation, not SSS; the similarly named concept does not establish this feature.

### Relevance

**NOT APPLICABLE** currently; **FUTURE** only if suitable character/material content is introduced.

### Candidate investigation

**F10:** conditional material prototype comparing native, half-resolution aware-upscale and masked scattering with matched diffuse/specular treatment.

### Required evidence

GPU cost versus skin screen area, format/sample matrix, ID/depth boundaries, shadow retention, color/profile correctness and animated edge captures.

### Do not do blindly

Do not introduce skin buffers for planetary atmosphere, accept the 80% blend universally or trade edge/shadow quality for a timing claim.

## 10. Direct-light compositing and precision

Source: **00:57:21–01:00:43**.

### Source recommendation

**SOURCE CLAIM:** combine separately accumulated diffuse/specular into an RGBA16 scene-lighting result before SDR/HDR tone mapping; separation mainly supports diffuse-only screen-space SSS and possible standalone skin-specular blur. Compare UE's checkerboard R11G11B10 extraction with full-resolution alternatives, KCD2's lower-precision skin-diffuse exception and Dead Space's RGBA16 secondary diffuse.

Crytek's two R11G11B10 lighting buffers combine with base color into RGBA16 because, as attributed to Crytek, precision is needed when material colors enter. The speaker explicitly wants SDR/HDR error tests before accepting that approach. Tone mapping inside the material-color combine (Phantom Pain) is another investigation; VFX and later AA compatibility are concerns. Merge compatible fog/functionality here, comparing fog atlases with possible ALU fog.

### Why the source says it matters

It balances precision with target/input bandwidth and avoids redundant passes through merging. `[OPINION IN SOURCE]` RGBA16 is preferred because several admired games use it; **“maybe it's placebo or coincidence”** (01:00:11) is an explicit limitation, not evidence that one format produces better contrast.

### Important exceptions

“RGBA16” in the transcript does not always specify float/UNORM semantics. Combined lighting can be appropriate when diffuse separation has no consumer. SSS/specular-blur dependencies and multisampled outputs can preclude a simple merge.

### Mundaris today

**MUNDARIS CURRENT STATE:** one forward sRGB color output, no standalone diffuse/specular scene lighting ([S1/S5](#s1--native-frame-and-ui-ownership)). Local ocean color compression is not high-precision lighting compositing.

### Relevance

**FUTURE** for an HDR lighting pipeline; current framebuffer precision/gamma audit is **MEDIUM**.

### Candidate investigation

**F11:** if frame-wide HDR is approved, compare explicitly specified lighting formats and combined/separate target needs before choosing them; benchmark only compatible fog/composite merges. **C10:** first document today's local compression and sRGB/blend boundaries.

### Required evidence

HDR numerical range/error, SDR/HDR display captures and user evaluation, target bandwidth/memory and whole merged-versus-separate stage cost with identical work/effects.

### Do not do blindly

Do not assert RGBA16 universally improves contrast, add diffuse/specular targets without consumers or merge across transparency/AA requirements merely to reduce pass count.

## 11. Sky and cloud placement in the frame

Source: **01:00:43–01:02:22**.

### Source recommendation

**SOURCE CLAIM:** compute sky/clouds after direct-light resolution. Its cloud budget is intended to trade screen area with basepass/direct/indirect shading rather than be added unconditionally to a worst-case opaque budget. KCD2 measurements inform the chart; the speaker favors further GT7 photographic sky research. Horizon/Nixxes tooling prevented the intended comparison.

### Why the source says it matters

The argument is that sky-heavy and opaque-heavy views can be different worst cases. It claims an equilibrium can keep elaborate clouds from materially affecting performance. This is a pipeline/content-dependent proposal, not a guarantee.

### Important exceptions

Clouds over visible terrain, atmospheric haze and transparent layers can overlap substantial opaque area. The source mentions a ~59 FPS worst-case sum only for its chart; that is not Mundaris's performance ambition or acceptance criterion.

### Mundaris today

**MUNDARIS CURRENT STATE:** terrain → ocean → alpha-blended cloud shell in the main pass, then depth-sampled atmosphere, then guides/UI ([S2/S5](#s2--celestial-passes-depth-and-persistent-resources)). Clouds are not a separate expensive volumetric marching grid. The main clear supplies space/background; there is no general sky dome. Depth truncates atmosphere but does not prevent fullscreen invocation. Fragment-depth output/discard mean early-depth savings cannot be assumed from pipeline compare state alone.

### Relevance

**HIGH** for current layer work/coverage, with much smaller retained GPU than CPU preparation costs ([E1](#e1--retained-checkpoint-not-fresh-working-tree-performance)).

### Candidate investigation

**C05:** measure layer GPU cost versus projected body/content area using existing toggles at fixed scenes. **C07:** inspect how completed depth limits useful atmosphere/cloud work; later compare bounded coverage/depth rejection only in an approved prototype.

### Required evidence

Matched orbit/nadir/horizon/local/day/night/resolution sweep, layer-only queries, affected/invoked pixel counts, depth discontinuities, scatter/color/alpha captures and near-surface shell precision. Toggle timing diagnoses cost, not the acceptability of disabling a visual layer.

### Do not do blindly

Do not relocate or merge layers without preserving ocean/cloud/atmosphere depth/compositing and guide order. Do not treat “no atmosphere” as an optimization at equal quality or assume sky cost cancels opaque cost.

## 12. Transparent and VFX rendering

Source: **01:02:22–01:04:29**.

### Source recommendation

**SOURCE CLAIM:** render basic/forward-lit decals before glass/transparency; use Phantom Pain-style **half-resolution min/max-depth** rendering for low-frequency sprite effects. Reuse supporting half-resolution resources with SSS if compatible. Fine particles remain native resolution; mixing native and low-res effect batches can require two half-res clears. Write **responsive masks** in alpha for transparent/VFX materials to reduce later temporal blending, reusing half-res effect blend alpha when possible.

### Why the source says it matters

Half width/height reduce pixel invocation opportunities to roughly a quarter; min/max-depth support is intended to avoid common low-resolution depth-edge discontinuities. Responsive masks describe effects lacking trustworthy temporal motion/history.

### Important exceptions

Fine/high-frequency content may not tolerate half resolution. Clears/composites/resampling and depth construction must be included in total cost. `[WIP IN SOURCE]` DOF/bloom ordering is unresolved: examples composite with tone mapping, while the desired DOF follows AA and AA follows tone mapping (01:04:29). The source says existing responsive implementations still look poor with long history; a mask is not alone a fix.

### Mundaris today

**MUNDARIS CURRENT STATE:** alpha-blended clouds/atmosphere and guide lines exist, but no general glass/particle/VFX, low-res min/max-depth targets or responsive-mask outputs ([S2/S5](#s2--celestial-passes-depth-and-persistent-resources)). Cloud color alpha is present for blending, **not consumed as an AA responsive mask**. Ocean is opaque in its pipeline despite representing water.

### Relevance

**FUTURE** for general VFX. Current full-resolution layer audit is **HIGH**; half-resolution planetary layers require separate visual/precision validation, not automatic analogy to sprites.

### Candidate investigation

**F12:** with representative low-frequency effects, compare native versus half-res min/max-depth rendering including all clears/upscale/composite work. **F13:** evaluate responsive masks only alongside an approved temporal-AA prototype.

### Required evidence

Total effect GPU cost, overdraw/clear/target bytes, depth-edge and fine-detail motion captures, alpha sorting/blending, temporal ghosting and mask precision/history behavior.

### Do not do blindly

Do not downsample all transparency, treat alpha as a ready responsive mask, or add temporal outputs when no temporal consumer exists.

## 13. Tone mapping, exposure, white balance and LUTs

Source: **01:04:45–01:19:09**.

### Source recommendation

**SOURCE CLAIM** `[OPINION IN SOURCE]`: focus first on limited-RGB SDR; prefer a GT/GT7-like largely linear midtone curve with contrast toe and smooth shoulder over ACES, Reinhard or AgX looks. GT7's controlled hue shifts toward neutral/white are preferred over the speaker's interpretation of display mapping's washout. Real-world flashlight/perception discussion motivates these preferences; it is not an independently verified perceptual model here.

Sub-techniques: examine toe/shoulder/exposure together; preserve vivid properly exposed material colors; calibrate environment-aware auto exposure and bounded automatic white balance, with artist range/bias controls. Consider Fox Engine-style **diffuse-only, pre-base-color** luminance downsampling so dark albedo/emissives do not distort exposure. Exclude emissives and account for non-screen-space subsurface/direct-light opacity effects when white-balancing. Accelerate fullscreen tone mapping with a small LUT; prefer Callisto's **3D 64-pixel-resolution RGBA16 LUT**, test cheaper precision, and investigate nonlinear LUT sampling to reduce banding. BT.2020 rendering space remains a further direction.

### Why the source says it matters

Curve shape allocates limited display range; hue behavior and exposure affect perceived contrast/detail/color. Lighting-only exposure is claimed to avoid material-dependent adaptation errors. LUT evaluation moves complex transforms to a smaller resource and lookup.

### Important exceptions

`[WIP IN SOURCE]` auto-exposure documentation needs work (01:15:51); the GT7 diffuse-only resource is inferred from slides, not established (01:17:28). The proposed simpler exposure/white-balance path, emissive exclusion, perceptual transitions/HK effect, LUT precision and color-space adoption need further tests. Callisto console-value decimal shifts (01:09:16–01:09:51) are implementation observations, not universal GT parameters. “ACES destroys detail” and GT7 “accurate naked eye” claims remain source opinions.

### Mundaris today

**MUNDARIS CURRENT STATE:** sRGB native/capture outputs, forward terrain color and local ocean `rgb/(1+rgb)`; no frame-wide HDR intermediate, auto exposure, white balance, grading LUT or SDR/HDR display mapper ([S1/S5/S6](#s1--native-frame-and-ui-ownership)). Explicit shader sRGB encode/decode and hardware sRGB output are present, not tone-map LUTs.

### Relevance

**MEDIUM** for current color-boundary audit; **FUTURE** for the proposed display pipeline.

### Candidate investigation

**C10:** trace color/blend boundaries now. **F11:** later compare specified HDR formats/curve families plus exposure inputs and LUT precision on a fixed physically interpretable color/light fixture. GT/GT7 is a candidate, not a mandated look.

### Required evidence

Linear numerical ramps, dark/saturated/highlight/hue fixtures, adaptation sequences across ground/sky/night and materials, emissive bias, SDR/HDR displays, LUT error/banding/cost, UI legibility and user visual approval.

### Do not do blindly

Do not replace today's shader colors with a favored curve and call it optimization; do not infer diffuse-only buffer availability from a debug slide or override product art goals with perceptual assertions.

## 14. Anti-aliasing — spatial and temporal components

Source: **01:19:38–01:24:05** (non-temporal); **01:24:25–01:42:43** (temporal/coverage transition).

### Source recommendation

**SOURCE CLAIM:** base the proposal on **SMAA 4x**: SMAA S2x spatial processing on each of two MSAA subsample layers plus SMAA T2x-like temporal work. It assigns MLAA/SMAA to jagged-line smoothing and MSAA to sampling temporally unstable edges. Crysis 3's SMAA is the reference; improve high-contrast edge detection, line detection and pattern handling using cited presentation code, use smaller edge/LUT formats, and avoid stacking morphological algorithms to compensate for incomplete implementations.

`[MEASURED BY SOURCE]` SMAA S2x is reported near 1 ms in dense grass, usually lower; a cited SMAA optimization presentation reportedly gives a 40% performance increase, with no full code examples in that presentation. The proposed effect-versus-edge budget cancellation is a scenario assumption.

Temporal sub-techniques and distinctions:

- **Two-frame blending:** blend current spatial output with previous **spatial** output, not previous temporal output/recursive accumulation. Preserve this distinction: merely storing one history texture does not mean only one old frame contributes.
- **Narrow resolve** rather than wide reconstruction that the speaker says spreads edge/texture blur. UE `r.TemporalAA.FilterSize` 1 → 0.09 is a demonstrated comparison, not a portable shader specification. Slight sharpening may be viable if merged without halos.
- **Responsive masks** for particles/transparency lacking motion; accurate **full-scene motion vectors**, including foliage/deformation, for reprojection and **velocity weighting**. Combine velocity weighting with color clamping; depth-based rejection is mentioned via Filmic SMAA. The source distinguishes velocity-based blend/rejection from using velocity only to reproject history.
- **Edge-only temporal blending:** TSCMAA/FFXIV analysis suggests applying temporal work only where the spatial edge mask indicates it. The proposal would use its saved custom MSAA coverage mask in the motion-vector buffer instead.
- **Texture jitter compensation:** synchronize texture sampling with camera subpixel jitter to protect interiors; lighting jitter remains a separate unresolved problem. It considers offsetting lighting or geometry rather than camera.
- **Programmable sample locations (PSL):** proposal to match a 4× MSAA pattern with two samples across frames, or use a Decima-inspired vertical/horizontal pattern with neighbor samples for thin detail, potentially avoiding whole-image jitter. Keep this distinct from ordinary default MSAA sample patterns and from temporal reprojection.

### Why the source says it matters

It aims to preserve texture detail while stabilizing edges without long-history trails. MSAA coverage/model masks are proposed to restrict expensive sample-frequency lighting and later blending. Helper-quad waste, multi-sample rasterization and repeated shading interact; MSAA memory/sample work is not captured by its resolve alone.

### Important exceptions

`[OPINION IN SOURCE]` “SMAA is the only viable morphological option,” “TAA cannot smooth edges better,” “ghosting is impossible with two frames,” “near-free basepass motion vectors,” and comparisons condemning UE/upscalers are **source claims**, not general facts. The source itself later shows two-frame particles/foliage ghosting (01:33:42–01:34:48) and acknowledges some two-frame implementations ghost (01:28:11). Preserve that internal limitation rather than repeating the earlier absolute statement.

`[WIP IN SOURCE]` missing TSCMAA code, uncertain FFXIV resolve width, jitter compensation, lighting offsets, coverage baking, PSL pattern behavior and history coverage storage remain proposals (01:38:12–01:42:43). “99% comparable to 4× MSAA” is an unvalidated target. Claims that two samples/neighbors equal supersampling quality do not establish equivalent shader/material sampling. The transcript does not establish wgpu exposure or supported hardware for PSL.

### Mundaris today

**MUNDARIS CURRENT STATE:** single-sample scene/depth and default multisample states; no SMAA/MSAA scene resolve, temporal jitter/history, motion attachment, responsive/coverage mask, edge-only temporal blending or programmable sample location setup ([S2/S3/S6](#s2--celestial-passes-depth-and-persistent-resources)). Egui integration does not imply scene AA. Terrain's octahedral body-direction packing is not an AA edge mask.

### Relevance

**FUTURE** for AA architecture; **MEDIUM** for current triangle/quad-overdraw diagnosis before adding sample multipliers.

### Candidate investigation

**F13:** when AA is approved, compare no-AA, spatial SMAA, ordinary supported MSAA and a bounded temporal hybrid on identical planetary navigation sequences. Within that experiment separately toggle narrow/wide resolve, previous-spatial versus accumulated history, velocity weighting, responsive masks and edge-only blending; investigate PSL only after capability proof. **C08/C09:** establish existing geometry/quad/fetch costs first.

### Required evidence

Temporal sequences, disocclusion/cuts/body rotation/deforming terrain and thin horizon/guide detail, texture/shading sharpness, motion-vector accuracy, aliasing/ghosting, per-component and total GPU cost, MSAA memory/resolve bandwidth, CPU work and supported-backend feature checks. Do not judge temporal behavior from still screenshots alone.

### Do not do blindly

Do not port a DX semantic (`SV_Coverage`) or PSL proposal as if already supported; do not assume accurate motion across observer/frame re-expression or terrain morphs; do not label two-frame history ghost-free. Do not silently replace visual or performance acceptance with the source's 60 FPS assumption.

## 15. Velocity packing, motion blur and “velocity compensation”

Source: **01:42:40–01:57:00**.

### Source recommendation

**SOURCE CLAIM** `[WIP IN SOURCE]`: reconstruct camera velocity from scene depth and combine animated-object vectors into a non-MSAA packed velocity target before tone mapping/exposure. Days Gone example uses native **R11G11B10** packed velocity and a **120×68 RGBA16 max tile** resource generated in the same draw, then dilation; packing reduces later depth/vector inputs. Skip an unnecessary motion-blur output clear through guaranteed overwrite.

The tile/dilation stage is red-coded in the chart because it produces blocky object outlines. Need for Speed's no-tile approach avoids that artifact but demonstrates background streaks crossing foreground objects without proper depth-aware occlusion. The source leaves tile retention undecided.

Other sub-techniques:

- Distinguish per-pixel blur with camera/object motion from “per-object” blur which removes camera motion in velocity packing.
- Proposed depth-dependent camera contribution with a U-shaped/low-res focus mask, tunable cutoff and extreme-motion behavior; **single-direction trailing** rather than symmetric camera-like blur is preferred for the speaker's naked-eye interpretation.
- Phantom Pain example packs half-res RGBA8: RG camera-near precision-biased velocity; BA bilinearly upscaled neighborhood tiles. Its **120×168** tile dimensions are transcribed differently from the earlier example; do not silently unify them.
- Half-resolution scene-color downsampling is only a possible scalability option, not accepted as the reference path. The shown 4K → 1080p blur context matters.
- Uniform sampling plus a strong **separable second pass** is favored over white/interleaved-gradient/blue noise; checkerboard remains an experiment. Determine sample counts and trail length through R&D; source examples contrast roughly 3 samples/direction with 10–12 alternating-pixel samples and compare pass costs, not equal-work universal speedups.

### Why the source says it matters

Packed velocity reduces repeated input fetching; merged tile generation can avoid another traversal. Half resolution reduces work. A competent separable pass is claimed to smooth sample steps at fewer samples, and depth awareness avoids false background streaks/large tile outlines.

### Important exceptions

The speaker says a dedicated video/testing is still needed. Half-res packed-input adoption awaits a plausible single-direction implementation; tile artifacts, sample count, pattern and trail length are unresolved. “Velocity compensation” is the source's preferred terminology, not a standard engine requirement. Its claim that properly simulated trails are needed even at 500 FPS and its rejection of camera-symmetric blur remain perceptual opinions.

### Mundaris today

**MUNDARIS CURRENT STATE:** no velocity render target/tile resources/motion blur ([S5/S6](#s5--planetary-layers-and-forward-shading)). World/camera velocity values are not per-pixel previous/current render motion; frame changes and morphs would require explicit history correspondence.

### Relevance

**FUTURE**, after motion vectors and user-approved motion effects; not a current terrain upload fix.

### Candidate investigation

**F14:** conditional motion-effects prototype comparing packed input, tile/no-tile depth-aware filtering and uniform-plus-separable versus matched-sample alternatives. Establish unblurred visual references and user motion preferences first.

### Required evidence

GPU cost including packing/tiles/dilation/separable/upscale, memory/precision error, foreground occlusion, trail direction/length, motion/frame-rate/cut sequences and user comfort/clarity. No unexplained tile outlines or loss of thin distant detail.

### Do not do blindly

Do not compress unsigned velocity without a specified packing transform, assume world velocity equals screen velocity, add motion blur to hide instability, or mandate the source's perceptual style.

## 16. Composite effects, bloom, lens/rain, UI and gamma

Source: **01:57:29–02:00:18**; outro/bonus chapter **02:00:29–02:01:25**.

### Source recommendation

**SOURCE CLAIM:** use **half-resolution compositing buffers** for low-frequency flares/light-source bloom and rain/lens drops, with half-res hardware depth for effect depth testing. Recycle available RGBA16 half-res targets; Need for Speed uses R11G11B10 HDR composites. Combine rain/lens effects with bloom so drops appear to absorb local lighting, along with appropriate limited-use grain/grading/chromatic aberration. The speaker criticizes native-resolution Crysis composites as expensive with little visible benefit.

`[WIP IN SOURCE]` velocity-driven motion effects/bloom need more analysis. Post-tone-map bloom can retain scene luminance (Fox Engine), but the speaker wants bloom reacting to the motion-blurred result. DOF/order questions from 01:04:29 remain open. UI research is explicitly early: a separate UI target could allow gamma correction while overlaying; useful brightness/gamma settings should visibly scale their calibration image rather than delete it.

### Why the source says it matters

Half resolution reduces fill work/invocations and bounds artist-driven overdraw. Compatible composite merging reduces full-frame traversals. Separating UI/display transfer can make calibration consistent without modifying UI design every time display settings change.

### Important exceptions

High-frequency effects, readable text and sharp UI can lose quality at half resolution. Another UI target has allocation/bandwidth/composite cost; the source does not specify a validated UI architecture or total UI budget. Bonus slides have no recoverable technical transcription; no content is invented for them.

### Mundaris today

**MUNDARIS CURRENT STATE:** no bloom/rain/lens/postprocess composite targets; egui loads and draws directly into the selected sRGB scene surface ([S1](#s1--native-frame-and-ui-ownership)). Planetary alpha compositing already exists at full content resolution, but is not a half-res camera-effect stage. Shader/hardware sRGB handling exists; no separate configurable gamma/brightness calibration pipeline is established by these paths.

### Relevance

**MEDIUM** for today's scene/UI color boundary; **FUTURE** for half-res composites/bloom; **LOW** for now-absent lens/rain optimization.

### Candidate investigation

**C10:** verify sRGB/linear blends and UI output with numerical patches, layer toggles and full-frame UI captures. **F12:** later compare full/half-res low-frequency compositing including depth/upscale/clear costs. **F01:** audit shared target lifetimes. Bloom-after-blur remains a source R&D question, not an adopted ordering.

### Required evidence

Color ramps, known alpha composites, readable UI at native/high-DPI sizes, format/display handling; future total composite GPU cost and lens/bloom moving-edge/depth artifacts at matched visual quality.

### Do not do blindly

Do not lower UI resolution, double-apply sRGB/gamma, merge UI before future scene tone mapping, or add a separate UI target without measuring its need. The source's effect-style preferences do not override user goals.

# Mundaris Optimization Cross-Reference

**24 explicitly indexed candidate investigations:** **C01–C10** address present architecture; **F01–F14** are prerequisite-dependent future research. This counts bounded investigation groups, not every sub-technique or every table row. Related controls within F13/F14 are not counted as separate candidates. Established mechanisms and inapplicable source recipes are included for lookup but do not inflate that count.

| Technique | Source position | Mundaris status | Relevance | Next action |
| --- | --- | --- | --- | --- |
| C01 — unchanged terrain uploads / keyed persistent geometry | 00:05:50–00:07:57; reuse analogy 00:08:33–00:10:13 | PARTIALLY IMPLEMENTED: buffer capacity persists; keyed residency/dirty uploads NOT IMPLEMENTED (S3/E1) | HIGH | Census payload identity/invalidation; compare residency proposal with full uploads. Mundaris-derived application, not a video terrain recipe. |
| C02 — cache invariant CPU preparation/classification | 00:05:50–00:07:57; 00:27:37 | NEEDS MEASUREMENT: CPU sample conversion/classification/packing remain (S4/E1) | HIGH | Separate invariant data from observer/proof-dependent work. |
| C03 — CPU/GPU morph responsibility | 00:05:50–00:07:57 | ARCHITECTURALLY DIFFERENT: CPU common-refinement interpolation/clipping, not GPU animation compute (S4) | HIGH | Measure morph stage/bytes; assess bounded GPU path and conservative fallback with residency. |
| C04 — persistent capacity, growth waits and staging lifetime | 00:08:01–00:10:13; 00:36:48–00:38:26 | PARTIALLY IMPLEMENTED: bounded persistent buffers, synchronous growth drain (S3) | HIGH | Profile growth generations/stalls and unique allocations before proposing a different staging mechanism. |
| Hardware clears; avoid clear-only geometry | 00:08:01–00:10:45 | ALREADY IMPLEMENTED as attachment Clear operations; physical backend cost NEEDS MEASUREMENT (S2) | MEDIUM | Preserve valid initialization; no clear-only geometry defect found. |
| C06 — clear/load/store lifetime audit | 00:08:01–00:13:59 | NEEDS MEASUREMENT: valid initial clear, later loads; no redundant clear established (S1/S2) | MEDIUM | Prove producer/consumer coverage; isolate clear/store cost before changes. |
| Hijack clear | 00:10:13–00:10:45 | NOT RELEVANT CURRENTLY: no eligible multi-output/history target chain (S2/S5) | NOT APPLICABLE | Retain idea; require initial/skipped/resize-frame safety if a consumer appears. |
| F01 — postprocess target reuse / resolve avoidance | 00:09:08–00:10:13; 00:36:48–00:38:26; 01:58:34 | FUTURE PIPELINE; current depth/capture reuse is a different mechanism (S2/S6) | FUTURE | Inspect real target live ranges/format/sample capabilities after targets exist. |
| Reverse-Z / 32-bit depth | 00:30:34 | ALREADY IMPLEMENTED for celestial/terrain; debug fixture uses forward depth (S6) | HIGH | Maintain depth contracts; no reimplementation needed. |
| C07 — completed-depth exploitation | 00:15:58–00:17:38; 01:00:43–01:02:22 | PARTIALLY IMPLEMENTED: depth tests and atmosphere ray truncation; no Hi-Z/stencil (S2/S5) | MEDIUM | Measure useful versus invoked layer work; verify fragment-depth/discard behavior. |
| C05 — bound unnecessary full-viewport layer work | 01:00:43–01:02:22 | NEEDS MEASUREMENT: three analytic layer triangles at content resolution (S5/E1) | HIGH | Fixed-camera coverage/resolution/toggle sweep before any coverage prototype. |
| C08 — draw order, overlap and helper-quad audit | 00:23:43–00:30:01 | PARTIALLY IMPLEMENTED: stitch batching/backface culling/depth; no camera-depth bucket sort (S3/S4) | MEDIUM | Measure triangle size/overlap; include sort CPU cost in a matched comparison. |
| C09 — geometry preparation / payload specialization | 00:26:33–00:27:37; 00:34:56–00:36:01 | PARTIALLY IMPLEMENTED: compact shared indices and packed storage; sample48/instance64/fallback80 remain (S3/S4) | MEDIUM | Audit actual consumers/fetch/packing precision; no blind bit reduction. |
| Indirect rendering / GPU culling | 00:05:50–00:07:57; 00:28:56 | ARCHITECTURALLY DIFFERENT: direct instanced mask draws and CPU conservative visibility (S3/S4) | LOW | Reconsider only if measured submission/culling, not upload/preparation, dominates; not an extra counted candidate. |
| F02 — selective depth-only prepass | 00:14:21–00:28:42 | NOT IMPLEMENTED (S2/S3) | FUTURE | Compare no/selective prepass after residency and overdraw evidence. |
| F03 — staged alpha-tested basepass ordering | 00:28:56–00:30:34 | FUTURE PIPELINE: no cutout asset pass (S5) | FUTURE | Test with actual foliage/hair and matched texture/depth work. |
| Bayer opacity LOD/shadow dithering | 00:34:24–00:34:56 | ARCHITECTURALLY DIFFERENT: geometric terrain morph, no shadow-map LOD (S4) | NOT APPLICABLE | Do not replace terrain morph/seam ownership; revisit for future asset LOD. |
| F04 — profile-ID/coverage-specialized deferred layout | 00:30:34–00:36:01 | FUTURE PIPELINE: no G-buffer/DMP/MSAA coverage (S5) | FUTURE | Prototype only if material/deferred requirements justify it. |
| Depth/displacement + sample-stencil repacking | 00:36:48–00:38:26 | NOT RELEVANT CURRENTLY: depth-only format, no displacement resolve/stencil (S2) | NOT APPLICABLE | Resolve transcript/diagram layout uncertainty before any future use. |
| F05 — decal direct-write versus decal buffers | 00:38:26–00:38:57; 01:02:22 | FUTURE PIPELINE (S5) | FUTURE | Compare resolve/memory and material semantics with real decals. |
| F06 — view/light-fitted shadow density | 00:39:03–00:41:12; 00:44:57 | FUTURE PIPELINE: no maps/cascades (S5) | FUTURE | Establish cast-shadow need and orthographic reference first. |
| F07 — caster culling / far cascade scheduling / distant approximation | 00:44:25–00:47:42 | FUTURE PIPELINE (S5) | FUTURE | Camera-cut/high-speed/body-motion tests plus update-age/cost evidence. |
| F08 — stencil-volume/model-specialized lighting | 00:41:12–00:44:25 | ARCHITECTURALLY DIFFERENT: forward single-sun terrain shading (S5) | FUTURE | Compare with forward/clustered compute only when many lights exist. |
| F09 — existing shadow/baked grid/probe hybrid GI/reflections | 00:49:49–00:53:39 | FUTURE PIPELINE; ocean sky approximation is not GI/SSR (S5) | FUTURE | Preserve source WIP; test edits/rotation/invalidation and all preprocessing. |
| F10 — masked/native versus half-res aware-upscale SSS | 00:53:48–00:57:14 | NOT RELEVANT CURRENTLY: no skin/diffuse-only target (S5) | FUTURE | Conditional on actual SSS material requirements. |
| F11 — HDR format/compositing/tone curve/exposure/LUT study | 00:57:25–01:00:43; 01:04:49–01:19:09 | FUTURE PIPELINE: local ocean compression only (S5) | FUTURE | Compare explicit formats/curves/exposure inputs at equal quality; no mandated GT7 adoption. |
| C10 — current color, alpha blend and UI/gamma boundaries | 01:04:49–01:19:09; 01:59:43–02:00:18 | PARTIALLY IMPLEMENTED: sRGB output/encode handling; no separate gamma calibration chain (S1/S5) | MEDIUM | Trace numeric color/composite paths and paired UI captures. |
| F12 — half-res min/max-depth VFX/composite effects | 01:02:55–01:04:29; 01:57:59–01:59:08 | FUTURE PIPELINE; clouds are a different full-res shell path (S5) | FUTURE | Low-frequency effect prototype including clears/upscale; fine effects remain a separate case. |
| F13 — spatial/temporal AA and mask/history/resolve study | 01:19:39–01:42:43 | NOT IMPLEMENTED: single-sample scene/no history/motion masks (S2/S6) | FUTURE | Supported MSAA/SMAA baseline, then bounded history/velocity/edge-only controls; capability-check PSL. |
| F14 — packed velocity / depth-aware separable motion effects | 01:42:43–01:57:29 | NOT IMPLEMENTED (S5) | FUTURE | Motion correspondence and user visual goals before tile/filter/packing tests. |
| Separate UI target with overlay gamma | 01:59:43–02:00:18 | ARCHITECTURALLY DIFFERENT: egui direct surface pass (S1) | LOW | C10 determines need; no additional target justified yet. |

# Highest-Value Near-Term Audits

This is a research-derived ranking of **10 current investigations**, not an implementation phase. The top four address work directly supported by current source and retained CPU/upload/memory evidence. The remaining GPU/color audits have useful mechanisms but **unmeasured current benefit**. Ordering is approximate, not permission to begin them or to reorder user priorities. “Residency work” below refers to the existing unresolved terrain-residency direction; it is not claimed implemented or accepted.

## 1. C01 — Unchanged terrain payload and residency/invalidation census

- **Why now:** `PlanetSurfaceRenderer::upload` writes every nonempty payload; E1 records 8,013,264 B on every unchanged repeat. Capacity reuse has not removed transfers.
- **Affected:** `planet_surface/gpu.rs::PlanetSurfaceRenderer::upload`, `prepare.rs::SurfaceStaging`, `celestial.rs::CelestialRenderer::draw`, app cover/identity handoff.
- **Measure:** bytes and write calls by samples/instances/fallback/uniform; exact unchanged/dirty causes for static view, observer changes, body rotation, edits/revisions, morph and body switch; CPU packing/upload API, GPU execution, capacity/in-flight memory and native latency. Payload identity alone must not become an expensive per-frame hash workaround.
- **Sequence:** **before and during residency work** as its baseline/acceptance evidence. A useful prototype should show which data remains resident and why changed data is invalidated; it must preserve precision and complete coverage.

## 2. C02 — Repeated CPU conversion, classification and byte packing

- **Why now:** E1's 13.9391 ms preparation median outweighs its individual GPU scopes. `append_inner` recomputes body-fixed classification and observer-relative positions before repacking. A retained all-layer record shows conversion 2.8369 ms, classification 2.6402 ms, boundary narrowing 3.2965 ms, packing 2.9806 ms within nested total preparation; these are **one record**, not medians or additive independent frame costs.
- **Affected:** `prepare.rs::SurfaceStaging::{append_inner,append_stitched}`, `view.rs::PreparedRenderFrame`, generated sample/classification ownership.
- **Measure:** CPU stage distributions and dependency/reuse keys, static versus camera/body motion, proof hits/misses, actual shared boundaries and f32 projection error. Identify work invariant under observer changes without removing required view proofs.
- **Sequence:** **before residency design decisions and during that work**; separate truly invariant metadata from observer transforms. Reprofile residual CPU work afterward rather than assuming a GPU cache removes it automatically.

## 3. C03 — Transition interpolation/clipping and outgoing fallback work

- **Why now:** `append_transition` samples every transition triangle and clips/packs CPU output per frame. Moving/descent workloads exercise this path; static E1 has no morph/fallback draw and cannot measure its cost.
- **Affected:** `prepare.rs::SurfaceStaging::append_transition`, `transition.rs::SurfaceTransition`, `planet_terrain/adaptive.rs::AdaptiveTerrainCover`, GPU fallback stream.
- **Measure:** active input/emitted triangles, interpolation/clipping/packing CPU time, bytes/frame, worker construction separately, precision/normal/seam hazards, GPU fallback cost and path coverage on transitions/cuts/near-plane crossings.
- **Sequence:** **baseline before; prototype coupled to/after a residency foundation**. GPU interpolation requires bounded resident endpoints and conservative CPU fallback; moving truth generation is a separate decision. Do not infer a speedup from shorter transition duration or changed admitted cover.

## 4. C04 — Growth stalls, persistent allocations and staging lifetime

- **Why now:** `upload` synchronously drains prior submissions on growth to preserve the 80 MiB GPU cap. E1's static last repeat has zero growth; it does not exclude cold/descent stalls. CPU accounting reaches 128 MiB with no descent headroom, while unique allocation/driver VRAM are unmeasured.
- **Affected:** `gpu.rs::PlanetSurfaceRenderer::upload`, `CpuUploadProfile`, CPU `TerrainPatchCache`, shared `GeneratedSurfacePatch` grids, outgoing staging and capture/query buffers.
- **Measure:** exact growth/wait event correlation, CPU wall waits versus GPU busy time, simultaneously live old/new/upload allocations, unique shared grids, RSS/driver VRAM where available. Distinguish application byte vectors, wgpu internal upload staging and capture readback; no explicit custom mapped upload ring/StagingBelt exists in the inspected path.
- **Sequence:** **before/during residency work**, because its resource/lifetime design must retain caps and real headroom. Do not replace `Queue::write_buffer` with a staging ring unless measured copy/allocation/wait cost justifies it. This is a Mundaris-derived audit, not a staging-buffer prescription from the transcript.

## 5. C05 — Full-content-viewport planetary-layer coverage

- **Why now:** `draw_shells`/`draw_atmosphere` issue fullscreen content triangles; miss/discard pixels and expensive cloud/scatter evaluation deserve an invocation census. E1 identifies cloud 0.12392 ms and atmosphere 0.18388 ms in one 1152×768 query repeat, enough to motivate a bounded audit but not a current optimization claim.
- **Affected:** `planetary.rs::PlanetaryRenderer::{draw_shells,draw_atmosphere}`, `planetary.wgsl::{fs_cloud,fs_atmosphere,fs_ocean}`, content viewport.
- **Measure:** invoked versus contributing pixels, projected body coverage, GPU distributions and resolution scaling with existing layer toggles; shell precision/depth/alpha correctness from orbit through 2 m and horizon views.
- **Sequence:** **small baseline alongside residency work; implementation experiments after it** unless new measurements show a dominant layer bottleneck. Do not reduce scattering quality/resolution merely to improve a timing.

## 6. C06 — Clear/load/store and target-consumer lifetime map

- **Why now:** current code already clears once and loads later; there is no established “unnecessary clear” bug. This audit prevents future accidental redundant work and unsafe discard decisions.
- **Affected:** `celestial.rs::CelestialRenderer::draw`, `depth`, `Renderer::render_frame`, resize and capture target ownership.
- **Measure:** per-resource producer/consumer/coverage, backend clear/store traffic, pass overhead and resize/empty-scene/space/scissor output. Existing main query includes the clear but cannot isolate its cost; use external capture or separately scoped approved instrumentation.
- **Sequence:** **read-only map before/alongside residency; any attachment-policy change after evidence**. Initial color/depth clear and depth stores feeding atmosphere remain required in the current pipeline.

## 7. C07 — How much completed depth actually saves layer work

- **Why now:** atmosphere reads depth to truncate rays; cloud/ocean shader depth is calculated per fragment. `GreaterEqual` alone does not prove early rejection of expensive material/scattering work.
- **Affected:** `planetary.wgsl::fs_atmosphere`, `ColorDepth`, shell pipelines, shared celestial depth.
- **Measure:** hidden/contributing layer pixel ratio, early/late depth behavior with fragment-depth/discard, no-atmosphere control, horizon/opaque discontinuities and shader cost. Potential bounds/masks require conservative coverage, not reference-sphere occlusion assumptions over displaced terrain.
- **Sequence:** **baseline with C05; depth-rejection prototypes after residency**. This audit does not authorize Hi-Z or a depth-only prepass.

## 8. C08 — Draw/pass organization and overlap versus quad cost

- **Why now:** nine instanced terrain draws in E1 are already batched by stitch variant. There is no evidence that indirect draw machinery or more passes would improve it. Source research highlights measuring overlap/helper quads separately.
- **Affected:** `gpu.rs::PlanetSurfaceRenderer::draw`, `prepare.rs` stable bucket grouping, `CelestialRenderer::draw` pass sequence and complete bind-state ownership.
- **Measure:** CPU grouping/encode cost, triangle projected-size distribution, fragment/helper-quad counters where available, total GPU cost of existing versus depth-ordered within-mask draws on identical covers; keep overlays/atmosphere ordering correct.
- **Sequence:** **after residency for experiments**, with cheap current counter baselines first. Only consider F02/prepass or indirect culling if measured residual costs justify them; fewer draws alone is not a win. Preserve the repaired complete-bind-state regression boundary.

## 9. C09 — Sample/instance/fallback payload consumers and precision

- **Why now:** sample48/instance64/fallback80 streams directly affect C01 transfers and C02 packing; shared indices already exist. Source advises vertex-size/workflow specialization, not arbitrary normal/position truncation.
- **Affected:** `gpu.rs` bind/stride layouts, `prepare.rs` packing, `planet_surface.wgsl::{Sample,Instance,vs_main,vs_clipped}`, capture/test layout contracts.
- **Measure:** real per-mode consumers, padding/alignment, GPU fetch/bandwidth, packing CPU, resident size and quantization/normal/color/classification error. Include Natural and every diagnostic, morph seams and large-body near-surface precision.
- **Sequence:** **layout census before residency design, packing/compression experiments afterward**. Do not combine format redesign with establishing basic residency; avoid invalidating comparisons through lost diagnostics or reduced detail.

## 10. C10 — Linear/sRGB compositing, local compression and UI boundaries

- **Why now:** terrain decode/encode, ocean local compression, alpha planetary blending and egui share an sRGB target. A precise map is needed before eventual HDR/tone mapping; no color regression or performance saving is asserted here.
- **Affected:** `Renderer::new_async/render_frame`, `planet_surface.wgsl::fs_main`, `planetary.wgsl` encode/blend outputs, capture and egui integration.
- **Measure:** numerical color/alpha patches and paired full-frame captures at current formats, layer toggles, dark/bright content and high-DPI UI; isolate GPU/display-transfer costs only when a proposed alternative exists.
- **Sequence:** **independent read-only audit alongside residency; HDR/AA/UI-target changes later and separately approved**. Keep the source's favored GT7 curve and gamma design as future candidates, not fixes for terrain convergence.

# Transcript uncertainty and limits to preserve

These are **suspected transcription/interpretation ambiguities**, not corrections to the source. Return to original audio/diagram before using them as implementation detail.

| Passage | Uncertainty / handling |
| --- | --- |
| 00:03:06 | “so it will hold control while scrolling” appears to describe diagram zoom input, not a pipeline step. |
| 00:20:23 | Broken phrasing around depth-writing-disabled overdraw/“cost is massive”; retain the stated comparison's depth-state limits rather than deriving a missing test method. |
| 00:24:51 | “three out of four failed quads” may mix pixels/helper lanes with quads; no exact utilization threshold inferred. |
| 00:30:34–00:31:06 | “10-bit normals” versus “drop in quality using 24-bit normals” is ambiguous. No repaired precision ordering invented. |
| 00:37:20–00:37:56 | “FIRST,” “32 displaced depth or 44-bit channel data,” remaining RGBA16 and separate MSAA depth/stencil repacking need the diagram/audio. No bit allocation or per-sample stencil implementation reconstructed. |
| 00:40:38–00:41:12 | “1080p plus 8% rule” is an undeclared external-analysis metric; not a shadow-density acceptance rule. |
| 00:57:25 | “RG11-B10” notation is retained in discussion as source shorthand; comparisons also name R11G11B10. Exact underlying formats need original captures. |
| 01:07:01 | “DeVries-Roselaw” appears to name a perceptual law, but its spelling/claimed application is not independently established here. |
| 01:07:34 | “Lanier smooth shoulder” likely includes a word-recognition ambiguity; do not invent a curve named Lanier or silently substitute a formula. |
| 01:26:03–01:26:33 | “UE5.8 TSR” version is not verified; quoted performance remains scoped to the source's unidentified build/settings. |
| 01:31:32 versus 01:35:58 | “Brian Karras” / “Brian Karis” is inconsistent in the transcript; author identity is not used as technical evidence. |
| 01:43:50 versus 01:51:59 | Tile sizes 120×68 and 120×168 may be different examples or a transcription error. Keep both scoped examples; verify before sizing resources. |
| Throughout | RGBA16/RG16 sometimes lack component-type detail; narrative throughput/“free”/“infinite history”/sample-equivalence phrasing is not an API specification or proof. |
| 02:00:58–02:01:25 | Explicitly uncertain outro recognition; bonus-slide technical content is unavailable in the transcript. |

The reference preserves controversial technical recommendations through attribution and preserves their test gaps. It neither removes them from research consideration nor promotes them to measured Mundaris facts. No new runtime, GPU, visual, backend or performance validation was performed for this documentation extraction.
