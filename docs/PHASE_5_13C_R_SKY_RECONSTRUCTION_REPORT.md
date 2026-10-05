# Phase 5.13C-R — cinematic sky reconstruction

**Follow-up:** the user requested less concentration on one band. The current
version-4 [composition checkpoint](PHASE_5_13C_R_SKY_COMPOSITION_REPORT.md) supersedes
this version-3 first look; this report and its evidence remain historical.

## Goal

Reconstruct the rejected blurry/repetitive decorative sky, not just its brightness
or noise. Preserve app-owned immutable seeded content, disposable renderer caches,
finite stars at 1e18–2e19 m and the 1e14 m observer envelope. User visual approval is
the artistic acceptance gate. No addressable universe or volumetric nebulae.

The four supplied images are the visual inputs: **1** rejected Mundaris baseline;
**2** Space Engineers 2 sculpted/branching dust and illuminated cloud boundaries;
**3** Space Engine fine stars, varied brightness and clusters; **4** Space Engine
isolated coloured nebulae and meaningful empty sky. This corrects the brief's
image-2/image-4 attribution without changing its visual objective. References are
not imported rendering assets; retain these numbered images with future discussion.

## Result

**PARTIAL — offscreen first-look candidate ready; native first-look gates BLOCKED.**

- IMPLEMENTED: version 3, four distinct connected cloud/dust graphs with cavities,
  local emission/reflection colours, finer finite stars and pixel-integrated cores.
- VERIFIED: 156 before/after PNG/JSON fixtures have identical camera, FOV,
  resolution and appearance controls. Return and initial-motion PNGs reproduce
  the toward image exactly; the exercised sequence retains one static upload.
- VERIFIED: focused locked debug/release checks, explicit sky GPU regressions,
  day/night GPU comparison, affected-crate Clippy, formatting and release app build.
- OBSERVED: full-size images, unscaled detail crops, cached textures, field samples
  and selected motion frames inspected. This is not artistic or motion acceptance.
- BLOCKED: native baseline could not acquire foreground in three attempts; no
  screenshot was accepted. The display is 5120×1440, so full native 3840×2160
  capture is also unavailable. Offscreen images are explicitly not native evidence.
- UNTESTED: user approval, complete current-sky native route, warm p95 performance,
  final full quality matrix and actual cold first-visible-frame latency.

Stop at user review. This phase does not close terrain, camera, UI or universe gates.

## Architecture changes

`sky_definition.rs` authors the immutable composition: broken arch, branching
pillar, split fan and reflection crescent, with tapered connected branches,
recursive absorbing forks, dense overlaps/knots and irregular cavities. The
renderer evaluates these structures in fixed directional tangent charts; noise
perturbs their interiors/edges instead of defining their connectivity. Local
illumination and separate emission/reflection channels supply colour hierarchy.
The legacy smooth recipe remains for definitions without a morphology graph,
including old focused fixtures; the default version-3 preset does not use it.

The renderer retains one 2048×1024 diffuse full-sphere background plus **four
1024×1024 focal charts**, each spanning approximately 27.5°. All have linear-light
mips and ordinary linear/trilinear filtering. Focal chart quotient-rule angular
gradients and feathered replacement avoid screen-locked detail and mip-0 forcing.
Longitude wrap filtering remains, and focal charts need no equirectangular seam.

CALCULATED from actual perspective focal length at 60° vertical FOV:

| Source texel footprint at chart/equatorial centre | 1440p | 4K |
| --- | ---: | ---: |
| Rejected 4096×2048 sphere | 1.913 physical px | 2.869 physical px |
| Version-3 1024 focal chart | 0.596 physical px | 0.894 physical px |

These correct the earlier rough FOV/height estimates of 2.1/3.2 px. The exact
centre calculation uses `height/(2*tan(FOV/2))`; this is a footprint calculation,
not a visual acceptance score. The diffuse base deliberately has lower resolution;
focal structures carry the cached fine detail. See `angular-footprints.json`.

The catalogue increases **48,000 → 131,072** stars, at the explicit admission cap
of 131,072. Foreground stars (1e18–3e18 m, approximately 24%) are unextinguished;
background populations (6e18–2e19 m) receive static morphology-dependent extinction.
Cluster widths/colour populations vary regionally. Cores use 0.35–0.63 px sigma
and Gaussian physical-pixel aperture integration; halos only apply above flux 3.
There is no clock/noise animation in the sky shader.

Draw accounting: one background draw and one six-vertex instanced star draw,
**786,432 requested star vertices** versus 288,000 previously. Off-view rejection
still happens in the vertex shader, not CPU catalogue culling. Packed star payload
is exactly **4,194,304 B**, versus 1,536,000 B previously (old allocated capacity
2,097,152 B). This is explicit increased draw work, not a measured GPU-cost win.

The existing Arc-identity residency and 112-byte frame uniform remain. Chart axes
are a separate static 208-byte uniform. Camera/ordinary playback changes do not
generate/upload catalogue, base or detail content. World/simulation, terrain,
navigation, atmosphere/HDR policy and dependencies are unchanged.

## Files changed

Relative to the retained dirty baseline, nine existing source files changed:

- `crates/app/src/sky_definition.rs`: versioned morphology and finite populations.
- `crates/renderer/src/sky.rs`, `sky_background.rs`, `sky_gpu.rs`,
  `shaders/sky.wgsl`: bounded inputs, generation, focal residency/filtering and cores.
- `crates/app/src/sky_capture.rs`, `crates/app/examples/sky_capture.rs`: requested
  dimensions, isolated layers, seam/pole/focal views and bounded motion sequence.
- `crates/app/src/developer_snapshot.rs`: optional backwards-compatible schema-4
  sky sizes/chart counts and separately labelled transient payload bound.
- `crates/renderer/tests/sky_capture_513c.rs`: cached-mip GPU seam/pole regression.

New sky-only supporting files: renderer `sky_structure.rs`; app examples
`sky_field.rs` and `sky_native_capture.rs`; phase-local runners, viewer and evidence.
All **193 other files** from the 202-file starting source manifest remain identical.
No unrelated dirty work was reverted, and no commit/push was performed.

## Tests

Windows / RX 9070 XT / Vulkan. Exact commands, results and logs are in
`docs/evidence/phase513cr/focused-20261005-02/commands.json`.

- Ten renderer sky unit tests and five app preset tests pass in debug and release.
- Existing adapter-required centroid/cache/toggle/depth/invalid-state test passes:
  maximum centroid error **0.002139788 physical px**, 960×640/60°, observer
  origin/1 AU/9e13 m. Not arbitrary-catalogue GPU precision acceptance.
- New adapter-required cached-mip seam/pole/-Z test passes: maximum linear channel
  discrepancy **0.001413084**, 257×257/60°, 256-pixel synthetic focal chart.
- Day/night adapter-required test passes: day on/off maximum **1/255**, night
  **240/255**, with identical foreground state and atmosphere actually drawn.
- Affected app/renderer all-target/all-feature Clippy, workspace formatting and
  locked release all-feature app build pass. Both fast checks pass; final paired
  Earth PNG/JSON was inspected and remains terrain-quality-pending/unsettled.

The first new GPU regression failed because its oracle compared an unfiltered
field point with a minified cached texture. It now independently reproduces
bilinear/trilinear cached linear mip sampling at the fixture footprint; the
0.012 tolerance was not relaxed and production rendering was not changed to pass.
The failure and corrected result are both retained.

## Measurements

Single cold first-look captures, not profiles or first-visible-frame measurements:

| 2560×1440 fixture | Before, version 2 | After, version 3 |
| --- | ---: | ---: |
| Renderer generation | 1047.5287 ms | 2300.9269 ms |
| Upload API work | 8.3217 ms | 7.9360 ms |
| Owned GPU payload | 46,836,508 B / 44.67 MiB | 37,749,052 B / 36.00 MiB |
| CPU definition capacity | 2,304,152 B | 6,305,880 B |

The 4K captures report generation 1038.8100 → 2279.8169 ms and upload API
6.5813 → 5.9283 ms. Some capture/build/native-attempt work overlapped; these are
identified observations, not controlled latency comparisons or speedup claims.
**Cold generation regresses by roughly 2.2×**, despite reduced resident GPU payload.

Version 3 records a conservative transient generation-payload bound of
**53,477,372 B / 51.00 MiB**, separately from definition/resident GPU bytes. This
is a calculated bound on owned generation allocations, not measured peak RSS,
allocator overhead, driver staging or VRAM. Actual transient process peak is open.

GPU readback/submission latency and actual first-visible-frame latency are not
measured separately here. CPU ≤0.1 ms p95 / GPU ≤0.5 ms p95 remain unchanged,
UNTESTED targets: no ≥30 warmups / ≥200 retained on/off samples were run. After
visual approval, profile cold work before choosing remedies such as precomputed
chart axes and bounded parallel generation; no new cold-start target is invented.

## Captures

Open [`review.html`](evidence/phase513cr/review.html) locally for unscaled paired
stills and the 30 FPS raw-image motion preview. Authority remains the original PNGs
and canonical JSON, not viewer playback performance.

- `before-matched-1440p/`, `after-1440p/`, `before-matched-4k/`, `after-4k/`:
  18 stills + 60 motion frames per directory, 60° vertical FOV, identical controls.
- Toward/along/away, nebula focus, separate stars/background, seam/pole, viewport,
  return, Earth day/night on/off, opaque silhouettes and airless Moon examples.
- `crops/`: identical 768×512 nebula and 512×512 star rectangles, no resizing,
  sharpening or exposure adjustment. `before-field/` and `after-field/`: actual
  base/focal pixels and 40,401 matching-coordinate pre-quantization field samples.
- Pure sky/atmosphere/silhouette fixtures intentionally admit no terrain/ocean/clouds.
  They are not native UX or settled terrain evidence. The final fast production
  Earth capture is still `quality_pending=true`, `settled=false`.

## Known failures

- Native baseline attempts 01–03 returned no accepted PNG: foreground request was
  rejected. `native-before-1440p-03/failure.json` retains native/foreground handles.
  Current-sky native comparison is UNTESTED behind that blocker. Operator-assisted
  foreground validation is needed; no engine interaction defect is established.
- Full native 4K is BLOCKED by the 5120×1440 display; the 4K PNGs are offscreen.
- One capture build overlapped a capture-helper edit and failed; the settled helper
  subsequently compiled. Baseline builds sharing the Cargo target caused stale
  renderer artifacts on later app builds; focused renderer release-artifact cleanup
  and the subsequent current-source build succeeded. Use separate baseline targets.
- Crop runner initially failed on assembly-reference/PowerShell expression issues;
  corrected runs succeeded. No source PNG was altered or overwritten.
- Artistic similarity, native interaction, long slow-motion stability, full GPU
  precision matrix, cold first-visible latency, raw warm profiles and final quality
  matrix remain open. Linux/remote CI remain UNTESTED.

## Evidence

See [evidence index](evidence/phase513cr/README.md), starting fingerprints and copied
baseline source, checkpoint fingerprints/matched-settings/footprints, raw failures,
focused logs and fast-check snapshots. Captures precede a diagnostic-only mip
inspection API addition and regression-oracle correction; their rendering recipe,
shader, cached generation and appearance inputs remain unchanged.

## Git state

Uncommitted on `main`, HEAD `d81bb2a17ad4f8c1d06eaa1619b6281b946641c3`, with substantial
pre-existing dirty/untracked work. HEAD alone does not identify this implementation.
No commit, push, dependency update or unrelated cleanup is authorized/performed.

## Reviewer follow-up

Review morphology, star hierarchy, colour, native-size crops and raw motion with
the supplied references. Do not accept the artistic result from tests/statistics.
Resolve missing native first-look evidence with the user before treating that
checkpoint as complete. Only after approval/source freeze proceed to warm on/off
profiles, the original eight-checkpoint current-sky native route and
`scripts/validate.ps1 -IncludeGpu` plus separate explicit sky GPU tests.
