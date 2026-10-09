// Observer-relative body spheres, lit by the shared scene lighting (lighting.wgsl).
struct Projection { matrix: mat4x4<f32> }
// color: linear albedo (lit) or chromaticity (emitter); flags: unlit, selected, BRDF;
// emission: emitter radiance in cd/m².
struct Draw { color: vec4<f32>, flags: vec4<u32>, emission: vec4<f32> }
@group(0) @binding(0) var<uniform> projection: Projection;
@group(1) @binding(0) var<uniform> draw: Draw;
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) view_pos: vec3<f32>,
}
@vertex fn vs_main(@location(0) view_position: vec4<f32>, @location(1) view_normal: vec4<f32>) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = projection.matrix * view_position;
    output.normal = view_normal.xyz;
    output.view_pos = view_position.xyz;
    return output;
}
@fragment fn fs_main(input: VertexOutput) -> SceneOut {
    let n = normalize(input.normal);
    let mode = view_mode();
    // Modes 13.. are terrain field overlays (M1); spheres show flat colour.
    let debug = (mode != VIEW_LIT && mode != VIEW_SHADOWS && mode != VIEW_UNLIT && mode < 10u) || mode >= 13u;
    if debug || draw.flags.x != 0u {
        var out: SceneOut;
        out.normal = encode_normal(n);
        out.ambient = vec4<f32>(0.0);
        if debug {
            out.direct = vec4<f32>(select(draw.color.rgb, n * 0.5 + vec3<f32>(0.5), mode == 2u), 1.0);
        } else {
            // Emitters are pre-exposed and kept inside the f16 range.
            out.direct = vec4<f32>(min(draw.emission.rgb * exposure.value, vec3<f32>(6.0e4)), 1.0);
        }
        return out;
    }
    return shade(input.view_pos, n, n, draw.color.rgb, f32(draw.flags.z), false);
}
