# Overnight continuation authorization

This task is being run unattended.

You are authorized to continue beyond Slice 2A into **Slice 2B only**, subject to the strict gates below.

Do not begin Slice 2C, whole-body streaming, global adaptive LOD replacement, or performance optimization.

The objective is to use unattended time productively without allowing an unverified architectural assumption to propagate through the renderer.

---

# A. Complete Slice 2A first

Implement, validate, measure and document Slice 2A exactly as specified above.

Do not partially implement 2A and then move on because another task appears more interesting.

Before considering Slice 2B, produce an internal Slice 2A checkpoint and evaluate every Slice 2A acceptance gate.

---

# B. Automatic continuation gate

Proceed to Slice 2B only if ALL of the following are true:

1. The GPU terrain path consumes the authoritative world surface through the derived tile builder.
2. No procedural terrain implementation has been duplicated into the renderer/shader.
3. Fixed tile identity is deterministic and correct.
4. World → derived-tile approximation error is measured and understood.
5. Derived-tile → GPU reconstruction error is within an explicit acceptable physical tolerance.
6. Large-radius / observer-relative precision tests pass.
7. Stable terrain content remains GPU resident across repeated frames.
8. Unchanged terrain produces zero repeated terrain-content upload bytes after warm-up.
9. Relevant authoritative changes invalidate the tile correctly.
10. Camera, lighting, debug mode and runtime FrameId do not invalidate terrain content.
11. RockyV5, IcyV3 and VolcanicV3 all use the same generic renderer/tile path.
12. The old renderer remains intact.
13. No unresolved crash, corruption, stale-content issue or fundamental resource-lifetime problem exists.
14. Required focused tests pass.
15. The source state used for the evidence is clearly identified.

If any of these fail:

**STOP.**

Do not work around the failure by weakening the gate.

Investigate it, document the blocker, preserve evidence and finish with a Slice 2A PARTIAL / BLOCKED handoff.

---

# C. Slice 2B — Parent/Child GPU Terrain Hierarchy Prototype

If Slice 2A passes, continue directly into Slice 2B.

## Goal

Extend the proven single resident tile into the smallest real LOD hierarchy:

    one parent tile
          ↓
      four children

Prove:

- exact spatial correspondence,
- canonical shared borders,
- coherent fallback,
- independent child readiness,
- correct parent reconstruction,
- watertight GPU morphing,
- no synchronous refinement requirement.

Do not scale beyond this controlled hierarchy.

---

# D. Fixed hierarchy only

Use one deterministic test region.

Create:

- one parent,
- four children,
- their derived CPU tiles,
- persistent GPU residency slots,
- compact patch instances.

Do not connect this to the whole planetary adaptive selector yet.

The hierarchy may be driven by an explicit debug/test fixture.

---

# E. Spatial correspondence

Verify that each child's footprint corresponds exactly to the expected quarter of the parent footprint.

Test:

- canonical cube-face coordinates,
- child orientation,
- tile UV mapping,
- patch anchors,
- body radius reconstruction,
- observer-relative positioning.

A child must refine the same world area represented by its parent.

There must be no independent resampling coordinate convention.

---

# F. Same-level child borders

All four children must agree exactly or within a proven representation tolerance on shared boundaries.

Test:

- height,
- reconstructed position,
- normal/gradient behavior,
- material data where filtered.

Do not hide cracks with skirts as the primary solution.

Skirts may be used only as a temporary diagnostic if clearly labelled and not accepted as the transition architecture.

The intended surface must be intrinsically coherent.

---

# G. Parent/child border compatibility

The child boundary representation must remain compatible with the parent representation used during transition.

Explicitly identify:

- which parent samples/triangles correspond to child boundary vertices,
- how filtering/gutters behave,
- how corner samples behave.

Test edges and corners independently.

---

# H. Reconstruct the ACTUAL rendered parent surface

This is a hard requirement.

Do not morph the child toward the analytic authoritative terrain height at the parent's nominal sample locations.

Do not merely blend:

    child_authoritative_height
        ↔
    parent_authoritative_height

The child transition endpoint must reconstruct the actual triangle surface rendered by the parent grid.

Conceptually:

    Parent tile
        ↓
    parent grid vertices
        ↓
    rendered parent triangles
        ↓
    evaluate parent triangle surface
       at child vertex coordinate
        ↓
    child's morph-from position

At morph fraction 0:

> the child's geometry must coincide with the parent's rendered triangle surface.

At morph fraction 1:

> the child must coincide with its own derived tile representation.

This is central to avoiding visible popping.

---

# I. GPU transition

Perform the parent → child transition on the GPU.

Use compact transition metadata such as:

- parent tile slot,
- child tile slot,
- child address,
- morph fraction,
- required reconstruction metadata.

Do not rebuild complete transition meshes on the CPU.

Morph at least:

- vertex position,
- geometric normal or the inputs required for coherent normal reconstruction.

Material transition must not visibly contradict geometry.

A minimal material morph/filter proof is sufficient; final material rendering is not required.

---

# J. Parent remains valid until children are ready

The state machine must permit:

    parent DRAWABLE
         ↓
    child requested
         ↓
    child building
         ↓
    child uploaded
         ↓
    child resident
         ↓
    child drawable
         ↓
    begin transition
         ↓
    child takes ownership

Until a child is drawable:

> continue rendering the parent.

Never expose a hole.

Never delete the parent because a child has merely been requested.

---

# K. Independent child readiness

Children must be able to become ready independently.

Test situations such as:

    Child 0 ready
    Child 1 delayed
    Child 2 ready
    Child 3 absent

The surface must remain coherent.

Do not require all four children to synchronously complete before the frame can proceed unless a clearly justified topology invariant requires atomic replacement.

If atomic four-child publication is selected for this prototype, document why and prove that generation/upload still happens asynchronously while the parent remains drawable.

---

# L. Artificial delay test

Add a deterministic debug/test facility that can delay child availability.

Examples:

- 50 ms,
- 250 ms,
- 500 ms.

Expected behavior:

    delay child
        ↓
    parent continues rendering
        ↓
    ordinary frame delivery continues
        ↓
    child eventually becomes ready
        ↓
    transition starts

A delayed child must not turn into an equivalent render-thread delay.

Measure this rather than merely observing it visually.

---

# M. Cancellation / stale-result test

Exercise:

1. request child,
2. begin build,
3. invalidate/cancel it,
4. change request/content generation,
5. allow original result to complete late.

The late result must not publish as current terrain.

Test GPU slot generation / request epoch protections implemented in 2A.

---

# N. Transition reversal

Test:

    parent
      ↓
    child morph begins
      ↓
    demand reverses / camera retreats

The system must have defined behavior.

Acceptable strategies include:

- smoothly reverse morph,
- complete then merge,
- cancel only before publication.

Choose one deterministic policy.

Do not allow geometry to pop because the demand changed mid-transition.

---

# O. Transition lifetime

Morph time must be configurable.

Use a reasonable prototype duration, but do not freeze it as final engine policy.

Test:

- zero/near-zero debug morph,
- ordinary morph,
- deliberately slow morph.

This makes cracks and endpoint disagreement easier to inspect.

---

# P. Transition torture-test scene

Create a dedicated deterministic scene/debug fixture.

It should repeatedly exercise:

1. parent only,
2. request children,
3. delayed child,
4. transition parent → children,
5. hold children,
6. transition/return toward parent,
7. repeat,
8. move camera across child borders,
9. skim shared edge,
10. inspect corner.

Provide debug overlays/modes for:

- tile key/address,
- tile slot,
- parent/child relationship,
- LOD level,
- morph fraction,
- residency state,
- border error,
- CPU tile residual,
- GPU reconstruction residual.

This scene should be reusable for Slice 3 development.

---

# Q. No terrain-content reupload during morph

Morphing must not require uploading complete terrain content every frame.

Terrain content remains resident.

Expected changing data during morph:

- compact instance metadata,
- morph fraction,
- possibly small transition state.

Measure terrain tile upload bytes separately.

A 150 ms morph must not mean 150 ms of repeated tile uploads.

---

# R. Resource lifecycle

Measure:

- parent tile CPU bytes,
- four child CPU tile bytes,
- parent GPU payload,
- children GPU payload,
- staging bytes,
- instance metadata,
- topology buffers.

Check that resident parent+children coexist without unexpected allocation churn.

Do not yet implement full pressure-driven eviction.

---

# S. Parent retention

During the prototype, retain the parent while any child depends on it for:

- fallback,
- reconstruction,
- transition.

Do not evict parent data while it is needed for morphing.

Make dependency/pinning explicit.

This will become important in the future cache.

---

# T. Normal continuity

Verify normals through the transition.

At minimum inspect and measure:

- parent endpoint,
- mid-morph,
- child endpoint,
- shared child boundaries.

Avoid a transition where geometry is watertight but lighting visibly pops because normal reconstruction changes independently.

---

# U. Material continuity

Provide a minimal proof that authoritative material data remains spatially coherent through refinement.

The prototype does not need final surface shading.

But avoid:

    geometry morphs smoothly
    material pattern jumps instantly

if the difference is caused purely by representation/filtering level.

Document whichever derived-material filtering/morph rule is chosen.

---

# V. Precision

Repeat the relevant large-radius/offset precision tests with the parent/child hierarchy.

A numerically correct single tile does not prove child reconstruction stays correct.

Specifically test:

- parent/child difference at large reference radius,
- border positions,
- morph endpoints,
- observer-relative reconstruction.

---

# W. Metrics

For the transition fixture expose:

- parent resident state,
- child resident states,
- child request/build/upload state,
- morph fractions,
- terrain content uploaded/frame,
- compact metadata uploaded/frame,
- CPU tile work,
- CPU renderer preparation,
- GPU terrain time,
- largest observed frame time during artificial delay,
- waits attributable to terrain.

If any terrain refinement operation causes a synchronous wait, identify it explicitly.

Do not hide it in aggregate frame timing.

---

# X. Slice 2B acceptance gates

Proceed to final 2B handoff only if:

## B1 — Parent/child correspondence
Children represent exactly their intended parent subregions.

## B2 — Same-level borders
Child-child shared borders are watertight.

## B3 — Correct parent reconstruction
Morph fraction zero reproduces the actual rendered parent triangle surface.

## B4 — Correct child endpoint
Morph fraction one reproduces the child's own tile reconstruction.

## B5 — Transition continuity
No crack or unacceptable geometric pop occurs through the transition.

## B6 — Normal/material coherence
Transition does not introduce an obvious independent lighting/material discontinuity.

## B7 — Persistent content
Morphing does not repeatedly upload unchanged terrain tiles.

## B8 — Readiness fallback
Missing/delayed children retain coherent parent coverage.

## B9 — No synchronous refinement wait
Artificially delayed child work does not stall ordinary frame delivery.

## B10 — Stale result protection
Cancelled/late work cannot publish into reused/new content.

## B11 — Precision
Large-radius/offset parent-child reconstruction stays within defined tolerance.

## B12 — Old path preserved
The existing renderer remains intact.

---

# Y. Do NOT continue past 2B

Even if 2B succeeds early, STOP.

Do not automatically implement:

- complete cube-sphere terrain coverage,
- adaptive whole-body selection,
- planetary streaming,
- predictive prefetch,
- high-speed descent,
- horizon occlusion,
- cache pressure/eviction policy,
- multi-body terrain streaming,
- refinement debt controller,
- indirect GPU terrain selection,
- mesh shaders,
- compute terrain generation.

Those require user/reviewer inspection of the 2A/2B architecture first.

Use remaining unattended time for:

- stronger tests,
- repeat validation,
- transition torture testing,
- additional precision cases,
- diagnostics,
- evidence cleanup,
- independent source review,
- documentation,
- profiling the implemented prototype.

Do not convert spare time into additional architectural scope.

---

# Z. Overnight final handoff

When complete, report two explicit checkpoints:

## Slice 2A
- PASS / PARTIAL / BLOCKED
- gate results
- tile format
- grid format
- world→tile error
- tile→GPU error
- upload behavior
- residency/resource measurements

## Slice 2B
- PASS / PARTIAL / NOT STARTED
- parent/child correspondence
- border results
- morph endpoint error
- transition observations
- artificial-delay frame behavior
- stale/cancellation behavior
- resource measurements

Finish with exactly one recommendation:

### READY TO PLAN SLICE 2C
Only if both 2A and 2B pass their architectural gates.

### HOLD AT SLICE 2B
If 2B has a contained issue that needs user/reviewer evaluation.

### BLOCKED IN SLICE 2A
If the fundamental resident-tile representation does not pass.

No commit or push unless explicitly authorized.