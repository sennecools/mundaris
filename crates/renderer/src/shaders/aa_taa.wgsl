// Temporal anti-aliasing (amendment 2026-10-10 anti-aliasing).
// The main pass renders with a sub-pixel Halton jitter. This pass reprojects
// last frame's output with the camera motion (depth + camera transform; no
// per-object motion vectors yet), clips it to the current 3x3 neighbourhood
// (variance clipping in YCoCg on luma-weighted colour), and blends 1:9 with
// the current frame. No sharpening. Runs on pre-exposed HDR radiance after
// the AO composite, so exposure, bloom and tonemap see the converged image.

struct Taa {
    jitter: vec4<f32>,      // current jitter in NDC (x, y), -, -
    projection: vec4<f32>,  // tan(half fov) and aspect: current (x, y), previous (z, w)
    rel0: vec4<f32>,        // previous-view-from-current-view rows [R | t]
    rel1: vec4<f32>,
    rel2: vec4<f32>,
    params: vec4<f32>,      // near m, current weight, reset (1 = take current), -
}

@group(0) @binding(0) var<uniform> taa: Taa;
@group(0) @binding(1) var current_tex: texture_2d<f32>;
@group(0) @binding(2) var depth_tex: texture_depth_2d;
@group(0) @binding(3) var history_tex: texture_2d<f32>;
@group(0) @binding(4) var linear_clamp: sampler;

fn rgb_to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn ycocg_to_rgb(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

// Luma weighting (Karis): resolves in a perceptual-ish space so bright
// samples do not dominate thin dark geometry.
fn compress(c: vec3<f32>) -> vec3<f32> {
    return c / (1.0 + luminance(c));
}

fn expand(c: vec3<f32>) -> vec3<f32> {
    return c / max(1.0 - luminance(c), 1.0e-4);
}

fn load_current(ip: vec2<i32>) -> vec3<f32> {
    let last = vec2<i32>(textureDimensions(current_tex)) - vec2<i32>(1);
    return max(textureLoad(current_tex, clamp(ip, vec2<i32>(0), last), 0).rgb, vec3<f32>(0.0));
}

// Catmull-Rom history fetch with five bilinear taps (Jimenez 2016).
fn sample_history(uv: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    let position = uv * size;
    let center = floor(position - 0.5) + 0.5;
    let f = position - center;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let tc0 = (center - 1.0) / size;
    let tc3 = (center + 2.0) / size;
    let tc12 = (center + w2 / w12) / size;
    var c = textureSampleLevel(history_tex, linear_clamp, vec2<f32>(tc12.x, tc0.y), 0.0).rgb * (w12.x * w0.y);
    c += textureSampleLevel(history_tex, linear_clamp, vec2<f32>(tc0.x, tc12.y), 0.0).rgb * (w0.x * w12.y);
    c += textureSampleLevel(history_tex, linear_clamp, vec2<f32>(tc12.x, tc12.y), 0.0).rgb * (w12.x * w12.y);
    c += textureSampleLevel(history_tex, linear_clamp, vec2<f32>(tc3.x, tc12.y), 0.0).rgb * (w3.x * w12.y);
    c += textureSampleLevel(history_tex, linear_clamp, vec2<f32>(tc12.x, tc3.y), 0.0).rgb * (w12.x * w3.y);
    let weight = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return max(c / weight, vec3<f32>(0.0));
}

@fragment
fn fs_taa(input: FullscreenOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(current_tex));
    let ip = vec2<i32>(input.position.xy);
    let current = load_current(ip);
    if taa.params.z > 0.5 {
        return vec4<f32>(current, 1.0);
    }

    // Neighbourhood statistics and the nearest depth of the 3x3 (so edges
    // reproject with the foreground's motion).
    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    var depth = 0.0;
    var depth_offset = vec2<i32>(0);
    let last = vec2<i32>(size) - vec2<i32>(1);
    for (var y = -1; y <= 1; y += 1) {
        for (var x = -1; x <= 1; x += 1) {
            let o = vec2<i32>(x, y);
            let c = rgb_to_ycocg(compress(load_current(ip + o)));
            m1 += c;
            m2 += c * c;
            let d = textureLoad(depth_tex, clamp(ip + o, vec2<i32>(0), last), 0);
            if d > depth {
                depth = d;
                depth_offset = o;
            }
        }
    }
    let mean = m1 / 9.0;
    let sigma = sqrt(max(m2 / 9.0 - mean * mean, vec3<f32>(0.0)));
    let box_min = mean - sigma;
    let box_max = mean + sigma;

    // Reproject: current pixel (jitter removed) -> view -> previous view -> uv.
    let near = taa.params.x;
    let pixel = input.position.xy + vec2<f32>(depth_offset);
    var ndc = vec2<f32>(pixel.x / size.x * 2.0 - 1.0, 1.0 - pixel.y / size.y * 2.0);
    ndc -= taa.jitter.xy;
    let ray = vec3<f32>(
        ndc.x * taa.projection.x * taa.projection.y,
        ndc.y * taa.projection.x,
        -1.0,
    );
    var previous_view: vec3<f32>;
    if depth > 0.0 {
        let view = ray * (near / depth);
        previous_view = vec3<f32>(
            dot(taa.rel0.xyz, view) + taa.rel0.w,
            dot(taa.rel1.xyz, view) + taa.rel1.w,
            dot(taa.rel2.xyz, view) + taa.rel2.w,
        );
    } else {
        // Sky: a direction, rotation only.
        previous_view = vec3<f32>(dot(taa.rel0.xyz, ray), dot(taa.rel1.xyz, ray), dot(taa.rel2.xyz, ray));
    }
    let delta = pixel - input.position.xy;
    if previous_view.z >= -1.0e-6 {
        return vec4<f32>(current, 1.0);
    }
    let previous_ndc = vec2<f32>(
        previous_view.x / (-previous_view.z * taa.projection.z * taa.projection.w),
        previous_view.y / (-previous_view.z * taa.projection.z),
    );
    let previous_uv = vec2<f32>(previous_ndc.x * 0.5 + 0.5, 0.5 - previous_ndc.y * 0.5)
        - delta / size;
    if any(previous_uv < vec2<f32>(0.0)) || any(previous_uv > vec2<f32>(1.0)) {
        return vec4<f32>(current, 1.0);
    }

    // Clip history towards the neighbourhood mean, then blend.
    let history = rgb_to_ycocg(compress(sample_history(previous_uv, size)));
    let center = 0.5 * (box_max + box_min);
    let extent = max(0.5 * (box_max - box_min), vec3<f32>(1.0e-5));
    let offset = history - center;
    let units = abs(offset / extent);
    let scale = max(units.x, max(units.y, units.z));
    let clipped = select(history, center + offset / scale, scale > 1.0);
    let current_c = rgb_to_ycocg(compress(current));
    let blended = mix(clipped, current_c, taa.params.y);
    return vec4<f32>(expand(ycocg_to_rgb(blended)), 1.0);
}
