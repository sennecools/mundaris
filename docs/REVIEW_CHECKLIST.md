# Review checklist

Use only the sections relevant to the question or task. This is a review aid, not a
mandatory checklist dump in conversation. Apply the evidence hierarchy in `AGENTS.md`.

## Architecture

- Does implementation match intended authoritative/derived ownership and dependencies?
- Was unrelated architecture changed or complexity increased unnecessarily?
- Are task scope and user-approved direction preserved?

## Correctness

- Are precision, determinism, publication, and resource invariants preserved?
- Are failure paths tested? Is cancellation safe, with no partial live-state publish?
- Does checked source match the claimed result?

## Performance

- Was the actual bottleneck measured, rather than assumed?
- Are before/after fixture, revision, hardware, quality, and sampling comparable?
- Are CPU, GPU, wall time, memory, and data movement distinguished?
- Is work reduced, or merely deferred/hidden/lowered in quality?

## Visual

- Are captures from the current code, with reproducible settings and build identity?
- Was terrain settled where required, and was useful quality actually reached?
- Are debug overlays contaminating judgment? Does machine state match image state?
- Does visual evidence meet the user's goal, not merely show functioning rendering?

## UX

- Does it actually feel/use better in native interaction?
- Is the UI understandable, and can the user perform the intended workflow?
- Are offscreen/headless evidence limits explicit?

## AI observability

- Can an agent inspect runtime state directly, not only infer it from pixels?
- Are diagnostics tied to actual rendered state, with pending/stale state distinguished?
- Are captures/snapshots reproducible and paired by frame/revision where required?

## Completion

- Did the requested acceptance gate actually pass with relevant evidence?
- What remains partial, failed, untested, or blocked?
- Were unsupported completion claims rejected and unchanged criteria preserved?
- Has the user retained product/vision authority and control over the next step?
