# Phase 5.12A — developer UI and diagnostic interface

## Status

**PASS for the scoped implementation and exercised Windows checks.** Human visual/
UX approval remains the user's decision; this is not terrain appearance acceptance,
camera acceptance, Acceptance A, or a performance optimization claim.

No terrain generator, morphology, LOD/error/quality threshold, camera mechanics,
128 MiB cap, GPU residency/generation, or planetary rendering architecture changed.
Existing uncommitted 5.11E work and new reviewer-workflow edits were preserved.

## Human UI

- Left: Scene, Camera, Time, Selected body and optional scene overlays.
- Right: Planet, Rendering, Terrain, Performance; planetary layers and diagnostics
  are separate. Both panels scroll; the inspection window no longer covers the scene.
- Advanced engine/terrain diagnostics are collapsed by default. LOD, cache, workers,
  memory, transitions, precision/bounds, timings, lighting, authoring and playback
  controls remain available. Technical LOD controls have short tooltips.
- Camera summaries use m/km/Mm/AU, signed terrain/mesh clearance and compact precision.
  Memory has an objective strictly-95%-exceeded advisory; no CPU warning threshold
  or GPU bottleneck diagnosis was invented.

Native observations: [overview UI](evidence/phase512a/native/overview-ui.png),
[Earth UI](evidence/phase512a/native/earth-ui.png). The exact release/all-feature
Solar launch reached overview, accepted F focus, and exited 0 after graceful close
on RX 9070 XT/Vulkan. The inherited egui sRGB framebuffer advisory is logged; no
render validation/panic failure was observed. Full operator/recovery/high-DPI and
Linux checks are not claimed.

## Canonical snapshot and capture

`crates/app/src/developer_snapshot.rs` owns schema 1, body/frame associations,
terrain status, warnings and formatting. `DeveloperSnapshot::collect` reads a
coherent published world/projection plus existing cover/cache, prepared camera,
render flags and measured stages; collection performs no procedural query or work
submission. Native summary rows and capture tools consume this object. Deep raw
diagnostics remain raw reports, not competing high-level status models.

Camera pose uses semantic system/body-translating/body-fixed frames and XYZW
orientation, not serialized runtime handles. Unavailable values are `null`.
Native GPU scopes are `latest_completed` (possibly older); capture scopes are
`same_frame`. Enabled layers are distinct from layers drawn in the image.

Named fixtures: **solar-overview, earth-orbit, earth-close, moon-orbit**.
Serial fixed operation budgets freeze world time and retain ordinary morph timing;
they do not pretend to settle terrain. PNG is lossless RGBA through the existing
`TerrainCaptureRenderer`. Snapshot/readback use the same prepared pose/state.
Pair writes stage both files and publish without replacing existing evidence;
handled encoding/publication failures roll back newly owned files.

```powershell
cargo run --locked --release -p mundaris_app --features terrain-capture,surface-profile --example developer_capture -- all target/mundaris-diagnostics/new-run
```

Each fixture emits `<scene>.png` and `<scene>.json`. Final inspected pairs are in
[the capture directory](evidence/phase512a/captures/). Earth close is an early,
coarse/ocean-dominated view, not accepted settled landform evidence.

## Developer workflow and validation

```powershell
./scripts/ai-check.ps1
./scripts/validate.ps1 -IncludeGpu
```

The first is a repeated-use fast review package: Git state/source hashes, format,
six headless interface tests, one GPU scene, Markdown summary and JSON command
results. It does not run the complete suite. The second records the full locked
quality matrix, including separately selected adapter tests when requested;
existing acceptance scripts remain unchanged. See
[AI development interface](AI_DEVELOPMENT_INTERFACE.md) for fields, terminology,
launch/capture commands, evidence limits and handoff guidance.

Windows results: format, locked all-target/all-feature check, warnings-denied Clippy
(all/default features), debug and release workspace tests, warnings-denied Rustdoc,
ignored long orbits, both existing GPU regressions and new capture regression pass.
Workspace matrices: **263 passed / 0 failed / 5 ignored** each. All five ignored
tests pass when selected separately. Final focused interface reruns: seven ordinary
all-feature tests plus the GPU test (four scene pairs, decoding and pixel repeats).
Tests cover serialization, units, actual engine reports, objective warning boundaries,
terrain-summary state, camera/body association and pair-failure cleanup.

The final fast checks passed for overview and the exact default
`./scripts/ai-check.ps1` Earth invocation. Their inspected pairs and command records
are retained in `evidence/phase512a/ai-check-final-overview/` and
`evidence/phase512a/ai-check-final-default/`; Earth reports `quality_pending=true`,
`settled=false`, while overview reports terrain as inactive rather than inventing
quality flags.

The initial release/GPU compilation failures were Windows locks caused by the
native launcher missing Cargo's intermediate process. That verified owned app
was gracefully closed, ownership handling corrected, and blocked checks rerun.
Original failures and successful retries are both retained in
[the evidence index](evidence/phase512a/README.md).

## Performance and files

The initial Earth fast run took about 11 seconds including rebuilds; later warm
checks are shorter. These are workflow durations, not engine speedups. Snapshot
and egui overhead were not isolated in a controlled benchmark. Capture still adds
GPU initialization/blocking readback, PNG encoding and disk I/O; none occurs in the
ordinary frame except an explicit native JSON export. No performance improvement
or zero-cost diagnostic claim follows.

Task edits: root Cargo manifests/lock; app manifest/module exports;
`gravity_orbits.rs` UI/snapshot wiring and `gravity_orbits/developer_ui.rs`;
`developer_snapshot.rs`, `developer_capture.rs`, corresponding capture example and
`tests/developer_interface.rs`; `scripts/ai-check.ps1`, `scripts/validate.ps1`;
README, the short AGENTS evidence loop, this report/interface guide and new evidence.
Renderer/world/math/terrain pipeline edits in Git status predate this phase.

## Deliberately deferred

- Camera/navigation redesign and human control-feel acceptance.
- LOD visualization correctness (including palette/path and hover hierarchy issues).
- Terrain morphology.
- GPU terrain residency/generation and GPU morph interpolation.
- **Acceptance A** — remains unpassed; this phase does not attempt it.
- Broader diagnostic matrices, visual acceptance, Linux/remote CI and exhaustive UX.

Everything remains uncommitted and unpushed. See retained Git status/source hashes
for the dirty inputs; HEAD alone does not identify the build.
