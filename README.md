# Mundaris

Mundaris is a native desktop world-building and planetary simulation project spanning astronomical and local surface scales. Checked reference frames, editable celestial state, deterministic Newtonian gravity, fixed-step orbital integration and adaptive procedural terrain are implemented as development checkpoints.

**Status:** Phase 4 smooth planetary surface LOD is implemented in the connected celestial explorer. Windows headless/release, CPU benchmarks and directed native approach/inspection evidence are recorded in [Phase 4 validation](docs/phase-4-validation.md) and [ADR 0006](docs/adr/0006-planet-surface-topology-and-lod.md). Complete operator/high-DPI/recovery, Linux and current-revision remote CI acceptance remain open. Full-planet CPU preparation misses the review target; see [performance](docs/performance.md). Phase 5 now includes deterministic filtered terrain, analytic erosion/lighting, bounded CPU caching, adaptive displaced stitching and common-refinement morphs. [Phase 5 evidence](docs/phase-5-validation.md) distinguishes implementation from still-open morphology, quality convergence and interactive-performance acceptance; earlier evidence is preserved.

The initial native development targets are **Windows x86-64** and **Linux x86-64**.

**Phase 5.8 content:** ordinary launch opens a ten-body gameplay Solar System
(Sun, all eight planets and Earth's Moon), with **400 km Earth radius / 800 km
diameter**. Radius, gravity, rotation, terrain and orbital configuration remain
independent; Earth-sized/larger engine regression capability is unchanged. See
[scale/system implementation](MUNDARIS_PHASE_5_8_GAMEPLAY_SCALE_AND_SOLAR_SYSTEM.md)
and [validation](docs/phase-5-validation.md). The inherited terrain performance,
quality convergence, morphology and platform acceptance gaps remain open.
The [Phase 5.8 capture index](docs/evidence/phase58/README.md) links retained scenes,
exact manifests and the reproduction command.

**Phase 5.10 checkpoint:** native terrain generation and stitch/morph construction
now use four bounded CPU workers (`MUNDARIS_TERRAIN_WORKERS=0..4` for comparisons),
under the unchanged 128 MiB aggregate cap. Interactive quality convergence still
fails acceptance A; morphology recovery is therefore blocked. See the
[implementation/gate report](MUNDARIS_PHASE_5_10_LOD_CONVERGENCE_AND_MORPHOLOGY_RECOVERY.md)
and [timed evidence](docs/evidence/phase510/README.md).

The [Phase 5.10B recovery checkpoint](docs/phase-5-10b-acceptance-a.md) corrects
certificate interpretation and reduces measured preparation costs, but Acceptance A
still fails: cold useful-quality convergence and operational memory headroom remain
open. [Latest evidence](docs/evidence/phase510b/README.md) separates current probes
and captures from intermediate experiments. Morphology remains unchanged.

**Planetary presentation continuation:** Solar terrain now defaults to **Natural**,
with body-fixed land materials and renderer-owned ocean, cloud and atmosphere layers.
The existing diagnostics remain available. A bounded successor-construction pipeline
reduces one refinement exclusion but does **not** pass Acceptance A; morphology is
unchanged. [Current report](docs/PHASE_5_PLANETARY_PRESENTATION_REPORT.md) and
[before/after evidence](docs/evidence/phase5-overnight-planetary/README.md) distinguish
visual-layer implementation from still-open convergence and visual acceptance.

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
cargo run --locked -p mundaris_app -- --solar-system
cargo run --locked -p mundaris_app -- --real-solar-system
cargo run --locked -p mundaris_app -- --reference-frames
cargo run --locked -p mundaris_app -- --celestial-model
cargo run --locked -p mundaris_app -- --gravity-orbits
```

Gameplay Solar System terrain is enabled by default for rocky bodies, but only
the observer-local required body generates surface geometry. Distant system view
does not generate terrain for all planets. Sun and gas/ice giants stay simple
far-body spheres. Use Home for system overview, select any named body and Focus/
Fit physical body for deterministic navigation; Earth, Moon and Mars have distinct
terrain definitions. `--real-solar-system` uses near-real radii/orbital lengths
through the same engine code (circular initial states, not an ephemeris).

The original `--gravity-orbits` fixture retains an opt-in **terrain checkpoint
preview** in the surface panel. Enable it there, or set `MUNDARIS_PHASE5_TERRAIN=1` before
launching. It authors the deterministic fixture once, then renders cached f64
displaced geometry; disabling the preview restores the Phase 4 sphere path.
The preview uses balanced adaptive displaced covers and common-refinement morphs
(150 ms by default; `MUNDARIS_TERRAIN_MORPH_MS=0` selects the static checkpoint).
Quality remains resource-constrained; native transition construction is off-thread,
but the serial comparison path can stall. Close-range terrain navigation and
interactive convergence remain incomplete. See
[checkpoint evidence](docs/phase-5-validation.md) and
[implementation](docs/phase-5-7-adaptive-terrain.md).

Normal invocation opens the paused gameplay Solar System overview. `--reference-frames` draws abstract axes and wire boxes attached to an analytically translating/rotating hierarchy, with source-centred rendering, precision diagnostics, pause/seek/reset, a `1e16 m` shared-offset stress mode, continuous approach, paused frame re-expression, and centimetre movement buttons. Begin approach while paused, then select Play. Re-expression and local movement are available while paused. This fixture contains no planet, terrain, or physics simulation.

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
is a smooth zero-height sphere unless the terrain checkpoint is enabled. Navigation uses one high-precision observer; moving or
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

**Phase 5.9 diagnostic:** **Readability** (blue below the content
sea datum, green lowlands, smooth grey rock by analytic slope), with the existing
directional lighting. The shading menu retains Elevation/Lit/Normals/Diffuse and
adds Slope/SeaMask/RockWeight. Earth uses a +350 m diagnostic sea datum; this is
colour only, not water geometry or biomes. LOD colours have a numeric hue legend.
**Natural** is now the normal Solar presentation, with independent **Ocean / Clouds /
Atmosphere** controls. Earth receives the three layers; Moon and Mars do not receive
Earth oceans/clouds. These are visual approximations, not physical fluids or climate.

The inspection panel separately reports signed **complete terrain clearance** and
**drawn mesh clearance**, plus explicit inside warnings, local patch/LOD, mesh
footprint, ready/pending counts and ownership. Clearance presets target displaced
terrain. Optional **Disabled / 2 m / 10 m / 100 m** guard pushes above both terrain
truth and the ready mesh; resource-constrained mesh quality can require extra
clearance. This is developer navigation, not collision. See
[Phase 5.9](MUNDARIS_PHASE_5_9_TERRAIN_READABILITY_AND_INSPECTION.md).

Retained [Phase 5.9 captures and measurements](docs/evidence/phase59/README.md)
separate complete readiness from quality convergence. Reproduce the native
offscreen matrix with:

```text
cargo run --locked --release -p mundaris_app --features terrain-capture,surface-profile --example solar_system_capture -- target/phase59 phase59 400
```

Focus Earth/Moon/Mars (or Aurelia/Luma in the legacy fixture), open **Planet surface / inspection**, set astronomical or exact
clearance targets, or run the continuous 30 s approach to 2 m. **I** explicitly
attaches the same observer to the rotating fixed frame; right drag looks locally,
WASD/QE move at reference-altitude-scaled editor speed and **H** looks toward the tangent
horizon. Look-at body controls and single physical steps observe the independent
moon/star. Smooth bodies retain the reference-sphere guard; terrain inspection has
the optional displaced-surface guard. Neither is collision/walking.

The legacy fixture without terrain uses a plain analytically shaded sphere;
gameplay rocky bodies use generated terrain when observer-local demand admits it.
Optional borders/LOD/face/bounds
views, pointer patch addressing and readiness/cache/quality counters expose the
derived structure. Far/surface ownership is exclusive per BodyId; no scene switch.

For a reproducible native route through the same implementation, set
`$env:MUNDARIS_PHASE4_VALIDATE='1'` in PowerShell before running `--gravity-orbits`.
Omit/unset it for normal paused startup. UI also exposes **Run integrated validation
route**. The optional terrain checkpoint uses this same route; physical atmospheric
simulation and terrain collision/navigation remain unimplemented.

## Quality checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

CI uses `--locked` for Cargo compilation/lint/test commands to enforce the committed `Cargo.lock`. Default tests remain headless; native GPU readback regressions are explicitly ignored and require a graphics adapter when selected. For interactive validation, run the app and check the panel, resize, minimize/restore, and clean exit. Compilation alone does not establish those runtime behaviors.

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
cargo bench --locked -p mundaris_math --bench terrain_noise
cargo bench --locked -p mundaris_world --bench terrain_generation
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
