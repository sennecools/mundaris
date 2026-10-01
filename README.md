# Mundaris

Mundaris is planned as a native desktop world-building and planetary simulation application, designed to span astronomical and local surface scales. The long-term vision includes editable moving celestial bodies, procedural worlds, and an editor workflow; none of those engine systems are implemented yet.

**Status:** early private development — repository and graphics-stack bootstrap.

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
```

The current app opens a native window, initializes `wgpu`, presents a clear frame, and displays a small `egui` bootstrap panel. It does not contain a planet, terrain, or simulation.

## Quality checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

CI uses `--locked` for Cargo compilation/lint/test commands to enforce the committed `Cargo.lock`. Tests remain headless; they do not create windows or GPU devices. For interactive validation, run the app and check the panel, resize, minimize/restore, and clean exit. Compilation alone does not establish those runtime behaviors.

## Workspace map

- `mundaris_app` — process entry point, native event loop, logging, and composition.
- `mundaris_renderer` — `wgpu` surface and minimal `egui` presentation integration.
- `mundaris_core` — reserved for small, genuinely shared foundations.
- `mundaris_math` — mathematical conventions and future coordinate primitives.
- `mundaris_world` — future authoritative world-domain state.
- `mundaris_simulation` — future evolution of authoritative state over time.
- `docs/` — architecture constraints, development standards, performance policy, roadmap, and ADRs.

## Repository status

This is proprietary, private software. No license is granted to use, copy, modify, or redistribute this repository. No open-source license is provided.
