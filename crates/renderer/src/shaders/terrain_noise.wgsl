// Integer-hashed gradient noise on dyadic octave ladders (ASTRUM_TERRAIN_PIPELINE.md
// §4.4, §17.1, App. A.1/A.2; M2 Shape design §3). Mirrors
// crates/world/src/terrain/surface/{noise,ladder}.rs, the CPU test oracle. No
// float hashing: lattice cells are integers, and the absolute lattice position
// never exists in f32 — each tile carries fixed-point anchors of its centre per
// ladder, from which `ladder_split` derives every octave's integer cell and
// fraction exactly.

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

// Quintic gradient noise at lattice coordinate `cell + q` (q any small f32).
// Returns (value, d value / d q).
fn gradient_noise_split(cell_in: vec3<i32>, q: vec3<f32>, seed: u32) -> vec4<f32> {
    let fl = floor(q);
    let cell = cell_in + vec3<i32>(fl);
    let t = q - fl;
    let u = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let du = t * t * (t * (t - 2.0) + 1.0) * 30.0;
    var value = 0.0;
    var gradient = vec3<f32>(0.0);
    for (var corner = 0u; corner < 8u; corner = corner + 1u) {
        let c = vec3<u32>(corner & 1u, (corner >> 1u) & 1u, (corner >> 2u) & 1u);
        let offset = vec3<f32>(c);
        let g = lattice_gradient(cell + vec3<i32>(c), seed);
        let d = dot(g, t - offset);
        let picked = select(vec3<f32>(1.0) - u, u, c == vec3<u32>(1u));
        let slope = select(-du, du, c == vec3<u32>(1u));
        let w = picked.x * picked.y * picked.z;
        value += w * d;
        gradient += g * w + vec3<f32>(slope.x * picked.y * picked.z, picked.x * slope.y * picked.z,
            picked.x * picked.y * slope.z) * d;
    }
    return vec4<f32>(value, gradient);
}

// Quintic gradient noise at small absolute lattice coordinates `q` (planet-scale
// fields, where |q| stays within a few hundred so f32 needs no lattice split).
fn gradient_noise(q: vec3<f32>, seed: u32) -> f32 {
    let fl = floor(q);
    let cell = vec3<i32>(fl);
    let t = q - fl;
    let u = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    var value = 0.0;
    for (var corner = 0u; corner < 8u; corner = corner + 1u) {
        let c = vec3<u32>(corner & 1u, (corner >> 1u) & 1u, (corner >> 2u) & 1u);
        let g = lattice_gradient(cell + vec3<i32>(c), seed);
        let picked = select(vec3<f32>(1.0) - u, u, c == vec3<u32>(1u));
        value += picked.x * picked.y * picked.z * dot(g, t - vec3<f32>(c));
    }
    return value;
}

// ---------------------------------------------------------------- 4D noise

fn pcg4d(v_in: vec4<u32>) -> vec4<u32> {
    var v = v_in * 1664525u + 1013904223u;
    v.x += v.y * v.w;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v.w += v.y * v.z;
    v ^= v >> vec4<u32>(16u);
    v.x += v.y * v.w;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v.w += v.y * v.z;
    return v;
}

fn lattice_bits4(cell: vec4<i32>, seed: u32) -> vec4<u32> {
    return pcg4d(bitcast<vec4<u32>>(cell)
        ^ vec4<u32>(seed, seed * 747796405u, seed * 2891336453u, seed * 277803737u));
}

fn lattice_gradient4(cell: vec4<i32>, seed: u32) -> vec4<f32> {
    let raw = vec4<f32>(lattice_bits4(cell, seed)) * (2.0 / 4294967295.0) - vec4<f32>(1.0);
    let length_squared = dot(raw, raw);
    if length_squared < 1.0e-6 {
        return vec4<f32>(1.0, 0.0, 0.0, 0.0);
    }
    return raw * inverseSqrt(length_squared);
}

struct Noise4 {
    value: f32,
    gradient: vec4<f32>, // d value / d q
}

// Quintic 4D gradient noise at lattice coordinate `cell + q` (16 corners).
fn gradient_noise4_split(cell_in: vec4<i32>, q: vec4<f32>, seed: u32) -> Noise4 {
    let fl = floor(q);
    let cell = cell_in + vec4<i32>(fl);
    let t = q - fl;
    let u = t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let du = t * t * (t * (t - 2.0) + 1.0) * 30.0;
    var out: Noise4;
    out.value = 0.0;
    out.gradient = vec4<f32>(0.0);
    for (var corner = 0u; corner < 16u; corner = corner + 1u) {
        let c = vec4<u32>(corner & 1u, (corner >> 1u) & 1u, (corner >> 2u) & 1u, (corner >> 3u) & 1u);
        let g = lattice_gradient4(cell + vec4<i32>(c), seed);
        let d = dot(g, t - vec4<f32>(c));
        let p = select(vec4<f32>(1.0) - u, u, c == vec4<u32>(1u));
        let s = select(-du, du, c == vec4<u32>(1u));
        let w = p.x * p.y * p.z * p.w;
        out.value += w * d;
        out.gradient += g * w + vec4<f32>(s.x * p.y * p.z * p.w, p.x * s.y * p.z * p.w,
            p.x * p.y * s.z * p.w, p.x * p.y * p.z * s.w) * d;
    }
    return out;
}

// ---------------------------------------------------------------- dyadic ladders

// Reciprocals of the ladder scales {1, 1.25, 1.5, 1.75}, rounded to f32
// (`LADDER_INVERSE_SCALES_F32`).
fn ladder_inverse_scale(ladder: u32) -> f32 {
    var inverse = array<f32, 4>(1.0, 0.8, 0.6666667, 0.5714286);
    return inverse[min(ladder, 3u)];
}

struct LadderPoint {
    cell: vec3<i32>,
    q: vec3<f32>,  // fraction relative to `cell`; may leave [0, 1) by the local part
    freq: f32,     // d q / d local: 2^-level / s
}

// Exact split of an octave's lattice coordinate (wavelength s · 2^level) at
// `local` metres from the tile centre, from the tile's anchor A and residual r
// on the octave's ladder (level in -11..=19): cell = A >> (11 + level), an
// exact floor by arithmetic shift; q = rem · 2^-(11 + level) + (local / s + r)
// · 2^-level. The remainder converts in two exact halves with one rounded sum.
fn ladder_split(anchor: vec3<i32>, residual: vec3<f32>, ladder: u32, level: i32,
    local: vec3<f32>) -> LadderPoint {
    let shift = u32(11 + level);
    var out: LadderPoint;
    out.cell = anchor >> vec3<u32>(shift);
    let rem = bitcast<vec3<u32>>(anchor) & vec3<u32>((1u << shift) - 1u);
    let high = vec3<f32>(rem >> vec3<u32>(12u)) * ldexp(1.0, 12 - i32(shift));
    let low = vec3<f32>(rem & vec3<u32>(0xfffu)) * ldexp(1.0, -i32(shift));
    let inverse = ladder_inverse_scale(ladder);
    let dyadic = ldexp(1.0, -level);
    out.q = (high + low) + (local * inverse + residual) * dyadic;
    out.freq = inverse * dyadic;
    return out;
}

// Frequency (cycles per metre) of an octave: 2^-level / s.
fn ladder_frequency(ladder: u32, level: i32) -> f32 {
    return ladder_inverse_scale(ladder) * ldexp(1.0, -level);
}

// Seed of one octave of a layer with seed salt `salt` (`ladder::octave_seed`).
fn ladder_octave_seed(salt: u32, ladder: u32, level: i32) -> u32 {
    return pcg3d(vec3<u32>(salt, 0x4c414444u ^ ladder, bitcast<u32>(level))).x;
}

// Sub-cell lattice offset in [0, 1)^3 of an octave seed: 24-bit fractions,
// exact in f32 (`ladder::octave_offset`).
fn ladder_octave_offset(seed: u32) -> vec3<f32> {
    let h = pcg3d(vec3<u32>(seed, seed ^ 0x9e3779b9u, seed ^ 0x7f4a7c15u));
    return vec3<f32>(h >> vec3<u32>(8u)) * (1.0 / 16777216.0);
}

// One octave of 3D gradient noise at `local` metres from the tile centre.
// Returns (value, d value / d local).
fn ladder_noise3(anchor: vec3<i32>, residual: vec3<f32>, ladder: u32, level: i32, seed: u32,
    local: vec3<f32>) -> vec4<f32> {
    let p = ladder_split(anchor, residual, ladder, level, local);
    let n = gradient_noise_split(p.cell, p.q + ladder_octave_offset(seed), seed);
    return vec4<f32>(n.x, n.yzw * p.freq);
}

struct LadderNoise4 {
    value: f32,
    gradient: vec3<f32>, // d value / d local
    dw: f32,             // d value / d w
}

// One octave of 4D gradient noise: the spatial lattice coordinate as in
// `ladder_noise3`, the 4th coordinate `w_cell + w_frac` already in lattice units
// (split by the caller). The seed offset applies to the spatial coordinates.
fn ladder_noise4(anchor: vec3<i32>, residual: vec3<f32>, ladder: u32, level: i32, seed: u32,
    local: vec3<f32>, w_cell: i32, w_frac: f32) -> LadderNoise4 {
    let p = ladder_split(anchor, residual, ladder, level, local);
    let n = gradient_noise4_split(vec4<i32>(p.cell, w_cell),
        vec4<f32>(p.q + ladder_octave_offset(seed), w_frac), seed);
    var out: LadderNoise4;
    out.value = n.value;
    out.gradient = n.gradient.xyz * p.freq;
    out.dw = n.gradient.w;
    return out;
}

// Band-limit weight of an octave at `frequency` (cycles per metre) for texels
// of `texel_m` (pipeline §9.3; `noise::octave_weight`): full below half the
// sampling limit 0.5 / texel, zero at and above it, smoothstep in between.
fn octave_weight(frequency: f32, texel_m: f32) -> f32 {
    if texel_m <= 0.0 {
        return 1.0;
    }
    let f_max = 0.5 / texel_m;
    let t = clamp((frequency - 0.5 * f_max) / (0.5 * f_max), 0.0, 1.0);
    return 1.0 - t * t * (3.0 - 2.0 * t);
}
