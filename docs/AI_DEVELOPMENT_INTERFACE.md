# AI-assisted development and diagnostics

This guide describes the current Phase 5.12A developer interface and a practical
workflow for using an AI assistant to investigate or change Mundaris. It is a
diagnostic/workflow guide, not a claim that visual, performance, or release
acceptance has passed. Read the [Phase 5 design](../MUNDARIS_PHASE_5_PROCEDURAL_TERRAIN_GENERATION.md),
[architecture](architecture.md), [engine invariants](engine-invariants.md), and
[coding standards](coding-standards.md) for the governing contracts. The
[engine mechanics reference](ENGINE_MECHANICS_REFERENCE.md) is the deeper source
for subsystem mechanics; this document intentionally does not duplicate it.

## A safe AI-assisted loop

1. State the goal and observable acceptance criteria. Separate a request to discuss
   an idea from authorization to edit. Ask the assistant to inspect current source,
   relevant phase requirements, tests, and dirty Git state before proposing work.
2. For implementation, specify a narrow task, exclusive file ownership for each
   editor, expected behavior, and a focused validation budget. Do not assign the
   same file to concurrent workers. Preserve unrelated local changes; request an
   isolated worktree for substantial parallel work.
3. Have the assistant follow existing APIs and ownership boundaries. World and
   simulation own truth; rendering and diagnostics are derived views. Do not broaden
   scope or change shared interfaces/dependencies without approval.
4. Run focused checks first, then the required validation matrix when authorized.
   Review the actual diff and evidence yourself. A passing test or generated image
   alone does not establish visual correctness, acceptable performance, or native
   interaction. Use [the review handoff format](REVIEW_HANDOFF.md) to report what
   was checked, what remains open, and the exact dirty-state/reproduction context.

## The current-frame developer panel

The native app's developer panel shows scene selection/focus, camera and clearance,
simulation state, terrain status, rendering controls, and performance diagnostics.
Camera controls are named **Surface Navigation** and **Advanced Free Flight**;
the flight speed control is a dimensionless user multiplier, not wheel input.
When copied controller diagnostics are available, the panel reports base speed/source,
user and boost multipliers, effective m/s, and the active safeguard. The separate
**Approach (debug)** control remains the explicit clearance-target tool, not ordinary
navigation. Navigation movement uses tangent WASD, local-up Q/E, and wheel-forward
input; right-drag controls look direction.
The active terrain body is reported independently of camera focus and selected
body; these identities can differ. Radial LOD values describe the camera-radial
chain, not whole-view quality. `source_radial_lod` is the currently contributing
source, `ready_radial_lod` the ready level, and `desired_radial_lod` the requested
target. Leaf counts likewise describe source/visible leaves, not a perceptual
quality score.

Terrain status is derived from recorded flags, not an idle worker queue. Its
vocabulary is `Not active`, `Resource constrained`, `Preparing`, `Transitioning`,
`Refining`, `Settled`, or `Updating`, in that priority order. `quality_pending=true`
means quality is still pending and must never be described as settled. `ready`
means a ready cover exists; it does not by itself mean desired quality is reached.
`settled=true` should be claimed only when the snapshot says so, and remains distinct
from human visual acceptance.

The **Export current frame JSON** action writes
`target/mundaris-diagnostics/frame.json`. It describes the current prepared frame,
but overwrites that scratch path on later exports: copy it into a unique run/evidence
directory immediately if it must be retained. For a paired, non-overwriting PNG and
JSON capture, use the capture command below instead.

## Canonical JSON snapshot, schema 4

The Rust contract is `mundaris_app::developer_snapshot::DeveloperSnapshot` in
`crates/app/src/developer_snapshot.rs`; schema version is currently `4`. JSON has
these top-level members:

| Member | Fields and interpretation |
| --- | --- |
| `schema_version` | Integer contract version (`4`), independent of world/persistence formats. Schema 1/2/3 snapshots remain distinct; consumers should branch on this version. |
| `general` | `frame_number`, `simulation_time_s`, `world_revision`, `selected_body`, `focused_body`, `camera_mode`, `paused`, `simulation_speed`. |
| `camera` | Source-frame `position_m` (3-vector), `orientation_xyzw` (quaternion), semantic `reference_frame`, optional `frame_body` and `reference_body`, optional `body_distance_m`, `reference_altitude_m`, `terrain_clearance_m`, `drawn_mesh_clearance_m`, plus `fov_y_degrees`, `near_plane_m`, `viewport_origin_pixels`, `viewport_size_pixels`, and optional observational `navigation` diagnostics. |
| `terrain` | Optional `active_body`; optional `source_radial_lod`, `ready_radial_lod`, `desired_radial_lod`; `source_leaf_count`, `visible_leaf_count`; optional `quality_pending`/`settled`; `ready`, `active_morph`, optional `morph_fraction`, `construction_pending`, `budget_constrained`, `transition_deferred`. |
| `work` | `worker_count`, `worker_busy_count`, `pending_requests`, `raw_resident_patches`. |
| `memory` | `used_bytes`, `cap_bytes`, `headroom_bytes`. Accounted CPU terrain bytes plus reservations, not process RSS or GPU VRAM. |
| `rendering` | `terrain_render_mode`, `terrain_enabled`, `ocean_enabled`, `clouds_enabled`, `atmosphere_enabled`, `patch_borders_enabled`, `lod_colors_enabled`, `navigation_markers_enabled`; `ocean_drawn`, `clouds_drawn`, `atmosphere_drawn` for this frame. |
| `performance` | Optional millisecond values `frame_cpu_ms`, `update_ms`, `terrain_update_ms`, `preparation_ms`, `terrain_preparation_ms`, `gpu_terrain_ms`, `gpu_transition_fallback_ms`; `gpu_timing_scope`; optional `upload_bytes`. |
| `warnings` | Objective advisory string codes (listed below), not inferred diagnoses. |
| `capture` | `null` for a regular native snapshot; otherwise scene/image names, dimensions, update/worker/step/morph fixture settings, adapter and backend. |
| `motion` | Optional mode-specific record (absent in schema 1/2). `mode` is `prescribed_analytic` or `newtonian`; `requested_time_s`, `published_time_s`, `paused`, `rate`, `sampling_status`, `latest_failure`; optional `analytic_body_count`, `solver_iterations`, `sampling_ms`, `publication_ms`, `measurement_scope`. |
| `sky` | Optional schema-4 decorative-sky record, absent in older snapshots. Versioned preset/seed, classification, inertial anchor/orientation, finite-star range, observer envelope, appearance controls, actual draw flags and separate preparation/generation/upload/GPU/residency observations. Not celestial destinations or world authority. |

Sky distances are **1e18–2e19 metres**, within a supported observer radius of
**1e14 metres** about its inertial anchor. Outside that envelope, sky draws are
unavailable (`outside_envelope=true`), not a camera clamp. Native sky resource
observations describe the latest submission and GPU observations the latest
completion; their scope strings explicitly allow an earlier frame. Captures label
same-frame observations. `generation_ms` covers renderer representation generation
(packed catalogue and background/mips), not app preset construction; it is null on
resident reuse. Upload bytes distinguish static content from the 112-byte frame
uniform. Capacities describe owned resident payloads, not transient generation,
whole-process RSS or driver VRAM. A cold/unavailable sky timestamp is null, not zero.

The current visual preset is version 2: 48,000 finite stars and a static 4096×2048
galactic background. Resource bounds permit at most 65,536 stars and that texture
size; this does not increase the terrain cap. Earlier version-1 sky captures remain
historical, and user visual acceptance is separate from recipe versioning.

Schema 3 adds `motion` without changing existing general/camera field semantics.
Canonical collection checks motion published time against the coherent world/frame
instant, and pause/rate against general fields. Sampling timing includes complete
candidate evaluation plus authoritative world commit; publication timing covers
frame publication/coherence validation, not rendering or native FPS. Statistics
refer to the last successful complete sample; failed sample timing is unavailable.
Newtonian analytic statistics/timings are null. Newtonian backlog/replay/forces and
conservation diagnostics remain restricted to that mode. Rejected nonfinite seek
values are identified in the failure string because they cannot be finite time values.

Body associations are objects `{ "index": ..., "name": ... }`, local to this
world snapshot and not persistent `BodyId`s. Camera pose coordinates are in the
named semantic source/reference frame, not absolute system coordinates and not a
serialized runtime `FrameId`. `reference_frame` is `system`, `body_fixed`, or
`body_translating`; `frame_body` identifies the body for the latter two. Camera
`camera.navigation`, when present, copies the current controller's attachment policy,
transition state, base speed and source, dimensionless user/boost multipliers,
effective speed, `requested_clearance_m`/`requested_distance_m` and target meaning,
pending forward distance, safeguard and optional minimum radial clearance,
logical-pixel angular sensitivities, wheel log scale, viewport height, terrain
query count/time (cumulative), last normalized wheel delta, and optional native
window-focus/viewport-keyboard/gesture ownership. Controller-only fixtures leave
native ownership unavailable (`null`). These are observations only: collecting a snapshot performs no
navigation or terrain queries. Speeds use m/s, distances use metres, angular
sensitivities use radians per logical pixel, and query time uses microseconds.
Camera orientation is XYZW order. Distances use metres; time uses seconds from the working
epoch; timings use milliseconds; memory and upload sizes use bytes. Missing or
unavailable measurements are JSON `null`, never a fabricated zero. Consumers
should tolerate optional values and check `schema_version` rather than relying on
debug-string enum names. Existing camera mode strings remain stable across schema
versions: `system_orbit`, `body_orbit`, `free_flight`, `surface_inspection`. Camera mode uses stable snake_case values such as
`system_orbit`, `body_orbit`, `free_flight`, `surface_inspection`; terrain render
modes include `natural`, `elevation`, `lit`, `normals`, `diffuse`, `readability`,
`slope`, `sea_mask`, and `rock_weight`.

The current objective warning codes are `terrain_memory_near_cap`,
`terrain_quality_pending`, `active_transition`, `source_below_desired`, and
`gpu_timing_unavailable`. The memory advisory appears only when accounted usage is
**strictly greater than 95%** of the configured **128 MiB** terrain CPU cap. It is
UI/diagnostic-only: it does not change admission or the cap. Warnings can coexist.

`frame_cpu_ms` represents the app's last update plus render preparation, excluding
UI, presentation, and GPU readback; it is not frame-to-photon latency or FPS.
`update_ms` and `preparation_ms` split those host stages; terrain-specific timings
are profile measurements when available. Native GPU query data is asynchronous
`latest_completed`, so it can describe an earlier frame (`gpu_timing_scope` says
which scope). Regular terrain and transition/fallback GPU timings are distinct.
The offscreen capture performs blocking readback and reports `same_frame`; do not
compare it as if it were a native asynchronous current-frame measurement.

## Fast focused check

From the repository root in PowerShell:

```powershell
./scripts/ai-check.ps1
```

This is a fast, focused check, **not** the full quality suite. By default it tests
the `earth-orbit` scene; accepted `-Scene` values are `solar-overview`, `earth-orbit`,
`earth-close`, and `moon-orbit`. For example:

```powershell
./scripts/ai-check.ps1 -Scene earth-close
```

Each run chooses a timestamped, fresh `target/ai-check/<timestamp>/` directory.
Alternatively, supply an empty destination with `-OutputDirectory`; existing
non-empty destinations are rejected. The script records initial `git status --short`
and HEAD, each command, exit code, duration and pass/fail result, command logs,
targeted test output, capture pair, `summary.md`, and `validation.json`. The HEAD is
not sufficient to identify source content when the working tree is dirty; retain
`git-status.txt` and `source-files.sha256.json` with the run. The check runs formatting check, the locked
`mundaris_app` `developer_interface` integration test, and a release offscreen
capture, then validates expected artifact/snapshot association. It does **not** run
the workspace-wide locked matrix by default. Inspect the PNG: artifact presence and
passing tests do not prove visual correctness or performance improvement.

## Explicit capture and native launch

For one fresh production-path offscreen fixture capture (requires the named Cargo
features):

```powershell
cargo run --locked --release -p mundaris_app --features terrain-capture,surface-profile --example developer_capture -- earth-orbit target/mundaris-diagnostics/<fresh-run>
```

Replace `<fresh-run>` with a new, unique directory name; captures refuse to replace
existing files. Scenes are `solar-overview`, `earth-orbit`, `earth-close`, and
`moon-orbit`; the scene argument may be `all` to write all four pairs into the
destination. The executable defaults to `earth-orbit` and
`target/mundaris-diagnostics` if arguments are omitted. Each scene produces
`<scene>.png` and `<scene>.json`, linked by capture metadata. PNG is lossless RGBA
from the same rendered frame represented by the snapshot, using the production
terrain/render preparation path and offscreen GPU readback. Fixture operation
budgets are 64 terrain updates, 16 ms per update step, 64 vertices per update, and
150 ms ordinary morph duration; these are deterministic fixture inputs, **not**
deterministic elapsed timings or a guarantee that terrain converged. Read `ready`,
`quality_pending`, `settled`, and other flags in that exact JSON before making a
quality claim.

`solar-overview` uses production navigation cross-markers so physically subpixel
bodies can be located without enlarging them. Orbit fixtures are source-centred,
sun-facing fixed poses; Earth orbit is at 800 km reference altitude, Earth close
at 10 km, and Moon orbit at one reference radius of altitude. Earth close can show
coarse/ocean-dominated presentation at this bounded early state; that is evidence
of the recorded scene, not accepted settled terrain or a morphology fix.

To launch the native application along its Solar System path:

```powershell
cargo run --locked --release -p mundaris_app --all-features -- --solar-system
```

Native interaction and visual acceptance require actually inspecting the running
application, not just successful compilation, offscreen capture, or launch. Check
the target scene and the panel's active/focus distinction, terrain status, overlays,
resize/minimize/restore behavior, and clean exit as applicable to the task. Preserve
the current-frame snapshot and a matching capture/evidence record when useful; do
not treat an offscreen fixture as proof of native UX.

Schema 3 fixtures now share default solar authoring and app motion sessions.
For individual scenes insert an optional signed fractional time argument between
scene and output directory: `earth-orbit -0.25 <fresh-directory>`.
`analytic-playback <fresh-directory>` retains four same-session pairs
at epoch, +86400 s, -86400 s and epoch again. These direct-time captures are not
native interaction; see [analytic playback](ANALYTIC_PLAYBACK.md).

## Full validation and evidence discipline

```powershell
./scripts/validate.ps1 -IncludeGpu
```

This writes a fresh `target/full-validation/<timestamp>/` package with command,
exit code, duration and result records. Omit `-IncludeGpu` on adapter-less hosts;
then explicitly report the GPU gates as untested. Native launch/operator checks
and terrain Acceptance A remain separate from this quality matrix.

To retry named failed checks into a **fresh** destination, `validate.ps1` also
accepts `-Only` (for example `-Only tests-release,native_full_frame -IncludeGpu`).
Such a selected run is not independently a full matrix; retain the original results
and explicitly identify which failures the retry supersedes.

The small script is not a substitute for the repository's locked quality commands.
Use the current [README validation guidance](../README.md) and
[CI workflow](../.github/workflows/ci.yml) as authoritative; do not copy stale phase
reports as a current validation record. For Phase 5.12A, the focused validation
matrix includes format check; locked default/all-feature checks; warnings-denied
Clippy for default and all features; locked debug and release tests; warnings-denied
Rustdoc; separately selected ignored long-orbit tests and adapter-required
`native_close_surface`, `native_full_frame`, and `developer_interface` capture
coverage; and a native application launch. Consult README/CI for exact quality
command forms and feature/target scopes. Ignored GPU/capture tests are not run by
ordinary tests and require an appropriate graphics adapter; native window evidence
is a separate runtime gate.

Do not rerun historical Phase 5.11E evidence scripts as if they validated current
source: those scripts can overwrite their old evidence directories. Make a new,
isolated evidence destination for any new measurements/captures. Report implementation
separately from verification. For visual claims inspect the paired image and record
scene, overlays, readiness/quality flags, source revision and dirty state. For
performance claims provide repeatable measured data, units, fixture, build/profile,
hardware, and raw output; a single capture timing is not a performance result.
Distinguish host CPU timings, asynchronous GPU timings, blocking capture readback,
accounted terrain memory, RSS, and actual VRAM. Record exact commands, results,
limitations, and retained evidence paths. Never claim acceptance solely from a
passing check or a `settled` flag.

This interface/workflow does not promise or implement a camera rewrite, LOD
visualization redesign, terrain morphology change, GPU-resident terrain/morph path,
or Acceptance A. Those are separate user/reviewer decisions and validation
scopes.
