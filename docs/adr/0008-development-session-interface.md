# ADR 0008: application-owned development sessions

Status: accepted implementation direction for Phase 5.14.

## Context

The existing developer panel, coherent snapshots and capture fixtures expose useful
engine observations, but an external inspector cannot control a running application
or bind native pixels to the exact state that prepared them. Application authority
must remain on the native event-loop thread. Rendering, transport and development
process orchestration must not enter world or simulation crates.

## Decision

`developer-tools` and `--dev-interface` are both required for a native endpoint.
The app binds an ephemeral IPv4 loopback TCP port and publishes an ignored session
descriptor. Version 1 UTF-8 JSON requests are newline-delimited and bounded at
64 KiB. Queries, receipts and descriptors have a separate bounded response budget.
IO threads enqueue requests and wake `EventLoopProxy`; the owner applies at most
16 requests per service turn from a 64-entry queue. Receipts and recent events
retain 1,024 entries. Receipt eviction is explicit.

Inspection is read-only. A caller must acquire a renewable 30-second lease before
mutation. Native human input, stop control, cancellation and expiry interrupt
automation. Body handles bind the session, world namespace and checked identity;
clients resolve semantic identities from inventory rather than interpreting handles.
Projection rebuilds preserve identity; world replacement invalidates it.

CLI and stdio MCP are adapters over the same client and operations. MCP negotiates
the 2025-11-25 lifecycle and tools contract. Starting the adapter launches no engine.
Owned lifecycle uses a fixed locked release build and an immutable executable copy;
it closes and confirms only the process it owns before rebuilding. Source attribution
records input hashes, Git state and the actual executable hash. A matching manifest
is required to attribute an attached executable to source.

The production session has one update/preparation boundary. Native and offscreen
hosts select presentation and readback; neither duplicates the authoritative world.
Native scheduling uses elapsed time and ordinary terrain workers. Declared
deterministic offscreen fixtures use fixed steps and serial count-based terrain
admission, with those differences recorded in the result.

Native evidence copies the acquired surface after scene and egui composition in
the same encoder before submission. One asynchronous readback slot is limited to
64 MiB and supported RGBA/BGRA8 formats with surface `COPY_SRC` capability. No
synchronous GPU wait is introduced into native rendering. PNG encoding, cropping,
snapshot serialization and disk publication run outside frame processing. The full
client image and viewport image derive from the same pixels. A completion manifest
is published last, and evidence directories cannot be overwritten.

Prepared frame, GPU submission and presentation request are distinct observations.
Successful submission does not establish monitor presentation. Asynchronous GPU
measurements retain their source submission. Minimized inspection returns a stale
observation; new visual capture fails explicitly. Unsupported or failed native
capture never substitutes an offscreen image.

## Consequences

No general networking framework or assistant dependency enters the engine domain.
SHA-256 and base64 are focused app dependencies. Native capture and tooling idle
costs require separate measurement. Socket failure, lost control, stale observation,
pending terrain and unavailable capture remain visible outcomes.

This interface does not accept terrain quality, visual appearance, human camera UX,
native hot reload, pixel-to-terrain inspection or unrelated performance gates.
