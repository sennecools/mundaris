# Phase 5.11E terrain recovery checkpoint

## Verdict

**PARTIAL. Acceptance A remains unpassed.** Native draw-state and deferred far
handoff failures are repaired and exercised through the production window route.
GPU-resident terrain, dirty geometry uploads, GPU morph interpolation, continuous
retargeting, and unique-allocation headroom accounting are not implemented. This
checkpoint is not completion of Phase 5.11E.

No terrain morphology, deterministic generator, physical radius, CPU cap, or
LOD/precision/error threshold is changed. The historical **114.533 ms** event is
not claimed fixed: neither its reproduction nor its corresponding root cause has
been established.

## Native recovery: PASS for the exercised paths

The missing group was **group 0, the celestial 64-byte projection uniform**, in
the historical-line draw in a new overlay pass. It was not a missing far-sphere
resource. A new pass has no inherited bindings; planetary group 0 also does not
constitute the celestial projection binding. Sphere, history, and styled-curve
sections now bind their complete renderer-owned draw state. Pipeline labels
distinguish these sections.

The full-frame fixture exercises the production `CelestialRenderer` ordering with
far spheres, ready terrain, CPU transition/fallback, planetary Natural layers and
toggles, diagnostics, exclusive far/terrain ownership, history, styled curves,
and deterministic repeated readback. On original `d81bb2a` ordering it fails with
the exact missing-group-0 validation error; repaired ordering passes. Capture
validation is scoped and returned as `RenderPreparationError::GpuProgress`, rather
than allowing the backend's secondary teardown panic to obscure the test failure.
This offscreen fixture alone is **not** native-window evidence.

The required native command was also run:

```powershell
cargo run --locked --release -p mundaris_app --all-features -- --solar-system
```

The unmodified overview reached 8,475 drawable probe records and closed cleanly.
It did not admit terrain. The opt-in `MUNDARIS_SOLAR_VALIDATE=1` route uses normal
Solar controls: overview, Earth focus, 10 km approach, inspection, horizon, lateral
movement, retreat, and overview. Hidden/long-gap time does not advance it; default
startup is unchanged.

That route first exposed a separate application error, `source far replacement
not ready`. A near-plane-crossing far sphere can be a `PrecisionMarker` when the
displaced terrain bound no longer requests admission. Such a marker is not a ready
opaque replacement. Population handoff now retains/re-culls ready source coverage
until far preparation succeeds, including when terrain is toggled off. Renderer
selection honors the retained cover. Tests cover both enabled and disabled terrain
and check Earth's observation owner, not the number of owners of unrelated bodies.

The repaired native route completed steps **0–7**, exited **0**, and logged clean
shutdown with no recorded validation/panic/render errors on **RX 9070 XT / Vulkan**.
Its owned window was closed gracefully. A separate application initially locked
the default release executable; it was never terminated. The rerun used
`CARGO_TARGET_DIR=target/planetary-validation` with the same Cargo command. A prior
early-close run is retained separately and does not count as full-route validation.

Evidence: [`evidence/phase511e/p0/`](evidence/phase511e/p0/), especially
`original-ordering.log`, `repaired-ordering.log`, `native-summary.json`,
`native-route.log`, `native-route-summary.json`, and the curated native excerpt.
GPU profiles are asynchronous latest-completed scopes, not input-to-present or
current-frame latency. Native launch success does not establish human control feel
or acceptance of shell visuals.

## Partial implementation

### Immutable sample sharing

Generated sample grids now use `Arc<[TerrainGeometrySample]>`. Unchanged stitched
wrappers share raw/reused grid allocations while retaining outward-expanded bounds
and canonical metadata. Stitching uses bounded stack scratch and indexed lookup.
Tests check actual allocation sharing, exact sample bits, exact boundaries, and
split/merge reuse. Constructor reservations include temporary `Vec`/`Arc`
coexistence and Arc metadata.

Admission still conservatively charges each referenced grid. No unique allocation
census exists; real sharing does **not** yet prove recovered operational headroom
under the unchanged **128 MiB** cap.

### Projected-duration progression

Publication computes a conservative screen displacement bound for the affine
triangle transition. A triangle is ignored only when all source/destination
endpoints are outside one common frustum plane with margin. Endpoint
depth/displacement bounds and an inflated perspective Jacobian bound the remaining
interval. Near-plane uncertainty returns infinity and retains ordinary timing;
unsafe normal interpolation likewise retains ordinary timing.

Bounded positional changes below an **8-pixel duration-policy envelope** may use
shorter positive timing, with a **64 ms floor** and no extension beyond the
captured ordinary duration (normally 150 ms). This does not change LOD or renderer
precision/error thresholds.
Construction and queued transitions retain their captured durations; later control
changes do not retime an active transition. Tests cover sampled affine positions,
view dependence, large origins/radii, near crossings, normal hazards, duration
capture, and the positive floor. The policy is not an invisibility proof for
normals, materials, lighting, or whole-view change.

There remains one immutable-endpoint successor. Morph evaluation/clipping and
repeated regular terrain conversion are still CPU work. No GPU-only interpolation,
resident-patch precision certificate, dirty-upload cache, coalesced retargeting,
or SIMD/threading optimization is claimed.

The per-reference Arc metadata charges can change admission at a nearly saturated
cap. Consequently, allocation-sharing tests prove unchanged sample/seam values for
matched geometry, not identical system-wide cover selection. The static 10 km
before/after covers and captures must be compared as differing admitted covers;
they do not justify a no-visual-regression or unchanged-detail acceptance claim.

## Measurements and validation

Fresh before/after measurements and the final quality matrix are retained under
[`evidence/phase511e/`](evidence/phase511e/). Source/executable SHA-256 fingerprints,
toolchain versions, exact commands, timestamps, and exit codes accompany each run.
Timing runs do not overlap builds/tests/native applications. The original worktree
retains original production source except the capture validation wrapper; the new
full-frame test is also copied there to demonstrate the original draw-state failure.
These fixture additions are visible in its source fingerprint.

Toolchain: `rustc 1.98.1 (48a229cea 2026-09-01)`, `cargo 1.98.1`,
`x86_64-pc-windows-msvc`. GPU capture uses RX 9070 XT / Vulkan with inside-pass
timestamps. `after/` preserves the first measurement snapshot; `after-final/`
rebuilds after test-only final corrections. No intermediate measurements are
silently relabeled as final-source evidence.

The measurement authority remains the recorded cold 1/2/5-second and descent,
motion, and body-switch routes. Camera-radial ready/rendered LOD is supporting
evidence only, **not useful whole-view quality**. Unchanged static capture repeats
measure full payload uploads; geometry residency hits/misses and GPU-path coverage
are not applicable because those architectures are absent.
Coverage of a new resident-transform/GPU-morph path is **0%**: the GPU still
rasterizes prepared terrain, but every geometry conversion and morph evaluation
uses the retained CPU preparation path. Existing GPU buffer capacity/growth
statistics are not a keyed terrain-residency hit/miss census.

### Final before/after measurements

Four workers, ordinary duration 150 ms, zero artificial soft-headroom reservation.
All routes/builds exit 0. Tables use **median / nearest-rank P95 / worst**, in ms;
total quantiles are computed from per-frame update + preparation sums. These are
wall-clock CPU opportunities, not native FPS or input-to-present latency.

| Descent | Update | Preparation | Update + preparation |
| --- | --- | --- | --- |
| Before 1 | 3.358 / 5.448 / 12.254 | 7.925 / 12.555 / 19.716 | 11.024 / 16.409 / 23.857 |
| Before 2 | 3.359 / 5.546 / 26.450 | 7.878 / 12.505 / 29.527 | 10.951 / 16.521 / 46.698 |
| Final after 1 | 3.598 / 4.404 / 10.348 | 6.414 / 9.828 / 18.865 | 10.123 / 13.130 / 22.686 |
| Final after 2 | 3.662 / 4.516 / 34.849 | 6.557 / 10.006 / 25.787 | 10.341 / 13.460 / 41.862 |

Descent shows lower CPU P95 in both repeats, but this is not GPU-residency or
dirty-upload evidence. Active-frame captured duration median changes from **150
to 64 ms**; final P95 is 69/70 ms, worst 86 ms. Local-useful completions change
239/243 → 312/316; that classifier is not whole-view quality. Worker publication
timelines and admitted covers differ, so there is no isolated causal attribution
of every latency change to sample sharing.

| Route | Before update + preparation | Final after update + preparation |
| --- | --- | --- |
| Cold 2 m | 7.691 / 15.130 / 42.534 | 7.881 / 16.371 / 31.129 |
| Cold 2 m with checkpoint GPU readback | 7.720 / 14.559 / 18.958 | 7.907 / 14.424 / 19.799 |
| Motion | 6.060 / 14.842 / 18.504 | 5.949 / 14.552 / 43.874 |
| Body switch | 2.492 / 6.382 / 7.100 | 2.524 / 6.664 / 19.239 |

Readback waits are not included in the cold-capture update/preparation sum.
Motion and switch worst frames are worse in the final run. No scheduler cause or
hitch-free behavior is asserted. The old 114.533 ms event remains unresolved.

#### Cold quality: no improvement established

| Checkpoint | Before rendered / ready radial LOD | Final after rendered / ready radial LOD | Desired |
| --- | --- | --- | --- |
| 1 s | 5 / 7 at 1,013 ms | 5 / 7 at 1,014 ms | 23 |
| 2 s | 8 / 10 at 2,008 ms | 8 / 10 at 2,008 ms | 23 |
| 5 s | 18 / 19 at 5,016 ms | 17 / 19 at 5,006 ms | 23 |

The earlier after snapshot reached rendered LOD18 at 5 s; the final repeat reaches
17. Cold captured durations remain 150 ms: near-view conservatism is not bypassed
to advertise faster convergence. Neither snapshot proves useful whole-view quality
at one/two seconds. **Acceptance A is not passed.**

#### Uploads and memory: architectural goals unmet

| Descent terrain payload B/frame | Median | P95 | Worst |
| --- | ---: | ---: | ---: |
| Before 1 | 3,929,952 | 5,555,888 | 8,563,360 |
| Before 2 | 3,929,952 | 5,583,760 | 8,563,360 |
| Final after 1 and 2 | 3,874,208 | 5,308,960 | 8,563,360 |

Different publication/transition timing changes payload distributions; it does not
demonstrate unchanged-frame dirty upload savings. All four descents peak at exactly
**134,217,728 B (128 MiB), zero accounted headroom**. Final cold peak leaves **1,848
B**, versus 3,147 B before; motion leaves 828 B. The switch route's roughly 7.25 MiB
free space also existed before and is not newly recovered operating margin.
`memory.csv` retains class breakdowns; maxima from different frames are not summed
into a fabricated simultaneous peak. Exact unique allocation census, per-owner
peak snapshots, RSS, and actual driver VRAM remain unmeasured.

The static 10 km fixture still uploads its full payload on all unchanged repeats:
**8,055,072 B before / 8,013,264 B final after**, including the 64-byte lighting
uniform. GPU terrain buffer capacities are 8,108,688 / 8,063,632 B, with zero
growth/waits on the last repeat. The small payload difference reflects 578 → 575
visible patches, not residency reuse. Planetary-mode CPU preparation medians over
20 repeats are **13.616 / 13.939 ms**; the stationary CPU cost is not repaired.
There are nine terrain draws, one far sphere, and three planetary layer draws.

#### Static GPU query evidence

RX 9070 XT / Vulkan, actual **last of eight all-layer repeats**, in ms; not medians,
not native presentation latency, and not added to enclosing pass totals:

| Scope | Before | Final after |
| --- | ---: | ---: |
| Main celestial pass | 0.53864 | 0.43520 |
| Regular terrain | 0.34740 | 0.28524 |
| CPU-prepared transition/fallback | unavailable: no draw | unavailable: no draw |
| Ocean | 0.02240 | 0.02112 |
| Clouds | 0.16384 | 0.12392 |
| Atmosphere pass/layer | 0.23336 | 0.18388 |
| Remaining celestial sphere | 0.00060 | 0.00048 |

These unmatched-cover single query intervals do not establish an optimization.
Disabled layers report `None`. Each snapshot's eight unchanged repeats are
pixel-identical internally, but before/final images are not identical across the
differing covers: **16 of 17** paired BMPs differ. Final static cover has 1,074
leaves versus 1,077 before, remains
quality-pending/unsettled, reaches the 800-update limit, and has one pending job
versus zero before. Additional conservative Arc charges affect saturated admission;
this is an unresolved accounting/admission limitation, not higher quality or headroom.

Exact reproduction commands (fresh measurement destinations are required):

```powershell
./docs/evidence/phase511e/measure.ps1 -Source C:/Users/senne/AppData/Local/Temp/opencode/mundaris-phase511e-original -Target target/phase511e-baseline -Destination docs/evidence/phase511e/before -Label phase511e-original-d81bb2a -RepeatDescent
./docs/evidence/phase511e/measure.ps1 -Destination docs/evidence/phase511e/after-final -RepeatDescent
./docs/evidence/phase511e/summarize.ps1
./docs/evidence/phase511e/validate.ps1 -TargetDir target/planetary-validation
./docs/evidence/phase511e/native-route.ps1
```

Validation retries remain in `validation-first-pass/` and
`validation-second-pass/`. The first run found a new fixture Clippy lint and an
old test's fixed-150-ms assertion, which no longer expressed captured-duration
invariance. The test now captures the selected duration, checks the positive
bounds, verifies that changing the setting leaves that exact duration unchanged,
and advances by the captured duration. The first Clippy correction then exposed a
fixture reference-type mismatch; it was corrected and checked before another full
matrix. These were test corrections, not production timing/threshold changes.

### Final quality matrix: PASS on this Windows host

All ten recorded commands exit **0**. Commands and full output are in
`evidence/phase511e/validation/quality-results.json` and its per-command `.txt`
files; Rustdoc is run with `RUSTDOCFLAGS=-D warnings` and restored afterward.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| Locked workspace/all-target/all-feature check | PASS |
| Locked all-target warnings-denied Clippy, all features | PASS |
| Locked all-target warnings-denied Clippy, default features | PASS |
| Locked workspace/all-feature debug tests | 256 passed / 0 failed / 4 ignored |
| Locked workspace/all-feature release tests | 256 passed / 0 failed / 4 ignored |
| Locked all-feature warnings-denied Rustdoc, no dependencies | PASS |
| Separately selected ignored release long-orbit tests | 2 passed |
| Separately selected ignored release native close-surface GPU fixture | 1 passed |
| Separately selected ignored release full-frame GPU fixture | 1 passed |

All four ignored tests pass when separately selected. The two GPU fixtures are
adapter-required offscreen tests, not native-window tests; the separate production
window route supplies native evidence. No Linux or remote-CI result is claimed.

## Outstanding acceptance requirements

| Requirement | Status |
| --- | --- |
| Native rendering and production-order GPU regression | PASS for exercised routes; not all adapters/platforms |
| Bounded GPU terrain residency and dirty unchanged uploads | FAIL / not implemented |
| GPU morph interpolation with conservative precision/seam proof and CPU fallback | FAIL / not implemented |
| Faster visible convergence and continuous/coalesced retargeting | PARTIAL policy only; Acceptance A not established |
| Several MiB repeatable real headroom under 128 MiB with unique allocation accounting | FAIL / not established |
| Planetary shell precision-path changes | NOT IMPLEMENTED; existing paths remain |
| Historical 114.533 ms root-cause resolution | OPEN; no fix claim |
| SIMD/threading audit beyond existing worker regressions | NOT PERFORMED |
| Human control feel, whole-view quality, and shell visual acceptance | NOT ESTABLISHED |
| Acceptance A | NOT PASSED |

## Changed files and Git

- Renderer draw state/capture: `crates/renderer/src/celestial.rs`, `terrain_capture.rs`.
- Sharing/stitching/projection: `crates/renderer/src/planet_surface/terrain_geometry.rs`,
  `stitching.rs`, `transition.rs`; corresponding renderer tests.
- Application admission/progression: `crates/app/src/planet_terrain.rs`,
  `planet_terrain/adaptive.rs`, `terrain_population.rs`, `gravity_orbits.rs`.
- Regressions: `crates/app/tests/terrain_population.rs`, `native_full_frame.rs`.
- Report/evidence/scripts: this report and `docs/evidence/phase511e/`.

All changes remain uncommitted and unpushed. The starting working tree was clean;
no unrelated user changes were reverted. The final short status is retained in
`evidence/phase511e/git-status-short.txt`.
