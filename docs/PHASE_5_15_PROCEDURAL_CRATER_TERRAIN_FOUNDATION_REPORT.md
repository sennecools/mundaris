# Phase 5.15: Procedural crater terrain foundation

## Goal

Implement a reusable procedural crater terrain for smaller bodies, with larger
recognizable landmarks and deterministic identity/seed/radius inputs. Infinite,
varied planetary systems with multiple moons remain the long-term product direction;
system generation is outside this focused terrain slice. The Solar System Moon is
only an integration fixture. See the [phase contract](../MUNDARIS_PHASE_5_15_PROCEDURAL_CRATER_TERRAIN_FOUNDATION.md).

## Result

**PAUSED / visually rejected.** The user requested a replacement LOD/rendering
plan after rejecting the latest result. See [the redesign proposal](PLANET_TERRAIN_RENDERING_REDESIGN.md).

IMPLEMENTED: `CrateredV1`, a reusable seeded crater recipe, support-aware
regional certificates, and additional world/app coverage. OBSERVED: the pre-update
`persistent-01` rim image contains a crater bowl, but its appearance remains poor.
Its diagnostics are `quality_pending=true` and `settled=false`; this does not establish
visual quality or completion. Latest diagnostic captures were visually rejected;
the final current-source full validation matrix was not rerun before the planning pivot.
Acceptance A remains open, and user visual acceptance has not been recorded.

## Architecture changes

- World assigns `CrateredV1` generator code 3. Its immutable feature catalogue is
  derived from the terrain identity, seed, config, and reference radius. Crater
  landmarks remain full-strength at every footprint; their profile delta is zero
  across footprints. Analytic curvature/error certificates, rather than feature
  fading, charge their coarse representation and drive refinement.
- Each profile has a flat bowl floor through normalized `s = 0.36`, a quintic wall
  rising to the surrounding surface at `s = 1`, and a compact rim over `s = 0.81`
  through `1.21`. Analytic derivatives are composed with the background field.
  Seeded size ceilings descend as
  `max(64 m, max_radius / (1 + 0.25i)^1.7)`; each generated radius is at least 65%
  of its ceiling, with the configured minimum applied. The first feature uses the
  configured maximum. Centers and sizes are generated, never hand-authored.
- App recipe uses 128 features, maximum radius `min(0.13R, 24,000 m)`, minimum
  radius 64 m, depth `0.085 × crater radius`, and rim height `0.040 × crater radius`.
  The angular macro background amplitude is `min(0.0008R, 120 m)`; quieter
  separately filtered bands remain independent of the landmarks. The existing
  gameplay Moon radius and preset orbit/setup remain unchanged.
- Regional certificates exclude crater supports fully outside the queried cap and
  enclose intersecting feature contributions. Stitch and boundary bounds use the
  same regional profile bound; existing LOD/error thresholds and readiness rules
  remain unchanged.
- The user explicitly revoked the former 128 MiB terrain CPU cap. Current source
  uses a 512 MiB aggregate CPU terrain accounting ceiling with demand-allocated
  storage. Generator catalogues and query workspaces are included in accounting.
  Selector reservation lifecycle and admission-constraint reporting are still being
  corrected and require final verification; configured capacity is not measured
  operational headroom.

## Files changed

Implementation and test paths include:

- `crates/world/src/terrain/{crater.rs,generator.rs,mod.rs}` and
  `crates/world/tests/{terrain_craters.rs,terrain_generation.rs}` for the versioned
  field, analytic query/bounds behavior, memory reporting, and compatibility tests.
- `crates/app/src/solar_system.rs`, `crates/app/src/planet_terrain.rs`,
  `crates/app/src/planet_terrain/{adaptive.rs,workers.rs}`, and
  `crates/app/tests/{solar_system.rs,terrain_crater_lod.rs}` for the reusable recipe,
  regional LOD certificates, accounting, and integration coverage.
- `crates/app/src/developer_capture.rs` and
  `crates/app/examples/developer_capture.rs` for repeatable orbit, regional, and rim
  fixture captures.
- The phase contract, architecture, dated reviewer context, and this report.

Pre-existing dirty Phase 5.14 developer-interface changes and unrelated user work
were preserved.

## Tests

The current source adds six world crater integration tests in
`crates/world/tests/terrain_craters.rs`, an app LOD integration test in
`crates/app/tests/terrain_crater_lod.rs`, and generator-code compatibility coverage
in `terrain_generation.rs`. These tests cover deterministic order and shared face
queries, analytic gradients and profile joins, complete/filtered certificates,
distant-support exclusion, and coarse landmark refinement without fading.

The initial full matrix in `target/phase515/full-01/` is incomplete. Its locked
workspace check, formatting, Clippy runs, debug workspace tests, rustdoc, and long
orbit test are recorded as passing. The release workspace test and following release
native/developer tests failed to remove `target/release/mundaris_dev.exe` with
Windows `Access is denied (os error 5)`. Existing MCP bridges held that binary; no
processes were killed. These results predate the current crater/profile/accounting
updates and are not final-source validation.

### Current-source validation - incomplete, implementation paused

`target/phase515/fast-recovery/validation.json` records a passing fast developer
check, including formatting, developer tests and the paired Moon capture. Current
focused checks also passed: world library (17 tests), `terrain_craters` (6),
`terrain_profile_difference` (2), app `solar_system` (6), `terrain_crater_lod` (1),
`developer_interface` (7), and the adaptive reservation-report test (1).

The final full matrix was not rerun after the user paused this implementation.
The isolated `target/phase515-validation-cache` is available to avoid the existing
release executable lock when work resumes. Earlier `full-01` is not final-source
completion evidence.

## Measurements

No performance target, speedup, FPS improvement, or operational headroom is claimed.
The 512 MiB value is a configured aggregate ceiling, not a performance measurement.
Earlier same-frame GPU timing fields are not comparative GPU evidence. Linux/native
operator acceptance and remote CI remain untested for this phase.

## Captures

The existing fixture uses Moon seed 2, gameplay radius 109,081.7768 m, and emits
orbit, regional, and rim pairs. Its update counts are deterministic work budgets,
not elapsed simulation time or proof of native responsiveness. To reproduce with an
isolated Cargo target directory:

```powershell
$env:CARGO_TARGET_DIR = "target/phase515-validation-cache"
cargo run --locked --release -p mundaris_app --features terrain-capture,surface-profile --example developer_capture -- crater-reference target/phase515/final-reference 2
```

`target/phase515/persistent-01/` is diagnostic pre-update evidence: its manifest
records seven generated features and its snapshots report a 128 MiB cap. It shows a
crater bowl, but the appearance remains poor; orbit, regional, and rim snapshots all
remain `quality_pending=true`, `settled=false`. It predates the current 128-feature
recipe and 512 MiB cap, so it cannot verify the current recipe, resource ceiling, or
settled quality. `fast-01`–`fast-03` and `reference-01`–`reference-06` are earlier
pre-fix experiments, including shallow/no-landmark and invalid-envelope cases; they
are retained as history, not current acceptance evidence.

### Latest diagnostic pairs - visual gate FAILED

`target/phase515/recovery-512/` contains the completed seed-2 orbit, regional and
rim PNG/JSON pairs and catalogue manifest for 128 features and the 512 MiB ceiling.
All three remain quality pending and unsettled. Orbit has source/desired radial
level 7/7 with an active morph; regional 11/11; rim 12/16. Regional and rim retain
2,046 source leaves under the separate 2,048-leaf complete-cover limit in
`terrain_population.rs`, while accounted memory remains around 139 MiB. They are
resource constrained by topology limits rather than exhaustion of the new RAM cap.

The regional console reports approximately 4.84 m rendered spacing. Its snapshot
records 24,778,272 submitted upload bytes, 145,813,642 accounted retained bytes,
48.923 ms terrain preparation and 88.151 ms total CPU work for that one offscreen
frame. The console records peak aggregate reservations of 242,775,669 bytes.
These values are diagnostic observations, not native FPS, process RSS or VRAM.

The inspected regional view is still a smooth artificial bowl and the rim view
lacks convincing surface structure. The user rejected the result and requested
redesign planning. These captures establish a failure, not visual acceptance.

## Known failures and open gates

- Whole-view and close-surface quality remain unaccepted. `persistent-01` is unsettled
  and visually poor despite showing a bowl.
- Selector reservation/release and admission reporting are implemented with a
  focused report-state test. A pinned-replacement integration regression and final
  full validation remain absent; native operational headroom remains unmeasured.
- The contract describes the current 128-feature recipe; this is an experiment,
  not approval of its appearance or acceptance of the replacement architecture.
- The final current-source validation/capture matrix, Linux/native operation, remote
  CI, and user visual review are pending. Acceptance A remains open.
- No procedural planetary-system assembly is implemented by this slice.

## Git state

Branch `main`, HEAD `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`; the working tree
contains pre-existing dirty Phase 5.14 changes and uncommitted Phase 5.15 work. No
commit or push was made. Builds and captures must be tied to their recorded source
fingerprints, not HEAD alone.

## Reviewer follow-up

Inspect the current diff and final isolated-matrix artifacts before accepting the
implementation. Review deterministic and conservative bounds separately from LOD
convergence, resource admission, and appearance. The user decides whether the paired
captures meet the desired visual result; do not begin procedural system generation
as part of this phase.
