// FXAA 3.11 quality (after T. Lottes, public algorithm description),
// amendment 2026-10-10 anti-aliasing. Runs on the tonemapped image: reads the
// display-encoded (gamma) bytes through a unorm view, blends along detected
// edges, and writes linear colour into the sRGB scene view.

@group(0) @binding(0) var ldr_tex: texture_2d<f32>;
@group(0) @binding(1) var ldr_sampler: sampler;

const EDGE_THRESHOLD: f32 = 0.125;      // relative contrast to act on
const EDGE_THRESHOLD_MIN: f32 = 0.0312; // ignore dark noise
const SUBPIX: f32 = 0.75;               // sub-pixel aliasing removal amount
const STEPS: i32 = 10;

fn fxaa_luma(c: vec3<f32>) -> f32 {
    // Display-encoded luma is perceptual enough for edge detection.
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

fn step_size(i: i32) -> f32 {
    var steps = array<f32, 10>(1.0, 1.0, 1.0, 1.0, 1.5, 2.0, 2.0, 2.0, 4.0, 8.0);
    return steps[i];
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(
        pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)),
        c / 12.92,
        c <= vec3<f32>(0.04045),
    );
}

fn load_luma(ip: vec2<i32>) -> f32 {
    let last = vec2<i32>(textureDimensions(ldr_tex)) - vec2<i32>(1);
    return fxaa_luma(textureLoad(ldr_tex, clamp(ip, vec2<i32>(0), last), 0).rgb);
}

fn sample_luma(uv: vec2<f32>) -> f32 {
    return fxaa_luma(textureSampleLevel(ldr_tex, ldr_sampler, uv, 0.0).rgb);
}

@fragment
fn fs_fxaa(input: FullscreenOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(ldr_tex));
    let texel = 1.0 / size;
    let uv = input.position.xy * texel;
    let ip = vec2<i32>(input.position.xy);
    let center = textureLoad(ldr_tex, ip, 0).rgb;
    let m = fxaa_luma(center);
    let n = load_luma(ip + vec2<i32>(0, -1));
    let s = load_luma(ip + vec2<i32>(0, 1));
    let e = load_luma(ip + vec2<i32>(1, 0));
    let w = load_luma(ip + vec2<i32>(-1, 0));
    let hi = max(max(max(n, s), max(e, w)), m);
    let lo = min(min(min(n, s), min(e, w)), m);
    let range = hi - lo;
    if range < max(EDGE_THRESHOLD_MIN, hi * EDGE_THRESHOLD) {
        return vec4<f32>(srgb_to_linear(center), 1.0);
    }
    let nw = load_luma(ip + vec2<i32>(-1, -1));
    let ne = load_luma(ip + vec2<i32>(1, -1));
    let sw = load_luma(ip + vec2<i32>(-1, 1));
    let se = load_luma(ip + vec2<i32>(1, 1));

    // Sub-pixel blend factor from the 3x3 low-pass contrast.
    let average = (2.0 * (n + s + e + w) + nw + ne + sw + se) / 12.0;
    let sub = clamp(abs(average - m) / range, 0.0, 1.0);
    let sub_blend = smoothstep(0.0, 1.0, sub);
    let sub_amount = sub_blend * sub_blend * SUBPIX;

    // Edge orientation.
    let horizontal = abs(nw + ne - 2.0 * n) + 2.0 * abs(w + e - 2.0 * m) + abs(sw + se - 2.0 * s);
    let vertical = abs(nw + sw - 2.0 * w) + 2.0 * abs(n + s - 2.0 * m) + abs(ne + se - 2.0 * e);
    let is_horizontal = horizontal >= vertical;

    // Which side of the edge the contrast is on.
    let l1 = select(w, n, is_horizontal);
    let l2 = select(e, s, is_horizontal);
    let g1 = abs(l1 - m);
    let g2 = abs(l2 - m);
    let side_one = g1 >= g2;
    let gradient = 0.25 * max(g1, g2);
    var step_len = select(texel.x, texel.y, is_horizontal);
    var local = 0.0;
    if side_one {
        step_len = -step_len;
        local = 0.5 * (l1 + m);
    } else {
        local = 0.5 * (l2 + m);
    }

    // Walk along the edge in both directions until the luma changes.
    var edge_uv = uv;
    if is_horizontal {
        edge_uv.y += step_len * 0.5;
    } else {
        edge_uv.x += step_len * 0.5;
    }
    let dir = select(vec2<f32>(0.0, texel.y), vec2<f32>(texel.x, 0.0), is_horizontal);
    var uv1 = edge_uv - dir;
    var uv2 = edge_uv + dir;
    var end1 = sample_luma(uv1) - local;
    var end2 = sample_luma(uv2) - local;
    var done1 = abs(end1) >= gradient;
    var done2 = abs(end2) >= gradient;
    for (var i = 1; i < STEPS; i += 1) {
        if done1 && done2 {
            break;
        }
        if !done1 {
            uv1 -= dir * step_size(i);
            end1 = sample_luma(uv1) - local;
            done1 = abs(end1) >= gradient;
        }
        if !done2 {
            uv2 += dir * step_size(i);
            end2 = sample_luma(uv2) - local;
            done2 = abs(end2) >= gradient;
        }
    }
    let d1 = select(uv.y - uv1.y, uv.x - uv1.x, is_horizontal);
    let d2 = select(uv2.y - uv.y, uv2.x - uv.x, is_horizontal);
    let nearer_one = d1 < d2;
    let dist = min(d1, d2);
    let edge_length = d1 + d2;
    let center_below = m - local < 0.0;
    let correct = select((end2 < 0.0) != center_below, (end1 < 0.0) != center_below, nearer_one);
    let edge_amount = select(0.0, 0.5 - dist / max(edge_length, 1.0e-6), correct);
    let amount = max(edge_amount, sub_amount);

    var final_uv = uv;
    if is_horizontal {
        final_uv.y += amount * step_len;
    } else {
        final_uv.x += amount * step_len;
    }
    let c = textureSampleLevel(ldr_tex, ldr_sampler, final_uv, 0.0).rgb;
    return vec4<f32>(srgb_to_linear(c), 1.0);
}
