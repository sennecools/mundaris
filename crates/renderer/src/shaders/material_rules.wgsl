// Surface material rules (pipeline §10.1, §10.4; milestone M4).
// PROTOTYPE: mirror of `astrum_world::terrain::material_rules`
// (crates/world/src/terrain/material_rules.rs), included by the atlas
// producer. f32 only; keep constants and formulas in step with the Rust file.
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
    let beach = 1.0 - mr_smoothstep(1.5, 3.0, i.height_m);
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

// Ground palette (material_rules::GroundPalette): linear material colours,
// strata contrast and the soil style pass in OKLCh.
struct GroundPalette {
    rock: vec3<f32>,
    scree: vec3<f32>,
    sand: vec3<f32>,
    wet_sediment: vec3<f32>,
    strata: f32,
    soil_amount: f32,
    chroma_gain: f32,
    chroma_max: f32,
    soften: f32,
    lightness_mid: f32,
    hue_pull: f32,
    anchors: u32,
    hue_anchors_deg: vec3<f32>,
}

const MR_PI: f32 = 3.14159265;

fn mr_cbrt(x: f32) -> f32 {
    return pow(max(x, 0.0), 1.0 / 3.0);
}

fn linear_to_oklab(c: vec3<f32>) -> vec3<f32> {
    let l = mr_cbrt(0.41222146 * c.x + 0.53633255 * c.y + 0.05144599 * c.z);
    let m = mr_cbrt(0.2119035 * c.x + 0.6806995 * c.y + 0.10739696 * c.z);
    let s = mr_cbrt(0.08830246 * c.x + 0.28171885 * c.y + 0.6299787 * c.z);
    return vec3<f32>(
        0.21045426 * l + 0.7936178 * m - 0.004072047 * s,
        1.9779985 * l - 2.4285922 * m + 0.4505937 * s,
        0.025904037 * l + 0.78277177 * m - 0.80867577 * s,
    );
}

fn oklab_to_linear(c: vec3<f32>) -> vec3<f32> {
    let l0 = c.x + 0.39633778 * c.y + 0.21580376 * c.z;
    let m0 = c.x - 0.105561346 * c.y - 0.06385417 * c.z;
    let s0 = c.x - 0.08948418 * c.y - 1.2914855 * c.z;
    let l = l0 * l0 * l0;
    let m = m0 * m0 * m0;
    let s = s0 * s0 * s0;
    return vec3<f32>(
        4.0767417 * l - 3.3077116 * m + 0.23096994 * s,
        -1.268438 * l + 2.6097574 * m - 0.34131938 * s,
        -0.0041960863 * l - 0.7034186 * m + 1.7076147 * s,
    );
}

fn mr_in_gamut(c: vec3<f32>) -> bool {
    return all(c >= vec3<f32>(0.0)) && all(c <= vec3<f32>(1.0));
}

// Signed angle difference in -π..π.
fn mr_wrap(d: f32) -> f32 {
    let x = d + 3.0 * MR_PI;
    return x - 2.0 * MR_PI * floor(x / (2.0 * MR_PI)) - MR_PI;
}

// Soil style pass (material_rules::style_soil) on a linear colour.
fn style_soil(colour: vec3<f32>, p: GroundPalette) -> vec3<f32> {
    if p.soil_amount <= 0.0 {
        return colour;
    }
    let lab = linear_to_oklab(colour);
    var hue = atan2(lab.z, lab.y);
    let chroma = min(length(lab.yz) * p.chroma_gain, p.chroma_max);
    var pull = 0.0;
    var nearest = 4.0;
    for (var k = 0u; k < min(p.anchors, 3u); k = k + 1u) {
        let d = mr_wrap(p.hue_anchors_deg[k] * MR_DEG - hue);
        if abs(d) < nearest {
            nearest = abs(d);
            pull = d;
        }
    }
    hue += p.hue_pull * pull;
    let lightness = lab.x + (p.lightness_mid - lab.x) * p.soften;
    let dir = vec2<f32>(cos(hue), sin(hue)) * chroma;
    var styled = oklab_to_linear(vec3<f32>(lightness, dir));
    if !mr_in_gamut(styled) {
        var lo = 0.0;
        var hi = 1.0;
        for (var i = 0; i < 12; i = i + 1) {
            let mid = 0.5 * (lo + hi);
            if mr_in_gamut(oklab_to_linear(vec3<f32>(lightness, dir * mid))) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        styled = clamp(oklab_to_linear(vec3<f32>(lightness, dir * lo)), vec3<f32>(0.0), vec3<f32>(1.0));
    }
    return colour + (styled - colour) * p.soil_amount;
}
