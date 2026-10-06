# Slice 2A resident terrain tile checkpoint

Date: 2026-10-06. Status: **PASS — ready for the authorized fixed Slice 2B proof**.

The opt-in prototype renders one derived tile from the published world surface
through a persistent GPU storage buffer and reusable indexed UV grid. The normal
production renderer remains available. This is a representation/residency proof;
it does not accept terrain art, whole-body quality, native control feel, or a
planetary performance target.

## Source and authority

The working tree started dirty at `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`.
Evidence lives in `target/terrain-redesign/slice2a/`. Initial/final manifests and
preservation differences identify the actual files; HEAD alone does not identify
the executable. No commit or push was made.

IMPLEMENTED: `CelestialBody` publishes a complete immutable definition and radius;
the app constructs `SurfaceGenerator`, and `ResidentTileBuilder` consumes its
complete f64 queries. Renderer/shader code receives derived samples, never geology.
The exact key includes canonical configuration words, radius bits, body identity,
authority/material revisions, format/filter versions, address, and resolution.
Camera, lighting, debug state, slot number, and runtime frame IDs are absent.

VERIFIED: all 25 initial world/math Rust source hashes remain unchanged. The current
reference example replayed the retained family, province, and hierarchy corpora:
2,577 + 3,601 + 7,085 queries, with all parsed values and f64 bits equal. See
`family-replay-audit.json`, `province-replay-audit.json`, and
`hierarchy-replay-audit.json`. Wrapper wall-clock timing is excluded from equality.

## Representation and canonical fixture

The canonical region is RockyV5, seed 0, reference radius 80,000 m, definition/body
identity `5931033225171238913`, cube face +Z, level 9, address (157,39). It contains
the retained Slice 1B.2 direction
`[-0.281506901494687,-0.6194762511124416,0.7328049117729327]`; the tile center is a
distinct point. `canonical-region.json` records the derivation and source metadata.

Format/filter version 1 uses 67×67 samples including a one-sample halo, a 65×65
UV-only vertex grid, and 8,192 triangles. Each CPU texel contains f32 radial offset
and four normalized material weights (20 B); the portable storage-buffer layout
uses 32 B. Height and material deliberately share a content lifetime. Normals use
central secants of displaced geometry through the halo. See ADR 0012 for the
filter and precision decisions.

The deterministic seven-tap filter weights the center by 1/4 and six projected
global-axis offsets by 1/8 each. Angular width is one quarter of nominal grid
spacing. This is a versioned prototype filter, not a final low-pass policy. Actual
tap lengths vary with direction and chart position. The accepted canonical build
has center spacing 3.437×2.922 m, nominal spacing 4.888 m, and filter tap lengths
0.840–1.213 m.

## Reconstruction evidence

MEASURED in `gpu-candidate-5/cold.json`: world → finite derived representation has
texel radial maximum/RMS 0.317530/0.086224 m, rendered-triangle-centroid position
maximum/RMS 0.788504/0.176603 m, and normal angle maximum/RMS
0.905330/0.323645 rad. These quantify loss of fine structure through filtering and
finite spacing. They are separate from shader precision and are not an artistic
acceptance claim.

MEASURED: canonical GPU reconstruction checked 4,225 vertices. Maximum local
position error is 0.027414 mm; final view-position error 0.037136 mm; normal error
0.00000591749 rad; material-component error 0. All position, normal, and material
values are finite. The predeclared limits are 1 mm for local/final positions and
0.001 rad for normals, with separate normalized-material checks.

VERIFIED: all 54 actual draw/readback cases pass across radii 109,000, 6,371,000,
and 70,000,000 m, six faces, and center/edge/corner regions. Worst view error is
0.069662 mm, local error 0.053008 mm, normal error 0.000014136 rad. Each large
radius uses a local footprint suited to the checked near-view budget.

VERIFIED: the separate ignored renderer test uses body and observer sibling frames,
nontrivial rotations, three radii, and common ancestor translations 0, 1.5e11, and
1e16 m. All 729 points pass; worst view error 0.022637 mm, independent of the common
translation (`gpu-precision-2.log`). The real Moon capture also records its actual
~1.5081e11 m root translation, but shared-frame cancellation alone is not evidence
of the sibling-frame conversion criterion.

## Residency, resources, and timing

VERIFIED in `gpu-candidate-5/residency-analysis.json`: the cold draw uploads exactly
143,648 B once. All 120 warm frames upload zero terrain bytes, preserve the exact
identity, keep cumulative upload count one, and keep allocation capacity/count
unchanged. Six camera/light/presentation captures also reuse content. Revision
invalidation uploads once. One-shot diagnostic readback is separately accounted;
ordinary frames do not rebuild, wait for tile construction, or read geometry back.

| Account | Prototype amount |
| --- | ---: |
| CPU retained texels | 89,780 B |
| Builder direct stack scratch estimate | 392 B |
| Authority query heap scratch | 0 B |
| Generator retained heap / bound | 3,376 / 3,728 B |
| GPU tile payload | 143,648 B |
| Shared grid vertex/index resources | 132,104 B |
| Compact parameters per ordinary draw | 176 B |
| Diagnostic output buffer capacity | 1,065,024 B |
| Additional transient mapped readback during canonical validation | 270,400 B |
| Total requested resident buffer capacity | 1,340,952 B |
| Cumulative buffer creations after cold preparation | 6 |
| Live resident buffers after cold preparation | 5 |

These are explicit requested buffer capacities, including the diagnostic output;
physical driver VRAM, allocator overhead, whole-process RSS, and queue-internal
staging memory are not measured. They are not permanent engine budgets.
The creation counter includes the replaced initial 32 B tile buffer; it is not
a count of simultaneously live buffers.
The mapped readback is allocated only for diagnostic validation (4,225×64 B in
the canonical case), copied, unmapped, and released. Its requested peak plus the
resident buffers is 1,611,352 B; ordinary draws have no mapped readback buffer.

MEASURED on AMD Radeon RX 9070 XT/Vulkan, 960×540 offscreen capture: canonical CPU
build 585.395 ms for 31,430 complete queries. Warm CPU preparation median/p95/max
0.89165/0.928/1.4403 ms. Warm GPU terrain timestamp median/p95/max
0.00400/0.00424/0.00436 ms, 120 positive samples. Cold timestamp zero is unavailable,
not a zero-cost draw. These scopes exclude complete frame delivery and diagnostic
readback; no FPS or speedup against the old renderer is inferred.

## Runtime and visual observations

VERIFIED: the owned native real-Solar-System scenario completed all checkpoints:
cold publication, unchanged presentation reuse, camera/light reuse, and one revision
invalidation. It produced ten paired native captures, including old CPU comparison
and disabling the fixture. Source/executable hashes are in `native-owned/` and the
scenario receipts in `native-scenario/`. Only this owned session was stopped.

OBSERVED: accepted offscreen and native lit captures contain displaced relief;
height/normal/material/UV modes and all three families display through the same
path. Material variation in this small region is subtle. The dense grid overlay
has visible aliasing. The CPU comparison uses the existing Grid16 path and its
existing palette/lighting; it is useful for coverage/relief comparison, not an
equal-density or identical-shading visual metric. Prototype images do not accept
the user's outstanding art or native UX goals.

`canonical-cpu-samples-final.json` records eleven exact complete/derived node
comparisons and six actual-triangle-centroid comparisons. Its key uses the same
surface revision 2 and material identity as the accepted cold fixture. Across all
4,225 authoritative interior nodes, the four material channels range respectively
0.483–0.824, 0.052–0.323, 0.056–0.096, and 0.028–0.138. The field varies materially;
the current diagnostic palette makes that variation subtle.

The fast developer check passes. Its separate Earth image was inspected; its
snapshot explicitly says `quality_pending=true`, `settled=false`. That legacy
image is not settled-terrain evidence for this prototype.

## Validation and continuation checkpoint

The complete 13-command quality matrix passes using the full run plus its focused
repair rerun (`quality-matrix.json`). Full debug/release workspace tests, rustdoc,
long-orbit checks, bridge tests, and all four GPU regression targets pass. Both
Clippy configurations, format, and all-target check pass in `quality-final/` after
correcting the runtime test's missing developer feature gate and a lint in the
additive CPU evidence example. Earlier rejected
GPU candidates are retained: viewport auto-refit, command-history-dependent
definition identity, and nominal-radius camera rejection were diagnosed and
corrected before the accepted candidate-5 run.

| Slice 2A gate | Result and evidence |
| --- | --- |
| A — authority preserved | PASS: inspected world → builder → renderer flow; no shader geology |
| B — deterministic derivation | PASS: focused builder tests, exact keys, historical bitwise replays |
| C — GPU displacement | PASS: actual render plus shared vertex/compute reconstruction, 68 paired captures |
| D — reconstruction error | PASS: canonical/54-case GPU errors within predeclared physical limits |
| E — precision | PASS: large radii, all faces, edges/corners; sibling-frame offsets through 1e16 m |
| F — persistent residency | PASS: 120 unchanged frames, stable key and allocations |
| G — changed-only uploads | PASS: all warm/presentation frames have zero terrain uploads |
| H — invalidation | PASS: revision and radius/definition runtime captures; exact-key/slot CPU tests cover other identity fields |
| I — generic families | PASS: RockyV5, IcyV3, VolcanicV3 use identical renderer/builder path |
| J — resources observable | PASS: CPU payload/scratch, GPU payload/capacity/topology/metadata, diagnostic costs separately recorded |
| K — old renderer preserved | PASS: baseline diff audit, existing GPU regressions, native comparison/disable captures |

The fifteen additional continuation conditions are individually recorded as PASS
in `slice2a-checkpoint.json`, including the distinction between approximation and
GPU error, no unresolved stale/lifetime failure, required tests, and attributed
source state. The source/executable manifests and accepted-source copy retain this
checkpoint independently of subsequent changes. Reviewer source inspection found
no blocking publication, lifetime, precision, or geometry-normal defect.

**Continuation decision: PASS.** Proceed only to the fixed parent + four children
prototype in the separately authorized Slice 2B contract. Stop after that proof;
this checkpoint does not authorize whole-body selection, streaming, or Slice 2C.
