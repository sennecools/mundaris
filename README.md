# Mundaris

Mundaris is planned as a native desktop world-building and planetary simulation application, designed to span astronomical and local surface scales. Checked reference frames, an editable celestial body model and explicit simulation-time control are implemented. Procedural worlds and gravitational evolution remain future work.

**Status:** early private development — Phase 2 celestial model/time implementation is present. Windows automated checks and native startup/close pass; full visual, Linux and current-change remote CI acceptance remain open. See [Phase 2 evidence](docs/phase-2-validation.md) and [Phase 1 evidence](docs/phase-1-validation.md).

The initial native development targets are **Windows x86-64** and **Linux x86-64**.

## Prerequisites

- Stable Rust with the `rustfmt` and `clippy` components (the included `rust-toolchain.toml` requests them).
- Native graphics drivers and a desktop session to run the interactive application.
- On Linux, native development packages for X11/Wayland and Vulkan. For Ubuntu/Debian, the same packages used by CI can be installed with:

  ```bash
  sudo apt-get install libvulkan-dev libwayland-dev libxkbcommon-dev \
    libxkbcommon-x11-dev libx11-dev libx11-xcb-dev libxcb-randr0-dev \
    libxcb-xfixes0-dev libxcb-shape0-dev libxcb-xkb-dev pkg-config
  ```

## Build and run

```bash
cargo build --workspace
cargo run -p mundaris_app
cargo run --locked -p mundaris_app -- --reference-frames
cargo run --locked -p mundaris_app -- --celestial-model
```

Normal invocation opens the bootstrap panel. `--reference-frames` draws abstract axes and wire boxes attached to an analytically translating/rotating hierarchy, with source-centred rendering, precision diagnostics, pause/seek/reset, a `1e16 m` shared-offset stress mode, continuous approach, paused frame re-expression, and centimetre movement buttons. Begin approach while paused, then select Play. Re-expression and local movement are available while paused. This fixture contains no planet, terrain, or physics simulation.

`--celestial-model` creates Solace, Aurelia and Luma at star/planet/moon-scale magnitudes. Their bounded `-600..600 s` prescribed motion is an analytic validation fixture, not orbital physics. The panel provides rate/reverse/pause, explicit seek/reset, body selection, translating/body-fixed focus and observer re-expression, projection rebuilding, and atomic name/mass/reference-radius edits. Body-local axes are drawn through the generic renderer; distant bodies have textual bearing/distance markers. Reset preserves body IDs, restores fixture properties/state, and selects Aurelia in body-fixed focus at `0 s`, `1x`, paused.

## Quality checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

CI uses `--locked` for Cargo compilation/lint/test commands to enforce the committed `Cargo.lock`. Tests remain headless; they do not create windows or GPU devices. For interactive validation, run the app and check the panel, resize, minimize/restore, and clean exit. Compilation alone does not establish those runtime behaviors.

## Workspace map

- `mundaris_app` — process entry point, native event loop, logging, and composition.
- `mundaris_renderer` — checked observer-relative CPU preparation, debug line drawing, `wgpu` surface and `egui` integration.
- `mundaris_core` — reserved for small, genuinely shared foundations.
- `mundaris_math` — finite SI coordinate/time values, rigid rotations/transforms, transactional frame trees, LCA conversions and instantaneous kinematics.
- `mundaris_world` — authoritative append-only celestial bodies, coherent system state/time and disposable body-to-frame projection.
- `mundaris_simulation` — checked requested-time playback control; future evolution of authoritative state.
- `docs/` — architecture constraints, development standards, performance policy, roadmap, and ADRs.

## Focused validation and benchmarks

```bash
cargo test --locked -p mundaris_math -p mundaris_renderer --release
cargo bench --locked -p mundaris_math --bench reference_frames
cargo bench --locked -p mundaris_renderer --bench view_preparation
cargo test --locked -p mundaris_world -p mundaris_simulation -p mundaris_math --release
cargo bench --locked -p mundaris_world --bench celestial_system
cargo bench --locked -p mundaris_world --bench frame_projection
```

Benchmarks are CPU-only, use Criterion, and stay outside normal CI. Workloads, hardware, results, and numerical limitations are in [performance notes](docs/performance.md); architectural decisions are in [ADR 0002](docs/adr/0002-reference-frames-and-precision.md) and [ADR 0003](docs/adr/0003-celestial-domain-and-time.md).

## Repository status

This is proprietary, private software. No license is granted to use, copy, modify, or redistribute this repository. No open-source license is provided.
