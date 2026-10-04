# Phase 5 planetary presentation and refinement continuation

## Outcome

**Partial. Acceptance A still fails.** Safe refinement pipelining and production
Natural/ocean/atmosphere/cloud rendering are implemented, with retained current
routes and captures. Cold source refinement advances faster, but descent has a
large CPU outlier and no operational memory headroom. Orbital presentation improves;
coastline, medium/close appearance and real-scale ocean artifacts prevent visual
acceptance. Neither Strong nor Substantial success is justified.

Baseline and current branch: `main`; HEAD:
`5d1fe77ced96229b3393c17e83965680ee6d0fbf`. Work remains uncommitted and unpushed.
Pre-existing design/workflow changes and every intermediate dataset are preserved.
The [evidence index](evidence/phase5-overnight-planetary/README.md) identifies current
results under `after/current-routes/` and `after/current-views/`, not `after/final-*`.

## Architecture truth and implemented changes

The mechanics reference was read in full before implementation. Source confirms
the latest retained [Phase 5.10B Acceptance A](phase-5-10b-acceptance-a.md) fails:
four native workers, adaptive mixed levels, relevant camera-radial priority rather
than primary screen-centre foveation, Grid16/289 samples, analytic displaced normals,
footprint filtering, and repeated observer-relative preparation/upload. Historical
architecture inventories saying terrain is deferred do not describe this path.

### Refinement

- One successor can construct during an active display morph. Its source is the
  active morph's **immutable complete stitched destination**, not its changing
  visible fraction. The complete result waits in one bounded successor slot.
- The successor displays only after its captured endpoint becomes the published
  source. Exactly one overlay displays at a time. Balanced sibling readiness,
  complete coverage, deterministic endpoint geometry and exact transitions remain.
- Captured display durations remain 150 ms; no hierarchy jumps, transition
  coalescing, arbitrary intermediate meshes or shortened morphs manufacture a pass.
- Obsolete unshown successors are cancelled/discarded and private selection restored
  to the immutable endpoint. Worker reservations retain shared source charges through
  cancellation acknowledgement, publication and session abandonment.
- Available completions publish in stable job-ID order **among available results**;
  an unfinished earlier job no longer blocks every finished slot. Wall-time polling
  can affect availability, but does not change authoritative samples/endpoints.
- Retry allowances of 16/24/32 MiB are ceilings. Full source, cache, renderer staging,
  selector and construction workspace charges determine a smaller actual overlay
  envelope when necessary, never below the initial 16 MiB in ordinary operation.
  Exact construction still rejects excess allocation. The cap is unchanged.

The rejected metadata redistribution experiment is absent from the final code:
the existing `32 / sessions.len().max(1)` allowance remains. Intermediate routes
exposed a warm-descent source L17 stall. Restoring metadata alone did not fix it;
admission tracing isolated an idle retry unable to reserve the entire 24 MiB ceiling.
Current descent at 100 m/350.033 ms admits 23,576,270 bytes against the 25,165,824-byte
ceiling. This restores progression without raising the cap or reducing accounting.

### Planetary presentation

- **Natural** is mode/index 8 and the normal Solar default. Earth/Rock/Mars material
  profiles use height, slope and body-fixed direction with quintic value noise and
  finest-band derivative filtering. No shader terrain displacement is introduced.
  Octahedral coordinates decode before interpolation; clipped coordinates reconstruct
  in f64 body space. Existing 48-byte sample/80-byte fallback strides remain unchanged.
- **Ocean:** a renderer-owned analytic radial surface at `R + sea_datum_m`, with
  reverse-Z depth writes, Fresnel, roughness, solar specular and a bounded sky-reflection
  approximation. It is actual ray/surface rendering, not blue terrain classification.
  Above-datum terrain depth-occludes it. It is opaque visual water, not fluid simulation
  or transmissive underwater optics.
- **Atmosphere:** body-relative ray integration, 12 view samples × 4 solar samples,
  Rayleigh/Mie-inspired scattering, density falloff and planetary shadow, stopped by
  completed scene depth. It is not a screen vignette or multiple-scattering solver.
- **Clouds:** four fixed body-coordinate noise bands, soft opacity, coherent sunlight,
  shell altitude, depth testing without depth writes. Coverage default is 0.32 after
  excessive-white-coverage captures. There is no drift, shadow map or cloud texture.
- **Lighting:** the same body-fixed surface-to-Sun direction feeds all layers;
  diagnostics preserve their modes and suppress the three presentation layers.
  Ocean/Clouds/Atmosphere checkboxes change rendering only. sRGB native and capture
  targets permit linear alpha compositing.

Gameplay Earth: radius 400,000 m, datum 350 m, cloud altitude 4,800 m, atmosphere
height 10,000 m. Real Earth uses its supplied radius, cloud altitude capped at 12,000 m
and atmosphere height capped at 100,000 m. Moon/Mercury/Venus use Rock with no Earth
layers; Mars uses red/brown land with no Earth ocean/cloud/atmosphere by default.

## Measured responsiveness and performance

Windows 10.0.26200, Rust 1.98.1; sequential release probes with
`surface-profile,terrain-capture`, four workers, 150 ms morphs, 768×512/60°.
CPU opportunities are not native input-to-present latency, FPS or isolated GPU time.
These are single runs, not statistically repeated causal benchmarks.
An existing native app was left running rather than terminated; background app/OS
load was not eliminated. This limits attribution, not the recorded latency failure.

### Cold tangent 2 m complete-terrain clearance

| Requested time | Before source | Current actual time | Current source / ready / desired |
| ---: | ---: | ---: | --- |
| 1 s | L4 | 1,013 ms | L5 / L7 / L23 |
| 2 s | L7 | 2,008 ms | L8 / L10 / L23 |
| 5 s | L16 | 5,007 ms | L18 / L19 / L23 |

Every checkpoint remains quality-pending/unsettled. L23 is not reached in the
five-second cold window, so there is no measured accepted time-to-useful-quality.
Warm seven-view descent is not cold convergence: source L18 at 100 m, L20 at 10 m,
L22 at 2 m at each 1/2/5-second checkpoint. Desired is L18/L23/L23 respectively;
radial agreement at 100 m does not establish central-visible or whole-view settlement.

### App-thread CPU distributions

Values are median / P95 / worst in milliseconds. Update+prepare is computed per
frame before quantiles; independent quantiles are not added.

| Route | Update | Prepare | Update + prepare | Transition append |
| --- | --- | --- | --- | --- |
| Before cold | 1.181 / 3.651 / 5.666 | 3.821 / 9.668 / 13.160 | 5.438 / 11.911 / 15.129 | 1.958 / 8.568 / 10.388 |
| Current cold | 1.863 / 5.705 / 8.714 | 5.167 / 12.436 / 16.409 | 7.901 / 14.263 / 19.179 | 3.557 / 9.466 / 11.434 |
| Before descent | 2.927 / 5.693 / 9.975 | 7.082 / 12.230 / 19.305 | 9.846 / 16.073 / 22.992 | 1.111 / 7.267 / 16.664 |
| Current descent | 3.470 / 5.700 / 88.816 | 8.056 / 12.870 / 85.925 | 11.201 / 16.883 / 114.533 | 1.219 / 7.478 / 50.905 |
| Current motion | 1.893 / 4.451 / 6.245 | 3.869 / 12.173 / 15.816 | 6.265 / 14.557 / 17.673 | 1.903 / 10.613 / 12.160 |
| Current switch | 0.322 / 1.012 / 3.574 | 1.124 / 5.656 / 6.160 | 2.438 / 6.349 / 7.036 | 0.975 / 5.529 / 6.000 |

The descent outlier is a **regression**, not dismissed as noise: at 10 km/4,349.477 ms,
update is 88.8164 ms (selection 87.4671 ms), prepare 25.7167 ms. At 10 km/4,591.093 ms,
prepare reaches 85.9245 ms; transition append peaks at 50.9045 ms at 4,475.400 ms.
No repeated investigation yet separates scheduling effects from algorithmic causes.
Existing full-cover selection/conversion/packing/upload remains on the app thread.

### Worker work and morph occupancy

| Route | Raw patches / samples completed | Completed worker CPU ms | Sampled morph / construction / overlap / ready-successor wait ms |
| --- | ---: | ---: | --- |
| Before cold | 437 / 126,293 | 2,964.258 | 2,623.688 / 1,410.037 / 0 / 0 |
| Current cold | 691 / 199,699 | 5,616.009 | 2,827.212 / 1,745.630 / 87.300 / 0 |
| Before descent | 960 / 277,440 | 8,459.902 | 24,347.532 / 6,275.526 / 0 / 0 |
| Current descent | 1,294 / 373,966 | 10,439.882 | 26,959.199 / 7,053.906 / 2,467.284 / 7,529.142 |

Cold raw completion rises from about 87 to 138 patches/s. More work/progress does
not prove a faster generator; world generation code is unchanged. Current motion/
switch complete 450/222 patches. Cold/descent/motion/switch cancellations are
171/422/97/13; completions classified current-local-useful are 169/238/92/48.
Those categories exclude prefetch and eventual/nonlocal usefulness, so the complement
must not be called wasted work. Raw reservation-rejection and transition-deferred
frame counts are zero on current routes, not proof that all admission always succeeds.

Occupancy integrates the preceding sampled state within each view. These overlapping
durations are **not additive CPU costs or an exact critical-path decomposition**.
Before, morph and construction explicitly exclude one another. Now construction can
overlap; source display still advances along serial captured 150 ms endpoints.
Memory can serialize admission. Twenty-three one-level display morphs alone imply
3.45 seconds if that chain is required, before generation and frame opportunities.

Observed unique active-transition profiles give the following **sum of six measured
construction-stage elapsed timers** (validation, pair mapping, clipping, endpoint
capture, boundary canonicalization and emission), median / P95 / worst ms:

| Route | Unique logged profiles | Stage sum |
| --- | ---: | --- |
| Before cold | 17 | 51.251 / 140.160 / 140.160 |
| Current cold | 19 | 49.113 / 191.412 / 191.412 |
| Before descent | 154 | 13.669 / 74.541 / 195.257 |
| Current descent | 161 | 13.145 / 71.169 / 200.381 |

Large overlays can therefore exceed the 150 ms morph duration even with pipelining.
These are not same-endpoint microbenchmarks or all-job distributions: the probe
logs the active mesh when a completion is observed, including an older active mesh
when a successor queues. The summarizer deduplicates exact profile records; it does
not infer job IDs, include unpublished/cancelled construction, or include stitching
and uninstrumented overhead in the six-stage sum. The raw logs and last-completion
stitch/morph frame fields remain available; persistent last timers are never summed
on every polling frame.

### Terrain accounting and uploads

| Route | Peak bytes before | Peak bytes current | Current median / P95 / worst upload bytes |
| --- | ---: | ---: | --- |
| Cold | 127,124,474 | 134,217,046 | 1,532,960 / 3,489,712 / 4,069,952 |
| Descent | 134,217,717 | 134,217,728 | 3,985,696 / 5,583,760 / 8,563,360 |
| Motion | not newly baselined | 134,217,374 | 1,100,048 / 5,542,288 / 7,369,056 |
| Switch | not newly baselined | 126,609,039 | 145,232 / 624,768 / 1,148,528 |

The hard cap is still **134,217,728 bytes (128 MiB)**. Current descent reaches it
exactly: no operational headroom. Raw completions increase, but sampled raw plus
bookkeeping class maxima **decrease**: cold 7,868,996→7,238,636 B, descent
12,435,604→10,180,316 B. Cold pinned raw grows 5,020,016→5,509,072 B; descent pinned
raw decreases 8,731,088→8,572,864 B. Increased aggregate peaks therefore must not be
described as increased total raw residency. Lower raw peaks do not recover headroom.
Accounting is capacities/reservations, not process RSS or GPU memory.

Current descent class maxima: raw/bookkeeping 10,180,316 B; pinned raw subset
8,572,864 B; worker reservations 38,104,748 B; worker fixed 2,621,440 B; sampled
completed-unpublished 0 B; stitched source 8,102,003 B; morph mesh 21,357,200 B;
selector scratch 173,200 B; actual outgoing renderer capacity 13,422,717 B;
boundary/proof capacity 6,039,355 B; full staging allowance 75,530,240 B;
frame-end accounted 125,829,120 B. Categories overlap and peak at different times;
do not sum them. Zero sampled unpublished bytes does not prove no unpublished lifetime.

### Fixed visual resources and capture timing

Three pipelines and retained bind groups, one 256-byte uniform buffer with 176-byte
active-frame upload, and a sampled binding/usage on the existing viewport
`Depth32Float` texture. No extra depth copy, shell mesh, cloud texture or CPU cloud
generation. The existing 1152×768 depth allocation is nominally 3,538,944 B; sampling
does not add a second texture. Driver pipeline/bind-group memory is unmeasured.
Depth/binding recreates on viewport resize; pipelines/uniforms persist. At most
three extra fullscreen triangle draws for the one admitted Earth body, never per patch.

Eight unchanged readbacks per variant are pixel-identical. Independent saved-BMP
comparison (`summarize-captures.ps1`, `after/current-views/layer-comparison.csv`)
also establishes actual layer effects: gameplay orbit changes 86,855 pixels without
ocean, 104,463 without clouds and 280,556 without atmosphere out of 884,736 pixels.
Every Moon/Mars disabled-layer intervention is exactly identical to all-layers;
close 2 m no-clouds is also identical, so clouds contribute no visible pixels there
despite the admitted cloud draw. Pixel difference is not a quality/error metric.
RX 9070 XT/Vulkan,
1152×768 `Rgba8UnormSrgb`; GPU timestamps are unavailable. Below are all-layers /
land-only **median milliseconds**, with identical terrain cover and unchanged cache.

| Scene | Host upload/encode | Render + wait + readback |
| --- | ---: | ---: |
| Gameplay orbit | 0.6808 / 0.5742 | 2.0499 / 1.7827 |
| 10 km | 0.3571 / 0.3322 | 1.8486 / 1.3424 |
| 2 m | 0.2983 / 0.2905 | 1.4935 / 1.1376 |
| Terminator | 0.6626 / 0.5816 | 1.8620 / 1.6409 |
| Grazing | 0.6044 / 0.5992 | 1.7789 / 1.6797 |
| Real Earth orbit | 0.6584 / 0.6113 | 1.8388 / 1.6464 |
| Moon | 0.6311 / 0.6203 | 1.7220 / 1.7278 |
| Mars | 0.6668 / 0.5852 | 1.8203 / 1.6694 |

Orbit's all-vs-land difference is 0.1066 ms encode and 0.2672 ms render/wait/readback;
10 km's is 0.0249/0.5062 ms. These are observed intervention differences, **not
isolated GPU layer costs**. Moon/Mars have zero extra layer draws in both variants;
their nonzero timing differences demonstrate measurement variation, not layer cost.
Orbit Natural vs Readability matched prepare medians are 25.2529 vs 25.3506 ms;
there is no demonstrated material CPU improvement. Full regular preparation remains.

## Visually observed, inferred and blocked

Current images visibly add water-like reflectance, cloud patterns, blue atmospheric
limb and coherent day/night shading without LOD hues in Natural. Orbit is more
planet-like than the baseline diagnostic sphere, but the no-clouds view still has
a huge nearly round smooth land mass. Terminator exposes coarse/angular coast shape.
The 10 km view is washed out by cloud/haze; 2 m remains a sparse green plane/horizon.
Moon is gray and Mars red/brown with no Earth layers, but neither is accepted detailed
morphology. No claim of photorealism, human navigation acceptance or temporal
anti-shimmer acceptance is made.

All eight GPU fixtures are `ready=true`, **quality-pending and unsettled**. They use
serial operation budgets (800 updates, 1,156 vertices/update) and zero-duration
transitions, not the four-worker timed acceptance route. Orbit/terminator/grazing
whole covers remain L3–14, despite radial source=desired L14. Medium has source radial
L15 but visible L0–15; close radial L22 versus desired L23 and visible L1–21, with
infinite maximum error. Whole-view quality cannot be inferred from radial agreement.

Real-scale Earth executes and renders through the same radius/configuration path,
but coast pixels visibly stipple. f64-factored shell constants and stable quadratic
roots improve near-shell clearances; f32 ray/depth fragment precision remains a
plausible, **unconfirmed** cause of stippling. Terrain narrowing tests do not certify
analytic shell fragment precision. Smooth coast intersection/real-scale visual
acceptance is therefore **not passed**. Static image repeatability does not prove
camera-motion stability; body-coordinate reconstruction tests cover coordinate
stability, not all temporal rendering artifacts.

**Terrain morphology is unchanged.** No generator operator, seed, V2 definition,
amplitudes, terrain controls, erosion, filtering, terrain revision/identity, analytic
derivatives, bounds, profile differences or certificate thresholds were tuned.
Morphology tuning remains blocked because neither Acceptance A nor a trustworthy
fully settled whole-view fixture passed. Render material noise is not new terrain.

## Validation

Focused current renderer/app regressions, headroom, worker and real-transition checks
passed during implementation. New tests exercise Natural/configuration/shader validity,
body profiles, shell constants, clipped body-coordinate reconstruction, camera-coordinate
stability, serial/worker successor overlap, reversal and reservation lifetime.
The existing overlay retry regression is extended to reject sub-16-MiB headroom and
admit a smaller-than-24-MiB ordinary retry envelope while retaining accounting.

The final quality matrix in `validation/quality-results.json` is **all exit code 0**:

| Check | Current result |
| --- | --- |
| Formatting | pass |
| Locked all-target/all-feature workspace check | pass |
| Locked all-target/all-feature workspace Clippy, warnings denied | pass |
| All-feature workspace debug tests | 245 passed, 0 failed, 3 ignored |
| All-feature workspace release tests | 245 passed, 0 failed, 3 ignored |
| Warnings-denied workspace Rustdoc | pass |
| Selected ignored long-orbit tests, release | 2 passed |
| Selected ignored native close-surface readback, release | 1 passed |
| Focused app/renderer default-feature all-target Clippy | pass |
| Final planetary configuration tests, debug and release | 3 passed each |
| Documentation UTF-8/whitespace/local links/30-answer ordering | pass |
| `git diff --check` | pass |

`first-quality-gates/` retains the original item-ordering Clippy failure and the
extended retry fixture's erroneous immediate-promotion assertion. Its correction
explicitly preserves the captured 150 ms morph and then supplies elapsed time to
finish it. `second-quality-gates/` retains a full debug/release pass but the next
Clippy failure in test configuration initialization. That test now uses struct
initializers with the same invalid values/assertions. A focused rerun of format,
workspace check, all-feature Clippy and Rustdoc updates the final matrix; the affected
configuration tests also rerun in both profiles. Default-feature Clippy passes.
This is a completed matrix with retained failures and affected-gate reruns, not a
claim that the first attempt passed. `quality-merge-first.json` retains a Windows
PowerShell array-merging bookkeeping error; its nested stale failure entry was fixed
and the gate summary regenerated from the retained full run and successful retries.
The existing running app locks `target/release/mundaris_app.exe`; validation uses
`target/planetary-validation` without terminating the user's process.
Linux, remote CI, native human controls/input-to-present, GPU timestamps and external
RSS/GPU peaks remain unmeasured. No unsafe Rust or simulation algorithm change is made.

## Evidence and reproduction

Root: `docs/evidence/phase5-overnight-planetary/`.

- `baseline.txt`, `before/cold-4/`, `before/descent-4/`, `before/views/`: current-code
  baseline before this pass, not substituted historical results.
- `after/current-routes/{cold-4,descent-4,motion-4,switch-4}/`: per-frame CPU,
  requested/actual LOD checkpoints, memory, worker and morph/successor detail,
  repository manifests, `transition-profiles.log`, `render-profiles.txt`.
- `after/current-views/*-manifest.txt`, `*.log`, `*.bmp`, `*.png`: eight scenes,
  Natural and all diagnostic modes, all/no-atmosphere/no-ocean/no-cloud/land-only
  interventions and exact-camera no-cull controls. PNGs are lossless BMP conversions.
- `summary.json` and `summarize.ps1`: reproducible route quantiles/occupancies.
- `summarize-captures.ps1` and `after/current-views/layer-comparison.csv`: independent
  saved-image pixel differences and manifest layer timings.
- `validation/` and `validate.ps1`: exact validation commands and UTF-8 transcripts.
- `delivery-source-sha256.csv`, `repository-final.txt`: final source inventory and
  Git state/stat, distinct from the baseline/capture-time manifests. Post-capture
  changes are regression assertions, test organization/initializers, comments and
  whitespace, not changes to the measured production algorithm.

Current capture label: `current-bounded-successor-envelope-natural-cloud032`.
Each scene retains body/terrain identity, f64 camera pose, radius/FOV/viewport,
source/desired/readiness, configuration, Sun, backend, timing, draw and memory state.
All eight matched-mode cache miss/eviction deltas are zero. Baseline UNORM vs current
sRGB targets differ, so across-revision pixel equality is not claimed.
See the evidence index for commands and retained failed/intermediate experiments.

## Explicit answers to the 30 final evidence questions

1. **Does current Acceptance A pass?** No. Cold L5/L8/L18 at 1/2/5 seconds versus
   desired L23, every checkpoint unsettled; descent reaches the cap and has 114.533 ms
   worst update+prepare. Current CSVs above are the evidence, not historical passes.
2. **Dominant refinement latency before?** Serial construction → 150 ms morph → next
   topology admission. Cold sampled morph 2,623.688 ms and construction 1,410.037 ms,
   zero overlap. These state durations do not prove additive causal percentages.
3. **Dominant refinement latency after?** Serial display endpoint chain remains;
   generation, complete-cover construction and memory admission still contribute.
   Cold morph 2,827.212 ms, construction 1,745.630 ms, only 87.300 ms overlap;
   descent ready-successor wait 7,529.142 ms over seven views. Exact causal ranking
   beyond the explicit display gate is not separately measured; observed overlay
   stage sums reach 191.412 ms cold/200.381 ms descent, also exceeding a 150 ms morph.
4. **Does active morph still serialize deeper refinement?** It no longer excludes
   construction of one successor, but it still serializes display promotion; one
   pending/queued successor bounds how far topology can run ahead.
5. **Close-surface time to useful LOD?** No accepted cold time measured: source L18
   at 5,007 ms versus L23. Warm 2 m has L22 by 1,010 ms after prior descent views,
   not a cold useful-quality pass.
6. **Raw throughput changed?** Observed cold 437→691 patches/about 5 s (~87→138/s);
   descent 960→1,294. More completed work, not an independently faster generator.
7. **Terrain memory peak changed?** Cold 127,124,474→134,217,046 B; descent
   134,217,717→134,217,728 B. Headroom did not improve.
8. **Cap unchanged?** Yes, 128 MiB/134,217,728 B.
9. **Per-frame CPU costs changed?** Cold total median/P95/worst
   5.438/11.911/15.129→7.901/14.263/19.179 ms; descent
   9.846/16.073/22.992→11.201/16.883/114.533 ms. More progress with worse CPU
   distributions; stage and upload tables above expose the regression.
10. **New GPU/renderer resources?** Three analytic fullscreen pipelines, retained
    bindings, one uniform buffer; existing depth becomes sampleable. No shell mesh/texture.
11. **Memory cost?** 256 B GPU uniform/176 B frame upload; existing Depth32Float
    3,538,944 B at capture viewport, no added depth allocation. Driver object overhead
    is unmeasured; sample/fallback strides and terrain cap are unchanged.
12. **Extra draws?** Up to three for one Earth surface owner; zero in diagnostics,
    disabled layers, Moon and Mars.
13. **Layer settings regenerate terrain?** No. Renderer-only configuration; matched
    captures preserve cover/cache state, with zero miss/eviction deltas in eight scenes.
14. **Sunlight regenerates terrain?** No. Lighting is not raw identity; changing Sun
    changes uniforms, not terrain definition/revision or generator samples.
15. **Actual ocean or palette?** Actual analytic radial water intersection/material
    with reverse-Z depth, not SeaMask/Readability blue. Real-scale coast stability fails.
16. **Atmosphere layer or vignette?** Actual renderer ray-integrated spherical density,
    solar optical path and depth termination. Approximate single scattering, not a vignette.
17. **Clouds stable in body coordinates?** Yes by construction; deterministic four-band
    body-coordinate noise and repeated pixel equality. Arbitrary-motion shimmer is unmeasured.
18. **Natural and diagnostics coexist?** Yes: Elevation, Normals, Diffuse, Lit,
    Readability, Slope, SeaMask, RockWeight, LOD and borders remain; diagnostics bypass layers.
19. **Earth orbit reads as a planet?** More so: limb/clouds/water/terminator visibly
    improve presentation. Full requested acceptance is not met: giant smooth continent,
    coarse coast and real-scale stippling remain.
20. **Underlying morphology changed?** No; gate remained blocked.
21. **Exact generator/config mechanism changed?** None. Only renderer material noise,
    layer configuration and scheduling changed.
22. **Terrain version/cache identity changes required?** None; V2 and revision/identity
    semantics remain. Environment/Sun/camera are not added to raw identity.
23. **Derivatives/bounds/certificates updated?** No need: geometry generator unchanged.
    Existing conservative bounds, footprint filtering and 0.125/0.0625 px thresholds remain.
24. **Cross-face/mixed-LOD seams correct?** Existing exact stitch/transition architecture
    is retained and exercised by regressions. No new terrain crack identified in inspected
    images; pending coarse coastline/shell artifacts are not a seam acceptance certificate.
25. **Same code survives real scale?** Execution and captures succeed, with supplied
    radius/altitudes and shell-constant tests. Ocean coast precision remains visibly defective;
    full real-scale visual acceptance is not claimed.
26. **Moon/Mars incorrectly get Earth oceans/clouds?** No: their configs and manifests
    show zero ocean/cloud/atmosphere draws, with gray Rock/red-brown Mars profiles.
27. **Visual deficiencies?** Giant smooth land mass, coarse angular/stippled coast,
    washed-out 10 km view, sparse close geometry, untuned non-Earth detail, no cloud
    shadows or advanced scattering, unmeasured temporal shimmer.
28. **Performance deficiencies?** Failed cold convergence, serial display chain,
    full-cover preparation/restaging/upload, 114.533 ms descent outlier, exact-cap peak,
    more completed raw work, no native latency or isolated GPU timings.
29. **Blocked acceptance items?** Existing responsiveness/headroom; trustworthy
    central-visible/whole-view quality; morphology tuning; smooth coast/real-scale
    ocean; convincing medium/close rendering; native interactive and Linux/remote CI evidence.
30. **Next phase?** Recover Acceptance A and operational headroom with repeated stage
    timing, then certify shell ray/depth precision and settled whole-view fixtures.
    Only then tune continental morphology/versioned operators and polish atmosphere/clouds.
