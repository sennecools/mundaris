# Mundaris Planet Terrain Redesign — Slice 2C
## Adaptive Regional LOD, Residency Cache & Refinement Scheduling

Proceed with **Slice 2C**.

This phase follows:

- Slice 2A: one resident GPU terrain tile
- Slice 2B: one fixed parent + four children with GPU morphing

Both architectural checkpoints passed.

Before modifying code, read and preserve:

- `docs/PLANET_TERRAIN_RENDERING_REDESIGN.md`
- `docs/PLANET_TERRAIN_SLICE_2A_REPORT.md`
- `docs/PLANET_TERRAIN_SLICE_2B_REPORT.md`
- ADR 0012
- ADR 0013
- current resident tile implementation
- current fixed hierarchy implementation
- current terrain developer diagnostics
- existing adaptive terrain/LOD/topology code
- current developer scenario/capture infrastructure
- relevant precision/reference-frame ADRs
- relevant Slice 1B.2 world-authority and tile-builder contracts

The repository is intentionally dirty.

Preserve unrelated work.

Do not commit or push unless explicitly authorized.

Do not begin full whole-planet streaming unless explicitly authorized after this phase.

---

# 1. Goal

Slice 2A proved:

- authoritative world → deterministic derived tile,
- persistent GPU residency,
- generic GPU displacement,
- stable precision,
- zero repeated terrain-content upload after warm-up.

Slice 2B proved:

- parent/child spatial correspondence,
- actual rendered-parent reconstruction,
- watertight fixed hierarchy transitions,
- no synchronous frame stall under delayed child generation,
- stale-result rejection,
- morph reversal,
- parent fallback.

Slice 2C must prove that the same architecture can scale from:

    one parent + four children

to:

    a dynamic regional adaptive hierarchy
    containing many simultaneously desired,
    resident,
    building,
    delayed,
    cancelled,
    transitioning,
    cached,
    and fallback-backed patches

without losing:

- frame stability,
- topology correctness,
- residency correctness,
- precision,
- deterministic identity,
- bounded work,
- observable resource behavior.

The primary goal is:

> Build a real adaptive regional terrain selection/cache/scheduler around the resident tile system while keeping quality allowed to lag behind demand instead of blocking frame delivery.

This is still not the final whole-planet streaming phase.

---

# 2. Core invariant

The central Slice 2C rule is:

> **Desired quality and currently drawable quality are separate states.**

The system must explicitly support:

    desired hierarchy
          ≠
    resident hierarchy
          ≠
    currently drawn hierarchy

A patch may be desired at a finer level while the renderer continues drawing a valid resident ancestor.

Never equate:

    selected for refinement
        =
    immediately drawable

---

# 3. High-level architecture

Implement approximately:

    Camera / view state
           ↓
    Adaptive regional selector
           ↓
      desired hierarchy
           ↓
    dependency/readiness analysis
           ↓
     refinement scheduler
           ↓
    request priority queue
           ↓
    bounded tile workers
           ↓
    CPU derived tile cache
           ↓
    bounded GPU residency cache
           ↓
    drawable hierarchy
           ↓
    parent fallback / morphs
           ↓
         render

Selection, generation, residency and draw publication must remain separate layers.

---

# 4. Scope

Slice 2C should operate over a **regional adaptive test domain**, not an entire planet.

The region must be large enough to contain:

- many simultaneously visible patches,
- several LOD levels,
- mixed resident states,
- multiple concurrent parent/child transitions,
- cache reuse,
- cancellation,
- camera motion.

The exact size is not prescribed.

Choose a region large enough to expose scalability issues while remaining debuggable.

The prototype should preferably exercise tens to low hundreds of visible/desired patches rather than five.

Do not immediately jump to thousands if a smaller regional fixture exposes the same architectural problems.

---

# 5. Adaptive selector

Introduce a real dynamic selector for the new resident-tile path.

Base desired refinement primarily on projected geometric error.

Reuse proven Mundaris concepts where appropriate:

- projected error,
- split/merge hysteresis,
- camera relevance,
- canonical cube-sphere addressing,
- balanced topology constraints.

Do not use simple fixed distance rings as the main policy.

Avoid:

    0–1 km = L12
    1–5 km = L11
    ...

Prefer per-patch projected error.

---

# 6. Desired vs drawable hierarchy

Maintain explicit concepts such as:

    DesiredPatchSet
    ResidentPatchSet
    DrawablePatchSet

Exact type names are not prescribed.

For each desired patch, record whether it is:

- absent,
- requested,
- building,
- built CPU,
- queued for upload,
- resident,
- drawable,
- transitioning,
- cached but not visible.

If a desired fine patch is unavailable:

> draw the nearest valid ancestor.

No holes.

No synchronous waiting.

---

# 7. Refinement debt

Implement a measurable **refinement debt / quality lag** concept.

This must quantify how far the drawable hierarchy is behind the desired hierarchy.

Possible metrics include:

- desired level minus drawable level,
- projected error excess,
- total unresolved projected error,
- unresolved visible area,
- weighted refinement deficit.

A single metric or a small set is acceptable.

It must be observable.

Expected behavior:

During rapid motion:

    refinement debt increases
    frame time stays stable

After motion stops:

    refinement debt decreases toward zero

This is preferable to holding debt near zero by stalling frames.

---

# 8. Hysteresis

Preserve split/merge hysteresis.

Avoid LOD thrashing near thresholds.

For example:

    split at error > A
    merge at error < B
    where B < A

Exact thresholds must be measured/configurable.

Do not freeze old values automatically.

---

# 9. Regional mixed-LOD topology

Slice 2B intentionally avoided unproved mixed-level topology by atomically publishing all four children.

Slice 2C must now establish a scalable mixed-LOD policy.

Required:

- adjacent visible regions may differ in LOD,
- topology remains watertight,
- refinement dependencies are explicit.

Use a proven policy such as:

- 2:1 adjacency balance,
- closure refinement,
- edge constraints,
- canonical boundary reconstruction,
- or another rigorously tested equivalent.

Do not rely on skirts as the accepted primary solution.

If skirts are used diagnostically, label them as temporary.

---

# 10. Parent/child transition reuse

Reuse the Slice 2B GPU morph architecture.

Do not create a second transition implementation.

For every transitioning child:

- coarse endpoint reconstructs the actual rendered parent surface,
- fine endpoint reconstructs the child tile,
- normals/materials remain coherent,
- terrain content remains resident,
- only compact transition metadata changes per frame.

---

# 11. Independent hierarchy publication

Unlike 2B's fixed atomic four-child replacement, Slice 2C must determine a scalable publication policy.

Options may include:

- atomic refinement closure groups,
- balanced local publication groups,
- independently drawable children with proven mixed-edge handling.

Choose the smallest policy that guarantees correctness.

Document:

- publication unit,
- dependency rules,
- why it remains watertight,
- how delayed siblings are handled.

Do not require global synchronization.

---

# 12. Request priority scheduling

Do not process requests FIFO.

Implement a deterministic priority system.

Priority should consider some combination of:

- projected geometric error,
- screen coverage,
- distance,
- camera centrality,
- approach velocity,
- expected visibility duration,
- parent readiness,
- transition dependencies,
- whether the patch fills a visible quality hole,
- whether the request is likely to become obsolete.

Exact formula is not prescribed.

Record the factors used.

---

# 13. Camera-motion prediction

Add a simple predictive term if useful.

At minimum consider:

- camera velocity,
- approach direction.

Do not build an elaborate AI predictor.

The purpose is to request likely-needed detail before it becomes urgent.

Predictive requests must remain lower priority than immediately visible missing dependencies where appropriate.

---

# 14. High-speed bias

At high camera speed, avoid generating fine terrain that will immediately leave the screen.

Implement a configurable policy that can reduce refinement pressure based on expected tile usefulness/lifetime.

Example concept:

    expected visible lifetime too short
        ↓
    deprioritize deepest refinement

Do not reduce coarse valid coverage.

---

# 15. Work admission

Add explicit bounded work admission.

Do not let one frame schedule unbounded work because many patches exceed the split threshold.

Control at least:

- new CPU tile jobs admitted,
- completed CPU results retained,
- uploads admitted,
- GPU tile publications,
- transitions started.

Budgets may be expressed as:

- count,
- bytes,
- time,
- or a combination.

Prototype values must be configurable and observable.

Do not present them as permanent engine constants.

---

# 16. CPU worker scheduling

Reuse the asynchronous worker model where appropriate.

Required:

- bounded queueing,
- cancellation/epoch protection,
- no render-thread blocking,
- no unbounded job accumulation.

Record:

- queued jobs,
- running jobs,
- completed unpublished jobs,
- cancelled jobs,
- stale completions.

---

# 17. Cancellation

Exercise real cancellation under camera motion.

Scenario:

    request patch A
        ↓
    worker starts
        ↓
    camera moves
        ↓
    A no longer needed

System should:

- cancel if cheap,
- deprioritize,
- or finish-and-cache if nearly complete.

Choose a deterministic policy.

Measure wasted work.

---

# 18. Cancellation waste telemetry

Expose:

- requests issued,
- jobs started,
- jobs cancelled before start,
- jobs cancelled during work if supported,
- stale completions rejected,
- completed-but-unused tiles,
- bytes built but never published,
- build time spent on discarded work.

This will matter greatly in future high-speed travel.

---

# 19. CPU derived tile cache

Introduce a real CPU derived tile cache.

Requirements:

- key by the Slice 2A content identity,
- distinguish pinned vs evictable,
- preserve resident/draw dependencies,
- reuse built tiles where possible,
- avoid rebuilding identical tiles unnecessarily.

Do not mix CPU tile cache identity with GPU slot identity.

---

# 20. GPU residency cache

Replace the fixed five-slot hierarchy with a configurable regional tile pool.

The GPU cache must track:

- slot index,
- slot generation,
- content key,
- resident state,
- in-flight usage,
- pinned dependency state,
- last-use/reuse metadata,
- evictability.

Do not yet target a final whole-planet memory policy.

---

# 21. No arbitrary permanent tile count

A prototype slot count is acceptable.

It must be:

- configurable,
- documented,
- deliberately chosen for the regional test,
- not treated as a permanent limit.

Do not replace the old 2,048-leaf problem with:

    const GPU_TILE_COUNT = 2048

and call it solved.

Expose when capacity, not memory, blocks quality.

---

# 22. Dependency pinning

A tile must remain pinned while required by:

- current draw,
- fallback,
- child transition,
- parent reconstruction,
- in-flight GPU submission,
- publication transaction.

Do not evict ancestors while children still rely on them.

---

# 23. GPU-safe eviction

Eviction must remain submission-safe.

A slot may be reused only when:

- no active draw references it,
- no transition depends on it,
- relevant GPU work is complete,
- the slot generation changes before reuse.

Do not globally wait for the GPU in normal operation.

---

# 24. Eviction policy

A simple initial policy is acceptable.

Examples:

- LRU with pinning,
- clock-like reuse,
- score based on visibility/recent use.

Prefer simple and observable.

Measure before adding complexity.

---

# 25. Reuse test

Add an explicit route:

    Region A
        ↓
    Region B
        ↓
    Region C
        ↓
    Region B
        ↓
    Region A

Measure:

- CPU cache hits,
- GPU cache hits,
- rebuild count,
- reupload count,
- convergence time,
- evictions.

This is a major acceptance fixture.

---

# 26. Upload budget

Introduce an explicit terrain upload admission policy.

Track:

- bytes queued,
- bytes uploaded/frame,
- number of tiles uploaded/frame,
- backlog.

Do not allow one frame to upload an unbounded set of completed tiles.

When uploads lag:

> keep drawing ancestors.

---

# 27. Publication budget

Separate:

- upload completion,
- publication into drawable hierarchy.

Do not necessarily start dozens of transitions in one frame.

Add a configurable limit or cost-based admission for:

- newly published refinements,
- transitions started.

---

# 28. Transition concurrency

Exercise multiple simultaneous morphs.

Measure:

- number of active transitions,
- metadata cost,
- GPU terrain time,
- frame stability.

The system should support more than one transition without requiring global topology freeze.

If a local closure requires coordinated morphs, define that explicitly.

---

# 29. Frame-time invariant

Carry forward the 2B rule:

> Terrain quality may lag. Ordinary frame delivery must not wait for terrain refinement.

Instrument and preserve:

- terrain-attributable waits,
- frame intervals,
- CPU terrain work,
- upload events,
- resource growth,
- transition events.

If the system violates this, report the exact cause.

---

# 30. Artificial load testing

Add deterministic developer controls to stress the scheduler.

Test combinations such as:

- 50 ms worker delay,
- 250 ms worker delay,
- 500 ms worker delay,
- reduced worker count,
- reduced GPU slots,
- reduced upload bytes/frame,
- reduced publication budget.

Expected:

- quality convergence slows,
- refinement debt increases,
- frame delivery remains coherent.

---

# 31. Deliberate overload test

Create a test where demand intentionally exceeds processing capacity.

For example:

- fast camera movement,
- limited worker throughput,
- small upload budget.

Observe:

- no holes,
- no invalid topology,
- no synchronous stalls,
- bounded queues,
- rising refinement debt,
- eventual recovery after camera stops.

This is a core Slice 2C proof.

---

# 32. Camera movement scenarios

Add deterministic regional scenarios such as:

## Slow approach

Refine gradually toward a target region.

## Fast flyover

Cross the region faster than full detail can converge.

## Orbit-like lateral pass

High lateral velocity, changing visible patches.

## Dive and stop

Rapid approach followed by stationary observation.

## Retreat

Fine detail becomes unnecessary.

## Backtrack

Return to recently visited terrain.

These are still regional fixtures, not whole-planet routes.

---

# 33. Quality convergence

Measure time to converge after the camera stops.

Record:

- starting debt,
- debt over time,
- time to zero/threshold,
- tiles built,
- tiles uploaded,
- cache reuse.

Do not silently lower desired quality because generation is slow.

---

# 34. Selection error

Track at least:

- desired projected error,
- drawable projected error,
- difference/debt.

This will later become one of the key whole-planet diagnostics.

---

# 35. Terrain build cost awareness

Slice 2B measured canonical child tile builds at roughly hundreds of milliseconds.

Do not assume build cost is negligible.

Use scheduling to accommodate it.

If profiling reveals obvious avoidable duplicate work, fix it.

Do not yet move generation to GPU compute merely because it is expensive.

---

# 36. No premature compute generation

Do not implement GPU compute terrain generation in 2C unless:

1. CPU tile generation is directly measured as the dominant blocker,
2. scheduler/cache improvements cannot meet the regional goals,
3. a separate evidence-backed mini-design is documented.

Default: keep CPU tile build.

---

# 37. No premature mesh shaders / indirect rendering

Do not add:

- mesh shaders,
- task shaders,
- bindless,
- sparse textures,
- raw Vulkan paths,
- indirect draw generation

unless strictly necessary.

The current indexed reusable-grid path should remain the baseline.

---

# 38. Regional draw submission

It is acceptable to issue multiple ordinary indexed-instanced draws.

Do not optimize draw submission count prematurely.

First measure:

- CPU encode cost,
- GPU draw cost,
- visible patch count.

If batching by topology/state is useful, reuse simple grouping.

---

# 39. Old renderer remains

The existing production terrain path remains available.

Do not remove it.

Slice 2C may add a regional new-path fixture or opt-in mode.

Old/new comparison remains valuable.

---

# 40. Developer diagnostics

Extend existing diagnostics with:

- desired patch count,
- drawable patch count,
- resident patch count,
- CPU cached patch count,
- building patch count,
- upload backlog,
- active transitions,
- refinement debt,
- maximum drawable projected error,
- cache hit/miss,
- evictions,
- rebuilds,
- reuploads,
- cancelled jobs,
- stale results,
- terrain upload bytes/frame,
- terrain-attributable waits,
- slot pressure,
- count/work pressure reason.

Keep them structured.

---

# 41. Visual debug modes

Add or preserve useful debug views:

- LOD level colors,
- desired vs drawable level,
- resident vs fallback,
- tile slot ID,
- parent/child dependency,
- morph fraction,
- cache state,
- selected/requested patches,
- projected error,
- refinement debt.

Debug modes must not change world identity.

---

# 42. Regional torture-test scene

Create a deterministic reusable Slice 2C scene.

It should contain enough terrain to drive:

- multiple levels,
- several simultaneous transitions,
- cache churn,
- cancellation.

The camera should execute a scripted route such as:

1. start coarse,
2. slow approach,
3. accelerate,
4. lateral turn,
5. stop,
6. wait for convergence,
7. retreat,
8. revisit previous area,
9. repeat under artificial worker delay,
10. repeat under reduced tile capacity.

Record states throughout.

---

# 43. Mixed-level seam proof

Add focused tests for mixed LOD edges.

Required cases:

- fine next to coarse,
- fine next to fine,
- corner where several levels meet,
- transition in progress beside stable neighbor,
- delayed neighbor.

Measure:

- position mismatch,
- normal mismatch,
- material mismatch.

Define explicit tolerances.

Do not rely only on screenshots.

---

# 44. Canonical cube topology

The regional test should include at least one difficult topology case where practical:

- cube-face edge,
- cube corner,
- or both.

Do not postpone all cross-face correctness to whole-planet integration.

Reuse canonical addressing/topology math.

---

# 45. Precision

Repeat large-radius/large-offset validation under adaptive mixed LOD.

A dynamic selector/cache may expose code paths not exercised by the fixed hierarchy.

Test:

- large reference radius,
- sibling frames,
- large common translation,
- mixed levels,
- transitioning edges.

---

# 46. Resource accounting

Expose separately:

## CPU

- cached tile payload,
- worker job scratch,
- completed unpublished payload,
- queue metadata.

## GPU

- tile payload capacity,
- active resident payload,
- pinned payload,
- evictable payload,
- in-flight payload,
- grid/topology,
- instance/transition metadata.

## Transfer

- queued upload bytes,
- uploaded bytes/frame,
- staging capacity.

Do not report these as physical VRAM unless actually measured.

---

# 47. Pressure behavior

When regional cache pressure occurs:

1. preserve current valid coverage,
2. preserve required ancestors,
3. reduce optional detail,
4. evict low-value detail,
5. allow refinement debt to rise.

Never create holes.

Never silently invalidate the world.

---

# 48. Failure reasons

Diagnostics must distinguish quality limits caused by:

- CPU build backlog,
- upload backlog,
- GPU slot pressure,
- publication budget,
- topology dependency,
- transition dependency,
- cancellation,
- count/work admission,
- memory policy.

Do not collapse them into a generic `budget_constrained`.

---

# 49. Tests

Add focused tests for:

## Selection

- deterministic desired hierarchy,
- split/merge hysteresis,
- projected-error ordering.

## Balance

- required adjacency constraints.

## Readiness

- desired child absent → parent fallback.

## Scheduling

- high-priority visible requests beat low-value requests.

## Cancellation

- obsolete work is cancelled/rejected.

## Cache

- hit/reuse,
- safe eviction,
- slot-generation protection.

## Upload

- budget respected,
- backlog drains.

## Mixed topology

- seam correctness.

## Refinement debt

- increases under overload,
- decreases after demand stabilizes.

## Precision

- dynamic mixed LOD remains within bounds.

---

# 50. Performance measurements

Measure at minimum:

- selector CPU time,
- request scheduling time,
- worker build throughput,
- tile build latency distribution,
- publication time,
- upload bytes/frame,
- upload backlog,
- active transitions,
- resident tiles,
- cache hit/miss/eviction,
- CPU render preparation,
- GPU terrain time,
- native frame intervals,
- frame p50/p95/p99,
- maximum observed terrain-correlated hitch,
- refinement debt over time,
- convergence time after stop.

Use real engine runtime where possible.

Do not infer smoothness only from offscreen captures.

---

# 51. Acceptance route

Run at least one native regional movement route with:

- no artificial delay,
- 50 ms delay,
- 250 ms delay,
- 500 ms delay.

Also test constrained capacity/upload budget.

For each route record:

- frame intervals,
- desired vs drawable quality,
- debt,
- queue sizes,
- uploads,
- cache state.

---

# 52. Acceptance gates

## C1 — Adaptive desired hierarchy

Projected-error selection produces a deterministic desired regional hierarchy.

## C2 — Drawable fallback hierarchy

Unavailable desired detail is backed by valid resident ancestors with no holes.

## C3 — Mixed-LOD topology

Adjacent mixed levels remain watertight within explicit tolerances.

## C4 — Asynchronous refinement

Worker/build/upload delays increase quality lag rather than frame stalls.

## C5 — Bounded scheduling

Job/upload/publication queues remain bounded under overload.

## C6 — Priority behavior

Important visible/approaching detail receives higher priority than low-value requests.

## C7 — Refinement debt

Debt is measurable, rises under overload, and converges after demand stabilizes.

## C8 — CPU tile cache

Previously built relevant tiles are reused rather than rebuilt unnecessarily.

## C9 — GPU residency cache

Resident tiles persist, cache pressure evicts only safe/low-value content, and reused slots cannot accept stale publication.

## C10 — Upload budget

Terrain uploads are bounded and observable; no uncontrolled upload burst is required for correctness.

## C11 — Cancellation

Obsolete work is cancelled/deprioritized/rejected correctly.

## C12 — Backtracking reuse

A→B→C→B→A demonstrates meaningful cache reuse.

## C13 — Multiple simultaneous transitions

The renderer supports multiple local transitions without global topology freeze.

## C14 — Precision

Mixed adaptive hierarchy remains within established physical precision expectations.

## C15 — Frame stability

No terrain-refinement operation produces an equivalent synchronous stall under tested delays/pressure.

## C16 — Resource observability

CPU/GPU/transfer/cache categories and pressure reasons are separately visible.

## C17 — Old path preserved

Existing terrain renderer remains operational.

---

# 53. What does NOT block 2C

Do not fail Slice 2C solely because:

- tile build latency remains high,
- visual art is imperfect,
- Rocky 8 m authoring remains weak,
- draw count is not yet optimal,
- cache policy could be more sophisticated,
- mesh shaders might later improve performance.

Those may become future optimization tasks.

---

# 54. What DOES block continuation

Stop if:

- mixed LOD creates unresolved cracks,
- cache eviction can remove required ancestors,
- stale tile publication can corrupt reused slots,
- terrain delays create render-thread stalls,
- queues can grow without bound,
- pressure creates holes,
- desired/drawable state cannot be separated cleanly,
- precision fails at mixed levels,
- cache identity can reuse wrong terrain.

---

# 55. Explicitly out of scope

Do NOT implement yet:

- complete six-face whole-planet coverage,
- orbit-to-ground full streaming route,
- terrain-aware horizon occlusion,
- full multi-body residency,
- final dynamic memory-budget policy,
- GPU compute terrain generation,
- mesh shaders,
- bindless terrain,
- sparse GPU residency,
- hierarchical object containers,
- rock/debris streaming,
- caves/volumetric terrain,
- atmosphere work,
- survival gameplay.

---

# 56. Full engine-interface authority

You are explicitly authorized to use and extend Mundaris's development interface during this phase.

Use it to:

- operate the real engine,
- run deterministic camera routes,
- inspect cache state,
- inject worker/upload delays,
- reduce slot capacity,
- capture timing,
- inspect refinement debt,
- observe seams/transitions,
- profile CPU/GPU terrain work.

If critical scheduler/cache behavior is not currently observable, add focused diagnostics to the existing structured developer interface.

Do not rely only on unit tests.

Runtime observation is a required part of this phase.

---

# 57. Runtime fault injection

Developer-only controls may be added for:

- worker delay,
- upload budget,
- publication budget,
- GPU slot count,
- worker count,
- forced cancellation,
- forced cache pressure,
- morph duration.

All must default off and remain outside world identity.

---

# 58. Evidence package

Produce a dedicated Slice 2C evidence root containing:

## Source identity

- initial/final state,
- input manifests,
- executable hashes,
- preservation audit.

## Selection

- desired hierarchy snapshots,
- drawable hierarchy snapshots,
- projected error/debt.

## Scheduling

- request priority traces,
- queue sizes,
- cancellation results.

## Cache

- hit/miss/eviction/reuse traces,
- slot generations,
- dependency pinning.

## Upload

- bytes/frame,
- backlog,
- publication counts.

## Precision/topology

- mixed-level seam tests,
- edge/corner fixtures,
- GPU reconstruction measurements.

## Runtime

- scripted movement scenarios,
- delay variants,
- constrained-capacity variants,
- frame timing distributions,
- convergence timing.

## Visual

- LOD/debug captures,
- parent/fallback views,
- mixed-level edges,
- transition states.

---

# 59. Handoff report

Create `PLANET_TERRAIN_SLICE_2C_REPORT.md`.

Use explicit classifications:

- IMPLEMENTED
- VERIFIED
- MEASURED
- OBSERVED
- PARTIAL
- FAILED / OPEN

Report:

- regional hierarchy scale,
- max desired patches,
- max drawable patches,
- cache capacities used,
- worker counts,
- upload limits,
- scheduling policy,
- mixed-LOD policy,
- refinement-debt definition,
- cache reuse results,
- cancellation waste,
- frame timing,
- pressure behavior,
- known weaknesses.

---

# 60. Stop condition

When Slice 2C is complete:

**STOP.**

Do not automatically begin full whole-planet streaming.

Finish with exactly one recommendation:

### READY TO PLAN WHOLE-PLANET STREAMING

Only if:

- mixed adaptive LOD is correct,
- scheduling is bounded,
- cache reuse works,
- overload degrades quality rather than frame delivery,
- resource pressure is observable,
- no fundamental regional scalability defect remains.

### HOLD IN SLICE 2C

If contained scheduler/cache/topology issues remain.

### BLOCKED BY ARCHITECTURAL DEFECT

If the resident hierarchy cannot scale safely beyond the fixed 2B prototype.

Do not commit or push unless explicitly authorized.