# Phase 5.13A — Analytic celestial motion handoff

## Goal

Implement the user-approved [Phase 5.13A foundation](../MUNDARIS_PHASE_5_13A_ANALYTIC_CELESTIAL_MOTION.md):
complete direct-time, checked prescribed orbital/spin sampling in independent
namespaced systems. Preserve Newtonian motion and ordinary app/rendering behavior.
The user/reviewer retains product and acceptance authority.

## Result

**PASS for the scoped foundation against the recorded Windows/MSVC criteria.**
Focused numerical/regression checks and all 11 final quality commands pass on the
same fingerprinted engine source. Reviewer acceptance remains separate. No visual acceptance is claimed;
default integration, distant rendering and universe addressing/streaming are not
implemented by this phase. Per-criterion evidence below applies only to the stated
fixtures and supported numerical envelope.

| Criterion | Result | Evidence |
| --- | --- | --- |
| Circular/eccentric motion | PASS | Circular cardinal phases; closed-form periapsis/apoapsis; independent 80-decimal quarter-period oracles for e=0.6/0.95. Scales 1 m, 1000 m, 1.5e11 m. |
| Consistent derivatives | PASS | Independent five-point derivative; closed-form velocities; star/planet/moon parent velocity included. |
| Correct spin | PASS | Tilted nontrivial local axis, zero and signed rates; Rodrigues orientation and angular derivative; exact-f64-input signed millennium spin decimal oracle. |
| Correct hierarchy | PASS | Reordered body/definition insertion, cycles/foreign/missing/duplicate rejection; changing parent spin leaves child translation/velocity identical. |
| History-independent sampling | PASS | Requested T, other times, then T; all 13 state components compared by f64 bits on this build/platform; failure followed by successful complete sample. |
| Bounded direct seeking | PASS | Epoch, +/-1 and +/-1000 Julian years with phase offsets; independent noncommensurate period/epoch millennium oracles; near/distant release benchmark below; no integration/replay path. |
| Transactional failure | PASS | Invalid authoring leaves authority unchanged; forced one-iteration exhaustion, unsupported time/cycles, arithmetic failure, external property revision/appends and foreign publication preserve full states/revision/time. |
| Multi-system boundary | PASS | Independently namespaced systems; one sample leaves the other untouched; no galactic placement required; foreign empty-system binding also rejected. |
| Frame-local precision | PASS | Sampled projection, 1e16 m common center translation, millimetre-scale local attachments converted below shared LCA; exact expected local rotation in exercised samples. |
| Existing behavior preserved | PASS against exercised regressions | Existing world/frame, gravity/fixed-step/replay/time tests pass debug/release; math precision/reference-frame tests pass; unchanged app fast capture/interface checks and all final quality commands pass. No human native UX claim. |

No proposed numerical tolerance was loosened after a failure.

## Architecture changes

**IMPLEMENTED:** world's immutable `CelestialMotionDefinition` contains one complete
`BodyMotion` per body, checked `EllipticOrbit` and `AxialSpin`, dense reference
indices and an O(N) iterative parent-before-child traversal. Checked namespaced IDs
and append-only topology bind the definition to a system. Stationary centers are
system-local; an ellipse's XY plane/+X periapsis orientation is in system axes.
Reference translation and velocity are added, never reference axial rotation.
Periods remain authored inputs independent of mass/reference radius.

**IMPLEMENTED:** simulation's concrete `AnalyticMotionProducer` owns the definition,
expected revision and reusable full-state candidates. Elapsed time is reduced by
period before forming mean anomaly. Safeguarded Newton/bisection has a bounded
iteration limit and explicit convergence error; small-anomaly residual and radial
denominator avoid near-parabolic cancellation. Velocity is the analytic derivative,
not differenced state. Spin composes initial orientation with local-axis rotation,
publishing the corresponding angular velocity in system axes. Signed-rate phase
reduction uses FMA/product-error and split 2pi, not an invented rounded spin period.
Stationary spin preserves the initial rotation exactly.

Every candidate is validated before one complete world commit; partially evaluated
scratch after errors is never authoritative and is overwritten on the next sample.
Its own commit advances its expected revision. **Every external celestial revision**
(including metadata/property edits) conservatively requires explicit reconstruction;
terrain-only changes do not. This is stricter than rejecting only physically
incompatible edits, follows current revision ownership, and does not change periods.
Candidate allocation address/capacity remains unchanged across the exercised samples;
source has no evaluation allocations. No global allocator instrumentation is claimed.

Existing mutation/frame APIs, Newtonian code, time controller, default composition,
camera/terrain/renderer/shaders remain unchanged. No generic backend framework,
graphics dependency, persistence/universe address format or streaming API is added.
World-domain authoring is a separately owned immutable value, not an additional
mutable store inside `CelestialSystem`.

## Files changed

Task-owned source/manifest edits:

- `crates/world/src/celestial_motion.rs`, `crates/world/tests/celestial_motion.rs`;
  minimal exports in `crates/world/src/lib.rs`.
- `crates/simulation/src/analytic_motion.rs`, `crates/simulation/tests/analytic_motion.rs`,
  `crates/simulation/benches/analytic_motion.rs`; minimal exports/benchmark target
  in `crates/simulation/src/lib.rs` and `crates/simulation/Cargo.toml`.

Task-owned documentation/evidence: phase specification, ADR 0007, focused sections
of `docs/architecture.md`/`docs/engine-invariants.md`, dated reviewer-context append,
this report and `docs/evidence/phase513a/`.

Implementation ownership: one owner for production source/evaluator/integration;
separate disjoint ownership for the new world test file and two new specification/
ADR documents. Final integration/review retained the agreed architecture. No other
files were intentionally edited, and no existing dirty work was discarded.

## Tests

Raw commands, exit codes and timings: [focused results](evidence/phase513a/final/focused-validation.json),
[fast results](evidence/phase513a/final/fast/validation.json), final full quality directory.

```powershell
cargo fmt --all
./scripts/ai-check.ps1 -OutputDirectory docs/evidence/phase513a/final/fast
cargo test --locked -p mundaris_world -p mundaris_simulation
cargo test --locked --release -p mundaris_world -p mundaris_simulation
cargo test --locked -p mundaris_simulation --test analytic_motion -- --nocapture --test-threads=1
cargo test --locked --release -p mundaris_simulation --test analytic_motion -- --nocapture --test-threads=1
cargo test --locked -p mundaris_math --test precision --test reference_frames
cargo bench --locked -p mundaris_simulation --bench analytic_motion
./scripts/validate.ps1 -OutputDirectory docs/evidence/phase513a/final/full-quality -IncludeGpu
git diff --check
```

Focused package runs: 75 tests pass plus one world doctest per profile; two existing
long-orbit cases are intentionally ignored here and explicitly included by the full
matrix's `long-orbits` command. New coverage is six world integration tests, fourteen
simulation integration tests (including subnormal rejection) and one simulation storage-reuse unit test. Independent
source/numerical review found no actionable issue; it is not a substitute for these
runtime results or cross-platform evidence.

**VERIFIED:** [final full matrix](evidence/phase513a/final/full-quality/validation.json)
records exit code 0 for all 11 commands: formatting, all-target/all-feature workspace
check, all-feature and default-feature Clippy with warnings denied, debug/release
workspace tests, documentation, long-orbit regressions and three explicit GPU
regressions. Workspace runs each report 297 passing tests including doctests and
five ignored cases. Separate commands execute those five cases: two long-orbit
tests and one each for close-surface, full-frame and developer-interface GPU checks.
[Raw test counts](evidence/phase513a/final/full-quality/test-counts.json) accompany
the command logs. These GPU checks do not establish human native-window interaction.

## Measurements

**MEASURED:** Windows 10.0.26200/MSVC, Rust 1.98.1, AMD Ryzen 7 9800X3D, reported
8 cores/8 logical processors, 33,377,583,104 bytes system memory; environment and
dirty-source fingerprint are retained in the [evidence index](evidence/phase513a/README.md).
Julian year = 31,557,600 seconds, not a real-calendar ephemeris.

Independent numerical evidence, identical observed errors in debug and release:

| Check | Maximum absolute error | Maximum normalized error | Bound |
| --- | --- | --- | --- |
| Cardinal/periapsis/apoapsis/quarter orbital position | 9.18701e-5 m at a=1.5e11 m | 6.84486e-16 (a=1000 m fixture) | 1e-11 |
| Corresponding orbital velocity | 1.32425e-11 m/s at scale 2.98653e4 m/s | 4.43406e-16 | 1e-10 |
| Noncommensurate millennium position | 0.0855053 m at a=1.5e11 m | 5.70035e-13 | 1e-11 |
| Noncommensurate millennium velocity | 3.40567e-7 m/s at scale 7.63407e5 m/s | 4.46115e-13 | 1e-10 |
| Independent five-point orbital derivative | 9.42754e-8 m/s at scale 2.98653e4 m/s | 3.15669e-12 | 1e-10 |
| Tilted Rodrigues orientation (test vector) | 4.96507e-16 m at vector scale 2.29129 m | 2.16694e-16 | 1e-11 |
| Spin-derived point velocity | 4.40366e-17 m/s at scale 2.29129e-4 m/s | 1.92192e-13 | 1e-10 |
| Signed millennium spin (unit vector) | 1.11023e-16 | 1.11023e-16 | 1e-11 |

Absolute and normalized maxima need not occur in the same fixture. Raw logs contain
all scales/errors/time samples. Noncommensurate seeking uses exact represented f64
input values in an independent 80-decimal oracle (period 1,234,567.89 s, epoch
1234.56789 s, M0=0.37, e=0.6); remaining f64 elapsed-subtraction rounding is visible
but below the fixed fixture tolerances. Near-parabolic tests establish finite
sampling/periapsis oracle behavior, not a universal bound over all e close to one.

**MEASURED:** complete candidate generation plus world commit, one stationary center
and sibling e=0.6 ellipses with spin, release Criterion; 30 samples/case, 500 ms
warmup, 2 s measurement. Alternates the listed time and time+1234 s; setup excluded.
Table shows slope point estimates and 95% confidence intervals in microseconds:

| Total bodies | Near (7,889,400 s) | +1000 years | -1000 years | Iterations total, near/+/-; max |
| --- | --- | --- | --- | --- |
| 10 | 1.5296 [1.5261, 1.5330] | 1.3134 [1.3095, 1.3168] | 1.3107 [1.3081, 1.3136] | 54 / 46 / 46; 6 |
| 100 | 16.527 [16.493, 16.560] | 14.810 [14.769, 14.858] | 15.132 [14.897, 15.551] | 594 / 523 / 523; 6 |
| 1000 | 157.30 [157.00, 157.60] | 155.83 [155.39, 156.25] | 156.02 [155.37, 156.88] | 5994 / 5276 / 5276; 6 |

Iteration totals describe the first listed instant, not every alternating sample.
Statistical outliers remain in raw results. Near/distant states differ, so this is
bounded seeking cost evidence, not a controlled speedup, equal physical fidelity
comparison, native FPS measurement or full engine performance claim.
Criterion's automatic change output compares the intermediate run; it is not
accepted here as evidence of an optimization improvement. The final absolute
estimates and raw samples are the phase's cost evidence.

## Captures

Final fast PNG/JSON pair is preserved under `docs/evidence/phase513a/final/fast/` and
was inspected together. OBSERVED: unchanged Earth-orbit scene, Vulkan/RX 9070 XT,
paused 0 s/frame 64. Snapshot: `quality_pending=true`, `settled=false`, source LOD 1
versus desired 14. This is a regression/tooling sanity check, not new motion visuals,
settled terrain or native human interaction acceptance. No new visual gate exists
for this headless phase.

## Known failures

No final scoped test or quality command remains failed. The first full matrix failed
Clippy on test-only literal grouping/excess precision; literals were corrected and
the original failed logs retained. A final subnormal guard now rejects a vanished
tangential velocity intermediate instead of publishing a false zero; its rollback
regression and refreshed final-source evidence are recorded separately. This is a
deliberate conservative envelope rejection: finite final mathematical velocity alone
does not admit a fixture whose required intermediate vanishes. Linux/current-revision
remote CI are **UNTESTED**, not verified by local Windows commands. Human native
interaction, terrain responsiveness/Acceptance A, camera and UI product acceptance
remain open from prior phases; this change makes no claim to repair them.

Numerical limits: |requested time|, |each epoch|, |elapsed time| <=2^36 s;
at most 2^32 cycles; explicit M0 within +/-TAU; 0<=e<1; solver default64, limit1..128.
No universal error bound follows from this finite envelope. Arithmetic underflow
that vanishes required orbital scales/velocity intermediates or overflow is rejected; extreme conditioning
may return convergence failure. Parent center addition and time subtraction remain
f64; shared local frames protect attachments, not already flattened distinct-body
centers. Cross-platform bitwise determinism, arbitrary calendar times and arbitrary
galactic offsets are not claimed.

## Evidence

[Evidence index](evidence/phase513a/README.md) links raw tests, independent oracle,
hardware, exact source identity, all nine Criterion measurement sets, paired capture
and full quality logs. Final source fingerprint is from the final/fast run; later
documentation additions do not alter the tested engine build.

## Git state

Branch `main`, HEAD `d81bb2a17ad4f8c1d06eaa1619b6281b946641c3`; task is **uncommitted**.
The checkout already had substantial app/renderer/workflow/manifest dirty and
untracked work before this task. Full dirty status is recorded in the evidence;
HEAD alone does not identify this build. No commit, push, reset or unrelated cleanup.

## Reviewer follow-up

Inspect source/diff and underlying numerical/benchmark/quality evidence against the
phase criteria; source existence or this summary is not acceptance authority.
Discuss default integration next, then visible-universe content and later universe
addressing/streaming. Do not start these phases automatically or declare camera/
terrain/visual acceptance solved.
