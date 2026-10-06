# Phase 5.14 — AI Engine Development Interface

## Goal

Deliver the approved inspection → control → actual capture → reproduce → rebuild
and replay loop through CLI and MCP. A visible runner-owned native application is
the default; explicit attachment inspects a separately launched enabled session
without rebuilding or replacing it. The user remains the acceptance authority.

## Scope and ownership

The opt-in interface supports gameplay/real-scale Solar System and existing gravity
sessions. It exposes existing selection, focus, overview, look-at, navigation,
clearance, playback, rendering, layers and sky settings through checked application
operations. It does not expose field mutation, shell execution, body/property
authoring, terrain algorithm changes, camera redesign or visual changes.

Application code owns contracts, commands, observation publication, coherent
preparation and manifests. Renderer code owns capture and render outcomes. CLI/MCP
own protocol adaptation; scenarios and lifecycle own replay and only their launched
processes. Domain crates remain free of transport and assistant dependencies.
See [ADR 0008](docs/adr/0008-development-session-interface.md).

## Required contracts

- Tooling is compiled with `developer-tools` and activated with `--dev-interface`.
  Ordinary launches have no endpoint, discovery publication or readback.
- Ephemeral loopback TCP uses versioned UTF-8 JSON, a bounded 64-request queue and
  owner-thread application in batches of at most 16. Retained events/receipts cap
  at 1,024. No socket, PNG encoding or disk publication runs in frame processing.
- Inventory returns opaque session/world-scoped handles, names, semantic identity,
  SI properties and local state/reference frame. Stale handles are rejected;
  projection rebuilds preserve them and world replacement invalidates them.
- Inspectors coexist with one explicit renewable 30-second controlling lease.
  Human input and stop control interrupt automation. Accepted, applied, failed,
  cancelled, expired and evicted outcomes are reported without erasing failures.
- Schema 5 preserves schema 1–4 interpretation and adds session, age, freshness,
  command sequence, drawability, errors and frame provenance. Prepared frames,
  submissions and presentation requests are distinct; monitor presentation is not
  inferred from return status.
- On-demand native evidence uses the acquired surface after scene and egui, before
  the same submission. Capability/format, one-slot, checked-allocation and 64 MiB
  guards return unsupported/busy/cancel/resize/not-drawable outcomes explicitly.
  Full-client PNG and viewport crop share pixels and one snapshot/request/frame.
  Immutable evidence has a completion manifest published last.
- Detailed terrain/work/memory/performance/error inspection is requested on demand.
  Minimized queries return stale state; captures fail and waits expire honestly.
- `mundaris_dev` adapts discovery, inspection, control, receipts, events, waits,
  capture, scenarios and owned lifecycle. MCP uses the negotiated 2025-11-25 stdio
  tools contract, structured results, PNG image content and stderr logs. The
  project server configuration preserves existing development-agent settings.
- Versioned scenarios define preset, initial settings, semantic references,
  actions, finite durations, checkpoints/predicates and bounded waits. One runner
  uses production session logic for native and offscreen hosts. Native execution
  uses real time and workers; declared deterministic fixtures use fixed steps and
  serial terrain operation budgets. Pending quality is recorded, never accepted
  by relaxing a criterion.
- Four fixtures cover overview/Earth/Moon/overview navigation, rendering/sky,
  analytic pause/seek/forward/reverse/reset, and terrain observations at nominal
  1/2/5 seconds, with actual native elapsed times retained.
- Rebuild/replay gracefully closes and confirms an owned process, builds with a
  fixed locked release command, copies an immutable executable, then replays the
  unchanged scenario. It never replaces a running Windows executable or terminates
  unrelated processes. Binary/scenario SHA-256, input fingerprints, Git state and
  runtime capabilities establish attribution; unmatched attached binaries report
  attribution unavailable.

## Acceptance and evidence

Acceptance requires demonstrated operation, not implemented APIs alone:

1. A fresh Codex session discovers the MCP tools, launches an owned visible app,
   inspects bodies, focuses Earth, changes rendering and obtains a matching native
   image/snapshot bundle.
2. CLI/MCP operations agree; explicit attachment inspects/acquires control without
   rebuilding or replacing the attached process.
3. All four scenarios run native/offscreen; declared deterministic fixtures repeat
   expected semantic state and image pixels. Native checkpoint/capture association
   is checked against actual source frames.
4. Owned rebuild/replay has a new session identity, matched binary/source manifest,
   stale-handle rejection and equivalent semantic checkpoint results.
5. Focused tests exercise malformed/nonfinite input, ambiguity, handles, bounded
   queues/history, failure retention, interruption, disconnect, cancellation,
   timeout, publication failure and native lifecycle/capture outcomes.
6. Native PNGs contain scene and UI, and the crop matches the full image pixels.
   Disabled and enabled-idle paths are inspected and measured separately from
   capture requests; raw costs are retained without asserting a speedup.
7. Focused locked checks, `ai-check.ps1` paired inspection, the README/CI quality
   matrix and `validate.ps1 -IncludeGpu` pass, followed by separate native and
   actual MCP checks. Windows native evidence is required. Linux compatibility and
   headless checks remain required; unavailable Linux native evidence stays open.
8. Development guidance, architecture/ADR, reviewer context and a handoff identify
   exact commands/evidence, implementation versus verification, and every gap.

No commits or pushes are authorized by this phase request. Preserve pre-existing
Codex setup edits and untracked experiments. Native hot reload and pixel inspection
are deferred. Terrain Acceptance A, visual approval, camera UX and unrelated
platform/performance gates remain separate.
