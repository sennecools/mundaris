# ADR 0001: Foundation stack

- **Status:** Accepted
- **Date:** 2026-10-01

## Context

Mundaris needs a native Windows/Linux foundation for a future large-scale world-building and simulation application. The bootstrap should validate graphics and editor UI without taking on a full engine's architecture or implementing domain systems early.

## Decision

Use stable Rust (2024 edition) in a Cargo workspace, with `wgpu` for portable GPU access, `winit` for native windows/events, `glam` for mathematical primitives, and `egui` through `egui-winit`/`egui-wgpu` for editor/developer UI. Target Windows and Linux first. Use structured `tracing` diagnostics and only a small blocking helper for initial GPU setup.

Do not use a full game engine: Mundaris needs explicit ownership and boundaries for authoritative world/simulation state versus rendering, and no current requirement justifies an engine framework's additional abstractions or constraints.

## Consequences

The stack is modular and keeps domain crates independent from platform and GPU APIs. Mundaris must compose windowing, rendering, and UI integration itself and maintain compatibility across these dependencies. `wgpu` improves backend portability but does not remove platform-specific driver/runtime requirements. No planet, terrain, simulation, or editor framework is implied by this decision.
