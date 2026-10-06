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

**Native Moon surface integration (2026-10-06):** both Solar System presets now
select the world-owned `RockyV5` surface for the Moon and route it through the
existing native terrain workers, cache, adaptive LOD, stitch/morph path and complete
clearance query. Live capture observed that path, but the user rejected the visual
result and reported performance around 10 FPS. The capture remained
`quality_pending=true` and `settled=false`; cadence samples and CPU stage timings
are not GPU-FPS measurements. Visual and performance acceptance are **FAILED**;
the cause is not yet established. See the
[dated handoff](docs/NATIVE_MOON_INTEGRATION_REPORT.md), the offscreen pair, and
the live capture at
`native/captures/12764-1791251339393283700-4-moon-orbit/`.

## Prerequisites

Development-agent setup: [Codex with Luna subagents](docs/codex-workflow.md) or
the existing [OpenCode workflow](docs/opencode-workflow.md). Both use `AGENTS.md`.

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
through the same engine code (prescribed circular motion, not an ephemeris).

Normal launch and both solar presets use **prescribed analytic motion**, initially
paused at `1000×`. Signed fractional seeks and reverse playback sample directly;
single-step samples ±60 seconds and Reset returns to the authored epoch. Explicit
periods retain the original preset pacing and do not change with mass/radius edits.
The approved stationary-Sun/Earth-centered policy, orbital/spin inventory, mode
boundaries and evidence limits are documented in [analytic playback](docs/ANALYTIC_PLAYBACK.md).
The original hierarchy/circular fixtures remain Newtonian with unchanged fixed-step
integration and replay. Scenario loading creates a new session; there is no live
conversion between histories. Velocity editing is unavailable in prescribed mode.

The default decorative sky uses a seeded, versioned finite-star catalogue and
cached procedural nebulae/dust, with a broken galactic backbone and off-band
structures. Version 4's visual composition is **accepted for now** by the user;
native capture, warm performance and remaining engineering gates are tracked
separately in the [sky checkpoint](docs/PHASE_5_13C_R_SKY_COMPOSITION_REPORT.md).
This is presentation content, not an addressable astronomical universe.

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

The [Slice 1B family contract](docs/PLANET_TERRAIN_SLICE_1B.md) adds compositional
world-owned surfaces: rocky history, fractured ice, volcanic resurfacing and a
bounded irregular shape stress fixture. Generate the deterministic 12-body
reference package and comparison sheets in a new directory:

```powershell
cargo run --locked --release -p mundaris_app --features terrain-capture --example surface_family_reference -- target/terrain-redesign/slice1b/reference-reproduction --orbit-cells 384 --local-cells 384
python scripts/surface-family-sheets.py target/terrain-redesign/slice1b/reference-reproduction
```

For a numerical replay of the same thirteen definitions without rendering:

```powershell
cargo run --locked --release -p mundaris_app --features terrain-capture --example surface_family_reference -- target/terrain-redesign/slice1b/corpus-reproduction --corpus-only
```

The sheet script requires Pillow. The package contains complete query corpora,
definition/phenotype metadata, scale/geometry/material diagnostics, neutral lighting
and labelled/unlabelled contact sheets. Native rendering retains the legacy terrain
path; compositional body publication and radial clearance share the world query.
This reference does not establish production performance, collision or user visual
acceptance. [ADR 0009](docs/adr/0009-compositional-body-surfaces.md) records the
star-shaped representation boundary and independent definition identities.
The [Slice 1B report](docs/PLANET_TERRAIN_SLICE_1B_REPORT.md) separates automated
verification, reference observations and remaining visual gates.

The [Slice 1B.1 province contract](docs/PLANET_TERRAIN_SLICE_1B_1.md) adds
versioned `RockyV4`, `IcyV2` and `VolcanicV2` geological directors. Select them
explicitly to retain the historical family replay above:

```powershell
cargo run --locked --release -p mundaris_app --features terrain-capture --example surface_family_reference -- target/terrain-redesign/slice1b1/reference-reproduction --provinces --orbit-cells 384 --local-cells 384
python scripts/surface-family-sheets.py target/terrain-redesign/slice1b1/reference-reproduction
cargo run --locked --release -p mundaris_app --features terrain-capture --example surface_family_reference -- target/terrain-redesign/slice1b1/corpus-reproduction --provinces --corpus-only
```

The province package includes all twelve fixed bodies, retained unbiased crops,
four deterministically selected provinces per family at 20 km, 2 km, 256 m and
32 m, uniform-grey geometry, individual director/process maps and selection
metadata. Targeted crops use an oblique inspection camera; standing-height
regression crops remain separate. Scalar maps use labelled body or tangent-map
projections rather than the perspective capture. The
[Slice 1B.1 report](docs/PLANET_TERRAIN_SLICE_1B_1_REPORT.md) records numerical
verification and visual limitations; [ADR 0010](docs/adr/0010-geological-province-directors.md)
records ownership and preservation requirements. These captures do not establish
native continuous approach or authorize Slice 2.

The [Slice 1B.2 contract](docs/PLANET_TERRAIN_SLICE_1B_2.md) adds explicit
`RockyV5`, `IcyV3` and `VolcanicV3` hierarchical geological residuals. Its
reference mode retains a 384-cell local mesh and adds an 8 m crop, same-feature
anchors, separate unbiased local crops and contribution diagnostics:

```powershell
cargo run --locked --release -p mundaris_app --features terrain-capture --example surface_family_reference -- target/terrain-redesign/slice1b2/reference-reproduction --hierarchy --orbit-cells 384 --local-cells 384
python scripts/surface-family-sheets.py target/terrain-redesign/slice1b2/reference-reproduction
cargo run --locked --release -p mundaris_app --features terrain-capture --example surface_family_reference -- target/terrain-redesign/slice1b2/corpus-reproduction --hierarchy --corpus-only
cargo run --locked --release -p mundaris_app --example surface_hierarchy_cost -- target/terrain-redesign/slice1b2/query-cost-reproduction.json
```

Every output path must be new. The cost example compares complete parent and
successor queries using identical authored parameters and three alternating-order
scans; it excludes rendering and makes no native FPS claim. See
[ADR 0011](docs/adr/0011-hierarchical-geological-residuals.md) for the authority
boundary. The [Slice 1B.2 report](docs/PLANET_TERRAIN_SLICE_1B_2_REPORT.md) indexes
the completed package, measured query cost and partial 8 m visual acceptance.
Historical modes above remain separately reproducible.

The opt-in resident GPU terrain prototype consumes those world-owned surfaces
through derived tiles. [Slice 2A](docs/PLANET_TERRAIN_SLICE_2A_REPORT.md) proves one
resident tile; [Slice 2B](docs/PLANET_TERRAIN_SLICE_2B_REPORT.md) extends only that fixed
region to a pinned parent and four asynchronous children. It does not replace
whole-body terrain selection. Generate paired transition evidence in a new path:

```powershell
cargo run --locked --release -p mundaris_app --features developer-tools --example resident_hierarchy_capture -- target/terrain-redesign/slice2b/reproduction
cargo test --locked --release -p mundaris_app --all-features --test resident_hierarchy_world_gpu -- --ignored --nocapture
cargo test --locked --release -p mundaris_renderer --all-features --test resident_hierarchy_gpu -- --ignored --nocapture
```

The `gpu_tile` developer command establishes a temporary published surface fixture;
`gpu_hierarchy` requests its four canonical children, controls morph duration,
injects worker delays, cancels requests, and explicitly requests GPU diagnostics.
`gpu_tile_view` selects height, normal, material, UV, or grid inspection. Snapshots
carry keys, physical slots, generations, readiness, morph fractions, upload bytes,
resource capacities, and reconstruction residuals. Ordinary morph frames request
no diagnostic readback. Disabling `gpu_tile` restores its original body authority
and camera. The native wall-paced torture scenario is
`scenarios/developer/gpu-hierarchy-transition.json`; the capture example controls
exact transition endpoints separately. See
[ADR 0013](docs/adr/0013-fixed-resident-terrain-hierarchy.md) for the fixed topology,
parent retention, and raw normal/material interpolation policy.

The [planet terrain redesign](docs/PLANET_TERRAIN_RENDERING_REDESIGN.md) starts with
a [Slice 1 reference prototype](docs/PLANET_TERRAIN_SLICE_1.md). Generate its
fixed-resolution moon terrain views and height/normal/material diagnostics with:

```powershell
cargo run --locked --release -p mundaris_app --features terrain-capture --example moon_surface_reference -- target/terrain-redesign/slice1-orbital/reference-reproduction --orbit-cells 768
```

Open the resulting `index.html` for the seed/view comparison. This temporary
software reference renderer defaults to the world-owned `MoonLikeV2` definition;
it does not replace the native application's terrain or validate GPU/LOD performance.
The output directory must be new. Optional `--seed 2 --view orbit --orbit-cells 512`
selects one fixture; `--terrain-version 1` replays the preserved V1 field. The
default orbital grid has 512 cells per cube face. CPU rays against the represented
triangles supply reference shadows; local crops omit casters outside their mesh.
Raw and filtered materials, analytic and mesh normals, shadow visibility and
unshadowed illumination remain separate diagnostics. User visual review is a
separate gate. See the [orbital refinement handoff](docs/PLANET_TERRAIN_SLICE_1_ORBITAL_REPORT.md)
and the [historical V1 handoff](docs/PLANET_TERRAIN_SLICE_1_REPORT.md).

The opt-in [development session interface](docs/AI_DEVELOPMENT_INTERFACE.md)
provides live inspection, leased control, native scene/UI captures, versioned
scenarios and owned rebuild/replay through CLI and MCP:

```powershell
cargo run --locked --release -p mundaris_app --features developer-tools -- --solar-system --dev-interface
cargo run --locked --release -p mundaris_app --features developer-tools --bin mundaris_dev -- sessions
```

`mundaris_app` remains the default binary. The project MCP adapter starts without
launching an application; launch is an explicit tool. Use an explicit session when
multiple apps are live. The existing Luna configuration and primary model choice
are preserved. Ordinary app launches expose no development endpoint.

For the fast developer review package, run `./scripts/ai-check.ps1`. It saves Git
state, focused results, and a deterministic PNG/JSON pair in a fresh
`target/ai-check/` run directory. This is **not full validation**. Native UI summaries
and capture JSON share one developer snapshot; commands, schema, fixtures and
evidence limits are in [AI development interface](docs/AI_DEVELOPMENT_INTERFACE.md).

The stronger `./scripts/validate.ps1 -IncludeGpu` records the full quality matrix
in a fresh `target/full-validation/` directory (omit `-IncludeGpu` on headless
hosts). Existing acceptance scripts remain separate; neither command establishes
native interaction or visual acceptance on its own.

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace --all-features
cargo test --locked --release --workspace --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps
```

CI uses `--locked` for Cargo compilation/lint/test commands to enforce the committed `Cargo.lock`. Default tests remain headless; native GPU readback regressions are explicitly ignored and require a graphics adapter when selected. For interactive validation, run the app and check the panel, resize, minimize/restore, and clean exit. Compilation alone does not establish those runtime behaviors.

`validate.ps1 -IncludeGpu` additionally selects the long-orbit checks, headless
bridge tests and the ignored `native_close_surface`, `native_full_frame`,
`developer_interface` and `developer_scenarios` GPU suites. The scenario suite
repeats all four deterministic fixtures and compares checkpoint values and pixels.
Native scenarios, actual stdio MCP, and fresh-session tool discovery remain
separate evidence. No tooling gate closes terrain Acceptance A or visual/camera
approval.

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
