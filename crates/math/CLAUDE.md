# astrum_math: frames, precision, time, noise

Loaded automatically when you work in this crate. Map note:
`D:/Astrum/Astrum/systems/Frames and precision.md`.

## Owns
Checked f64 SI coordinates and time, rigid transforms, the runtime frame tree
with LCA evaluation, surface/cube tangents and topology, and the shared noise
primitives (`noise.rs`). Depends only on `glam`.

## Never
- No `wgpu`, `winit`, `egui` or world/simulation types here.
- No f32 narrowing for rendering: only the renderer narrows, at its observer boundary.
- No ambient RNG or system time in anything generation uses (determinism).

## Rules that bite
- Physical values: `f64`, metres, seconds, radians. Units in names (`radius_meters`).
- Frames are right-handed and orthonormal; column vectors, inner transform first;
  camera forward is local `-Z`.
- Subtract shared ancestry and the observer in the source frame *before*
  rotating; flattening to root positions loses local detail even in f64.
- Noise used by terrain must match the WGSL side bit-for-bit where tests
  compare GPU to the CPU oracle (`terrain_noise` tests and bench).

## Where things are
`frames.rs` frame tree · `coordinates.rs` SI positions · `transform.rs` rigid
transforms · `time.rs` epoch time · `surface.rs` cube/sphere surface maths ·
`noise.rs` hashing and noise.

## Test
`cargo test -p astrum_math` (fast, headless). Benches: `reference_frames`, `terrain_noise`.

## Contracts
`docs/adr/0002-reference-frames-and-precision.md`, `docs/engine-invariants.md`
("Scale, motion, and frames"), `docs/architecture.md` ("State and representation boundaries").
