# Phase 4 validation record — 2026-10-02

## Status

Smooth planetary surface LOD and connected inspection are implemented in
`--gravity-orbits`. The authoritative requirements remain the
[Phase 4 specification](../MUNDARIS_PHASE_4_PLANET_SURFACE_REPRESENTATION_AND_LOD.md).
[ADR 0006](adr/0006-planet-surface-topology-and-lod.md) records decisions and concrete
API choices. **Full acceptance remains open** for complete human/operator and
platform evidence and performance review misses. Phase 5 has not begun.

The complete original repository, including all tracked source, tests, benches,
shaders, manifests/lockfile, configuration and documents, plus the supplied new
Phase 4 specification, was read before source work. Existing design-document
changes were preserved. The Windows baseline had 96 runtime tests/seven compile-fail
documentation tests passing and two ordinarily ignored long runs.

## Implemented contracts and numerical evidence

| Contract | Evidence / result |
| --- | --- |
| Mapping/address | Exact six face bases, normalized radial mapping, checked level≤30 and coordinates, deterministic child order/Morton traversal, compact native address and no frame/GPU identity |
| Stable surface location | Checked body-fixed `Direction3`; no persisted rounded-direction hash or patch-dependent location |
| Adjacency/corners | Independent 24-transition table, reciprocal edge/reversal fixtures, levels 0/1/5/16/30, all eight corners and nested sample keys/positions bit-equal on this target |
| Stitching | All 16 masks: positive integer/spherical winding, exact signed area, manifold incidence, dense non-overlap coverage and diameter≤sqrt(5) grid steps; cross-face fine/coarse boundary key fixtures |
| Sphere/bounds | Unit norm≤2e-14, radius envelope≤32 EPS R; non-grid cap/ball and triangle containment, independent barycentric error checks, synthetic ±1,000 m extent margins |
| LOD/error | f64 physical-pixel error includes all stitch variants and 64 EPS R floor; H600/800/2160 and FOV30/60/90 radial projection checks; doubling physical focal size doubles finite error bounds |
| Hysteresis | Explicit upper/lower excursions split/merge, deadband retains history, stationary/tiny-oscillation 1,000-update paths have zero transitions after structural settling |
| Coverage/balance | Exact per-face area/no ancestor overlap; edge difference≤1; adversarial corner closure to level20 terminates and adds exactly 3 leaves/forced split |
| Readiness/budgets | Work0/1/32 starvation, rapid reversal and identical-schedule replay preserve complete covers; local transactions retain parents until all child/guard metadata validates |
| Culling | Five normalized content planes, grazing/near/large-ball fixtures; independent all-stitch triangle front-face horizon oracle; at/inside disables outside horizon rejection |
| Precision | Source-centred metre/centimetre fixture is invariant under 0/1.5e11/1e16 shared offsets; CPU local delta≤1e-9 m, body-frame entry/remap≤1e-7 m, basis≤1e-12 |
| GPU contract | Explicit sample32/instance64/clipped64 byte layouts/padding; matching naga validation; shared drawn boundary records bit-equal; separate body batches cannot alias boundary samples |
| Clipped fallback | Below-reference coarse-triangle interior torture fixture requires and counts fallback; clipped output≤0.05 px, finite/bounded; bad masks reject before submission |
| Handoff | One observation ID/opaque responsibility per request; ordinary fixture samples 441 matched radial directions and measures maximum exact before/after displacement **0.0370578412 px**, below 0.35 px |
| Moving body | Real hierarchy advances ten h60 ticks, translates >1e6 m and spins; fixed inspection pose/local geometry stays within body precision while independent moon state changes |
| Read-only policy | BodyId/properties/state/time/revision exact copies unchanged by selection, LOD, culling, handoff or preparation; world/simulation source unchanged |
| Tangents/navigation | Poles, near-poles, orthogonality/handedness≤1e-12 and transported continuity; local mouse look changes orientation without centre orbit, diagonal motion normalized, metre guard is navigation only |

### Representative settled counts

R6.4e6 m, 1280×800 physical content, FOV60°, near0.1 m, grid16, split0.125/
merge0.0625, radial approach history from six roots. Counts include culled guards.
Tiny views use the far app representation; six logical roots below are selector
measurements, not a claim that tiny planets upload patches.

| Clearance/view | Desired estimate | Balanced/active cover | Visible | Max level | Max bounded error px |
| --- | --- | --- | --- | --- | --- |
| 1e11 m, tiny | 6 | 6 | 5 | 0 | 0.001014 |
| 83,000 km, ~100 px | 81 | 84 | 62 | 2 | 0.097836 |
| 10,000 km, large full planet | 483 | 510 | 402 | 4 | 0.123549 |
| 1,000 km | 303 | 351 | 98 | 6 | 0.118435 |
| 100 km | 105 | 189 | 24 | 8 | 0.030150 |
| 10 km | 117 | 129 | 8 | 10 | 0.106774 |
| 1 km | 177 | 333 | 16 | 14 | 0.006821 |
| 100 m | 201 | 213 | 8 | 17 | 0.001545 |
| 10 m | 213 | 225 | 4 | 18 | 0.120223 |
| 2 m, downward | 213 | 225 | 4 | 18 | 0.120223 |

Grazing centre/edge/corner at 2 m: respectively **225/32**, **330/50**, **348/46**
cover/visible; max level18 and bounded errors≤0.123768 px. At 100 km the three
grazing visible counts are 50/92/128. No normal measured view hit the emergency
visible cap. Deep levels are influenced by cap balls crossing the near plane and
the conservative stitched/numeric bound, rather than metre terrain sampling.

The 2,048-record quota reversal test at 2250×1290 settles at 567/457, then 513/198
and 588/300 cover/visible after direction/clearance changes. The 3840×2160 downward
2 m state is 237/4, level19, error0.086742 px. Cache residency peaks at 1,654 in
that path; owned cover/scratch capacities are 159,088 bytes. A 24-keyframe lateral/
rapid/starved/reversal path settles repeatedly, reaches the bounded cache capacity,
evicts inactive metadata and returns to six roots on departure.

### Depth and precision limits

Actual adjacent f32 reverse-depth reconstruction with near0.1 m:

| View depth | Adjacent reconstructed distance difference |
| --- | --- |
| 2 m | 1.490116e-7 m |
| 10 m | 9.313225e-7 m |
| 100 m | 1.164153e-5 m |
| 1 km | 7.275956e-5 m |
| 5 km | 4.547473e-4 m |
| 1e8 m | 11.10223 m |
| 1e9 m | 69.38893 m |
| 1.5e11 m | 12,197.27 m |

Near 1 cm separations and 1 m at 5 km remain ordered. These distant errors do not
promise metre accuracy for the star. One infinite-far reverse-Z attachment is
cleared once; all opaque spheres/patches precede no-write curves and UI. No evidence
required depth partitioning. Existing root-flattening loss tests remain negative
precision evidence, not a supported local preparation path.

## Native Windows evidence

Windows11 Pro 10.0.26200, Ryzen7 9800X3D (8 logical processors), Radeon RX9070XT,
driver32.0.31041.1004, **Vulkan**, Rust1.98.1/MSVC, optimized profile. The optional
integrated route was run in a native 1280×800 client with dock-excluded content.
Window captures were inspected and remain temporary validation products.

Observed route: paused system names/guide structure; same Aurelia focus; astronomical
clearance; continuous 30 s approach; explicit 100 km/10 km/1 km/100 m/10 m/2 m
checkpoints with borders; co-rotating inspection without pose jump; lateral movement;
curved horizon; Solace observation; **twenty real h60 commits**, world revision3→23/
time0→1200 s; independent Luma observation; departure/whole-system return and repeated
approach. Wide-to-tall resize, minimize/restore and normal close executed. Stderr
was empty and the process logged clean shutdown. No duplicate opaque planet, holes,
reversed patches or conspicuous normal-view cracks were seen in these checkpoints.
The inspected near route reported finite quality≤0.0955 px and no cap constraint.

Earlier development inspection exposed duplicate widget IDs and global-cover cache
churn at a large viewport; both were corrected and regression-tested. Direct native
mouse/keyboard injection was not consistently reliable on this desktop; the route
uses the actual production command/navigation/render path. It is **directed native
evidence**, not a full human mouse/wheel feel or high-DPI/operator signoff. The repeat
approach was observed in progress before resize/close; exhaustive visual traversal
of every cube edge/corner, every backend and manual recovery remains operator work.

Reproduce the same mode:

```powershell
$env:MUNDARIS_PHASE4_VALIDATE='1'
cargo run --locked --release -p mundaris_app -- --gravity-orbits
```

Normal invocation omits that variable. UI also exposes the integrated validation
route, exact clearance targets, Surface Inspection (I), horizon (H), independent
look-at body commands, single physical step, normal/LOD/face/border/bounds views,
pointer patch address and bounded address/mask/metadata diagnostics.

## Quality and remaining acceptance

Windows locked workspace build/check/all-target Clippy, formatting, debug and
focused optimized tests, unchanged ignored orbital long runs, Rustdoc with denied
warnings and whitespace checks pass. The original suite had **120 runtime tests and
seven compile-fail documentation tests**, with two ordinarily ignored orbital tests
executed separately in release. No physics tolerance changed. The long circular
energy maximum is1.1213312e-11, eccentric3.6288931e-6 and hierarchy1.5336759e-13,
reproducing the prior envelopes.

```text
cargo build --locked --workspace
cargo check --locked --workspace --all-targets --all-features
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked -p mundaris_simulation -p mundaris_world -p mundaris_math -p mundaris_renderer -p mundaris_app --release --all-features
cargo test --locked -p mundaris_simulation --release --test orbits long_run -- --ignored --nocapture
cargo doc --locked --workspace --all-features --no-deps
git diff --check
```

Rustdoc used PowerShell `$env:RUSTDOCFLAGS='-D warnings'`.
Registered CPU benchmarks executed; [performance](performance.md#phase-4-baseline--2026-10-02)
contains actual workload counts/distributions and review misses. No project unsafe,
new crate, dependency upgrade, noise, terrain, collision or Phase 5 implementation.

Open: Linux native quality/release/visual acceptance (available WSL has no
cargo/rustc/gcc/pkg-config on its login PATH); current implementation-revision remote
CI (`gh` unavailable; nothing pushed); complete human controls/high-DPI/edge-corner/
backend/surface-recovery sequence; allocation profiler/GPU timestamp/presentation-p95
evidence; full-planet CPU review-target miss. Earlier unperformed phase acceptance
remains open in its original records. No unperformed operator work is checked off.

## Continuation verification — 2026-10-02

Recovery started at `d9707a4` with nothing staged. Math, GPU integration and bounded
ready transactions were already committed; app integration/tests/benchmark and
implementation documentation were present but uncommitted. Existing work was kept.
No completed surface mathematics or thresholds were redesigned.

Two regressions were added: zero-metadata-work rapid approach retains exactly six
ready roots, one surface observation/opaque owner, checked finite preparation and
honest pending quality; return below the far threshold still waits for readiness.
The second verifies that inspection diagnostics and horizon look use the same
transported tangent after regional reanchoring, instead of reconstructing a static
chart basis. Analytic hovered-patch diagnostics explicitly identify covering regions,
not necessarily drawn triangles.

Fresh Windows workspace debug tests (**122 runtime tests and seven compile-fail
doctests**) and the focused five-crate release suite pass; both
ignored release orbital runs reproduce the recorded energy envelopes. Build,
all-target check, warnings-denied Clippy, formatting and warnings-denied Rustdoc
were rerun. Both CPU benchmark targets were rerun with the viewer closed; the
separate [continuation measurements](performance.md#continuation-verification--2026-10-02)
reproduce the full-view CPU review miss. Earlier temporary native route logs and
2 m/horizon captures were recovered and inspected; these corroborate the earlier
directed evidence, not a new operator acceptance claim.

The current optimized Windows/Vulkan viewer also reran the integrated route through
astronomical approach, 2 m inspection, horizon, independent moon observation and
twenty h60 commits (revision3→23/time0→1200 s), then overview return, tall resize and
minimize/restore. New captures were inspected; the near horizon reports 0.0955 px
quality with no cap constraint. The app logged clean shutdown and empty stderr.
The route's PowerShell wrapper returned failure because its process exit-code field
was unavailable after closure; this is not recorded as a successful wrapper run.
A separate normal startup/close check retained the process handle and confirmed
exit0/empty stderr. Complete human controls/high-DPI/backend recovery acceptance
still remains open.

## CPU and acceptance closeout — 2026-10-02

Recovered baseline `ef9ab84`/`5a7215c`; existing `.gitignore`, agent/workflow and
script changes were preserved separately from engine work. The new
[CPU profiling record](phase-4-profiling.md) identifies the exact11.143ms workload,
existing-stage timings, all requested view classes, cold/steady differences,
capacity/dependency observations and small packing/grouping before/after changes.
No architectural or LOD-threshold change was made. The full-view review miss remains.

### Fresh directed Windows route

Optimized native Windows/Vulkan/RX9070XT viewer, initial1280×800 physical client,
**DPI1.0**, opt-in CPU profiling enabled. The integrated production route ran from
overview/focus/astronomical clearance through continuous approach and surface
responsibility, bordered100km/10km/1km/100m/10m/2m checkpoints, co-rotating inspection,
lateral movement, horizon, independent Solace/Luma observations, departure/overview
and **completed the repeated approach back to2m**. Log confirms revision3→23,
time0→1200s from twenty realh60 commits. Tall resize and minimize/restore executed;
native wrapper retained the process handle and confirmed **exit0/empty stderr**.
Captures of overview/approach, the listed altitude checkpoints, lateral/horizon,
star/moon, completed repeat and restored window were inspected. No conspicuous
holes, reversed patches or broken visible patch borders were seen in those captures.
Near quality was0.0955px, tall-restored0.1016px, no constrained refinement.

This is directed checkpoint evidence, not live human visual/control signoff. The
capture named handoff still showed far responsibility before transfer; later approach
showed surface responsibility, not an observed exact transition frame. Static
captures cannot establish absence of temporal culling flicker, LOD oscillation,
handoff pop or precision jitter. No exhaustive native edge/corner/high-orbit sweep,
additional backend, direct-control/picking feel or forced device recovery is claimed.
Moon/star captures show navigation observations near the horizon, not proof of
simultaneous opaque mesh visibility. Earlier headless precision/handoff tests remain
the quantitative evidence for those contracts.

### Native CPU/cadence observations

Build/run the optional probe:

```powershell
$env:MUNDARIS_PHASE4_VALIDATE='1'
$env:RUST_LOG='info,mundaris_app::gravity_orbits=debug,wgpu_hal=warn'
cargo run --locked --release -p mundaris_app --features surface-profile -- --gravity-orbits
```

14,228 accepted drawable samples in the directed route. Selected windows below are
offsets from the first probe sample; times are median/p95 milliseconds. App update
includes simulation/navigation/publication; preparation includes surfaces, guides,
history/observations; surface is its subset. Render includes UI/acquisition/uploads/
encoding/submission/present and possible FIFO wait, **not GPU execution duration**.
Cadence is successive CPU update-start intervals, not compositor presentation timing.

| Window | N / patches | Update | Preparation | Surface | Render/present call | Cadence |
| --- | --- | --- | --- | --- | --- | --- |
| Overview0.5–1.5s | 119 / 0 | 0.0060/0.0087 | 0.0474/0.0691 | 0/0 | 8.1196/8.4567 | 8.3392/8.7419 |
| Approach14–18s | 448 / 46–388 | 0.0064/0.0068 | 5.0482/12.5247 | 4.2190/10.1503 | 3.2038/6.4269 | 8.3699/13.5189 |
| 100km38–39s | 120 / 22 | 0.0061/0.0067 | 1.4087/1.4482 | 0.5766/0.5967 | 6.7603/7.1097 | 8.3413/8.7727 |
| 2m53–54s | 120 / 3 | 0.0061/0.0068 | 1.5285/1.5770 | 0.0805/0.0843 | 6.6039/6.9610 | 8.3237/8.6904 |
| Horizon62–64s | 240 / 40 | 0.0061/0.0068 | 2.7748/2.8373 | 1.1879/1.2086 | 5.3525/5.7329 | 8.3305/8.7276 |
| Moon72–74s | 240 / 38 | 0.0062/0.0070 | 2.8287/2.8838 | 1.2734/1.2943 | 5.3064/5.6439 | 8.3272/8.7297 |

The route is mainly paused plus twenty single steps: pump medians0.0003ms in these
windows are not active high-warp throughput. Approach max preparation15.8930ms and
cadence16.9073ms; route-wide maximum accepted interval55.1596ms near resize/repeat.
Logging/instrumentation and UI are enabled, content is dock-excluded rather than the
benchmark's1280×800, and the camera follows a tilted moving-body route. Do not compare
native patch counts as if workload matched the independent radial benchmark. No GPU
timestamp, compositor p95, or driver-allocation conclusion follows from these clocks.

### Explicit open/dispositioned gates

- High-DPI: native scale remained1.0; no global display settings changed. Manual
  non100% scaling must verify physical LOD/viewport, UI/labels, picking, handoff and
  diagnostics. Existing headless physical-resolution tests are not native DPI signoff.
- Lifecycle: directed minimize/restore passes; a new headless near-surface regression
  covers long accepted-gap cancellation, hidden duration, stable inspection pose,
  zero debt after Resume and a subsequent real commit. This is simulated host duration,
  **not actual OS sleep/resume**. OS sleep and long live-process stall remain manual.
- Linux: Ubuntu WSL2 starts, but `cargo`, `rustc`, `gcc`, `pkg-config` are absent on
  login PATH and common Cargo installation directories are absent. No Linux Cargo or
  graphical check ran; no toolchain/VM/container infrastructure was installed.
- Remote CI: `gh` unavailable; unauthenticated GitHub Actions REST request returned404.
  No authenticated current status is available. Local HEAD is eight commits ahead of
  `origin/main` at recovery, so remote-tracking history predates Phase4; no fetch/push
  or CI configuration change was made. Linux quality/Windows compatibility/current-HEAD
  pass cannot be asserted. Workflow remains Linux fmt/Clippy/tests and Windows check;
  it supplies no native visual acceptance.
- Allocations/presentation: owned-container/dependency events and CPU cadence now have
  evidence; external heap/driver/GPU profiling and compositor/GPU timing remain open.
- Architecture: **unchanged and ready to freeze**. No structural fix is required
  before later field work; expensive full-view steady sampling and cold spikes remain
  documented performance risks. Full Phase4 acceptance/Phase5 clearance is conditional
  on completing or explicitly dispositioning the remaining gates, not automatically
  granted by this profiling task. No Phase5 implementation started.

### Closeout quality evidence

Locked Windows workspace build/all-target-all-feature check, warnings-denied Clippy,
workspace debug tests and focused five-crate all-feature release tests pass. Suite
now contains124 runtime tests and seven compile-fail doctests (two ordinarily ignored
orbital long runs executed separately). New tests cover complete sample bytes,
stable mixed-mask/multi-batch ordering, boundary capacity accounting and near-surface
stall/hidden-time recovery. The new inspection lifecycle test compares orientation
within the existing1e-12 envelope because zero-duration inspection reconstructs a
normalized quaternion; no existing tolerance was changed.

Ignored optimized orbital tests retain circular energy1.1213311916402153e-11,
eccentric3.628893073864288e-6 and hierarchy1.5336758750031128e-13 maxima.
Warnings-denied Rustdoc and formatting pass. Both complete Phase4 CPU benchmark
targets execute; raw distributions are retained outside tracked documentation.
Final whitespace/UTF-8/LF/newline/local-document-link/fence checks are recorded
separately from native evidence. Phases1–3.5 suites and world/simulation algorithms
remain unchanged; no time, history, picking, navigation or precision tolerance weakened.
All12 closeout files passed strict UTF-8/LF/final-newline/trailing-whitespace checks;
the four changed Markdown documents have balanced fences and15 valid local links/
anchors. Source milestone `3c79011` contains the validated closeout implementation.

## Files and milestones

- Math: `surface.rs`, lib export, topology/tangent tests.
- Renderer: `planet_surface/{mod,topology,bounds,cache,cover,lod,prepare,gpu}.rs`,
  shader, celestial/view/lib integration, two surface tests and benchmark target.
- App: surface session/anchor module, camera/explorer/lib integration, navigation/
  paths tests and integrated CPU benchmark target.
- Documentation: this record, ADR0006, performance, README, architecture/roadmap,
  engine design and the implementation record in the authoritative Phase4 contract.

Milestones begin with `9a50c55` (math), `2adb120` (surface renderer baseline),
`d9707a4` (bounded local transactions/resources), and `5a7215c` (connected app
handoff/inspection, paths, integrated benchmark and continuation regressions).
Documentation milestone hashes are supplied in the completion report. Existing
user design changes were preserved; no commit claims complete platform acceptance.
