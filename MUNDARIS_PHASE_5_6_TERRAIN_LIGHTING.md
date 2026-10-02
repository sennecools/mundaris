# Phase 5.6 terrain lighting implementation

This is a narrow renderer checkpoint, not the final lighting system or a terrain
LOD milestone. [Validation and measurements](docs/phase-5-validation.md) separate
working implementation from remaining depth/morphology acceptance.

## Executable boundary

- World continues producing terrain positions/analytic radial-graph normals.
- App owns `TerrainLighting` runtime selection. No lighting enters terrain truth,
  cache identity, generation scheduling or certificates.
- Renderer retains body-fixed terrain normals through regular/clipped staging,
  normalizes their interpolation per fragment, and evaluates one directional sun.
  Position precision/conversion, reverse-Z, winding and depth ownership are unchanged.
- A generated-terrain flag isolates this path from legacy smooth-sphere shading.
  One 32-byte uniform binds to both surface routes; vertex formats are unchanged.

```text
N = interpolated body-fixed analytic normal, renormalized
L = body-fixed unit vector pointing surface -> sun
d = max(dot(N,L),0)
lit_linear = decode_srgb(base_elevation_rgb) * (ambient + diffuse_strength*d)
ambient = 0.06; diffuse_strength = 0.94
default L = normalize(0.8,0.3,0.25)
```

Normal debug colour is the stable body-axis `0.5*N+0.5` remap. Diffuse debug shows
linear `d`. Elevation mode removes lighting but keeps the existing colour ramp.
There is no normal exaggeration or fake contrast. Terrain debug colours decode to
linear before lighting and encode for the renderer's usual non-sRGB output; sRGB
targets encode in hardware. Other renderer/UI colour handling is not rewritten.

Body-fixed light co-rotates with each body. It never follows camera orientation.
Preset overhead/side/grazing/terminator/night vectors are relative to +Z, not a
camera-derived local frame. Validation crops retain explicit deterministic sun
vectors relative to their fixed scene directions, unchanged across each A/B.

## Running and evidence

```powershell
$env:MUNDARIS_PHASE5_TERRAIN='1'
$env:MUNDARIS_TERRAIN_MODE='lit' # elevation, normals, diffuse also supported
$env:MUNDARIS_TERRAIN_SUN='grazing' # optional +Z-relative preset
cargo run --locked -p mundaris_app -- --gravity-orbits
cargo run --locked --release -p mundaris_app --features terrain-capture --example terrain_lighting_capture -- target/phase56-captures
cargo bench --locked -p mundaris_app --bench terrain_lighting
```

The opt-in capture feature submits through the production `CelestialRenderer` with
a fixed RGBA8 target/readback, not a separate lighting shader or CPU image renderer.
Ordinary tests do not create GPU devices. Native runtime controls are in the terrain
preview panel. Input strengths and sun vectors are validated without cache resets.

[Evidence](docs/evidence/phase56/README.md) includes thirteen scenes × four exact-
camera modes, metadata, host timings and four operational native screenshots. Fine
windows are isolated same-level diagnostic meshes, **not** integrated adaptive LOD.
True-scale lit relief remains subdued; diffuse debug is the clearest local diagnostic.
The live level4 preview cannot show filtered-out erosion. Branching and general
mountain/valley/peak visual quality remain open, rather than being declared solved.

## Validation commands

```text
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo test --locked --workspace --all-features
cargo test --locked --workspace --all-features --release
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked -p mundaris_math -p mundaris_world -p mundaris_renderer -p mundaris_simulation -p mundaris_app --release --all-features --test terrain_noise --test terrain_generation --test terrain_bounds --test terrain_bands --test terrain_erosion --test terrain_error --test terrain_geometry --test terrain_identity --test planet_terrain --test terrain_geometry_error --test terrain_lighting
cargo test --locked -p mundaris_simulation --release --test orbits long_run -- --ignored --nocapture
```

Rustdoc uses `RUSTDOCFLAGS="-D warnings"` with
`cargo doc --locked --workspace --all-features --no-deps`.

No terrain self-shadowing/AO/atmosphere/material system, mixed-LOD displaced
stitching, common-refinement morphing or final adaptive selection is implemented.
CPU terrain geometry stays 13,872 bytes per Grid16 patch, GPU patch bytes stay 9,312;
only a 32-byte per-frame uniform is added. GPU timing is not available. No new
commits or pushes were made for this checkpoint.
