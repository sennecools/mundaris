// Periodic base terrain. Mirrors `noise.rs` (f64 reference) in f32.

struct Params {
    n: u32,
    height_layer_count: u32,
    has_warp: u32,
    seed_base: u32,
    amplitude_m: f32,
    warp_amplitude: f32,
    hardness_base: f32,
    hardness_variation: f32,
}

struct Layer {
    kind: u32,
    frequency: u32,
    octaves: u32,
    lacunarity: u32,
    gain: f32,
    weight: f32,
    sharpness: f32,
    role: u32,
}

const ROLE_WARP_X: u32 = 16u;
const ROLE_WARP_Y: u32 = 17u;
const ROLE_HARDNESS: u32 = 18u;
const DIAGONAL: f32 = 0.70710678118654752;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> layers: array<Layer>;
@group(0) @binding(2) var<storage, read_write> height_out: array<f32>;
@group(0) @binding(3) var<storage, read_write> hardness_out: array<f32>;

fn pcg(value: u32) -> u32 {
    let state = value * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn octave_seed(role: u32, octave: u32) -> u32 {
    return pcg(params.seed_base ^ (role * 0x9e3779b9u) ^ (octave * 0x85ebca6bu));
}

fn gradient(index: u32) -> vec2<f32> {
    switch index {
        case 0u: { return vec2<f32>(1.0, 0.0); }
        case 1u: { return vec2<f32>(-1.0, 0.0); }
        case 2u: { return vec2<f32>(0.0, 1.0); }
        case 3u: { return vec2<f32>(0.0, -1.0); }
        case 4u: { return vec2<f32>(DIAGONAL, DIAGONAL); }
        case 5u: { return vec2<f32>(-DIAGONAL, DIAGONAL); }
        case 6u: { return vec2<f32>(DIAGONAL, -DIAGONAL); }
        default: { return vec2<f32>(-DIAGONAL, -DIAGONAL); }
    }
}

fn corner(ix: u32, iy: u32, seed: u32, d: vec2<f32>) -> f32 {
    return dot(gradient(pcg(ix ^ pcg(iy ^ pcg(seed))) & 7u), d);
}

fn fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn wrap_cell(c: f32, frequency: u32) -> u32 {
    let f = i32(frequency);
    return u32(((i32(c) % f) + f) % f);
}

fn periodic_noise(p: vec2<f32>, frequency: u32, seed: u32) -> f32 {
    let x = p * f32(frequency);
    let cell = floor(x);
    let t = x - cell;
    let ix0 = wrap_cell(cell.x, frequency);
    let iy0 = wrap_cell(cell.y, frequency);
    let ix1 = (ix0 + 1u) % frequency;
    let iy1 = (iy0 + 1u) % frequency;
    let n00 = corner(ix0, iy0, seed, t);
    let n10 = corner(ix1, iy0, seed, t - vec2<f32>(1.0, 0.0));
    let n01 = corner(ix0, iy1, seed, t - vec2<f32>(0.0, 1.0));
    let n11 = corner(ix1, iy1, seed, t - vec2<f32>(1.0, 1.0));
    let s = vec2<f32>(fade(t.x), fade(t.y));
    let a = n00 + (n10 - n00) * s.x;
    let b = n01 + (n11 - n01) * s.x;
    return a + (b - a) * s.y;
}

fn layer_value(layer: Layer, p: vec2<f32>, role: u32) -> f32 {
    var frequency = layer.frequency;
    var amplitude = 1.0;
    var total = 0.0;
    var norm = 0.0;
    var weight = 1.0;
    for (var octave = 0u; octave < layer.octaves; octave++) {
        let n = periodic_noise(p, frequency, octave_seed(role, octave));
        var contribution = n;
        if layer.kind == 1u {
            let r = clamp(1.0 - abs(n) * 1.4142, 0.0, 1.0);
            var ridge = 0.0;
            if r > 0.0 {
                ridge = pow(r, layer.sharpness);
            }
            contribution = ridge * weight;
            weight = clamp(contribution * 2.0, 0.0, 1.0);
        }
        total += contribution * amplitude;
        norm += amplitude;
        amplitude *= layer.gain;
        frequency *= layer.lacunarity;
    }
    return total / norm;
}

@compute @workgroup_size(16, 16)
fn base_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.n || id.y >= params.n {
        return;
    }
    let p = (vec2<f32>(id.xy) + 0.5) / f32(params.n);
    var warped = p;
    if params.has_warp != 0u {
        let warp = layers[params.height_layer_count];
        warped += params.warp_amplitude * vec2<f32>(
            layer_value(warp, p, ROLE_WARP_X),
            layer_value(warp, p, ROLE_WARP_Y),
        );
    }
    var height = 0.0;
    for (var i = 0u; i < params.height_layer_count; i++) {
        let layer = layers[i];
        height += layer.weight * layer_value(layer, warped, layer.role);
    }
    let hardness_layer = layers[params.height_layer_count + 1u];
    let hardness = params.hardness_base
        + params.hardness_variation * layer_value(hardness_layer, p, ROLE_HARDNESS);
    let index = id.y * params.n + id.x;
    height_out[index] = height * params.amplitude_m;
    hardness_out[index] = clamp(hardness, 0.0, 0.95);
}
