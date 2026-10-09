// Integer-hashed gradient noise on a split lattice (ASTRUM_TERRAIN_PIPELINE.md
// §4.4, §17.1, App. A.1/A.2). Mirrors crates/world/src/terrain/surface/noise.rs,
// the CPU test oracle. No float hashing: lattice cells are integers, and the
// absolute lattice position never exists in f32 — the CPU supplies each
// octave's integer cell and f32 fraction at the node centre.

struct OctaveOrigin {
    cell: vec3<i32>,
    seed: u32,
    frac: vec3<f32>,
    freq: f32,
    amplitude: vec4<f32>, // x = amplitude (m) including the node's band-limit weight
}

fn pcg3d(v_in: vec3<u32>) -> vec3<u32> {
    var v = v_in * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

fn lattice_bits(cell: vec3<i32>, seed: u32) -> vec3<u32> {
    return pcg3d(bitcast<vec3<u32>>(cell) ^ vec3<u32>(seed, seed * 747796405u, seed * 2891336453u));
}

// Unit lattice gradient; near-zero raw vectors fall back to +X (as on the CPU).
fn lattice_gradient(cell: vec3<i32>, seed: u32) -> vec3<f32> {
    let raw = vec3<f32>(lattice_bits(cell, seed)) * (2.0 / 4294967295.0) - vec3<f32>(1.0);
    let length_squared = dot(raw, raw);
    if length_squared < 1.0e-6 {
        return vec3<f32>(1.0, 0.0, 0.0);
    }
    return raw * inverseSqrt(length_squared);
}

// Quintic gradient noise of one octave at `local` metres from the node centre.
// Returns (value, d value / d local).
fn split_gradient_noise(o: OctaveOrigin, local: vec3<f32>) -> vec4<f32> {
    let q = o.frac + local * o.freq;
    let fl = floor(q);
    let cell = o.cell + vec3<i32>(fl);
    let t = q - fl;
    let u = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let du = t * t * (t * (t - 2.0) + 1.0) * 30.0;
    var value = 0.0;
    var gradient = vec3<f32>(0.0);
    for (var corner = 0u; corner < 8u; corner = corner + 1u) {
        let c = vec3<u32>(corner & 1u, (corner >> 1u) & 1u, (corner >> 2u) & 1u);
        let offset = vec3<f32>(c);
        let g = lattice_gradient(cell + vec3<i32>(c), o.seed);
        let d = dot(g, t - offset);
        let picked = select(vec3<f32>(1.0) - u, u, c == vec3<u32>(1u));
        let slope = select(-du, du, c == vec3<u32>(1u));
        let w = picked.x * picked.y * picked.z;
        value += w * d;
        gradient += g * w + vec3<f32>(slope.x * picked.y * picked.z, picked.x * slope.y * picked.z,
            picked.x * picked.y * slope.z) * d;
    }
    return vec4<f32>(value, gradient * o.freq);
}
