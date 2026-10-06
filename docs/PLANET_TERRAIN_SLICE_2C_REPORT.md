# Planet terrain redesign — Slice 2C report

Date: 2026-10-06. Branch `main`, HEAD
`ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`; uncommitted, with substantial
pre-existing changes. Evidence root: `target/terrain-redesign/slice2c/`.

## Goal

Implement the authorized [Slice 2C contract](PLANET_TERRAIN_SLICE_2C.md): finite
adaptive regional terrain, desired/resident/drawable separation, bounded
asynchronous preparation, cache reuse, mixed-level topology, local transitions,
cancellation and observable resource pressure. User acceptance remains separate
from implementation and numerical validation. No successor phase was started.

## Result

**PARTIAL.** The regional implementation and focused correctness checks pass.
Final cold and warm native pressure routes preserve coverage, avoid C-stage
reupload churn and recover on retreat. **C15 fails:** the identified native delay
benchmark contains terrain CPU publication/advance hitches, with a maximum wall
frame interval of 336.30 ms. C4 and C17 remain partial. Zero ordinary worker/GPU
waits does not establish stable frames or accepted visual quality.

| Criterion | Classification | Evidence and limit |
| --- | --- | --- |
| C1 Adaptive desired hierarchy | VERIFIED | Deterministic selector, hysteresis, footprint demand and 2:1 closure; core tests and native desired covers up to 256 patches. Error is a heuristic. |
| C2 Drawable fallback hierarchy | VERIFIED | Ancestors remain drawable while children are unavailable; final 12-slot native pressure retains a 10-patch cover and retreats to one root. |
| C3 Mixed-LOD topology | VERIFIED | Canonical boundaries, actual cross-face/corner GPU tests and intermediate morph fractions pass explicit tolerances. Finite tested fixtures only. |
| C4 Asynchronous refinement | PARTIAL | No ordinary worker join/GPU wait; injected delay increases convergence time. CPU publication still stalls frames; see C15. |
| C5 Bounded scheduling | VERIFIED | Bounded jobs/completions, CPU capacity, upload/publication/transition caps; constrained CPU quartet and GPU frontier regressions pass. |
| C6 Priority behavior | VERIFIED | Error, approach and high-speed factors are observable and tested; moving offscreen traces record priorities. No per-job native hitch attribution is claimed. |
| C7 Refinement debt | VERIFIED | Area-normalized projected-error debt rises under pressure and reaches zero after normal convergence/retreat. Insufficient capacity intentionally leaves debt. |
| C8 CPU tile cache | VERIFIED | Exact key reuse, eviction invalidation and evicted merge-parent restoration tested; normal route needs 340 completed builds plus the seeded root. |
| C9 GPU residency cache | VERIFIED | Full keys, generations and safe completion watermarks; stale/key-reuse/authority/shrink regressions. Final cold and warm C-stage reuploads are zero. |
| C10 Upload budget | VERIFIED | Current native pressure byte maximum is 39,200, exactly its cap; offscreen trace verifies one tile per frame. Native frame rows do not expose tile count. |
| C11 Cancellation | VERIFIED | Before-start/during-delay cancellation and stale-result rejection tested and exercised. Already executing canonical builds remain finite, noninterruptible work. |
| C12 Backtracking reuse | VERIFIED | Normal A→B→C→B→A retains all 341 exact GPU keys at return B without additional completed builds/uploads; final pressure retreat separately recovers. |
| C13 Simultaneous transitions | VERIFIED | Independent local boundary versions and disjoint closures tested; native normal runs observe up to three active morphs with cap four. |
| C14 Precision | VERIFIED | Actual world/GPU fixtures, mixed faces/corners and common 1e13 m translation pass established physical tolerances. |
| C15 Frame stability | FAILED | Normal native wall intervals reach 249–336 ms, advance up to 286.96 ms and publication up to 208.84 ms. |
| C16 Resource observability | VERIFIED | Separate payload, shared references, resident/pinned/capacity buffers, boundaries, worker stacks, transfers, debt and pressure reasons. Accounting is not RSS/VRAM. |
| C17 Old path preserved | PARTIAL | Default path and toggle restoration pass tests; final Earth AI capture is operational but quality-pending. Original-Moon native capture was badly framed and camera-arrival retry timed out. |

## Architecture changes

The fixture is opt-in through `gpu_regional`; the default remains disabled.
**IMPLEMENTED:** `RegionalTerrain` owns deterministic projected-error selection,
split/merge hysteresis, 2:1 balance, priorities, bounded workers/completions,
immutable CPU payload caching and upload admission. The app owns local drawable
publication, morph closures and dependency pins. The renderer owns GPU content
keys, generations and submission-safe replacement.

Projected error combines spherical grid sagitta and an authored relief proxy with
predicted distance to the patch footprint. Prediction is 0.15 s; the high-speed
threshold is 500 m/s. This is not a certificate against the complete procedural
world. Exact stable views reuse selection after a fixed point; per-pass score and
ancestry memoization avoid repeated work without camera quantization.

A four-child split/merge and affected neighbors form one local transition
closure. Disjoint closures morph concurrently; overlapping closures defer. Fine
odd edge samples interpolate the actual coarse edge. A canonical owner supplies
shared position, raw normal varying and material. Parent reconstruction uses the
outgoing parent's corrected indexed triangles. Endpoint buffers change with
topology; compact metadata supplies per-frame fractions.

CPU admission reserves a complete quartet when competing frontiers cannot all
fit. Retreat restores missing merge parents from cached payloads. GPU admission
accounts for peak overlap of roots, drawable tiles, morph parents and incoming
children, and admits balanced frontiers that fit. Merge restoration takes
precedence. A pool smaller than the finite domain protects admitted siblings and
excludes premature descendants. A monotonic GPU-use clock survives reconfiguration,
avoiding incorrect LRU ordering against retained warm entries.

Pins and eviction invalidation use the full current key, including authority
revision. Same-address obsolete keys cannot protect stale slots or clear current
replacement residency. Uploads within one packet protect assigned slots. Legacy
and regional slots occupy separate physical ranges. Completion callbacks avoid
ordinary-frame waits; refused replacement retains/rebases the prior cover.
Shrink preserves generation history. See
[ADR 0014](adr/0014-regional-resident-terrain-scheduling.md).

Scenario output uses buffered serialization with explicit flush/error handling,
removing earlier diagnostic I/O disruption without changing simulation timing or
accepting a failing terrain frame distribution.

## Files changed

Core additions: `crates/app/src/regional_terrain.rs`,
`crates/app/src/gravity_orbits/regional_fixture.rs`,
`crates/renderer/src/regional_edges.rs`,
`crates/renderer/src/regional_resident.rs`, focused tests, regional capture
example, `scenarios/developer/gpu-regional-route.json`, contract/report and ADR.
Integration touches the app frame path, developer protocol/snapshot/schema,
celestial renderer, resident shader/slot preparation and capture reports.
`docs/REVIEWER_CONTEXT.md` records the dated result. The preservation audit
distinguishes task edits from pre-existing dirty/untracked work; a whole-file Git
diff cannot identify only this task's edits.

## Tests

**VERIFIED, current Rust inputs:** all nine commands in
`final-after-warm-frontier/command-results.json` exit zero:

- `cargo fmt --all -- --check`
- `cargo check --locked --workspace --all-targets --all-features`
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps` — explicitly repeated after the initial log did not establish warning denial.
- `cargo test --locked -p mundaris_app --features developer-tools --test regional_runtime` — 3 passed.
- `cargo test --locked -p mundaris_app --features developer-tools --test regional_terrain` — 15 passed.
- `cargo test --locked -p mundaris_app --features developer-tools --lib gravity_orbits::regional_fixture::lifecycle_tests` — 8 passed.
- `cargo test --locked --release --workspace --all-features` — passed, 101.114 s.

The final AI check passes; its paired Earth snapshot is ready, quality-pending and
unsettled. Release workspace tests use `CARGO_TARGET_DIR=target/slice2c-validation-target`
because the pre-existing developer process locks the default release executable.
That process was preserved.

The mandatory 13-command manifest now records **13 current passes**. The remaining
six gates were refreshed against unchanged Rust inputs in
`final-after-warm-frontier/six-gates-current-inputs`: full debug workspace tests
(520.183 s), ignored long-orbits (2 passed), native-close-surface (1),
native-full-frame (1), developer-interface capture (1), and developer-scenarios
GPU replay (1). The bridge gate passes within the current release workspace run.
Explicit strict rustdoc also passes. Both batches fingerprint the same 268 Rust
inputs. Earlier logs remain historical; numerical regional-world GPU evidence
still has the narrower unchanged-renderer/test scope stated below. Quality
commands do not override failed C15 or partial native/visual acceptance.

## Measurements

Windows/Vulkan; Ryzen 7 9800X3D and Radeon RX 9070 XT. Native viewport 660×726;
offscreen viewport 640×402. RockyV5 seed 0, radius 80 km, +Z root L9 `(157,39)`,
32 cells, L9–L13. Maximum desired/drawable cover 256 patches; complete finite
domain 341 tile addresses.

Normal route: GPU capacity 384; CPU cap 512 tiles/12,544,000 payload bytes; four workers, each with
4 MiB reserved stack; queue 128; completion channel eight entries; admission up to worker
count; two uploads/1 MiB per frame; two publications/four transitions; 150 ms morph;
split/merge thresholds 0.15/0.075 px. Pressure changes GPU capacity to 12 and
uploads to one tile/39,200 bytes. Recorded configuration, not defaults, is
authoritative for each run.

### Native delay route

**MEASURED:** `native-route-13/native-route-analysis.json`; 197 capture-free
route actions/checkpoints complete, plus five initial settings, with 58 checkpoints. Each configuration fits within
the 8,192-row history. Scripted approach/lateral/retreat waypoints approximate
movement; they do not prove continuous human navigation UX. Each configuration
resets CPU state but retains GPU entries, so variants include warm reuse rather
than identical cold-cache conditions.

| Delay | Frames | Frame p50/p95/p99/max, ms | Advance max, ms | Publication max, ms | Stop→C |
| --- | ---: | --- | ---: | ---: | --- |
| 0 ms | 1,733 | 9.95 / 55.75 / 170.47 / 249.39 | 238.17 | 163.26 | 0.163 s; detail already built at B |
| 50 ms | 1,995 | 10.69 / 52.12 / 147.18 / 265.60 | 249.87 | 175.02 | 18.19 s |
| 250 ms | 3,093 | 11.66 / 45.23 / 118.47 / 258.35 | 240.61 | 166.13 | 35.45 s |
| 500 ms | 4,396 | 12.47 / 37.90 / 78.55 / 336.30 | 286.96 | 208.84 | 57.81 s |

All C checkpoints converge to 256 L13 drawable patches and zero debt. Normal
retreat A settles to four L10 patches through hysteresis. Return B retains
341/341 exact GPU keys without another completed build/upload. The counter named
`rebuilds` counts all successful completed builds, including first builds; it does
not mean repeated work. Each normal variant completes 340 builds plus the seeded
root. Pure build means are 171.6–175.5 ms; completed builds per configuration
elapsed second decline from 10.20 to 4.13/s across delay variants.

The worst 500 ms interval ends at configuration frame 3773. The preceding
frame's advance is 286.96 ms, publication 208.84 ms and resident prepare 290.35 ms.
Its latest GPU timer is 0.08476 ms from source frame 29081, not a fresh query for
that interval. CPU stages exclude later diagnostics/presentation; wall intervals
include those costs.

**Source identity limit:** full delay app hash
`f391cdaed4ef2e5ae402ee837488bd7b756ea4de98b25abd07721cd739fd6cb4`
predates final small-pool frontier/pinning/LRU repairs. The normal pool holds the
whole finite domain and does not exercise constrained admission; the publication
retry path remains in current source. These are measurements of that identified
binary, not a final-binary delay rerun or claimed final performance improvement.

### Final native pressure, cold and warm

**MEASURED:** `native-pressure-16` and `native-pressure-17`, same session 15, app
hash `fc5bc2f8afbe966c8b701252d0612d12a48aa4f0cd9c77a53cb9ae1ef1d2c063`.
Both routes complete 41 actions/checkpoints plus five initial settings. All 268 recorded source inputs match the current
build manifest. Timing routes contain no capture readback.

| Run | Frames | Frame p95/max, ms | Advance / publication max, ms | C reuploads / evictions | Retreat to root |
| --- | ---: | --- | --- | --- | --- |
| Cold 16 | 448 | 41.70 / 51.30 | 44.94 / 9.96 | 0 / 1 | 27.97 ms |
| Warm 17 | 455 | 41.59 / 44.73 | 42.04 / 1.44 | 0 / 13 | 20.29 ms |

At C both have 256 desired L13 patches and ten drawable patches (two L10, eight
L11), ready/quality-pending; debt 0.721179 projected-error px. Four balanced
frontiers need at least 15 peak slots against capacity 12. Retreat reaches one
L9 root with zero debt. Final totals: one reupload/two evictions cold, one
reupload/14 evictions warm; reconfiguration replaces old authority keys. Upload
bytes never exceed 39,200. Each run completes 48 builds, pure means 168.57/168.17
ms. These histories contain no unique GPU timing query frames. There is no
contractually established absolute 15 ms pacing target.

Preserved earlier failures: native route 13 churned 3,993 reuploads at C; warm
retry 15 accumulated 8,242 while missing a fourth sibling. These motivated final
admission/frontier pin and monotonic-clock repairs. They are superseded for cache
correctness by cold/warm 16/17, not erased or promoted to passes.

### GPU, cancellation, resources and precision

`native-gpu-samples-13` has 12 unique completed GPU source frames: terrain
0–0.17436 ms, lower-middle median 0.02492 ms. The late set has 13 unique sources,
0.053–0.09684 ms, median 0.0694 ms. First late sample age is 6,713 frames;
subsequent ages are 65–102. Request latency is separate from GPU duration.
Repeated values are not independent queries. These asynchronous measurements use
the full-delay binary; final pressure GPU cost and per-hitch attribution are not
established.

The native two-worker/500 ms cancellation probe completes: 20 requests, six jobs
started, 14 before-start cancellations, two during-work cancellations and 16
stale completions; queued/running work drains to zero. Four successful builds
total 659,845 microseconds pure build time. Discarded-build bytes/time are zero
because completed cached builds remain eligible, not because cancellation is
free. The separate offscreen probe records two before-start and two during-delay
cancellations and four stale completions. Recent worker samples may include
cancelled zero-duration entries and artificial delay; pure builder time excludes
delay. Waste counts built instances discarded before any residency acknowledgement,
not unique addresses or future lifetime waste.

Normal checkpoint maxima, bytes: CPU payload cache 8,354,500; CPU slot references
8,354,500; CPU boundaries 8,110,080; GPU resident tiles 19,846,512; GPU pinned
tiles 14,899,434; GPU tile capacity 22,349,152; boundary capacity 7,299,072.
Four worker stack reservations total 16,777,216. CPU references can share `Arc`
storage with cache payloads: do not sum them as separate allocations. GPU capacity
can retain a physical high-water mark after logical shrink. Physical RSS/VRAM
was not measured.

**MEASURED:** `worldgpu.json` covers six actual RockyV5 fixtures at 80 km,
Earth-size and 70,000 km radii, same-face and actual cross-face edges/corners,
plus common translation 1e13 m. Fractions 0, 0.25, 0.5, 0.75 and 1 pass.
Maximum GPU/f64 errors: local position 1.434e-5 m, view position 8.259e-5 m,
normal angle 2.01e-6 rad, material L2 5.55e-8. Shared-edge view difference
maximum 1.057e-4 m. Translation residual deltas zero. Limits: 1 mm, 1 mrad and
material L2 1e-5; zero violations. This verifies tested topology/reconstruction,
not every address or accepted morphology.

## Captures

`native-final-capture-summary.json` indexes session-15 PNG/snapshot/complete
pairs. **OBSERVED:** the LOD overview shows a continuous finite mixed-level cover;
the slot overview shows distinct resident patches. The close pressure image is
dominated by amber ancestor fallback, ready/quality-pending. Diagnostic coloring
is not terrain-art acceptance. Captures are separate from timing distributions.

Original-Moon capture is wrongly framed at roughly 133 AU while transitioning;
terrain is inactive. Focus/clearance retry times out waiting for camera arrival
and produces no accepted replacement. This native camera/old-Moon verification
remains partial. Current Earth AI PNG was inspected and shows the ordinary
renderer working, with `quality_pending=true`, `settled=false`.

Offscreen captures retain paired snapshots and fixed 16 ms clock semantics;
they do not measure native pacing. Candidate 14 verifies finite admission before
the final warm-clock repair. Earlier morph/LOD images have poor framing or
pending coarsening: zero positive debt alone is not settledness. Earlier human
input lease losses are preserved and were not overridden. Final pressure and
mixed-cover runs complete in the separately authorized unattended window.

## Known failures

**FAILED C15 / PARTIAL C4:** current `publish()` can build boundaries for a full
candidate cover before detecting an active neighboring morph that blocks its
closure. `advance()` retries candidates across frames. **INFERRED:** repeated
full-cover work contributes to the measured hitch; the publication timer also
includes group setup/other retries, so it does not isolate that cause. This is a
contained scheduler/publication issue requiring focused work and a fresh normal
native delay benchmark before acceptance.

Finite domain, heuristic error, fixed prototype budgets and noninterruptible
canonical CPU builds remain limitations. Final normal-delay GPU/performance was
not repeated after final small-pool repairs. Native original-Moon camera arrival
is unverified. Physical memory, continuous human camera UX and terrain-art
acceptance are untested by these checks. Existing user-rejected global terrain
and camera criteria remain open.

## V4 forward-compatibility

Read the supplied [Terra Firmer transcript](../transcript.whisper.reviewed.timestamped.md)
at 14:14–18:17, 18:44–22:50, 27:42–31:12, 31:12–35:00, 35:00–37:25 and
37:25–39:40 before applying the update. Its reusable simulation-data and
geometry/appearance separation inform the boundary; its climate payload and
library dimensions are not copied into Mundaris.

**Changed:** ADR 0014 now explicitly preserves geometric truth and LOD selection
independent of appearance, plus independent future field policies. No production API or terrain-truth change
was necessary: complete height/gradient computation precedes material adjustment,
and 2C scoring reads geometry. Existing material channels remain inherited
Slice 2A render-parity transport; they add no biome or climate decisions. Their
current combined tile key/filter is not a contract for future environment fields.

The existing shape → geological/province field → regional/local/fine residual
hierarchy preserves physical scale semantics rather than reducing terrain to an
octave stack. Canonical `SurfaceLocation` and `SurfaceGenerator::evaluate_point`
remain the query boundary; `SurfaceSample::terrain()`, `normal()` and geological
detail diagnostics supply geometry to later derivations. `SurfaceDefinition`
separates geometry/material identity; exact configuration must accompany identity
namespaces in future caches. World-space units, not render LOD or generation
order, define the complete truth. Height is radial displacement in metres and
its tangent gradient is metres per unit-direction change. Current full-workspace
tests verify material/atmosphere changes leave geometry and detail contributions
unchanged. The older footprint-filtered `TerrainGenerator` API is a separate
legacy path, not the oracle used by 2C; this update does not migrate its consumers.

**Deferred:** slope/curvature, drainage/flow, erosion/sediment/deposition,
temperature/moisture, climate rules, ecosystems and variance/statistical LOD
fields. Future field products may use independent resolutions, filters, revisions,
caches and LOD policies, sampling canonical geometry without extending today's
`TileTexel` or inheriting its material-weight normalization. Climate-driven
physical terrain change, such as glaciers, requires an explicit deterministic,
versioned generation stage; a biome label cannot add height noise. No hydrology,
climate, material system, scattering or morphology rewrite was introduced.

## Evidence

Start with `evidence-index-final.json`, `preservation-audit-final.json`,
`final-inputs.json`, `final-status.txt`, session descriptors/build manifests,
`native-route-13/native-route-analysis.json`, final pressure analyses,
`native-final-capture-summary.json`, `worldgpu.json`, and
`final-after-warm-frontier/mandatory-13-matrix.json`. Raw results, checkpoints,
traces, GPU source IDs and failed/intermediate logs remain beside the indices.
Documentation was finalized after native builds; Rust hashes independently
identify the binaries. `native-stop-15.json` records shutdown of only the
task-owned final session.

Reproduction uses the recorded native session/build manifest and the developer
CLI's `--registry <registry> --session <session-id> scenario @<scenario.json>
<output-directory>`. The timing inputs are `native-capture-free.json` for all
delays and `native-pressure-only.json` for final cold/warm pressure; run the latter
twice in the same fresh session to preserve the warm-reconfiguration condition.
`native-pressure-visual.json` is a separate capture route. The checked-in
`scenarios/developer/gpu-regional-route.json` retains the larger capture route;
its readbacks must not be mixed into capture-free timing distributions.

## Git state

No commit or push. Initial manifest contains 5,156 paths. Final audit checks
missing/unexpectedly changed pre-existing inputs separately from 17 expected
existing-path task edits and new task files. Full path lists and status are in
the audit; all dirty files must not be attributed to this task. The supplied
transcript is a separately changed existing input: its current hash differs from
the initial manifest, and this task did not edit it. The final audit distinguishes
that input change from the 17 task edits and any unclassified changes.

## Reviewer follow-up

Inspect current source and underlying timing/capture/precision evidence. Resolve
the contained publication retry cost, rerun the normal native delay benchmark on
exact resulting inputs, and complete native old-Moon verification before
acceptance. This handoff does not authorize successor implementation or whole-
planet streaming. Work stops at Slice 2C.

**HOLD IN SLICE 2C**
