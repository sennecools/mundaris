# Mundaris — Project Bootstrap Specification

> **Purpose:** This document is the complete initialization brief for the Mundaris repository.
>
> **Current phase:** repository/bootstrap only.
>
> **Do not begin implementing the actual planetary engine, terrain system, gravity simulation, LOD system, procedural generation, vegetation, water, tectonics, or world editor in this task.** The goal is to create a clean, compilable, maintainable foundation that can support those systems later without prematurely committing to implementation details.

---

# 1. Project Identity

**Working product name:** `Mundaris`

**Repository name:** `mundaris`

**Rust crate prefix:** `mundaris_`

Mundaris is intended to become a native desktop world-building and planetary simulation application. The long-term vision is a seamless tool capable of representing editable moving planets from astronomical scale down to local surface scale, with increasingly detailed procedural terrain, vegetation, biomes, oceans, atmosphere, orbital simulation, and potentially much deeper simulation systems later.

The current task is **not** to build any of that yet.

The current task is to establish a professional private Rust codebase that can grow into that application.

The project may become a commercial product. Treat the repository as proprietary/private software from the beginning.

- Do **not** add an open-source license.
- Do **not** publish crates to crates.io.
- Set workspace crates to `publish = false`.
- Do not add contributor language implying public contributions are expected.
- Do not add generated-by/tooling/vendor provenance to source files, documentation, commit messages, or repository metadata.
- Do not add badges or services that expose a private repository unnecessarily.

---

# 2. Primary Goal of This Task

Initialize a private, production-quality Rust workspace that:

1. builds on stable Rust;
2. targets Windows and Linux natively;
3. uses `wgpu + winit + glam` as the graphics/platform foundation;
4. uses `egui` for editor/developer UI;
5. has clear crate boundaries and dependency direction;
6. has fast formatting, linting, tests, and CI;
7. establishes coding, performance, architecture, and Git conventions;
8. contains only the smallest possible native smoke application needed to prove the stack works;
9. contains documentation for future architectural invariants;
10. makes no attempt to implement the actual engine systems yet;
11. is committed to Git;
12. is pushed to a **private** GitHub repository when credentials/tools permit it.

The result should feel like the beginning of a serious engine/tool codebase, not a tutorial repository and not a giant speculative framework.

---

# 3. Non-Goals for This Task

Do **not** implement any of the following in this bootstrap:

- orbital gravity;
- N-body simulation;
- planet generation;
- planet rendering;
- cube spheres;
- quadtrees;
- clipmaps;
- terrain LOD;
- terrain density fields;
- voxel terrain;
- terrain editing;
- erosion;
- tectonics;
- biomes;
- vegetation generation;
- forests;
- oceans;
- water simulation;
- rivers;
- atmosphere rendering;
- volumetric clouds;
- first-person movement;
- floating origins beyond basic documented abstractions;
- celestial-body ECS/entity systems;
- serialization formats for worlds;
- asset pipelines;
- scripting/plugin systems;
- networking;
- multiplayer;
- world export;
- modding;
- procedural noise implementation;
- shader frameworks beyond the minimum required for a smoke frame;
- editor tooling beyond a tiny bootstrap/debug panel;
- elaborate configuration systems;
- premature task schedulers/job systems;
- custom allocators;
- broad abstraction layers with no current caller.

Document intended architectural boundaries where useful, but **do not implement future systems just because they are listed in the vision.**

---

# 4. Long-Term Engine Invariants

The repository structure must not make these future requirements unnecessarily difficult. These are architectural constraints, not features to implement now.

## 4.1 Seamless scale

The application should eventually support continuous observation from solar-system scale to planetary surface scale without fundamentally treating those scales as unrelated worlds or separate game levels.

The future system may internally transition between representations and reference frames, but those transitions should be implementation details.

## 4.2 Moving celestial bodies

Planets and moons must eventually be able to translate and rotate because they participate in orbital simulation.

Future terrain, vegetation, objects, water, and local observers should therefore use body-local/reference-frame-relative coordinates rather than storing everything directly in a universal world coordinate system.

## 4.3 Hierarchical reference frames

Future architecture must be able to support a hierarchy conceptually similar to:

```text
universe frame
    -> stellar-system frame
        -> celestial-body frame
            -> local surface frame
                -> observer/render frame
```

Do not fully implement this hierarchy yet, but math/domain boundaries must not assume that one global `Vec3` coordinate space is sufficient forever.

## 4.4 Precision policy

Long-distance simulation state will eventually require double precision while GPU/local rendering generally benefits from single precision.

The intended policy is:

- `f64` for authoritative astronomical/simulation-space coordinates and calculations where precision matters;
- `f32` for local GPU-facing render data;
- conversions between them must happen intentionally at clearly defined boundaries;
- never casually downcast astronomical values to `f32` deep inside domain code.

The bootstrap should document this policy and prepare the math crate for it without inventing a large coordinate API now.

## 4.5 Procedural data is authoritative; meshes are caches

Future meshes are disposable render products.

A generated mesh must never become the authoritative representation of terrain or world state.

Eventually the flow should resemble:

```text
world definition + deterministic procedural generation + sparse edits
                              |
                              v
                      requested representation
                              |
                              v
                        temporary mesh/cache
                              |
                              v
                              GPU
```

This is critical for LOD regeneration and terrain editing.

## 4.6 Deterministic procedural generation

Future procedural content must be reproducible from explicit seeds and stable coordinates/identifiers.

Avoid architecture that relies on ambient/global random-number state.

The same untouched region should regenerate identically when unloaded and revisited.

## 4.7 Sparse persistent modifications

A future planet may contain enormous amounts of implicit procedural content. Persistent storage should primarily need to store what differs from procedural generation.

Conceptually:

```text
procedural base + sparse modifications = current world
```

This must eventually apply independently to terrain, vegetation, placed objects, and other editable/generated systems.

## 4.8 Locally editable terrain

The future design should be able to support editing terrain, including changes that cannot be represented by a simple 2D heightfield such as tunnels, caves, overhangs, excavation, craters, and added material.

Do not build the volumetric system now.

However, avoid documentation or APIs that define a planet permanently as only `direction -> height` with no extension path.

A likely future direction is a hybrid representation:

- procedural/global surface representation for most of a planet;
- sparse local volumetric/density-field modifications where necessary.

This is a future design direction, not a bootstrap implementation requirement.

## 4.9 Independent world layers

Terrain, vegetation, water, atmosphere, simulation, and rendering are distinct systems.

Do not bake trees into terrain meshes or make the renderer the owner of world state.

## 4.10 Hierarchical LOD with no conceptual hard world cutoff

The long-term visual goal is not infinite detail. It is continuous reduction of representation quality with distance rather than an obvious arbitrary world cutoff.

An object can disappear as an individual representation only when its contribution has transitioned into a higher-level representation.

Examples for the future:

- individual tree -> simplified tree -> impostor/cluster -> canopy representation -> biome/terrain contribution;
- local terrain -> regional terrain -> continent terrain -> planetary surface representation.

Do not implement this now. Preserve the possibility.

## 4.11 Multi-scale generation

Future planetary features should be generated at meaningful scales rather than by applying one noise function at ever-higher resolution.

Likely conceptual layers:

- planetary/continental structure;
- mountain chains and ocean basins;
- regional terrain;
- local terrain;
- surface detail.

Later tectonic-inspired generation or true tectonic simulation should be able to replace earlier generation stages without requiring the entire renderer to be redesigned.

## 4.12 Observer-relative rendering

Rendering should eventually spend detail around the observer rather than attempting to instantiate the whole world.

The renderer is a view of authoritative state, not the authoritative state itself.

---

# 5. Technology Decisions

These decisions are made for the bootstrap and should not be re-litigated during initialization unless there is a concrete incompatibility.

## Language

- Rust
- stable toolchain
- Rust 2024 edition
- Cargo workspace

Create `rust-toolchain.toml` using the stable channel and ensure `rustfmt` and `clippy` components are installed.

Do not pin to nightly.

Do not use unstable language features.

## Graphics

- `wgpu`
- `winit`
- `glam`

Use current stable, mutually compatible non-prerelease releases available at initialization time.

Prefer crates.io releases over Git dependencies.

Do not use a game engine or rendering engine framework.

Do not use Bevy.

## UI

Use:

- `egui`
- `egui-winit`
- `egui-wgpu`

The bootstrap UI should remain tiny and exist only to prove integration and provide a future editor/debug surface.

## Logging/diagnostics

Use:

- `tracing`
- `tracing-subscriber`

Prefer structured logging over scattered `println!` calls.

`println!` may be used only for tiny command/bootstrap output where structured logging provides no value.

## Errors

Use:

- `thiserror` for reusable/domain/library error types when such errors actually exist;
- `anyhow` only at application/composition boundaries where contextual aggregation is useful.

Do not make every crate depend on `anyhow` by default.

Do not create elaborate error enums for code that does not exist yet.

## Async/bootstrap helper

If required for native `wgpu` initialization, a small helper such as `pollster` is acceptable.

Do not introduce a general async runtime such as Tokio merely to initialize `wgpu`.

## Serialization

Do not add serialization dependencies until there is actual state to serialize.

## Randomness

Do not add a random-number dependency until procedural generation work begins.

When it does begin later, deterministic seeded generation will be required.

---

# 6. Workspace Structure

Create a deliberately small workspace. Do not explode the project into dozens of crates.

Use this initial structure:

```text
mundaris/
├── .github/
│   └── workflows/
│       └── ci.yml
├── crates/
│   ├── app/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs
│   ├── core/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs
│   ├── math/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs
│   ├── world/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs
│   ├── simulation/
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs
│   └── renderer/
│       ├── Cargo.toml
│       └── src/
│           └── lib.rs
├── docs/
│   ├── architecture.md
│   ├── engine-invariants.md
│   ├── coding-standards.md
│   ├── performance.md
│   ├── roadmap.md
│   └── adr/
│       └── 0001-foundation-stack.md
├── .editorconfig
├── .gitattributes
├── .gitignore
├── Cargo.lock
├── Cargo.toml
├── README.md
├── rust-toolchain.toml
└── rustfmt.toml
```

Do not add placeholder directories just to appear comprehensive.

Do not add empty `assets/`, `shaders/`, `examples/`, `benches/`, or `tools/` directories until they are needed.

Git does not track empty directories anyway.

---

# 7. Crate Responsibilities

Crate boundaries matter. Keep dependencies directional and avoid cycles.

## `mundaris_core`

Purpose: small, dependency-light project-wide foundation types and policies that genuinely need to be shared.

It must **not** become a junk drawer.

Appropriate future examples:

- strongly typed IDs;
- shared lightweight configuration primitives;
- common project-level invariants;
- tiny dependency-free utility abstractions used across multiple major systems.

Inappropriate examples:

- rendering code;
- terrain algorithms;
- giant `utils.rs` modules;
- random convenience functions used by one caller;
- platform/window code.

For bootstrap, keep this crate almost empty.

## `mundaris_math`

Purpose: project-specific mathematical conventions and future coordinate/reference-frame primitives.

It may depend on `glam`.

Document the `f64` simulation / `f32` rendering precision policy here.

Do not wrap every `glam` type merely for the sake of wrapping it.

Do not implement full reference-frame machinery yet.

## `mundaris_world`

Purpose: future authoritative world/celestial-domain state.

This crate will eventually describe what exists, not how it is rendered.

It should not depend on `wgpu`, `winit`, or `egui`.

For bootstrap, provide module/crate documentation and no speculative planet implementation.

## `mundaris_simulation`

Purpose: future evolution of authoritative state over time: orbital mechanics, rotation, physical simulation, time stepping, and later simulation systems.

It must not own platform rendering.

It may eventually depend on `mundaris_world`, `mundaris_math`, and `mundaris_core`.

For bootstrap, keep it almost empty.

## `mundaris_renderer`

Purpose: GPU/rendering infrastructure and visual representations.

This is where `wgpu` integration belongs.

It may consume snapshots/views of world state later, but it must not become the owner of authoritative world state.

The bootstrap renderer may initialize `wgpu`, own a native surface-related rendering context as appropriate, clear the frame, and integrate the minimal `egui` pass needed for smoke testing.

Do not add planet rendering abstractions yet.

## `mundaris_app`

Purpose: executable composition root.

Responsibilities:

- native process entry point;
- logging initialization;
- `winit` application/event loop;
- creating/wiring major subsystems;
- top-level error handling;
- minimal bootstrap UI;
- future editor orchestration.

Business/domain logic does not belong here.

---

# 8. Dependency Direction

Aim for a dependency graph similar to:

```text
mundaris_app
    |-- mundaris_renderer
    |-- mundaris_simulation
    |-- mundaris_world
    |-- mundaris_math
    `-- mundaris_core

mundaris_renderer
    |-- mundaris_math
    `-- mundaris_core

mundaris_simulation
    |-- mundaris_world
    |-- mundaris_math
    `-- mundaris_core

mundaris_world
    |-- mundaris_math
    `-- mundaris_core

mundaris_math
    `-- mundaris_core (only if genuinely useful)

mundaris_core
    `-- minimal/no project dependencies
```

Do not force a dependency simply to make this diagram exact.

The important rules are:

- lower-level/domain crates never depend on `app`;
- world/simulation crates never depend on `winit`, `wgpu`, or `egui`;
- platform concerns do not leak into domain state;
- avoid cycles;
- renderer does not own simulation truth.

---

# 9. Root Cargo Workspace

The root `Cargo.toml` should:

- define all workspace members;
- use the current Cargo resolver appropriate for Rust 2024;
- centralize package metadata that is genuinely shared;
- centralize shared dependency versions using `[workspace.dependencies]` where practical;
- centralize lints using workspace lints;
- avoid complicated build scripts;
- avoid release-profile micro-tuning without measurements.

Each crate should use:

```toml
publish = false
```

Prefer inheriting workspace edition/version metadata where appropriate.

Initial package version may be:

```text
0.0.1
```

This is a bootstrap, not a public semantic-versioning promise.

---

# 10. Coding Semantics and Style

These are repository-wide rules.

## 10.1 Clarity over cleverness

Prefer boring, explicit Rust over dense abstractions.

The project is expected to become technically difficult because of the problem domain. The surrounding code should therefore be unusually readable.

## 10.2 Name things by domain meaning

Bad:

```rust
let p = ...;
let val = ...;
let data = ...;
```

Better:

```rust
let body_position = ...;
let terrain_patch = ...;
let observer_origin = ...;
```

Short names such as `x`, `y`, `z`, `dt`, `i`, or `j` are acceptable where mathematically conventional and tightly scoped.

## 10.3 Units must be explicit

Do not leave ambiguous values in APIs.

Prefer semantic names or types that make units obvious.

Examples:

```rust
radius_meters
elapsed_seconds
angular_velocity_radians_per_second
```

As the project grows, consider newtypes for safety-critical/highly reused units. Do not introduce a full units framework during bootstrap unless there is already a real use case.

Never mix kilometers/meters or degrees/radians silently.

## 10.4 Explicit precision

Use `f32`/`f64` deliberately.

Do not use aliases such as a project-wide `type Real = f32` that obscure precision choices.

Precision is architecturally meaningful in this project.

## 10.5 Avoid primitive obsession where identity matters

Future IDs should become strong types rather than raw integers passed everywhere.

For example, later prefer a `BodyId` over an unexplained `u64` when body identity becomes a real concept.

Do not invent dozens of ID types in bootstrap without actual entities.

## 10.6 Ownership should reveal architecture

Prefer clear ownership over pervasive `Rc<RefCell<_>>`, `Arc<Mutex<_>>`, or shared global state.

Only introduce synchronization when concurrency requires it.

Only introduce reference counting when shared ownership is actually required.

## 10.7 No global mutable state

Do not use mutable statics or hidden global registries.

Configuration and state should be passed/owned explicitly.

## 10.8 Keep modules focused

Avoid enormous files and generic `utils.rs` modules.

When a module grows, split by responsibility/domain concept rather than arbitrary line count.

Do not split tiny code into many files prematurely.

## 10.9 Public APIs should be intentionally small

Default to private/module-visible implementation details.

Use `pub` only when another crate/module genuinely needs the item.

Do not expose internals preemptively for hypothetical future callers.

## 10.10 Documentation

Use Rustdoc on:

- public crate purpose;
- important public types;
- invariants;
- non-obvious safety/performance constraints;
- coordinate/precision assumptions.

Do not add comments that merely restate the code.

Comments should explain **why**, constraints, or non-obvious choices.

---

# 11. Error Handling

Use `Result` for recoverable operations.

Avoid `unwrap()` and `expect()` in production paths.

Acceptable exceptions:

- tests where failure should immediately abort the test;
- impossible states that are genuinely proven by local invariants, with a useful `expect` message;
- tiny bootstrap code where API contracts make failure impossible and handling would only obscure the invariant.

Even in those cases, prefer proper propagation when reasonable.

Top-level application startup should produce useful contextual error messages.

Do not silently ignore errors.

Do not log an error and then return the same error unless there is a deliberate reason; avoid duplicate reporting.

---

# 12. Panic and Assertions

Use assertions for programmer invariants, not user/environment failures.

Use `debug_assert!` for expensive checks that only need development validation.

A missing GPU adapter, invalid surface configuration, or failed OS operation is not an assertion; handle it as a runtime error.

---

# 13. Unsafe Code Policy

For the initial repository, use:

```rust
#![forbid(unsafe_code)]
```

in project crates where practical.

Do not add project-owned unsafe code during bootstrap.

If a future optimization or low-level platform requirement genuinely needs unsafe code, isolate it in the narrowest possible module/crate and document:

- why safe Rust was insufficient;
- the exact invariants that make the block sound;
- tests around the boundary;
- measured reason the complexity is justified.

Do not weaken the policy globally merely because a dependency internally uses unsafe code.

---

# 14. Performance Philosophy

Performance is a first-class requirement for Mundaris, but premature complexity is not.

Use the following rules.

## 14.1 Measure before optimizing

Do not add complicated data structures, custom allocators, unsafe code, GPU compute, multithreading, or caching systems without a concrete workload and measurement.

## 14.2 Design hot paths to be optimizable

Even before optimization:

- avoid unnecessary ownership cloning;
- avoid hidden heap allocation in obvious per-frame loops;
- avoid string formatting in hot loops;
- prefer contiguous data when processing large homogeneous sets;
- avoid virtual/dynamic dispatch in tight loops unless justified;
- keep simulation and rendering representations separable;
- avoid lock-heavy designs;
- keep deterministic generation functions pure where practical.

## 14.3 Frame loop allocation awareness

Once rendering work begins, per-frame heap allocations should be treated as something to inspect intentionally rather than accepted blindly.

Do not build a custom allocator now.

## 14.4 CPU/GPU responsibility

Future large-scale procedural rendering should be designed with the possibility of moving suitable highly parallel work to compute shaders/GPU generation.

Do not prematurely move logic to GPU before there is a workload to profile.

## 14.5 Data-oriented design where it matters

Future systems containing millions of similar items may benefit from SoA/packed representations.

Do not turn every domain type into SoA now.

The domain model should not prevent later specialized render/simulation buffers.

## 14.6 Caches are disposable

Caches must never be the only copy of authoritative user edits or simulation state.

## 14.7 Benchmark policy

Do not run benchmarks in normal CI.

When performance-sensitive algorithms begin to exist:

- add focused reproducible benchmarks;
- document hardware-sensitive results carefully;
- compare against a meaningful baseline;
- avoid benchmark suites that take minutes for every developer action.

Do not add empty benchmark scaffolding merely for appearance.

---

# 15. Determinism Policy

Future procedural world generation must be deterministic when given the same:

- project/world version;
- generator version;
- seed;
- coordinates;
- configuration.

When procedural generation is later implemented:

- never depend on system time for world generation unless explicitly requested;
- never use ambient thread-local RNG as authoritative generation state;
- derive local seeds deterministically;
- version generation algorithms when persistence compatibility begins to matter.

Document this now in `docs/engine-invariants.md`.

Do not implement a generator in bootstrap.

---

# 16. Minimal Native Smoke Application

The bootstrap should contain the smallest application that proves the chosen stack works on native desktop.

It should:

1. initialize structured logging;
2. create a `winit` native window;
3. initialize `wgpu` safely;
4. choose an appropriate adapter/device/surface format;
5. react correctly to resize events;
6. clear/present a frame;
7. integrate a tiny `egui` panel;
8. shut down cleanly;
9. compile without warnings.

The window title should be:

```text
Mundaris
```

The tiny bootstrap UI may display only information such as:

```text
Mundaris
Bootstrap environment
Renderer initialized
```

Optionally display adapter/backend information if easy and clean.

Do **not** implement:

- a camera;
- 3D meshes;
- a sphere;
- stars;
- terrain;
- custom lighting;
- simulation time;
- planet sliders;
- editor panels pretending unfinished systems exist.

The smoke app is infrastructure validation, not milestone 1 of the engine.

Use the current non-deprecated APIs of the selected crate versions.

Do not copy an old `wgpu`/`winit` tutorial that relies on outdated event-loop or surface APIs.

Keep window ownership and `wgpu::Surface` lifetime handling explicit and safe. Do not leak the window or use unsafe workarounds solely to avoid thinking through ownership.

---

# 17. UI Boundary

`egui` is an editor/developer UI layer, not the domain model.

Future slider values should modify commands/state through deliberate interfaces rather than allowing UI code to own planets directly.

For bootstrap, only integrate enough UI to prove the stack.

Do not create a generic editor framework yet.

---

# 18. Formatting

Add `rustfmt.toml` with conservative formatting preferences.

Prefer standard Rust formatting. Do not create highly customized formatting rules that surprise contributors/developers.

CI must verify:

```bash
cargo fmt --all -- --check
```

Local formatting command:

```bash
cargo fmt --all
```

---

# 19. Linting

Configure workspace lints in the root Cargo manifest.

At minimum:

- deny unsafe project code where applicable;
- surface unused/must-use issues;
- run Clippy with warnings denied in CI.

CI command:

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Do not enable every pedantic/nursery lint globally just to claim strictness.

If a stricter lint is enabled, it should improve this project rather than create endless noise.

Do not sprinkle broad `#[allow(...)]` attributes across the codebase.

Local lint suppression must be narrow and accompanied by a reason when non-obvious.

---

# 20. Tests

The bootstrap should have a small number of meaningful tests, not fake coverage.

Requirements:

- library crates compile and have test modules available;
- add tests only for real bootstrap behavior/invariants;
- no test should require a GPU, display server, or interactive window;
- renderer/platform smoke execution should not run as a CI unit test;
- future mathematical/reference-frame logic must have strong deterministic unit tests.

CI:

```bash
cargo test --workspace --all-features
```

Keep CI headless-compatible.

---

# 21. Documentation Files

Create useful, concise documentation now so future work has a stable architectural target.

## `README.md`

Include:

- product name;
- one-paragraph vision;
- status clearly marked as early private development/bootstrap;
- supported developer platforms: Windows and Linux;
- toolchain prerequisites;
- build/run commands;
- quality commands (`fmt`, `clippy`, `test`);
- concise workspace map;
- note that the codebase is proprietary/private and is not licensed for redistribution.

Do not create marketing hype.

Do not imply completed features that do not exist.

## `docs/architecture.md`

Include:

- crate responsibilities;
- dependency direction;
- separation of authoritative world state from rendering;
- reference-frame direction;
- `f64` simulation / `f32` local render boundary;
- why meshes/caches are non-authoritative;
- future sparse-edit direction;
- explicit note that most systems are intentionally not implemented yet.

## `docs/engine-invariants.md`

Include the long-term invariants from this brief:

- seamless scale;
- moving bodies;
- hierarchical frames;
- procedural determinism;
- sparse edits;
- locally editable terrain path;
- independent layers;
- hierarchical LOD;
- observer-relative rendering;
- multi-scale generation;
- replaceable algorithms.

This document should describe constraints rather than locking in specific algorithms prematurely.

## `docs/coding-standards.md`

Summarize:

- naming;
- units;
- precision;
- error handling;
- ownership;
- visibility;
- unsafe policy;
- comments/docs;
- dependency discipline;
- no global mutable state;
- no speculative abstractions.

## `docs/performance.md`

Summarize:

- profile before optimizing;
- frame-loop allocation awareness;
- CPU/GPU boundaries;
- data locality;
- caches;
- future benchmarks;
- headroom philosophy;
- determinism vs optimization;
- avoid hidden `O(n^2)` behavior in systems expected to scale.

## `docs/roadmap.md`

Keep this intentionally high-level.

Example phases:

```text
0. Repository/bootstrap
1. Rendering/reference-frame foundations
2. Single static procedural planet
3. Planetary terrain LOD
4. Seamless surface approach
5. Editable terrain foundation
6. Celestial motion/simulation integration
7. Biomes/vegetation representation
8. Water/atmosphere depth
9. World-building/editor workflow
10. Advanced planetary simulation experiments
```

Make clear that phases can change after experiments and profiling.

Do not turn roadmap guesses into promises.

## `docs/adr/0001-foundation-stack.md`

Write a short Architecture Decision Record documenting:

- context;
- decision to use Rust stable + `wgpu` + `winit` + `glam` + `egui`;
- why a full game engine is intentionally not used;
- Windows/Linux-first scope;
- consequences/tradeoffs;
- status: accepted.

Do not write a novel.

---

# 22. Git Ignore Rules

Create an appropriate `.gitignore` for a private Rust/native graphics project.

Ignore at least:

- `/target/`;
- local logs;
- temporary capture/profiling output;
- OS metadata;
- common IDE-local folders/files unless intentionally shared;
- local environment/config files containing machine-specific values or secrets;
- crash dumps;
- generated GPU capture files;
- local benchmark output.

Do **not** ignore `Cargo.lock`.

This is an application/workspace and `Cargo.lock` must be committed.

Never commit secrets, tokens, private keys, credentials, or machine-specific absolute paths.

---

# 23. `.editorconfig`

Create a small `.editorconfig` covering:

- UTF-8;
- LF line endings in repository text files;
- final newline;
- trimming trailing whitespace where appropriate;
- 4-space indentation for Rust;
- 2-space indentation for YAML if desired.

Do not overconfigure editors.

---

# 24. `.gitattributes`

Use `.gitattributes` to normalize repository text line endings to LF while allowing native tooling to work correctly.

Do not commit accidental CRLF churn from Windows development.

Binary files should not be treated as text when assets are added later.

---

# 25. CI Requirements

Create one GitHub Actions workflow at:

```text
.github/workflows/ci.yml
```

Keep it fast.

## Linux quality job

Run on `ubuntu-latest` and perform:

1. checkout;
2. stable Rust toolchain with rustfmt/clippy;
3. any minimal Linux native dependencies required to compile current `winit`/`wgpu` configuration;
4. cache Cargo build data in a sensible way;
5. `cargo fmt --all -- --check`;
6. `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
7. `cargo test --workspace --all-features`.

## Windows compatibility job

Run on `windows-latest` and perform a fast native compatibility check such as:

```bash
cargo check --workspace --all-targets --all-features
```

Do not duplicate the entire slow test suite on Windows unless there is later evidence that it is necessary.

## CI principles

- no GPU/display execution;
- no interactive tests;
- no benchmark execution;
- no release builds on every commit;
- no giant dependency audit suite during this bootstrap;
- no automatic deployment;
- no automatic public release;
- no code coverage service yet.

The workflow should trigger on pushes and pull requests to the main development branch.

---

# 26. Platform Policy

Initial supported native development targets:

- Windows x86-64;
- Linux x86-64.

macOS is not a bootstrap requirement.

Web/WASM is not a bootstrap requirement.

Mobile is not a bootstrap requirement.

Do not distort architecture to support hypothetical platforms before Windows/Linux foundations are stable.

At the same time, avoid unnecessary platform-specific assumptions in domain crates.

Platform-specific code belongs at platform/app/renderer boundaries.

---

# 27. Linux Graphics Windowing

Prefer normal current `winit` Linux support rather than hard-coding the project to one compositor/display protocol without need.

The project should be able to compile for a normal modern Linux desktop environment.

CI may install build packages required for X11/Wayland compilation, but the application must not assume the CI environment can actually open a window.

---

# 28. Dependency Discipline

Every dependency has long-term maintenance and compile-time cost.

Rules:

1. use the standard library first where it is enough;
2. add dependencies only for concrete current needs;
3. prefer mature crates with narrow responsibilities;
4. avoid overlapping libraries solving the same problem;
5. avoid Git dependencies unless necessary;
6. avoid prerelease dependencies for the bootstrap;
7. use workspace dependency declarations where versions must stay aligned;
8. document unusual dependency choices in an ADR if they materially affect architecture;
9. do not add an ECS, physics engine, task scheduler, asset framework, serialization framework, scripting runtime, or database during bootstrap;
10. remove unused dependencies before the initial commit.

Run:

```bash
cargo tree
```

before finalizing to inspect the initial dependency graph for obvious accidental duplication or unexpected heavy dependencies.

Do not obsess over unavoidable transitive dependencies from graphics/UI crates.

---

# 29. Build Hygiene

Before committing:

```bash
cargo fmt --all
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

On a machine with a working native graphical session, also run the application once and verify:

- a window opens;
- `wgpu` initializes;
- the frame presents;
- resize works;
- the tiny `egui` panel renders;
- closing the window exits cleanly;
- normal startup/shutdown does not emit obvious errors.

If graphical execution is impossible in the current environment, do not fake success. Complete the compile/test validation and state that interactive smoke execution could not be performed.

---

# 30. Logging Conventions

Initialize `tracing_subscriber` in the app.

Reasonable default development behavior:

- readable compact logs;
- `info` for lifecycle events;
- `debug` for detailed developer diagnostics;
- `warn` for recoverable abnormal conditions;
- `error` for failures.

Do not log every frame.

Do not log inside future hot loops by default.

Avoid logging secrets, user file contents, or huge debug dumps accidentally.

---

# 31. Configuration

Do not build a full settings system during bootstrap.

If log filtering needs configuration, use normal environment-based `tracing` filtering or a tiny default.

Do not invent a TOML configuration format before actual application settings exist.

---

# 32. Future File/Persistence Compatibility

No world save format exists yet.

Do not create one now.

When persistence is eventually introduced, remember:

- procedural generator versions may affect reproducibility;
- save formats will require explicit versioning/migrations;
- user edits are valuable authoritative data;
- render caches must never be required to restore a world;
- caches should be safely rebuildable.

Document this direction only.

---

# 33. Future Terrain Editing Compatibility

The bootstrap architecture and docs must preserve a path toward terrain editing.

Do not define terrain permanently as a static GPU mesh.

Do not define user edits as vertex edits to a particular LOD mesh.

The intended future conceptual flow is:

```text
edit operation
    -> authoritative terrain modification data
    -> invalidate affected derived caches/LOD nodes
    -> regenerate required representations
```

An edit must survive changing LOD.

Again: document this; do not implement it.

---

# 34. Future Vegetation Compatibility

Vegetation will likely be procedurally generated from environmental fields and deterministic spatial seeds, with sparse overrides for user changes.

A future tree should be able to exist independently from the terrain mesh so that individual trees may be removed/modified without rewriting the planet.

Far-distance forest representations may be aggregate/canopy/biome representations while nearby vegetation resolves into individual instances.

Do not implement vegetation in bootstrap.

Do not create `Tree` structs just to reserve the idea.

---

# 35. Future Biome Compatibility

Avoid a future architecture where biomes are only hard-painted integer regions.

The preferred direction is environmental fields such as temperature, moisture, elevation, slope, latitude, and other factors from which biome/ecological classifications can be derived.

This lets later climate systems replace simpler initial heuristics.

Document, do not implement.

---

# 36. Future Water Compatibility

Initial future ocean rendering may be extremely simple: a planetary sea-level surface with visual shaders.

Actual large-scale fluid simulation is explicitly not an early requirement.

Rivers may later be generated from drainage/hydrology models without simulating every volume of water.

Do not add a water crate now.

---

# 37. Future Tectonics Compatibility

Tectonics is a possible later feature, not an initial dependency.

The architecture must allow procedural terrain generation algorithms to be replaced or augmented by better generation models later.

A future progression might be:

```text
simple procedural terrain
-> multi-scale mountain/continental generation
-> erosion-informed generation
-> tectonic-inspired generation
-> optional actual tectonic/geological simulation experiments
```

The renderer should not care which algorithm ultimately produced authoritative terrain data.

This is precisely why terrain generation and rendering must remain separate responsibilities.

---

# 38. Future Gravity/Orbit Compatibility

Celestial motion will eventually be authoritative simulation state.

Do not make render transforms the owner of orbital state.

Future local features should inherit a body's transform through reference frames rather than individually updating every terrain object/tree when the planet moves.

Conceptually:

```text
body transform in system frame
    + object position in body-local frame
    = render/world transform when required
```

Do not implement orbital physics now.

---

# 39. Replaceability Principle

A major design philosophy for Mundaris:

> Early approximations must be replaceable by higher-fidelity systems without requiring unrelated systems to be rewritten.

Examples:

- simple orbit -> N-body orbit;
- noise terrain -> erosion-informed terrain -> tectonic terrain;
- simple biome heuristic -> climate-driven biome fields;
- sphere ocean -> advanced water rendering;
- coarse vegetation density -> biome/ecology-driven species generation.

This means interfaces should describe required data/behavior, not encode one specific algorithm as the identity of the entire engine.

However, do not create abstract traits for every hypothetical algorithm during bootstrap. Abstraction should appear when there are at least concrete reasons/implementations that need the boundary.

---

# 40. Avoid Premature Generalization

This is important.

Do not build:

- a plugin framework;
- a generic dependency injection container;
- an event bus for everything;
- a reflection system;
- a custom ECS;
- a scene graph;
- a resource manager;
- a generic job graph;
- a serialization registry;
- a shader hot-reload framework;
- a full editor command framework;
- a generic planet-component framework;
- a generic LOD framework;
- a generic simulation graph.

Those may or may not be useful later.

The bootstrap should establish boundaries and quality, not guess the final implementation of every system.

---

# 41. Git Workflow

Initialize Git if the repository is not already initialized.

Default branch should be:

```text
main
```

Use clean conventional-style commit messages without being dogmatic.

Examples:

```text
chore: bootstrap Rust workspace
build: add native CI checks
refactor: clarify renderer ownership
feat: add reference frame primitives
fix: handle surface resize after minimize
```

Do not include tooling provenance in commits.

Do not create noisy commits such as:

```text
update files
changes
fix stuff
```

For this initialization task, prefer a small coherent commit history rather than 20 micro-commits.

A good bootstrap history would be either one polished commit or 2-3 logical commits, for example:

```text
chore: bootstrap Mundaris workspace
build: add formatting linting and CI
```

Only commit after checks pass.

---

# 42. GitHub Requirements

This project must remain private.

## If a GitHub remote already exists

- verify it points to the intended repository;
- do not change repository visibility;
- commit and push `main` normally.

## If no remote exists

If GitHub CLI (`gh`) is installed and authenticated:

1. create a repository named `mundaris`;
2. create it as **private**;
3. use the current local repository as the source;
4. configure `origin`;
5. push `main`.

Never create a public repository as a fallback.

A command conceptually similar to this is acceptable when appropriate:

```bash
gh repo create mundaris --private --source=. --remote=origin --push
```

Verify the actual CLI syntax/environment before executing.

## If GitHub credentials are unavailable

Do not block the repository bootstrap.

- initialize Git;
- make the validated local commit(s);
- do not invent credentials or remotes;
- report clearly that the local repository is ready but could not be pushed because authenticated GitHub access was unavailable.

Do not print or store authentication tokens.

---

# 43. Repository Privacy/Safety

Before pushing, inspect:

```bash
git status
git diff --cached
```

Ensure there are no:

- credentials;
- personal environment files;
- absolute local paths;
- generated build artifacts;
- editor histories;
- terminal transcripts;
- unrelated files;
- large binaries;
- private keys;
- API tokens.

Use `git status --ignored` if useful to verify expected generated files are actually ignored.

---

# 44. README Build Instructions

The README should make local development straightforward.

Expected commands:

```bash
cargo build --workspace
cargo run -p mundaris_app
```

Quality:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Do not claim installation packages/distribution exist yet.

---

# 45. Root Convenience Scripts

Do not introduce Python/Node/Make/Just solely to wrap four Cargo commands during bootstrap.

Plain Cargo commands are sufficient.

A task runner can be added later if the workflow genuinely becomes complex.

---

# 46. Version Control of Toolchain

`rust-toolchain.toml` should specify stable and components required for development/CI.

Do not pin to a nightly date.

The project should track stable Rust unless a future measured requirement proves nightly is necessary.

If stable compiler behavior eventually changes significantly, handle upgrades deliberately through normal maintenance commits.

---

# 47. GPU Backend Expectations

Do not hard-code Mundaris to one GPU vendor.

The renderer must be written against `wgpu` abstractions.

Initial native expectations include normal `wgpu` backends appropriate to Windows/Linux.

Do not force CUDA.

Do not add vendor-specific GPU libraries.

Do not assume NVIDIA hardware.

Keep future compute paths portable where reasonable.

---

# 48. Debugging and Validation

Development builds should be easy to diagnose.

- use structured startup logs;
- report selected GPU adapter/backend at info/debug level if cleanly available;
- report surface/device initialization failures with context;
- handle resize/minimize edge cases without noisy panic loops;
- avoid rendering when the drawable area is zero-sized if the platform requires it.

Do not add a giant debug overlay yet.

---

# 49. Source File Headers

Do not add boilerplate copyright headers to every source file.

Do not add generated-by headers.

Do not add decorative banners.

Crate/module docs are enough.

---

# 50. Naming Conventions

Rust conventions:

- `snake_case` modules/functions/variables;
- `CamelCase` types/traits;
- `SCREAMING_SNAKE_CASE` constants;
- crate names use `mundaris_*` with underscores.

Prefer nouns for data types and verbs for operations.

Avoid abbreviations unless universally clear in graphics/math context (`gpu`, `cpu`, `lod`, `ui`, `id`, `dt`).

Do not abbreviate important domain concepts just to make names shorter.

---

# 51. Future Concurrency Philosophy

Do not add multithreading infrastructure in bootstrap.

Future work will likely include parallel jobs for terrain generation, procedural content, simulation, streaming, and GPU preparation.

When concurrency arrives:

- keep deterministic outcomes where required;
- avoid fine-grained shared mutation;
- prefer immutable inputs + produced outputs/jobs;
- batch work;
- minimize synchronization in frame-critical paths;
- measure scheduling overhead.

Document only.

---

# 52. Future Streaming Philosophy

The world will eventually be too large to materialize fully.

Future systems should request representations based on observer needs and discard/reuse caches when irrelevant.

Streaming should be a property of derived data, not proof that distant authoritative world state ceased to exist.

Do not implement streaming now.

---

# 53. Future Memory Philosophy

Avoid thinking of a world as billions of persistent heap objects.

The intended direction is:

- compact high-level world parameters;
- deterministic procedural reconstruction;
- spatially chunked/region-based derived data;
- sparse user modifications;
- bounded caches;
- aggregate distant representations.

This should influence future design decisions, not produce speculative memory managers today.

---

# 54. Future Editor Philosophy

Mundaris may eventually become a sellable world-building application rather than merely a rendering demo.

The engine should therefore avoid assumptions that everything is controlled only by hard-coded developer parameters.

Long-term editor operations may include:

- changing planet properties;
- changing orbital properties;
- changing procedural generation parameters;
- terrain editing;
- painting/controlling biome/ecological inputs;
- vegetation editing;
- time/simulation controls;
- saving/loading/exporting worlds.

Do not implement these editor systems now.

The bootstrap UI only proves that a native editor UI layer exists.

---

# 55. Commercial-Product Awareness

Because the project may eventually be sold:

- keep third-party dependencies and licenses inspectable;
- avoid copy-pasting incompatible code;
- do not add GPL/AGPL dependencies casually to proprietary runtime code;
- document dependency licensing concerns before introducing anything unusual;
- avoid bundled assets without clear commercial rights;
- do not add an open-source project license to Mundaris;
- keep the repository private.

During bootstrap, prefer permissively licensed mainstream Rust dependencies normally compatible with proprietary applications.

Do not create a full legal compliance system yet.

---

# 56. Workspace Lint/Policy Suggestions

Use sensible workspace lint configuration rather than per-file duplication.

Suggested philosophy:

```text
unsafe code: forbidden for our initial crates
warnings in CI: denied through command line
missing docs: not globally denied yet
pedantic clippy: selective, not blanket
```

Do not turn bootstrap into a fight with style lints.

The quality bar is clean architecture and zero warnings, not the maximum possible lint count.

---

# 57. Minimal `core`, `math`, `world`, and `simulation` Contents

Do not fill these crates with fake future APIs.

A good bootstrap state may be as little as:

- crate-level Rustdoc explaining responsibility;
- a tiny, genuinely useful constant/type only if required by the smoke app;
- `#![forbid(unsafe_code)]`;
- lint inheritance.

Empty-by-design crates are acceptable when they establish workspace boundaries, provided their docs explain that implementation intentionally begins later.

Do not add `Planet`, `Moon`, `Star`, `TerrainChunk`, `Biome`, `Tree`, or `Orbit` structs yet merely because they will probably exist someday.

---

# 58. Renderer Bootstrap Boundary

The renderer implementation should be just enough to answer:

> Can this repository safely create a native window, acquire a GPU device/surface, render/present frames, resize, and display a minimal UI using the chosen stack?

Once the answer is yes, stop.

Do not continue into engine feature work.

If useful, structure the renderer with a small top-level type such as `Renderer`, but do not introduce a hierarchy of render passes or generic resources before they exist.

Avoid huge `state.rs` tutorial blobs if a clearer module split is easy, but also avoid fragmenting 150 lines into eight files.

Use judgment.

---

# 59. Application Event Loop Semantics

Use the current recommended `winit` application lifecycle for the chosen version.

The app must correctly handle:

- startup/resume as required by API;
- window creation;
- close request;
- resize;
- redraw requests;
- surface errors that require reconfiguration;
- minimized/zero-size window cases;
- normal exit.

Do not create a busy loop that renders as fast as possible without considering redraw/control flow semantics.

For bootstrap, a normal continuous redraw approach is acceptable if cleanly implemented, but document why.

---

# 60. Surface Error Handling

Handle normal `wgpu::SurfaceError` categories deliberately according to current API semantics.

Typical categories may include lost/outdated/timeout/out-of-memory depending on the selected `wgpu` version.

Do not blindly panic on every transient surface error.

Out-of-memory may reasonably be fatal.

Keep implementation consistent with current API behavior rather than assumptions from an older tutorial.

---

# 61. No Fake Abstraction Tests

Do not create tests like:

```rust
assert_eq!(2 + 2, 4);
```

just to prove tests exist.

If there is nothing meaningful to test in a nearly empty crate yet, that is fine.

Compilation, Clippy, and the smoke app already validate the bootstrap.

---

# 62. CI Caching

Use a maintained Cargo/Rust cache action or straightforward GitHub cache configuration if it clearly reduces CI time.

Keep the workflow understandable.

Do not add a complicated cache key system that is harder to maintain than the build itself.

Pin actions to stable major versions normally used in current GitHub workflows.

---

# 63. Security Basics

No network service is needed.

No telemetry is needed.

No analytics is needed.

No updater is needed.

No crash upload is needed.

No account system is needed.

No secrets are needed.

Do not introduce them.

---

# 64. Definition of Done

The bootstrap task is complete only when all of the following are true.

## Repository

- [ ] Git repository initialized or existing repository verified.
- [ ] Default branch is `main`.
- [ ] Root Cargo workspace exists.
- [ ] Six initial crates exist: app, core, math, world, simulation, renderer.
- [ ] Crate names use `mundaris_*`.
- [ ] All crates are `publish = false`.
- [ ] No open-source license was added.
- [ ] `.gitignore`, `.gitattributes`, `.editorconfig`, `rustfmt.toml`, and `rust-toolchain.toml` exist.
- [ ] `Cargo.lock` is committed.

## Technology

- [ ] Stable Rust only.
- [ ] Rust 2024 edition.
- [ ] `wgpu + winit + glam` selected.
- [ ] `egui` integration selected.
- [ ] Structured tracing/logging initialized.
- [ ] No game engine dependency.
- [ ] No unnecessary async runtime.

## Native smoke app

- [ ] Builds on the current development machine.
- [ ] Creates native window when graphical environment is available.
- [ ] Initializes `wgpu`.
- [ ] Presents a frame.
- [ ] Handles resize.
- [ ] Shows tiny `egui` bootstrap panel.
- [ ] Exits cleanly.
- [ ] Contains no planet/terrain/simulation feature implementation.

## Quality

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo check --workspace --all-targets --all-features` passes.
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo test --workspace --all-features` passes.
- [ ] No compiler warnings.
- [ ] No project-owned unsafe code.
- [ ] No obviously unused dependencies.

## Documentation

- [ ] README accurately describes current status.
- [ ] Architecture doc exists.
- [ ] Engine invariants doc exists.
- [ ] Coding standards doc exists.
- [ ] Performance doc exists.
- [ ] High-level roadmap exists.
- [ ] Foundation-stack ADR exists.
- [ ] Documentation does not claim unfinished systems exist.

## CI

- [ ] Linux quality job exists.
- [ ] Windows native compile-check job exists.
- [ ] CI is headless.
- [ ] CI does not run benchmarks/release builds.
- [ ] CI config is valid and reasonably fast.

## Git/GitHub

- [ ] Working tree is clean after final commit.
- [ ] Commit messages are professional and concise.
- [ ] Staged content checked for secrets/unrelated files.
- [ ] If authenticated GitHub access exists, remote repository is verified/created as **private**.
- [ ] `main` pushed to GitHub when possible.
- [ ] If push is impossible, local commit is complete and the blocker is reported plainly.

---

# 65. Final Validation Sequence

Perform this sequence before declaring completion:

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo check --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo tree
git status
```

If a graphical session is available:

```bash
cargo run -p mundaris_app
```

Verify the window manually, close it, then inspect:

```bash
git diff
git status
```

Stage intentionally:

```bash
git add ...
git diff --cached
```

Then commit.

Push only to a verified private GitHub repository.

---

# 66. Completion Report

At the end, provide a concise report containing:

1. what files/crates were created;
2. the dependency/architecture shape;
3. validation commands and whether each passed;
4. whether the native smoke app was actually run;
5. commit hash(es);
6. GitHub remote and whether push succeeded;
7. any unavoidable bootstrap compromises or follow-up items.

Do not propose beginning engine implementation automatically.

Do not continue past the bootstrap scope.

Stop after the repository foundation is complete and validated.

---

# 67. Guiding Principle

When uncertain during this initialization, use this priority order:

1. preserve future correctness and architectural flexibility;
2. keep authoritative simulation/world state independent from rendering;
3. keep the code understandable;
4. keep the dependency graph small;
5. keep performance-sensitive future paths possible;
6. avoid speculative abstractions;
7. leave advanced engine work for later milestones.

Mundaris will be difficult because its eventual scale is difficult. The bootstrap should make the difficult future work easier, not make the repository complicated before that work even begins.
