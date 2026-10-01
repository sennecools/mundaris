# Phase 2 validation record — 2026-10-01

## Status

Celestial domain/time/projection and the `--celestial-model` engineering fixture
are implemented. Windows automated debug/release checks and native startup/close
pass. **Full Definition of Done remains open for the complete Windows visual
sequence, Linux native quality/release/interactive evidence, and current-change
remote CI.** Phase 1's pending cross-platform acceptance also remains open.

Requirements: [Phase 2 specification](../MUNDARIS_PHASE_2_CELESTIAL_MODEL_AND_TIME.md).
Decisions: [ADR 0003](adr/0003-celestial-domain-and-time.md).
Measured baseline: [performance](performance.md#phase-2-baseline--2026-10-01).

## Implemented scope

- Math: checked finite `SimulationInstant` in seconds relative to a working epoch.
- Simulation: finite unrestricted rate, explicit pause/seek and transactional
  monotonic-duration scaling; requested time only, no world evolution.
- World: namespaced stable append-only IDs, 1–128-byte UTF-8 names, positive
  finite mass/reference radius, checked f64 position/velocity/orientation/spin,
  coherent system time, checked revision, independent property/state edits.
- State batches: arbitrary caller ordering, wrong/unknown/duplicate rejection,
  atomic commits. Changing time requires all bodies. Same-time subset edits are
  permitted. Reusable dense scratch supplies O(U) work without hot allocation.
- Projection: one translating root child and rotating child per body, read-only
  tree exposure, exact shared sample time, represented revision, live appends
  preserving mappings and rebuilds with fresh caller-supplied tree namespaces.
- App: Solace/Aurelia/Luma fixture, explicit producer → world → frames sequencing,
  engineering diagnostics, rate/reverse/pause/seek/reset, selection/focus,
  kinematic observer re-expression, atomic property drafts and projection rebuild.
  Generic body-local axes and textual distant bearing/distance markers only.
- World Criterion lookup/edit/state/build/full-republish/live-append targets.

No additional workspace crate, renderer domain dependency, unsafe project code,
gravity/orbital solver, persistent serialization, domain taxonomy/primary
hierarchy, deletion, snapshot framework or parallel infrastructure was introduced.
Lockfile changes add only workspace dependency edges; no packages were upgraded.

### Fixture constants and semantics

| Body | Mass kg | Reference radius m | Orientation / spin |
| --- | --- | --- | --- |
| Solace | 1.98847e30 | 6.957e8 | identity baseline, +Y 1e-5 rad/s |
| Aurelia | 5.9722e24 | 6.371e6 | +23.4° about system Z baseline; authored local +Y 0.2 rad/s |
| Luma | 7.342e22 | 1.7374e6 | −0.3 rad about system X baseline; authored local Y −0.07 rad/s |

Centers and analytic velocities use the specification's linear-plus-sine formulas
at `-600..600 s`. Solace center is fixed; Aurelia starts at X=1.5e11 m. Luma's
X=1.5e11+384e6 m and the authored sine phase gives Y=5e6 sin(0.4) m at zero.
Orientation is `q_initial * spin_local(t)` and system angular velocity is
`q_initial * omega_local`, tested independently by rotated-basis differentiation.
Names have no physical behavior. These motions are not orbits.

Selecting a body focuses its body-fixed center from local `(0,0,8) m` with camera
forward −Z; translating focus instead exposes the independent rotating axes.
Re-expression preserves instantaneous pose/physical velocity; focus deliberately
attaches with zero relative velocity. Reset preserves body identity, restores all
fixture names/properties/state, publishes `0 s`, selects Aurelia/body-fixed focus
and sets `1x` paused. Rebuild preserves domain state and local observer data while
remapping debug frame IDs. Seek validates its target before commit. Out-of-bounds
playback requests pause with an error; requested and authoritative times may then
differ. No invalid time or property is silently clamped.

## Numerical and transactional evidence

All tests are headless; no window/GPU is created by ordinary tests. Both debug and
release pass **42 runtime tests and six compile-fail documentation tests** across
the workspace (13 new runtime tests). Existing Phase 1 tolerance budgets remain.

| Coverage | Evidence |
| --- | --- |
| Positive properties / names | finite positive mass/radius accepted; zero/negative/NaN/±infinity rejected; Unicode and 128-byte boundary accepted, empty/over-limit names rejected without mutation |
| Identity and edits | unique IDs including index zero, stable after append/rename, wrong-system/out-of-range rejection, property edits preserve kinematics, state edits preserve properties |
| Batch transactionality | foreign ID, duplicate and incomplete new-time batches preserve all bodies/time/revision; reversed-order full batch commits once; duplicate scratch reusable after failure |
| Revision failures | forced private revision overflow rejects insertion/editor/state mutation, retaining records/time/count |
| Numeric validity | non-finite vector/quaternion components and non-unit quaternion reject before constructing `BodyState`; zero derivatives valid |
| Time | finite ±time/zero accepted, explicit overflow errors, 1x/10x/0.1x/negative/zero rate, pause/resume, seek/reset and error rollback |
| Determinism | 1,201 integer ticks −600..600 sampled through forward and reverse requests exactly equal direct samples on this target; shuffled seek/reset/replay samples equal pure fixture samples |
| Analytic velocity | independent central differences at five ±times, h=0.001 s, error <1e-5 m/s for ~1e7 m excursions |
| Analytic spin | rotated-basis derivative equals omega cross basis within 2e-9 for h=0.001 s, allowing central-difference truncation at 0.2 rad/s |
| Projection topology | expected parent/depth, identity anchor rotation/zero spin, fixed zero translation/linear derivative, exact copied orientation and derivatives |
| Precision | center copies at 0/6.371e6/1.5e11/1e16 m and velocities are component-identical f64 values; no narrowing |
| Spin independence | changing A spin leaves A center, B frame transform and B local point in system coordinates exactly unchanged |
| Publication | exact system sample time/represented revision, property-only edits leave transforms unchanged, wrong-system failure retains published state/revision, rebuild transforms equal despite distinct frame IDs |
| App/editor | invalid parsed mass/radius/name drafts retain the entire body/revision; valid edits preserve state; seek/focus/re-expression/rebuild/reset preserve ownership/identity |
| Local observer | fixed-frame origin view displacement remains exactly `(0,0,-8) m` at −600/0/600 s; translating↔fixed pose round trip <1e-12 m |

`BodyState::new` is infallible because all four inputs are already checked private
math types. The API does not accept invalid raw states and then substitute defaults.
Projection capacity/revision and numeric preflight occur before topology mutation;
private frame ancestry guarantees append insertions remain valid. Publication
records the world revision only after the ordered Phase 1 batch succeeds.

## Windows quality evidence

Stable Rust 1.98.1 (`48a229cea`), Rust 2024, x86_64-pc-windows-msvc. All pass:

```bash
cargo build --locked --workspace
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked -p mundaris_world -p mundaris_simulation -p mundaris_math -p mundaris_app -p mundaris_renderer --release --all-features
cargo doc --locked --workspace --all-features --no-deps
git diff --check
```

Rustdoc uses `RUSTDOCFLAGS=-D warnings`. Both world benchmark targets compiled and
ran with the parameters recorded in performance notes. Six-package non-publishable
workspace, original lint policy and domain-agnostic renderer remain intact.

## Native Windows evidence and remaining visual sequence

Windows 11 Pro x86-64 10.0.26200, AMD Radeon RX 9070 XT, driver 32.0.31041.1004.
Native startup selected **Vulkan**. The debug `--celestial-model` executable stayed
running with a Mundaris window for 15 seconds, accepted a normal window-close
request and logged clean shutdown. No shader/pipeline/presentation error occurred.
Existing driver/loader startup warnings: missing Khronos validation layer,
registry layer-manifest lookup and duplicate AMD switchable-graphics layer.
This startup/close check does not establish visual correctness of every control.

Run `cargo run --locked -p mundaris_app -- --celestial-model` and record these
observations on each target desktop before full acceptance:

1. Inspect all three bodies and system/projected revisions at `0 s`.
2. Focus Aurelia translating, run 1x then 100x, inspect spinning axes and independent
   Luma bearing/position; focus body-fixed and confirm stable local axes.
3. Pause, seek negative/positive times, reverse at −1x/−100x and revisit a known
   explicit instant. Reset and confirm `0 s`, restored properties and paused focus.
4. Select each body; use both focus modes and translating/body-fixed re-expression.
5. Apply valid name/mass/radius edits; reject NaN/zero/negative/overlong drafts and
   confirm authoritative properties/state/revision do not change on failure.
6. Rebuild projection; inspect changed runtime IDs and unchanged body identity/state.
7. Resize, minimize/restore, then close normally. Record OS/GPU/backend and findings.

## Open acceptance items

- Full Windows visual sequence above has not been operator-verified for Phase 2.
- Linux native quality/release and visual checks remain unverified. Available
  Ubuntu WSL2 reports x86-64 kernel 6.18.33.2-microsoft-standard-WSL2; `cargo`,
  `rustc`, `gcc` and `pkg-config` are absent from its login-shell PATH. No Linux
  success or timing result is inferred from Windows.
- Current-change Linux-quality/Windows-compatibility CI has not run. Existing
  workflow covers workspace targets, but these working-tree changes have not been
  committed/pushed. Remote acceptance must use the normal repository workflow.
- Phase 1's Linux/remote acceptance remains a prerequisite for full Phase 2 signoff.

## Required implementation-review answers

1. **Instant ownership:** math holds the checked seconds value; world and playback
   depend on math, and simulation can later depend on world without a cycle.
2. **Authoritative translation/rotation:** world `BodyState` holds system center,
   system velocity, body-to-system unit quaternion and system-axis angular velocity.
3. **Disposable tree:** yes; rebuilding leaves all IDs, body records/time/revision
   untouched and yields equivalent transforms.
4. **Body-owned authoritative frame IDs:** no; only projection associations hold them.
5. **Accidental moon spin ancestry:** no; every translating anchor is a root child.
6. **Mass/radius edits move bodies:** no; editor/property APIs do not touch state.
7. **Failed batch leaves mixed times:** no; validate all IDs/duplicates/completeness
   before committing any state/time/revision.
8. **UI/renderer bypass validation:** no; world fields are private, inspection/tree
   access is immutable, and all editor commands invoke validated world APIs.
9. **Clock evolves bodies:** no; the analytic producer explicitly samples and
   commits authoritative world state, then publishes its projection.
10. **Phase 3 readiness:** yes; dense `bodies()` enumeration supplies mass/position/
    velocity and `update_states` accepts a new coherent full-state batch without
    changing ownership. Spin and local attached content remain independent.

Deferred gravity/integration/catch-up/seek, barycentres, persistent IDs/deletion,
shape/terrain/rendering and simulation snapshots remain deferred.
