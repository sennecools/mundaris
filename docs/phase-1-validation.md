# Phase 1 validation record — 2026-10-01

## Status

The Phase 1 implementation and all locally executable automated criteria pass on Windows x86-64. The full Windows visual sequence was reported passed by the operator. **Full Definition of Done remains open for Linux native build/numerical/release/interactive evidence and current-revision remote CI.** Phase 2 implementation is now recorded separately in [Phase 2 validation](phase-2-validation.md); this Phase 1 record preserves its original evidence.

The authoritative requirements are [Phase 1](../MUNDARIS_PHASE_1_REFERENCE_FRAMES.md). [ADR 0002](adr/0002-reference-frames-and-precision.md) records concrete signatures/ownership decisions; [performance](performance.md) contains reproducible workloads, timings and distributions.

## Implemented modules and boundaries

- `crates/math/src/coordinates.rs`: private finite SI wrappers, tagged values, pose, same-frame arithmetic and checked kinematic-point construction.
- `crates/math/src/transform.rs`: checked unit quaternion, explicit composition/inverse, rigid transforms and parent-axis derivative semantics.
- `crates/math/src/frames.rs`: opaque handles, append-only single-root topology, transactional ordered publication/reparenting, coherent borrow, LCA pose and derivative preparation/re-expression.
- `crates/math/tests/{primitives,reference_frames,precision,kinematics}.rs`: analytic, property/depth, invalid-input/topology, source quantization, derivative and scale regressions.
- `crates/renderer/src/view.rs`: source-centred CPU path, representation budgets, checked narrowing and caller-storage batches.
- `crates/renderer/src/debug.rs`, `src/shaders/debug.wgsl`: poisoned full-frame preparation, reusable explicit packing, line/depth resources and projection-only WGSL.
- `crates/renderer/tests/view_precision.rs`: camera axes, million-metre subtraction order, frame/batch failures and common ancestry.
- `crates/app/src/reference_frames.rs`: pure analytic fixture, unchanged primitives, observer/session controls, approach/handoff, bearing markers and replay tests. Main selects the one validation flag and retains native lifecycle.
- Math/renderer Criterion targets, manifests/lockfile, README/current architecture/roadmap/performance/ADR/evidence updated. Core/world/simulation/redraw functionality and existing CI jobs remain as at the preserved baseline.

Current project dependency graph: app → math/renderer; renderer → math. World/simulation have no graphics dependencies. No new crate, unsafe project code, simulation/terrain/domain model, general scene graph, global cache/rebase or prohibited infrastructure exists.

## Numerical results

Debug and optimized release pass the same tolerances. Tests check finite output before error comparisons. Required tolerances were not loosened.

| Coverage | Required bound / recorded result |
| --- | --- |
| Analytic parent/child composition | `(10,22,29) m`, `<=1e-12 m`; correct noncommuting order, +Z/+Y quarter turns |
| Inverse, bounded local/depth sweeps | Points `<=1e-9 m`; rotated basis `<=1e-12`; depths 1/4/8 |
| 100,000 small rotations | Unit squared-norm and orthogonality `<=1e-12`; q/−q orientations tested |
| Invalid inputs | NaN/infinity in physical components/scalars, zero/non-unit quaternion, zero direction, finite overflow, invalid render budget/projection all reject |
| Topology/publication | Wrong/missing handles, root edits, cycle/self-parent, duplicate/unordered update and invalid time reject transactionally; valid reparent preserves descendant local state/IDs |
| Lifetime/type safety | Six compile-fail Rustdoc examples cover point addition, velocity/displacement/render misuse and math/view borrows blocking mutation |
| Body branches around 6.371e6 m | Position `<=1e-7 m`; local/source-centred million-metre subtraction `<=1e-9 m` |
| Root/independent input at 1.5e11 m | Delta/round trip `<=1e-3 m`; independently rounded 0.01 m input explicitly differs from ideal |
| Shared ancestry at 0/1.5e11/1e16 m | Translation/orientation changes cancel within `1e-9 m`; 0.001 m detail survives locally and demonstrably disappears through root flattening at 1e16 m |
| Derivative analytics | Translation/spin/nested nonparallel motion and inverse: local `<=1e-8 m/s`, body `<=1e-6 m/s`; central difference at t±1e-4 s, local coordinates <=10 m, `<=1e-6 m/s` |
| Missing motion | Required branch fails; shared unknown motion and same-frame conversion succeed; failed app migration leaves observer untouched |
| Pose and velocity re-expression | Astronomical `<=1e-3 m`, `<=5e-4 m/s`, basis `<=1e-12`; local/body cases use tighter `1e-7 m`, `1e-6 m/s` |
| Renderer narrowing | Actual component error `<=1e-5 m`/100 m, `<=1e-4 m`/1 km, `<=1e-3 m`/10 km; Euclidean boundary checked; range/overflow/precision errors reject |
| Shader/layout | naga parse/validation; 32-byte vertices with offsets 0/16, position w=1, color; 64-byte column-major projection; positive forward clip w, near→0/far→1 within `2e-6`; right/up signs |
| Repeatability/batches | Identical-query component bits repeat on this build; direct/prepared outputs agree; indexed batch error and failed-frame submission guard |

### Analytic replay and approach

`cargo test --locked -p mundaris_app -- --nocapture` and the optimized app test both report:

- Replay `n/60`, `n=0..36,000`, at **both** shared offsets: **72,002 samples**. Every fixture endpoint is checked, including kilometre marker and centimetre-separated primitives.
- Maximum CPU view-displacement error against independent source-local displacement: **0.000000e0 m** (`<=1e-9 m`).
- Maximum actual component narrowing error: **3.051758e-6 m** (`<=1e-3 m`).
- Source endpoint arrays stay unchanged; independent root observer sees more than `1e6 m` of changed relative position, proving motion is evaluated.
- Full approach sampled at 60 Hz for 30 s. Handoff near `1e5 m`: position **9.551027593537028e-6 m**, velocity **1.91021920573485e-6 m/s**, basis **0**. Subsequent near samples stay regional. Endpoint derivatives are zero; central differences independently validate the trajectory derivative. The approach derivative check has `1e-2 m/s` absolute finite-difference tolerance for its approximately `1e7 m/s` far trajectory; it is separate from the specification's small local-frame derivative fixture, which retains `1e-6 m/s`.
- Paused `0.01 m` local movement passes `1e-9 m`; wrong-tree/unknown-motion migrations retain original observer; reset allocates a fresh namespace.

These fixtures establish the documented envelope, not universal accuracy for arbitrary numbers/depths or already flattened coordinates.

## Build/quality evidence — Windows x86-64

Stable Rust 1.98.1, Rust 2024, six non-publishable packages. The following pass:

```bash
cargo build --locked --workspace
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked -p mundaris_math -p mundaris_renderer --release
cargo test --locked -p mundaris_app --release -- --nocapture
cargo doc --locked --workspace --all-features --no-deps
git diff --check
```

Rustdoc was run with `RUSTDOCFLAGS=-D warnings` supplied in PowerShell. The final headless suite has **29 runtime tests plus six compile-fail documentation tests**; no device/display test is required. Both Criterion targets compile in all-targets checks and have been executed locally. Dependency-feature inspection confirms glam default/std and the intended project direction; there are no unrelated dependency upgrades. Documentation links, final newlines and LF text were reviewed.

## Native application evidence

Windows 11 Pro x86-64 (build 26200), AMD Radeon RX 9070 XT, driver 32.0.31041.1004; startup selected **Vulkan**. A 1280×800 default attached-mode process ran for 65 seconds, stayed running, received a normal window-close request, and logged clean shutdown. No shader/pipeline/presentation error occurred. Driver/loader startup warnings were present: unavailable Khronos validation layer, registry layer lookup and duplicate AMD switchable-graphics layer. Those are recorded environment diagnostics, not silently suppressed or represented as a completely warning-free graphical run.

The operator reported the full Section 15 visual sequence passed on this Windows desktop: 60 seconds attached at each offset, pause/seek/reset, approach/handoff, root/rotating/regional re-expression, centimetre movement, resize/minimize/restore and close; no shimmer/snap or UI/lifecycle problem reported. This visual report supplements the numerical tests. The 65-second startup run separately supplies the observed adapter/backend evidence; no GPU screenshot/pixel comparison is claimed.

## Open platform Definition of Done items

1. **Linux x86-64 native quality and focused release tests:** no Linux result is claimed. The available Ubuntu WSL2 environment reports x86-64 Linux kernel 6.18.33.2 but has no `rustc`, `cargo`, `rustup`, `gcc` or `pkg-config` on its login-shell PATH. A provisioned supported Linux development host is still required.
2. **Linux native interactive sequence:** execute the same Section 15 sequence and record OS/GPU/backend and observations. Windows visual success cannot satisfy this criterion.
3. **Current-revision remote CI:** existing Linux quality and Windows compatibility workflow jobs have not executed for these local commits. Nothing was pushed. Run them through the normal repository workflow and retain the results before full acceptance.

The specification's combined Windows/Linux build and interactive checkboxes remain open. The correctness, architecture and performance review gates are completed for the implemented scope: independent analytic/scale tests, ownership/dependency/lifetime inspection and measured no-cache hot paths. Cross-platform acceptance still gates Phase 2.

## Milestones

The existing working-tree bootstrap hardening/specifications were preserved in a separate baseline commit with approval. Implementation then proceeded in specification order, formatting/testing/linting before each stable milestone:

- `c5ab892` — `chore: record hardened bootstrap and phase 1 specification`
- `da39ad0` — `feat(math): add coordinate and rigid transform primitives`
- `7586d2a` — `feat(math): add hierarchical reference frames`
- `eacbb8b` — `feat(math): add moving-frame kinematics`
- `7c81c4d` — `feat(renderer): add observer-relative precision path`
- `f135cb2` — `feat(app): add reference frame validation mode`
- `402a046` — `test: add scale regressions and CPU benchmarks`
- `5f70fc9` — `fix(app): enforce body-local re-expression budgets`
- Documentation milestone — `docs: record phase 1 reference-frame architecture`
