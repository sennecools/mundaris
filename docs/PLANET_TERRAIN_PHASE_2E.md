Phase 2E — Engine Observability, Terrain Pacing Lab & 2D Closure
We are not starting Phase 3A yet.
The resident Moon terrain backend is now integrated. Existing numerical/GPU correctness tests and the current terrain quality suite pass, but native pacing still fails:
- terrain publication has exceeded its 2 ms frame-thread budget;
- a recent warm 100 km native window reached roughly 157.8 ms host-frame time, above the existing 100 ms failure threshold;
- close-range terrain detail can still take minutes to converge;
- boundary preparation has been observed as a serialized stage;
- publication can wait behind active merge handoffs.
At this point, do not continue making speculative terrain scheduling changes without better evidence.
This phase has two sequential goals:
1. build a small but serious engine observability and profiling layer that humans and coding agents can use to understand CPU/GPU/frame/job behavior;
2. use those tools immediately to identify and fix the remaining 2D terrain pacing/detail-arrival blockers.
Do not stop after implementing the profiler/UI. Continue into the 2D fixes and close the remaining acceptance gates.
1. Scope and non-goals
This is infrastructure for understanding the existing engine, not another terrain redesign.
Preserve:
- resident terrain architecture;
- existing tile topology;
- parent/child reconstruction;
- exact-key residency;
- morphing;
- fallback;
- terrain authority;
- precision rules;
- bounded memory;
- deterministic generation;
- existing passing numerical/GPU tests.
Do not begin:
- 3A landform improvements;
- climate/environment fields;
- materials redesign;
- vegetation;
- GPU terrain generation;
- erosion systems;
- atmosphere changes;
- general ECS/job-system rewrites;
- a complete external-profiler replacement.
The profiling infrastructure should be reusable by later phases, but implement only what is justified by current needs.
2. First produce a baseline
Before changing behavior, inspect the current code and establish a reproducible baseline.
Record:
- current HEAD and working-tree state;
- active Moon terrain backend;
- existing profiling/instrumentation facilities;
- existing egui diagnostic panels;
- current CPU frame timing;
- current GPU timing capability;
- terrain queue stages;
- existing benchmark/native routes;
- current publication timing;
- current 100 km warm route;
- close-range refinement/convergence behavior.
Preserve baseline artifacts so after changes we can compare the same workload.
Do not optimize anything before the initial measurement is captured.
3. Add a lightweight engine profiling foundation
Build a reusable internal profiling layer suitable for high-frequency engine code.
It should support nested named CPU spans such as:
Frame
├─ Simulation
├─ Terrain
│  ├─ Desired cover
│  ├─ Completion drain
│  ├─ Publication scan
│  ├─ Adoption
│  │  ├─ Dependency validation
│  │  ├─ Resident state update
│  │  ├─ Boundary handling
│  │  ├─ Transition setup
│  │  └─ Upload preparation
│  └─ Drawable preparation
├─ Renderer preparation
├─ GPU submission
└─ UI

Requirements:
- nested spans;
- thread identity;
- worker identity where useful;
- start/end timestamps;
- rolling p50/p95/max;
- current-frame value;
- configurable threshold/warning levels;
- very low overhead when enabled;
- cheap/no-op path when disabled;
- no high-frequency text logging.
Prefer a clean internal abstraction so future systems can use the same instrumentation.
Do not scatter raw timing code throughout the application without structure.
4. Terrain job lifecycle tracing
Every terrain request should be traceable through the complete pipeline.
Use its existing stable terrain key/address/revision where possible.
Track timestamps/events for stages such as:
requested
queued_generation
generation_started
generation_finished

queued_boundary
boundary_started
boundary_finished

fully_prepared
publishable

publication_started
publication_finished

resident
drawable
transition_finished

cancelled
superseded
discarded_stale
evicted

From these timestamps derive per-tile latency such as:
request → generation start
generation duration
generation → boundary start
boundary duration
boundary → publishable
blocked duration
publication wait
publication duration
request → drawable
request → final transition completion

The point is to answer questions like:
Why did this visible tile take 14 seconds to become detailed?

without inferring it from source inspection.
5. Track blocking reasons explicitly
A prepared result that cannot publish must have a machine-readable reason.
At minimum distinguish:
- active split overlap;
- active merge overlap;
- parent dependency;
- child dependency;
- neighbor/boundary dependency;
- resident slot unavailable;
- GPU/resource pressure;
- revision mismatch;
- stale result;
- publication budget exhausted;
- other explicitly named reason.
Do not collapse all of these into not_ready.
Track:
count currently blocked
oldest blocked age
p95 blocked age
total block time
number of retries/rechecks

A transient block must not require expensive work to be reconstructed every frame.
6. Queue diagnostics must include AGE, not only depth
Track both count and age for each stage:
generation queued
generation active

generated awaiting boundary work

boundary queued
boundary active

prepared

publishable

blocked by split
blocked by merge
blocked by dependency
blocked by resource

publication queue

For each queue expose:
- count;
- oldest item age;
- median/p95 age where inexpensive;
- throughput;
- completed per second;
- dropped/cancelled/superseded count.
This should immediately reveal whether generation, boundary preparation, transition waiting, or publication is the real bottleneck.
7. Add a human-facing “Performance Lab” UI
Create a polished developer-facing egui panel/window named something like:
Performance Lab
This should become a generally useful engine diagnostics surface, not a terrain-only debug dump.
Keep it compact enough to leave the actual scene visible.
Use collapsible sections/tabs rather than displaying hundreds of numbers at once.
Suggested top-level views:
Overview
Timeline
Terrain
Jobs
GPU
Captures
Benchmarks

Overview
Show immediately:
Frame CPU        11.4 ms     p95 14.1     max 21.8
GPU Frame         8.2 ms     p95  9.0
FPS               87

Terrain backend   RESIDENT TILE

Terrain CPU        1.7 ms
Publication        0.43 ms
Terrain GPU        ...

CPU terrain mem   118 MiB / limit
GPU terrain mem   ...

Upload/frame       0 B

Make budget violations visually obvious.
Use restrained status colouring:
- normal;
- warning;
- failure.
Do not make the entire UI flash or become unreadable.
8. Add a live mini timeline
I want to be able to watch the engine working, not only read averages.
Add a compact scrolling timeline covering the last few seconds.
It should support at least:
CPU frame time
terrain total
publication
GPU frame time

Display the frame budget as a reference line.
A second compact worker timeline should make concurrency obvious:
Main
Terrain Worker 0
Terrain Worker 1
Terrain Worker 2
Terrain Worker 3
Boundary Worker(s)
Render/GPU submission

It does not need to become a full Tracy clone.
The purpose is to visually expose patterns such as:
four generation workers busy
while one boundary worker serializes everything

or:
main thread repeatedly stalls during publication

Allow pause/freeze and frame selection.
9. Terrain pipeline UI
The Terrain view should make the refinement pipeline understandable at a glance.
Show something equivalent to:
GENERATION
queued       28
active        4
throughput   18.4 tiles/s
oldest       220 ms

BOUNDARIES
queued       41
active        1
throughput    5.2 tiles/s
oldest        2.8 s

PREPARED
total        76
publishable  12

BLOCKED
merge        43    oldest 9.6 s
split         4
neighbor      8
resource      9

PUBLICATION
throughput    4.7 tiles/s
current       0.3 ms
p95           0.8 ms
max           7.2 ms
budget        2.0 ms

Include:
- desired tiles;
- resident tiles;
- drawable tiles;
- fallback tiles;
- active transitions;
- refinement debt;
- bytes uploaded this frame;
- generated tiles this frame;
- published tiles this frame.
10. Add a Tile Inspector
Let a developer click/select a visible terrain patch or choose one from a diagnostic list.
Show:
body
face/address
LOD
revision
desired state
resident state
drawable state
parent
children
neighbors
transition state
current blockers

requested at
generated at
boundary prepared at
publication at
drawable at
total age

Where practical, allow selection from the scene through an existing debug/picking mechanism.
This is for debugging pathological long-lived tiles.
11. Quality-convergence metric
Add a machine-readable metric representing how far the currently drawable terrain is from the currently desired visual quality.
Do not use a simple fraction of tiles at target LOD, because a tiny horizon tile should not count the same as a large central screen region.
Base the measure on existing projected-error/perceptual information where possible.
Produce at least:
visible convergence
center-screen convergence
worst visible unresolved error

The exact mathematical representation should be documented and tested.
The purpose is to distinguish:
useful visual convergence

from:
every last requested tile has completed

Use this later for benchmark acceptance.
12. Deterministic benchmark runner
Add a reproducible native benchmark/scenario runner for terrain.
It must be able to execute known Moon routes without manually flying the camera.
Include deterministic viewpoints or trajectories covering approximately:
far orbit
1000 km
500 km
250 km
100 km
25 km
5 km
1 km
close surface

Exact distances can be adjusted to current camera semantics.
Also include:
rapid approach
rapid retreat
camera reversal
lateral relocation
settle at close range

Keep constant:
- body;
- camera orientation;
- relevant generator configuration;
- render resolution;
- LOD settings;
- benchmark duration;
- major rendering toggles.
This must allow before/after performance comparisons.
13. Automated screenshot checkpoints
During deterministic native routes, automatically capture screenshots at useful checkpoints.
For example:
arrival
+0.5 s
+1 s
+2 s
+5 s
+10 s
settled

Name/associate them with benchmark telemetry.
These images will later be useful for:
- terrain detail arrival;
- 3A landform-quality comparison;
- LOD transition regressions;
- visual inspection by humans and AI.
Do not use screenshots as the only acceptance evidence.
14. Automatic bad-frame capture
Add a bounded automatic capture system.
Trigger on conditions such as:
frame CPU > configured threshold
publication > 2 ms
terrain update > threshold
queue age > threshold
convergence stalls unexpectedly

Maintain a small rolling history so when a trigger occurs the capture contains frames before and after the event.
Produce a directory/artifact similar to:
terrain-captures/
  2026-...-bad-frame-0042/
      summary.json
      frames.jsonl
      spans.jsonl
      terrain_jobs.jsonl
      queues.jsonl
      resident_state.json
      screenshot.png

Keep file sizes bounded.
These artifacts must be useful without requiring the interactive UI.
15. Machine-readable output for coding agents
Every important diagnostic visible in the UI must also be available in structured output.
Do not make the UI the only source of truth.
Produce stable, versioned schemas for benchmark/capture results.
At minimum include:
build/git identity
platform/GPU/backend
scenario/configuration

frame timing summary
frame timing series

terrain stage timing
queue count/age
throughput
blocking reasons

memory
upload bytes
generation counts

convergence over time

outlier frames

Avoid gigantic unbounded dumps.
The coding agent must be able to run a benchmark, read the resulting JSON, identify the bottleneck and compare it numerically to the baseline.
16. GPU observability
Add GPU timing using wgpu timestamp queries where supported by the active adapter/backend.
The system must gracefully report unsupported capability rather than breaking execution.
Start with broad timings:
complete GPU frame
terrain GPU work/pass
atmosphere
clouds
major post/render stages

Do not spend this phase instrumenting every draw call.
Also improve wgpu resource/pass/debug labels so external GPU tools see meaningful names such as:
Moon Resident Terrain
Terrain Upload
Terrain Main Pass
Atmosphere
Clouds

This should prepare the project for RenderDoc/PIX/Radeon GPU Profiler/Nsight use without coupling Mundaris to one vendor.
17. General developer UI cleanup
While adding Performance Lab, improve the usability of the existing developer diagnostics UI where necessary.
Do not redesign the entire application UI.
The developer interface should have a clear hierarchy such as:
Scene
Camera
Planet
Rendering
Terrain
Performance Lab
Captures / Benchmarks

Important rules:
- frequently needed status at top;
- advanced diagnostics collapsed by default;
- units always shown;
- clear distinction between current / p95 / max / budget;
- human-readable names rather than internal IDs where possible;
- copy/export diagnostic summary;
- reset maxima button;
- pause profiler button;
- start/stop capture;
- benchmark controls;
- backend clearly displayed;
- no hundreds of ungrouped text labels.
Preserve the main viewport as the dominant element.
18. Keep instrumentation safe and cheap
Measure profiler overhead.
The profiling system itself must not create the pacing problem.
Requirements:
- bounded ring buffers;
- bounded capture history;
- no allocation-heavy operation in per-frame hot paths where avoidable;
- no synchronous disk writes from performance-critical frame code;
- background or deferred artifact serialization;
- no unbounded string creation/logging;
- runtime enable/disable where appropriate.
Include profiler overhead measurements in the completion report.
19. After observability is implemented: DO NOT STOP
Once the instrumentation is trustworthy, immediately rerun the currently failing 2D scenarios.
Use the new data to determine the actual dominant cause of:
1. publication exceeding 2 ms;
2. 157.8 ms warm host frames;
3. minute-scale close-range detail arrival.
Produce an evidence-based bottleneck report.
It must answer:
Where is time spent?
Which queue grows?
Which queue has the oldest work?
Which stage has the lowest throughput?
Which blocker accounts for the greatest waiting time?
Are expensive operations being retried?
Are merge/split handoffs causing local or global serialization?
Are already-prepared results waiting unnecessarily?
Is boundary preparation actually the dominant bottleneck?
Does publication cost come from adoption itself or preparation hidden inside it?

Do not assume the previous suspected causes remain correct if measurements contradict them.
20. Then finish 2D using the evidence
After identifying the measured bottlenecks, continue implementation and fix them.
Likely solution classes may include, but must be justified by profiling:
- bounded parallel boundary preparation;
- separating preparation from adoption;
- caching prepared publication artifacts;
- avoiding repeated preparation of blocked candidates;
- skipping blocked candidates rather than stopping the publication loop;
- making transition blocking local rather than global;
- improving publication candidate indexing;
- eliminating avoidable scans;
- reducing main-thread state mutation;
- splitting oversized adoption work;
- improving visible-quality prioritization;
- aging/starvation protection.
Do not implement all of these blindly.
Implement only what the measurements support.
21. Required publication model
Aim for this conceptual flow:
REQUEST
   ↓
GENERATE
   ↓
PREPARE BOUNDARIES / DERIVED DATA
   ↓
IMMUTABLE PREPARED RESULT
   ↓
┌─────────────────────┐
│ dependency not ready│ → PARK CHEAPLY
└─────────────────────┘
            ↓ ready
      PUBLICATION QUEUE
            ↓
      CHEAP ADOPTION
            ↓
         RESIDENT
            ↓
         DRAWABLE

The frame thread should not perform expensive terrain construction.
A blocked prepared result should not be rebuilt every frame.
An unrelated ready region should not wait because another region has an active merge.
22. 2D completion acceptance
Phase 2E is not complete until both observability and 2D closure are achieved.
Retain all existing terrain correctness tests.
Required outcomes:
Correctness
- all current terrain quality gates remain passing;
- all existing numerical GPU reconstruction tests remain passing;
- no regression to seams, morphs, fallback, revisions or stale-result handling.
Publication
- frame-thread publication stays within the established 2 ms budget in the accepted native routes;
- report current, p95 and max;
- individual adoption work cannot trivially violate the entire budget.
Host frame
- existing 100 km warm route no longer exceeds the established 100 ms hard failure threshold;
- report p50, p95 and max;
- target should be substantially better than merely 99 ms where reasonably achievable.
Detail arrival
- close-range useful visual detail arrives in seconds rather than minutes;
- report useful-convergence time;
- separately report exhaustive target convergence.
Pipeline
- no unexplained single-worker stage limits overall terrain throughput;
- if a stage is intentionally serialized, evidence must show it is not the bottleneck;
- unrelated regions must make progress while local transitions are active;
- prepared blocked work must not redo expensive construction repeatedly.
Settled scene
After convergence and with unchanged camera/world revisions:
terrain content regeneration = 0
terrain content upload       = 0
unnecessary tile rebuild    = 0

Observability
- human Performance Lab works;
- deterministic benchmark route works;
- automatic failure capture works;
- machine-readable reports work;
- screenshots correlate with benchmark captures;
- profiling overhead is measured and acceptable.
23. Completion report
At the end provide:
1. the architecture of the profiling system;
2. UI screenshots/descriptions;
3. machine-readable artifact schema and example paths;
4. profiler overhead;
5. baseline timings before optimization;
6. measured bottleneck diagnosis;
7. exact 2D changes made as a result;
8. before/after publication p50/p95/max;
9. before/after frame p50/p95/max;
10. before/after generation/boundary/publication throughput;
11. queue-age comparison;
12. blocker-time comparison;
13. useful-detail convergence before/after;
14. exhaustive convergence before/after;
15. memory/upload behavior;
16. all regression/quality results;
17. remaining known limitations;
18. whether terrain transport/infrastructure can now be frozen for Phase 3A;
19. exact command(s) I can use to launch:
    - normal solar-system view;
    - Performance Lab;
    - deterministic Moon benchmark;
    - capture mode.
End with exactly one recommendation:
READY FOR 3A
or
NOT READY FOR 3A — remaining blocker: ...
Do not begin Phase 3A automatically.