// Adds ambient × AO into the direct HDR target (additive blend). AO is
// upsampled and denoised by `filtered_ao` (ao_filter.wgsl).

@group(0) @binding(0) var<uniform> post: Post;
@group(0) @binding(1) var depth_tex: texture_depth_2d;
@group(0) @binding(2) var ambient_tex: texture_2d<f32>;
@group(0) @binding(3) var ao_tex: texture_2d<f32>;

@fragment
fn fs_composite(input: FullscreenOut) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(input.position.xy);
    let ambient = textureLoad(ambient_tex, ip, 0).rgb;
    if all(ambient <= vec3<f32>(0.0)) {
        return vec4<f32>(0.0);
    }
    if post.ao.w < 0.5 {
        return vec4<f32>(ambient, 0.0);
    }
    return vec4<f32>(ambient * filtered_ao(ip), 0.0);
}
