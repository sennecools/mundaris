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
    rotate_octaves: u32,
    slope_damping: f32,
    domain_00: f32,
    domain_01: f32,
    domain_10: f32,
    domain_11: f32,
}

const ROLE_WARP_X: u32 = 16u;
const ROLE_WARP_Y: u32 = 17u;
const ROLE_HARDNESS: u32 = 18u;
const SQRT2: f32 = 1.41421356237309505;

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

// Mirrors GRADIENTS in noise.rs: unit vectors at odd multiples of 11.25 degrees.
var<private> GRADIENTS: array<vec2<f32>, 16> = array<vec2<f32>, 16>(
    vec2<f32>(0.98078528040323043, 0.19509032201612825),
    vec2<f32>(0.83146961230254524, 0.55557023301960218),
    vec2<f32>(0.55557023301960229, 0.83146961230254524),
    vec2<f32>(0.19509032201612833, 0.98078528040323043),
    vec2<f32>(-0.19509032201612819, 0.98078528040323043),
    vec2<f32>(-0.55557023301960196, 0.83146961230254535),
    vec2<f32>(-0.83146961230254535, 0.55557023301960218),
    vec2<f32>(-0.98078528040323043, 0.19509032201612861),
    vec2<f32>(-0.98078528040323043, -0.19509032201612836),
    vec2<f32>(-0.83146961230254546, -0.55557023301960196),
    vec2<f32>(-0.55557023301960218, -0.83146961230254524),
    vec2<f32>(-0.19509032201612866, -0.98078528040323032),
    vec2<f32>(0.19509032201612830, -0.98078528040323043),
    vec2<f32>(0.55557023301960184, -0.83146961230254546),
    vec2<f32>(0.83146961230254524, -0.55557023301960218),
    vec2<f32>(0.98078528040323032, -0.19509032201612872),
);

fn gradient(index: u32) -> vec2<f32> {
    return GRADIENTS[index];
}

fn gradient_at(ix: u32, iy: u32, seed: u32) -> vec2<f32> {
    return gradient(pcg(ix ^ pcg(iy ^ pcg(seed))) & 15u);
}

fn fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

fn fade_derivative(t: f32) -> f32 {
    return 30.0 * t * t * (t - 1.0) * (t - 1.0);
}

fn wrap_cell(c: f32, frequency: u32) -> u32 {
    let f = i32(frequency);
    return u32(((i32(c) % f) + f) % f);
}

// Value and gradient with respect to the lattice coordinates p * frequency.
fn periodic_noise_d(p: vec2<f32>, frequency: u32, seed: u32) -> vec3<f32> {
    let x = p * f32(frequency);
    let cell = floor(x);
    let t = x - cell;
    let ix0 = wrap_cell(cell.x, frequency);
    let iy0 = wrap_cell(cell.y, frequency);
    let ix1 = (ix0 + 1u) % frequency;
    let iy1 = (iy0 + 1u) % frequency;
    let g00 = gradient_at(ix0, iy0, seed);
    let g10 = gradient_at(ix1, iy0, seed);
    let g01 = gradient_at(ix0, iy1, seed);
    let g11 = gradient_at(ix1, iy1, seed);
    let n00 = dot(g00, t);
    let n10 = dot(g10, t - vec2<f32>(1.0, 0.0));
    let n01 = dot(g01, t - vec2<f32>(0.0, 1.0));
    let n11 = dot(g11, t - vec2<f32>(1.0, 1.0));
    let s = vec2<f32>(fade(t.x), fade(t.y));
    let ds = vec2<f32>(fade_derivative(t.x), fade_derivative(t.y));
    let a = n00 + (n10 - n00) * s.x;
    let b = n01 + (n11 - n01) * s.x;
    let da_x = g00.x + (g10.x - g00.x) * s.x + (n10 - n00) * ds.x;
    let db_x = g01.x + (g11.x - g01.x) * s.x + (n11 - n01) * ds.x;
    let da_y = g00.y + (g10.y - g00.y) * s.x;
    let db_y = g01.y + (g11.y - g01.y) * s.x;
    return vec3<f32>(
        a + (b - a) * s.y,
        da_x + (db_x - da_x) * s.y,
        da_y + (db_y - da_y) * s.y + (b - a) * ds.y,
    );
}

fn layer_value(layer: Layer, p: vec2<f32>, role: u32) -> f32 {
    var frequency = layer.frequency;
    // q = D·p, wrapped; jacobian rows track d(q)/d(p) (see noise.rs).
    var j0 = vec2<f32>(layer.domain_00, layer.domain_01);
    var j1 = vec2<f32>(layer.domain_10, layer.domain_11);
    let q0 = vec2<f32>(dot(j0, p), dot(j1, p));
    var q = q0 - floor(q0);
    var d = vec2<f32>(0.0, 0.0);
    var amplitude = 1.0;
    var total = 0.0;
    var norm = 0.0;
    var weight = 1.0;
    for (var octave = 0u; octave < layer.octaves; octave++) {
        let nd = periodic_noise_d(q, frequency, octave_seed(role, octave));
        let n = nd.x;
        var contribution = n;
        if layer.kind == 1u {
            let ridge_base = clamp(1.0 - abs(n) * SQRT2, 0.0, 1.0);
            var ridge = 0.0;
            if ridge_base > 0.0 {
                ridge = pow(ridge_base, layer.sharpness);
            }
            contribution = ridge * weight;
            weight = clamp(contribution * 2.0, 0.0, 1.0);
        } else if layer.kind == 2u {
            contribution = 2.0 * SQRT2 * abs(n) - 1.0;
        }
        if layer.slope_damping > 0.0 {
            let s = sqrt(abs(j0.x * j1.y - j0.y * j1.x));
            d += vec2<f32>(j0.x * nd.y + j1.x * nd.z, j0.y * nd.y + j1.y * nd.z) / s;
            contribution /= 1.0 + layer.slope_damping * dot(d, d);
        }
        total += contribution * amplitude;
        norm += amplitude;
        amplitude *= layer.gain;
        if layer.rotate_octaves != 0u {
            let next = vec2<f32>(2.0 * q.x - q.y, q.x + 2.0 * q.y);
            q = next - floor(next);
            let r0 = j0;
            j0 = 2.0 * r0 - j1;
            j1 = r0 + 2.0 * j1;
        } else {
            frequency *= layer.lacunarity;
        }
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
