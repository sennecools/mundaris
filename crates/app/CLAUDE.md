# astrum_app: composition root, Studio, dev interface

Loaded automatically when you work in this crate. Map notes:
`D:/Astrum/Astrum/systems/` (Studio editor, Dev interface, Camera and
navigation, Terrain LOD and atlas, Test solar system, Profiling).

## Owns
The native process and event loop, composition of world/simulation/renderer,
and session state that is disposable. Main groups:
| Area | Files |
| --- | --- |
| Runtime host | `main.rs`, `lib.rs`, `gravity_orbits.rs` + `gravity_orbits/` |
| Studio UI (egui) | `studio_ui/`, `studio/`, `planet_editor.rs`, `profiler_ui.rs` |
| Camera and navigation | `celestial_camera.rs`, `surface_anchor.rs`, `system_view.rs`, `celestial_selection.rs`, `celestial_labels.rs` |
| Terrain runtime | `planet_lod/` (`producer.rs`, `select.rs`, `collision.rs`, `hydrology.rs`, `tier_a.rs`) |
| Solar system | `solar_system.rs`, `shared_system.rs`, `orbit_guides.rs`, `trails.rs`, `interactive_clock.rs` |
| Lighting and sky setup | `scene_lighting.rs`, `render_settings.rs`, `sky_definition.rs` |
| Dev interface (astrum-dev) | `developer_*.rs`, `bin/astrum_dev.rs` |
| Profiling | `profiler.rs`, `engine_profile.rs`, `performance_capture.rs`, `profile_export.rs` |

## Never
- Don't move domain truth into the app; the app composes, world/simulation own.
- Don't add alternate demo scenes: one shared test solar system
  (`repo/content/test-solar-system.json`) for the user and agents.
- Never rewrite `repo/content/lighting.json` (user-owned).

## Rules that bite
- Native runs: `scripts/shared-test-system.ps1` with Windows PowerShell 5.1,
  started from `D:/Astrum`. pwsh 7 writes a receipt that breaks the user's launch.
- `--dev-interface` enables the astrum-dev MCP; after rebuilding `astrum_dev`,
  copy it to `D:/Astrum/target/mcp/`. The protocol is a Coordinator seam.
- GPU tests (`tests/gpu_*`) return early without an adapter, so a pass on a
  machine without a GPU proves nothing. Slow ones are `#[ignore]` (run with
  `--ignored` when you change what they cover). `developer_*` tests and the
  `astrum_dev` binary need `--features developer-tools`.
- Benchmarks are serialized: take `D:/Astrum/ai/lanes/locks/bench.lock`.

## Ownership (parallel lanes)
Studio lane: `studio_ui/`, `studio/`, `planet_editor.rs`, `profiler_ui.rs`;
`gravity_orbits/` is a Studio seam. Terrain lane: `planet_lod/` and
`examples/terrain_*`. `developer_*.rs`: Coordinator. Rest: ask the Coordinator.

## Test
`cargo test -p astrum_app --features developer-tools`; full matrix:
`scripts/validate.ps1` (add `-IncludeGpu` for native checks).

## Contracts
`docs/STUDIO_UI.md`, `docs/AI_DEVELOPMENT_INTERFACE.md`,
`docs/CAMERA_NAVIGATION.md`, ADRs 0005, 0008, 0017.
