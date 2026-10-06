# Phase 5.14 — AI engine development interface



Date: 2026-10-05. Evidence is from a dirty working tree on `main`, baseline

`ef40ed3c81c2a4b66f3cd359c508c8944cc5183d`. This report indexes evidence; it does

not supersede source, raw results, or the user's acceptance criteria.



## Goal



Implement the approved [Phase 5.14 contract](../MUNDARIS_PHASE_5_14_AI_ENGINE_DEVELOPMENT_INTERFACE.md):

typed control and observation of an opt-in native session, actual surface captures,

CLI/MCP clients, common native/offscreen scenarios, and owned rebuild/replay.

Terrain algorithms, rendering appearance, human camera UX, simulation rules, and

existing acceptance thresholds remain outside this phase.



## Result



**PARTIAL acceptance.** The interface is IMPLEMENTED and substantial Windows native,

protocol, scenario, repeatability, and lifecycle operation has been demonstrated.

The complete Windows quality matrix passes all 13 checks. Actual Windows native,
MCP, four native scenarios, deterministic offscreen repeats, and rebuild/replay
checks pass. Linux native and physical adapter/surface/device-fault coverage remain
open. The final verification record and its limits appear below.

Code existence and passing tests are not a claim that every phase acceptance gate

has been demonstrated. Terrain Acceptance A, visual approval, human navigation

acceptance, and unrelated performance gates remain open.



## Architecture changes



- `developer-tools` is opt-in; `--dev-interface` starts an ephemeral loopback service

  only for the three supported application presets. Normal launch has no endpoint.

  Socket workers enqueue bounded typed requests; the native event-loop owner applies

  at most 16 per turn. Capacity is 64, retained history 1,024, lease duration 30 s.

- `DevCommand` maps to production application operations, including relevant UI

  controls and existing public method wrappers. Inventory returns opaque handles

  scoped to session/world lifetime. Projection rebuilds preserve the namespace;

  world replacement invalidates it. No arbitrary field/shell/authoring operation is

  exposed.

- Inspectors are read-only until acquiring the single controlling lease. Navigation

  input, UI mutation, expiry, cancellation, and the visible stop control interrupt

  automation. Terminal failures/cancellation cannot be overwritten by a late result.

- Schema 5 preserves schema 1–4 interpretation and adds session, drawable/freshness,

  command sequence, frame identities, errors, and asynchronous measurement source

  information. Prepared frames, GPU submissions, and presentation requests differ;

  presentation request does not prove monitor presentation.

- Native capture copies the acquired surface after scene and egui composition in

  the same submission. It checks COPY_SRC/format and allocation sizes, uses one

  asynchronous readback slot with a 64 MiB limit, and publishes PNGs/snapshot plus a

  completion manifest off the render path. Full image and viewport crop use the same

  pixels. Publication refuses replacement; cancelled partial evidence is retained.

- Native development sessions request GPU timestamp measurements on demand through

  performance diagnostics. Idle inspection does not initiate timestamp/image

  readback or procedural point sampling. Ordinary profiling behavior is preserved.

- `mundaris_dev` is a CLI and thin stdio MCP adapter using the same client/service.

  The adapter negotiates MCP 2025-11-25, returns structured tool results and capture

  image content, and starts without launching an application. Project registration

  preserves the existing primary/child settings and the application's default binary.

- One scenario executor uses host adapters around production update/preparation,

  camera, simulation, terrain, commands and snapshots. Native uses wall time and

  ordinary workers; offscreen uses fixed 60 Hz steps and serial operation budgets.

  Determinism is claimed only for declared offscreen fixtures.

- Owned lifecycle checks PID/session/binary ownership, gracefully confirms shutdown,

  performs a fixed locked release build, fingerprints inputs, copies an immutable

  executable, and records its SHA-256. Output directories are atomically claimed, static

  JSON evidence refuses replacement, and only progress.json is intentionally

  mutable. Attached builds without a matching manifest

  report unavailable source attribution. Cancellation is bounded throughout waits,

  scenario progress and process operations. No unrelated process is terminated.



## Files changed



Application protocol/service/client/scenario modules and the `mundaris_dev` binary;

shared `gravity_orbits/developer.rs`, `frame_host.rs`, and `visual_controls.rs`;

native application integration and schema; renderer outcome/capture/timestamp code;

four scenario JSON files; focused integration tests; validation and native/MCP/

measurement helpers; project MCP registration; development guide, architecture,

[ADR 0008](adr/0008-development-session-interface.md), and reviewer context.



Pre-existing changes in `.gitignore`, `AGENTS.md`, `README.md`, OpenCode/Codex workflow

documentation/configuration and launcher, plus untracked `experiments/`, were

preserved. Some shared files also received task changes. No commit or push occurred.



## Tests



The original full Windows matrix is retained in

`target/phase514/full-validation/validation.json`, invoked as:



```powershell

./scripts/validate.ps1 -IncludeGpu -OutputDirectory target/phase514/full-validation

```



Twelve checks passed; warnings-denied all-feature Clippy failed on test-module

placement. The module was moved and the failure was superseded by selected quality

runs (`quality-retry`, then `quality-final`). The original failure was not replaced. The final complete matrix subsequently

passed **all 13 checks** in `target/phase514/final-validation/validation.json`:



```powershell

./scripts/validate.ps1 -IncludeGpu -OutputDirectory target/phase514/final-validation

```



This final run passed debug tests in 452.19 s, release tests in 82.55 s,

`developer_interface` GPU tests in 24.03 s, and all deterministic scenario repeats

in 31.46 s. These are validation durations, not engine performance measurements.

A subsequent test-only ambiguity check is separately indexed below.

The matrix included locked workspace all-target/all-feature checking, default and

all-feature Clippy, debug/release workspace tests, Rustdoc, ignored long-orbit tests,

bridge coverage, and explicitly selected four GPU/capture test targets. Debug tests

took 469.84 s and release tests 87.17 s in that run; these are suite durations, not

engine performance.



Later locked all-feature application debug/release runs are in `app-debug-final.log`

and `app-release-final.log`. Linux WSL Ubuntu headless workspace all-feature tests

passed in `linux-tests-final.log`, followed by application coverage in

`linux-app-final.log`. Linux uses an isolated runtime/target cache under

`/home/senne/.cache/mundaris-phase514`; dependencies were installed because the WSL

distribution initially lacked Rust and native development packages. This is local

Linux compatibility evidence, not remote CI or Linux native acceptance.



The fast check passed in `target/phase514/ai-check/validation.json`; its paired PNG

was inspected. Focused final commands include:



```powershell

cargo test --locked -p mundaris_app --all-features --lib developer

./scripts/validate.ps1 -IncludeGpu -Only format,workspace-check,clippy-all-features,clippy-default,rustdoc,developer_bridge -OutputDirectory target/phase514/final-contract-quality

cargo build --locked --release -p mundaris_app --features developer-tools --bins

```



The final output-claim fix has a focused concurrent-claim/static-overwrite test;

`cargo test --locked -p mundaris_app --features developer-tools --lib

developer_scenarios::tests` passed four tests. The final Linux focused run passed

21 developer tests; final Linux workspace all-target/all-feature Clippy passed with

`-D warnings`. Linux invocations use the repository manifest at

`/mnt/c/Users/senne/Documents/GitHub/mundaris/Cargo.toml`, the repository's `stable` toolchain selection (actual versions are logged), and

`CARGO_HOME`, `RUSTUP_HOME`, `CARGO_TARGET_DIR` under

`/home/senne/.cache/mundaris-phase514/{cargo,rustup,target}`. Raw logs identify the

compiler invocation and outputs; no Linux native window acceptance is claimed.



## Measurements



MEASURED: `target/phase514/verified-measurement-fixed3/measurement.json` uses

Windows 11, AMD Radeon RX 9070 XT/Vulkan, gameplay Solar System, release executable

SHA-256 `12326d1eeebf3c3e6e5ebadf17be03220cc090da8bfb7ef39fb352590af164c3`,

and a verified physical 1280×800 client area. Each sequential run warms up for

3 s, then records three approximately 2 s process samples. Actual window geometry

is retained before/after every sample and capture; both processes exited normally.



| Observation | Ordinary launch | Developer interface enabled, idle |

| --- | --- | --- |

| CPU time per sample, seconds | 0.2500 / 0.1250 / 0.234375 | 0.1250 / 0.218750 / 0.093750 |

| CPU as percent of one core | 12.498 / 6.249 / 11.717 | 6.250 / 10.937 / 4.687 |

| Ending working set, MiB | 296.770 / 296.809 / 296.809 | 298.051 / 298.184 / 298.191 |



One requested 1280×800 native capture completed in **201.914 ms wall time**, with

**62.5 ms process CPU** over the same interval. It includes ordinary frame work,

service polling, readback, encoding and publication; it does not isolate GPU copy

cost. Source frame 1108, world revision 11, client/crop PNGs and completion manifest

are retained. The helper invocation was:



```powershell

python scripts/measure-developer-interface.py --binary target/release/mundaris_app.exe --output target/phase514/verified-measurement-fixed3

```



These small raw samples do not establish a speedup or a causal overhead percentage.

Ordinary launches retain existing continuous timestamp profiling; enabled sessions

request it on demand. Process working set is not driver VRAM accounting. The

measurement predates the final bridge argument validation and runner output-claim

changes; its exact binary is identified above. Earlier `verified-measurement`

recorded a 2558×1408 client despite the requested size and is superseded, not used

as a comparable 1280×800 fixture. Failed geometry/helper retries remain available.



## Captures



Windows evidence uses AMD Radeon RX 9070 XT/Vulkan. Native full-client captures show

the scene and developer UI; exact viewport crops and source-frame/revision metadata

are checked. Native resize/minimize/restore, busy/cancel, human-wheel interruption,

and stale minimized observation have been exercised. The inspected Earth capture

can remain `quality_pending`; it does not establish settled terrain or visual

acceptance. Offscreen fixtures omit native UI/OS/surface interaction and are labeled

accordingly.



## Known failures



- A later fresh-session repeat (`fresh-codex-validated.jsonl`) discovered the
  final adapter, launched and inspected an owned app, then ended with external
  service capacity exhaustion. It is not a complete fresh-session PASS; the earlier
  full successful run is separate evidence. Cleanup could not select a live session; subsequent
  independent PID checks found the application absent; `fresh-codex-validated-process-final.json` and registry
  discovery confirm no live session remains.
- Linux native window/capture evidence remains UNTESTED; Linux headless checks do

  not substitute for it.

- Real adapter rejection of COPY_SRC/unsupported surface format and forced native

  surface/device loss are not yet demonstrated on hardware. Capability/format/size

  rejection and bounded slot logic have unit coverage; minimize/restore is actual

  native evidence. This does not prove all adapter/driver fault behavior.

- Historical intermediate runs are retained: repeated mapping caused a native

  panic, an unfocused UI cancelled automation transitions, early direct Python TCP

  handling reset connections, the first fresh-Codex attempt exposed missing registry

  schema properties, and an early Linux compile overlapped incomplete module edits.

  A late test-only verification attempt also failed formatting and encountered the

  running MCP executable's Windows file lock; its clean rerun is indexed separately.

  Fixes and superseding evidence must be read separately from those failed runs.

- The first successful fresh-session run recovered from an invalid wait argument

  shape, a semantic body reference passed where a handle was required, and an expired

  lease. The wait-validation gap is fixed and the actual MCP negative check now

  rejects immediately. Invalid handles and expired ownership were correctly rejected;

  tool descriptions now explain handles, predicate syntax and lease renewal.

- Windows callers that wait for captured-pipe EOF during launch/rebuild can wait

  until the child application exits even though the CLI JSON response was written.

  Interactive invocation, real-file streams and MCP framed responses were exercised.

- Build attribution records repository compilation inputs, compiler/tool versions

  and the fixed command. It is not a hermetic build: arbitrary external compiler

  wrappers, inherited flags or global Cargo configuration are not fully fingerprinted.

- Performance measurements have a small, explicitly described sample budget. No

  unmeasured optimization or universal overhead target is claimed.



## Evidence



All evidence is ignored under `target/`; paths below are repository-relative.

Current-phase evidence is from the dirty source/binaries identified in each manifest.



| Evidence | Result / limits |

| --- | --- |

| `target/phase514/handoff-native/result.json` | PASS: final-binary rerun of all 12 actual native checks after the full matrix, including real 30 s expiry. |
| `target/phase514/handoff-mcp/result.json`, `mcp-rpc.json` | PASS: final-binary actual stdio MCP rerun, 14 named checks; full-client image inspected. |
| `target/phase514/handoff-owned/build-manifest.json`, `handoff-stop.json` | Final build/source attribution and graceful process exit. |
| `target/phase514/verified-native/result.json` | PASS: 12 actual native checks, including malformed/oversized transport, exclusive leases, failure retention, exact PNG/crop association, publication refusal, busy/cancel/retry, requested GPU timing source, resize/minimize/stale wait/restore, wheel interruption, disconnect and 30 s expiry. |

| `target/phase514/contract-mcp/result.json`, `mcp-rpc.json` | PASS: actual stdio negotiation/schema, typed actions, native image bytes, immediate malformed wait rejection, cancellation and released ownership; 14 named checks. |

| `target/phase514/verified-cli-mcp-equivalence-sky.json` | PASS: six equivalent application state/settings comparisons across CLI and MCP. |

| `target/phase514/verified-{navigation,rendering,analytic-playback,convergence-1-2-5s}/scenario-result.json` | All four native scenarios PASS, with checkpoint/capture evidence; pending terrain remains a recorded result. |

| `target/developer-scenario-repeat/1791224918260463900/` | All four deterministic offscreen fixtures executed twice; semantic checkpoint values and decoded RGBA pixels matched. The repeat directory was produced by the final complete validation run. |

| `target/phase514/verified-replay/rebuild-replay.json` | PASS: graceful stop, fixed locked build, immutable copy, new session identity, unchanged scenario, six equivalent semantic checkpoints. |

| `target/phase514/verified-stale-handle.json`, `verified-stop.json` | Old session handle rejected after rebuild; owned replacement confirmed exited. |

| `target/phase514/verified-presets3/result.json` | PASS: ordinary launch has no registry endpoint; manually launched real-scale and gravity fixture sessions support inspect/control/capture, report source attribution unavailable, and owned-stop refuses unrelated/unowned sessions. |

| `target/phase514/fresh-codex-final.jsonl` | Fresh project session discovered MCP, explicitly launched, changed Earth settings, received native images and matching frame 13873, released/stopped and saw an empty registry. Three recoverable errors are discussed below. |

| `target/phase514/verified-measurement-fixed3/measurement.json` | Verified fixed geometry, raw enabled/ordinary process costs, one native capture cost, graceful cleanup. |

| `target/phase514/final-validation/validation.json` | PASS: complete 13-check Windows matrix against final production source. |

| `target/phase514/final-ambiguity-retry/validation.json`, `linux-final-ambiguity-test.log` | PASS: final test-only addition, format/Clippy and six bridge tests; two live endpoints reject ambiguity and honor explicit selection. |
| `target/phase514/final-ai-check-retry/validation.json` | PASS: fast developer check; paired schema-5 image inspected, quality pending. |
| `target/phase514/final-contract-quality/validation.json` | Six selected quality/protocol checks PASS after final MCP validation fix. |

| `target/phase514/linux-evidence-claim-tests.log`, `linux-evidence-claim-clippy.log` | Final Linux focused tests (21 PASS) and warnings-denied workspace all-target/all-feature Clippy PASS. |



Actual MCP helper command after rebuilding the bridge:



```powershell

python scripts/check-developer-mcp.py --session target/phase514/contract-registry/26012-1791224186233861500.json --output target/phase514/contract-mcp

```



The session was owned and visible, launched with the CLI using file streams, then

closed with `mundaris_dev stop --session 26012-1791224186233861500 --registry

target/phase514/contract-registry`; `contract-stop.json` confirms exit. This run used

app SHA-256 `7aeadb20eef192631812aabb754a2bb122015dad737175b6d4e49da27df56d96`

and bridge SHA-256 `b34ce57b3a74984a6a653a03683cb9a02c29c089defca0403fa40fe91a1df8bd`.



The native fault helper's reproduction command is:



```powershell

python scripts/check-developer-native.py --session target/phase514/verified-registry/11700-1791222491911813200.json --output target/phase514/verified-native --lease-expiry

```



The final native/MCP reruns after the complete matrix used session
`26496-1791225489036129200`, app SHA-256
`969e7aa1b0711c2fba72e82265264c3fd1b31c31b3570e16f9e121d29498bfd7`, and:

```powershell
python scripts/check-developer-mcp.py --session target/phase514/handoff-registry/26496-1791225489036129200.json --output target/phase514/handoff-mcp
python scripts/check-developer-native.py --session target/phase514/handoff-registry/26496-1791225489036129200.json --output target/phase514/handoff-native --lease-expiry
mundaris_dev stop --session 26496-1791225489036129200 --registry target/phase514/handoff-registry
```

`handoff-stop.json` confirms `process_exited=true`. Native frame 4818/revision 11
binds the full-client image, exact crop and snapshot. The full-client image returned
by the final actual MCP check was inspected: it contains the Earth scene, controls,
Automation active indicator, clouds disabled and the terrain quality-pending UI.
The offscreen fast-check image is separately paired with schema-5 snapshot,
ready LOD 1/desired LOD 14, `quality_pending=true`, `settled=false`.

Descriptors disappear after normal shutdown; replay these checks with a fresh owned

session and fresh output path. Do not replace retained evidence directories. Capture

paths remain valid after the producing process exits. Scenario/build manifests retain

actual commands, runtime capabilities, source inputs, scenario hashes and binary hashes.



## Git state



Uncommitted working tree on `main`, baseline HEAD stated above. Build manifests

retain compilation-input fingerprints, binary hashes and the observed Git state;

HEAD alone does not identify this implementation. Final status is retained with

the final evidence package. Pre-existing changes were not reset or committed.



## Reviewer follow-up



Inspect the final diff and raw evidence using [REVIEW_CHECKLIST.md](REVIEW_CHECKLIST.md),

especially actual native PNGs/paired snapshots, cancellation receipts, fresh MCP

tool discovery, source attribution and replay equivalence. Resolve the explicitly

open native/platform/fault gates before calling the entire phase accepted. This

phase does not authorize starting the next engine phase.
