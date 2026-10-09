// Energy-conserving bloom (Jimenez, "Next generation post processing in Call
// of Duty: Advanced Warfare"): 13-tap downsample with a Karis average on the
// first level against fireflies, then tent upsampling added level by level.

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;

fn tap(uv: vec2<f32>, offset: vec2<f32>) -> vec3<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source));
    return textureSampleLevel(source, linear_clamp, uv + offset * texel, 0.0).rgb;
}

fn karis(a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, d: vec3<f32>) -> vec4<f32> {
    let avg = (a + b + c + d) * 0.25;
    let w = 1.0 / (1.0 + luminance(avg));
    return vec4<f32>(avg * w, w);
}

fn thirteen(uv: vec2<f32>, first: bool) -> vec3<f32> {
    let a = tap(uv, vec2<f32>(-2.0, 2.0));
    let b = tap(uv, vec2<f32>(0.0, 2.0));
    let c = tap(uv, vec2<f32>(2.0, 2.0));
    let d = tap(uv, vec2<f32>(-2.0, 0.0));
    let e = tap(uv, vec2<f32>(0.0, 0.0));
    let f = tap(uv, vec2<f32>(2.0, 0.0));
    let g = tap(uv, vec2<f32>(-2.0, -2.0));
    let h = tap(uv, vec2<f32>(0.0, -2.0));
    let i = tap(uv, vec2<f32>(2.0, -2.0));
    let j = tap(uv, vec2<f32>(-1.0, 1.0));
    let k = tap(uv, vec2<f32>(1.0, 1.0));
    let l = tap(uv, vec2<f32>(-1.0, -1.0));
    let m = tap(uv, vec2<f32>(1.0, -1.0));
    if first {
        let g0 = karis(j, k, l, m);
        let g1 = karis(a, b, d, e);
        let g2 = karis(b, c, e, f);
        let g3 = karis(d, e, g, h);
        let g4 = karis(e, f, h, i);
        let sum = g0 * 0.5 + (g1 + g2 + g3 + g4) * 0.125;
        return sum.rgb / max(sum.a, 1.0e-6);
    }
    return e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
}

@fragment
fn fs_down_first(input: FullscreenOut) -> @location(0) vec4<f32> {
    return vec4<f32>(thirteen(input.uv, true), 1.0);
}

@fragment
fn fs_down(input: FullscreenOut) -> @location(0) vec4<f32> {
    return vec4<f32>(thirteen(input.uv, false), 1.0);
}

@fragment
fn fs_up(input: FullscreenOut) -> @location(0) vec4<f32> {
    let r = post.bloom.z;
    var sum = tap(input.uv, vec2<f32>(0.0, 0.0)) * 4.0;
    sum += (tap(input.uv, vec2<f32>(0.0, -r)) + tap(input.uv, vec2<f32>(-r, 0.0))
        + tap(input.uv, vec2<f32>(r, 0.0)) + tap(input.uv, vec2<f32>(0.0, r))) * 2.0;
    sum += tap(input.uv, vec2<f32>(-r, -r)) + tap(input.uv, vec2<f32>(r, -r))
        + tap(input.uv, vec2<f32>(-r, r)) + tap(input.uv, vec2<f32>(r, r));
    return vec4<f32>(sum / 16.0, 1.0);
}
