// Experimental app-composed MoonFieldsV1 scalar recipe mirror.
// Inputs are anchored body-local scalars and prepared q2 recipe data; no
// absolute body coordinates or observer-relative values enter this kernel.
@group(0) @binding(0) var<storage, read> input: array<f32>;
@group(0) @binding(1) var<storage, read> parameters: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<vec4<f32>>;

const POINT_STRIDE: u32 = 657u; // 9 header scalars + 162 four-scalar recipes
const MAX_CRATERS: u32 = 162u;

fn smooth_range(value: f32, low: f32, high: f32) -> f32 {
    let t = clamp((value - low) / (high - low), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn crater_profile(q2: f32, freshness: f32, regional_strength: f32, regional: f32) -> f32 {
    let inside = 1.0 - q2;
    let fresh = 1.0 - freshness;
    let bowl = -(inside * inside) * (0.18 + 0.30 * fresh) * regional_strength;
    let rim = q2 * inside * inside * inside * (256.0 / 27.0)
        * (0.10 + 0.34 * fresh) * regional_strength;
    let ejecta = smooth_range(q2, 0.42, 0.98) * inside * inside * (0.10 + 0.22 * fresh);
    let degradation = smooth_range(regional + 0.15, -0.25, 0.30);
    return (bowl + rim + ejecta) * (1.0 - degradation * (0.28 * freshness));
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let point_count = arrayLength(&input) / POINT_STRIDE;
    if (id.x >= point_count || 2u * id.x + 1u >= arrayLength(&output)) {
        return;
    }
    let base = id.x * POINT_STRIDE;
    let global_height = input[base];
    let plains = input[base + 1u];
    let highlands = input[base + 2u];
    let regional = input[base + 3u];
    let count = min(u32(input[base + 8u]), MAX_CRATERS);
    let budgets = array<f32, 3>(48.0, 8.0, 1.5);
    var numerators = array<f32, 3>(0.0, 0.0, 0.0);
    var denominators = array<f32, 3>(1.0, 1.0, 1.0);
    for (var i = 0u; i < count; i += 1u) {
        let recipe = base + 9u + i * 4u;
        let band = min(u32(input[recipe]), 2u);
        let q2 = input[recipe + 1u];
        let freshness = input[recipe + 2u];
        let regional_strength = input[recipe + 3u];
        let window = pow(1.0 - q2, 3.0);
        numerators[band] += window * crater_profile(q2, freshness, regional_strength, regional) * budgets[band];
        denominators[band] += window;
    }
    var band_heights = array<f32, 3>(0.0, 0.0, 0.0);
    for (var band = 0u; band < 3u; band += 1u) {
        band_heights[band] = numerators[band] / denominators[band];
    }
    let height = global_height + band_heights[0] + band_heights[1] + band_heights[2];
    let impact_weight = abs(band_heights[0]) / 49.0
        + abs(band_heights[1]) / 9.0
        + abs(band_heights[2]) / 2.5;
    var weights = array<f32, 4>(
        clamp(0.62 + 0.30 * plains - 0.12 * impact_weight, 0.0, 1.0),
        clamp(0.22 + 0.54 * highlands - 0.15 * impact_weight, 0.0, 1.0),
        clamp(0.08 + 0.78 * plains, 0.0, 1.0),
        clamp(0.08 + 0.75 * impact_weight, 0.0, 1.0),
    );
    var total = weights[0] + weights[1] + weights[2] + weights[3];
    for (var channel = 0u; channel < 4u; channel += 1u) {
        weights[channel] /= total;
        weights[channel] *= input[base + 4u + channel];
    }
    total = weights[0] + weights[1] + weights[2] + weights[3];
    for (var channel = 0u; channel < 4u; channel += 1u) {
        weights[channel] /= total;
    }
    output[2u * id.x] = vec4<f32>(height, 0.0, 0.0, 0.0);
    output[2u * id.x + 1u] = vec4<f32>(weights[0], weights[1], weights[2], weights[3]);
}
