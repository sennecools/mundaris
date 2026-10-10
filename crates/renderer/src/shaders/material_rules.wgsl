// Surface material rules (pipeline §10.1, §10.4; milestone M4).
// PROTOTYPE: standalone mirror of `astrum_world::terrain::material_rules`
// (crates/world/src/terrain/material_rules.rs). Not included by any pipeline
// yet. f32 only; keep constants and formulas in step with the Rust file.
// Slopes are angles from horizontal in radians.

const MR_DEG: f32 = 0.017453292;

struct MaterialInput {
    height_m: f32,
    slope: f32,
    uphill_slope: f32,
    temperature_c: f32,
    moisture: f32,
    hardness: f32,
    sediment: f32,
    flow: f32,
}

// Weights sum to 1. Order: bedrock, scree, soil, sand, snow, wet sediment.
struct MaterialWeights {
    bedrock: f32,
    scree: f32,
    soil: f32,
    sand: f32,
    snow: f32,
    wet_sediment: f32,
}

fn mr_smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn mr_soil_depth(i: MaterialInput) -> f32 {
    let base = (0.4 + 0.6 * mr_smoothstep(0.0, 0.5, i.moisture)) * (1.0 - 0.5 * i.hardness);
    let slope_factor = mr_smoothstep(15.0 * MR_DEG, 45.0 * MR_DEG, i.slope);
    return base * (1.0 - slope_factor) + 0.5 * i.sediment;
}

fn material_weights(i: MaterialInput) -> MaterialWeights {
    let slope = i.slope;
    let cold = 1.0 - mr_smoothstep(-3.0, 3.0, i.temperature_c);
    let snow = cold * (1.0 - mr_smoothstep(35.0 * MR_DEG, 55.0 * MR_DEG, slope));

    let flat_ground = 1.0 - mr_smoothstep(3.0 * MR_DEG, 9.0 * MR_DEG, slope);
    let wet_raw = mr_smoothstep(0.35, 0.7, i.flow) * flat_ground * mr_smoothstep(0.1, 0.4, i.moisture);

    let gentle = 1.0 - mr_smoothstep(6.0 * MR_DEG, 14.0 * MR_DEG, slope);
    let beach = 1.0 - mr_smoothstep(3.0, 6.0, i.height_m);
    let arid = (1.0 - mr_smoothstep(0.15, 0.4, i.moisture)) * mr_smoothstep(0.3, 0.6, i.sediment);
    let sand_raw = max(beach, arid) * gentle;

    let scree_raw = mr_smoothstep(30.0 * MR_DEG, 45.0 * MR_DEG, i.uphill_slope)
        * (1.0 - mr_smoothstep(18.0 * MR_DEG, 32.0 * MR_DEG, slope))
        * mr_smoothstep(5.0 * MR_DEG, 12.0 * MR_DEG, slope);

    let rock = max(
        1.0 - mr_smoothstep(0.1, 0.35, mr_soil_depth(i)),
        mr_smoothstep(32.0 * MR_DEG, 52.0 * MR_DEG, slope));

    var rem = 1.0 - snow;
    let wet = rem * wet_raw;
    rem -= wet;
    let sand = rem * sand_raw;
    rem -= sand;
    let scree = rem * scree_raw;
    rem -= scree;
    return MaterialWeights(rem * rock, scree, rem * (1.0 - rock), sand, snow, wet);
}

// Integer hash to 0..1 (PCG output function), equal to Rust `hash_unit`.
fn mr_hash_unit(n: u32) -> f32 {
    let state = n * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    let h = (word >> 22u) ^ word;
    return f32(h >> 8u) / 16777216.0;
}

// Layer tone in -1..1 by height, warped by a low-frequency `warp` in ~-1..1.
// `period_m` is the mean layer thickness. Continuous across layer edges.
fn strata_tone(height_m: f32, warp: f32, period_m: f32) -> f32 {
    let phase = height_m / period_m + 1.5 * warp;
    let layer = floor(phase);
    let f = phase - layer;
    let i = u32(i32(layer));
    let a = mr_hash_unit(i) * 2.0 - 1.0;
    let b = mr_hash_unit(i + 1u) * 2.0 - 1.0;
    return a + (b - a) * mr_smoothstep(0.8, 1.0, f);
}

fn strata_visibility(slope: f32, hardness: f32) -> f32 {
    return mr_smoothstep(35.0 * MR_DEG, 60.0 * MR_DEG, slope) * (0.4 + 0.6 * hardness);
}

fn strata_colour_factor(tone: f32, visibility: f32) -> f32 {
    return 1.0 + 0.35 * tone * visibility;
}

// Mean-matching (§10.2): detail albedo re-centred on the target colour.
fn tint_detail(detail: vec3<f32>, detail_mean: vec3<f32>, target_colour: vec3<f32>) -> vec3<f32> {
    return detail / max(detail_mean, vec3<f32>(1.0e-4)) * target_colour;
}
