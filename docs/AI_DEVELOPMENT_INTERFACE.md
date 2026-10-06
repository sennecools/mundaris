# AI-assisted development and diagnostics

This guide describes the Phase 5.14 developer interface and a practical
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

## Canonical JSON snapshot, schema 5

The Rust contract is `mundaris_app::developer_snapshot::DeveloperSnapshot` in
`crates/app/src/developer_snapshot.rs`; schema version is currently `5`. JSON has
these top-level members:

| Member | Fields and interpretation |
| --- | --- |
| `schema_version` | Integer contract version (`5`), independent of world/persistence formats. Schema 1–4 snapshots remain distinct; consumers should branch on this version. |
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
| `development` | Optional schema-5 live-session provenance: `session_id`, `observation_age_ms`, `stale`, `drawable`, `command_sequence`, `prepared_frame`, optional `submitted_frame`, `presentation_requested`, optional `capture_id`, `current_errors`, `asynchronous_measurement_source`, and optional `gpu_measurement_source_frame`. Absent from ordinary fixture snapshots and older JSON. |

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
debug-string enum names. The current Rust model uses defaults for additive optional
records and fields and preserves the version value when deserializing. The
`developer_interface` test checks current-schema round-trip and one schema-1-shaped
value with `camera.navigation` omitted; that is compatibility evidence for those
cases, not a migration or validation of every historical schema-1-through-4 file.
Older snapshots do not gain live provenance retroactively. Camera mode uses stable
snake_case values `system_orbit`, `body_orbit`, `free_flight`, and
`surface_inspection`; terrain render
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

## Live native observe–act–check interface

The optional `developer-tools` feature enables the app-owned loopback service,
the `mundaris_dev` CLI, native surface capture, bounded profiling and the scenario
runner. It implies `terrain-capture` and `surface-profile`; it does not expose the
service in ordinary builds. The native service is available for the Gravity Orbits
app path (Solar System, real Solar System, and gravity-orbits presets), and is
started only when `--dev-interface` is present:

```powershell
cargo run --locked --release -p mundaris_app --features developer-tools -- --solar-system --dev-interface
```

The service binds only to `127.0.0.1` on an ephemeral port. It writes a session
descriptor into `MUNDARIS_DEV_REGISTRY` (default
`target/developer-sessions`) and evidence beneath `MUNDARIS_DEV_OUTPUT` (default
`target/developer-evidence`). A descriptor carries protocol version, session ID,
endpoint, PID, preset, executable path, output directory, executable SHA-256 and
an optional build manifest. The manifest is accepted only when its binary hash
matches the running executable. Inspecting a descriptor is not proof that the
process remains alive; `mundaris_dev sessions` discovers registry entries by
making a session-bound live capabilities request. Operations fail on no live
session, and callers must select `--session <id>` if discovery finds multiple.

Build the bridge binary once when using the CLI or MCP server:

```powershell
cargo build --locked --release -p mundaris_app --bin mundaris_dev --features developer-tools
$dev = "target/release/mundaris_dev.exe"
& $dev --registry target/developer-sessions sessions
& $dev --registry target/developer-sessions inspect
& $dev --registry target/developer-sessions launch solar-system target/developer-owned/first
```

Global `--registry` and `--session` options can be supplied with CLI commands.
The `launch` command builds the release app with `developer-tools`, snapshots
source fingerprints before and after the build, copies an immutable executable,
and starts a marked owned session. A manually launched app can be inspected and
controlled, but it is not eligible for `rebuild-replay` unless it carries the
launcher ownership marker.
On Windows, invoke owned launch/rebuild from an interactive terminal or redirect their streams to real files. A parent that captures pipe output and waits for EOF may keep waiting while the launched native process remains alive; the lifecycle response is already written. MCP consumes framed responses without waiting for process EOF.

The CLI returns one JSON value per invocation. `inspect` includes the descriptor,
snapshot, body inventory, control owner and source-attribution availability.
`diagnostics terrain`, `diagnostics performance`, and `diagnostics errors` return
bounded diagnostic views; `events [AFTER_SEQUENCE]` returns bounded recent events
and a `history_lost` flag; `receipt <COMMAND_ID>` checks retained command status.
The event and receipt history is capped at 1024 items, and the accepted command
queue at 64 requests. The app owner applies at most 16 requests per turn.

Mutations require an exclusive control lease. `control acquire [OWNER]` returns a
lease token with a 30-second lifetime; renew it during longer work and release it
when done. Only the lease holder can submit typed `DevCommand` actions or request
native capture. Human stop/release and lease expiry interrupt automated navigation
and pending capture. For example, after acquiring a lease and copying its returned
token into `$lease`:

```powershell
& $dev --registry target/developer-sessions action $lease '{"action":"focus","body":"<opaque-body-handle>"}'
& $dev --registry target/developer-sessions receipt <command-id>
& $dev --registry target/developer-sessions wait --predicate '{"path":"terrain.settled","equals":true}' --timeout 30
& $dev --registry target/developer-sessions capture $lease earth-check
& $dev --registry target/developer-sessions control release $lease
```

Action submission normally returns `accepted` with a `command_id`; check its
receipt for `applied` or `failed` and, after an observation turn, prepared/submitted
frame identity. CLI `wait` can poll a receipt or a bounded dot-path predicate from
the snapshot, for up to 30 seconds. MCP clients run
`mundaris_dev mcp` over stdio, send JSON-RPC `initialize`, then
`notifications/initialized`; the server negotiates protocol `2025-11-25`. Its
named tools include `mundaris_sessions`, `mundaris_capabilities`, `mundaris_inspect`, `mundaris_diagnostics`,
`mundaris_control`, `mundaris_action`, `mundaris_receipt`, `mundaris_wait`,
`mundaris_events`, `mundaris_capture`, `mundaris_scenarios`, and
`mundaris_ownedlifecycle`. MCP tool results provide `structuredContent`, text
content and `isError`; capture additionally returns full-client and viewport PNG
image blocks. Arguments are checked against the advertised tool schema before
engine operations. MCP wait uses `timeout_s` and a predicate such as
`{"path":"general.frame_number","at_least":3}`; unknown fields, invalid types,
and inverted numeric bounds fail immediately. Wait supports cancellation
notifications. Do not parse human log
output in place of the structured response.

The inspected body inventory gives each body a session/world-scoped opaque handle,
plus optional semantic identity, name, mass, radius, system-inertial position and
velocity, surface availability, state reference frame, and supported operations.
Use a handle only with the session that supplied it; world replacement invalidates
it. Scenario actions resolve body names or semantic identities against the current
inventory, then submit the opaque handle. A snapshot's selected/focused body
association is still local to that snapshot and is not a long-term object ID.

On `mundaris_inspect`, `development.stale` is recomputed from current drawability,
observation age (over 250 ms is stale), world revision and command sequence. Read
`drawable`, `observation_age_ms`, `command_sequence`, `prepared_frame`, and
`submitted_frame` together. `presentation_requested` means the renderer requested
surface presentation;
it does not prove monitor presentation. Native GPU timing is asynchronous and its
source frame is reported separately. If `development` is absent, that snapshot
does not carry live-session freshness metadata; if `stale=true`, wait for a newer
observation before treating it as the result of the last action.

A native `capture <LEASE> <NAME>` is an on-demand copy of the rendered native
surface after scene and UI drawing, not a synthetic offscreen frame. It requires a
drawable window. The service checks capture/submission identity and publishes an
immutable per-command directory containing `client.png`, `viewport.png`,
`snapshot.json`, and `complete.json`; it refuses to overwrite an existing bundle.
The receipt and manifest identify capture ID, source submission frame, prepared
frame, world revision, dimensions, and evidence paths. Capture may fail with
`not_drawable`, `capture_timeout`, `capture_identity_mismatch`, or publication
errors. A minimize/occlusion/resize boundary should be treated as a capture
failure or a reason to request a new paired capture, not evidence of the prior
frame's appearance.

### Replayable scenario files

Scenario JSON uses its own `schema: 1`, separate from snapshot schema. It names a
preset, may define initial settings and a `deterministic` declaration, and contains
1–256 ordered steps. There may be up to 64 initial settings; each step has exactly
one of `action`, `wait`, `capture`, or `checkpoint`. Action steps may include a
wall-time duration of up to 60 seconds. Wait/checkpoint
predicates address snapshot fields with `equals`, `approximately` plus tolerance,
`at_least`, or `at_most`, with a timeout up to 300 seconds. Names are restricted
to ASCII letters, digits, `_`, and `-`; artifact names must be unique. A minimal
example is:

```json
{
  "schema": 1,
  "preset": "solar-system",
  "deterministic": true,
  "initial_settings": [],
  "steps": [
    {"action": {"action": "focus", "body": "solar:earth"}},
    {"wait": {"predicate": {"path": "camera.reference_frame", "equals": "body_translating"}, "timeout_s": 30}},
    {"checkpoint": {"name": "earth-focus", "predicate": {"path": "general.focused_body.name", "equals": "Earth"}}},
    {"capture": {"name": "earth-view"}}
  ]
}
```

Run a scenario against one live session with `mundaris_dev --session <id>
scenario @scenario.json <fresh-output-directory>`. Add `--offscreen` to run it
without a native session. Offscreen mode requires `deterministic: true` and uses
fixed 60 Hz steps for declared deterministic fixtures. It does not exercise
the native window, UI presentation, OS events, or the native surface-copy path.
Native mode is wall-paced and reports `deterministic: false`; it waits for fresh
post-action observations and captures native paired evidence. Do not compare their
timing or acceptance semantics as if they were the same host.

Scenario outputs retain `scenario.json`, incremental `progress.json`,
`scenario-result.json`, and checkpoint snapshots. Offscreen named capture pairs
are written into that scenario directory. Native capture pairs are published
under the live session's `MUNDARIS_DEV_OUTPUT` directory; the scenario result
records their receipt and paths. Results record scenario SHA-256, host mode, clock
mode, preset and session provenance. Use
`rebuild-replay @scenario.json <fresh-output-directory>` only with a session owned
by the developer launcher: its ownership marker must match session ID, PID and
binary hash. It gracefully stops and confirms exit of only the marked process,
builds `mundaris_app` in locked release `developer-tools` mode, records source
fingerprints and build logs, copies and hashes an immutable executable, launches the same preset from that
copy, reruns the scenario, and compares named semantic checkpoint values. The
replay manifest records before/after session IDs, PIDs and binary hashes. This is
a rebuild/replay workflow; it does not establish native visual acceptance or
deterministic wall-clock behavior.

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

The schema 5 terrain section may include `generator_algorithm`, `certificate_kind`,
`refinement_demand_kind` (`projected_sample_spacing` or `certified_error`), and
`target_certifiable`. These distinguish the selected authority, certificate,
refinement signal and whether the global target is actually certified. Projected
sample spacing is only a demand guide: it does not promote geometry to certified
quality. The first post-fix `moon-orbit` offscreen pair is under
`target/moon-lod-fix/20261006/`; it remains coarse at ready radial LOD 1 versus
desired LOD 7 and reports `target_certifiable=false`. The earlier
`ai-check-v2` pair records the previous LOD 1 versus 30 behavior. Inspect each
paired image and JSON together. See the
[native Moon integration handoff](NATIVE_MOON_INTEGRATION_REPORT.md).

The pre-fix live capture remains under
`native/captures/12764-1791251339393283700-4-moon-orbit/`. A later 1 km sampling
lease was interrupted at 20/100 samples; the script stopped before requesting a
capture, and no capture was requested after interruption. Preliminary cadence values
are not GPU-FPS measurements; see the dated handoff for the exact fixture and limits.

The later live Moon capture is retained under
`native/captures/12764-1791251339393283700-4-moon-orbit/` (`viewport.png`,
`snapshot.json`, `complete.json`). It observed source radial LOD 16, ready LOD 17,
desired LOD 30, `quality_pending=true`, and budget constraint at 327,278 m clearance.
The user rejected the live visual result and reported performance around 10 FPS.
An approach to 1,000 m clearance was reached through a developer command; that
capture was cancelled by `human_input`, which the tool respected, and was not
retried after the user's subsequent steering. The paired offscreen `ai-check-v2`
result and this live observation have different fixtures and readiness states.
Consult the handoff before comparing them or drawing FPS conclusions.

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

Phase 5.14 adds headless `developer_bridge`/`developer_commands` coverage and an
explicitly selected ignored `developer_scenarios` GPU test. The latter runs all
four declared deterministic fixtures twice, comparing semantic checkpoints and
decoded image pixels. The full validation script includes these checks. Separate
native checks can use `scripts/check-developer-native.py --session <descriptor>
--output <fresh-directory>` and `scripts/check-developer-mcp.py --session
<descriptor> --registry <registry> --output <fresh-directory>` after building
`mundaris_dev` with `developer-tools`. The native script requires Windows/Pillow;
it uses the CLI client and window messages against that explicitly selected app.
The MCP script exercises the real stdio adapter and image results. Neither script
starts or replaces the selected application. Measurements are available through
`scripts/measure-developer-interface.py --binary <app-executable> --output
<fresh-directory>`; this script launches and closes only its own comparison apps.

In a native `--dev-interface` session, GPU timestamp readback is requested by the
`performance` diagnostic operation instead of running continuously while the
interface is idle. The immediate response retains the previous completed value
and its source frame; inspect a subsequent fresh observation for the requested
measurement. Unsupported timestamp queries remain unavailable. Ordinary launches
retain their existing profiling behavior. Native image readback starts only for
an explicit capture request; observation and inventory do not sample procedural
terrain fields. `check-developer-presets.py` separately checks ordinary launch
discovery isolation and attachment/capture for real-scale Solar System and gravity
fixtures without a build manifest.

## Resident terrain transition fixture

Slice 2A/2B adds opt-in typed fixture operations to the same session interface.
Select a body, then enable `gpu_tile` to publish its temporary generated definition
and build the fixed parent. `gpu_hierarchy` accepts `enabled`, `refine`,
`morph_duration_ms` (0–10000), four `child_delays_ms` (0–5000 each),
`request_mask` (0–15), `cancel_pending`, and `diagnostic_validate`. The default
duration is 150 ms. Delays run on workers; cancellation rejects old request epochs
while their bounded CPU work may finish. Reversing `refine` changes the existing
morph target while retaining its resident endpoints.

The optional `resident_hierarchy` snapshot records parent/child keys and slots,
publication generations, independent CPU/GPU readiness, actual submitted morph
fraction, desired fraction, upload bytes, worker/build diagnostics, resource
capacities, and explicit GPU validation residuals. Pending-work frame intervals
identify whether elapsed time came from the native wall clock or the controlled
offscreen clock. Their first interval after configuration is excluded because it
can precede the request. Capture-free native measurements are required to separate
ordinary delivery from screenshot/diagnostic readback.

`gpu_tile_view` mode 0 is lit; modes 1–5 inspect height, normals, materials, UVs,
and grid lines. Presentation changes preserve terrain content identity. Explicit
`diagnostic_validate` checks the five patches over successive submitted frames;
ordinary transitions do not request geometry readback. The fixed native scenario
`scenarios/developer/gpu-hierarchy-transition.json` uses wall-paced asynchronous
readiness and is marked non-deterministic in time. The
`resident_hierarchy_capture` example separately controls exact endpoints and
midpoints and writes paired PNG/JSON. This fixture does not activate the planetary
adaptive selector. Disabling `gpu_tile` restores original authority and camera;
see [ADR 0013](adr/0013-fixed-resident-terrain-hierarchy.md).

Slice 2C adds `gpu_regional` after an active `gpu_tile` root. It controls regional
depth, CPU cache entries, GPU slots, worker count/delay, tile and byte upload
admission, publication groups, simultaneous transitions, morph duration and
split/merge projected-error thresholds. These are developer fixture parameters;
they do not enter world identity. `resident_regional` reports the desired and
drawn covers, refinement debt, CPU jobs/cache, upload backlog, physical slot
generations/pins/in-flight state, local transitions and distinct pressure reasons.
Changing `gpu_tile_view` during this fixture does not request geometry readback.
Regional modes 6–10 display LOD, slot identity, morph fraction, parent dependency,
and retained-cover fallback. These modes require the regional fixture to be active.
Native elapsed intervals remain separate from fixed-step offscreen timing.

The regional path keeps resident ancestors while requested detail is unavailable.
The route in `scenarios/developer/gpu-regional-route.json` exercises movement,
0/50/250/500 ms worker delays and constrained capacity. The
`regional_terrain_capture` example saves paired debug images and frame traces.
Neither fixture activates whole-planet streaming. See
[ADR 0014](adr/0014-regional-resident-terrain-scheduling.md) and the
[Slice 2C contract](PLANET_TERRAIN_SLICE_2C.md).
