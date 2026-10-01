# Mundaris

Mundaris is planned as a native desktop world-building and planetary simulation application, designed to span astronomical and local surface scales. The long-term vision includes editable moving celestial bodies, procedural worlds, and an editor workflow; none of those engine systems are implemented yet.

**Status:** early private development — repository and graphics-stack bootstrap.

The initial native development targets are **Windows x86-64** and **Linux x86-64**.

## Prerequisites

- Stable Rust with the `rustfmt` and `clippy` components (the included `rust-toolchain.toml` requests them).
- Native graphics drivers and a desktop session to run the interactive application.
- On Linux, the development packages required by `winit`/`wgpu` for the selected X11/Wayland and Vulkan configuration.

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
