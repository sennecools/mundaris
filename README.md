# Mundaris

Mundaris is a native desktop world-building and planetary simulation project spanning astronomical and local surface scales. Checked reference frames, editable celestial state, deterministic Newtonian gravity, fixed-step orbital integration and minimal celestial debug rendering are implemented. Procedural worlds remain future work.

**Status:** Phase 4 smooth planetary surface LOD is implemented in the connected celestial explorer. Windows headless/release, CPU benchmarks and directed native approach/inspection evidence are recorded in [Phase 4 validation](docs/phase-4-validation.md) and [ADR 0006](docs/adr/0006-planet-surface-topology-and-lod.md). Complete operator/high-DPI/recovery, Linux and current-revision remote CI acceptance remain open. Full-planet CPU preparation misses the review target; see [performance](docs/performance.md). Earlier phase evidence remains preserved. Phase 5 has not begun.

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
cargo run --locked -p mundaris_app -- --gravity-orbits
```

Normal invocation opens the bootstrap panel. `--reference-frames` draws abstract axes and wire boxes attached to an analytically translating/rotating hierarchy, with source-centred rendering, precision diagnostics, pause/seek/reset, a `1e16 m` shared-offset stress mode, continuous approach, paused frame re-expression, and centimetre movement buttons. Begin approach while paused, then select Play. Re-expression and local movement are available while paused. This fixture contains no planet, terrain, or physics simulation.

`--celestial-model` creates Solace, Aurelia and Luma at star/planet/moon-scale magnitudes. Their bounded `-600..600 s` prescribed motion is an analytic validation fixture, not orbital physics. The panel provides rate/reverse/pause, explicit seek/reset, body selection, translating/body-fixed focus and observer re-expression, projection rebuilding, and atomic name/mass/reference-radius edits. Body-local axes are drawn through the generic renderer; distant bodies have textual bearing/distance markers. Reset preserves body IDs, restores fixture properties/state, and selects Aurelia in body-fixed focus at `0 s`, `1x`, paused.

`--gravity-orbits` starts the mutual-gravity Solace/Aurelia/Luma hierarchy at `0 s`,
fixed `h=60 s`, overview, Aurelia selected, `1000x` selected but paused. Resume
explicitly; changing a rate alone does not unpause. The circular oracle is an
explicit replacement fixture with `h=10 s`. Startup fits current physical structure
and instantaneous guides in the dock-excluded viewport. All bodies remain at their
true physical positions and radius; subpixel bodies have derived markers and
collision-laid-out labels.

| Action | Control |
| --- | --- |
| Select | Sphere/marker/displaced label/list; Tab or Shift+Tab |
| Disambiguate coincident markers | Repeat click cycles every candidate; list remains available |
| Smooth focus | F, double-click, Focus, Next/Previous Focus |
| Whole-system overview | Home / Whole System |
| Local overview | Selected Subsystem / Reference + Companions / explicit membership |
| Body/System Orbit | Left drag; multiplicative wheel zoom |
| Unfocus into Free Flight | Escape / Free Flight, preserving displayed pose |
| Editor free movement | WASD, Q/E vertical, right-drag look; Shift temporary boost |
| Free-flight speed | Logarithmic multiplier / wheel powers of two |

Body zoom controls **clearance above the real reference sphere**, with a one-metre/
ULP floor and smooth wall-time response. Aurelia/Luma now use normalized cube-sphere,
balanced stitched surface patches when projected curvature requires them. The surface
is a smooth zero-height sphere. Navigation uses one high-precision observer; moving or
focusing it changes no world state. Physical spheres, markers, labels and curves are
separate products. Dashed curves are instantaneous two-body **orbit guides**, not
full N-body predictions; solid age-fading curves are **actual committed history**.

The panel distinguishes requested/authoritative time, measured achieved rate,
pending ticks/time, work per update, lag and overload in a prominent toolbar.
Requested rate presets include 10000x and a finite custom rate. All rates remain
baseline exact full N-body KDK with the configured physical step. At most 512 work
units run per update in at-most-32-unit chunks, starting with one unit and checking
a 4 ms interactive budget between chunks. Exceeding the 65,536-tick admission cap halts new demand visibly while
retained debt drains at unchanged h. Pause cancels debt. Reverse restores bounded
snapshots or privately replays positive steps from the branch baseline. Seek shows
quantization before confirmation; replay can be cancelled without changing live
state. Reset preserves IDs/edited baseline/focus; loading an original fixture is
separate. Mass/radius/velocity edits start a paused new branch; names retain history.
Trails are actual committed history, inertial or explicitly simultaneous
body-relative history. They are cleared on branch/seek/direction/physical edits;
presentation/reference switches preserve synchronized records. Automatic guide
references are conservative derived relationships, with explicit pair overrides
and honest unavailable/unbound diagnostics; they never change gravity or parenting.

Minimize/occlusion/suspension excludes hidden demand. A drawable wall gap greater
than the default 250 ms is rejected entirely, cancels debt/navigation progression,
and pauses until Resume, with a visible diagnostic. The session threshold is
explicitly adjustable. There is no hidden sleep catch-up or approximate time-warp
mode. Rate samples report their actual wall window and tick-limited quantization.

## Planet surface inspection

Focus Aurelia/Luma, open **Planet surface / inspection**, set astronomical or exact
clearance targets, or run the continuous 30 s approach to 2 m. **I** explicitly
attaches the same observer to the rotating fixed frame; right drag looks locally,
WASD/QE move at clearance-scaled editor speed and **H** looks toward the tangent
horizon. Look-at body controls and single physical steps observe the independent
moon/star. The reference-sphere clearance guard is navigation, not collision/walking.

Normal mode is a plain analytically shaded sphere. Optional borders/LOD/face/bounds
views, pointer patch addressing and readiness/cache/quality counters expose the
derived structure. Far/surface ownership is exclusive per BodyId; no scene switch.

For a reproducible native route through the same implementation, set
`$env:MUNDARIS_PHASE4_VALIDATE='1'` in PowerShell before running `--gravity-orbits`.
Omit/unset it for normal paused startup. UI also exposes **Run integrated validation
route**. No procedural terrain, noise, atmosphere or collision is implemented.

## Quality checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

CI uses `--locked` for Cargo compilation/lint/test commands to enforce the committed `Cargo.lock`. Tests remain headless; they do not create windows or GPU devices. For interactive validation, run the app and check the panel, resize, minimize/restore, and clean exit. Compilation alone does not establish those runtime behaviors.

## Workspace map

- `mundaris_app` — process entry point, native event loop, logging, and composition.
- `mundaris_renderer` — observer-relative precision, forward-depth debug lines, reverse-Z celestial spheres/surface patches/trails, `wgpu` and `egui` integration.
- `mundaris_core` — reserved for small, genuinely shared foundations.
- `mundaris_math` — finite SI coordinate/time values, rigid rotations/transforms, transactional frame trees, LCA conversions and instantaneous kinematics.
- `mundaris_world` — authoritative append-only celestial bodies, coherent system state/time and disposable body-to-frame projection.
- `mundaris_simulation` — deterministic serial f64 gravity/KDK, fixed-step demand/backlog, snapshots/replay, and conserved-quantity diagnostics.
- `docs/` — architecture constraints, development standards, performance policy, roadmap, and ADRs.

## Focused validation and benchmarks

```bash
cargo test --locked -p mundaris_math -p mundaris_renderer --release
cargo bench --locked -p mundaris_math --bench reference_frames
cargo bench --locked -p mundaris_renderer --bench view_preparation
cargo test --locked -p mundaris_world -p mundaris_simulation -p mundaris_math --release
cargo bench --locked -p mundaris_world --bench celestial_system
cargo bench --locked -p mundaris_world --bench frame_projection
cargo test --locked -p mundaris_simulation --release --test orbits long_run -- --ignored --nocapture
cargo bench --locked -p mundaris_simulation --bench gravity
cargo bench --locked -p mundaris_simulation --bench fixed_steps
cargo bench --locked -p mundaris_renderer --bench celestial_preparation
cargo bench --locked -p mundaris_app --bench trail_history
cargo bench --locked -p mundaris_app --bench celestial_navigation
cargo bench --locked -p mundaris_app --bench orbit_guides
cargo bench --locked -p mundaris_renderer --bench planet_surface
cargo bench --locked -p mundaris_app --bench planet_surface_approach
```

Benchmarks are CPU-only, use Criterion, and stay outside normal CI. Large 512-step
1024-body probes take several seconds per batch and extend Criterion's measurement
time. Workloads, distributions and limitations are in [performance notes](docs/performance.md).
Decisions are in [ADR 0002](docs/adr/0002-reference-frames-and-precision.md),
[ADR 0003](docs/adr/0003-celestial-domain-and-time.md),
[ADR 0004](docs/adr/0004-gravity-integration-and-playback.md), and
[ADR 0005](docs/adr/0005-celestial-navigation-system-view-and-timewarp.md), and
[ADR 0006](docs/adr/0006-planet-surface-topology-and-lod.md).

## Repository status

This is proprietary, private software. No license is granted to use, copy, modify, or redistribute this repository. No open-source license is provided.
