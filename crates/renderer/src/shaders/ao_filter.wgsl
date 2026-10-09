// AO denoise and upsample shared by the composite and the AO-only view.
// GTAO uses a 4×4 ordered noise pattern; any 4×4 window holds each pattern
// cell exactly once, so a uniform 4×4 average cancels it. A depth-aware
// weight keeps silhouettes. Expects `post`, `depth_tex` and `ao_tex`.

fn filtered_ao(ip: vec2<i32>) -> f32 {
    let d = textureLoad(depth_tex, ip, 0);
    let depth = select(1.0e30, post.camera.x / d, d > 0.0);
    let scale = post.size.zw / post.size.xy;
    let ao_pos = (vec2<f32>(ip) + vec2<f32>(0.5)) * scale - vec2<f32>(0.5);
    let base = vec2<i32>(floor(ao_pos)) - vec2<i32>(1);
    let limit = vec2<i32>(post.size.zw) - vec2<i32>(1);
    var sum = 0.0;
    var weights = 0.0;
    for (var y = 0; y < 4; y++) {
        for (var x = 0; x < 4; x++) {
            let t = clamp(base + vec2<i32>(x, y), vec2<i32>(0), limit);
            let s = textureLoad(ao_tex, t, 0).xy;
            let range = exp(-abs(s.y - depth) / max(depth * 0.02, 1.0e-3));
            let w = select(range, 0.0, s.y <= 0.0);
            sum += s.x * w;
            weights += w;
        }
    }
    return select(1.0, sum / weights, weights > 1.0e-5);
}
