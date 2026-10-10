// MSAA resolve of the main-pass targets (amendment 2026-10-10 anti-aliasing).
// One fullscreen pass reads the multisampled MRT and depth and writes the
// single-sample targets the post chain reads:
// - direct radiance: weighted mean with w = 1 / (1 + luma) (Karis), so a
//   bright sky behind thin dark twigs does not swallow them before tonemap;
// - ambient radiance: plain mean;
// - normal and depth: taken from the nearest sample, so GTAO sees one
//   consistent surface per pixel (no AO halos along silhouettes).
// Infinite reverse-Z: larger depth is nearer; the sky clears to 0.

@group(0) @binding(0) var ms_direct: texture_multisampled_2d<f32>;
@group(0) @binding(1) var ms_normal: texture_multisampled_2d<f32>;
@group(0) @binding(2) var ms_ambient: texture_multisampled_2d<f32>;
@group(0) @binding(3) var ms_depth: texture_depth_multisampled_2d;

struct ResolveOut {
    @location(0) direct: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) ambient: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fs_resolve(input: FullscreenOut) -> ResolveOut {
    let ip = vec2<i32>(input.position.xy);
    let count = i32(textureNumSamples(ms_direct));
    var direct = vec4<f32>(0.0);
    var weight = 0.0;
    var ambient = vec4<f32>(0.0);
    var nearest = 0;
    var depth = textureLoad(ms_depth, ip, 0);
    for (var s = 0; s < count; s += 1) {
        let c = textureLoad(ms_direct, ip, s);
        let w = 1.0 / (1.0 + luminance(max(c.rgb, vec3<f32>(0.0))));
        direct += c * w;
        weight += w;
        ambient += textureLoad(ms_ambient, ip, s);
        let d = textureLoad(ms_depth, ip, s);
        if d > depth {
            depth = d;
            nearest = s;
        }
    }
    var out: ResolveOut;
    out.direct = direct / max(weight, 1.0e-6);
    out.ambient = ambient / f32(count);
    out.normal = textureLoad(ms_normal, ip, nearest);
    out.depth = depth;
    return out;
}
