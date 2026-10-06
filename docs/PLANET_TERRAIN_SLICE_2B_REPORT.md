# Slice 2B fixed resident hierarchy checkpoint

Date: 2026-10-06. Status: **PASS — all B1–B12 gates; stopped after Slice 2B**.

## Goal

Complete the user's gated continuation: accept one resident GPU terrain tile
first, then prove one parent and four children, transition/seam correctness, and
stop. The acceptance authority is [Slice 2B](PLANET_TERRAIN_SLICE_2B.md), B1–B12.
The prerequisite [Slice 2A checkpoint](PLANET_TERRAIN_SLICE_2A_REPORT.md) passed
all 15 continuation conditions before hierarchy implementation began.

## Result

VERIFIED: the fixed hierarchy's focused CPU/GPU tests, paired offscreen captures,
native torture scenario, capture-free native delay measurements, single-tile
regression, fast developer check and all 13 final quality commands pass.
No Slice 2C, whole-body selector, streaming, pressure-driven eviction, or
performance optimization was started. No commit or push was made.

| Gate | Result | Evidence |
| --- | --- | --- |
| B1: Correspondence | PASS | CPU tests prove canonical quarter mapping, orientation, shared coordinates, anchors, and parent-local reconstruction. |
| B2: Same-level borders | PASS | Actual-world GPU seam maximum 0.018394 mm across three radius/interior-edge-corner fixtures and five fractions; shared material residual is zero. |
| B3: Actual parent surface | PASS | Independent CPU triangle/centroid tests reconstruct the rendered `[a,b,c,b,d,c]` surface and its raw attributes at fraction zero; actual GPU checks and paired coarse images corroborate it. |
| B4: Child endpoint | PASS | Fraction one matches each child's own derived representation in CPU tests and actual GPU readback. |
| B5: Continuity | PASS | Shared-edge/corner tests at 0, 0.25, 0.5, 0.75, 1; inspected intermediate native/offscreen captures; reversal retains the same resident endpoints. |
| B6: Normal/material coherence | PASS | Raw parent normal varying and material interpolation survive subdivision; measured residuals and inspected normal/material endpoint/intermediate captures. |
| B7: Persistent content | PASS | 821 unchanged-slot offscreen frames have zero terrain-content uploads; native camera/mode/repeated-transition captures preserve upload counts. |
| B8: Readiness fallback | PASS | Three children can build/upload independently with the fourth absent; full parent coverage remains drawn. |
| B9: No synchronous refinement wait | PASS | Native worker-pending frame intervals remain below 9.00 ms for 50/250/500 ms injected delays; frame-path polling/queueing is nonblocking. |
| B10: Stale protection | PASS | At least four completed old requests are rejected after epoch cancellation in both hosts; authority invalidates before polling; GPU transaction regression preserves the previous valid publication. |
| B11: Precision | PASS | Actual world tiles and independent common-offset/sibling-frame GPU tests pass explicit 1 mm position, 1 mrad normal, and 1e-5 material tolerances. |
| B12: Old path | PASS | Native restoration, 68-pair single-tile regression, and all 13 final quality commands pass. |

## Architecture changes

IMPLEMENTED: world ownership remains `CelestialBody` → immutable definition →
`SurfaceGenerator` → app-owned `ResidentTileBuilder`. The renderer consumes derived
texels. No geological authority or procedural terrain was moved into a shader.
All 53 initial world/math Rust files, including six benchmark files, retain their
initial hashes; the source/test subset contains 47 files, including 25 source files.
Slice 2A's 13,263-query historical family/province/hierarchy bitwise replay remains
attributed to its immutable prerequisite checkpoint.

The existing resident renderer now owns exactly five physical slots: pinned parent
slot zero and four canonical child slots. One indexed UV grid and pipeline are
shared. Independent asynchronous builds and uploads do not transfer coverage:
the parent draws until all four children are GPU drawable, then all four child
draws publish at fraction zero. Atomic coverage avoids an unproved mixed-level
edge topology; it does not require synchronous generation or block frames.
See [ADR 0013](adr/0013-fixed-resident-terrain-hierarchy.md).

Positions morph on the GPU from barycentric evaluation of actual parent triangles
to child derived vertices. Equal-resolution child grids preserve parent grid lines
and triangle diagonals under subdivision; the supported resolution policy remains
power-of-two, matching the builder. Both endpoints use a common parent-local
frame; child-minus-parent body-fixed anchors are subtracted in f64 before narrowing.
Final positioning uses the parent's prepared observer-relative transform. Large
absolute anchors are never subtracted in f32.

The coarse endpoint also preserves the actual parent's **raw** interpolated normal
varying and material weights. Blend these toward the child's attributes, then
normalize after raster interpolation. Premature per-vertex normalization would
change the coarse shading field. No CPU transition mesh, skirt, or ordinary-frame
geometry readback is used.

Four persistent workers have one queued job each, four bounded pending entries,
a 32-entry result channel, and a maximum of 12 job copies. Each worker requests a
4 MiB stack: 16 MiB virtual stack reservation, not measured committed memory.
Ordinary frames use `try_recv`/`try_send`; disabling or cancelling does not join
workers. Cancellation changes publication epochs; already-running finite CPU work
may finish and is rejected when late. Shutdown alone joins workers, so shutdown
latency can include an in-progress finite tile build; its worst case is UNTESTED.

Parent authority is rechecked before accepting completions. Invalidation advances
the epoch and suppresses child upload/readiness until reconfiguration. Renderer
publication preflights every changed payload and candidate token before mutation;
an invalid later child cannot poison earlier slots or discard the last valid set.

Reversal moves the existing common fraction toward the new target. Children retain
ownership until returning to zero. Ready children survive repeat demand. Morph
duration supports zero, ordinary 150 ms, and deliberately slow inspection values;
these are fixture parameters, not final engine policy. Diagnostics distinguish
the requested fraction from the renderer's last submitted fraction.

## Files changed

Task additions/updates, separate from the extensive pre-existing dirty tree:

- `crates/renderer/src/resident_hierarchy.rs`: shared correspondence and f64 reference reconstruction.
- `crates/renderer/src/resident_tile/gpu.rs`, `shaders/resident_tile.wgsl`: five resident slots, transactional publication, GPU morph and diagnostics.
- `crates/app/src/gravity_orbits/hierarchy_fixture.rs`: bounded workers, epochs, readiness, reversal, native timing and diagnostics.
- App frame-host/protocol/snapshot/service/bridge integration: the opt-in `GpuHierarchy` command and observable submitted state.
- `crates/app/examples/resident_hierarchy_capture.rs`: fixed offscreen torture fixture; `scenarios/developer/gpu-hierarchy-transition.json`: reusable native replay.
- App/renderer `resident_hierarchy*` tests: correspondence, raw attributes, CPU triangle subdivision, actual GPU precision and publication rollback.
- README, architecture, interface guide, reviewer context, ADR 0013, this report, and the evidence-manifest script.

The initial 5,128-file manifest and final preservation differences identify exact
task edits; the current Git diff alone also contains unrelated user work.

## Tests

Windows, locked Cargo, AMD Radeon RX 9070 XT/Vulkan. Final focused commands pass:

```powershell
cargo test --locked -p mundaris_app --all-features --test resident_hierarchy --test resident_hierarchy_runtime
cargo test --locked -p mundaris_app --all-features --lib hierarchy_fixture::tests::
cargo clippy --locked -p mundaris_app --all-targets --all-features -- -D warnings
cargo test --locked --release -p mundaris_renderer --all-features --test resident_hierarchy_transaction -- --ignored --nocapture
cargo test --locked --release -p mundaris_renderer --all-features --test resident_hierarchy_gpu -- --ignored --nocapture
cargo test --locked --release -p mundaris_renderer --all-features --test resident_tile -- --ignored --nocapture
cargo test --locked --release -p mundaris_app --all-features --test resident_hierarchy_world_gpu -- --ignored --nocapture
```

CPU hierarchy/runtime integration tests: 6 pass; fixture unit tests: 3 pass.
Each explicit GPU command passes and requires an actual adapter. The final
13-command workspace matrix passes: format, workspace check, all-feature/default
Clippy, debug/release tests, rustdoc with denied warnings, long orbits, developer
bridge and four existing ignored GPU suites. Raw commands/results are in
`full-validation-final/validation.json`; debug/release durations were
495.35/97.76 s. The fast developer check also passes.

Final broad commands:

```powershell
$env:CARGO_TARGET_DIR = Join-Path (Get-Location) 'target/slice2a-validation'
./scripts/ai-check.ps1 -OutputDirectory target/terrain-redesign/slice2b/ai-check-final
./scripts/validate.ps1 -IncludeGpu -OutputDirectory target/terrain-redesign/slice2b/full-validation-final
```

The fast check passes all ten records. Its paired Earth-orbit capture was inspected
and reports `quality_pending=true`, `settled=false`; it verifies the normal fixture
and interface, not settled terrain quality. Remote CI and Linux checks are UNTESTED.

The single-tile regression runs `resident_tile_capture` followed by
`scripts/check-resident-tile-evidence.ps1` and the metadata-aware residency audit:
68 PNG/JSON pairs, all 54 precision cases, all three surface families, 120 warm
frames, revision invalidation, and presentation-only reuse pass. Its worst view
error remains 0.069662 mm; every warm terrain upload is zero. Hierarchy support
changes single-patch metadata from 176 to 384 B and preallocates the five-slot
pool; this is explicit resource growth, not a claimed single-tile optimization.

Validation binaries use `CARGO_TARGET_DIR=target/slice2a-validation` to avoid an
unrelated running developer executable's lock. Cache directories are junctions
to the existing target; top-level binaries are separate. Unrelated sessions were
left running. Native evidence uses its own immutable executable and manifest.

## Measurements

Canonical fixture: RockyV5, seed 0, body/definition identity
`5931033225171238913`, radius 80,000 m, +Z parent level 9 (157,39), 64 cells.
Children are level 10 (314,78), (315,78), (314,79), (315,79). The original
Slice 1B.2 retained direction lies within this region. Dirty source HEAD is
`ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`; manifests identify actual inputs.

MEASURED: actual-world GPU readback checks five patches × five fractions × 4,225
vertices × three fixtures = **316,875 points**. Errors compare GPU reconstruction
against f64 derived-tile/morph reconstruction, not directly against analytic geology.

| Fixture | Max view error, mm | Max normal error, rad | Max child seam, mm |
| --- | ---: | ---: | ---: |
| 80 km interior | 0.061434 | 6.3821e-6 | 0.017994 |
| 6,371 km face edge | 0.084965 | 7.2660e-6 | 0.018394 |
| 70,000 km face corner | 0.074704 | 1.2337e-5 | 0.015728 |

Maximum local error is 0.029833 mm; maximum material L2 error is 5.8681e-8.
Shared material L2 error is zero; maximum seam normal error is 1.0976e-5 rad.
Independent synthetic-payload GPU testing adds 18,225 points across three radii,
common offsets 0/1.5e11/1e16 m, nontrivially rotated sibling frames and all fractions:
max position 0.052562 mm, normal 5.3727e-8 rad, seam 0.017060 mm. That test isolates
precision; the actual-world tests establish authoritative derived input.

World → coarse tile approximation remains separate: canonical parent maximum/RMS
radial error 0.317530/0.086224 m; triangle-centroid position error
0.788504/0.176603 m; normal angle 0.905330/0.323645 rad. Canonical children have
maximum radial errors 0.1525–0.1816 m and triangle-centroid errors 0.3676–0.5110 m.
These quantify filtering/spacing loss; they do not accept fine appearance.
Each canonical tile performs 31,430 authoritative queries. Direct canonical
builds take about 571–598 ms per child in the identified world-GPU run; asynchronous
worker builds and queue ages are separately recorded in frame snapshots.

Native delay measurements run without capture or diagnostic GPU readback while
children are pending. Telemetry includes the interval ending at the last completion
and omits the first pre-request interval; each phase uses fresh parent content.

| Injected delay per child, ms | Pending frame intervals | Mean interval, ms | Largest interval, ms |
| ---: | ---: | ---: | ---: |
| 0 | 104 | 8.3322 | 9.0635 |
| 50 | 115 | 8.3397 | 8.8330 |
| 250 | 140 | 8.3334 | 8.8852 |
| 500 | 166 | 8.3362 | 8.9952 |

The 50–500 ms worker delays did not become equivalent render-thread stalls.
This is native interval evidence for this fixed fixture, not general FPS acceptance.
Ordinary terrain build/GPU waits are zero by the frame path and diagnostics.
Explicit reconstruction diagnostics intentionally wait/read back and are separate.

Offscreen candidate 2 records 832 frames and 58 capture pairs: 821 unchanged-slot
frames upload zero content; 762 frames have queued/building children. Warm CPU
preparation median/p95/max is 0.9119/1.1466/4.0735 ms; GPU terrain timestamps are
0.00404/0.01188/0.02476 ms. This mixes parent and four-child draws. Pending outer
offscreen calls, including image readback, are 1.6211/1.9369/19.8169 ms; these
are not native frame times. No performance improvement or universal target is claimed.

| Resource | Canonical resident parent + four children |
| --- | ---: |
| CPU retained texel payload | 448,900 B (89,780 parent + 359,120 children) |
| GPU terrain payload / initial uploads | 718,240 B (143,648 per tile, five content uploads) |
| Shared grid | 132,104 B |
| Metadata per patch / five-patch frame | 384 B / 1,920 B |
| Uniform stride / fixed uniform pool | 512 B / 2,560 B |
| Five terrain buffer capacities | 1,310,720 B (262,144 each) |
| Fixed diagnostic output capacity | 1,065,024 B |
| Total requested resident buffer capacity | 2,510,408 B |
| Explicit canonical diagnostic readback | 270,400 B additional; requested peak 2,780,808 B |
| Live resident buffers / cumulative creations | 9 / 14, stable after warm-up |

Changed payload packing allocates 143,648 B per canonical tile before queue upload;
unchanged frames pack/upload zero tile bytes. Driver-internal staging, allocator
overhead, process RSS, committed worker stack memory, and physical VRAM are UNTESTED.
Reported buffer capacities are requested bytes, not driver residency measurements.
Parent pinning is explicit while children use its fallback or reconstruction.

## Captures

OBSERVED: inspected parent/child coarse endpoint, lit intermediate/reversal,
normal/material 0/0.75/1, central shared corner, close edge crossings and native
restoration captures show no obvious internal hole or independent shading jump.
UV resets between child quadrants are intentional debug coordinates. Grid aliasing,
isolated patch boundaries and subtle material contrast are prototype presentation.

Same-camera parent versus four-child fraction-zero images differ at 221 of 518,400
pixels. 220 differ by at most one channel unit; one **outer boundary** pixel
(766,207) changes from background to terrain due to raster contour rounding.
The images are not byte-identical. CPU plane/subdivision proof, GPU physical
tolerances and internal-seam inspection establish the geometric gate separately.

Accepted offscreen evidence: `capture-candidate-2/`; earlier candidate retained.
Reproduce in a fresh path with
`cargo run --locked --release -p mundaris_app --features developer-tools --example resident_hierarchy_capture -- target/terrain-redesign/slice2b/reproduction`.
The close central-junction shots use offset (0,0,-190), about 28.75 m above the
anchor; crossings use ±15 m lateral displacement. Older outer-corner shots are
context, not central four-child corner proof. Snapshot keys, resident generations,
actual submitted fraction and validation flags identify each captured state.

Native `native-scenario-3/` completes 47 steps and 13 paired viewport/client
captures using `scenarios/developer/gpu-hierarchy-transition.json` (SHA-256
`fc7b4de68e2f3b50a042bd3d796f167cfab9395b50d97bffa6f4562fc79e8fd4`).
Content and request sequence are fixed; native timing is explicitly wall-paced,
not byte-deterministic. Session `3848-1791260818671701900`, PID 3848, executable
SHA-256 `c0034b37aa5aefbf5484f71e617a566fca99548111e1bbe13b24b0d95338c447`.
Its final shutdown reports `process_exited=true`. Native GPU timestamps unavailable
in these captures remain null; offscreen timing is not relabelled as native timing.

Native replay uses the developer CLI: `mundaris_dev --registry <fresh-registry>
launch real-solar-system <fresh-owned-directory>`, then `mundaris_dev --registry
<registry> --session <returned-session-id> scenario
@scenarios/developer/gpu-hierarchy-transition.json <fresh-evidence-directory>`.
Capture-free timing reproduces with `measure-native-delays.ps1` in the evidence
root, supplying that CLI, registry, session and another fresh output path.

## Known failures

No unresolved focused hierarchy gate or final quality-command failure remains.
Earlier failed runs are preserved: a reserved
WGSL identifier, an overlarge test patch exceeding the declared local-view budget,
an `acos` same-normal comparison, scenario integer-versus-float equality, retained
children between replays, and initial command-receipt handling. Their fixture/code
corrections have passing reruns. Source review also found and closed zero-morph
advance, reversal ownership, stale-parent publication and partial transaction defects.

UNTESTED/outside scope: final terrain art, human native control feel, global mixed
LOD coverage, global Acceptance A, other platforms/adapters, eviction under pressure,
long-range streaming, worst-case worker shutdown latency and physical memory use.
The normal-path `ai-check` terrain readiness must retain its actual quality flags;
unsettled terrain is not settled quality proof. Existing dated visual failures are
not cleared by this fixed hierarchy checkpoint.

## Evidence

Root: `target/terrain-redesign/slice2b/`.

- Prerequisite: `prerequisite-slice2a-checkpoint.json`, immutable 2A source manifest.
- Current input identity: `accepted-source-files.json`, binaries and preservation manifests; immutable `accepted-source/` checkpoint and `accepted-documents.json`.
- `final-source-audit.json`: all 257 final Rust/Cargo inputs match the validation state. Native/offscreen/fast-check inputs differ only in a later comment correction explaining the existing power-of-two policy; no executable statement changed.
- CPU/focused checks: `runtime-final.log`, `clippy-final-retry.log`.
- Actual GPU: `world-gpu.log`, `world-gpu-measurements.json`; independent precision and transaction final logs.
- Offscreen: `capture-candidate-2/hierarchy-audit.json`, raw frames, PNG/JSON pairs, `coarse-image-comparison.json`.
- `index.html`: offline review index of all 58 offscreen and 13 native pairs, with submitted fractions/readiness/uploads and direct JSON links.
- Native: `native-scenario-3/scenario-result.json`, `native-evidence-analysis.json`, session capture manifests, `native-delay-measurements-2/delay-measurements.json` and raw actions/inspections.
- Regression: `single-tile-regression/evidence-analysis.json`, `residency-analysis.json`, all 68 pairs.
- Final broad checks: `ai-check-final/`, `full-validation-final/`, `quality-matrix.json`.
- Raw evidence hashes: `accepted-evidence-files.json`; test executable hashes: `accepted-test-binaries.json`.
- Gate decision: `slice2b-checkpoint.json`; no continuation beyond it.

## Git state

Branch `main`, HEAD `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`; extensive initial
and final dirty/untracked state. Uncommitted task changes coexist with preserved
user work. Initial/final SHA-256 manifests and preservation differences identify
the implementation; HEAD alone does not. No commit, push or unrelated-session
shutdown occurred.

## Reviewer follow-up

Inspect the actual source and identified raw paired evidence against B1–B12 before
choosing broader hierarchy policy. This prototype proves fixed five-slot transition
and seam behavior; it does not establish a planetary selector/cache architecture.
Stop here and discuss any Slice 2C plan with the user.

**READY TO PLAN SLICE 2C.** Both prerequisite and fixed-hierarchy architectural
checkpoints pass. Planning requires user/reviewer discussion; this handoff does
not authorize implementing another slice.
