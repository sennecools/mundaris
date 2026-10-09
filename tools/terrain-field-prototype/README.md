# Terrain Field Author

Local node-based authoring for deterministic terrain data. The browser edits a
saved recipe graph; Rust validates and evaluates its fields. This standalone tool
produces terrain data and inspection maps. Production engine integration and
planet rendering remain separate work.

## Start the editor

Run in PowerShell 7:

```powershell
& D:/Astrum/worktrees/procedural-terrain-data/tools/terrain-field-prototype/scripts/start-authoring.ps1 -Background
```

The launcher builds with the package lockfile, starts the service on
[127.0.0.1:4179](http://127.0.0.1:4179/), and prints its process receipt. Open that
URL. Build output stays in `D:/Astrum/target/procedural-terrain-data`; default
export bundles go to `D:/Astrum/target/terrain-authoring/exports`. Use
`-OutputRoot` to choose another export directory, `-Port` if the default is busy,
and `-NoBuild` to reuse the current binary. Without `-Background`, the service runs
in the terminal until Ctrl+C. Background process receipts and logs are under
`D:/Astrum/target/terrain-authoring/logs`.

Stop only the process identified by your receipt, after confirming that its
executable is the authoring tool. A browser tab closing does not stop the service.
The service binds loopback only and does not publish content online.

## Author a recipe

1. Load a template. The rocky feature template contains a finite crater process;
   the contrasting highland recipe combines relief bands and cold climate fields.
2. Select a node to edit its parameters or inspect its returned scalar map. Drag
   nodes to arrange them. Source selectors change real input connections; wires
   show those dependencies. Type/unit errors and cycles are rejected.
3. Switch height, humidity, temperature, material colors, weights or feature
   support and choose a cube face. These are field maps, not terrain meshes.
4. Lock geometry or climate while varying appearance. Identical graph/version,
   seeds and directions reproduce identical fields on the declared toolchain.
5. Save the graph JSON, reload it, or undo edits. Export builds an actual lossless
   bundle from the same graph evaluator into the service's output directory.

An invalid edit retains the last successful preview; the displayed error must be
resolved before export. Palette changes leave physical fields and material weights
unchanged. Layout/labels are saved but do not change field identity. Constrained
variants are recipe variations, not a guarantee of attractive planets.

Original feature relief/support, graph compilation, variation, derivatives and
bundle format details are documented in the continuation report and code. Surface
normals are numerical derivatives of the radial field; they do not create geometry
or certify collision/LOD continuity. No erosion or physical climate simulation is
claimed. There is no native planet, gas-giant, vegetation or engine integration.

## Original scalar recipe CLI

The first-slice JSON recipe format and its CLI remain available below. Graphs have
a separately versioned format and commands; do not relabel old recipe bundles.

This isolated Rust package evaluates deterministic continuous scalar fields at
normalized body-local directions. It exports physical height (metres), authored
humidity `[0,1]`, temperature (kelvin), three named normalized material weights,
and a palette preview. Height, climate, material response and palette have
separate seed/dependency identities. No grid resolution, face seed, request order,
camera, transform or runtime handle enters a query.

The evaluator is smooth seeded gradient noise over 3D direction coordinates.
Frequency is radius divided by authored wavelength in metres; octaves double
frequency with configured persistence. The two original recipes demonstrate a
low-humidity airless rocky field and a temperate highland field. They are recipe
examples, not calibrated planets or claims of convincing morphology. The model
does not simulate erosion, hydrology, climate circulation, or geological history.

## Build and run

From this directory, with the workspace's pinned Rust toolchain:

```powershell
$env:CARGO_TARGET_DIR='D:/Astrum/target/procedural-terrain-data'
cargo test --locked
cargo run --locked -- export recipes/airless-rocky.json D:/Astrum/ai/tasks/2026-10-08-procedural-terrain-data/exports/airless-rocky-32 32
cargo run --locked -- verify D:/Astrum/ai/tasks/2026-10-08-procedural-terrain-data/exports/airless-rocky-32
cargo run --locked -- export recipes/temperate-highlands.json D:/Astrum/ai/tasks/2026-10-08-procedural-terrain-data/exports/temperate-highlands-16 16
cargo run --locked -- verify D:/Astrum/ai/tasks/2026-10-08-procedural-terrain-data/exports/temperate-highlands-16
```

The lockfile is included; `cargo generate-lockfile` was used once when creating
the package. Each command above uses the pinned lock. Export refuses an existing output directory.
It writes into a sibling staging directory and renames the completed result into
place; a failed export removes its staging directory. Maximum resolution is 256
per face, octave count is at most eight, and one request is capped at
6 × 256 × 256 samples. Wavelength and max-octave lattice coordinates are also
validated before evaluation.

## Export format

Each cube face stores row-major texel-center samples in `*.f64le`. A record is 48
bytes: six IEEE-754 binary64 little-endian scalars in manifest order: height,
temperature, humidity, then the three material weights. Palette color is an
appearance preview and is not part of the raw physical record. `*.png` is an
8-bit color preview. Each face also has scalar PNG maps for height, temperature,
humidity and each material weight. Scalar maps use recipe-declared ranges shared
across all faces; faces are never independently normalized. PNGs are diagnostics,
not authoritative field data.

Face mapping for `(u,v)` in `[-1,1]²` is normalized from these vectors:

| Face | Direction vector before normalization |
| --- | --- |
| `px` | `[1, v, -u]` |
| `nx` | `[-1, v, u]` |
| `py` | `[u, 1, -v]` |
| `ny` | `[u, -1, v]` |
| `pz` | `[u, v, 1]` |
| `nz` | `[-u, v, -1]` |

Rows advance `v` from -1 to +1 and columns advance `u` from -1 to +1. Samples
are texel centers; no halo is stored and no within-face wrap is applied. Cross-face
consumers must query a common normalized body-local direction. The manifest stores
the face basis, units, observed height/temperature extrema, conservative authored
channel ranges, seed namespaces, identities, record layout, byte counts and SHA-256
hashes. The copied source recipe is included and hashed. `verify` checks supported
manifest/layout versions, recipe validity and identities, bounded file metadata,
the exact expected file set, safe filenames, raw lengths and all listed hashes.

Identity includes stable JSON recipe serialization plus explicit dependency hashes;
SHA-256 values are diagnostics and integrity checks, not collision-proof equality.
Bitwise reproducibility is scoped to the declared algorithm and Rust toolchain;
cross-platform floating-point identity is not promised. This package is a Slice A
data prototype only. It is not linked to engine crates and does not produce a
planet mesh, runtime streaming data, collision data, normals, erosion assets, or
a user-reviewed visual acceptance result.
